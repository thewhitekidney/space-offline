//! Planetary interaction: the colony, and the simulation that advances it.
//!
//! # Where this came from, and why not from EvEJS
//!
//! This was briefed as a port of EvEJS's `services/planet/` (11,586 lines,
//! `planetRuntimeStore.js` chief among them). It is not one. **Our own client
//! ships CCP's colony simulation**, as shared client/server Python, and
//! EvEJS's store is a reimplementation of it. So the port is from the
//! original:
//!
//! ```text
//!   eve/common/script/planet/baseColony.pyc      RunSimulation, EvaluatePin,
//!                                                SchedulePin, PrimeSimulation,
//!                                                RouteCommodityOutput/Input,
//!                                                ExecuteRoute, TransferCommodities,
//!                                                the whole Validate* family
//!   eve/common/script/planet/colonyData.pyc      GetSortedRoutesForPin
//!   eve/common/script/planet/entities/*.pyc      the six pin classes
//!   eve/common/script/planet/commandStream.pyc   the 13 opcodes and their args
//!   eve/common/script/planet/surfacePoint.pyc    surface geometry
//!   eve/common/script/util/planetCommon.pyc      the shared constants and the
//!                                                command-centre table
//! ```
//!
//! Every constant and formula below is read out of that bytecode and marked
//! `[V]`. Where EvEJS and the client disagree the client wins; the three
//! disagreements found are recorded at the sites and in
//! `docs/lanes/planet.md`.
//!
//! # Why this file is std-only and knows nothing about the wire
//!
//! The brief's framing -- *"given a colony and a `now`, compute the state"* --
//! is also the client's own: `BaseColony.RunSimulation(runSimUntil)` steps a
//! priority queue of pin events up to a deadline and stops. There is no
//! thread and no ticker. That matters for two of our rules at once:
//!
//! - `docs/reference/world-state.md` §4 rule 4 forbids store I/O on the synchronous
//!   draw path. A simulation that is a pure function of `(colony, now)` does
//!   no I/O at all, so it cannot break the rule, and it needs no lock of its
//!   own.
//! - `server/deps.allow` is empty. The time arithmetic here is integer
//!   FILETIME ticks throughout and **no calendar is needed** -- PI never
//!   formats a date server-side -- so this file does not introduce a second
//!   copy of `ledger::civil_from_filetime`. If one is ever needed, call that.
//!
//! # What is here and what is not
//!
//! Here: pins, links, routes, the extraction program and its decay curve,
//! schematics and production, the 13-opcode command stream the PI window
//! submits, and the validation each command is specified by; since P5.3
//! also `CreateProgram`'s yield arithmetic, the command-centre launch, the
//! expedited transfer, the world-file records and the static file's reader;
//! since W9 also the customs offices -- one per planet, their tax
//! (`SpaceportPin.GetImportTax`/`GetExportTax`) and the launchpad side of an
//! import or export (`docs/lanes/planet.md` §11).
//! Not here: the resource heatmap -- see `docs/lanes/planet.md` §5.
//! The heatmap especially: the client asks for it as a spherical-harmonic
//! byte blob, and inventing one would be inventing CCP's planet surfaces. So
//! the per-head *value* an extractor sees is the one assumed number in this
//! file ([`ASSUMED_FIELD_VALUE_AT_FULL_QUALITY`], **[A]**).

use std::collections::BTreeMap;

use crate::content::{Boxed, Content, Origin, Sources, TableSpec};
use crate::item::ItemType;

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// `const.SEC` -- 100-nanosecond ticks in a second. **[V]** `appConst.pyc`.
///
/// Every time in this file is a blue FILETIME tick count, the same unit
/// `service::filetime_now()` returns, because that is what the client sends
/// and expects: `pin.lastRunTime`, `expiryTime` and `currentSimTime` are all
/// `blue.os.GetWallclockTime()` values.
pub const SEC: i64 = 10_000_000;
/// `const.MIN`. **[V]**
pub const MIN: i64 = 60 * SEC;
/// `const.HOUR`. **[V]**
pub const HOUR: i64 = 60 * MIN;

// ---------------------------------------------------------------------------
// Group ids: which kind of pin a type is
// ---------------------------------------------------------------------------
//
// All six **[V]** from our own client's `inventorycommon/const.pyc`, and all
// six agree with EvEJS `planetStaticData.js:22-38`. 6 of 6, no build delta.
// `planetCommon.GetPinEntityType` is the function that does this mapping and
// it tests exactly these six names.

/// `groupExtractorPins`. The pre-Tyrannis extractor; superseded by the ECU but
/// still a distinct group, and the client still maps it to a pin class.
pub const GROUP_EXTRACTOR_PINS: i64 = 1026;
/// `groupCommandPins`.
pub const GROUP_COMMAND_PINS: i64 = 1027;
/// `groupProcessPins` -- the factories.
pub const GROUP_PROCESS_PINS: i64 = 1028;
/// `groupStoragePins`.
pub const GROUP_STORAGE_PINS: i64 = 1029;
/// `groupSpaceportPins` -- the launchpad.
pub const GROUP_SPACEPORT_PINS: i64 = 1030;
/// `groupExtractionControlUnitPins`.
pub const GROUP_ECU_PINS: i64 = 1063;

/// Dogma attribute ids PI reads.
///
/// **All twenty [V]** from our client's `out/pyc/dogma/const.pyc`, and all
/// twenty agree with EvEJS `planetStaticData.js:40-60`. 20 of 20, no build
/// delta -- which is the strongest single piece of evidence that the PI data
/// model did not move between build 3396210 and 3619879.
pub mod attr {
    pub const POWER_OUTPUT: i64 = 11;
    pub const POWER_LOAD: i64 = 15;
    pub const CPU_OUTPUT: i64 = 48;
    pub const CPU_LOAD: i64 = 49;
    pub const HARVESTER_TYPE: i64 = 709;
    pub const LOGISTICAL_CAPACITY: i64 = 1631;
    pub const PLANET_RESTRICTION: i64 = 1632;
    pub const POWER_LOAD_PER_KM: i64 = 1633;
    pub const CPU_LOAD_PER_KM: i64 = 1634;
    pub const CPU_LOAD_LEVEL_MODIFIER: i64 = 1635;
    pub const POWER_LOAD_LEVEL_MODIFIER: i64 = 1636;
    pub const IMPORT_TAX: i64 = 1638;
    pub const EXPORT_TAX: i64 = 1639;
    pub const IMPORT_TAX_MULTIPLIER: i64 = 1640;
    pub const EXPORT_TAX_MULTIPLIER: i64 = 1641;
    pub const PIN_EXTRACTION_QUANTITY: i64 = 1642;
    pub const PIN_CYCLE_TIME: i64 = 1643;
    pub const EXTRACTOR_DEPLETION_RANGE: i64 = 1644;
    pub const EXTRACTOR_DEPLETION_RATE: i64 = 1645;
    pub const ECU_DECAY_FACTOR: i64 = 1683;
    pub const ECU_MAX_VOLUME: i64 = 1684;
    pub const ECU_OVERLAP_FACTOR: i64 = 1685;
    pub const ECU_NOISE_FACTOR: i64 = 1687;
    pub const ECU_AREA_OF_INFLUENCE: i64 = 1689;
    pub const EXTRACTOR_HEAD_CPU: i64 = 1690;
    pub const EXTRACTOR_HEAD_POWER: i64 = 1691;
}

// ---------------------------------------------------------------------------
// Shared constants, all [V] from planetCommon.pyc / ecuPin.pyc
// ---------------------------------------------------------------------------

/// `planetCommon.LINK_MAX_UPGRADE`. **[V]**
pub const LINK_MAX_UPGRADE: i64 = 10;
/// `planetCommon.MAX_WAYPOINTS` -- waypoints *between* the endpoints, so a
/// legal path is at most `MAX_WAYPOINTS + 2` pins long. **[V]**
pub const MAX_WAYPOINTS: usize = 5;
/// `planetCommon.ECU_MAX_HEADS`. **[V]**
pub const ECU_MAX_HEADS: usize = 10;
/// `planetCommon.RADIUS_DRILLAREAMIN`. **[V]**
pub const RADIUS_DRILLAREAMIN: f64 = 0.01;
/// `planetCommon.RADIUS_DRILLAREAMAX`. **[V]**
pub const RADIUS_DRILLAREAMAX: f64 = 0.05;
/// `ecuPin.EXTRACTOR_MAX_CYCLES`. **[V]**
pub const EXTRACTOR_MAX_CYCLES: i64 = 120;

/// Defaults for the five ECU tuning attributes.
///
/// **These are effectively global constants, not per-type data.** None of the
/// eight ECU types carries a row for any of the five in
/// `res:/staticdata/typedogma.fsdbinary`, so every one of them resolves to the
/// attribute's own `defaultValue` in
/// `res:/staticdata/dogmaattributes.fsdbinary`. A `Statics` that knows the
/// ECU's group and nothing else therefore still simulates correctly if these
/// are used as the fallback -- and getting a decay factor of `0.0` instead,
/// which is what a bare "missing attribute is zero" rule gives, makes
/// extraction never decay at all. That is a silently wrong simulation rather
/// than a visibly broken one, which is why these are written down.
///
/// **[V]** now, values and absence both: `tools/dogma.py` reads
/// `dogmaattributes` through its loader-derived layout and gives
/// `defaultValue` 0.012000000104308128, 0.800000011920929,
/// 9.199999809265137, 0.5 and 0.30000001192092896 -- float32 constants
/// widened to doubles, which is why they are not the round numbers below.
/// `tools/pi_statics.py` writes those exact doubles into every pin type's
/// attributes, so a server started with `--planets` never reads these
/// constants; they remain the fallback for a table without the rows (the
/// fixtures, and [`create_program`] on a type it does not know).
pub mod ecu_default {
    /// `attributeEcuDecayFactor` (1683).
    pub const DECAY_FACTOR: f64 = 0.012;
    /// `attributeEcuNoiseFactor` (1687).
    pub const NOISE_FACTOR: f64 = 0.8;
    /// `attributeEcuMaxVolume` (1684) -- the scalar
    /// `baseColony.CreateProgram` multiplies the summed head values by to get
    /// `qtyPerCycle`.
    pub const MAX_VOLUME: f64 = 9.2;
    /// `attributeEcuOverlapFactor` (1685) -- how much two overlapping heads
    /// penalise each other.
    pub const OVERLAP_FACTOR: f64 = 0.5;
    /// `attributeEcuAreaOfInfluence` (1689) -- the angular radius a head must
    /// sit inside, in radians.
    pub const AREA_OF_INFLUENCE: f64 = 0.3;
}
/// `ecuPin.PROGRAM_INSTALLATION_COOLDOWN = 1 * const.MIN`. **[V]**
///
/// EvEJS writes the same number as `60 * SECOND_TICKS`
/// (`planetRuntimeStore.js:31`); they agree.
pub const PROGRAM_INSTALLATION_COOLDOWN: i64 = MIN;

/// `planetCommon.commandCenterInfoPerLevel`, read straight out of the
/// client's own module constants. **[V]**, six rows.
///
/// `(powerOutput, cpuOutput, upgradeCost)` by level. EvEJS's
/// `COMMAND_CENTER_INFO_BY_LEVEL` has the same six rows with the same
/// eighteen numbers -- 6 of 6 identical -- so this one is corroborated
/// rather than merely ported.
///
/// `upgradeCost` is cumulative from level 0, which is why
/// [`command_centre_upgrade_cost`] subtracts: `planetCommon.GetUpgradeCost`
/// is literally `info[desired].upgradeCost - info[current].upgradeCost`.
pub const COMMAND_CENTRE_LEVELS: [(i64, i64, i64); 6] = [
    (6_000, 1_675, 0),
    (9_000, 7_057, 580_000),
    (12_000, 12_136, 1_510_000),
    (15_000, 17_215, 2_710_000),
    (17_000, 21_315, 4_210_000),
    (19_000, 25_415, 6_310_000),
];

/// The highest level a command centre can reach: `GetMaxCommandUpgradeLevel`
/// is `max(commandCenterInfoPerLevel.keys())`. **[V]**
pub const MAX_COMMAND_CENTRE_LEVEL: i64 = 5;

/// Power and CPU a command centre of this level supplies, or `None` for a
/// level the table does not have -- which is a refusal, not a zero.
pub fn command_centre_output(level: i64) -> Option<(i64, i64)> {
    let row = COMMAND_CENTRE_LEVELS.get(usize::try_from(level).ok()?)?;
    Some((row.0, row.1))
}

/// `planetCommon.GetUpgradeCost(currentLevel, desiredLevel)`. **[V]**
pub fn command_centre_upgrade_cost(current: i64, desired: i64) -> Option<i64> {
    let c = COMMAND_CENTRE_LEVELS.get(usize::try_from(current).ok()?)?;
    let d = COMMAND_CENTRE_LEVELS.get(usize::try_from(desired).ok()?)?;
    Some(d.2 - c.2)
}

// ---------------------------------------------------------------------------
// Static data
// ---------------------------------------------------------------------------

/// One factory recipe. `planetSchematics` in SDE terms.
///
/// `cycle_time` is in **seconds**, as the table stores it; the simulation
/// multiplies by [`SEC`]. Keeping the stored unit rather than normalising on
/// load means a schematic row can be compared against the client's own
/// without a conversion in the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schematic {
    pub schematic_id: i64,
    pub cycle_time: i64,
    /// Which factory types may run it. `SETSCHEMATIC` refuses a pin whose
    /// `typeID` is not in here -- that is `CannotAssignSchematicToPinType`,
    /// and it is the check that stops a basic factory running a P3 recipe.
    pub pin_type_ids: Vec<i64>,
    /// `(typeID, quantity)` consumed per cycle.
    pub inputs: Vec<(i64, i64)>,
    /// `(typeID, quantity)` produced per cycle.
    pub outputs: Vec<(i64, i64)>,
}

/// Everything about one inventory type that PI needs.
///
/// A projection of the client's own type and dogma tables, which is why it
/// belongs on the `static.db` side of `docs/reference/world-state.md` §1: **regenerable,
/// safe to delete.** Nothing in a [`Colony`] is derivable from it and nothing
/// in it is derivable from a colony.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PinType {
    pub type_id: i64,
    pub group_id: i64,
    /// Cubic metres this pin can hold. Only meaningful for a storage-family
    /// pin; `0.0` for the rest, which is also what the client's
    /// `BasePin.GetCapacity` returns for them.
    pub capacity: f64,
    /// Dogma attributes, by id. Read through [`PinType::attr`] so that a
    /// missing attribute is `0.0` in exactly one place.
    pub attributes: BTreeMap<i64, f64>,
}

impl PinType {
    /// One attribute, or `0.0`.
    ///
    /// `0.0` and not an `Option`, because that is what the client does:
    /// `planetCommon.GetCPUAndPowerForPinType` initialises every slot to 0
    /// and overwrites only the attributes the type actually carries. An
    /// `Option` here would push a `unwrap_or(0.0)` to a dozen call sites and
    /// change no answer.
    pub fn attr(&self, id: i64) -> f64 {
        self.attributes.get(&id).copied().unwrap_or(0.0)
    }
}

/// The read-only table the simulation runs against.
///
/// **Deliberately not populated by this module.** Every number in it is CCP's
/// and lives in the client's own static corpus; inventing a schematic or a
/// capacity here would put fabricated CCP data in our source, which is the
/// one thing `docs/reference/framing.md` rules out. It is read at startup from the file
/// `tools/pi_statics.py` writes out of the player's own install into `out/`
/// (gitignored) -- the same shape the owner chose for celestials
/// (`docs/port-tracker.md` §32.4, option 1) -- by [`Statics::parse`], and
/// held in `Content` as [`STATICS`] (CP60), read with [`statics_in`]. Tests
/// run on [`fixture`], made of invented rows. An empty `Statics` is legal and
/// makes every colony read answer "nothing here", which is true.
#[derive(Debug, Clone, Default)]
pub struct Statics {
    pin_types: BTreeMap<i64, PinType>,
    schematics: BTreeMap<i64, Schematic>,
    /// Cubic metres per unit, for commodities. Storage capacity is checked in
    /// volume, not in units, so a commodity with no volume here is treated as
    /// weightless -- see [`Pin::acceptable_quantity`], which says why that is
    /// the client's behaviour and not a shortcut.
    volumes: BTreeMap<i64, f64>,
    /// `types.basePrice`, by pin type: what placing one costs.
    /// `ClientColony.PostValidateCreatePin` adds `evetypes.GetBasePrice` to
    /// the edit session's bill for every pin **except** a command centre,
    /// which is paid for with the item instead. **[V]**
    base_prices: BTreeMap<i64, f64>,
    /// `globaltypes.resourceDistribution`: planet type -> resource type ->
    /// quality in `[0, 1]`. **[V]** table, 8 planet types x 15 resources.
    resources: BTreeMap<i64, BTreeMap<i64, f64>>,
    /// The inventory rows the item store needs for PI: every command centre
    /// (deployed out of a ship's hold) and every commodity (launched into a
    /// hangar). See [`item_types_in`].
    item_rows: Vec<ItemRow>,
    /// `importTaxMultiplier` (attribute 1640) per commodity, the per-type
    /// factor of both customs taxes ([`Statics::import_tax`]). A type with
    /// no row reads the attribute's own default, `1.0` **[V]**
    /// (`dogmaattributes` 1640 `defaultValue`), exactly as the client's
    /// `GetTypeAttribute` does.
    import_tax_multipliers: BTreeMap<i64, f64>,
    /// The customs office this server anchors at every planet, or `None`
    /// when the static file has no `customsoffice` record -- an older
    /// extract -- in which case no office exists and nothing can be
    /// imported, which is what the sandbox had before.
    customs: Option<CustomsType>,
}

/// The one customs-office type the sandbox anchors, as `tools/pi_statics.py`
/// reads it out of the client: the InterBus Customs Office, type 4318,
/// group 1025, category 46 **[V]** (`types.fsdbinary`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CustomsType {
    pub type_id: i64,
    /// `types.capacity`, cubic metres: 35,000 for 4318 **[V]**. What one
    /// character may keep in the office ([`CustomsOffice::capacity`]).
    pub capacity: f64,
    /// `types.radius`, metres: 8,000 **[V]**. The ball's radius.
    pub radius: f64,
    /// `npcCustomsOfficeTaxRate` (attribute 1780) on the type: `0.17` for
    /// 4318 **[V]** (`typedogma`). The rate `GetTaxRate` answers.
    pub tax_rate: f64,
}

/// One inventory type PI hands to the item store. A plain owned row, so this
/// module can carry it without the store's `&'static str` name;
/// [`Installed::build`] is where it becomes an `item::ItemType`.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemRow {
    pub type_id: i64,
    pub group_id: i64,
    pub category_id: i64,
    pub volume: f64,
    pub capacity: f64,
    pub name: String,
}

impl Statics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_pin_type(mut self, t: PinType) -> Self {
        self.pin_types.insert(t.type_id, t);
        self
    }

    pub fn with_schematic(mut self, s: Schematic) -> Self {
        self.schematics.insert(s.schematic_id, s);
        self
    }

    pub fn with_volume(mut self, type_id: i64, volume: f64) -> Self {
        self.volumes.insert(type_id, volume);
        self
    }

    pub fn with_base_price(mut self, type_id: i64, price: f64) -> Self {
        self.base_prices.insert(type_id, price);
        self
    }

    pub fn with_resource(mut self, planet_type_id: i64, resource: i64, quality: f64) -> Self {
        self.resources
            .entry(planet_type_id)
            .or_default()
            .insert(resource, quality);
        self
    }

    pub fn with_customs_type(mut self, t: CustomsType) -> Self {
        self.customs = Some(t);
        self
    }

    pub fn with_import_tax_multiplier(mut self, type_id: i64, multiplier: f64) -> Self {
        self.import_tax_multipliers.insert(type_id, multiplier);
        self
    }

    /// The customs-office type, if the static file named one.
    pub fn customs_type(&self) -> Option<&CustomsType> {
        self.customs.as_ref()
    }

    /// `GetTypeAttribute(typeID, attributeImportTaxMultiplier)`, defaulted
    /// to the attribute's `defaultValue` of `1.0` **[V]**.
    pub fn import_tax_multiplier(&self, type_id: i64) -> f64 {
        self.import_tax_multipliers
            .get(&type_id)
            .copied()
            .unwrap_or(DEFAULT_IMPORT_TAX_MULTIPLIER)
    }

    /// `SpaceportPin.GetImportTax(commodities, taxRate)` **[V]**
    /// (`spaceportPin.pyc:99-107`):
    ///
    /// ```text
    ///     importTaxRate = GetTypeAttribute(self.typeID, attributeImportTax)
    ///     total = sum(qty * importTaxRate * GetTypeAttribute(t, attributeImportTaxMultiplier))
    ///     return total * taxRate
    /// ```
    pub fn import_tax(&self, pin_type_id: i64, commodities: &BTreeMap<i64, i64>, rate: f64) -> f64 {
        self.customs_tax(pin_type_id, attr::IMPORT_TAX, commodities, rate)
    }

    /// `SpaceportPin.GetExportTax` **[V]** (`spaceportPin.pyc:110-118`):
    /// [`Statics::import_tax`] with the pin's `exportTax` -- and **still the
    /// commodity's `importTaxMultiplier`**, not its `exportTaxMultiplier`.
    /// That is the client's code as shipped (line 115 loads
    /// `attributeImportTaxMultiplier`); the window quotes with it, so the
    /// server charges with it, or the quote and the bill would differ.
    pub fn export_tax(&self, pin_type_id: i64, commodities: &BTreeMap<i64, i64>, rate: f64) -> f64 {
        self.customs_tax(pin_type_id, attr::EXPORT_TAX, commodities, rate)
    }

    fn customs_tax(
        &self,
        pin_type_id: i64,
        pin_attr: i64,
        commodities: &BTreeMap<i64, i64>,
        rate: f64,
    ) -> f64 {
        let pin_rate = self.pin_type(pin_type_id).map_or(0.0, |t| t.attr(pin_attr));
        let total: f64 = commodities
            .iter()
            .map(|(type_id, qty)| *qty as f64 * pin_rate * self.import_tax_multiplier(*type_id))
            .sum();
        total * rate
    }

    pub fn with_item_row(mut self, row: ItemRow) -> Self {
        self.volumes.insert(row.type_id, row.volume);
        self.item_rows.push(row);
        self
    }

    /// What placing one pin of this type costs, in ISK. `0.0` for an unknown
    /// type, which can never be placed anyway: [`Colony::validate_create_pin`]
    /// refuses it first.
    pub fn base_price(&self, type_id: i64) -> f64 {
        self.base_prices.get(&type_id).copied().unwrap_or(0.0)
    }

    /// `{resourceTypeID: quality}` for one planet type, qualities in `[0, 1]`.
    /// Empty for a planet type the table does not list -- a gas giant is
    /// listed, a shattered planet is not, and the client blacklists those
    /// for PI anyway (`planetCommon.IsPlanetTypeBlackListed`).
    pub fn resources(&self, planet_type_id: i64) -> Option<&BTreeMap<i64, f64>> {
        self.resources.get(&planet_type_id)
    }

    pub fn item_rows(&self) -> &[ItemRow] {
        &self.item_rows
    }

    /// Read the file `tools/pi_statics.py` writes.
    pub fn load(path: &std::path::Path) -> Result<Statics, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Statics::parse(&text)
    }

    /// The same from text. The format is the tool's docstring's: one
    /// tab-separated record per line, `#` for comments,
    ///
    /// ```text
    ///     pintype    typeID groupID capacity basePrice metaLevel  id=v;id=v...
    ///     schematic  schematicID cycleTimeSeconds  pin,pin  t=q;...  t=q;...
    ///     resource   planetTypeID  t=quality;...
    ///     itemtype   typeID groupID categoryID volume capacity name
    ///     taxmult    typeID importTaxMultiplier
    ///     customsoffice  typeID capacity radius taxRate
    /// ```
    ///
    /// **A malformed line is an error, not a skip**, unlike the celestial
    /// catalogue's leniency: a PI table with a row missing is a colony that
    /// simulates *differently* rather than one that is visibly incomplete --
    /// a dropped schematic input is a factory that runs for free. An unknown
    /// record tag is skipped, so a newer tool can add one.
    pub fn parse(text: &str) -> Result<Statics, String> {
        fn int(field: Option<&&str>, line: usize) -> Result<i64, String> {
            field
                .and_then(|v| v.trim().parse::<i64>().ok())
                .ok_or_else(|| format!("line {line}: expected an integer"))
        }
        fn real(field: Option<&&str>, line: usize) -> Result<f64, String> {
            field
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("line {line}: expected a number"))
        }
        fn pairs<T: std::str::FromStr>(
            field: Option<&&str>,
            line: usize,
        ) -> Result<Vec<(i64, T)>, String> {
            let text = field.copied().unwrap_or("").trim();
            if text.is_empty() {
                return Ok(Vec::new());
            }
            text.split(';')
                .map(|kv| {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("line {line}: expected key=value, got {kv:?}"))?;
                    let k = k
                        .trim()
                        .parse::<i64>()
                        .map_err(|_| format!("line {line}: bad key {k:?}"))?;
                    let v = v
                        .trim()
                        .parse::<T>()
                        .map_err(|_| format!("line {line}: bad value {v:?}"))?;
                    Ok((k, v))
                })
                .collect()
        }
        let mut s = Statics::new();
        for (i, raw) in text.lines().enumerate() {
            let n = i + 1;
            let line = raw.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            match f[0] {
                "pintype" => {
                    let type_id = int(f.get(1), n)?;
                    let attributes = pairs::<f64>(f.get(6), n)?.into_iter().collect();
                    s.base_prices.insert(type_id, real(f.get(4), n)?);
                    s.pin_types.insert(
                        type_id,
                        PinType {
                            type_id,
                            group_id: int(f.get(2), n)?,
                            capacity: real(f.get(3), n)?,
                            attributes,
                        },
                    );
                }
                "schematic" => {
                    let schematic_id = int(f.get(1), n)?;
                    let pin_type_ids = f
                        .get(3)
                        .copied()
                        .unwrap_or("")
                        .split(',')
                        .filter(|p| !p.trim().is_empty())
                        .map(|p| {
                            p.trim()
                                .parse::<i64>()
                                .map_err(|_| format!("line {n}: bad pin type {p:?}"))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    s.schematics.insert(
                        schematic_id,
                        Schematic {
                            schematic_id,
                            cycle_time: int(f.get(2), n)?,
                            pin_type_ids,
                            inputs: pairs::<i64>(f.get(4), n)?,
                            outputs: pairs::<i64>(f.get(5), n)?,
                        },
                    );
                }
                "resource" => {
                    let planet_type_id = int(f.get(1), n)?;
                    let dist = pairs::<f64>(f.get(2), n)?;
                    s.resources
                        .insert(planet_type_id, dist.into_iter().collect());
                }
                "itemtype" => {
                    let row = ItemRow {
                        type_id: int(f.get(1), n)?,
                        group_id: int(f.get(2), n)?,
                        category_id: int(f.get(3), n)?,
                        volume: real(f.get(4), n)?,
                        capacity: real(f.get(5), n)?,
                        name: f.get(6).copied().unwrap_or("").to_owned(),
                    };
                    s = s.with_item_row(row);
                }
                "taxmult" => {
                    s.import_tax_multipliers
                        .insert(int(f.get(1), n)?, real(f.get(2), n)?);
                }
                "customsoffice" => {
                    s.customs = Some(CustomsType {
                        type_id: int(f.get(1), n)?,
                        capacity: real(f.get(2), n)?,
                        radius: real(f.get(3), n)?,
                        tax_rate: real(f.get(4), n)?,
                    });
                }
                _ => continue,
            }
        }
        Ok(s)
    }

    /// `(pin types, schematics, planet types, item types)`, for the startup
    /// line.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.pin_types.len(),
            self.schematics.len(),
            self.resources.len(),
            self.item_rows.len(),
        )
    }

    pub fn pin_type(&self, type_id: i64) -> Option<&PinType> {
        self.pin_types.get(&type_id)
    }

    /// Every pin type, in type-id order.
    pub fn pin_types(&self) -> impl Iterator<Item = &PinType> {
        self.pin_types.values()
    }

    pub fn schematic(&self, schematic_id: i64) -> Option<&Schematic> {
        self.schematics.get(&schematic_id)
    }

    /// Every schematic, in id order (SOC-B6: the PI farmer picks a recipe
    /// by its input).
    pub fn schematics(&self) -> impl Iterator<Item = &Schematic> {
        self.schematics.values()
    }

    pub fn volume(&self, type_id: i64) -> f64 {
        self.volumes.get(&type_id).copied().unwrap_or(0.0)
    }

    /// How many pin types and schematics we know. Used by the service arm to
    /// answer honestly when the static layer has not been loaded.
    pub fn is_empty(&self) -> bool {
        self.pin_types.is_empty() && self.schematics.is_empty()
    }

    /// Which pin class a type is, or `None` for a type that is not a pin.
    ///
    /// `planetCommon.GetPinEntityType` raises `RuntimeError` for a type the
    /// client has never heard of and returns `None` for a known type in none
    /// of the six groups. We cannot tell those apart without the whole type
    /// table, and both are refusals, so both are `None` here.
    pub fn pin_kind(&self, type_id: i64) -> Option<PinKind> {
        PinKind::from_group(self.pin_type(type_id)?.group_id)
    }
}

/// The six pin classes, in the client's own taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PinKind {
    Command,
    Extractor,
    Process,
    Spaceport,
    Storage,
    Ecu,
}

impl PinKind {
    /// `planetCommon.GetPinEntityType`'s group dispatch. **[V]**
    pub fn from_group(group_id: i64) -> Option<Self> {
        match group_id {
            GROUP_COMMAND_PINS => Some(PinKind::Command),
            GROUP_EXTRACTOR_PINS => Some(PinKind::Extractor),
            GROUP_PROCESS_PINS => Some(PinKind::Process),
            GROUP_SPACEPORT_PINS => Some(PinKind::Spaceport),
            GROUP_STORAGE_PINS => Some(PinKind::Storage),
            GROUP_ECU_PINS => Some(PinKind::Ecu),
            _ => None,
        }
    }

    /// `BasePin.IsStorage` -- a pin that holds commodities for other pins.
    ///
    /// **Three classes, not one.** The command centre and the launchpad are
    /// storage as far as routing is concerned, and missing that is how a
    /// colony ends up unable to route anything to its own launchpad.
    pub fn is_storage(self) -> bool {
        matches!(
            self,
            PinKind::Storage | PinKind::Spaceport | PinKind::Command
        )
    }

    /// `ProcessPin.IsProcessor` -- the only destination that consumes.
    pub fn is_processor(self) -> bool {
        matches!(self, PinKind::Process)
    }

    /// `BasePin.IsConsumer`. Same set as [`PinKind::is_processor`] today;
    /// kept as its own name because the client keeps them as two methods and
    /// routes read them at different points.
    pub fn is_consumer(self) -> bool {
        matches!(self, PinKind::Process)
    }

    /// `BasePin.IsProducer` -- a pin whose cycle creates commodities.
    pub fn is_producer(self) -> bool {
        matches!(self, PinKind::Ecu | PinKind::Extractor | PinKind::Process)
    }

    /// `BasePin.IsExtractor`.
    pub fn is_extractor(self) -> bool {
        matches!(self, PinKind::Ecu | PinKind::Extractor)
    }
}

// ---------------------------------------------------------------------------
// Errors, named after the client's own message ids
// ---------------------------------------------------------------------------

/// A refusal, named as the client names it.
///
/// Every variant is a `UserError` message id the client itself raises in
/// `baseColony.Validate*`, so the name is the contract and not a label we
/// chose. The PI window pre-validates locally against the same list, which
/// means a refusal the client did not predict is a bug on one side or the
/// other -- and the name is what makes that legible in a log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiError {
    CannotManagePlanetWithoutCommandCenter,
    CannotBuildMultipleCommandPins,
    CannotCreatePinWrongPlanetType,
    InvalidPinType,
    PinDoesNotExist,
    CanOnlyManageOwnPins,
    LinkAlreadyExists,
    LinkDoesNotExist,
    CannotUpgradeLinkTooHigh,
    CannotLinkPinToItself,
    RouteDoesNotExist,
    CreateRouteWithoutCommodities,
    CreateRouteTooShort,
    CannotRouteTooManyWaypoints,
    RouteFailedValidationPinDoesNotExist,
    RouteFailedValidationPinNotYours,
    RouteFailedValidationLinkDoesNotExist,
    RouteFailedValidationCannotRouteCommodities,
    CannotRouteStorageMustGoToConsumer,
    CannotRouteDestinationWillNotAccept,
    CreateRouteCommodityNotProduced,
    CreateRouteCommodityProductionTooSmall,
    CreateRouteDestinationCannotAcceptCommodity,
    CannotAssignSchematicToPinType,
    SchematicDoesNotExist,
    CannotDowngradeCommandCenter,
    CannotUpgradeCommandCenterPastMaximum,
    CannotAddToColonyCpuUsageExceeded,
    CannotAddToColonyPowerUsageExceeded,
    CannotAddHeadAlreadyExists,
    CannotPlaceHeadLimitReached,
    CannotEditHeadsWhileProgramActive,
    HeadDoesNotExist,
    HeadOutsideAreaOfInfluence,
    SurfacePointOutOfRange,
    CantInstallProgramNeedsCooldown,
    NotAnExtractor,
    // Launches, expedited transfers, and the checks a submit makes outside
    // the colony (the item, the wallet, the skills). Every one is a message
    // id the client raises itself in `clientPlanet.pyc`, `clientColony.pyc`
    // or `baseColony.pyc` **[V]**.
    PleaseSelectCommoditiesToLaunch,
    CannotLaunchWithoutColony,
    CanOnlyLaunchFromCommandCenters,
    CannotLaunchCommandPinNotReady,
    CannotLaunchCommoditiesNotFound,
    RouteFailedValidationExpeditedSourceNotStorage,
    RouteFailedValidationExpeditedSourceLacksCommodity,
    RouteFailedValidationExpeditedSourceLacksCommodityQty,
    RouteFailedValidationExpeditedDestinationCannotAccept,
    RouteFailedValidationNoBandwidthAvailable,
    RouteFailedValidationExpeditedSourceNotReady,
    CannotBuildCommandPin,
    CannotBuildCommandPinWithoutShip,
    CannotPlaceCommandCenterNotEnoughSkill,
    CantUpgradeCommandCenterSkillRequired,
    CannotBuildNotEnoughCash,
    /// Not a client message id: see [`Pin::install_program`] for why we
    /// refuse where the client silently does nothing.
    ProgramQuoteIsZero,
    // **Customs offices.** No client file raises these: they are the
    // server's half of `ImportExportWithPlanet`, and each is a real
    // `dialogs` row in this build (`findings/user-errors.tsv`, ids
    // 3329-3388) **[V]** -- so the name is CCP's, and only the moment we
    // raise it is ours.
    /// 3331.
    CannotImportNoColony,
    /// 3332; the client raises it too (`importExportUI.py:379`).
    CannotImportEndpointNotFound,
    /// 3333.
    CannotImportEndpointNotSpaceport,
    /// 3334.
    CannotImportNotEnoughWarehouseSpace,
    /// 3373.
    CannotImportNotInSameSolarSystem,
    /// 3348.
    CannotExportNoColony,
    /// 3349.
    CannotExportEndpointNotFound,
    /// 3350.
    CannotExportEndpointNotSpaceport,
    /// 3351, `typeName`.
    CannotExportCommodityNotFound {
        type_id: i64,
    },
    /// 3352, `contained`, `desired`, `typeName`.
    CannotExportCommodityNotEnough {
        type_id: i64,
        contained: i64,
        desired: i64,
    },
    /// 3387.
    CannotExportNotEnoughSpace,
    /// 3353.
    CannotExportNotInSameSolarSystem,
    /// 3335; the client raises it first (`importExportUI.py:774`).
    PleaseSelectCommoditiesToImport,
    /// 3304: something that is not a PI commodity
    /// ([`can_put_type_in_customs_office`]) put into a customs office.
    CannotPutInCargolink,
    /// No dialog row: `ConfirmCommodityTransfer` catches it **by
    /// `e.msg`** (`importExportUI.py:788`) and re-quotes at the new rate.
    TaxChanged,
}

impl PiError {
    /// The exact token to put in a `UserError`, which is the variant's own
    /// name. Written out rather than derived so that renaming a variant in
    /// Rust cannot silently change what goes on the wire.
    pub fn message_id(&self) -> &'static str {
        match self {
            PiError::CannotManagePlanetWithoutCommandCenter => {
                "CannotManagePlanetWithoutCommandCenter"
            }
            PiError::CannotBuildMultipleCommandPins => "CannotBuildMultipleCommandPins",
            PiError::CannotCreatePinWrongPlanetType => "CannotCreatePinWrongPlanetType",
            PiError::InvalidPinType => "InvalidPinType",
            PiError::PinDoesNotExist => "PinDoesNotExist",
            PiError::CanOnlyManageOwnPins => "CanOnlyManageOwnPins",
            PiError::LinkAlreadyExists => "LinkAlreadyExists",
            PiError::LinkDoesNotExist => "LinkDoesNotExist",
            PiError::CannotUpgradeLinkTooHigh => "CannotUpgradeLinkTooHigh",
            PiError::CannotLinkPinToItself => "CannotLinkPinToItself",
            PiError::RouteDoesNotExist => "RouteDoesNotExist",
            PiError::CreateRouteWithoutCommodities => "CreateRouteWithoutCommodities",
            PiError::CreateRouteTooShort => "CreateRouteTooShort",
            PiError::CannotRouteTooManyWaypoints => "CannotRouteTooManyWaypoints",
            PiError::RouteFailedValidationPinDoesNotExist => "RouteFailedValidationPinDoesNotExist",
            PiError::RouteFailedValidationPinNotYours => "RouteFailedValidationPinNotYours",
            PiError::RouteFailedValidationLinkDoesNotExist => {
                "RouteFailedValidationLinkDoesNotExist"
            }
            PiError::RouteFailedValidationCannotRouteCommodities => {
                "RouteFailedValidationCannotRouteCommodities"
            }
            PiError::CannotRouteStorageMustGoToConsumer => "CannotRouteStorageMustGoToConsumer",
            PiError::CannotRouteDestinationWillNotAccept => "CannotRouteDestinationWillNotAccept",
            PiError::CreateRouteCommodityNotProduced => "CreateRouteCommodityNotProduced",
            PiError::CreateRouteCommodityProductionTooSmall => {
                "CreateRouteCommodityProductionTooSmall"
            }
            PiError::CreateRouteDestinationCannotAcceptCommodity => {
                "CreateRouteDestinationCannotAcceptCommodity"
            }
            PiError::CannotAssignSchematicToPinType => "CannotAssignSchematicToPinType",
            PiError::SchematicDoesNotExist => "SchematicDoesNotExist",
            PiError::CannotDowngradeCommandCenter => "CannotDowngradeCommandCenter",
            PiError::CannotUpgradeCommandCenterPastMaximum => {
                "CannotUpgradeCommandCenterPastMaximum"
            }
            PiError::CannotAddToColonyCpuUsageExceeded => "CannotAddToColonyCPUUsageExceeded",
            PiError::CannotAddToColonyPowerUsageExceeded => "CannotAddToColonyPowerUsageExceeded",
            PiError::CannotAddHeadAlreadyExists => "CannotAddHeadAlreadyExists",
            PiError::CannotPlaceHeadLimitReached => "CannotPlaceHeadLimitReached",
            PiError::CannotEditHeadsWhileProgramActive => "CannotEditHeadsWhileProgramActive",
            PiError::HeadDoesNotExist => "HeadDoesNotExist",
            PiError::HeadOutsideAreaOfInfluence => "HeadOutsideAreaOfInfluence",
            PiError::SurfacePointOutOfRange => "SurfacePointOutOfRange",
            PiError::CantInstallProgramNeedsCooldown => "CantInstallProgramNeedsCooldown",
            PiError::NotAnExtractor => "NotAnExtractor",
            PiError::PleaseSelectCommoditiesToLaunch => "PleaseSelectCommoditiesToLaunch",
            PiError::CannotLaunchWithoutColony => "CannotLaunchWithoutColony",
            PiError::CanOnlyLaunchFromCommandCenters => "CanOnlyLaunchFromCommandCenters",
            PiError::CannotLaunchCommandPinNotReady => "CannotLaunchCommandPinNotReady",
            PiError::CannotLaunchCommoditiesNotFound => "CannotLaunchCommoditiesNotFound",
            PiError::RouteFailedValidationExpeditedSourceNotStorage => {
                "RouteFailedValidationExpeditedSourceNotStorage"
            }
            PiError::RouteFailedValidationExpeditedSourceLacksCommodity => {
                "RouteFailedValidationExpeditedSourceLacksCommodity"
            }
            PiError::RouteFailedValidationExpeditedSourceLacksCommodityQty => {
                "RouteFailedValidationExpeditedSourceLacksCommodityQty"
            }
            PiError::RouteFailedValidationExpeditedDestinationCannotAccept => {
                "RouteFailedValidationExpeditedDestinationCannotAccept"
            }
            PiError::RouteFailedValidationNoBandwidthAvailable => {
                "RouteFailedValidationNoBandwidthAvailable"
            }
            PiError::RouteFailedValidationExpeditedSourceNotReady => {
                "RouteFailedValidationExpeditedSourceNotReady"
            }
            PiError::CannotBuildCommandPin => "CannotBuildCommandPin",
            PiError::CannotBuildCommandPinWithoutShip => "CannotBuildCommandPinWithoutShip",
            PiError::CannotPlaceCommandCenterNotEnoughSkill => {
                "CannotPlaceCommandCenterNotEnoughSkill"
            }
            PiError::CantUpgradeCommandCenterSkillRequired => {
                "CantUpgradeCommandCenterSkillRequired"
            }
            PiError::CannotBuildNotEnoughCash => "CannotBuildNotEnoughCash",
            PiError::ProgramQuoteIsZero => "ProgramQuoteIsZero",
            PiError::CannotImportNoColony => "CannotImportNoColony",
            PiError::CannotImportEndpointNotFound => "CannotImportEndpointNotFound",
            PiError::CannotImportEndpointNotSpaceport => "CannotImportEndpointNotSpaceport",
            PiError::CannotImportNotEnoughWarehouseSpace => "CannotImportNotEnoughWarehouseSpace",
            PiError::CannotImportNotInSameSolarSystem => "CannotImportNotInSameSolarSystem",
            PiError::CannotExportNoColony => "CannotExportNoColony",
            PiError::CannotExportEndpointNotFound => "CannotExportEndpointNotFound",
            PiError::CannotExportEndpointNotSpaceport => "CannotExportEndpointNotSpaceport",
            PiError::CannotExportCommodityNotFound { .. } => "CannotExportCommodityNotFound",
            PiError::CannotExportCommodityNotEnough { .. } => "CannotExportCommodityNotEnough",
            PiError::CannotExportNotEnoughSpace => "CannotExportNotEnoughSpace",
            PiError::CannotExportNotInSameSolarSystem => "CannotExportNotInSameSolarSystem",
            PiError::PleaseSelectCommoditiesToImport => "PleaseSelectCommoditiesToImport",
            PiError::CannotPutInCargolink => "CannotPutInCargolink",
            PiError::TaxChanged => "TaxChanged",
        }
    }
}

impl core::fmt::Display for PiError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.message_id())
    }
}

impl std::error::Error for PiError {}

// ---------------------------------------------------------------------------
// Surface geometry
// ---------------------------------------------------------------------------

/// A point on the planet's surface, in the client's own coordinates.
///
/// **These are not latitude and longitude in the everyday sense**, despite
/// the field names the client uses on a pin. `SurfacePoint.__init__` takes
/// `theta=longitude, phi=latitude` and `_CalcXYZ` is
///
/// ```text
///     radSinPhi = radius * sin(phi)
///     x = radSinPhi * cos(theta)
///     z = radSinPhi * sin(theta)
///     y = radius * cos(phi)
/// ```
///
/// so `phi` ("latitude") is a **polar angle measured from the pole**, in
/// `[0, pi]`, and `theta` ("longitude") is in `[0, 2pi]`. A port that treated
/// latitude as signed from the equator would put every pin in the wrong
/// hemisphere and get every link length wrong, which is why this is spelled
/// out rather than left to the field name. **[V]** `surfacePoint.pyc`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfacePoint {
    /// `phi`, the polar angle. The client calls it latitude.
    pub latitude: f64,
    /// `theta`. The client calls it longitude.
    pub longitude: f64,
}

impl SurfacePoint {
    pub fn new(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
        }
    }

    /// `SurfacePoint.GetAngleBetween` -- the great-circle angle, in radians.
    ///
    /// Computed through the unit Cartesian dot product exactly as the client
    /// does, rather than through the haversine form, because the client's
    /// clamp is asymmetric and that asymmetry is observable: it clamps
    /// `dotProduct > 1.0` down to `1.0` and does **not** clamp the lower end.
    /// We clamp both, because `acos` of anything below `-1.0` is a `NaN` here
    /// where in Python it is a `ValueError`, and a `NaN` length would
    /// silently poison every CPU and bandwidth sum downstream. The lower
    /// clamp can only ever bite on floating-point noise at exactly antipodal
    /// points, where `acos(-1.0) = pi` is also the right answer.
    pub fn angle_to(self, other: SurfacePoint) -> f64 {
        let (x1, y1, z1) = self.unit_xyz();
        let (x2, y2, z2) = other.unit_xyz();
        let dot = (x1 * x2 + y1 * y2 + z1 * z2).clamp(-1.0, 1.0);
        dot.acos()
    }

    /// `SurfacePoint.GetDistanceToOther` with the planet's radius: the arc
    /// length in whatever unit the radius is in. The client's planet radius
    /// is metres, so this is metres.
    pub fn distance_to(self, other: SurfacePoint, planet_radius: f64) -> f64 {
        if planet_radius <= 0.0 {
            return 0.0;
        }
        planet_radius * self.angle_to(other)
    }

    fn unit_xyz(self) -> (f64, f64, f64) {
        let sin_phi = self.latitude.sin();
        (
            sin_phi * self.longitude.cos(),
            self.latitude.cos(),
            sin_phi * self.longitude.sin(),
        )
    }

    /// The point as `BasePin.SetLatitude`/`SetLongitude` would store it.
    /// **[V]**
    ///
    /// ```text
    ///     self.latitude  = min(math.pi, max(-math.pi, latitude))
    ///     self.longitude = abs(longitude % (2 * math.pi))
    /// ```
    ///
    /// **The client normalises on store rather than refusing**, and this is
    /// the first thing a port gets wrong twice over:
    ///
    /// - Latitude is clamped to `[-pi, pi]`, so it is **signed**. A port that
    ///   assumed the `[0, pi]` polar range `SurfacePoint` uses would reject
    ///   half the sphere.
    /// - `longitude % (2 * pi)` is **Python's** modulo, which is
    ///   non-negative for a negative left operand. Rust's `%` is not, so
    ///   this is `rem_euclid`. The client's outer `abs()` is then redundant,
    ///   and reproducing it with Rust's `%` would mirror a negative longitude
    ///   into the wrong hemisphere.
    pub fn normalised(self) -> Self {
        if !self.latitude.is_finite() || !self.longitude.is_finite() {
            // The client has no guard here; a NaN would propagate into every
            // link length and make every CPU check pass by comparison with
            // NaN being false. Zero is the colony's own pole and is the least
            // wrong finite answer.
            return SurfacePoint::new(0.0, 0.0);
        }
        SurfacePoint::new(
            self.latitude
                .clamp(-std::f64::consts::PI, std::f64::consts::PI),
            self.longitude.rem_euclid(std::f64::consts::TAU),
        )
    }

    /// Whether the client could have sent this point at all.
    ///
    /// Only finiteness: the ranges are enforced by [`SurfacePoint::normalised`]
    /// because that is what the client does.
    pub fn is_in_range(self) -> bool {
        self.latitude.is_finite() && self.longitude.is_finite()
    }
}

// ---------------------------------------------------------------------------
// The extraction program
// ---------------------------------------------------------------------------

/// `planetCommon.GetProgramLengthFromHeadRadius`. **[V]**
///
/// ```text
///     return (headRadius - RADIUS_DRILLAREAMIN) / RADIUS_DRILLAREADIFF * 335 + 1
/// ```
///
/// **In hours.** `RADIUS_DRILLAREADIFF` is `0.04`, so the minimum radius
/// gives a one-hour program and the maximum gives 336 hours -- two weeks, the
/// longest extraction EVE offers. `baseColony.CreateProgram:1428` feeds it
/// straight into [`cycle_time_from_program_length`], and
/// `surveyUI.py:310 _GetCycleTimeAndNumCyclesFromSliderVal` uses the same two
/// functions, which is why the slider the player drags and the program the
/// server installs agree without a round trip.
///
/// **The client does not clamp the radius here; EvEJS does**
/// (`planetRuntimeStore.js:2929`). Ours does not either, so that a radius the
/// client would have accepted produces the number the client would have
/// produced. The clamp belongs at the point the radius enters the colony --
/// [`Pin::install_program`] -- not in the formula.
pub fn program_length_from_head_radius(head_radius: f64) -> f64 {
    (head_radius - RADIUS_DRILLAREAMIN) / (RADIUS_DRILLAREAMAX - RADIUS_DRILLAREAMIN) * 335.0 + 1.0
}

/// `planetCommon.GetCycleTimeFromProgramLength`, in **hours**. **[V]**
///
/// ```text
///     return 0.25 * 2 ** max(0, math.floor(math.log(programLength / 25.0, 2)) + 1)
/// ```
///
/// So a short program cycles every 15 minutes and each doubling of the
/// program's length doubles the cycle: 0.25, 0.5, 1, 2, 4 hours. The `max(0,
/// ...)` is what pins the floor at 0.25h for every program shorter than 25.
pub fn cycle_time_from_program_length(program_length: f64) -> f64 {
    let exponent = (program_length / 25.0).log2().floor() + 1.0;
    0.25 * 2f64.powf(exponent.max(0.0))
}

/// `EcuPin.GetProgramOutput(currentTime, baseValue, cycleTime, startTime)`.
/// **[V]**, transcribed from `ecuPin.pyc` offset 375 and reproduced here
/// because it is the one formula in PI a player watches directly: the survey
/// window draws it as a bar chart before they commit, so a divergent curve is
/// visible on the first click.
///
/// ```text
///     timeDiff  = currentTime - startTime
///     cycleNum  = max((timeDiff + const.SEC) / cycleTime - 1, 0)
///     barWidth  = cycleTime / const.SEC / 900.0
///     t         = (cycleNum + 0.5) * barWidth
///     decayValue = baseValue / (1 + t * decayFactor)
///     phaseShift = baseValue ** 0.7
///     sinStuff  = max(0, (cos(phaseShift + t/12) + cos(phaseShift/2 + t/5) + cos(t/2)) / 3)
///     barHeight = decayValue * (1 + noiseFactor * sinStuff)
///     return int(barWidth * barHeight)
/// ```
///
/// Two Python-2 details a Rust port gets wrong by default, and both change
/// the answer:
///
/// - **`(timeDiff + SEC) / cycleTime` is integer division**, because both
///   operands are tick counts. Written as `div_euclid` here. Using float
///   division and flooring afterwards agrees for non-negative `timeDiff` and
///   disagrees for a negative one, which happens whenever a stale
///   `installTime` is ahead of the clock.
/// - **`cycleTime / const.SEC` is also integer division**, so a `cycleTime`
///   that is not a whole number of seconds truncates *before* the division
///   by 900. EvEJS divides in floating point here
///   (`planetRuntimeStore.js:1911`). It cannot matter in practice because
///   every cycle time comes from `cycle_time_from_program_length` and is a
///   whole number of seconds, but the faithful arithmetic is free.
/// - `int(...)` truncates toward zero. `max(0, ...)` on the result is
///   belt-and-braces: `sinStuff` is already clamped at 0 and `decayValue` is
///   non-negative for a non-negative `baseValue`.
///
/// EvEJS's `getEcuProgramOutput` agrees with the client term for term, which
/// is the strongest corroboration available for a formula neither of us
/// invented.
pub fn ecu_program_output(
    current_time: i64,
    start_time: i64,
    base_value: i64,
    cycle_time: i64,
    decay_factor: f64,
    noise_factor: f64,
) -> i64 {
    if base_value <= 0 || cycle_time <= 0 {
        return 0;
    }
    let time_diff = current_time.saturating_sub(start_time);
    // Python 2 `/` on two ints: floor division. `div_euclid` is floor
    // division for a positive divisor, and `cycle_time > 0` here.
    let cycle_num = ((time_diff.saturating_add(SEC)).div_euclid(cycle_time) - 1).max(0);
    // Integer division first, as above, then the float divide by 900.
    let bar_width = (cycle_time / SEC) as f64 / 900.0;
    let t = (cycle_num as f64 + 0.5) * bar_width;
    let decay_value = base_value as f64 / (1.0 + t * decay_factor);
    let phase_shift = (base_value as f64).powf(0.7);
    let sin_stuff =
        ((phase_shift + t / 12.0).cos() + (phase_shift / 2.0 + t / 5.0).cos() + (t / 2.0).cos())
            / 3.0;
    let sin_stuff = sin_stuff.max(0.0);
    let bar_height = decay_value * (1.0 + noise_factor * sin_stuff);
    let out = bar_width * bar_height;
    if !out.is_finite() {
        return 0;
    }
    (out.trunc() as i64).max(0)
}

/// `EcuPin.GetMaxOutput(baseOutput, cycleTime)`. **[V]**
///
/// ```text
///     scalar = self.GetAttribute(attributeEcuNoiseFactor) + 1
///     return int(scalar * baseOutput) * cycleTime / const.SEC / 900.0
/// ```
///
/// This is **not** the production figure -- it is the cap a route from an
/// extractor may reserve, which is why `GetProductMaxOutput` and not
/// `GetProducts` is what route validation reads. The noisiest possible cycle,
/// so a route sized to it never over-commits.
///
/// Note where `int()` falls: on `scalar * baseOutput`, *before* the
/// multiplication by the cycle time. Moving it to the end would round
/// differently for every noise factor that is not a whole number, and every
/// real one is not.
pub fn ecu_max_output(base_output: i64, cycle_time: i64, noise_factor: f64) -> i64 {
    if base_output <= 0 || cycle_time <= 0 {
        return 0;
    }
    let scalar = noise_factor + 1.0;
    let scaled = (scalar * base_output as f64).trunc() as i64;
    // `int(...) * cycleTime` is exact integer arithmetic; `/ const.SEC` is
    // integer division again, and only the final `/ 900.0` is float.
    let per_cycle = scaled.saturating_mul(cycle_time) / SEC;
    ((per_cycle as f64) / 900.0).trunc() as i64
}

// ---------------------------------------------------------------------------
// Pins
// ---------------------------------------------------------------------------

/// `basePin.STATE_*`. **[V]**, all four, from `basePin.pyc`'s module
/// constants.
///
/// EvEJS carries only `STATE_IDLE` and `STATE_ACTIVE`
/// (`planetRuntimeStore.js:24-25`) and drops the negative two. We keep them:
/// `state` goes on the wire as an integer the client compares against its own
/// four names, and `STATE_EDITMODE` is what the PI window puts a pin into
/// while the player is dragging it. A server that cannot represent `-2` cannot
/// round-trip a colony the client sent.
pub mod state {
    pub const EDITMODE: i64 = -2;
    pub const DISABLED: i64 = -1;
    pub const IDLE: i64 = 0;
    pub const ACTIVE: i64 = 1;
}

/// One extractor head: `(headID, latitude, longitude)` on the wire.
///
/// A tuple and not a struct on the wire, in that order -- `ecuPin.Serialize`
/// sends it as a list of 3-sequences and `SetHeads` reads `head[0..2]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Head {
    pub head_id: i64,
    pub latitude: f64,
    pub longitude: f64,
}

impl Head {
    pub fn point(&self) -> SurfacePoint {
        SurfacePoint::new(self.latitude, self.longitude)
    }
}

/// One pin in a colony.
///
/// **A single struct rather than six, deliberately.** The client has six pin
/// classes and a type hierarchy; we have one struct with the class-specific
/// fields as `Option`s and a [`PinKind`] derived from the type's group. The
/// reason is the wire: `baseColony.Serialize` sends a *flat* keyval per pin
/// whose field set depends on the class, the client reconstructs the class
/// from `typeID`, and every consumer here already has to ask "what kind is
/// this" before touching a field. Six Rust types would mean six match arms at
/// every one of those sites and a seventh for "the type id says process but
/// the row has ECU fields", which is a real thing a stored colony can say.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub pin_id: i64,
    pub type_id: i64,
    pub owner_id: i64,
    pub latitude: f64,
    pub longitude: f64,
    /// When this pin last completed a cycle. Never `None` on a live pin: the
    /// client's `RecalculateCurrentSimTime` takes the **minimum** of these
    /// over the non-storage pins to recover a lost `currentSimTime`, so a
    /// missing one is how a colony loses its place in time.
    pub last_run_time: i64,
    /// `typeID -> quantity`. A `BTreeMap` and not a `HashMap` because the
    /// serialised order reaches the client and a stable order makes a capture
    /// diff readable.
    pub contents: BTreeMap<i64, i64>,
    pub state: i64,

    // --- command centre and launchpad ---
    /// Last time commodities were launched from here. `0` means never, which
    /// is what the client sends; `CommandCenterContainer._UpdateInfoCont`
    /// compares `nextLaunchTime` against the wall clock, so the distinction
    /// between "never" and "just now" is load-bearing in the UI.
    pub last_launch_time: i64,

    // --- factory ---
    pub schematic_id: Option<i64>,
    /// `ProcessPin` initialises both of these to **true**, which is why a
    /// restored pin that omits them must come back as `true` and not as
    /// `Default` -- a never-primed factory never starts. EvEJS hit this and
    /// wrote the fix down at `planetRuntimeStore.js:1254-1266`; ours is the
    /// `Default` impl below.
    pub has_received_inputs: bool,
    pub received_inputs_last_cycle: bool,

    // --- extraction control unit ---
    pub cycle_time: i64,
    pub program_type: Option<i64>,
    pub qty_per_cycle: i64,
    pub expiry_time: Option<i64>,
    pub install_time: Option<i64>,
    pub head_radius: f64,
    pub heads: Vec<Head>,

    /// **Lane clock**: the game clock's pace for this pin's timers. Not
    /// state (never saved): [`Colonies`] sets it on every pin before each
    /// pass. A cycle lasts `pace.span(cycle_time)` of wall time, an ECU's
    /// decay curve is read at `pace.elapsed` of the wall time since its
    /// install, and a program's expiry is `num_cycles` paced cycles after
    /// its install. `cycle_time` itself, the yield of a cycle and the
    /// route bandwidths stay nominal, so a game day of extraction yields a
    /// game day's ore in `1/N` of the wall time.
    pub pace: crate::game_clock::GameClock,
}

impl Pin {
    /// A freshly placed pin, as `CREATEPIN` makes one.
    pub fn new(pin_id: i64, type_id: i64, owner_id: i64, point: SurfacePoint, now: i64) -> Self {
        let point = point.normalised();
        Pin {
            pin_id,
            type_id,
            owner_id,
            latitude: point.latitude,
            longitude: point.longitude,
            last_run_time: now,
            contents: BTreeMap::new(),
            state: state::IDLE,
            last_launch_time: 0,
            schematic_id: None,
            // True, for the reason on the field.
            has_received_inputs: true,
            received_inputs_last_cycle: true,
            cycle_time: 0,
            program_type: None,
            qty_per_cycle: 0,
            expiry_time: None,
            install_time: None,
            head_radius: RADIUS_DRILLAREAMIN,
            heads: Vec::new(),
            pace: crate::game_clock::GameClock::REAL,
        }
    }

    pub fn point(&self) -> SurfacePoint {
        SurfacePoint::new(self.latitude, self.longitude)
    }

    pub fn kind(&self, statics: &Statics) -> Option<PinKind> {
        statics.pin_kind(self.type_id)
    }

    /// `BasePin.IsActive` is `activityState > STATE_IDLE`; **`EcuPin`
    /// overrides it** to also require a program. **[V]**
    ///
    /// `>` and not `== STATE_ACTIVE`, so a hypothetical state above 1 would
    /// also read as active -- which is what the client means by it.
    pub fn is_active(&self, statics: &Statics) -> bool {
        if self.state <= state::IDLE {
            return false;
        }
        match self.kind(statics) {
            Some(PinKind::Ecu) | Some(PinKind::Extractor) => self.program_type.is_some(),
            _ => true,
        }
    }

    pub fn quantity(&self, type_id: i64) -> i64 {
        self.contents.get(&type_id).copied().unwrap_or(0)
    }

    /// Set a quantity, **removing the key at zero**.
    ///
    /// Removing rather than storing `0` is not tidiness: `len(contents) < 1`
    /// is the test `RouteCommodityInput` uses to decide whether a storage pin
    /// has anything to push, and `GetSortedRoutesForPin` skips a commodity
    /// that is not a key at all. A retained zero would make an empty silo
    /// look full of nothing and route forever.
    fn set_quantity(&mut self, type_id: i64, quantity: i64) {
        if quantity > 0 {
            self.contents.insert(type_id, quantity);
        } else {
            self.contents.remove(&type_id);
        }
    }

    /// `StoragePin.GetCapacity`, in cubic metres, or `None` for a pin that is
    /// not storage.
    ///
    /// `None` and not `0.0`: zero capacity and "this pin is not a container"
    /// are different answers, and [`Pin::acceptable_quantity`] branches on
    /// which.
    pub fn capacity(&self, statics: &Statics) -> Option<f64> {
        if !self.kind(statics)?.is_storage() {
            return None;
        }
        Some(statics.pin_type(self.type_id).map_or(0.0, |t| t.capacity))
    }

    /// Volume of what is in the pin.
    pub fn used_volume(&self, statics: &Statics) -> f64 {
        self.contents
            .iter()
            .map(|(type_id, qty)| statics.volume(*type_id) * *qty as f64)
            .sum()
    }

    /// `StoragePin.GetFreeSpace` -- **not clamped at zero**. **[V]**
    ///
    /// The client has two nearly identical methods and they disagree on
    /// purpose, so both are here:
    ///
    /// - `BasePin.GetCapacityRemaining` is `max(0, capacity - capacityUsed)`
    ///   and is what `CanAccept` divides by. That one is
    ///   [`Pin::capacity_remaining`].
    /// - `StoragePin.GetFreeSpace` recomputes the used volume from the
    ///   contents and does **not** clamp, so an over-filled silo reports a
    ///   negative figure.
    ///
    /// The difference is observable because `GetFreeSpace` is the **sort key
    /// for storage routes** in `GetSortedRoutesForPin`: an over-filled silo
    /// sorts ahead of an empty one and is offered goods first. Clamping here
    /// would reorder deliveries in a colony that is already over-full, which
    /// is exactly the colony a player is trying to debug.
    pub fn free_space(&self, statics: &Statics) -> f64 {
        match self.capacity(statics) {
            Some(cap) => cap - self.used_volume(statics),
            None => 0.0,
        }
    }

    /// `BasePin.GetCapacityRemaining` -- clamped at zero.
    pub fn capacity_remaining(&self, statics: &Statics) -> f64 {
        match self.capacity(statics) {
            Some(cap) => (cap - self.used_volume(statics)).max(0.0),
            None => 0.0,
        }
    }

    /// What this pin produces in one cycle, as `typeID -> quantity`.
    ///
    /// `EcuPin.GetProducts` is `{programType: qtyPerCycle}` and
    /// `ProcessPin.GetProducts` is the schematic's outputs. **This is the
    /// production figure, not the route cap** -- [`Pin::product_max_output`]
    /// is the cap, and conflating them is how a route gets sized to the
    /// average instead of the peak and silently drops the surplus.
    pub fn products(&self, statics: &Statics) -> BTreeMap<i64, i64> {
        let mut out = BTreeMap::new();
        match self.kind(statics) {
            Some(PinKind::Ecu) | Some(PinKind::Extractor) => {
                if let Some(program) = self.program_type
                    && self.qty_per_cycle > 0
                {
                    out.insert(program, self.qty_per_cycle);
                }
            }
            Some(PinKind::Process) => {
                if let Some(s) = self.schematic(statics) {
                    for (type_id, qty) in &s.outputs {
                        if *qty > 0 {
                            *out.entry(*type_id).or_insert(0) += *qty;
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// `GetProductMaxOutput` -- the most a cycle can ever yield, which is
    /// what a route from this pin may reserve.
    ///
    /// For an ECU that is [`ecu_max_output`]: the noisiest cycle, not the
    /// nominal one. For a factory it is the same as [`Pin::products`],
    /// because a schematic has no noise.
    pub fn product_max_output(&self, statics: &Statics) -> BTreeMap<i64, i64> {
        match self.kind(statics) {
            Some(PinKind::Ecu) | Some(PinKind::Extractor) => {
                let mut out = BTreeMap::new();
                let Some(program) = self.program_type else {
                    return out;
                };
                let noise = statics
                    .pin_type(self.type_id)
                    .map_or(0.0, |t| t.attr(attr::ECU_NOISE_FACTOR))
                    .max(0.0);
                let qty = ecu_max_output(self.qty_per_cycle, self.cycle_time, noise);
                if qty > 0 {
                    out.insert(program, qty);
                }
                out
            }
            _ => self.products(statics),
        }
    }

    /// `ProcessPin.GetConsumables` -- the schematic's inputs, or empty for
    /// anything that is not a factory.
    pub fn consumables(&self, statics: &Statics) -> BTreeMap<i64, i64> {
        let mut out = BTreeMap::new();
        if self.kind(statics) != Some(PinKind::Process) {
            return out;
        }
        if let Some(s) = self.schematic(statics) {
            for (type_id, qty) in &s.inputs {
                if *qty > 0 {
                    out.insert(*type_id, *qty);
                }
            }
        }
        out
    }

    fn schematic<'a>(&self, statics: &'a Statics) -> Option<&'a Schematic> {
        statics.schematic(self.schematic_id?)
    }

    /// `ProcessPin.GetCycleTime` / `EcuPin.GetCycleTime`, in ticks.
    ///
    /// The factory's comes from its schematic (stored in seconds, so scaled
    /// here); the ECU's is stored on the pin, because it was fixed when the
    /// program was installed and the schematic table has nothing to say about
    /// it. Everything else has no cycle at all.
    pub fn cycle_time(&self, statics: &Statics) -> i64 {
        match self.kind(statics) {
            Some(PinKind::Ecu) | Some(PinKind::Extractor) => self.cycle_time.max(0),
            Some(PinKind::Process) => self
                .schematic(statics)
                .map_or(0, |s| (s.cycle_time.saturating_mul(SEC)).max(1)),
            _ => 0,
        }
    }

    /// `ProcessPin.GetInputBufferState` -- the priority a factory gets in
    /// `GetSortedRoutesForPin`.
    ///
    /// ```text
    ///     1 - (sum(have[i] / want[i] for i in inputs) / len(inputs))
    /// ```
    ///
    /// So **lower is fuller**, and the client pushes into a min-heap, which
    /// means the *emptiest* factory is served first. Getting the sign wrong
    /// here starves one factory in a pair and is invisible until the colony
    /// has two.
    pub fn input_buffer_state(&self, statics: &Statics) -> f64 {
        let Some(s) = self.schematic(statics) else {
            return 1.0;
        };
        if s.inputs.is_empty() {
            return 1.0;
        }
        let filled: f64 = s
            .inputs
            .iter()
            .map(|(type_id, want)| self.quantity(*type_id) as f64 / *want as f64)
            .sum();
        1.0 - filled / s.inputs.len() as f64
    }

    /// How much of `type_id` this pin will take, given it was offered
    /// `desired`.
    ///
    /// Two completely different rules, which is the whole reason this is one
    /// function:
    ///
    /// - A **factory** takes only what its schematic still wants, capped at
    ///   one batch. `BasePin.CanAccept` on a `ProcessPin` is
    ///   `demand - have`, so a factory never stockpiles two batches of one
    ///   input; it is the routing that keeps one batch in reserve.
    /// - A **storage** pin takes whatever fits by volume.
    ///
    /// A commodity with no volume in [`Statics`] is accepted without limit.
    /// That is the client's behaviour, not a shortcut: `GetCapacity` is in
    /// cubic metres and a zero-volume type contributes nothing to the sum, so
    /// an unbounded number of them fits. It is also the honest answer while
    /// the volume table is incomplete -- refusing instead would make a
    /// missing static row look like a full silo.
    pub fn acceptable_quantity(&self, statics: &Statics, type_id: i64, desired: i64) -> i64 {
        // `BasePin.CanAccept`'s first line: a pin that is disabled or in edit
        // mode takes nothing, whatever kind it is.
        if self.state < state::IDLE || desired <= 0 {
            return 0;
        }
        match self.kind(statics) {
            Some(PinKind::Process) => {
                // `ProcessPin.CanAccept`: bounded by **one cycle's demand**,
                // not by volume. A factory never stockpiles two batches of an
                // input; keeping one in reserve is the routing's job.
                let want = self
                    .consumables(statics)
                    .get(&type_id)
                    .copied()
                    .unwrap_or(0);
                if want <= 0 {
                    return 0;
                }
                // The client returns `demand - have` unclamped, so it can be
                // negative for an over-filled buffer, and `_AddCommodity`
                // then rejects anything `< 1`. Clamping at zero here reaches
                // the same decision by the same path and keeps the type
                // honest.
                desired.min((want - self.quantity(type_id)).max(0))
            }
            Some(k) if k.is_storage() => {
                let volume = statics.volume(type_id);
                if volume <= 0.0 {
                    // `BasePin.CanAccept` divides by this unguarded, so a
                    // zero-volume commodity is a `ZeroDivisionError` on
                    // Tranquility. We accept without limit instead, which is
                    // the same answer the volume arithmetic gives in the
                    // limit and is also the honest answer while the volume
                    // table is incomplete: refusing would make a missing
                    // static row look like a full silo.
                    return desired;
                }
                // `int(capacityRemaining / volume)` -- `capacityRemaining` is
                // the **clamped** figure, and `int()` truncates toward zero.
                let fits = (self.capacity_remaining(statics) / volume).trunc();
                if fits <= 0.0 {
                    0
                } else {
                    desired.min(fits as i64)
                }
            }
            // `EcuPin.CanAccept` returns 0 unconditionally -- an extractor is
            // not a container. So does an unknown type: a pin whose type is
            // not in `Statics` is one we cannot reason about, and silently
            // accepting into it would lose commodities.
            _ => 0,
        }
    }

    /// `BasePin.AddCommodity` -- returns how much was actually taken.
    ///
    /// The return value is the contract: `TransferCommodities` removes from
    /// the source only what the destination took, so a short accept leaves
    /// the remainder where it was instead of destroying it.
    pub fn add_commodity(&mut self, statics: &Statics, type_id: i64, quantity: i64) -> i64 {
        let accepted = self.acceptable_quantity(statics, type_id, quantity);
        if accepted <= 0 {
            return 0;
        }
        self.set_quantity(type_id, self.quantity(type_id) + accepted);
        if self.kind(statics) == Some(PinKind::Process) {
            self.has_received_inputs = true;
        }
        accepted
    }

    /// `BasePin.RemoveCommodity` -- returns how much was actually removed.
    pub fn remove_commodity(&mut self, type_id: i64, quantity: i64) -> i64 {
        let removed = self.quantity(type_id).min(quantity.max(0));
        if removed <= 0 {
            return 0;
        }
        self.set_quantity(type_id, self.quantity(type_id) - removed);
        removed
    }

    /// `ProcessPin.HasEnoughInputs`.
    fn has_enough_inputs(&self, statics: &Statics) -> bool {
        let Some(s) = self.schematic(statics) else {
            return false;
        };
        !s.inputs.is_empty()
            && s.inputs
                .iter()
                .all(|(type_id, want)| self.quantity(*type_id) >= *want)
    }

    /// `ProcessPin.CanActivate`, `EcuPin.CanActivate`.
    ///
    /// For a factory: a full batch is in the buffer. For an ECU: a program is
    /// installed and its quote is positive -- `EcuPin.InstallProgram` logs an
    /// error and refuses to activate a zero `qtyPerCycle`, so a colony
    /// restored with one must come back idle rather than cycling nothing
    /// forever.
    pub fn can_activate(&self, statics: &Statics) -> bool {
        match self.kind(statics) {
            // `ProcessPin.CanActivate` **[V]**:
            //
            // ```text
            //     if self.activityState < STATE_IDLE: return False
            //     if self.schematicID is None: return False
            //     if self.IsActive(): return True
            //     if self.hasReceivedInputs or self.receivedInputsLastCycle: return True
            //     if not self.HasEnoughInputs(): return False
            //     return True
            // ```
            //
            // The two flags short-circuit **before** the input check, which
            // is why a factory that was just delivered into is activatable
            // even while its buffer is still short: it is being given a
            // chance to be evaluated, and `Run` decides whether it can
            // actually consume. Collapsing this to `has_enough_inputs` -- as
            // an obvious reading would -- stops a half-fed factory ever being
            // looked at again.
            Some(PinKind::Process) => {
                self.state >= state::IDLE
                    && self.schematic_id.is_some()
                    && (self.is_active(statics)
                        || self.has_received_inputs
                        || self.received_inputs_last_cycle
                        || self.has_enough_inputs(statics))
            }
            // `EcuPin.CanActivate` **[V]**: `activityState < STATE_ACTIVE`
            // returns False -- note **ACTIVE (1)**, where every other pin
            // compares against IDLE (0). So an ECU that has gone idle can
            // never be revived by the simulation; only a fresh
            // `InstallProgram` sets it active again. That is the mechanism
            // that makes an expired program stay expired.
            Some(PinKind::Ecu) | Some(PinKind::Extractor) => {
                self.state >= state::ACTIVE && self.program_type.is_some()
            }
            // `StoragePin.CanActivate` is a hard `False`, and the command
            // centre and launchpad inherit it. Storage pins never cycle; they
            // only transfer.
            _ => false,
        }
    }

    /// `BasePin.GetNextRunTime` -- when this pin wants to be evaluated next,
    /// or `None` for a pin with no cycle.
    ///
    /// An ECU past its expiry has no next run: the program is spent and the
    /// pin goes idle. A factory that is not active has no next run either --
    /// it is woken by a delivery, through `SchedulePin`, not by a timer.
    pub fn next_run_time(&self, statics: &Statics) -> Option<i64> {
        // `ProcessPin.GetNextRunTime` **[V]**:
        //
        // ```text
        //     if not self.IsActive() and self.HasEnoughInputs(): return None
        //     return BasePin.GetNextRunTime(self)
        // ```
        //
        // **This is the kick-start path, and `None` here means "now".** An
        // idle factory that has just been given a full batch returns `None`,
        // `can_run` reads `None` as runnable, and it fires on the very next
        // pass instead of waiting out a cycle it is not running. A port that
        // read `None` as "never" -- the natural reading of `Option` -- would
        // give every factory a one-cycle stall after every delivery, and the
        // colony would still work, just slower, which is the kind of bug
        // nobody finds.
        if self.kind(statics) == Some(PinKind::Process)
            && !self.is_active(statics)
            && self.has_enough_inputs(statics)
        {
            return None;
        }
        // `BasePin.GetNextRunTime` is `lastRunTime + GetCycleTime()`, wrapped
        // in a `try` that swallows a `None` cycle time and returns `None`.
        // A factory with no schematic and a cleared ECU both land there.
        let cycle = self.cycle_time(statics);
        if cycle <= 0 {
            return None;
        }
        // Lane clock: a cycle lasts `pace.span(cycle)` of wall time.
        Some(self.last_run_time.saturating_add(self.pace.span(cycle)))
    }

    /// `BasePin.CanRun(runTime)`, with `ProcessPin` and `StoragePin`'s
    /// overrides. **[V]**
    ///
    /// ```text
    ///     # BasePin
    ///     if not self.CanActivate(): return False
    ///     nextRunTime = self.GetNextRunTime()
    ///     return nextRunTime is None or nextRunTime <= rt
    ///
    ///     # ProcessPin
    ///     if not self.IsActive() and not self.CanActivate(): return False
    ///     ... same tail ...
    ///
    ///     # StoragePin
    ///     return False
    /// ```
    ///
    /// Two things to keep:
    ///
    /// - **`<=`, not `<`.** A cycle landing exactly on the deadline runs.
    ///   EvEJS admits an event only on `nextRunTime < targetTime`
    ///   (`planetRuntimeStore.js:2385, :2397`) and so drops it. One cycle,
    ///   every time the deadline happens to fall on a cycle boundary -- which
    ///   for a colony primed at a cycle boundary is *every* pass.
    /// - **`None` is runnable.** See [`Pin::next_run_time`].
    pub fn can_run(&self, statics: &Statics, sim_end_time: i64) -> bool {
        match self.kind(statics) {
            // `StoragePin.CanRun` is a hard `False`. It matters: a storage
            // pin's cycle time is literally `0`, so its inherited
            // `GetNextRunTime` is `lastRunTime + 0` -- always due -- and
            // without this override every silo in the colony would be
            // evaluated on every tick forever.
            Some(k) if k.is_storage() => false,
            Some(PinKind::Process) => {
                if !self.is_active(statics) && !self.can_activate(statics) {
                    return false;
                }
                match self.next_run_time(statics) {
                    None => true,
                    Some(next) => next <= sim_end_time,
                }
            }
            Some(_) => {
                if !self.can_activate(statics) {
                    return false;
                }
                match self.next_run_time(statics) {
                    None => true,
                    Some(next) => next <= sim_end_time,
                }
            }
            None => false,
        }
    }

    /// `BasePin.Run(runTime)` -- advance this pin one cycle and return what
    /// it produced.
    ///
    /// The ECU and the factory differ in where the cycle's cost is paid:
    ///
    /// - The **ECU** pays nothing. It yields [`ecu_program_output`] for the
    ///   cycle number implied by `runTime`, and goes idle once `runTime`
    ///   reaches the expiry.
    /// - The **factory** pays on *activation*, not on completion: the inputs
    ///   were consumed when the pin went active a cycle ago, so this call
    ///   emits the outputs and then tries to consume the next batch. If there
    ///   is one it stays active; if not it goes idle and waits for a
    ///   delivery. That ordering is why a factory with a full input route
    ///   never misses a cycle and one without it loses exactly one.
    pub fn run(&mut self, statics: &Statics, run_time: i64) -> BTreeMap<i64, i64> {
        let mut products = BTreeMap::new();
        match self.kind(statics) {
            // `EcuPin.Run` **[V]**:
            //
            // ```text
            //     self.lastRunTime = runTime
            //     if self.programType is None: return {}
            //     products = {}
            //     if self.IsActive():
            //         products[self.programType] = self.GetProgramOutput(runTime)
            //         if self.expiryTime <= runTime:
            //             self.SetState(STATE_IDLE)
            //     return products
            // ```
            //
            // Two orderings that are the whole behaviour:
            //
            // - **`lastRunTime` is written first and unconditionally**, even
            //   for a pin with no program that returns nothing. So reading a
            //   dormant ECU still advances its clock, and a program installed
            //   later starts from now rather than from whenever it last ran.
            // - **The expiry check is `<=` and comes *after* the output is
            //   banked.** The cycle on which a program expires still pays
            //   out. EvEJS returns empty and idles when `runTime > expiryTime`
            //   (`planetRuntimeStore.js:1856`), which loses the last cycle of
            //   every program. Over a 336-hour program that is one cycle in
            //   84; over a 1-hour program it is one in four.
            //
            // Note there is **no expiry cap on `GetNextRunTime`** either --
            // expiry is enforced by this state change alone, and
            // `EcuPin.CanActivate`'s `state >= STATE_ACTIVE` is what then
            // keeps the pin out of the queue for good.
            Some(PinKind::Ecu) | Some(PinKind::Extractor) => {
                self.last_run_time = run_time;
                let Some(program) = self.program_type else {
                    return products;
                };
                if !self.is_active(statics) {
                    return products;
                }
                let decay = statics
                    .pin_type(self.type_id)
                    .map_or(0.0, |t| t.attr(attr::ECU_DECAY_FACTOR));
                let noise = statics
                    .pin_type(self.type_id)
                    .map_or(0.0, |t| t.attr(attr::ECU_NOISE_FACTOR));
                // `installTime` is the curve's origin, not `lastRunTime`: the
                // decay is measured from when the program started, so a pin
                // read late must not restart its curve.
                let start = self.install_time.unwrap_or(self.last_run_time);
                // Lane clock: the curve is read at the game time since the
                // install, so cycle k yields cycle k's quantity at any pace.
                let game_time = start.saturating_add(self.pace.elapsed(run_time - start));
                let qty = ecu_program_output(
                    game_time,
                    start,
                    self.qty_per_cycle,
                    self.cycle_time,
                    decay,
                    noise,
                );
                if qty > 0 {
                    products.insert(program, qty);
                }
                if self.expiry_time.is_some_and(|expiry| expiry <= run_time) {
                    self.state = state::IDLE;
                }
            }
            // `ProcessPin.Run` **[V]**: produce-then-consume, with exactly
            // one cycle of latency.
            //
            // ```text
            //     products = self.products.copy() if self.IsActive() else {}
            //     canConsume = all(contents[t] >= demands[t] for t in demands)
            //     if canConsume:
            //         for t, q in demands: self.RemoveCommodity(t, q)
            //         self.SetState(STATE_ACTIVE)
            //     else:
            //         self.SetState(STATE_IDLE)
            //     self.receivedInputsLastCycle = self.hasReceivedInputs
            //     self.hasReceivedInputs = False
            //     self.lastRunTime = runTime
            //     return products
            // ```
            //
            // The output belongs to the cycle that is *ending* -- it is paid
            // because the pin was already active on entry -- and the inputs
            // consumed here are for the cycle that is *starting*. That is why
            // a factory with a reliable input route never misses a cycle and
            // one without it loses exactly one and then idles.
            //
            // The flag rotation happens **after** the state decision and
            // before the return, and `lastRunTime` is last. Unlike
            // `BasePin.Run` there is no `min(state, STATE_IDLE)` clamp here.
            Some(PinKind::Process) => {
                if self.is_active(statics)
                    && let Some(s) = self.schematic(statics)
                {
                    for (type_id, qty) in &s.outputs {
                        if *qty > 0 {
                            *products.entry(*type_id).or_insert(0) += *qty;
                        }
                    }
                }
                if self.consume_inputs(statics) {
                    self.state = state::ACTIVE;
                } else {
                    self.state = state::IDLE;
                }
                self.received_inputs_last_cycle = self.has_received_inputs;
                self.has_received_inputs = false;
                self.last_run_time = run_time;
            }
            // `BasePin.Run` sets `lastRunTime` and clamps the state down with
            // `min(activityState, STATE_IDLE)`, returning nothing. A storage
            // pin never reaches here -- `CanRun` is a hard `False` -- so this
            // is the unknown-type case, and advancing the clock without
            // producing is the honest answer.
            _ => {
                self.last_run_time = run_time;
                self.state = self.state.min(state::IDLE);
            }
        }
        products
    }

    /// Take one batch out of the buffer, all or nothing.
    fn consume_inputs(&mut self, statics: &Statics) -> bool {
        if !self.has_enough_inputs(statics) {
            return false;
        }
        let inputs: Vec<(i64, i64)> = self
            .schematic(statics)
            .map(|s| s.inputs.clone())
            .unwrap_or_default();
        for (type_id, qty) in inputs {
            self.remove_commodity(type_id, qty);
        }
        true
    }

    /// `ProcessPin.InstallSchematic` -- set the recipe and keep what of the
    /// buffer the new recipe can still use.
    ///
    /// The retention rule is the client's: for each input of the *new*
    /// schematic keep `min(have, want)`, and discard everything else. So
    /// switching a factory from one P1 to another does not silently hand the
    /// player a free batch, and does not throw away a stack the new recipe
    /// wants.
    pub fn install_schematic(
        &mut self,
        statics: &Statics,
        schematic_id: i64,
    ) -> Result<(), PiError> {
        if self.kind(statics) != Some(PinKind::Process) {
            return Err(PiError::CannotAssignSchematicToPinType);
        }
        let schematic = statics
            .schematic(schematic_id)
            .ok_or(PiError::SchematicDoesNotExist)?;
        if !schematic.pin_type_ids.contains(&self.type_id) {
            return Err(PiError::CannotAssignSchematicToPinType);
        }
        let mut retained = BTreeMap::new();
        for (type_id, want) in &schematic.inputs {
            let keep = self.quantity(*type_id).min(*want);
            if keep > 0 {
                retained.insert(*type_id, keep);
            }
        }
        self.schematic_id = Some(schematic.schematic_id);
        self.contents = retained;
        self.state = state::IDLE;
        // `ClientColony.InstallSchematicForPin` stimulates the pin after
        // replacing its schematic, so the next simulation pass will try to
        // start it. Setting the flag rather than activating here keeps the
        // "a pin only starts with a full batch" rule in one place.
        self.has_received_inputs = true;
        Ok(())
    }

    /// `EcuPin.InstallProgram`.
    ///
    /// `program_type` of `None` is Stop Program and is always allowed; it is
    /// the path the survey window uses before the player edits heads, because
    /// heads are locked while a program runs.
    ///
    /// The cooldown is `EcuPin.CanInstallProgram`: one minute from the last
    /// install. It is checked against `now` and not against `last_run_time`,
    /// because the client's `GetNextEditTime` is
    /// `installTime + PROGRAM_INSTALLATION_COOLDOWN`.
    pub fn install_program(
        &mut self,
        statics: &Statics,
        program_type: Option<i64>,
        head_radius: f64,
        qty_per_cycle: i64,
        now: i64,
    ) -> Result<(), PiError> {
        let kind = self.kind(statics).ok_or(PiError::InvalidPinType)?;
        if !kind.is_extractor() {
            return Err(PiError::NotAnExtractor);
        }
        if let Some(installed) = self.install_time
            && self.program_type.is_some()
            && now < installed.saturating_add(PROGRAM_INSTALLATION_COOLDOWN)
        {
            return Err(PiError::CantInstallProgramNeedsCooldown);
        }
        if !(RADIUS_DRILLAREAMIN..=RADIUS_DRILLAREAMAX).contains(&head_radius)
            && program_type.is_some()
        {
            // `baseColony.ValidateInstallProgram:1188` raises
            // `RuntimeError('Cannot install a program with a completely
            // bonkers radius')` for a radius outside [0.01, 0.05]. EvEJS
            // clamps instead (`planetRuntimeStore.js:2929`). Refusing is the
            // client's answer and the safe one: a clamped radius quietly
            // installs a program of a different length than the one the
            // player was quoted.
            return Err(PiError::SurfacePointOutOfRange);
        }
        let Some(program) = program_type else {
            // `EcuPin.ClearProgram`: everything the program owned goes back
            // to nothing, and the heads stay so that Restart Extractors can
            // try the same layout again.
            self.program_type = None;
            self.qty_per_cycle = 0;
            self.expiry_time = None;
            self.install_time = None;
            self.state = state::IDLE;
            return Ok(());
        };
        if qty_per_cycle <= 0 {
            // `EcuPin.InstallProgram` logs an error and **returns without
            // installing anything** -- its message about "defaulting to
            // EXTRACTOR_MAX_CYCLES cycles" is copy-pasted from
            // `extractorPin.InstallDeposit`, which really does default; the
            // ECU does not. We refuse instead of no-opping, because a caller
            // that cannot tell the difference leaves a pin the player thinks
            // is running. The client's own survey window will not offer a
            // zero quote, so this refusal is one it never provokes.
            return Err(PiError::ProgramQuoteIsZero);
        }
        let radius = head_radius;
        let length = program_length_from_head_radius(radius);
        let cycle_hours = cycle_time_from_program_length(length);
        let cycle_time = ((cycle_hours * HOUR as f64).trunc() as i64).max(1);
        // `int(length / cycleTimeHours)` cycles, capped at
        // EXTRACTOR_MAX_CYCLES.
        let num_cycles = ((length / cycle_hours).trunc() as i64).clamp(1, EXTRACTOR_MAX_CYCLES);
        self.head_radius = radius;
        self.program_type = Some(program);
        self.qty_per_cycle = qty_per_cycle.max(0);
        self.cycle_time = cycle_time;
        self.install_time = Some(now);
        self.last_run_time = now;
        // Lane clock: `num_cycles` paced cycles, so the last one runs at
        // the expiry exactly, at any pace.
        self.expiry_time =
            Some(now.saturating_add(self.pace.span(cycle_time).saturating_mul(num_cycles)));
        self.state = state::ACTIVE;
        Ok(())
    }

    /// `EcuPin.GetAreaOfInfluence` -- the angular radius inside which a head
    /// must sit, as an `attributeEcuAreaOfInfluence` on the ECU's type.
    pub fn area_of_influence(&self, statics: &Statics) -> f64 {
        statics
            .pin_type(self.type_id)
            .map_or(0.0, |t| t.attr(attr::ECU_AREA_OF_INFLUENCE))
    }

    /// `EcuPin.AddHead`, with the three refusals the client raises.
    pub fn add_head(&mut self, statics: &Statics, head: Head) -> Result<(), PiError> {
        if !self.kind(statics).is_some_and(PinKind::is_extractor) {
            return Err(PiError::NotAnExtractor);
        }
        if !head.point().is_in_range() {
            return Err(PiError::SurfacePointOutOfRange);
        }
        let head = Head {
            head_id: head.head_id,
            latitude: head.point().normalised().latitude,
            longitude: head.point().normalised().longitude,
        };
        if self.heads.iter().any(|h| h.head_id == head.head_id) {
            return Err(PiError::CannotAddHeadAlreadyExists);
        }
        if self.heads.len() >= ECU_MAX_HEADS {
            return Err(PiError::CannotPlaceHeadLimitReached);
        }
        // Heads are locked while the program runs: the yield was quoted for
        // this layout, so changing it mid-program would make the quote a lie.
        if self.state > state::IDLE {
            return Err(PiError::CannotEditHeadsWhileProgramActive);
        }
        let influence = self.area_of_influence(statics);
        if influence > 0.0 && self.point().angle_to(head.point()) > influence {
            return Err(PiError::HeadOutsideAreaOfInfluence);
        }
        self.heads.push(head);
        self.heads.sort_by_key(|h| h.head_id);
        Ok(())
    }

    /// `EcuPin.RemoveHead`.
    pub fn remove_head(&mut self, statics: &Statics, head_id: i64) -> Result<(), PiError> {
        if !self.kind(statics).is_some_and(PinKind::is_extractor) {
            return Err(PiError::NotAnExtractor);
        }
        if self.state > state::IDLE {
            return Err(PiError::CannotEditHeadsWhileProgramActive);
        }
        let before = self.heads.len();
        self.heads.retain(|h| h.head_id != head_id);
        if self.heads.len() == before {
            return Err(PiError::HeadDoesNotExist);
        }
        Ok(())
    }

    /// `EcuPin.SetHeadPosition`.
    pub fn move_head(
        &mut self,
        statics: &Statics,
        head_id: i64,
        point: SurfacePoint,
    ) -> Result<(), PiError> {
        if !self.kind(statics).is_some_and(PinKind::is_extractor) {
            return Err(PiError::NotAnExtractor);
        }
        if !point.is_in_range() {
            return Err(PiError::SurfacePointOutOfRange);
        }
        let point = point.normalised();
        if self.state > state::IDLE {
            return Err(PiError::CannotEditHeadsWhileProgramActive);
        }
        let influence = self.area_of_influence(statics);
        if influence > 0.0 && self.point().angle_to(point) > influence {
            return Err(PiError::HeadOutsideAreaOfInfluence);
        }
        let head = self
            .heads
            .iter_mut()
            .find(|h| h.head_id == head_id)
            .ok_or(PiError::HeadDoesNotExist)?;
        head.latitude = point.latitude;
        head.longitude = point.longitude;
        Ok(())
    }

    /// `GetNextAvailableHeadID` -- the lowest id not in use.
    pub fn next_head_id(&self) -> i64 {
        let mut candidate = 0;
        while self.heads.iter().any(|h| h.head_id == candidate) {
            candidate += 1;
        }
        candidate
    }

    /// CPU and power this pin draws, `(cpu, power)`.
    ///
    /// `planetCommon.GetCPUAndPowerForPinType` plus, **for an ECU only**, one
    /// `attributeExtractorHeadCPU`/`Power` per placed head. A port that
    /// forgot the per-head term would let a player fit ten heads for the
    /// price of none, which is the single biggest balance lever in PI.
    pub fn cpu_power(&self, statics: &Statics) -> (f64, f64) {
        let Some(t) = statics.pin_type(self.type_id) else {
            return (0.0, 0.0);
        };
        let mut cpu = t.attr(attr::CPU_LOAD);
        let mut power = t.attr(attr::POWER_LOAD);
        if self.kind(statics).is_some_and(PinKind::is_extractor) {
            let heads = self.heads.len() as f64;
            cpu += t.attr(attr::EXTRACTOR_HEAD_CPU).max(0.0) * heads;
            power += t.attr(attr::EXTRACTOR_HEAD_POWER).max(0.0) * heads;
        }
        (cpu, power)
    }
}

// ---------------------------------------------------------------------------
// Links and routes
// ---------------------------------------------------------------------------

/// One link between two pins.
///
/// The endpoints are stored **sorted**, because `baseColony.GetLink` looks a
/// link up by an unordered pair and a colony with both `(a,b)` and `(b,a)`
/// would answer "no such link" to half the lookups and "already exists" to
/// the other half.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub endpoint1: i64,
    pub endpoint2: i64,
    pub type_id: i64,
    pub level: i64,
}

impl Link {
    pub fn new(a: i64, b: i64, type_id: i64, level: i64) -> Self {
        let (endpoint1, endpoint2) = if a <= b { (a, b) } else { (b, a) };
        Link {
            endpoint1,
            endpoint2,
            type_id,
            level,
        }
    }

    pub fn connects(&self, a: i64, b: i64) -> bool {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        self.endpoint1 == lo && self.endpoint2 == hi
    }

    /// Cubic metres per hour this link can carry.
    ///
    /// `logisticalCapacity * 2 ** level` -- every upgrade doubles it. The
    /// attribute is on the link *type*, which is why a colony needs the
    /// static table to answer a bandwidth question at all.
    pub fn bandwidth(&self, statics: &Statics) -> f64 {
        let capacity = statics
            .pin_type(self.type_id)
            .map_or(0.0, |t| t.attr(attr::LOGISTICAL_CAPACITY))
            .max(0.0);
        capacity * 2f64.powi(self.level.clamp(0, 62) as i32)
    }

    /// `planetCommon.GetCpuUsageForLink(typeID, length, level)`. **[V]**
    ///
    /// ```text
    ///     baseCpuUsage + int(ceil(cpuUsagePerKm * (length / 1000.0) * (level + 1) ** cpuUsageLevelModifier))
    /// ```
    ///
    /// Note `length` is metres and the `/1000.0` makes it kilometres, and
    /// that the per-km term is rounded **up** on its own before being added
    /// to the base. Rounding the total instead is off by up to one per link.
    pub fn cpu_usage(&self, statics: &Statics, length_metres: f64) -> f64 {
        self.usage(
            statics,
            length_metres,
            attr::CPU_LOAD,
            attr::CPU_LOAD_PER_KM,
            attr::CPU_LOAD_LEVEL_MODIFIER,
        )
    }

    /// `planetCommon.GetPowerUsageForLink`. Same shape as the CPU one.
    pub fn power_usage(&self, statics: &Statics, length_metres: f64) -> f64 {
        self.usage(
            statics,
            length_metres,
            attr::POWER_LOAD,
            attr::POWER_LOAD_PER_KM,
            attr::POWER_LOAD_LEVEL_MODIFIER,
        )
    }

    fn usage(
        &self,
        statics: &Statics,
        length_metres: f64,
        base_attr: i64,
        per_km_attr: i64,
        level_modifier_attr: i64,
    ) -> f64 {
        let Some(t) = statics.pin_type(self.type_id) else {
            return 0.0;
        };
        let base = t.attr(base_attr).trunc();
        let per_km = t.attr(per_km_attr);
        let modifier = t.attr(level_modifier_attr);
        let level = self.level.max(0) as f64;
        base + (per_km * (length_metres / 1000.0) * (level + 1.0).powf(modifier)).ceil()
    }
}

/// One route: a commodity, a quantity per cycle, and the path it takes.
///
/// The path is the whole path, endpoints included, so the shortest legal one
/// has two entries. Every consecutive pair must be an existing link, which is
/// `RouteFailedValidationLinkDoesNotExist` and the reason a route survives a
/// link removal only by being deleted with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub route_id: i64,
    pub owner_id: i64,
    pub path: Vec<i64>,
    pub commodity_type_id: i64,
    pub commodity_quantity: i64,
}

impl Route {
    pub fn source(&self) -> Option<i64> {
        self.path.first().copied()
    }

    pub fn destination(&self) -> Option<i64> {
        self.path.last().copied()
    }
}

// ---------------------------------------------------------------------------
// The colony
// ---------------------------------------------------------------------------

/// One player's whole installation on one planet.
#[derive(Debug, Clone, PartialEq)]
pub struct Colony {
    pub planet_id: i64,
    pub owner_id: i64,
    pub planet_type_id: i64,
    /// Metres. Only used for link lengths, and a radius of zero makes every
    /// link free -- which is why [`Colony::cpu_power`] is only meaningful
    /// once the planet's real radius is in here. The client sends it as a
    /// Python **float** in `GetPlanetInfo` and rejects an int outright
    /// (`eveplanet/verification.py::_VerifyDiamater`), noted by EvEJS at
    /// `planetMgrService.js:393`; that is a wire concern, handled in
    /// `service/planet.rs`.
    pub planet_radius: f64,
    /// Command-centre level, 0..=5.
    pub level: i64,
    /// How far the simulation has been advanced.
    pub current_sim_time: i64,
    pub pins: Vec<Pin>,
    pub links: Vec<Link>,
    pub routes: Vec<Route>,
}

/// What one [`Colony::run_simulation`] pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimReport {
    /// How many pin cycles were evaluated.
    pub events: usize,
    /// Pin ids whose contents or state changed, so the arm knows which
    /// `OnRefreshPins` to send without diffing the whole colony.
    pub touched: Vec<i64>,
    /// True if the event cap stopped the pass before the deadline.
    ///
    /// The client has no cap -- it runs inside a locked service and trusts
    /// itself. We need one, because `run_simulation` is reachable from a
    /// client call: a colony left alone for a year with a 15-minute cycle is
    /// 35,000 events, and a malformed one with a zero-length cycle would not
    /// terminate at all. When this is set the colony's `current_sim_time` is
    /// left where the pass stopped, so the next call continues rather than
    /// skipping the gap.
    pub capped: bool,
}

/// How many pin cycles one pass will evaluate.
///
/// EvEJS uses 20,000 (`planetRuntimeStore.js:30`) and logs a warning at the
/// cap. Ours is the same number for the same reason, and the honest handling
/// is in [`SimReport::capped`]: a capped pass is resumable, not lost.
pub const MAX_SIMULATION_EVENTS: usize = 20_000;

impl Colony {
    pub fn new(planet_id: i64, owner_id: i64, planet_type_id: i64, now: i64) -> Self {
        Colony {
            planet_id,
            owner_id,
            planet_type_id,
            planet_radius: 0.0,
            level: 0,
            current_sim_time: now,
            pins: Vec::new(),
            links: Vec::new(),
            routes: Vec::new(),
        }
    }

    /// Lane clock: run every pin at `pace` ([`Pin::pace`]).
    pub fn set_pace(&mut self, pace: crate::game_clock::GameClock) {
        for p in &mut self.pins {
            p.pace = pace;
        }
    }

    pub fn pin(&self, pin_id: i64) -> Option<&Pin> {
        self.pins.iter().find(|p| p.pin_id == pin_id)
    }

    pub fn pin_mut(&mut self, pin_id: i64) -> Option<&mut Pin> {
        self.pins.iter_mut().find(|p| p.pin_id == pin_id)
    }

    fn pin_index(&self, pin_id: i64) -> Option<usize> {
        self.pins.iter().position(|p| p.pin_id == pin_id)
    }

    pub fn command_pin(&self, statics: &Statics) -> Option<&Pin> {
        self.pins
            .iter()
            .find(|p| p.kind(statics) == Some(PinKind::Command))
    }

    pub fn link(&self, a: i64, b: i64) -> Option<&Link> {
        self.links.iter().find(|l| l.connects(a, b))
    }

    pub fn route(&self, route_id: i64) -> Option<&Route> {
        self.routes.iter().find(|r| r.route_id == route_id)
    }

    /// Metres between two pins along the surface.
    pub fn pin_distance(&self, a: i64, b: i64) -> f64 {
        match (self.pin(a), self.pin(b)) {
            (Some(pa), Some(pb)) => pa.point().distance_to(pb.point(), self.planet_radius),
            _ => 0.0,
        }
    }

    /// `(cpu_supply, cpu_usage, power_supply, power_usage)`.
    ///
    /// The supply is the command centre's, and it is **zero without a command
    /// centre** rather than the level-0 figure: a colony with no command pin
    /// has no power plant, and answering 6000 would let the player build on a
    /// planet they have not landed on. The command pin itself draws nothing;
    /// every other pin and every link does.
    pub fn cpu_power(&self, statics: &Statics) -> (i64, i64, i64, i64) {
        let has_command = self.command_pin(statics).is_some();
        let (power_supply, cpu_supply) = if has_command {
            command_centre_output(self.level.clamp(0, MAX_COMMAND_CENTRE_LEVEL)).unwrap_or((0, 0))
        } else {
            (0, 0)
        };
        let mut cpu = 0.0;
        let mut power = 0.0;
        for p in &self.pins {
            if p.kind(statics) == Some(PinKind::Command) {
                continue;
            }
            let (c, w) = p.cpu_power(statics);
            cpu += c;
            power += w;
        }
        for l in &self.links {
            let length = self.pin_distance(l.endpoint1, l.endpoint2);
            cpu += l.cpu_usage(statics, length);
            power += l.power_usage(statics, length);
        }
        (
            cpu_supply,
            cpu.ceil() as i64,
            power_supply,
            power.ceil() as i64,
        )
    }

    /// Cubic metres per hour a route needs on every link it crosses.
    ///
    /// The cycle time it is measured against is the **source's**, unless the
    /// source is a plain storage pin -- which has no cycle -- in which case
    /// it is the destination's. That asymmetry is the client's
    /// (`baseRoute`/`planetCommon.GetBandwidth` with
    /// `commodityVolume * HOUR / cycleTime`) and it is what makes a
    /// silo-to-factory route cost bandwidth at the factory's rate.
    pub fn route_bandwidth(&self, statics: &Statics, route: &Route) -> f64 {
        let volume = statics.volume(route.commodity_type_id) * route.commodity_quantity as f64;
        let source = route.source().and_then(|id| self.pin(id));
        let dest = route.destination().and_then(|id| self.pin(id));
        let source_cycle = source.map_or(0, |p| p.cycle_time(statics));
        let dest_cycle = dest.map_or(0, |p| p.cycle_time(statics));
        let cycle =
            if source_cycle > 0 && source.and_then(|p| p.kind(statics)) != Some(PinKind::Storage) {
                source_cycle
            } else if dest_cycle > 0 {
                dest_cycle
            } else {
                source_cycle.max(dest_cycle)
            };
        if cycle <= 0 {
            return volume;
        }
        volume * (HOUR as f64 / cycle as f64)
    }

    /// `planetCommon.GetExpeditedTransferTime`. **[V]**
    ///
    /// ```text
    ///     long(ceil(max(5 * const.MIN, float(volume) / linkBandwidth * const.HOUR)))
    /// ```
    ///
    /// Five minutes minimum, however small the shipment. EvEJS agrees
    /// (`planetRuntimeStore.js:35`).
    pub fn expedited_transfer_time(link_bandwidth: f64, volume: f64) -> i64 {
        let floor = 5 * MIN;
        if link_bandwidth <= 0.0 {
            return floor;
        }
        let needed = (volume / link_bandwidth * HOUR as f64).ceil();
        if !needed.is_finite() {
            return floor;
        }
        (needed as i64).max(floor)
    }
}

// ---------------------------------------------------------------------------
// The simulation
// ---------------------------------------------------------------------------

/// One aggregated delivery, ready to be made: the heap's key followed by the
/// destination, the commodity and the quantity.
///
/// `(priority, destination, typeID, quantity)`, where the priority is
/// `GetInputBufferState` for a factory and `GetFreeSpace` for a silo -- both
/// min-heap keys, so the emptiest goes first. The client pushes the same
/// shape onto a `heapq` as `(priority, (destID, typeID, qty))`; it is flat
/// here because Rust sorts a tuple of four as happily as a nested pair.
type SortedRoute = (f64, i64, i64, i64);

/// A scheduled pin evaluation. Ordered by time, then by pin id.
///
/// The ordering is not a tidiness choice: the client pushes
/// `(runTime, pinID)` **tuples** onto a `heapq`, so Python compares the pin
/// id when two events land on the same tick, and two factories fed by one
/// silo are served lowest-id-first. A different tiebreak gives a different
/// colony, so this derives `Ord` on the fields in that order and nothing
/// else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SimEvent {
    run_time: i64,
    pin_id: i64,
}

impl Colony {
    /// `BaseColony.RecalculateCurrentSimTime`. **[V]**
    ///
    /// The **minimum** `lastRunTime` over the non-storage pins, because that
    /// is the earliest point from which any cycle still has to be replayed.
    /// A storage pin's `lastRunTime` is meaningless -- it never cycles -- and
    /// including it would drag the clock back to whenever the silo was last
    /// delivered into.
    ///
    /// Falls back to `now` for an empty colony, which is the client's
    /// behaviour and not a guess: `len(pins) < 1` short-circuits to
    /// `blue.os.GetWallclockTime()`.
    pub fn recalculate_current_sim_time(&mut self, statics: &Statics, now: i64) {
        if self.pins.is_empty() {
            self.current_sim_time = now;
            return;
        }
        let mut earliest: Option<i64> = None;
        for p in &self.pins {
            if p.kind(statics).is_some_and(PinKind::is_storage) {
                continue;
            }
            earliest = Some(match earliest {
                Some(t) => t.min(p.last_run_time),
                None => p.last_run_time,
            });
        }
        self.current_sim_time = earliest.unwrap_or(now);
    }

    /// `BaseColony.PrimeSimulation` -- queue every pin that could run before
    /// the deadline.
    fn prime_simulation(&self, statics: &Statics, sim_end_time: i64) -> Vec<SimEvent> {
        let mut queue = Vec::new();
        for p in &self.pins {
            if p.can_run(statics, sim_end_time) {
                self.schedule_pin(statics, p, &mut queue);
            }
        }
        queue
    }

    /// `BaseColony.SchedulePin`. **[V]**
    ///
    /// ```text
    ///     nextRunTime = pin.GetNextRunTime()
    ///     for (evalTime, evalPinID) in self.simQueue:
    ///         if evalPinID == pin.id:
    ///             if nextRunTime is None or nextRunTime < evalTime:
    ///                 self.simQueue.remove((evalTime, evalPinID))
    ///             else:
    ///                 return
    ///     if nextRunTime is None or nextRunTime < self.currentSimTime:
    ///         self.AddTimer(pin.id, self.currentSimTime)
    ///     else:
    ///         self.AddTimer(pin.id, nextRunTime)
    /// ```
    ///
    /// Two things worth not losing:
    ///
    /// - An existing entry for this pin is **replaced only if the new time is
    ///   earlier**, and otherwise the call does nothing. That is what makes
    ///   `SchedulePin` idempotent and keeps a delivery from pushing a second
    ///   event for a factory that is already queued.
    /// - A pin with **no** next run time is still queued, at
    ///   `currentSimTime`. That is how an idle factory that just received a
    ///   delivery gets evaluated at once instead of waiting for a cycle it
    ///   does not have.
    fn schedule_pin(&self, statics: &Statics, pin: &Pin, queue: &mut Vec<SimEvent>) {
        let next = pin.next_run_time(statics);
        if let Some(existing) = queue.iter().position(|e| e.pin_id == pin.pin_id) {
            match next {
                Some(t) if t >= queue[existing].run_time => return,
                _ => {
                    queue.remove(existing);
                }
            }
        }
        let run_time = match next {
            Some(t) if t >= self.current_sim_time => t,
            _ => self.current_sim_time,
        };
        queue.push(SimEvent {
            run_time,
            pin_id: pin.pin_id,
        });
    }

    /// `BaseColony.RunSimulation(runSimUntil)` -- advance the colony to
    /// `sim_end_time`. **[V]**, ported from `baseColony.pyc` offset 246.
    ///
    /// ```text
    ///     if self.currentSimTime is None: self.RecalculateCurrentSimTime()
    ///     self.PrimeSimulation(simEndTime)
    ///     while len(self.simQueue) > 0:
    ///         (simTime, simPinID) = heapq.heappop(self.simQueue)
    ///         if simTime > simEndTime: break
    ///         self.currentSimTime = simTime
    ///         simPin = self.GetPin(simPinID)
    ///         if simPin is None: LogTraceback; continue
    ///         if simPin.IsExtractor() and (cycleTime is None or cycleTime <= 0): break
    ///         if not simPin.CanRun(self.currentSimTime): continue
    ///         self.EvaluatePin(simPin)
    ///     self.currentSimTime = simEndTime
    /// ```
    ///
    /// Four details that are easy to get wrong and all four change the
    /// result:
    ///
    /// - **`simTime > simEndTime` breaks; equality runs.** A cycle landing
    ///   exactly on the deadline is produced. EvEJS drops it.
    /// - **A zero-or-negative extractor cycle time `break`s the whole loop**,
    ///   it does not `continue`. That is CCP's own guard against the infinite
    ///   loop a zero cycle would be, and it abandons the pass rather than
    ///   skipping the pin -- so a malformed ECU stops its colony's
    ///   simulation, visibly, instead of spinning.
    /// - **`currentSimTime` is set *before* the pin runs**, and `EvaluatePin`
    ///   reads it as the cycle's time. So the pin's `lastRunTime` becomes the
    ///   scheduled time, not the deadline, and the next cycle is a whole
    ///   `cycleTime` after it -- drift-free over a year of catch-up.
    /// - **`currentSimTime` lands on `simEndTime` at the end**, whether or
    ///   not anything ran. Leaving it at the last event would replay that
    ///   event's tail on the next pass.
    ///
    /// Returns what happened; the colony is mutated in place.
    pub fn run_simulation(&mut self, statics: &Statics, sim_end_time: i64) -> SimReport {
        let mut report = SimReport::default();
        if sim_end_time <= self.current_sim_time {
            // Nothing to do, and in particular do **not** move the clock
            // backwards: a client whose own clock is behind ours must not be
            // able to rewind a colony by asking about it.
            return report;
        }
        let mut queue = self.prime_simulation(statics, sim_end_time);
        let mut touched: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();

        while !queue.is_empty() {
            if report.events >= MAX_SIMULATION_EVENTS {
                report.capped = true;
                break;
            }
            // `heappop`: the smallest `(run_time, pin_id)`.
            let next = queue
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| **e)
                .map(|(i, e)| (i, *e));
            let Some((index, event)) = next else { break };
            if event.run_time > sim_end_time {
                break;
            }
            queue.remove(index);
            self.current_sim_time = event.run_time;
            let Some(pin_index) = self.pin_index(event.pin_id) else {
                // The client logs a traceback and continues. A scheduled pin
                // that no longer exists means something removed it mid-pass,
                // which cannot happen here -- the pass holds `&mut self` --
                // so this is unreachable rather than merely unlikely, and
                // continuing is still the right answer if it ever is not.
                continue;
            };
            let kind = self.pins[pin_index].kind(statics);
            if kind.is_some_and(PinKind::is_extractor) && self.pins[pin_index].cycle_time <= 0 {
                break;
            }
            if !self.pins[pin_index].can_run(statics, self.current_sim_time) {
                continue;
            }
            self.evaluate_pin(statics, pin_index, &mut queue, &mut touched);
            report.events += 1;
        }

        if !report.capped {
            self.current_sim_time = sim_end_time;
        }
        report.touched = touched.into_iter().collect();
        report
    }

    /// `BaseColony.EvaluatePin`. **[V]**
    ///
    /// ```text
    ///     if not pin.CanActivate() and not pin.IsActive(): return
    ///     commodities = pin.Run(self.currentSimTime)
    ///     if pin.IsConsumer(): self.RouteCommodityInput(pin)
    ///     if pin.IsActive() or pin.CanActivate(): self.SchedulePin(pin)
    ///     if len(commodities) == 0: return
    ///     self.RouteCommodityOutput(pin, commodities)
    /// ```
    ///
    /// The input routing happens **after** the cycle and **before** the
    /// reschedule, which is the whole reason a well-fed factory never idles:
    /// `Run` consumed the next batch if it could, `RouteCommodityInput` tops
    /// the buffer back up from the silos, and only then does the pin decide
    /// whether it has a reason to be queued again.
    fn evaluate_pin(
        &mut self,
        statics: &Statics,
        pin_index: usize,
        queue: &mut Vec<SimEvent>,
        touched: &mut std::collections::BTreeSet<i64>,
    ) {
        let run_time = self.current_sim_time;
        {
            let pin = &self.pins[pin_index];
            if !pin.can_activate(statics) && !pin.is_active(statics) {
                return;
            }
        }
        let pin_id = self.pins[pin_index].pin_id;
        let commodities = self.pins[pin_index].run(statics, run_time);
        touched.insert(pin_id);

        let kind = self.pins[pin_index].kind(statics);
        if kind.is_some_and(PinKind::is_consumer) {
            self.route_commodity_input(statics, pin_index, touched);
        }
        {
            let pin = &self.pins[pin_index];
            if pin.is_active(statics) || pin.can_activate(statics) {
                self.schedule_pin(statics, pin, queue);
            }
        }
        if commodities.is_empty() {
            return;
        }
        self.route_commodity_output(statics, pin_id, commodities, queue, touched, 0);
    }

    /// `colonyData.GetSortedRoutesForPin`. **[V]**
    ///
    /// Routes out of `pin_id` are aggregated by `(destination, commodity)` --
    /// the client keeps them pre-aggregated in `megaRoutesBySource` -- then
    /// split into two heaps: factories keyed on
    /// [`Pin::input_buffer_state`] and storage keyed on
    /// [`Pin::free_space`]. Both are **min-heaps**, so the emptiest factory
    /// and the emptiest silo go first.
    ///
    /// A commodity the source did not actually produce this cycle is skipped
    /// entirely: the `commodityTypeID not in commodities` test. So a route is
    /// not a standing order, it is a share of what came out.
    fn sorted_routes_for_pin(
        &self,
        statics: &Statics,
        pin_id: i64,
        commodities: &BTreeMap<i64, i64>,
    ) -> (Vec<SortedRoute>, Vec<SortedRoute>) {
        let mut mega: BTreeMap<(i64, i64), i64> = BTreeMap::new();
        for r in &self.routes {
            if r.source() != Some(pin_id) || r.commodity_quantity <= 0 {
                continue;
            }
            let Some(dest) = r.destination() else {
                continue;
            };
            if !commodities.contains_key(&r.commodity_type_id) {
                continue;
            }
            *mega.entry((dest, r.commodity_type_id)).or_insert(0) += r.commodity_quantity;
        }
        let mut processor = Vec::new();
        let mut storage = Vec::new();
        for ((dest, type_id), qty) in mega {
            let Some(dest_pin) = self.pin(dest) else {
                continue;
            };
            match dest_pin.kind(statics) {
                Some(k) if k.is_processor() => {
                    processor.push((dest_pin.input_buffer_state(statics), dest, type_id, qty))
                }
                Some(k) if k.is_storage() => {
                    storage.push((dest_pin.free_space(statics), dest, type_id, qty))
                }
                // A destination that accepts nothing is not an error here --
                // route creation already refused it -- so a colony that
                // somehow holds one just does not deliver.
                _ => {}
            }
        }
        // `f64` is not `Ord`, so sort rather than `BinaryHeap`. `total_cmp`
        // and not `partial_cmp().unwrap()`: a `NaN` priority from a missing
        // static row must order deterministically rather than panic in front
        // of a player.
        let key = |e: &(f64, i64, i64, i64)| (e.1, e.2, e.3);
        processor.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| key(a).cmp(&key(b))));
        storage.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| key(a).cmp(&key(b))));
        (processor, storage)
    }

    /// `BaseColony.RouteCommodityOutput`. **[V]**
    ///
    /// Processor routes first, then storage routes, and the storage pass caps
    /// each delivery at `ceil(remaining / routes_left)` so that a cycle's
    /// output is **spread evenly** over the silos instead of filling the
    /// first one. That cap is the only place the two passes differ, and
    /// `ceil` rather than `floor` is what stops a remainder being stranded.
    ///
    /// The tail is the recursion: a commodity that lands in a silo from a
    /// *non*-storage source is immediately offered onwards, so a
    /// factory -> silo -> factory chain moves in one cycle rather than two.
    /// A consumer that received something is rescheduled instead, because it
    /// now has a reason to run.
    ///
    /// **The client has no recursion limit here.** Ours stops at depth 8,
    /// which EvEJS also chose (`planetRuntimeStore.js:2064`). A colony cannot
    /// legally contain a storage cycle -- route validation refuses
    /// storage-to-storage -- so the limit is unreachable by construction and
    /// is there for a stored colony that is already wrong.
    fn route_commodity_output(
        &mut self,
        statics: &Statics,
        source_pin_id: i64,
        mut commodities: BTreeMap<i64, i64>,
        queue: &mut Vec<SimEvent>,
        touched: &mut std::collections::BTreeSet<i64>,
        depth: usize,
    ) {
        if depth > 8 {
            return;
        }
        let source_is_storage = self
            .pin(source_pin_id)
            .and_then(|p| p.kind(statics))
            .is_some_and(PinKind::is_storage);
        let mut received: BTreeMap<i64, BTreeMap<i64, i64>> = BTreeMap::new();

        let (processor_routes, storage_routes) =
            self.sorted_routes_for_pin(statics, source_pin_id, &commodities);
        for (is_storage_pass, mut routes) in [(false, processor_routes), (true, storage_routes)] {
            if commodities.is_empty() {
                break;
            }
            while !routes.is_empty() {
                let (_, dest, type_id, qty) = routes.remove(0);
                let available = commodities.get(&type_id).copied().unwrap_or(0);
                if available <= 0 {
                    continue;
                }
                let mut amount = available.min(qty);
                if is_storage_pass {
                    // `math.ceil(float(commodities.get(typeID, 0)) / (len(listOfRoutes) + 1))`
                    // -- `listOfRoutes` has already been popped, so the
                    // divisor counts this route plus the ones still to come.
                    let remaining_routes = routes.len() as i64 + 1;
                    let share = (available + remaining_routes - 1) / remaining_routes;
                    amount = amount.min(share);
                }
                if amount <= 0 {
                    continue;
                }
                let moved =
                    self.transfer_commodities(statics, source_pin_id, dest, type_id, amount);
                if moved <= 0 {
                    continue;
                }
                touched.insert(dest);
                touched.insert(source_pin_id);
                let left = available - moved;
                if left > 0 {
                    commodities.insert(type_id, left);
                } else {
                    commodities.remove(&type_id);
                }
                *received
                    .entry(dest)
                    .or_default()
                    .entry(type_id)
                    .or_insert(0) += moved;
                if commodities.is_empty() {
                    break;
                }
            }
        }

        for (dest, added) in received {
            let Some(index) = self.pin_index(dest) else {
                continue;
            };
            let kind = self.pins[index].kind(statics);
            if kind.is_some_and(PinKind::is_consumer) {
                let pin = &self.pins[index];
                self.schedule_pin(statics, pin, queue);
            } else if !source_is_storage && kind.is_some_and(PinKind::is_storage) {
                self.route_commodity_output(statics, dest, added, queue, touched, depth + 1);
            }
        }
    }

    /// `BaseColony.RouteCommodityInput`. **[V]**
    ///
    /// For every route *into* this factory whose source is a storage pin,
    /// offer the silo's whole contents and let `TransferCommodities` cap it
    /// at the route's quantity and the factory's demand. Routes are taken in
    /// `routeID` order, which is the order the client stores them in and
    /// therefore the order the player created them in.
    fn route_commodity_input(
        &mut self,
        statics: &Statics,
        pin_index: usize,
        touched: &mut std::collections::BTreeSet<i64>,
    ) {
        let dest_pin_id = self.pins[pin_index].pin_id;
        let mut inbound: Vec<(i64, i64, i64, i64)> = self
            .routes
            .iter()
            .filter(|r| r.destination() == Some(dest_pin_id) && r.commodity_quantity > 0)
            .filter_map(|r| {
                Some((
                    r.route_id,
                    r.source()?,
                    r.commodity_type_id,
                    r.commodity_quantity,
                ))
            })
            .collect();
        inbound.sort_by_key(|(route_id, _, _, _)| *route_id);

        for (_, source_id, type_id, qty) in inbound {
            let Some(source) = self.pin(source_id) else {
                continue;
            };
            if !source.kind(statics).is_some_and(PinKind::is_storage) {
                continue;
            }
            if source.contents.is_empty() {
                continue;
            }
            let have = source.quantity(type_id);
            if have <= 0 {
                continue;
            }
            let moved =
                self.transfer_commodities(statics, source_id, dest_pin_id, type_id, have.min(qty));
            if moved > 0 {
                touched.insert(source_id);
                touched.insert(dest_pin_id);
            }
        }
    }

    /// `BaseColony.TransferCommodities`. **[V]**
    ///
    /// ```text
    ///     amtToMove = min(commodities[typeID], qty)
    ///     if maxAmount is not None: amtToMove = min(maxAmount, amtToMove)
    ///     if amtToMove <= 0: return (0, 0)
    ///     amtMoved = destPin.AddCommodity(typeID, amtToMove)
    ///     if sourcePin.IsStorage(): sourcePin.RemoveCommodity(typeID, amtMoved)
    ///     return (typeID, amtMoved)
    /// ```
    ///
    /// **The source is debited only if it is storage.** A factory's or an
    /// ECU's output was never in its contents -- `Run` returned it as a value
    /// -- so debiting it would remove the player's stock to pay for goods
    /// that came out of thin air. Getting this backwards drains a colony's
    /// silos exactly as fast as it produces.
    ///
    /// Returns how much actually moved, which is what the destination took
    /// and not what was offered.
    fn transfer_commodities(
        &mut self,
        statics: &Statics,
        source_pin_id: i64,
        dest_pin_id: i64,
        type_id: i64,
        amount: i64,
    ) -> i64 {
        if amount <= 0 || source_pin_id == dest_pin_id {
            return 0;
        }
        let Some(dest_index) = self.pin_index(dest_pin_id) else {
            return 0;
        };
        let moved = self.pins[dest_index].add_commodity(statics, type_id, amount);
        if moved <= 0 {
            return 0;
        }
        let Some(source_index) = self.pin_index(source_pin_id) else {
            return moved;
        };
        if self.pins[source_index]
            .kind(statics)
            .is_some_and(PinKind::is_storage)
        {
            self.pins[source_index].remove_commodity(type_id, moved);
        }
        moved
    }
}

// ---------------------------------------------------------------------------
// The command stream
// ---------------------------------------------------------------------------

/// The thirteen opcodes `UserUpdateNetwork` carries.
///
/// **[V]**, and this is the single most build-sensitive thing in PI, so it was
/// read out of our own client rather than taken from EvEJS. The module is
/// `eve/common/script/planet/commandStream.pyc`; its module body assigns the
/// constants `1`..`13` to `COMMAND_CREATEPIN`..`COMMAND_INSTALLPROGRAM` in
/// exactly this order, and `CommandStream.Serialize` builds, per command, a
/// `(command.id, argsTuple)` 2-tuple whose argument order is the one
/// transcribed on each variant below.
///
/// EvEJS's `planetCostCalculator.js:24-37` has the same thirteen names with
/// the same thirteen numbers: **13 of 13, no build delta.** That agreement is
/// worth stating because the inventory flagged it as the single riskiest
/// unknown in the subsystem -- *"opcode numbering and the command payload
/// shape are client-build-specific and undocumented; wrong opcode = colony
/// silently mangled"* (`docs/evejs/inventory/industry-mining.md:138`). It is now
/// neither undocumented nor unknown.
///
/// The whole PI window submits **one** of these streams per Submit, from
/// `ClientPlanet._SubmitChanges`, and `CommandStream._RemoveSynonymsAndAntonyms`
/// has already collapsed redundant pairs before it is sent -- so a stream is a
/// minimal delta, not a replay of the colony.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `1` -- `(pinID, typeID, latitude, longitude)`.
    CreatePin {
        pin_id: Id,
        type_id: i64,
        point: SurfacePoint,
    },
    /// `2` -- `(pinID,)`.
    RemovePin { pin_id: Id },
    /// `3` -- `(endpoint1, endpoint2, level)`.
    CreateLink {
        endpoint1: Id,
        endpoint2: Id,
        level: i64,
    },
    /// `4` -- `(endpoint1, endpoint2)`.
    RemoveLink { endpoint1: Id, endpoint2: Id },
    /// `5` -- `(endpoint1, endpoint2, level)`.
    SetLinkLevel {
        endpoint1: Id,
        endpoint2: Id,
        level: i64,
    },
    /// `6` -- `(routeID, path, typeID, quantity)`.
    CreateRoute {
        route_id: Id,
        path: Vec<Id>,
        type_id: i64,
        quantity: i64,
    },
    /// `7` -- `(routeID,)`.
    RemoveRoute { route_id: Id },
    /// `8` -- `(pinID, schematicID)`.
    SetSchematic { pin_id: Id, schematic_id: i64 },
    /// `9` -- `(pinID, level)`.
    UpgradeCommandCenter { pin_id: Id, level: i64 },
    /// `10` -- `(pinID, headID, latitude, longitude)`.
    AddExtractorHead {
        pin_id: Id,
        head_id: i64,
        point: SurfacePoint,
    },
    /// `11` -- `(pinID, headID)`.
    KillExtractorHead { pin_id: Id, head_id: i64 },
    /// `12` -- `(pinID, headID, latitude, longitude)`.
    MoveExtractorHead {
        pin_id: Id,
        head_id: i64,
        point: SurfacePoint,
    },
    /// `13` -- `(pinID, typeID, headRadius)`.
    ///
    /// `typeID` is the **resource** being extracted, not the ECU's type, and
    /// `None` is Stop Program. The client sends `None` in that slot, which is
    /// why this is an `Option` and not a sentinel.
    InstallProgram {
        pin_id: Id,
        program_type: Option<i64>,
        head_radius: f64,
    },
}

impl Command {
    /// The opcode, so that a decoder and this enum cannot drift apart
    /// silently.
    pub fn opcode(&self) -> i64 {
        match self {
            Command::CreatePin { .. } => 1,
            Command::RemovePin { .. } => 2,
            Command::CreateLink { .. } => 3,
            Command::RemoveLink { .. } => 4,
            Command::SetLinkLevel { .. } => 5,
            Command::CreateRoute { .. } => 6,
            Command::RemoveRoute { .. } => 7,
            Command::SetSchematic { .. } => 8,
            Command::UpgradeCommandCenter { .. } => 9,
            Command::AddExtractorHead { .. } => 10,
            Command::KillExtractorHead { .. } => 11,
            Command::MoveExtractorHead { .. } => 12,
            Command::InstallProgram { .. } => 13,
        }
    }
}

/// A pin, link or route id as the stream carries it.
///
/// **A temporary id is any tuple.** `BaseColony.IsTemporaryID` is the whole
/// of
///
/// ```text
///     return type(id) == types.TupleType
/// ```
///
/// -- no namespace, no range, no structure. The client mints one for every
/// object it created in this edit session, submits the stream with those
/// tuples in place of ids, and the server replies with a colony in which they
/// have been replaced by real ids; `BaseColony.RegisterTemporaryID` and
/// `ReplaceTemporaryIDsInPath` are the client's side of that.
///
/// **EvEJS requires the tuple's first element to be a namespace tag** -- `1`
/// for pins, `2` for routes (`planetRuntimeStore.js:1096
/// isTemporaryID(value, namespace)`). Our client imposes no such thing, so a
/// server that insisted on it would reject a stream the client considers
/// well-formed. We follow the client: any tuple is temporary, and what it
/// contains is opaque.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Id {
    /// A real id the client already knew.
    Real(i64),
    /// A tuple the client minted during this edit session. Carried verbatim,
    /// because it is the key the reply is matched against.
    Temporary(Vec<i64>),
}

impl Id {
    pub fn is_temporary(&self) -> bool {
        matches!(self, Id::Temporary(_))
    }
}

/// How the server answers "how much will this extractor yield per cycle".
///
/// **This is the one part of PI our client cannot give us**, and it is a
/// trait rather than a function so that the gap is in the type system instead
/// of in a comment.
///
/// `baseColony.CreateProgram:1328` computes the quote as
/// `maxVolume * sum(builder.GetValueAt(harmonic, theta, phi) * overlap_i)`
/// over the ECU's heads. `maxVolume` and the overlap maths are ours
/// ([`ecu_default::MAX_VOLUME`], [`ecu_default::OVERLAP_FACTOR`]), but
/// `builder` is `_eveplanetresources`, a **native** extension, and the
/// `harmonic` it evaluates is a spherical-harmonic coefficient blob the
/// *server* authors and ships to the client through
/// `planetMgr.GetResourceData`. The client only evaluates it.
///
/// So there is no per-planet resource field in the client to read, and no
/// amount of static-data work will produce one: it has to be generated, and
/// then a compatible evaluator written, before an extractor can be quoted.
/// See `docs/lanes/planet.md`.
pub trait ProgramQuote {
    /// Units per cycle for `program_type` at this ECU's current heads, or `0`
    /// if the planet's resource field is unknown.
    fn quote(&self, colony: &Colony, ecu_pin_id: i64, program_type: i64, head_radius: f64) -> i64;
}

/// The honest quote while there is no resource field: nothing.
///
/// An `INSTALLPROGRAM` against this is refused with
/// [`PiError::ProgramQuoteIsZero`] rather than installing a program that
/// yields nothing, which is the difference between an error the player can
/// read and an extractor that looks installed and is not.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoResourceField;

impl ProgramQuote for NoResourceField {
    fn quote(&self, _: &Colony, _: i64, _: i64, _: f64) -> i64 {
        0
    }
}

/// Everything applying a stream needs that is not in the stream.
pub struct ApplyContext<'a> {
    /// The acting character. Every pin the stream touches must belong to it,
    /// which is `CanOnlyManageOwnPins`.
    pub owner_id: i64,
    /// `blue.os.GetWallclockTime()`, as `service::filetime_now()`.
    pub now: i64,
    /// Lane clock: the pace every pin of the colony runs at ([`Pin::pace`]).
    pub pace: crate::game_clock::GameClock,
    /// Where the next real pin id comes from. The client never chooses one.
    pub next_pin_id: &'a mut i64,
    /// Where the next real route id comes from.
    pub next_route_id: &'a mut i64,
    /// The link type `CREATELINK` builds. One type exists --
    /// `groupPlanetaryLinks` (1036) holds exactly one row, typeID 2280
    /// "Link" -- but it is passed in rather than hardcoded because it is
    /// static data and this module holds none.
    pub link_type_id: i64,
    pub quote: &'a dyn ProgramQuote,
}

/// What applying a stream produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// Temporary id -> the real id it became, so the caller can answer the
    /// client's `RegisterTemporaryID`.
    pub id_map: Vec<(Id, i64)>,
    /// Pin ids this stream created. **Only these may be charged for**: the
    /// post-simulation pin set still holds the command centre from the first
    /// submit, and charging from that re-charged the player on every later
    /// edit until their stack ran dry -- EvEJS's own bug, recorded at
    /// `planetMgrService.js:903-915`. Recording the created set rather than
    /// re-deriving it is the fix, and it is cheap enough to do from the
    /// start.
    pub created_pins: Vec<i64>,
}

impl Colony {
    /// Apply a command stream, **all or nothing**.
    ///
    /// The colony is simulated up to `ctx.now` first, exactly as
    /// `BaseColony` does before validating an edit -- a stream must be
    /// checked against the colony as it *is*, not as it was when the player
    /// opened the window, or a factory's buffer state and an extractor's
    /// yield are both stale.
    ///
    /// Atomicity is a clone and a swap, which is what a language with
    /// ownership gives us for free. **Do not port EvEJS's answer to this.**
    /// It has no transactions, so it grew a durable receipt ledger stored
    /// inside the items table, a whole-table sync fence and per-domain
    /// compensating sagas (`docs/port-tracker.md` §5 on `itemStore.js:86`).
    /// Every one of those exists to compensate for a missing primitive we
    /// have.
    pub fn apply(
        &mut self,
        statics: &Statics,
        commands: &[Command],
        ctx: &mut ApplyContext<'_>,
    ) -> Result<ApplyReport, PiError> {
        let mut draft = self.clone();
        draft.set_pace(ctx.pace);
        draft.run_simulation(statics, ctx.now);
        let mut report = ApplyReport::default();
        let mut next_pin_id = *ctx.next_pin_id;
        let mut next_route_id = *ctx.next_route_id;

        for command in commands {
            draft.apply_one(
                statics,
                command,
                ctx,
                &mut report,
                &mut next_pin_id,
                &mut next_route_id,
            )?;
        }

        // The whole-colony checks run once, at the end, not per command. A
        // stream legitimately passes through states it would be refused in --
        // the obvious one is replacing a link with a better one, which is
        // REMOVELINK then CREATELINK and leaves the colony disconnected in
        // between -- so per-command validation would reject edits the client
        // was right to submit.
        draft.validate(statics, ctx.owner_id)?;

        *ctx.next_pin_id = next_pin_id;
        *ctx.next_route_id = next_route_id;
        *self = draft;
        Ok(report)
    }

    fn resolve(&self, id: &Id, report: &ApplyReport) -> Option<i64> {
        match id {
            Id::Real(n) => Some(*n),
            Id::Temporary(_) => report
                .id_map
                .iter()
                .find(|(k, _)| k == id)
                .map(|(_, real)| *real),
        }
    }

    fn apply_one(
        &mut self,
        statics: &Statics,
        command: &Command,
        ctx: &ApplyContext<'_>,
        report: &mut ApplyReport,
        next_pin_id: &mut i64,
        next_route_id: &mut i64,
    ) -> Result<(), PiError> {
        match command {
            Command::CreatePin {
                pin_id,
                type_id,
                point,
            } => {
                self.validate_create_pin(statics, *type_id)?;
                let real = match pin_id {
                    Id::Real(n) if *n > 0 => *n,
                    Id::Real(_) => return Err(PiError::PinDoesNotExist),
                    Id::Temporary(_) => {
                        let allocated = *next_pin_id;
                        *next_pin_id += 1;
                        report.id_map.push((pin_id.clone(), allocated));
                        allocated
                    }
                };
                self.pins.retain(|p| p.pin_id != real);
                let mut pin = Pin::new(real, *type_id, ctx.owner_id, *point, ctx.now);
                pin.pace = ctx.pace;
                self.pins.push(pin);
                self.pins.sort_by_key(|p| p.pin_id);
                report.created_pins.push(real);
            }
            Command::RemovePin { pin_id } => {
                let id = self
                    .resolve(pin_id, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                let pin = self.pin(id).ok_or(PiError::PinDoesNotExist)?;
                if pin.owner_id != ctx.owner_id {
                    return Err(PiError::CanOnlyManageOwnPins);
                }
                self.pins.retain(|p| p.pin_id != id);
                // A link or a route that outlived its endpoint is the one
                // thing a colony must never hold: every later lookup would
                // raise `RouteFailedValidationPinDoesNotExist`, including the
                // read the window does to draw itself. The client removes
                // them with the pin and so do we.
                self.links
                    .retain(|l| l.endpoint1 != id && l.endpoint2 != id);
                self.routes.retain(|r| !r.path.contains(&id));
            }
            Command::CreateLink {
                endpoint1,
                endpoint2,
                level,
            } => {
                let a = self
                    .resolve(endpoint1, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                let b = self
                    .resolve(endpoint2, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                self.validate_endpoints(statics, a, b, ctx.owner_id)?;
                if self.link(a, b).is_some() {
                    return Err(PiError::LinkAlreadyExists);
                }
                if *level < 0 || *level > LINK_MAX_UPGRADE {
                    return Err(PiError::CannotUpgradeLinkTooHigh);
                }
                self.links.push(Link::new(a, b, ctx.link_type_id, *level));
                self.links.sort_by_key(|l| (l.endpoint1, l.endpoint2));
            }
            Command::RemoveLink {
                endpoint1,
                endpoint2,
            } => {
                let a = self
                    .resolve(endpoint1, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                let b = self
                    .resolve(endpoint2, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                if self.link(a, b).is_none() {
                    return Err(PiError::LinkDoesNotExist);
                }
                self.links.retain(|l| !l.connects(a, b));
                // Every route that crossed it goes too, for the reason on
                // `RemovePin`.
                self.routes.retain(|r| !route_uses_link(r, a, b));
            }
            Command::SetLinkLevel {
                endpoint1,
                endpoint2,
                level,
            } => {
                let a = self
                    .resolve(endpoint1, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                let b = self
                    .resolve(endpoint2, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                if *level < 0 || *level > LINK_MAX_UPGRADE {
                    return Err(PiError::CannotUpgradeLinkTooHigh);
                }
                let link = self
                    .links
                    .iter_mut()
                    .find(|l| l.connects(a, b))
                    .ok_or(PiError::LinkDoesNotExist)?;
                link.level = *level;
            }
            Command::CreateRoute {
                route_id,
                path,
                type_id,
                quantity,
            } => {
                let real = match route_id {
                    Id::Real(n) if *n > 0 => *n,
                    Id::Real(_) => return Err(PiError::RouteDoesNotExist),
                    Id::Temporary(_) => {
                        let allocated = *next_route_id;
                        *next_route_id += 1;
                        report.id_map.push((route_id.clone(), allocated));
                        allocated
                    }
                };
                let mut resolved = Vec::with_capacity(path.len());
                for step in path {
                    resolved.push(
                        self.resolve(step, report)
                            .ok_or(PiError::RouteFailedValidationPinDoesNotExist)?,
                    );
                }
                let route = Route {
                    route_id: real,
                    owner_id: ctx.owner_id,
                    path: resolved,
                    commodity_type_id: *type_id,
                    commodity_quantity: *quantity,
                };
                self.validate_route(statics, &route, ctx.owner_id)?;
                self.routes.retain(|r| r.route_id != real);
                self.routes.push(route);
                self.routes.sort_by_key(|r| r.route_id);
            }
            Command::RemoveRoute { route_id } => {
                let id = self
                    .resolve(route_id, report)
                    .ok_or(PiError::RouteDoesNotExist)?;
                if self.route(id).is_none() {
                    return Err(PiError::RouteDoesNotExist);
                }
                self.routes.retain(|r| r.route_id != id);
            }
            Command::SetSchematic {
                pin_id,
                schematic_id,
            } => {
                let id = self
                    .resolve(pin_id, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                let owner = ctx.owner_id;
                let pin = self.pin_mut(id).ok_or(PiError::PinDoesNotExist)?;
                if pin.owner_id != owner {
                    return Err(PiError::CanOnlyManageOwnPins);
                }
                // Cloned out of the borrow: `install_schematic` needs
                // `&Statics` and `&mut Pin` at once, and the pin lives in
                // `self.pins`.
                let mut edited = pin.clone();
                edited.install_schematic(statics, *schematic_id)?;
                *self.pin_mut(id).expect("just found it") = edited;
            }
            Command::UpgradeCommandCenter { pin_id, level } => {
                let id = self
                    .resolve(pin_id, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                let pin = self.pin(id).ok_or(PiError::PinDoesNotExist)?;
                if pin.owner_id != ctx.owner_id {
                    return Err(PiError::CanOnlyManageOwnPins);
                }
                if pin.kind(statics) != Some(PinKind::Command) {
                    return Err(PiError::CannotManagePlanetWithoutCommandCenter);
                }
                // `ValidateCommandCenterUpgrade`: no downgrades and no going
                // past the table. A downgrade would have to refund, and the
                // client has no UI for it.
                if *level <= self.level {
                    return Err(PiError::CannotDowngradeCommandCenter);
                }
                if *level > MAX_COMMAND_CENTRE_LEVEL {
                    return Err(PiError::CannotUpgradeCommandCenterPastMaximum);
                }
                self.level = *level;
            }
            Command::AddExtractorHead {
                pin_id,
                head_id,
                point,
            } => {
                let id = self
                    .resolve(pin_id, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                let owner = ctx.owner_id;
                let mut edited = self.pin(id).ok_or(PiError::PinDoesNotExist)?.clone();
                if edited.owner_id != owner {
                    return Err(PiError::CanOnlyManageOwnPins);
                }
                edited.add_head(
                    statics,
                    Head {
                        head_id: *head_id,
                        latitude: point.latitude,
                        longitude: point.longitude,
                    },
                )?;
                *self.pin_mut(id).expect("just found it") = edited;
            }
            Command::KillExtractorHead { pin_id, head_id } => {
                let id = self
                    .resolve(pin_id, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                let owner = ctx.owner_id;
                let mut edited = self.pin(id).ok_or(PiError::PinDoesNotExist)?.clone();
                if edited.owner_id != owner {
                    return Err(PiError::CanOnlyManageOwnPins);
                }
                edited.remove_head(statics, *head_id)?;
                *self.pin_mut(id).expect("just found it") = edited;
            }
            Command::MoveExtractorHead {
                pin_id,
                head_id,
                point,
            } => {
                let id = self
                    .resolve(pin_id, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                let owner = ctx.owner_id;
                let mut edited = self.pin(id).ok_or(PiError::PinDoesNotExist)?.clone();
                if edited.owner_id != owner {
                    return Err(PiError::CanOnlyManageOwnPins);
                }
                edited.move_head(statics, *head_id, *point)?;
                *self.pin_mut(id).expect("just found it") = edited;
            }
            Command::InstallProgram {
                pin_id,
                program_type,
                head_radius,
            } => {
                let id = self
                    .resolve(pin_id, report)
                    .ok_or(PiError::PinDoesNotExist)?;
                let owner = ctx.owner_id;
                let mut edited = self.pin(id).ok_or(PiError::PinDoesNotExist)?.clone();
                if edited.owner_id != owner {
                    return Err(PiError::CanOnlyManageOwnPins);
                }
                // The quote is taken against the colony as it stands, with
                // this ECU's heads as they now are -- so an ADDEXTRACTORHEAD
                // earlier in the same stream is already accounted for, which
                // is the order the survey window submits in.
                let qty = match program_type {
                    Some(program) => ctx.quote.quote(self, id, *program, *head_radius),
                    None => 0,
                };
                edited.install_program(statics, *program_type, *head_radius, qty, ctx.now)?;
                *self.pin_mut(id).expect("just found it") = edited;
            }
        }
        Ok(())
    }
}

/// Whether a route's path crosses the link between `a` and `b`.
fn route_uses_link(route: &Route, a: i64, b: i64) -> bool {
    route
        .path
        .windows(2)
        .any(|w| (w[0] == a && w[1] == b) || (w[0] == b && w[1] == a))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

impl Colony {
    /// `baseColony.ValidateCreatePin`. **[V]**
    ///
    /// Three refusals, in the client's order:
    ///
    /// - A type in none of the six pin groups is `InvalidPinType`.
    /// - A **second** command centre is `CannotBuildMultipleCommandPins`;
    ///   anything else **without** one is
    ///   `CannotManagePlanetWithoutCommandCenter`. So the command centre is
    ///   both required first and allowed only once, and that pair of rules is
    ///   what makes `created_pins` safe to charge for: a `CREATEPIN` of a
    ///   command centre that survives validation is always a genuine first
    ///   placement.
    /// - `attributePlanetRestriction` (1632) must match the planet, which is
    ///   `CannotCreatePinWrongPlanetType`. It is how a Barren Storage
    ///   Facility is kept off a lava planet; all eight of each pin family
    ///   differ only in this attribute.
    pub fn validate_create_pin(&self, statics: &Statics, type_id: i64) -> Result<(), PiError> {
        let kind = statics.pin_kind(type_id).ok_or(PiError::InvalidPinType)?;
        if kind == PinKind::Command {
            if self.command_pin(statics).is_some() {
                return Err(PiError::CannotBuildMultipleCommandPins);
            }
        } else if self.command_pin(statics).is_none() {
            return Err(PiError::CannotManagePlanetWithoutCommandCenter);
        }
        let restriction = statics
            .pin_type(type_id)
            .map_or(0.0, |t| t.attr(attr::PLANET_RESTRICTION))
            .trunc() as i64;
        if restriction > 0 && self.planet_type_id > 0 && restriction != self.planet_type_id {
            return Err(PiError::CannotCreatePinWrongPlanetType);
        }
        Ok(())
    }

    /// `baseColony.ValidateCreateLink`'s endpoint checks.
    ///
    /// `BasePin.CanLinkTo` refuses a pin to itself, and both ends must be the
    /// caller's. The *bandwidth* and *CPU* consequences are not checked here:
    /// they are properties of the finished colony and are checked once, in
    /// [`Colony::validate`], for the reason given on [`Colony::apply`].
    fn validate_endpoints(
        &self,
        _statics: &Statics,
        a: i64,
        b: i64,
        owner_id: i64,
    ) -> Result<(), PiError> {
        if a == b {
            return Err(PiError::CannotLinkPinToItself);
        }
        for id in [a, b] {
            let pin = self.pin(id).ok_or(PiError::PinDoesNotExist)?;
            if pin.owner_id != owner_id {
                return Err(PiError::CanOnlyManageOwnPins);
            }
        }
        Ok(())
    }

    /// `baseColony.ValidateCreateRoute`, and `planetCommon.GetRouteValidationInfo`.
    /// **[V]** on every refusal's name.
    ///
    /// The checks that are not obvious:
    ///
    /// - **`path.len() - 2 > MAX_WAYPOINTS`.** The cap is on the pins
    ///   *between* the endpoints, so a legal path is at most seven long. The
    ///   client's own router will not build a longer one, so a longer one is
    ///   either a hand-built packet or a bug.
    /// - **A storage source may only feed a consumer.** `storage -> storage`
    ///   is refused with `CannotRouteStorageMustGoToConsumer`, and that
    ///   refusal is what makes the storage-to-storage recursion in
    ///   [`Colony::route_commodity_output`] unreachable in a legal colony.
    /// - **A producer source must actually produce the commodity**, and the
    ///   sum of its routes for that commodity must fit inside one cycle's
    ///   *maximum* output. The cap is [`Pin::product_max_output`] -- the
    ///   noisiest cycle -- not the nominal yield, because the client sizes
    ///   its own route proposals against `GetProductMaxOutput` and a server
    ///   using the nominal figure would reject routes the client offered.
    /// - **Every consecutive pair of the path must be a link.** This is the
    ///   check that makes a route a physical thing rather than a wish.
    ///
    /// **The order is the client's, and it is observable.** The message
    /// constants in `ValidateCreateRoute` run, in bytecode order:
    /// `CreateRouteWithoutCommodities`, `CreateRouteTooShort`,
    /// `CannotRouteTooManyWaypoints`, `CannotRouteStorageMustGoToConsumer`,
    /// `CannotRouteDestinationWillNotAccept`,
    /// `CreateRouteCommodityNotProduced`,
    /// `CreateRouteCommodityProductionTooSmall`,
    /// `CreateRouteDestinationCannotAcceptCommodity`,
    /// `RouteFailedValidationPinDoesNotExist`,
    /// `RouteFailedValidationPinNotYours`,
    /// `RouteFailedValidationLinkDoesNotExist`,
    /// `RouteFailedValidationCannotRouteCommodities` -- so the **production
    /// cap is checked before the ownership walk**, which is not the order
    /// anyone would choose. It is kept because the player sees the first
    /// refusal and only the first, and a test here was written against the
    /// intuitive order before the bytecode settled it.
    pub fn validate_route(
        &self,
        statics: &Statics,
        route: &Route,
        owner_id: i64,
    ) -> Result<(), PiError> {
        if route.commodity_type_id <= 0 || route.commodity_quantity < 1 {
            return Err(PiError::CreateRouteWithoutCommodities);
        }
        if route.path.len() < 2 {
            return Err(PiError::CreateRouteTooShort);
        }
        if route.path.len() - 2 > MAX_WAYPOINTS {
            return Err(PiError::CannotRouteTooManyWaypoints);
        }
        let source_id = route.source().ok_or(PiError::CreateRouteTooShort)?;
        let dest_id = route.destination().ok_or(PiError::CreateRouteTooShort)?;
        let source = self
            .pin(source_id)
            .ok_or(PiError::RouteFailedValidationPinDoesNotExist)?;
        let dest = self
            .pin(dest_id)
            .ok_or(PiError::RouteFailedValidationPinDoesNotExist)?;
        let source_kind = source.kind(statics).ok_or(PiError::InvalidPinType)?;
        let dest_kind = dest.kind(statics).ok_or(PiError::InvalidPinType)?;

        if source_kind.is_storage() && !dest_kind.is_consumer() {
            return Err(PiError::CannotRouteStorageMustGoToConsumer);
        }
        if !dest_kind.is_consumer() && !dest_kind.is_storage() {
            return Err(PiError::CannotRouteDestinationWillNotAccept);
        }
        if source_kind.is_producer() {
            let produced = source
                .product_max_output(statics)
                .get(&route.commodity_type_id)
                .copied()
                .unwrap_or(0);
            if produced <= 0 {
                return Err(PiError::CreateRouteCommodityNotProduced);
            }
            let already: i64 = self
                .routes
                .iter()
                .filter(|r| {
                    r.route_id != route.route_id
                        && r.source() == Some(source_id)
                        && r.commodity_type_id == route.commodity_type_id
                })
                .map(|r| r.commodity_quantity)
                .sum();
            if already + route.commodity_quantity > produced {
                return Err(PiError::CreateRouteCommodityProductionTooSmall);
            }
        }
        if dest_kind.is_consumer()
            && dest
                .consumables(statics)
                .get(&route.commodity_type_id)
                .copied()
                .unwrap_or(0)
                <= 0
        {
            return Err(PiError::CreateRouteDestinationCannotAcceptCommodity);
        }

        let mut previous: Option<i64> = None;
        for step in &route.path {
            let pin = self
                .pin(*step)
                .ok_or(PiError::RouteFailedValidationPinDoesNotExist)?;
            if owner_id != 0 && pin.owner_id != owner_id {
                return Err(PiError::RouteFailedValidationPinNotYours);
            }
            if let Some(prev) = previous
                && self.link(prev, *step).is_none()
            {
                return Err(PiError::RouteFailedValidationLinkDoesNotExist);
            }
            previous = Some(*step);
        }
        Ok(())
    }

    /// Every whole-colony invariant, checked once after a stream.
    ///
    /// Order matters only in what the player is told first, and the client's
    /// order is: the command centre, then links and routes, then the power
    /// budget. Reporting "out of CPU" to someone who has not landed a command
    /// centre would be true and useless.
    pub fn validate(&self, statics: &Statics, owner_id: i64) -> Result<(), PiError> {
        // `ValidateCreatePin`'s rule, restated over the finished colony: at
        // most one command centre, and nothing at all without one.
        let command_pins = self
            .pins
            .iter()
            .filter(|p| p.kind(statics) == Some(PinKind::Command))
            .count();
        if command_pins > 1 {
            return Err(PiError::CannotBuildMultipleCommandPins);
        }
        if command_pins == 0
            && (!self.pins.is_empty() || !self.links.is_empty() || !self.routes.is_empty())
        {
            return Err(PiError::CannotManagePlanetWithoutCommandCenter);
        }

        let mut seen: Vec<(i64, i64)> = Vec::new();
        for link in &self.links {
            if link.level > LINK_MAX_UPGRADE || link.level < 0 {
                return Err(PiError::CannotUpgradeLinkTooHigh);
            }
            for id in [link.endpoint1, link.endpoint2] {
                if self.pin(id).is_none() {
                    return Err(PiError::PinDoesNotExist);
                }
            }
            let key = (link.endpoint1, link.endpoint2);
            if seen.contains(&key) {
                return Err(PiError::LinkAlreadyExists);
            }
            seen.push(key);
        }

        // Bandwidth is summed per link over every route that crosses it, and
        // compared against that link's capacity. Checking each route against
        // each link in isolation would pass a colony whose two routes
        // together overload one link, which is the only way a player ever
        // actually overloads one.
        let mut per_link: BTreeMap<(i64, i64), f64> = BTreeMap::new();
        for route in &self.routes {
            self.validate_route(statics, route, owner_id)?;
            let usage = self.route_bandwidth(statics, route);
            for pair in route.path.windows(2) {
                let (lo, hi) = if pair[0] <= pair[1] {
                    (pair[0], pair[1])
                } else {
                    (pair[1], pair[0])
                };
                *per_link.entry((lo, hi)).or_insert(0.0) += usage;
            }
        }
        for ((a, b), usage) in per_link {
            let link = self
                .link(a, b)
                .ok_or(PiError::RouteFailedValidationLinkDoesNotExist)?;
            // `+ 1e-9`: the capacity is a power of two times a dogma float
            // and the usage is a division by a tick count, so an exactly-full
            // link lands either side of its capacity depending on the
            // rounding. The client compares in Python floats and hits the
            // same thing; refusing an exactly-full link would refuse the
            // route the client's own planner proposed.
            if usage > link.bandwidth(statics) + 1e-9 {
                return Err(PiError::RouteFailedValidationCannotRouteCommodities);
            }
        }

        // The power budget last, and only when there is a command centre to
        // supply it -- `validateColonyCpuPower` returns early otherwise.
        if command_pins == 1 {
            let (cpu_supply, cpu_usage, power_supply, power_usage) = self.cpu_power(statics);
            if cpu_usage > cpu_supply {
                return Err(PiError::CannotAddToColonyCpuUsageExceeded);
            }
            if power_usage > power_supply {
                return Err(PiError::CannotAddToColonyPowerUsageExceeded);
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Every colony the world holds
// ---------------------------------------------------------------------------

/// The planetary-interaction half of the world.
///
/// One named type so that wiring it into `Registry` is a single field rather
/// than four, and so that the id allocators cannot drift apart from the
/// colonies they allocate for.
///
/// **Durable, and nothing in it is derivable from `static.db`** -- which puts
/// the whole of it on the `world.db` side of `docs/reference/world-state.md` §1. The
/// static table, which is the other side, is not a field: it is `Content`'s
/// ([`statics_in`]).
#[derive(Debug, Clone, Default)]
pub struct Colonies {
    /// Keyed by `(planetID, ownerID)`, because a planet holds one colony per
    /// character and several characters may share a planet --
    /// `GetCommandPinsForPlanet` exists precisely to list the others'.
    colonies: BTreeMap<(i64, i64), Colony>,
    /// Pin ids. **Must be `>= 100000000`** for the same reason
    /// `Character::ship_id` must be: below that the client routes an item
    /// lookup to its own sqlite before it ever asks us
    /// (`docs/reference/world-state.md` §2.4). EvEJS starts its at
    /// `900000000000`, which is the range Tranquility uses, and a pin id is
    /// an item id -- the command centre's own -- so it has to be in it.
    next_pin_id: i64,
    next_route_id: i64,
    /// Lane clock: the pace [`Self::advance`] and [`Self::apply`] run every
    /// colony at. Not state.
    pace: crate::game_clock::GameClock,
}

/// Where pin ids start. In the range Tranquility uses, and well clear of the
/// client's local-sqlite cutoff.
pub const FIRST_PIN_ID: i64 = 900_000_000_000;

impl Colonies {
    /// An empty world. It holds no static table: every call that simulates
    /// takes the `Content`'s ([`statics_in`], CP60), so a world can never
    /// run against a stale copy.
    pub fn new() -> Self {
        Colonies {
            colonies: BTreeMap::new(),
            next_pin_id: FIRST_PIN_ID,
            next_route_id: 1,
            pace: crate::game_clock::GameClock::REAL,
        }
    }

    /// Lane clock: run every colony at `pace` from the next pass on
    /// (`crate::game_clock::set`).
    pub fn set_pace(&mut self, pace: crate::game_clock::GameClock) {
        self.pace = pace;
    }

    pub fn get_mut(&mut self, planet_id: i64, owner_id: i64) -> Option<&mut Colony> {
        self.colonies.get_mut(&(planet_id, owner_id))
    }

    /// The durable half: every colony and both id allocators. The static
    /// table is not here: it is regenerable and lives in `Content`.
    pub fn saved(&self) -> Saved {
        Saved {
            colonies: self.colonies.values().cloned().collect(),
            next_pin_id: self.next_pin_id,
            next_route_id: self.next_route_id,
        }
    }

    /// Replace the colonies and allocators with stored ones. The allocators take the `max` with what the colonies already
    /// use, for the reason `item::Store::restore` does: an allocator that
    /// trails its own rows hands out a live id.
    pub fn restore(&mut self, saved: Saved) {
        let mut next_pin_id = saved.next_pin_id.max(FIRST_PIN_ID);
        let mut next_route_id = saved.next_route_id.max(1);
        self.colonies.clear();
        for c in saved.colonies {
            for p in &c.pins {
                if p.pin_id >= FIRST_PIN_ID {
                    next_pin_id = next_pin_id.max(p.pin_id + 1);
                }
            }
            for r in &c.routes {
                next_route_id = next_route_id.max(r.route_id + 1);
            }
            self.colonies.insert((c.planet_id, c.owner_id), c);
        }
        self.next_pin_id = next_pin_id;
        self.next_route_id = next_route_id;
    }

    pub fn get(&self, planet_id: i64, owner_id: i64) -> Option<&Colony> {
        self.colonies.get(&(planet_id, owner_id))
    }

    /// Every colony on one planet, whoever owns it.
    pub fn on_planet(&self, planet_id: i64) -> Vec<&Colony> {
        self.colonies
            .range((planet_id, i64::MIN)..=(planet_id, i64::MAX))
            .map(|(_, c)| c)
            .collect()
    }

    /// Every colony one character owns, in planet order.
    pub fn for_owner(&self, owner_id: i64) -> Vec<&Colony> {
        self.colonies
            .values()
            .filter(|c| c.owner_id == owner_id)
            .collect()
    }

    pub fn insert(&mut self, colony: Colony) {
        self.colonies
            .insert((colony.planet_id, colony.owner_id), colony);
    }

    pub fn remove(&mut self, planet_id: i64, owner_id: i64) -> Option<Colony> {
        self.colonies.remove(&(planet_id, owner_id))
    }

    pub fn is_empty(&self) -> bool {
        self.colonies.is_empty()
    }

    /// Advance one colony to `now` and say what happened.
    ///
    /// **This is the only thing the read path calls**, and it does no I/O --
    /// which is what makes it legal on the synchronous draw path that
    /// `docs/reference/world-state.md` §4 rule 4 protects.
    pub fn advance(
        &mut self,
        statics: &Statics,
        planet_id: i64,
        owner_id: i64,
        now: i64,
    ) -> Option<SimReport> {
        let colony = self.colonies.get_mut(&(planet_id, owner_id))?;
        colony.set_pace(self.pace);
        Some(colony.run_simulation(statics, now))
    }

    /// Apply a command stream to one colony, creating it if the stream starts
    /// with a command centre.
    pub fn apply(
        &mut self,
        statics: &Statics,
        edit: &ColonyEdit<'_>,
        commands: &[Command],
    ) -> Result<ApplyReport, PiError> {
        let key = (edit.planet_id, edit.owner_id);
        let mut colony = self.colonies.get(&key).cloned().unwrap_or_else(|| {
            Colony::new(edit.planet_id, edit.owner_id, edit.planet_type_id, edit.now)
        });
        colony.planet_type_id = edit.planet_type_id;
        colony.planet_radius = edit.planet_radius;
        let mut next_pin_id = self.next_pin_id;
        let mut next_route_id = self.next_route_id;
        let report = {
            let mut ctx = ApplyContext {
                owner_id: edit.owner_id,
                now: edit.now,
                pace: self.pace,
                next_pin_id: &mut next_pin_id,
                next_route_id: &mut next_route_id,
                link_type_id: edit.link_type_id,
                quote: edit.quote,
            };
            colony.apply(statics, commands, &mut ctx)?
        };
        self.next_pin_id = next_pin_id;
        self.next_route_id = next_route_id;
        // A colony that ended up with no pins at all is not a colony. The
        // client's own `UserAbandonPlanet` is the explicit path, but
        // removing the last pin reaches the same state and leaving an empty
        // shell behind would make `GetPlanetsForChar` list a planet the
        // player has nothing on.
        if colony.pins.is_empty() {
            self.colonies.remove(&key);
        } else {
            self.colonies.insert(key, colony);
        }
        Ok(report)
    }
}

/// Which colony a command stream is for, and the planet facts it needs.
///
/// A struct rather than seven positional arguments, because six of the seven
/// are `i64` or `f64` and a transposed pair would be a colony on the wrong
/// planet with no type error -- the same reason `Request` carries its own
/// fields by name.
pub struct ColonyEdit<'a> {
    pub planet_id: i64,
    pub owner_id: i64,
    pub planet_type_id: i64,
    /// Metres. Only link lengths use it, so a zero makes every link free.
    pub planet_radius: f64,
    /// `service::filetime_now()`.
    pub now: i64,
    /// The one member of `groupPlanetaryLinks`; see `service/planet.rs`.
    pub link_type_id: i64,
    pub quote: &'a dyn ProgramQuote,
}

// ---------------------------------------------------------------------------
// Customs offices: the only way goods reach a planet, and one way they leave
// ---------------------------------------------------------------------------

/// `invConst.groupPlanetaryCustomsOffices`. **[V]**
/// `inventorycommon/const.pyc:883`; `planetInfoSvc.GetPocosForPlanet` files
/// an orbital under it, and `baseCheckers.IsCustomsOffice` tests it.
pub const GROUP_PLANETARY_CUSTOMS_OFFICES: i64 = 1025;
/// `const.categoryOrbital`. **[V]** `inventorycommon/const.pyc:347`;
/// `planetInfoSvc.DoBallsAdded` tracks an orbital only if
/// `idCheckers.IsOrbital(slimItem.categoryID)`.
pub const CATEGORY_ORBITAL: i64 = 46;
/// `const.ownerInterBus`, who owns every office no corporation holds.
/// **[V]** `inventorycommon/const.pyc:3498`.
pub const OWNER_INTERBUS: i64 = 1_000_148;
/// `entityConst.STATE_ANCHORED`. **[V]** `eve/common/script/mgt/
/// entityConst.pyc:20`. An orbital's bracket reads `slimItem.orbitalState`
/// unguarded (`inSpaceBracket.UpdateOrbitalState:1633`, called for every
/// category-46 bracket by `bracketMgr.AddBracketIfWanted:697`), so the slim
/// must carry one. Which state an operating InterBus office is in is
/// **[A]**: anchored is EvEJS's choice too (**[V2]**), and it draws no
/// progress bar.
pub const ORBITAL_STATE_ANCHORED: i64 = -4;
/// `importTaxMultiplier`'s `defaultValue`. **[V]** `dogmaattributes` 1640.
pub const DEFAULT_IMPORT_TAX_MULTIPLIER: f64 = 1.0;
/// `const.flagHangar`, the flag every item in an office has: the import
/// window lists `customsOffice.List(const.flagHangar)` and skips any other
/// flag (`importExportUI.SetCustomsOfficeContent:351-354`). **[V]**
pub const CUSTOMS_OFFICE_FLAG: i64 = 4;

/// `const.categoryCommodity`, `groupGeneral`, `categoryPlanetaryResources`,
/// `categoryPlanetaryCommodities`. **[V]** `inventorycommon/const.pyc:338,
/// :673, :353, :351`.
const CATEGORY_COMMODITY: i64 = 17;
const GROUP_GENERAL: i64 = 280;
const CATEGORY_PLANETARY_RESOURCES: i64 = 42;
const CATEGORY_PLANETARY_COMMODITIES: i64 = 43;

/// `planetCommon.CanPutTypeInCustomsOffice` **[V]**
/// (`planetCommon.pyc:170-180`): a PI resource or commodity, or a
/// commodity of `groupGeneral` -- and nothing else.
pub fn can_put_type_in_customs_office(group_id: i64, category_id: i64) -> bool {
    match category_id {
        CATEGORY_PLANETARY_RESOURCES | CATEGORY_PLANETARY_COMMODITIES => true,
        CATEGORY_COMMODITY => group_id == GROUP_GENERAL,
        _ => false,
    }
}

/// **[A]** Where office ids start. An office is not in the item store -- it
/// is furniture, one per planet, derived from the planet -- so its id is a
/// pure function of the planet's: `FIRST_CUSTOMS_OFFICE_ID + planetID`.
///
/// The band is chosen by what it must avoid, all of it read from this
/// crate: above `MIN_PLAYER_ITEM` (the client answers anything below
/// 90,000,000 from its own sqlite), above the NPC band
/// (`npc::END_NPC_ID`, 8,000,000,000 exclusive) and **below
/// `item::ESCROW_BASE`** (9,000,000,000), because `item::is_escrow` is a
/// plain `>=` and every item put in an office has the office as its
/// location. Tranquility's own ids are ~10^12; EvEJS uses `1.2e12 +
/// planetID` (**[V2]**), which would read as escrow here.
pub const FIRST_CUSTOMS_OFFICE_ID: i64 = 8_000_000_000;

/// **[A]** How far beyond the planet's warp-in point the office floats, in
/// metres: a ship that warps to the planet lands 12 km off the office's
/// 8 km hull. Tranquility's offices sit in the planet's grid at positions
/// the client does not ship; nothing reads this but the ball.
pub const CUSTOMS_OFFICE_STANDOFF: f64 = 20_000.0;

/// The office over `planet_id`.
pub fn customs_office_id(planet_id: i64) -> i64 {
    FIRST_CUSTOMS_OFFICE_ID + planet_id
}

/// The planet an office id names, if it is in the office band at all.
/// Whether that planet exists is the catalogue's question
/// ([`customs_office`]).
pub fn customs_office_planet(item_id: i64) -> Option<i64> {
    let planet = item_id.checked_sub(FIRST_CUSTOMS_OFFICE_ID)?;
    // Planet ids are 40,000,000-50,000,000 on Tranquility's map.
    (40_000_000..50_000_000).contains(&planet).then_some(planet)
}

/// One customs office: an InterBus office anchored over one planet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CustomsOffice {
    pub item_id: i64,
    pub planet_id: i64,
    pub solar_system_id: i64,
    pub type_id: i64,
    pub position: [f64; 3],
    pub radius: f64,
    /// Cubic metres **per character**: the import window's capacity bar is
    /// `customsOffice.GetCapacity()` over the viewer's own items, and an
    /// office is a shared building in which everybody has their own hangar
    /// (**[A]** on the per-character split; the figure is the type's).
    pub capacity: f64,
    pub tax_rate: f64,
}

impl CustomsOffice {
    fn over(planet: &crate::celestial::Celestial, solar_system_id: i64, t: &CustomsType) -> Self {
        CustomsOffice {
            item_id: customs_office_id(planet.id),
            planet_id: planet.id,
            solar_system_id,
            type_id: t.type_id,
            position: customs_office_position(planet),
            radius: t.radius,
            capacity: t.capacity,
            tax_rate: t.tax_rate,
        }
    }

    /// What approach, orbit and warp resolve this office to. No warp-in
    /// point: `HasWarpInPoint` needs category 2 and a radius of 90 km
    /// (`crate::warp_in`) **[V]**, and an office is category 46 of 8 km.
    pub fn target(&self) -> crate::model::Target {
        crate::model::Target {
            position: self.position,
            radius: self.radius,
            warp_in: None,
        }
    }

    /// The office as a ball: a body that never moves, like a planet's
    /// (`destiny::celestial_ball`) -- not free, `Mode::Stop`, no mass.
    pub fn ball(&self) -> crate::destiny::Ball {
        use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION};
        Ball {
            ball_id: self.item_id,
            mode: Mode::Stop,
            radius: self.radius as f32,
            position: self.position,
            flags: FLAGS_STATIC_BALL,
            mass: 0.0,
            cloak: 0,
            harmonic: -1,
            corporation_id: OWNER_INTERBUS as i32,
            alliance_id: -1,
            physics: None,
            formation_id: NO_FORMATION,
        }
    }

    /// The office's slim item, a `foo.SlimItem` as `SetState` wants one.
    ///
    /// **The one wire shape in this file**, kept here rather than in
    /// `destiny.rs` because every attribute beyond the first five is an
    /// office's own and read by PI code:
    ///
    /// - `planetID`: `planetInfoSvc._AddOrbital:158` files the orbital under
    ///   it, `GetPocosForPlanet` finds it there for the planet's "Customs
    ///   Office" submenu, and `PlanetaryImportExportUI.ApplyAttributes:112`
    ///   raises if it is `None`. **[V]**
    /// - `level`: read by `CustomsOffice.LoadModel:34` (`spaceObject/
    ///   customsOffice.pyc`) and stored by the import window (`:110`)
    ///   **[V]**; a level that is not 1 or 2 draws the type's own graphic.
    ///   `0` is **[A]** (EvEJS's, **[V2]**).
    /// - `orbitalState`, and the three the bracket reads beside it when the
    ///   state is anchored -- `orbitalTimestamp`, `orbitalHackerID`,
    ///   `orbitalHackerProgress` (`inSpaceBracket.pyc:1644-1670`) **[V]**.
    ///
    /// `groupID` and `categoryID` are not sent: the client derives them from
    /// `typeID`, as it does for every slim (`destiny::rock_slim`).
    pub fn slim(&self) -> eve_marshal::PyValue {
        use eve_marshal::PyValue;
        crate::pyutil::instance(
            "foo.SlimItem",
            vec![
                ("itemID", PyValue::Int(self.item_id)),
                ("typeID", PyValue::Int(self.type_id)),
                ("ownerID", PyValue::Int(OWNER_INTERBUS)),
                ("charID", PyValue::None),
                // **[A]** The owning corporation, as for a station.
                ("corpID", PyValue::Int(OWNER_INTERBUS)),
                ("planetID", PyValue::Int(self.planet_id)),
                ("level", PyValue::Int(0)),
                ("orbitalState", PyValue::Int(ORBITAL_STATE_ANCHORED)),
                ("orbitalTimestamp", PyValue::None),
                ("orbitalHackerID", PyValue::None),
                ("orbitalHackerProgress", PyValue::None),
            ],
        )
    }
}

/// **[A]** The office's position: on the line from the planet's centre
/// through its warp-in point, [`CUSTOMS_OFFICE_STANDOFF`] beyond it. A
/// planet small enough to have no warp-in point (none in this sandbox)
/// gets one planet radius plus 1,000 km out along +x.
fn customs_office_position(planet: &crate::celestial::Celestial) -> [f64; 3] {
    let c = planet.position;
    let w = crate::warp_in::warp_in_point(planet).unwrap_or([
        c[0] + planet.radius + 1_000_000.0,
        c[1],
        c[2],
    ]);
    let d = [w[0] - c[0], w[1] - c[1], w[2] - c[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len <= 0.0 {
        return [w[0] + CUSTOMS_OFFICE_STANDOFF, w[1], w[2]];
    }
    let k = CUSTOMS_OFFICE_STANDOFF / len;
    [w[0] + d[0] * k, w[1] + d[1] * k, w[2] + d[2] * k]
}

/// Every office in one solar system, in planet-id order: one over **every
/// planet** the catalogue serves there (**[A]**: InterBus anchors one at
/// each planet of high-security space; nothing in the client lists them).
/// Empty when the static file named no office type.
pub fn customs_offices_in(
    catalogue: &crate::celestial::Catalogue,
    statics: &Statics,
    solar_system_id: i64,
) -> Vec<CustomsOffice> {
    let Some(t) = statics.customs_type() else {
        return Vec::new();
    };
    catalogue
        .served_in(solar_system_id)
        .filter(|c| c.kind == crate::celestial::Kind::Planet)
        .map(|c| CustomsOffice::over(c, solar_system_id, t))
        .collect()
}

/// One office by its item id, or `None` for an id that names no office.
pub fn customs_office(
    catalogue: &crate::celestial::Catalogue,
    statics: &Statics,
    item_id: i64,
) -> Option<CustomsOffice> {
    let t = statics.customs_type()?;
    let planet = catalogue.get(customs_office_planet(item_id)?)?;
    if planet.kind != crate::celestial::Kind::Planet || !catalogue.is_served(planet.id) {
        return None;
    }
    let system = catalogue.system_of(planet.id).unwrap_or(0);
    Some(CustomsOffice::over(planet, system, t))
}

impl Colony {
    /// The endpoint an import or export names: one of this owner's
    /// **launchpads**. `BaseColony.GetImportEndpoints` lists the pins that
    /// `IsSpaceport()` and are **not** `IsCommandCenter()`
    /// (`baseColony.pyc:1218-1226`) **[V]** -- so a command centre, which
    /// launches, does not import or export.
    fn customs_endpoint(
        &mut self,
        statics: &Statics,
        owner_id: i64,
        pin_id: i64,
        not_found: PiError,
        not_spaceport: PiError,
    ) -> Result<&mut Pin, PiError> {
        let pin = self.pin_mut(pin_id).ok_or(not_found)?;
        if pin.owner_id != owner_id {
            return Err(PiError::CanOnlyManageOwnPins);
        }
        if pin.kind(statics) != Some(PinKind::Spaceport) {
            return Err(not_spaceport);
        }
        Ok(pin)
    }

    /// Commodities arriving from a customs office into launchpad `pin_id`.
    ///
    /// All or nothing: `SpaceportPin.CanImportCommodities` refuses when
    /// `capacityUsed + volume > GetCapacity()` (`spaceportPin.pyc:86-90`)
    /// **[V]**, and a short accept after that test (a float edge) refuses
    /// too rather than keep part -- the caller runs this on a copy, so a
    /// refusal changes nothing and nothing is lost.
    pub fn import_commodities(
        &mut self,
        statics: &Statics,
        owner_id: i64,
        pin_id: i64,
        commodities: &BTreeMap<i64, i64>,
    ) -> Result<(), PiError> {
        let pin = self.customs_endpoint(
            statics,
            owner_id,
            pin_id,
            PiError::CannotImportEndpointNotFound,
            PiError::CannotImportEndpointNotSpaceport,
        )?;
        let volume: f64 = commodities
            .iter()
            .map(|(t, q)| statics.volume(*t) * *q as f64)
            .sum();
        let capacity = pin.capacity(statics).unwrap_or(0.0);
        if pin.used_volume(statics) + volume > capacity {
            return Err(PiError::CannotImportNotEnoughWarehouseSpace);
        }
        for (type_id, qty) in commodities {
            if pin.add_commodity(statics, *type_id, *qty) != *qty {
                return Err(PiError::CannotImportNotEnoughWarehouseSpace);
            }
        }
        Ok(())
    }

    /// Commodities leaving launchpad `pin_id` for a customs office. Every
    /// type must be there in full, or nothing moves.
    pub fn export_commodities(
        &mut self,
        statics: &Statics,
        owner_id: i64,
        pin_id: i64,
        commodities: &BTreeMap<i64, i64>,
    ) -> Result<(), PiError> {
        let pin = self.customs_endpoint(
            statics,
            owner_id,
            pin_id,
            PiError::CannotExportEndpointNotFound,
            PiError::CannotExportEndpointNotSpaceport,
        )?;
        for (type_id, qty) in commodities {
            let contained = pin.quantity(*type_id);
            if contained < 1 {
                return Err(PiError::CannotExportCommodityNotFound { type_id: *type_id });
            }
            if contained < *qty {
                return Err(PiError::CannotExportCommodityNotEnough {
                    type_id: *type_id,
                    contained,
                    desired: *qty,
                });
            }
        }
        for (type_id, qty) in commodities {
            pin.remove_commodity(*type_id, *qty);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The static table, in `Content` (CP60)
// ---------------------------------------------------------------------------

/// The registration row in `content_tables::TABLES` (coordinator-only).
///
/// Ruling 3 (`docs/content/plan.md` §1.5, CA7): this replaces the two
/// process statics that held the PI table and its item types. The table is
/// an [`Installed`]: the [`Statics`] and the item types built from its rows.
pub const STATICS: TableSpec = TableSpec {
    name: "planet",
    load,
    fixture: || Box::new(Installed::build(fixture())) as Boxed,
};

/// The PI static table as `Content` holds it: the [`Statics`] and the
/// `item::ItemType`s of its item rows.
///
/// The item types are leaked once per build, because `ItemType`'s name is
/// `&'static str` (it was designed for a const table) and the item store
/// hands out `&'static ItemType`. A bundle builds this once; every
/// `Content::fixture()` builds the four fixture rows again, the trade
/// `typedata::Installed` makes for the same reason.
#[derive(Debug, Clone)]
pub struct Installed {
    statics: Statics,
    items: &'static [ItemType],
}

impl Installed {
    /// The table, and an item type for each of its item rows.
    ///
    /// Every row is a **stack** type (`singleton: false`): a command centre
    /// is bought, hauled and stacked packaged, and a commodity is a
    /// commodity.
    pub fn build(statics: Statics) -> Installed {
        let items: Vec<ItemType> = statics
            .item_rows()
            .iter()
            .map(|r| ItemType {
                type_id: r.type_id,
                name: Box::leak(r.name.clone().into_boxed_str()),
                group_id: r.group_id,
                category_id: r.category_id,
                singleton: false,
                volume: r.volume,
                capacity: r.capacity,
            })
            .collect();
        Installed {
            statics,
            items: Box::leak(items.into_boxed_slice()),
        }
    }

    pub fn statics(&self) -> &Statics {
        &self.statics
    }

    /// The item store's rows for every command centre and commodity.
    pub fn item_types(&self) -> &'static [ItemType] {
        self.items
    }
}

/// The bundle's `planets/pi-statics.tsv`, or [`fixture`] without a bundle.
///
/// A bundle without the file is an empty table: no colony can be built,
/// which is what a server without `--planets` had before. A file that is
/// there but malformed, or has no pin type or no schematic, is an `Err`
/// naming it ([`from_file`]).
fn load(src: &Sources) -> Result<Boxed, String> {
    let Some(bundle) = src.bundle else {
        return Ok(Box::new(Installed::build(fixture())));
    };
    src.note(Origin::Bundle);
    let Some(path) = bundle.path(crate::content::PI_STATICS) else {
        return Ok(Box::new(Installed::build(Statics::new())));
    };
    Ok(Box::new(from_file(&path)?))
}

/// One static file, built. Fatal, as `--planets` always was: a PI table
/// that failed to load is a planet window that refuses every pin with no
/// explanation. The error names the file.
pub fn from_file(path: &std::path::Path) -> Result<Installed, String> {
    let statics = Statics::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (pins, schematics, _, _) = statics.counts();
    if pins == 0 || schematics == 0 {
        return Err(format!(
            "{}: no pin types or no schematics. Regenerate it with tools/pi_statics.py",
            path.display()
        ));
    }
    Ok(Installed::build(statics))
}

/// `--planets FILE`: replace the bundle's (or the fixture's) table with this
/// file's, before the world is built. Returns the startup line.
pub fn override_in(content: &mut Content, path: &std::path::Path) -> Result<String, String> {
    let t = from_file(path).map_err(|e| format!("--planets {e}"))?;
    let (pins, schematics, planet_types, items) = t.statics.counts();
    content.insert(STATICS.name, t);
    Ok(format!(
        "PI: {pins} pin types, {schematics} schematics, {planet_types} planet types, \
         {items} item types from {}",
        path.display()
    ))
}

/// The content table. Panics, as `Content::get` does, when [`STATICS`] is
/// not registered.
pub fn installed_in(content: &Content) -> &Installed {
    content.get::<Installed>()
}

/// The static table every colony simulates against.
pub fn statics_in(content: &Content) -> &Statics {
    &installed_in(content).statics
}

/// The command centres and commodities, for the item store
/// (`item::find_with`).
pub fn item_types_in(content: &Content) -> &'static [ItemType] {
    installed_in(content).items
}

/// A `Content` of fixtures with `statics` as the PI table: for a test that
/// needs rows [`fixture`] does not have.
#[cfg(test)]
pub(crate) fn content_with(statics: Statics) -> Content {
    let mut c = Content::fixture();
    c.insert(STATICS.name, Installed::build(statics));
    c
}

/// [`FIXTURE`], parsed.
pub fn fixture() -> Statics {
    Statics::parse(FIXTURE).expect("the planet fixture parses")
}

/// Invented PI rows: the table without a bundle, and every planet test's.
///
/// **Not the client's ids**: types from 990_600_001 up (`typedata`'s
/// fixture is 990_000_001 up), schematic 99_061, planet type 990_600_001.
/// Group ids and attribute ids are protocol vocabulary (class A,
/// [`attr`]). The numbers are the test colony's and claim nothing; every
/// test asserts a behaviour, not a value.
///
/// No `customsoffice` record, on purpose: with one, every test world with a
/// planet would grow an office ball. A test that wants offices adds the
/// type ([`Statics::with_customs_type`]).
pub const FIXTURE: &str = "\
# planet::FIXTURE: invented rows, ids from 990600001 (not the client's)
pintype\t990600011\t1027\t500.0\t0.0\t0\t48=1675.0;11=6000.0;1639=3.0
pintype\t990600012\t1063\t0.0\t0.0\t0\t49=400.0;15=2600.0;1690=110.0;1691=550.0;1683=0.012;1687=0.8;1689=0.3;1632=990600001.0
pintype\t990600013\t1029\t12000.0\t0.0\t0\t49=500.0;15=700.0
pintype\t990600014\t1030\t10000.0\t0.0\t0\t49=3600.0;15=700.0;1638=0.5;1639=1.0
pintype\t990600015\t1028\t0.0\t0.0\t0\t49=200.0;15=800.0
pintype\t990600016\t1036\t0.0\t0.0\t0\t49=15.0;1634=0.2;1635=1.4;15=10.0;1633=0.15;1636=1.2;1631=1250.0
schematic\t99061\t1800\t990600015\t990600021=3000\t990600022=20
itemtype\t990600011\t1027\t41\t1000.0\t500.0\tFixture Command Centre
itemtype\t990600021\t1032\t42\t0.005\t0.0\tFixture Resource
itemtype\t990600022\t1042\t43\t0.19\t0.0\tFixture Product
itemtype\t990600023\t1034\t43\t0.38\t0.0\tFixture Taxed Product
taxmult\t990600023\t400.0
";

// ---------------------------------------------------------------------------
// Extraction programs: baseColony.CreateProgram
// ---------------------------------------------------------------------------

/// The resource density at one point of a planet's surface.
///
/// On Tranquility this is a spherical-harmonic field the **server** authors
/// and the client evaluates natively (`_eveplanetresources.GetValueAt`,
/// `docs/lanes/planet.md` §5). We have no such field, so the one impl here is
/// [`UniformField`] -- an assumption, and it is behind this trait so that a
/// real field replaces it without touching [`create_program`].
pub trait ResourceField {
    fn value_at(&self, resource_type_id: i64, point: SurfacePoint) -> f64;
}

/// **[A]** The head value a resource of quality `1.0` gives everywhere on the
/// planet.
///
/// Chosen, not read: the client's field is server-authored and we have none.
/// The number is picked so a ten-head ECU on a quality-1.0 resource quotes a
/// `qtyPerCycle` of `ecuMaxVolume * 10 * 15 = 1380` before overlap -- which,
/// through the client's own decay curve, is a few thousand units per
/// two-hour cycle, the order of magnitude players know. The morning test can
/// retune it; nothing else depends on its value.
pub const ASSUMED_FIELD_VALUE_AT_FULL_QUALITY: f64 = 15.0;

/// **[A]** A field that is flat across the whole planet: every point is worth
/// `quality * ASSUMED_FIELD_VALUE_AT_FULL_QUALITY` of each resource, with the
/// quality the planet *type* lists in `globaltypes` **[V]**.
///
/// So a resource the planet type does not carry is worth nothing anywhere,
/// which is the one property of the real field this keeps exactly, and
/// where you put the heads does not matter except through overlap -- which
/// is the one property it loses.
pub struct UniformField<'a> {
    pub qualities: Option<&'a BTreeMap<i64, f64>>,
}

impl ResourceField for UniformField<'_> {
    fn value_at(&self, resource_type_id: i64, _point: SurfacePoint) -> f64 {
        self.qualities
            .and_then(|q| q.get(&resource_type_id))
            .copied()
            .unwrap_or(0.0)
            * ASSUMED_FIELD_VALUE_AT_FULL_QUALITY
    }
}

/// What a program would yield: `baseColony.CreateProgram`'s first three
/// return values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramResult {
    /// `int(maxValue)` -- the base per-cycle yield the decay curve scales.
    pub qty_per_cycle: i64,
    /// FILETIME ticks.
    pub cycle_time: i64,
    pub num_cycles: i64,
}

/// `baseColony.CreateProgram(harmonic, ecuPinID, resourceTypeID, points,
/// headRadius)`, ported. **[V]** line for line, `baseColony.pyc:1328-1437`,
/// except that `builder.GetValueAt` is `field`.
///
/// - Each head's value is the field at the head, floored at zero.
/// - **Own overlap**: every pair of this ECU's heads closer than two radii
///   loses `overlap * overlapFactor` of its value, where `overlap` is the
///   lens area of two equal circles over one circle's area.
/// - **Others' overlap**: every head of *another* ECU in the colony on the
///   same resource cuts this head by `2 * overlap * overlapFactor`, the lens
///   of two unequal circles this time, with the client's own guards for a
///   zero distance (`0.0001`) and a zero radius (`RADIUS_DRILLAREAMAX`).
/// - `qtyPerCycle = int(ecuMaxVolume * sum(values))`.
///
/// Distances are angles on the unit sphere: `SurfacePoint`'s default radius
/// is `1.0`, and `headRadius` is compared against them directly.
///
/// `ecu_pin_id` is excluded from the "other ECUs" walk; it may be a pin the
/// colony does not hold yet (a temporary id in an edit session), which is
/// why the heads come in as an argument rather than off the pin.
#[allow(clippy::too_many_arguments)]
pub fn create_program(
    statics: &Statics,
    colony: &Colony,
    ecu_pin_id: i64,
    ecu_type_id: i64,
    resource_type_id: i64,
    heads: &[Head],
    head_radius: f64,
    field: &dyn ResourceField,
) -> ProgramResult {
    let ecu = statics.pin_type(ecu_type_id);
    let overlap_factor = ecu.map_or(ecu_default::OVERLAP_FACTOR, |t| {
        t.attributes
            .get(&attr::ECU_OVERLAP_FACTOR)
            .copied()
            .unwrap_or(ecu_default::OVERLAP_FACTOR)
    });
    let max_volume = ecu.map_or(ecu_default::MAX_VOLUME, |t| {
        t.attributes
            .get(&attr::ECU_MAX_VOLUME)
            .copied()
            .unwrap_or(ecu_default::MAX_VOLUME)
    });
    let points: Vec<(i64, SurfacePoint)> = heads.iter().map(|h| (h.head_id, h.point())).collect();
    let mut value: BTreeMap<i64, f64> = points
        .iter()
        .map(|(i, p)| (*i, field.value_at(resource_type_id, *p).max(0.0)))
        .collect();

    // OverlapOwnHeads.
    for (a, (ia, pa)) in points.iter().enumerate() {
        for (ib, pb) in points.iter().skip(a + 1) {
            let dist = pa.angle_to(*pb);
            if dist < head_radius * 2.0 {
                let r2 = head_radius * head_radius;
                let overlap = (2.0 * r2 * (0.5 * dist / head_radius).clamp(-1.0, 1.0).acos()
                    - 0.5 * dist * (4.0 * r2 - dist * dist).max(0.0).sqrt())
                    / (std::f64::consts::PI * r2);
                let modifier = (1.0 - overlap * overlap_factor).clamp(0.0, 1.0);
                *value.entry(*ia).or_default() *= modifier;
                *value.entry(*ib).or_default() *= modifier;
            }
        }
    }

    // OverlapOthersHeads.
    let others: Vec<(f64, Vec<SurfacePoint>)> = colony
        .pins
        .iter()
        .filter(|p| p.pin_id != ecu_pin_id && p.program_type == Some(resource_type_id))
        .filter(|p| p.kind(statics) == Some(PinKind::Ecu))
        .map(|p| (p.head_radius, p.heads.iter().map(Head::point).collect()))
        .collect();
    let head_area = std::f64::consts::PI * head_radius * head_radius;
    for (index, p1) in &points {
        let mut modifier = 1.0;
        for (other_radius, other_heads) in &others {
            if modifier == 0.0 {
                break;
            }
            for p2 in other_heads {
                let d = p1.angle_to(*p2);
                let d = if d == 0.0 { 0.0001 } else { d };
                let big_r = if head_radius == 0.0 {
                    RADIUS_DRILLAREAMAX
                } else {
                    head_radius
                };
                let r = if *other_radius == 0.0 {
                    RADIUS_DRILLAREAMAX
                } else {
                    *other_radius
                };
                if d > big_r + r {
                    continue;
                }
                let (r2, big_r2, d2) = (r * r, big_r * big_r, d * d);
                let alpha = (d2 + r2 - big_r2) / (2.0 * d * r);
                let overlap = if alpha < -1.0 {
                    std::f64::consts::PI * r2 / head_area
                } else if alpha > 1.0 {
                    1.0
                } else {
                    let f1 = r2 * alpha.acos();
                    let f2 = big_r2
                        * ((d2 + big_r2 - r2) / (2.0 * d * big_r))
                            .clamp(-1.0, 1.0)
                            .acos();
                    let f3 = 0.5
                        * ((-d + r + big_r) * (d + r - big_r) * (d - r + big_r) * (d + r + big_r))
                            .max(0.0)
                            .sqrt();
                    (f1 + f2 - f3) / head_area
                };
                modifier *= (1.0 - overlap * 2.0 * overlap_factor).clamp(0.0, 1.0);
            }
        }
        *value.entry(*index).or_default() *= modifier;
    }

    let max_value = max_volume * value.values().sum::<f64>();
    let length = program_length_from_head_radius(head_radius);
    let hours = cycle_time_from_program_length(length);
    let num_cycles = if hours > 0.0 {
        (length / hours).trunc() as i64
    } else {
        0
    };
    ProgramResult {
        qty_per_cycle: max_value.trunc() as i64,
        cycle_time: (hours * HOUR as f64).trunc() as i64,
        num_cycles,
    }
}

/// The quote `INSTALLPROGRAM` uses: [`create_program`] over the ECU's heads
/// as they stand after the rest of the stream, against a resource field.
pub struct FieldQuote<'a> {
    pub statics: &'a Statics,
    pub field: &'a dyn ResourceField,
}

impl ProgramQuote for FieldQuote<'_> {
    fn quote(&self, colony: &Colony, ecu_pin_id: i64, program_type: i64, head_radius: f64) -> i64 {
        let Some(pin) = colony.pin(ecu_pin_id) else {
            return 0;
        };
        create_program(
            self.statics,
            colony,
            ecu_pin_id,
            pin.type_id,
            program_type,
            &pin.heads,
            head_radius,
            self.field,
        )
        .qty_per_cycle
    }
}

// ---------------------------------------------------------------------------
// Launches and expedited transfers
// ---------------------------------------------------------------------------

/// `SpaceportPin.GetLaunchCycleTime` -- `60 * const.SEC`. **[V]**
/// `spaceportPin.pyc:38`.
pub const LAUNCH_CYCLE_TIME: i64 = 60 * SEC;

impl Colony {
    /// `planetMgr.UserLaunchCommodities(commandPinID, {typeID: qty})`, the
    /// colony half: take the commodities off the command centre and say what
    /// was taken. Where they go is the caller's business.
    ///
    /// The refusals and their order are `ClientPlanet.LaunchCommodities`'s
    /// **[V]** (`clientPlanet.pyc:541-562`), which the client runs before it
    /// calls us, so a refusal here means the two sides disagree about the
    /// colony rather than that the player clicked something wrong:
    ///
    /// - nothing selected is `PleaseSelectCommoditiesToLaunch`;
    /// - only a **command centre** launches (`CanOnlyLaunchFromCommandCenters`)
    ///   -- a launchpad's goods leave through a customs office;
    /// - once a minute (`CannotLaunchCommandPinNotReady`): `GetNextLaunchTime`
    ///   is `lastLaunchTime + 60 s`, `None` when never launched;
    /// - every type must be in the pin with at least one unit
    ///   (`CannotLaunchCommoditiesNotFound`) -- `SpaceportPin.CanLaunch`.
    ///
    /// `SpaceportPin.FetchLaunchCommodities` then removes **up to** each
    /// quantity: asking for more than is there launches what is there.
    pub fn launch(
        &mut self,
        statics: &Statics,
        owner_id: i64,
        pin_id: i64,
        commodities: &BTreeMap<i64, i64>,
        now: i64,
    ) -> Result<BTreeMap<i64, i64>, PiError> {
        if commodities.is_empty() {
            return Err(PiError::PleaseSelectCommoditiesToLaunch);
        }
        let pin = self
            .pin(pin_id)
            .filter(|p| p.kind(statics) == Some(PinKind::Command))
            .ok_or(PiError::CanOnlyLaunchFromCommandCenters)?;
        if pin.owner_id != owner_id {
            return Err(PiError::CanOnlyManageOwnPins);
        }
        if pin.last_launch_time != 0 && pin.last_launch_time + LAUNCH_CYCLE_TIME > now {
            return Err(PiError::CannotLaunchCommandPinNotReady);
        }
        if commodities.keys().any(|t| pin.quantity(*t) < 1) {
            return Err(PiError::CannotLaunchCommoditiesNotFound);
        }
        let pin = self.pin_mut(pin_id).expect("just found it");
        let mut launched = BTreeMap::new();
        for (type_id, qty) in commodities {
            let taken = pin.remove_commodity(*type_id, *qty);
            if taken > 0 {
                launched.insert(*type_id, taken);
            }
        }
        pin.last_launch_time = now;
        Ok(launched)
    }

    /// `BaseColony.ValidateExpeditedTransfer` then
    /// `ExecuteExpeditedTransfer`. **[V]** `baseColony.pyc:983-1050` and
    /// `:573-618`, refusals in the client's order.
    ///
    /// Moves what the destination will take, removes exactly that from the
    /// source, and puts the source on cooldown: its `lastRunTime` becomes
    /// `now + GetExpeditedTransferTime(minBandwidth, moved)`, which is what
    /// `StoragePin.GetNextTransferTime` reads. Returns what moved.
    pub fn expedited_transfer(
        &mut self,
        statics: &Statics,
        owner_id: i64,
        path: &[i64],
        commodities: &BTreeMap<i64, i64>,
        now: i64,
    ) -> Result<BTreeMap<i64, i64>, PiError> {
        if path.len() < 2 {
            return Err(PiError::CreateRouteTooShort);
        }
        if commodities.is_empty() {
            return Err(PiError::CreateRouteWithoutCommodities);
        }
        let source = self
            .pin(path[0])
            .ok_or(PiError::RouteFailedValidationPinDoesNotExist)?;
        let dest = self
            .pin(path[path.len() - 1])
            .ok_or(PiError::RouteFailedValidationPinDoesNotExist)?;
        if !source.kind(statics).is_some_and(PinKind::is_storage) {
            return Err(PiError::RouteFailedValidationExpeditedSourceNotStorage);
        }
        for (type_id, qty) in commodities {
            if !source.contents.contains_key(type_id) {
                return Err(PiError::RouteFailedValidationExpeditedSourceLacksCommodity);
            }
            if *qty > source.quantity(*type_id) {
                return Err(PiError::RouteFailedValidationExpeditedSourceLacksCommodityQty);
            }
            if dest.acceptable_quantity(statics, *type_id, *qty) < 1 {
                return Err(PiError::RouteFailedValidationExpeditedDestinationCannotAccept);
            }
        }
        let mut min_bandwidth: Option<f64> = None;
        for (i, id) in path.iter().enumerate() {
            let pin = self
                .pin(*id)
                .ok_or(PiError::RouteFailedValidationPinDoesNotExist)?;
            if pin.owner_id != owner_id {
                return Err(PiError::RouteFailedValidationPinNotYours);
            }
            if i == 0 {
                continue;
            }
            let link = self
                .link(path[i - 1], *id)
                .ok_or(PiError::RouteFailedValidationLinkDoesNotExist)?;
            let bandwidth = link.bandwidth(statics);
            if min_bandwidth.is_none_or(|m| bandwidth < m) {
                min_bandwidth = Some(bandwidth);
            }
        }
        let min_bandwidth = min_bandwidth.unwrap_or(0.0);
        if min_bandwidth <= 0.0 {
            return Err(PiError::RouteFailedValidationNoBandwidthAvailable);
        }
        // `StoragePin.CanTransfer`: the quantities again, then the cooldown,
        // which is `lastRunTime` itself.
        let source = self.pin(path[0]).expect("checked above");
        if source.last_run_time > now {
            return Err(PiError::RouteFailedValidationExpeditedSourceNotReady);
        }

        // ExecuteExpeditedTransfer.
        let (source_id, dest_id) = (path[0], path[path.len() - 1]);
        let mut to_push = BTreeMap::new();
        for (type_id, qty) in commodities {
            let n = source.quantity(*type_id).min(*qty);
            if n >= 1 {
                to_push.insert(*type_id, n);
            }
        }
        if to_push.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut moved = BTreeMap::new();
        {
            let dest = self.pin_mut(dest_id).expect("checked above");
            for (type_id, qty) in &to_push {
                moved.insert(*type_id, dest.add_commodity(statics, *type_id, *qty));
            }
        }
        let volume: f64 = moved
            .iter()
            .map(|(t, q)| statics.volume(*t) * *q as f64)
            .sum();
        let source = self.pin_mut(source_id).expect("checked above");
        for (type_id, qty) in &moved {
            source.remove_commodity(*type_id, *qty);
        }
        source.last_run_time = now + Colony::expedited_transfer_time(min_bandwidth, volume);
        moved.retain(|_, q| *q > 0);
        Ok(moved)
    }
}

// ---------------------------------------------------------------------------
// The world file
// ---------------------------------------------------------------------------

/// Every colony and both id allocators: what `store::Snapshot` keeps of PI.
///
/// **Without the statics**, which are regenerable from the client and are
/// loaded into `Content` at every start (`docs/reference/world-state.md` §1: static data is
/// never in the world file). Rendered and parsed here, like
/// `service::Bookmarks`, so `store.rs` holds it opaquely.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Saved {
    pub colonies: Vec<Colony>,
    pub next_pin_id: i64,
    pub next_route_id: i64,
}

impl Saved {
    /// The record tags this type owns in the world file.
    pub const RECORDS: &'static [&'static str] =
        &["picolony", "pipin", "pilink", "piroute", "pinext"];

    /// One line per colony, pin, link and route, then the allocators. A
    /// colony's line comes before its parts, which [`Saved::parse_record`]
    /// relies on.
    ///
    /// Floats are written with Rust's `{}`, which is the shortest text that
    /// parses back to the same `f64` -- a pin that moved by one ulp across a
    /// restart would change every link length it is part of.
    pub fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        fn opt(v: Option<i64>) -> i64 {
            v.unwrap_or(-1)
        }
        for c in &self.colonies {
            let _ = writeln!(
                out,
                "picolony\t{}\t{}\t{}\t{}\t{}\t{}",
                c.planet_id,
                c.owner_id,
                c.planet_type_id,
                c.planet_radius,
                c.level,
                c.current_sim_time
            );
            for p in &c.pins {
                let contents = p
                    .contents
                    .iter()
                    .map(|(t, q)| format!("{t}={q}"))
                    .collect::<Vec<_>>()
                    .join(";");
                let heads = p
                    .heads
                    .iter()
                    .map(|h| format!("{}={}={}", h.head_id, h.latitude, h.longitude))
                    .collect::<Vec<_>>()
                    .join(";");
                // The trailing `.` is a marker, so an empty `heads` is still
                // a field rather than a line that ends in a tab.
                let _ = writeln!(
                    out,
                    "pipin\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t.",
                    c.planet_id,
                    c.owner_id,
                    p.pin_id,
                    p.type_id,
                    p.owner_id,
                    p.latitude,
                    p.longitude,
                    p.last_run_time,
                    p.state,
                    p.last_launch_time,
                    opt(p.schematic_id),
                    u8::from(p.has_received_inputs),
                    u8::from(p.received_inputs_last_cycle),
                    p.cycle_time,
                    opt(p.program_type),
                    p.qty_per_cycle,
                    opt(p.expiry_time),
                    opt(p.install_time),
                    p.head_radius,
                    contents,
                    heads,
                );
            }
            for l in &c.links {
                let _ = writeln!(
                    out,
                    "pilink\t{}\t{}\t{}\t{}\t{}\t{}",
                    c.planet_id, c.owner_id, l.endpoint1, l.endpoint2, l.type_id, l.level
                );
            }
            for r in &c.routes {
                let path = r
                    .path
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let _ = writeln!(
                    out,
                    "piroute\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    c.planet_id,
                    c.owner_id,
                    r.route_id,
                    r.owner_id,
                    r.commodity_type_id,
                    r.commodity_quantity,
                    path
                );
            }
        }
        let _ = writeln!(out, "pinext\t{}\t{}", self.next_pin_id, self.next_route_id);
    }

    /// Read one record whose tag is in [`Saved::RECORDS`].
    pub fn parse_record(&mut self, f: &[&str]) -> Result<(), String> {
        fn int(f: &[&str], i: usize) -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("{}: field {i} is not an integer", f[0]))
        }
        fn real(f: &[&str], i: usize) -> Result<f64, String> {
            f.get(i)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("{}: field {i} is not a number", f[0]))
        }
        fn opt(v: i64) -> Option<i64> {
            (v != -1).then_some(v)
        }
        if f[0] == "pinext" {
            self.next_pin_id = int(f, 1)?;
            self.next_route_id = int(f, 2)?;
            return Ok(());
        }
        if f[0] == "picolony" {
            self.colonies.push(Colony {
                planet_id: int(f, 1)?,
                owner_id: int(f, 2)?,
                planet_type_id: int(f, 3)?,
                planet_radius: real(f, 4)?,
                level: int(f, 5)?,
                current_sim_time: int(f, 6)?,
                pins: Vec::new(),
                links: Vec::new(),
                routes: Vec::new(),
            });
            return Ok(());
        }
        let (planet, owner) = (int(f, 1)?, int(f, 2)?);
        let colony = self
            .colonies
            .iter_mut()
            .rev()
            .find(|c| c.planet_id == planet && c.owner_id == owner)
            .ok_or_else(|| format!("{}: no colony ({planet}, {owner}) before it", f[0]))?;
        match f[0] {
            "pipin" => {
                let mut contents = BTreeMap::new();
                for kv in f.get(20).copied().unwrap_or("").split(';') {
                    if let Some((t, q)) = kv.split_once('=') {
                        let t = t.parse::<i64>().map_err(|_| "pipin: bad contents")?;
                        let q = q.parse::<i64>().map_err(|_| "pipin: bad contents")?;
                        contents.insert(t, q);
                    }
                }
                let mut heads = Vec::new();
                for h in f.get(21).copied().unwrap_or("").split(';') {
                    let parts: Vec<&str> = h.split('=').collect();
                    if parts.len() == 3 {
                        let bad = || "pipin: bad head".to_owned();
                        heads.push(Head {
                            head_id: parts[0].parse().map_err(|_| bad())?,
                            latitude: parts[1].parse().map_err(|_| bad())?,
                            longitude: parts[2].parse().map_err(|_| bad())?,
                        });
                    }
                }
                colony.pins.push(Pin {
                    pin_id: int(f, 3)?,
                    type_id: int(f, 4)?,
                    owner_id: int(f, 5)?,
                    latitude: real(f, 6)?,
                    longitude: real(f, 7)?,
                    last_run_time: int(f, 8)?,
                    contents,
                    state: int(f, 9)?,
                    last_launch_time: int(f, 10)?,
                    schematic_id: opt(int(f, 11)?),
                    has_received_inputs: int(f, 12)? != 0,
                    received_inputs_last_cycle: int(f, 13)? != 0,
                    cycle_time: int(f, 14)?,
                    program_type: opt(int(f, 15)?),
                    qty_per_cycle: int(f, 16)?,
                    expiry_time: opt(int(f, 17)?),
                    install_time: opt(int(f, 18)?),
                    head_radius: real(f, 19)?,
                    heads,
                    pace: crate::game_clock::GameClock::REAL,
                });
            }
            "pilink" => colony.links.push(Link {
                endpoint1: int(f, 3)?,
                endpoint2: int(f, 4)?,
                type_id: int(f, 5)?,
                level: int(f, 6)?,
            }),
            "piroute" => {
                let path = f
                    .get(7)
                    .copied()
                    .unwrap_or("")
                    .split(',')
                    .filter(|p| !p.is_empty())
                    .map(|p| p.parse::<i64>().map_err(|_| "piroute: bad path".to_owned()))
                    .collect::<Result<Vec<_>, _>>()?;
                colony.routes.push(Route {
                    route_id: int(f, 3)?,
                    owner_id: int(f, 4)?,
                    commodity_type_id: int(f, 5)?,
                    commodity_quantity: int(f, 6)?,
                    path,
                });
            }
            other => return Err(format!("{other}: not a PI record")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Fixture
    // -----------------------------------------------------------------
    //
    // Every test here runs on `fixture()` (CP60): `FIXTURE`'s invented rows.
    // The names below say what each id is in that file. A Barren-like
    // colony: command centre, ECU, storage, launchpad, basic factory and a
    // link type, one R0 resource and one P1 product on one schematic.

    const PLANET_BARREN: i64 = 990_600_001;
    const TYPE_COMMAND_CENTRE: i64 = 990_600_011;
    const TYPE_ECU: i64 = 990_600_012;
    const TYPE_STORAGE: i64 = 990_600_013;
    const TYPE_LAUNCHPAD: i64 = 990_600_014;
    const TYPE_BASIC_FACTORY: i64 = 990_600_015;
    const TYPE_LINK: i64 = 990_600_016;

    /// The fixture's R0 resource and P1 product, and the schematic that
    /// turns 3000 of one into 20 of the other on a 1800-second cycle.
    const TYPE_AQUEOUS_LIQUIDS: i64 = 990_600_021;
    const TYPE_WATER: i64 = 990_600_022;
    const SCHEMATIC_WATER: i64 = 99_061;

    fn statics() -> Statics {
        fixture()
    }

    const OWNER: i64 = 90_000_001;
    /// An arbitrary but realistic epoch: a FILETIME somewhere in the 2020s.
    const T0: i64 = 133_000_000_000_000_000;
    /// A 15-minute extraction cycle, the shortest the game offers.
    const CYCLE_15M: i64 = 9_000_000_000;

    /// Pins sit close together so that link length never dominates the CPU
    /// budget; the radius is a plausible Barren one.
    fn colony() -> Colony {
        let mut c = Colony::new(90_000_100, OWNER, PLANET_BARREN, T0);
        c.planet_radius = 4_000_000.0;
        c.level = 1;
        for (i, (id, type_id)) in [
            (1_i64, TYPE_COMMAND_CENTRE),
            (2, TYPE_ECU),
            (3, TYPE_BASIC_FACTORY),
            (4, TYPE_LAUNCHPAD),
        ]
        .into_iter()
        .enumerate()
        {
            let point = SurfacePoint::new(1.5, 0.01 * i as f64);
            c.pins.push(Pin::new(id, type_id, OWNER, point, T0));
        }
        c.links.push(Link::new(1, 2, TYPE_LINK, 0));
        c.links.push(Link::new(2, 3, TYPE_LINK, 0));
        c.links.push(Link::new(3, 4, TYPE_LINK, 0));
        c
    }

    /// A quote that answers a fixed number, standing in for the resource
    /// field we do not have. Named for what it is so that no test reads as if
    /// the yield were computed.
    struct FixedQuote(i64);
    impl ProgramQuote for FixedQuote {
        fn quote(&self, _: &Colony, _: i64, _: i64, _: f64) -> i64 {
            self.0
        }
    }

    fn ctx<'a>(
        now: i64,
        pin: &'a mut i64,
        route: &'a mut i64,
        quote: &'a dyn ProgramQuote,
    ) -> ApplyContext<'a> {
        ApplyContext {
            owner_id: OWNER,
            now,
            next_pin_id: pin,
            next_route_id: route,
            link_type_id: TYPE_LINK,
            quote,
            pace: crate::game_clock::GameClock::REAL,
        }
    }

    /// Arm the fixture's ECU with a program and point its output at the
    /// factory, then the factory's at the launchpad.
    fn armed_colony(qty_per_cycle: i64) -> Colony {
        armed_colony_at(qty_per_cycle, crate::game_clock::GameClock::REAL)
    }

    /// [`armed_colony`] with every pin at `pace` before the program is
    /// installed (lane clock).
    fn armed_colony_at(qty_per_cycle: i64, pace: crate::game_clock::GameClock) -> Colony {
        let s = statics();
        let mut c = colony();
        c.set_pace(pace);
        {
            let ecu = c.pin_mut(2).expect("the fixture has an ECU");
            ecu.heads.push(Head {
                head_id: 0,
                latitude: 1.5,
                longitude: 0.01,
            });
            ecu.install_program(
                &s,
                Some(TYPE_AQUEOUS_LIQUIDS),
                RADIUS_DRILLAREAMIN,
                qty_per_cycle,
                T0,
            )
            .expect("a positive quote installs");
        }
        c.pin_mut(3)
            .expect("the fixture has a factory")
            .install_schematic(&s, SCHEMATIC_WATER)
            .expect("the basic factory may run the fixture schematic");
        c.routes.push(Route {
            route_id: 1,
            owner_id: OWNER,
            path: vec![2, 3],
            commodity_type_id: TYPE_AQUEOUS_LIQUIDS,
            commodity_quantity: 3000,
        });
        c.routes.push(Route {
            route_id: 2,
            owner_id: OWNER,
            path: vec![3, 4],
            commodity_type_id: TYPE_WATER,
            commodity_quantity: 20,
        });
        c
    }

    // -----------------------------------------------------------------
    // The extraction curve
    // -----------------------------------------------------------------

    /// The curve, against values computed independently from the bytecode's
    /// own arithmetic rather than from this implementation.
    ///
    /// The seven cycles of a 15-minute program and one far out on the tail
    /// pin the decay, the noise and the truncation at once: the sequence
    /// *rises* from the nominal 1000 to 1765, falls to a trough near 927, and
    /// is still noisy at 511 eighty cycles later. A port with the wrong
    /// `phaseShift` exponent, the wrong `/3`, or the noise term dropped
    /// produces a smooth monotone decay and would pass a "it goes down" test.
    #[test]
    fn the_extraction_curve_matches_the_clients_own_bar_graph() {
        let d = ecu_default::DECAY_FACTOR;
        let n = ecu_default::NOISE_FACTOR;
        let expected = [1765, 1659, 1498, 1311, 1131, 987, 927];
        for (i, want) in expected.iter().enumerate() {
            let cycle = i as i64 + 1;
            let got = ecu_program_output(cycle * CYCLE_15M, 0, 1000, CYCLE_15M, d, n);
            assert_eq!(
                got, *want,
                "cycle {cycle} of a 15-minute program on a 1000-unit quote"
            );
        }
        assert_eq!(
            ecu_program_output(80 * CYCLE_15M, 0, 1000, CYCLE_15M, d, n),
            511,
            "the tail is still noisy, not a smooth decay"
        );
    }

    /// A four-hour cycle, where `barWidth` is 16 rather than 1.
    ///
    /// `barWidth` scales the yield linearly with the cycle length, so a long
    /// program's cycle pays out proportionally more. A port that normalised
    /// the output per hour instead would agree on the 15-minute case -- where
    /// `barWidth` is exactly 1 -- and be wrong by 16x here. That is why the
    /// 15-minute case alone is not enough.
    #[test]
    fn a_longer_cycle_scales_the_yield_by_bar_width() {
        let d = ecu_default::DECAY_FACTOR;
        let n = ecu_default::NOISE_FACTOR;
        let cycle_4h = 4 * HOUR;
        assert_eq!((cycle_4h / SEC) as f64 / 900.0, 16.0, "barWidth is 16 here");
        for (i, want) in [14_598, 13_856, 10_810, 9_569].iter().enumerate() {
            let cycle = i as i64 + 1;
            assert_eq!(
                ecu_program_output(cycle * cycle_4h, 0, 1000, cycle_4h, d, n),
                *want,
                "cycle {cycle} of a four-hour program"
            );
        }
    }

    /// `cycleNum` is **floor** division, and the only place that is visible
    /// is a `startTime` in the future -- which is what a stale `installTime`
    /// after a clock correction looks like.
    ///
    /// Floor division of a negative numerator gives a negative cycle number,
    /// which `max(..., 0)` then pins at zero. Truncation toward zero would
    /// give `-0` for the same input and agree by luck; it disagrees for a
    /// `timeDiff` below `-cycleTime`. Either way the answer must be the first
    /// cycle's and never a panic or a negative yield.
    #[test]
    fn a_start_time_in_the_future_yields_the_first_cycle_and_never_less() {
        let d = ecu_default::DECAY_FACTOR;
        let n = ecu_default::NOISE_FACTOR;
        let first = ecu_program_output(0, 0, 1000, CYCLE_15M, d, n);
        for ahead in [1, CYCLE_15M / 2, CYCLE_15M, 10 * CYCLE_15M] {
            assert_eq!(
                ecu_program_output(0, ahead, 1000, CYCLE_15M, d, n),
                first,
                "a start time {ahead} ticks in the future clamps to cycle 0"
            );
        }
    }

    #[test]
    fn nothing_is_extracted_without_a_cycle_or_a_quote() {
        let d = ecu_default::DECAY_FACTOR;
        let n = ecu_default::NOISE_FACTOR;
        assert_eq!(ecu_program_output(T0, 0, 0, CYCLE_15M, d, n), 0);
        assert_eq!(ecu_program_output(T0, 0, 1000, 0, d, n), 0);
        assert_eq!(ecu_program_output(T0, 0, -5, CYCLE_15M, d, n), 0);
    }

    /// `GetMaxOutput` is the route cap, and it must be at least the largest
    /// yield any cycle actually produces -- otherwise route validation would
    /// refuse a route the colony then overflows.
    #[test]
    fn max_output_bounds_every_cycle_the_curve_produces() {
        let d = ecu_default::DECAY_FACTOR;
        let n = ecu_default::NOISE_FACTOR;
        let cap = ecu_max_output(1000, CYCLE_15M, n);
        assert_eq!(cap, 1800, "int(1.8 * 1000) * cycleTime / SEC / 900");
        for cycle in 1..200 {
            let got = ecu_program_output(cycle * CYCLE_15M, 0, 1000, CYCLE_15M, d, n);
            assert!(
                got <= cap,
                "cycle {cycle} produced {got}, above the route cap {cap}"
            );
        }
    }

    /// The slider's own arithmetic: radius -> program length -> cycle time.
    ///
    /// Four radii, including both ends. The band structure is the point: 0.02
    /// and 0.03 are a factor of two apart in length and land on different
    /// cycle times, and both give 84 cycles.
    #[test]
    fn program_length_and_cycle_time_quantise_the_way_the_slider_does() {
        for (radius, length, hours, cycles) in [
            (RADIUS_DRILLAREAMIN, 1.0, 0.25, 4),
            (0.02, 84.75, 1.0, 84),
            (0.03, 168.5, 2.0, 84),
            (RADIUS_DRILLAREAMAX, 336.0, 4.0, 84),
        ] {
            let got_length = program_length_from_head_radius(radius);
            assert!(
                (got_length - length).abs() < 1e-9,
                "radius {radius}: length {got_length}, wanted {length}"
            );
            let got_hours = cycle_time_from_program_length(got_length);
            assert!(
                (got_hours - hours).abs() < 1e-12,
                "radius {radius}: cycle {got_hours}h, wanted {hours}h"
            );
            assert_eq!(
                (got_length / got_hours).trunc() as i64,
                cycles,
                "radius {radius}: cycle count"
            );
        }
    }

    // -----------------------------------------------------------------
    // Static tables
    // -----------------------------------------------------------------

    #[test]
    fn the_command_centre_table_is_the_clients_six_rows() {
        assert_eq!(COMMAND_CENTRE_LEVELS.len(), 6);
        assert_eq!(command_centre_output(0), Some((6_000, 1_675)));
        assert_eq!(command_centre_output(5), Some((19_000, 25_415)));
        assert_eq!(
            command_centre_output(6),
            None,
            "a level off the table is a refusal, not a zero"
        );
        // `GetUpgradeCost` subtracts, because the table is cumulative.
        assert_eq!(command_centre_upgrade_cost(0, 5), Some(6_310_000));
        assert_eq!(command_centre_upgrade_cost(4, 5), Some(2_100_000));
        // Monotone in both columns, which is what makes "upgrade only"
        // legal: a downgrade would owe a refund.
        for w in COMMAND_CENTRE_LEVELS.windows(2) {
            assert!(w[1].0 > w[0].0 && w[1].1 > w[0].1 && w[1].2 > w[0].2);
        }
    }

    #[test]
    fn a_pin_kind_is_decided_by_its_group_and_nothing_else() {
        for (group, kind) in [
            (GROUP_COMMAND_PINS, PinKind::Command),
            (GROUP_EXTRACTOR_PINS, PinKind::Extractor),
            (GROUP_PROCESS_PINS, PinKind::Process),
            (GROUP_SPACEPORT_PINS, PinKind::Spaceport),
            (GROUP_STORAGE_PINS, PinKind::Storage),
            (GROUP_ECU_PINS, PinKind::Ecu),
        ] {
            assert_eq!(PinKind::from_group(group), Some(kind));
        }
        // 1036 is `groupPlanetaryLinks` and 1025 is
        // `groupPlanetaryCustomsOffices`; neither is a pin.
        assert_eq!(PinKind::from_group(1036), None);
        assert_eq!(PinKind::from_group(1025), None);
        // Three classes are storage, not one.
        assert!(PinKind::Command.is_storage());
        assert!(PinKind::Spaceport.is_storage());
        assert!(PinKind::Storage.is_storage());
        assert!(!PinKind::Process.is_storage());
    }

    // -----------------------------------------------------------------
    // Geometry
    // -----------------------------------------------------------------

    /// `SetLongitude` is Python's modulo, which is non-negative. Rust's `%`
    /// is not, so a negative longitude is the one input that tells the two
    /// apart.
    #[test]
    fn surface_points_are_normalised_the_way_the_client_stores_them() {
        let p = SurfacePoint::new(0.5, -0.25).normalised();
        assert!(
            (p.longitude - (std::f64::consts::TAU - 0.25)).abs() < 1e-12,
            "a negative longitude wraps forwards, not into a negative: {}",
            p.longitude
        );
        // Latitude is clamped to [-pi, pi] and is therefore signed.
        assert_eq!(
            SurfacePoint::new(-1.0, 0.0).normalised().latitude,
            -1.0,
            "the southern hemisphere is representable"
        );
        assert_eq!(
            SurfacePoint::new(10.0, 0.0).normalised().latitude,
            std::f64::consts::PI
        );
        // A NaN must not reach the distance arithmetic.
        assert!(
            SurfacePoint::new(f64::NAN, 0.0)
                .normalised()
                .latitude
                .is_finite()
        );
    }

    #[test]
    fn distance_is_the_great_circle_arc() {
        let r = 4_000_000.0;
        let a = SurfacePoint::new(1.0, 0.0);
        assert_eq!(a.distance_to(a, r), 0.0, "a pin is no distance from itself");
        // A quarter turn along a meridian is a quarter of the circumference.
        let b = SurfacePoint::new(1.0 + std::f64::consts::FRAC_PI_2, 0.0);
        let want = r * std::f64::consts::FRAC_PI_2;
        assert!(
            (a.distance_to(b, r) - want).abs() < 1.0,
            "{} vs {want}",
            a.distance_to(b, r)
        );
        // A radius of zero makes every link free, which is why
        // `Colony::planet_radius` has to be filled in before a CPU check
        // means anything.
        assert_eq!(a.distance_to(b, 0.0), 0.0);
    }

    // -----------------------------------------------------------------
    // The simulation
    // -----------------------------------------------------------------

    /// **The property that makes this subsystem safe to run off a clock
    /// instead of a thread**: advancing to `T` in one step and in two must
    /// give the same colony.
    ///
    /// If it did not, the colony a player sees would depend on how often
    /// anyone happened to look at it -- and because every read of a colony
    /// advances it, that means on how often they opened the window. This is
    /// the test that would catch a cycle being lost or double-counted at a
    /// resume boundary, and it is checked at a boundary that falls *inside* a
    /// cycle as well as on one.
    #[test]
    fn advancing_in_one_step_and_in_two_give_the_same_colony() {
        let s = statics();
        let target = T0 + 20 * HOUR;
        let mut one = armed_colony(1000);
        one.run_simulation(&s, target);
        for split in [
            T0 + 1,
            T0 + CYCLE_15M / 3,
            T0 + CYCLE_15M,
            T0 + 7 * HOUR,
            T0 + 19 * HOUR + 1,
        ] {
            let mut two = armed_colony(1000);
            two.run_simulation(&s, split);
            two.run_simulation(&s, target);
            assert_eq!(two, one, "a pause at {} changed the outcome", split - T0);
        }
    }

    /// A read cannot move the colony backwards.
    ///
    /// The client sends its own clock in places and a client behind ours must
    /// not be able to rewind a colony by asking about it -- which, because
    /// every read advances the simulation, would otherwise replay cycles and
    /// duplicate goods.
    #[test]
    fn a_target_in_the_past_changes_nothing() {
        let s = statics();
        let mut c = armed_colony(1000);
        c.run_simulation(&s, T0 + 10 * HOUR);
        let after = c.clone();
        let report = c.run_simulation(&s, T0 + HOUR);
        assert_eq!(report.events, 0);
        assert_eq!(c, after, "a backwards target must be a no-op");
    }

    /// The boundary cycle, which is where EvEJS and the client disagree.
    ///
    /// `RunSimulation` breaks on `simTime > simEndTime`, so a cycle landing
    /// exactly on the deadline runs. EvEJS admits an event only on
    /// `nextRunTime < targetTime` and drops it. The colony is primed at a
    /// cycle boundary, so this is not an edge case: it is every pass whose
    /// deadline is a whole number of cycles after the install.
    #[test]
    fn a_cycle_landing_exactly_on_the_deadline_runs() {
        let s = statics();
        let mut on = armed_colony(1000);
        let on_report = on.run_simulation(&s, T0 + CYCLE_15M);
        assert_eq!(
            on_report.events, 1,
            "the ECU's first cycle is due exactly at the deadline"
        );
        let mut before = armed_colony(1000);
        assert_eq!(
            before.run_simulation(&s, T0 + CYCLE_15M - 1).events,
            0,
            "one tick earlier it is not due"
        );
    }

    /// A storage pin's cycle time is literally zero, so its inherited
    /// `GetNextRunTime` is always "now". Only `StoragePin.CanRun` being a
    /// hard `False` stops every silo in the colony being evaluated forever.
    #[test]
    fn a_storage_pin_never_cycles() {
        let s = statics();
        let c = colony();
        for id in [1, 4] {
            let pin = c.pin(id).expect("fixture pin");
            assert_eq!(pin.cycle_time(&s), 0);
            assert!(!pin.can_run(&s, T0 + 10 * HOUR), "pin {id} must never run");
            assert!(!pin.can_activate(&s), "pin {id} must never activate");
        }
        // And a colony of nothing but storage produces no events at all.
        let mut idle = Colony::new(1, OWNER, PLANET_BARREN, T0);
        idle.pins.push(Pin::new(
            1,
            TYPE_LAUNCHPAD,
            OWNER,
            SurfacePoint::new(1.0, 0.0),
            T0,
        ));
        assert_eq!(idle.run_simulation(&s, T0 + 365 * 24 * HOUR).events, 0);
    }

    /// The whole point of the subsystem: extraction becomes a commodity.
    ///
    /// Aqueous Liquids out of the ECU, through the factory on the fixture schematic,
    /// and into the launchpad as Water. Checked at several `now` values
    /// because the chain has a latency at every hop -- the factory produces
    /// the cycle *after* it consumes -- and a port can easily get a chain
    /// that moves goods one hop per pass instead of the whole way.
    #[test]
    fn a_full_production_chain_turns_extraction_into_a_commodity() {
        let s = statics();
        let mut c = armed_colony(4000);
        // One extraction cycle: the ECU has produced and the factory has
        // taken a batch, but no Water exists yet.
        c.run_simulation(&s, T0 + CYCLE_15M);
        assert_eq!(
            c.pin(4).unwrap().quantity(TYPE_WATER),
            0,
            "the factory's first cycle has not finished yet"
        );
        assert!(
            c.pin(3).unwrap().is_active(&s),
            "the factory consumed a batch and started"
        );
        // The fixture schematic is a 30-minute cycle, so by an hour in there is Water
        // on the launchpad.
        c.run_simulation(&s, T0 + HOUR);
        let after_hour = c.pin(4).unwrap().quantity(TYPE_WATER);
        assert!(
            after_hour > 0,
            "after an hour the launchpad should hold Water, holds {after_hour}"
        );
        // And it keeps accumulating, in multiples of the schematic's output.
        c.run_simulation(&s, T0 + 12 * HOUR);
        let after_twelve = c.pin(4).unwrap().quantity(TYPE_WATER);
        assert!(
            after_twelve > after_hour,
            "{after_twelve} should exceed {after_hour}"
        );
        assert_eq!(
            after_twelve % 20,
            0,
            "Water only ever arrives 20 at a time, the schematic's output"
        );
        // Nothing was created from nothing: the launchpad holds no raw
        // material, because the route to it carries Water only.
        assert_eq!(c.pin(4).unwrap().quantity(TYPE_AQUEOUS_LIQUIDS), 0);
    }

    /// The factory's one-cycle latency, isolated.
    ///
    /// `ProcessPin.Run` pays the output because the pin was *already* active
    /// on entry and consumes for the cycle that is starting. So a factory
    /// handed exactly one batch produces exactly once and then idles -- and a
    /// port that consumed-then-produced in the same call would produce
    /// immediately and be a cycle ahead forever.
    #[test]
    fn a_factory_produces_the_cycle_after_it_consumes() {
        let s = statics();
        let mut c = colony();
        c.pin_mut(3)
            .unwrap()
            .install_schematic(&s, SCHEMATIC_WATER)
            .unwrap();
        c.pin_mut(3)
            .unwrap()
            .contents
            .insert(TYPE_AQUEOUS_LIQUIDS, 3000);
        c.routes.push(Route {
            route_id: 1,
            owner_id: OWNER,
            path: vec![3, 4],
            commodity_type_id: TYPE_WATER,
            commodity_quantity: 20,
        });
        // The first pass consumes the batch and starts the cycle.
        c.run_simulation(&s, T0 + 1);
        assert!(c.pin(3).unwrap().is_active(&s));
        assert_eq!(c.pin(3).unwrap().quantity(TYPE_AQUEOUS_LIQUIDS), 0);
        assert_eq!(c.pin(4).unwrap().quantity(TYPE_WATER), 0);
        // 1800 seconds later it pays out, finds no second batch, and idles.
        c.run_simulation(&s, T0 + 1 + 1800 * SEC);
        assert_eq!(c.pin(4).unwrap().quantity(TYPE_WATER), 20);
        assert!(!c.pin(3).unwrap().is_active(&s), "no second batch, so idle");
        // And it stays idle forever rather than producing from nothing.
        c.run_simulation(&s, T0 + 100 * HOUR);
        assert_eq!(c.pin(4).unwrap().quantity(TYPE_WATER), 20);
    }

    /// `ProcessPin.GetNextRunTime` returns `None` for an idle factory with a
    /// full buffer, and `CanRun` reads `None` as **now**.
    ///
    /// A port that read `Option::None` as "never" -- the natural reading --
    /// would give every factory a one-cycle stall after every delivery. The
    /// colony would still work, just slower, which is the kind of bug nobody
    /// finds. So it is pinned here directly rather than only through the
    /// chain test.
    #[test]
    fn an_idle_factory_with_a_full_buffer_wants_to_run_immediately() {
        let s = statics();
        let mut pin = Pin::new(
            3,
            TYPE_BASIC_FACTORY,
            OWNER,
            SurfacePoint::new(1.0, 0.0),
            T0,
        );
        pin.install_schematic(&s, SCHEMATIC_WATER).unwrap();
        assert!(!pin.is_active(&s));
        assert!(
            pin.next_run_time(&s).is_some(),
            "an empty factory waits out its cycle"
        );
        pin.contents.insert(TYPE_AQUEOUS_LIQUIDS, 3000);
        assert_eq!(
            pin.next_run_time(&s),
            None,
            "a full buffer means 'run now', expressed as None"
        );
        assert!(
            pin.can_run(&s, T0),
            "and `can_run` must read that None as runnable"
        );
    }

    /// `EcuPin.Run` banks the output *before* idling on expiry, with `<=`.
    ///
    /// EvEJS returns empty and idles when `runTime > expiryTime`
    /// (`planetRuntimeStore.js:1856`), losing the last cycle of every
    /// program. On a one-hour program that is one cycle in four.
    #[test]
    fn the_cycle_a_program_expires_on_still_pays_out() {
        let s = statics();
        let mut pin = Pin::new(2, TYPE_ECU, OWNER, SurfacePoint::new(1.5, 0.01), T0);
        pin.heads.push(Head {
            head_id: 0,
            latitude: 1.5,
            longitude: 0.01,
        });
        pin.install_program(
            &s,
            Some(TYPE_AQUEOUS_LIQUIDS),
            RADIUS_DRILLAREAMIN,
            1000,
            T0,
        )
        .unwrap();
        let expiry = pin.expiry_time.expect("an installed program expires");
        assert_eq!(
            expiry,
            T0 + 4 * CYCLE_15M,
            "a minimum-radius program is four 15-minute cycles"
        );
        // Run the final cycle, which lands exactly on the expiry.
        let products = pin.run(&s, expiry);
        assert!(
            products.get(&TYPE_AQUEOUS_LIQUIDS).copied().unwrap_or(0) > 0,
            "the expiring cycle must still pay: {products:?}"
        );
        assert_eq!(pin.state, state::IDLE, "and then the program is spent");
        // And an idle ECU can never be revived by the simulation --
        // `EcuPin.CanActivate` compares against STATE_ACTIVE, not IDLE.
        assert!(!pin.can_activate(&s));
        assert!(!pin.can_run(&s, expiry + 10 * HOUR));
    }

    /// Lane clock: at pace N an extraction program and a factory run N
    /// times faster and yield exactly what pace 1 yields in N times the
    /// time; the program expires after its four cycles, N times sooner.
    #[test]
    fn at_pace_n_a_colony_produces_a_day_in_a_day_over_n() {
        let s = statics();
        let pace = crate::game_clock::GameClock::new(4).unwrap();
        let mut real = armed_colony(4000);
        let mut fast = armed_colony_at(4000, pace);
        assert_eq!(
            fast.pin(2).unwrap().expiry_time,
            Some(T0 + 4 * CYCLE_15M / 4)
        );
        assert_eq!(
            real.pin(2).unwrap().cycle_time,
            fast.pin(2).unwrap().cycle_time
        );
        for hours in [1, 3, 12] {
            real.run_simulation(&s, T0 + hours * HOUR);
            fast.run_simulation(&s, T0 + hours * HOUR / 4);
            let water = |c: &Colony| c.pin(4).unwrap().quantity(TYPE_WATER);
            assert!(water(&real) > 0 || hours == 1);
            assert_eq!(water(&fast), water(&real), "after {hours} game hours");
        }
        assert_eq!(fast.pin(2).unwrap().state, state::IDLE);
    }

    /// A program's whole yield over its life, as a sanity bound.
    ///
    /// Four cycles of a minimum program, each capped by `GetMaxOutput`, and
    /// nothing after expiry. This is the test that fails if expiry is not
    /// enforced at all -- the colony would extract forever.
    #[test]
    fn a_program_stops_at_its_expiry_and_not_before() {
        let s = statics();
        let mut c = armed_colony(1000);
        c.run_simulation(&s, T0 + 365 * 24 * HOUR);
        let total = c.pin(4).unwrap().quantity(TYPE_WATER);
        let cap = ecu_max_output(1000, CYCLE_15M, ecu_default::NOISE_FACTOR);
        assert!(
            total > 0,
            "a year of simulation on a four-cycle program should still have produced"
        );
        // Four extraction cycles, each at most `cap`, is the whole raw
        // supply; at 3000 in per 20 out the Water cannot exceed that ratio.
        let raw_bound = 4 * cap;
        assert!(
            total <= raw_bound / 3000 * 20 + 20,
            "{total} Water implies more than {raw_bound} raw units were extracted"
        );
        assert_eq!(c.pin(2).unwrap().state, state::IDLE);
    }

    /// `TransferCommodities` debits the source **only if it is storage**.
    ///
    /// A factory's and an ECU's output was never in its contents -- `Run`
    /// returned it as a value -- so debiting it would remove the player's
    /// stock to pay for goods that came out of thin air. Getting this
    /// backwards drains a colony's silos exactly as fast as it produces, and
    /// the net effect at the launchpad looks almost right.
    #[test]
    fn a_transfer_debits_a_silo_and_never_a_producer() {
        let s = statics();
        let mut c = colony();
        // Replace the factory with a second silo so the colony is
        // storage -> consumer only where it has to be.
        c.pins.push(Pin::new(
            5,
            TYPE_STORAGE,
            OWNER,
            SurfacePoint::new(1.5, 0.04),
            T0,
        ));
        c.links.push(Link::new(4, 5, TYPE_LINK, 0));
        c.pin_mut(4).unwrap().contents.insert(TYPE_WATER, 100);
        let moved = c.transfer_commodities(&s, 4, 5, TYPE_WATER, 30);
        assert_eq!(moved, 30);
        assert_eq!(
            c.pin(4).unwrap().quantity(TYPE_WATER),
            70,
            "a storage source is debited"
        );
        assert_eq!(c.pin(5).unwrap().quantity(TYPE_WATER), 30);
        // Now from the ECU, which holds nothing and must not be debited into
        // a negative.
        let from_producer = c.transfer_commodities(&s, 2, 5, TYPE_WATER, 10);
        assert_eq!(from_producer, 10, "the destination still takes it");
        assert_eq!(
            c.pin(2).unwrap().quantity(TYPE_WATER),
            0,
            "a producer source is not debited and does not go negative"
        );
    }

    /// A destination takes only what fits, and the source keeps the rest.
    #[test]
    fn a_short_accept_leaves_the_remainder_where_it_was() {
        let s = statics();
        let mut c = colony();
        c.pins.push(Pin::new(
            5,
            TYPE_STORAGE,
            OWNER,
            SurfacePoint::new(1.5, 0.04),
            T0,
        ));
        // 12,000 m3 of capacity at 0.19 m3 per unit is 63,157 units.
        let fits = (12_000.0f64 / 0.19).trunc() as i64;
        c.pin_mut(4)
            .unwrap()
            .contents
            .insert(TYPE_WATER, fits + 500);
        let moved = c.transfer_commodities(&s, 4, 5, TYPE_WATER, fits + 500);
        assert_eq!(moved, fits, "only what fits by volume");
        assert_eq!(
            c.pin(4).unwrap().quantity(TYPE_WATER),
            500,
            "the rest stays in the source rather than vanishing"
        );
    }

    /// A factory's buffer is bounded by **one cycle's demand**, not by volume.
    #[test]
    fn a_factory_never_stockpiles_two_batches_of_an_input() {
        let s = statics();
        let mut pin = Pin::new(
            3,
            TYPE_BASIC_FACTORY,
            OWNER,
            SurfacePoint::new(1.0, 0.0),
            T0,
        );
        pin.install_schematic(&s, SCHEMATIC_WATER).unwrap();
        assert_eq!(
            pin.add_commodity(&s, TYPE_AQUEOUS_LIQUIDS, 10_000),
            3000,
            "one batch is 3000"
        );
        assert_eq!(pin.add_commodity(&s, TYPE_AQUEOUS_LIQUIDS, 10_000), 0);
        // And it refuses a commodity its schematic does not want at all.
        assert_eq!(pin.add_commodity(&s, TYPE_WATER, 10), 0);
    }

    /// An ECU is not a container: `EcuPin.CanAccept` is a hard zero.
    #[test]
    fn an_extractor_accepts_nothing() {
        let s = statics();
        let mut pin = Pin::new(2, TYPE_ECU, OWNER, SurfacePoint::new(1.0, 0.0), T0);
        assert_eq!(pin.add_commodity(&s, TYPE_AQUEOUS_LIQUIDS, 100), 0);
        assert_eq!(pin.capacity(&s), None, "and it has no capacity at all");
    }

    /// A disabled or edit-mode pin takes nothing, whatever kind it is.
    /// `BasePin.CanAccept`'s first line, and the reason the two negative
    /// states are carried at all.
    #[test]
    fn a_disabled_pin_accepts_nothing() {
        let s = statics();
        for forbidden in [state::DISABLED, state::EDITMODE] {
            let mut pin = Pin::new(4, TYPE_LAUNCHPAD, OWNER, SurfacePoint::new(1.0, 0.0), T0);
            pin.state = forbidden;
            assert_eq!(
                pin.add_commodity(&s, TYPE_WATER, 10),
                0,
                "state {forbidden} must accept nothing"
            );
        }
    }

    /// `GetSortedRoutesForPin` keys storage routes on `GetFreeSpace`, which
    /// is **not** clamped at zero, and the storage pass caps each delivery at
    /// `ceil(remaining / routes_left)` so a cycle is spread over the silos
    /// rather than filling the first.
    #[test]
    fn storage_deliveries_are_spread_rather_than_filling_the_first_silo() {
        let s = statics();
        let mut c = colony();
        // Three silos off the factory, all empty, all reachable.
        for (i, id) in [5_i64, 6, 7].into_iter().enumerate() {
            c.pins.push(Pin::new(
                id,
                TYPE_STORAGE,
                OWNER,
                SurfacePoint::new(1.5, 0.05 + 0.01 * i as f64),
                T0,
            ));
            c.links.push(Link::new(3, id, TYPE_LINK, 0));
            c.routes.push(Route {
                route_id: id,
                owner_id: OWNER,
                path: vec![3, id],
                commodity_type_id: TYPE_WATER,
                commodity_quantity: 20,
            });
        }
        let mut queue = Vec::new();
        let mut touched = std::collections::BTreeSet::new();
        let products: BTreeMap<i64, i64> = [(TYPE_WATER, 21)].into_iter().collect();
        c.route_commodity_output(&s, 3, products, &mut queue, &mut touched, 0);
        let got: Vec<i64> = [5, 6, 7]
            .iter()
            .map(|id| c.pin(*id).unwrap().quantity(TYPE_WATER))
            .collect();
        assert_eq!(
            got.iter().sum::<i64>(),
            21,
            "every unit is delivered, none dropped: {got:?}"
        );
        assert!(
            got.iter().all(|n| *n > 0),
            "all three silos get a share: {got:?}"
        );
        // `ceil(21/3) = 7` for the first, then `ceil(14/2) = 7`, then 7.
        assert_eq!(
            got,
            vec![7, 7, 7],
            "an even split, with ceil absorbing the remainder"
        );
    }

    // -----------------------------------------------------------------
    // Route validity
    // -----------------------------------------------------------------

    /// A route is a physical thing: every consecutive pair of its path must
    /// be a link that exists.
    #[test]
    fn a_route_must_follow_links_that_exist() {
        let s = statics();
        let c = armed_colony(4000);
        // 2 -> 3 is linked and legal.
        assert_eq!(
            c.validate_route(
                &s,
                &Route {
                    route_id: 9,
                    owner_id: OWNER,
                    path: vec![2, 3],
                    commodity_type_id: TYPE_AQUEOUS_LIQUIDS,
                    commodity_quantity: 1,
                },
                OWNER
            ),
            Ok(())
        );
        // 2 -> 4 is not: the fixture links 2-3 and 3-4, never 2-4.
        assert_eq!(
            c.validate_route(
                &s,
                &Route {
                    route_id: 9,
                    owner_id: OWNER,
                    path: vec![2, 4],
                    commodity_type_id: TYPE_AQUEOUS_LIQUIDS,
                    commodity_quantity: 1,
                },
                OWNER
            ),
            Err(PiError::RouteFailedValidationLinkDoesNotExist)
        );
        // The multi-hop version is legal because every hop is linked.
        assert_eq!(
            c.validate_route(
                &s,
                &Route {
                    route_id: 9,
                    owner_id: OWNER,
                    path: vec![2, 3, 4],
                    commodity_type_id: TYPE_AQUEOUS_LIQUIDS,
                    commodity_quantity: 1,
                },
                OWNER
            ),
            Ok(())
        );
    }

    /// Each of the route refusals, by the name the client raises.
    ///
    /// Table-driven because the *names* are the contract -- the PI window
    /// pre-validates against the same list, so a refusal under the wrong name
    /// is a message the player cannot act on.
    #[test]
    fn every_route_refusal_is_the_one_the_client_names() {
        let s = statics();
        let mut c = armed_colony(4000);
        // Enough links for a seven-hop path, to reach the waypoint cap.
        for id in 5..=12 {
            c.pins.push(Pin::new(
                id,
                TYPE_STORAGE,
                OWNER,
                SurfacePoint::new(1.5, 0.05 + 0.005 * id as f64),
                T0,
            ));
        }
        for pair in [(4, 5), (5, 6), (6, 7), (7, 8), (8, 9), (9, 10), (10, 11)] {
            c.links.push(Link::new(pair.0, pair.1, TYPE_LINK, 0));
        }
        let route = |path: Vec<i64>, type_id: i64, qty: i64| Route {
            route_id: 99,
            owner_id: OWNER,
            path,
            commodity_type_id: type_id,
            commodity_quantity: qty,
        };
        let cases: Vec<(Route, PiError)> = vec![
            (
                route(vec![3, 4], TYPE_WATER, 0),
                PiError::CreateRouteWithoutCommodities,
            ),
            (route(vec![3], TYPE_WATER, 1), PiError::CreateRouteTooShort),
            (
                // Nine pins is seven waypoints, two more than MAX_WAYPOINTS.
                route(vec![3, 4, 5, 6, 7, 8, 9, 10, 11], TYPE_WATER, 1),
                PiError::CannotRouteTooManyWaypoints,
            ),
            (
                route(vec![3, 999], TYPE_WATER, 1),
                PiError::RouteFailedValidationPinDoesNotExist,
            ),
            (
                // Launchpad (storage) to silo (storage): storage must feed a
                // consumer, and this refusal is what makes the
                // storage-to-storage recursion in the router unreachable.
                route(vec![4, 5], TYPE_WATER, 1),
                PiError::CannotRouteStorageMustGoToConsumer,
            ),
            (
                // The ECU produces Aqueous Liquids, not Water.
                route(vec![2, 3], TYPE_WATER, 1),
                PiError::CreateRouteCommodityNotProduced,
            ),
            (
                // The fixture schematic wants Aqueous Liquids; the factory will not
                // accept Water as an input.
                route(vec![4, 3], TYPE_WATER, 1),
                PiError::CreateRouteDestinationCannotAcceptCommodity,
            ),
        ];
        for (r, want) in cases {
            assert_eq!(
                c.validate_route(&s, &r, OWNER),
                Err(want.clone()),
                "path {:?} type {} qty {} should be {}",
                r.path,
                r.commodity_type_id,
                r.commodity_quantity,
                want
            );
        }
    }

    /// The sum of a producer's routes for one commodity may not exceed one
    /// cycle's **maximum** output.
    ///
    /// The cap is `GetProductMaxOutput` -- the noisiest cycle -- and not the
    /// nominal yield, because the client sizes its own route proposals
    /// against that figure. A server using the nominal one would refuse
    /// routes the client offered the player, which reads as the window being
    /// broken.
    #[test]
    fn a_producers_routes_may_not_outrun_one_cycles_maximum() {
        let s = statics();
        let mut c = armed_colony(1000);
        c.routes.clear();
        let cap = c
            .pin(2)
            .unwrap()
            .product_max_output(&s)
            .get(&TYPE_AQUEOUS_LIQUIDS)
            .copied()
            .unwrap();
        assert!(
            cap > 1000,
            "the cap is the noisy maximum ({cap}), above the nominal 1000"
        );
        let whole = Route {
            route_id: 1,
            owner_id: OWNER,
            path: vec![2, 3],
            commodity_type_id: TYPE_AQUEOUS_LIQUIDS,
            commodity_quantity: cap,
        };
        assert_eq!(c.validate_route(&s, &whole, OWNER), Ok(()));
        let over = Route {
            commodity_quantity: cap + 1,
            ..whole.clone()
        };
        assert_eq!(
            c.validate_route(&s, &over, OWNER),
            Err(PiError::CreateRouteCommodityProductionTooSmall)
        );
        // And the cap is on the **sum**: a second route for the same
        // commodity has to fit in what the first left.
        c.routes.push(whole);
        let second = Route {
            route_id: 2,
            owner_id: OWNER,
            path: vec![2, 3],
            commodity_type_id: TYPE_AQUEOUS_LIQUIDS,
            commodity_quantity: 1,
        };
        assert_eq!(
            c.validate_route(&s, &second, OWNER),
            Err(PiError::CreateRouteCommodityProductionTooSmall),
            "two routes together may not exceed one cycle"
        );
    }

    /// Someone else's pin is not routable through, even if it is on the same
    /// planet.
    ///
    /// `route_id: 2` on purpose -- it **replaces** the fixture's existing
    /// Water route rather than adding to it. The first draft of this test
    /// used a fresh id and got `CreateRouteCommodityProductionTooSmall`,
    /// because the production cap is checked **before** the ownership walk.
    /// That turned out to be the client's own order
    /// (`ValidateCreateRoute`'s message constants run
    /// `WithoutCommodities, TooShort, TooManyWaypoints,
    /// StorageMustGoToConsumer, DestinationWillNotAccept, CommodityNotProduced,
    /// ProductionTooSmall, DestinationCannotAcceptCommodity,
    /// PinDoesNotExist, PinNotYours, LinkDoesNotExist,
    /// CannotRouteCommodities` **[V]**), so the test was confounded and the
    /// code was right.
    #[test]
    fn a_route_may_not_pass_through_another_characters_pin() {
        let s = statics();
        let mut c = armed_colony(4000);
        c.pin_mut(4).unwrap().owner_id = OWNER + 1;
        assert_eq!(
            c.validate_route(
                &s,
                &Route {
                    route_id: 2,
                    owner_id: OWNER,
                    path: vec![3, 4],
                    commodity_type_id: TYPE_WATER,
                    commodity_quantity: 20,
                },
                OWNER
            ),
            Err(PiError::RouteFailedValidationPinNotYours)
        );
    }

    /// Bandwidth is summed **per link over every route that crosses it**.
    ///
    /// Checking each route against each link in isolation would pass a colony
    /// whose two routes together overload one link -- which is the only way a
    /// player ever actually overloads one.
    #[test]
    fn link_bandwidth_is_summed_across_every_route_that_crosses_it() {
        let s = statics();
        let mut c = armed_colony(4000);
        assert_eq!(c.validate(&s, OWNER), Ok(()));
        let link = c.link(3, 4).unwrap();
        let capacity = link.bandwidth(&s);
        assert_eq!(capacity, 1250.0, "logisticalCapacity * 2**0");
        // One Water route at 20 per 1800-second cycle is
        // 20 * 0.19 * (HOUR / 1800s) = 7.6 m3/h -- far inside it.
        let one = c.route_bandwidth(&s, c.route(2).unwrap());
        assert!((one - 7.6).abs() < 1e-9, "{one}");
        // Pile on enough identical routes to exceed the link, and the colony
        // must be refused as a whole even though no single route does.
        let needed = (capacity / one).ceil() as i64 + 1;
        for n in 0..needed {
            c.routes.push(Route {
                route_id: 100 + n,
                owner_id: OWNER,
                path: vec![3, 4],
                commodity_type_id: TYPE_WATER,
                commodity_quantity: 20,
            });
        }
        // Raise the producer cap so the refusal is about bandwidth and not
        // about production, which would be the wrong error to assert.
        c.pin_mut(3).unwrap().schematic_id.replace(SCHEMATIC_WATER);
        match c.validate(&s, OWNER) {
            Err(PiError::RouteFailedValidationCannotRouteCommodities) => {}
            Err(PiError::CreateRouteCommodityProductionTooSmall) => {}
            other => panic!("an overloaded link must be refused, got {other:?}"),
        }
        // A level-1 link doubles the capacity: 1250 -> 2500.
        let upgraded = Link::new(3, 4, TYPE_LINK, 1);
        assert_eq!(upgraded.bandwidth(&s), 2500.0);
    }

    /// A link's cost grows with its length and its level, and the per-km term
    /// is rounded **up on its own** before the base is added.
    #[test]
    fn a_links_cost_is_its_base_plus_a_ceilinged_per_km_term() {
        let s = statics();
        let link = Link::new(1, 2, TYPE_LINK, 0);
        // At zero length only the base remains.
        assert_eq!(link.cpu_usage(&s, 0.0), 15.0);
        assert_eq!(link.power_usage(&s, 0.0), 10.0);
        // 10 km at 0.2 CPU/km, level 0, modifier 1.4: 0.2 * 10 * 1 = 2.
        assert_eq!(link.cpu_usage(&s, 10_000.0), 17.0);
        // Upgrading costs more CPU, not less: (level+1) ** 1.4 grows.
        let upgraded = Link::new(1, 2, TYPE_LINK, 3);
        assert!(upgraded.cpu_usage(&s, 10_000.0) > link.cpu_usage(&s, 10_000.0));
    }

    /// `GetExpeditedTransferTime`'s five-minute floor.
    #[test]
    fn an_expedited_transfer_never_takes_less_than_five_minutes() {
        assert_eq!(Colony::expedited_transfer_time(1250.0, 0.0), 5 * MIN);
        assert_eq!(Colony::expedited_transfer_time(0.0, 1000.0), 5 * MIN);
        // A shipment that needs an hour of bandwidth takes an hour.
        let got = Colony::expedited_transfer_time(1250.0, 1250.0);
        assert_eq!(got, HOUR);
    }

    // -----------------------------------------------------------------
    // The command stream
    // -----------------------------------------------------------------

    /// The thirteen opcodes, in the client's own numbering.
    ///
    /// `commandStream.pyc`'s module body assigns `1`..`13` to
    /// `COMMAND_CREATEPIN`..`COMMAND_INSTALLPROGRAM` in this order. The
    /// inventory called this *"the single riskiest unknown"* in PI --
    /// *"wrong opcode = colony silently mangled"* -- so it is pinned as a
    /// test rather than only as a comment.
    #[test]
    fn the_thirteen_opcodes_are_the_clients_thirteen() {
        let p = Id::Real(1);
        let all = vec![
            Command::CreatePin {
                pin_id: p.clone(),
                type_id: 0,
                point: SurfacePoint::new(0.0, 0.0),
            },
            Command::RemovePin { pin_id: p.clone() },
            Command::CreateLink {
                endpoint1: p.clone(),
                endpoint2: p.clone(),
                level: 0,
            },
            Command::RemoveLink {
                endpoint1: p.clone(),
                endpoint2: p.clone(),
            },
            Command::SetLinkLevel {
                endpoint1: p.clone(),
                endpoint2: p.clone(),
                level: 0,
            },
            Command::CreateRoute {
                route_id: p.clone(),
                path: vec![],
                type_id: 0,
                quantity: 0,
            },
            Command::RemoveRoute {
                route_id: p.clone(),
            },
            Command::SetSchematic {
                pin_id: p.clone(),
                schematic_id: 0,
            },
            Command::UpgradeCommandCenter {
                pin_id: p.clone(),
                level: 0,
            },
            Command::AddExtractorHead {
                pin_id: p.clone(),
                head_id: 0,
                point: SurfacePoint::new(0.0, 0.0),
            },
            Command::KillExtractorHead {
                pin_id: p.clone(),
                head_id: 0,
            },
            Command::MoveExtractorHead {
                pin_id: p.clone(),
                head_id: 0,
                point: SurfacePoint::new(0.0, 0.0),
            },
            Command::InstallProgram {
                pin_id: p,
                program_type: None,
                head_radius: 0.0,
            },
        ];
        assert_eq!(all.len(), 13, "the client has exactly thirteen");
        for (i, c) in all.iter().enumerate() {
            assert_eq!(c.opcode(), i as i64 + 1, "{c:?}");
        }
    }

    /// `BaseColony.IsTemporaryID` is `type(id) == types.TupleType` -- any
    /// tuple, with no namespace tag. EvEJS requires one
    /// (`planetRuntimeStore.js:1096`); our client does not, so a server that
    /// insisted would reject a well-formed stream.
    #[test]
    fn a_temporary_id_is_any_tuple_and_is_allocated_a_real_one() {
        let s = statics();
        let mut c = colony();
        c.pins.retain(|p| p.pin_id == 1);
        c.links.clear();
        let mut next_pin = 900_000_000_000_i64;
        let mut next_route = 1_i64;
        let quote = NoResourceField;
        // A one-element tuple, which EvEJS's namespace check would read as
        // namespace 7 and refuse.
        let temp = Id::Temporary(vec![7]);
        assert!(temp.is_temporary());
        let report = {
            let mut cx = ctx(T0, &mut next_pin, &mut next_route, &quote);
            c.apply(
                &s,
                &[
                    Command::CreatePin {
                        pin_id: temp.clone(),
                        type_id: TYPE_STORAGE,
                        point: SurfacePoint::new(1.5, 0.02),
                    },
                    Command::CreateLink {
                        endpoint1: Id::Real(1),
                        endpoint2: temp.clone(),
                        level: 0,
                    },
                ],
                &mut cx,
            )
            .expect("a silo beside the command centre is a legal colony")
        };
        let real = report
            .id_map
            .iter()
            .find(|(k, _)| *k == temp)
            .map(|(_, v)| *v)
            .expect("the temporary id must be mapped");
        assert_eq!(real, 900_000_000_000);
        assert_eq!(next_pin, 900_000_000_001, "the allocator advanced");
        assert!(
            c.pin(real).is_some(),
            "and the pin exists under the real id"
        );
        assert_eq!(report.created_pins, vec![real]);
        // The link in the same stream resolved the temporary id too.
        assert!(c.link(1, real).is_some());
    }

    /// **A stream that fails anywhere leaves the colony exactly as it was.**
    ///
    /// This is the property EvEJS does not have: with no transaction it grew
    /// a durable receipt ledger inside the items table, a sync fence and
    /// per-domain compensating sagas. Here it is a clone and a swap, so it is
    /// worth a test that actually tries to half-apply a stream.
    #[test]
    fn a_stream_that_fails_leaves_the_colony_untouched() {
        let s = statics();
        let mut c = armed_colony(4000);
        let before = c.clone();
        let mut next_pin = 900_000_000_000_i64;
        let mut next_route = 10_i64;
        let quote = NoResourceField;
        let mut cx = ctx(T0, &mut next_pin, &mut next_route, &quote);
        let err = c
            .apply(
                &s,
                &[
                    // Legal on its own.
                    Command::SetLinkLevel {
                        endpoint1: Id::Real(3),
                        endpoint2: Id::Real(4),
                        level: 4,
                    },
                    // And then one that cannot be: a second command centre.
                    Command::CreatePin {
                        pin_id: Id::Temporary(vec![1, 1]),
                        type_id: TYPE_COMMAND_CENTRE,
                        point: SurfacePoint::new(1.5, 0.3),
                    },
                ],
                &mut cx,
            )
            .expect_err("a second command centre must be refused");
        assert_eq!(err, PiError::CannotBuildMultipleCommandPins);
        assert_eq!(c, before, "the earlier command must not have landed");
        assert_eq!(next_pin, 900_000_000_000, "and no id was consumed");
    }

    /// Nothing can be built before the command centre, and only one of it.
    #[test]
    fn the_command_centre_is_required_first_and_allowed_once() {
        let s = statics();
        let mut empty = Colony::new(1, OWNER, PLANET_BARREN, T0);
        assert_eq!(
            empty.validate_create_pin(&s, TYPE_STORAGE),
            Err(PiError::CannotManagePlanetWithoutCommandCenter)
        );
        assert_eq!(empty.validate_create_pin(&s, TYPE_COMMAND_CENTRE), Ok(()));
        empty.pins.push(Pin::new(
            1,
            TYPE_COMMAND_CENTRE,
            OWNER,
            SurfacePoint::new(1.5, 0.0),
            T0,
        ));
        assert_eq!(
            empty.validate_create_pin(&s, TYPE_COMMAND_CENTRE),
            Err(PiError::CannotBuildMultipleCommandPins)
        );
        assert_eq!(empty.validate_create_pin(&s, TYPE_STORAGE), Ok(()));
        // A link type is not a pin type at all.
        assert_eq!(
            empty.validate_create_pin(&s, TYPE_LINK),
            Err(PiError::InvalidPinType)
        );
    }

    /// `attributePlanetRestriction` keeps a Barren ECU off a lava planet.
    #[test]
    fn a_pin_restricted_to_another_planet_type_is_refused() {
        let s = statics();
        let mut lava = Colony::new(1, OWNER, 2015, T0);
        lava.pins.push(Pin::new(
            1,
            TYPE_COMMAND_CENTRE,
            OWNER,
            SurfacePoint::new(1.5, 0.0),
            T0,
        ));
        assert_eq!(
            lava.validate_create_pin(&s, TYPE_ECU),
            Err(PiError::CannotCreatePinWrongPlanetType),
            "the fixture's ECU is restricted to the fixture planet type"
        );
        // The same ECU is fine on Barren.
        let mut barren = Colony::new(1, OWNER, PLANET_BARREN, T0);
        barren.pins.push(Pin::new(
            1,
            TYPE_COMMAND_CENTRE,
            OWNER,
            SurfacePoint::new(1.5, 0.0),
            T0,
        ));
        assert_eq!(barren.validate_create_pin(&s, TYPE_ECU), Ok(()));
    }

    /// Removing a pin takes its links and routes with it.
    ///
    /// A link or route that outlived its endpoint is the one thing a colony
    /// must never hold: every later read would refuse, including the read the
    /// PI window does to draw itself.
    #[test]
    fn removing_a_pin_removes_what_was_attached_to_it() {
        let s = statics();
        let mut c = armed_colony(4000);
        let mut next_pin = 900_000_000_000_i64;
        let mut next_route = 10_i64;
        let quote = NoResourceField;
        let mut cx = ctx(T0, &mut next_pin, &mut next_route, &quote);
        c.apply(
            &s,
            &[Command::RemovePin {
                pin_id: Id::Real(3),
            }],
            &mut cx,
        )
        .expect("removing the factory is legal");
        assert!(c.pin(3).is_none());
        assert!(
            c.links.iter().all(|l| l.endpoint1 != 3 && l.endpoint2 != 3),
            "its links went with it"
        );
        assert!(
            c.routes.iter().all(|r| !r.path.contains(&3)),
            "and so did its routes"
        );
        assert_eq!(c.validate(&s, OWNER), Ok(()), "and the result is coherent");
    }

    /// The CPU and power budget, and the fact that there is none without a
    /// command centre.
    #[test]
    fn the_power_budget_is_the_command_centres_and_zero_without_one() {
        let s = statics();
        let c = armed_colony(4000);
        let (cpu_supply, cpu_usage, power_supply, power_usage) = c.cpu_power(&s);
        assert_eq!(
            (cpu_supply, power_supply),
            (7_057, 9_000),
            "the fixture is a level-1 command centre"
        );
        assert!(cpu_usage > 0 && power_usage > 0);
        assert_eq!(c.validate(&s, OWNER), Ok(()), "and it fits");
        // Strip the command centre and the supply is zero, not the level-0
        // figure: a colony with no command pin has no power plant.
        let mut headless = c.clone();
        headless.pins.retain(|p| p.pin_id != 1);
        let (cpu, _, power, _) = headless.cpu_power(&s);
        assert_eq!((cpu, power), (0, 0));
        // Each head on an ECU costs CPU and power. Ten of them must not be
        // free, which is the single biggest balance lever in PI.
        let one_head = c.pin(2).unwrap().cpu_power(&s);
        let mut ten = c.pin(2).unwrap().clone();
        ten.heads.clear();
        for head_id in 0..10 {
            ten.heads.push(Head {
                head_id,
                latitude: 1.5,
                longitude: 0.01,
            });
        }
        let ten_heads = ten.cpu_power(&s);
        assert!(
            ten_heads.0 > one_head.0 && ten_heads.1 > one_head.1,
            "heads must cost: {one_head:?} vs {ten_heads:?}"
        );
    }

    /// Too many pins overruns the budget and the whole colony is refused.
    #[test]
    fn a_colony_past_its_power_budget_is_refused() {
        let s = statics();
        let mut c = colony();
        // Launchpads are 3600 CPU each; a level-1 centre supplies 7057.
        for id in 5..=8 {
            c.pins.push(Pin::new(
                id,
                TYPE_LAUNCHPAD,
                OWNER,
                SurfacePoint::new(1.5, 0.05 + 0.01 * id as f64),
                T0,
            ));
            c.links.push(Link::new(1, id, TYPE_LINK, 0));
        }
        assert_eq!(
            c.validate(&s, OWNER),
            Err(PiError::CannotAddToColonyCpuUsageExceeded)
        );
        // Upgrading the command centre pays for it.
        c.level = 5;
        assert_eq!(c.validate(&s, OWNER), Ok(()));
    }

    /// The command centre upgrades up and never down.
    #[test]
    fn the_command_centre_upgrades_in_one_direction_only() {
        let s = statics();
        let mut c = armed_colony(4000);
        let mut next_pin = 900_000_000_000_i64;
        let mut next_route = 10_i64;
        let quote = NoResourceField;
        {
            let mut cx = ctx(T0, &mut next_pin, &mut next_route, &quote);
            c.apply(
                &s,
                &[Command::UpgradeCommandCenter {
                    pin_id: Id::Real(1),
                    level: 3,
                }],
                &mut cx,
            )
            .expect("1 -> 3 is an upgrade");
        }
        assert_eq!(c.level, 3);
        for (level, want) in [
            (3, PiError::CannotDowngradeCommandCenter),
            (2, PiError::CannotDowngradeCommandCenter),
            (6, PiError::CannotUpgradeCommandCenterPastMaximum),
        ] {
            let mut cx = ctx(T0, &mut next_pin, &mut next_route, &quote);
            assert_eq!(
                c.apply(
                    &s,
                    &[Command::UpgradeCommandCenter {
                        pin_id: Id::Real(1),
                        level,
                    }],
                    &mut cx,
                )
                .err(),
                Some(want),
                "level {level}"
            );
        }
    }

    /// A schematic may only go on a factory type its own `pins` set names,
    /// and switching recipes keeps only what the new one wants.
    #[test]
    fn a_schematic_is_refused_on_the_wrong_pin_and_keeps_only_what_it_wants() {
        let s = statics();
        let mut silo = Pin::new(4, TYPE_LAUNCHPAD, OWNER, SurfacePoint::new(1.0, 0.0), T0);
        assert_eq!(
            silo.install_schematic(&s, SCHEMATIC_WATER),
            Err(PiError::CannotAssignSchematicToPinType),
            "a launchpad is not a factory"
        );
        let mut factory = Pin::new(
            3,
            TYPE_BASIC_FACTORY,
            OWNER,
            SurfacePoint::new(1.0, 0.0),
            T0,
        );
        assert_eq!(
            factory.install_schematic(&s, 99_999),
            Err(PiError::SchematicDoesNotExist)
        );
        factory.install_schematic(&s, SCHEMATIC_WATER).unwrap();
        // Over-fill the buffer and stock something the recipe does not want,
        // then reinstall: the surplus is clamped and the stranger dropped.
        factory.contents.insert(TYPE_AQUEOUS_LIQUIDS, 9999);
        factory.contents.insert(TYPE_WATER, 50);
        factory.install_schematic(&s, SCHEMATIC_WATER).unwrap();
        assert_eq!(
            factory.quantity(TYPE_AQUEOUS_LIQUIDS),
            3000,
            "clamped down to one cycle's demand"
        );
        assert_eq!(
            factory.quantity(TYPE_WATER),
            0,
            "and a commodity the recipe does not want is dropped"
        );
        assert_eq!(factory.state, state::IDLE);
    }

    /// Installing a program with no resource field is **refused**, not faked.
    ///
    /// This is the one part of PI the client cannot give us: the
    /// spherical-harmonic resource blob is server-authored and the evaluator
    /// is native. [`NoResourceField`] is the honest stand-in, and the
    /// refusal is what keeps an extractor from looking installed while
    /// yielding nothing.
    #[test]
    fn installing_a_program_without_a_resource_field_is_refused_not_faked() {
        let s = statics();
        let mut c = colony();
        c.pin_mut(2).unwrap().heads.push(Head {
            head_id: 0,
            latitude: 1.5,
            longitude: 0.01,
        });
        let mut next_pin = 900_000_000_000_i64;
        let mut next_route = 10_i64;
        let none = NoResourceField;
        {
            let mut cx = ctx(T0, &mut next_pin, &mut next_route, &none);
            assert_eq!(
                c.apply(
                    &s,
                    &[Command::InstallProgram {
                        pin_id: Id::Real(2),
                        program_type: Some(TYPE_AQUEOUS_LIQUIDS),
                        head_radius: RADIUS_DRILLAREAMIN,
                    }],
                    &mut cx,
                )
                .err(),
                Some(PiError::ProgramQuoteIsZero)
            );
        }
        assert_eq!(
            c.pin(2).unwrap().program_type,
            None,
            "and nothing was installed"
        );
        // With a quote it installs, and the program's shape is the slider's.
        let quote = FixedQuote(1000);
        let mut cx = ctx(T0, &mut next_pin, &mut next_route, &quote);
        c.apply(
            &s,
            &[Command::InstallProgram {
                pin_id: Id::Real(2),
                program_type: Some(TYPE_AQUEOUS_LIQUIDS),
                head_radius: RADIUS_DRILLAREAMIN,
            }],
            &mut cx,
        )
        .expect("a positive quote installs");
        let ecu = c.pin(2).unwrap();
        assert_eq!(ecu.program_type, Some(TYPE_AQUEOUS_LIQUIDS));
        assert_eq!(ecu.qty_per_cycle, 1000);
        assert_eq!(ecu.cycle_time, CYCLE_15M);
        assert_eq!(ecu.state, state::ACTIVE);
    }

    /// A radius outside `[0.01, 0.05]` is refused, where EvEJS clamps.
    ///
    /// A clamped radius quietly installs a program of a different length than
    /// the one the player was quoted, which is worse than a refusal.
    #[test]
    fn a_bonkers_head_radius_is_refused_rather_than_clamped() {
        let s = statics();
        let quote = FixedQuote(1000);
        for radius in [0.0, 0.009, 0.051, 10.0] {
            let mut pin = Pin::new(2, TYPE_ECU, OWNER, SurfacePoint::new(1.5, 0.01), T0);
            assert_eq!(
                pin.install_program(&s, Some(TYPE_AQUEOUS_LIQUIDS), radius, 1000, T0),
                Err(PiError::SurfacePointOutOfRange),
                "radius {radius}"
            );
        }
        // Stop Program is allowed at any radius, because it does not install
        // one -- the client sends whatever radius the slider happened to be
        // at.
        let mut pin = Pin::new(2, TYPE_ECU, OWNER, SurfacePoint::new(1.5, 0.01), T0);
        pin.heads.push(Head {
            head_id: 0,
            latitude: 1.5,
            longitude: 0.01,
        });
        pin.install_program(&s, Some(TYPE_AQUEOUS_LIQUIDS), 0.03, 1000, T0)
            .unwrap();
        assert_eq!(pin.install_program(&s, None, 0.0, 0, T0 + 10 * MIN), Ok(()));
        assert_eq!(pin.program_type, None);
        // `ClearProgram` leaves cycleTime, headRadius and heads alone, so
        // Restart Extractors can try the same layout again.
        assert_eq!(pin.heads.len(), 1);
        assert!(pin.cycle_time > 0);
        assert_eq!(pin.head_radius, 0.03);
        let _ = quote;
    }

    /// The one-minute edit cooldown, keyed on `installTime`.
    #[test]
    fn a_program_cannot_be_replaced_inside_a_minute() {
        let s = statics();
        let mut pin = Pin::new(2, TYPE_ECU, OWNER, SurfacePoint::new(1.5, 0.01), T0);
        pin.heads.push(Head {
            head_id: 0,
            latitude: 1.5,
            longitude: 0.01,
        });
        pin.install_program(&s, Some(TYPE_AQUEOUS_LIQUIDS), 0.02, 1000, T0)
            .unwrap();
        assert_eq!(PROGRAM_INSTALLATION_COOLDOWN, 60 * SEC);
        assert_eq!(
            pin.install_program(&s, Some(TYPE_AQUEOUS_LIQUIDS), 0.02, 1000, T0 + 59 * SEC),
            Err(PiError::CantInstallProgramNeedsCooldown)
        );
        assert_eq!(
            pin.install_program(&s, Some(TYPE_AQUEOUS_LIQUIDS), 0.02, 1000, T0 + 60 * SEC),
            Ok(())
        );
    }

    /// Heads are locked while a program runs, and bounded by number and by
    /// the ECU's area of influence.
    #[test]
    fn heads_are_bounded_and_locked_while_a_program_runs() {
        let s = statics();
        let mut pin = Pin::new(2, TYPE_ECU, OWNER, SurfacePoint::new(1.5, 0.5), T0);
        let near = |n: f64| Head {
            head_id: n as i64,
            latitude: 1.5 + n * 0.01,
            longitude: 0.5,
        };
        for n in 0..ECU_MAX_HEADS {
            pin.add_head(&s, near(n as f64)).expect("inside the limit");
        }
        assert_eq!(
            pin.add_head(&s, near(99.0)).err(),
            Some(PiError::CannotPlaceHeadLimitReached)
        );
        assert_eq!(
            pin.add_head(&s, near(0.0)).err(),
            Some(PiError::CannotAddHeadAlreadyExists),
            "even at the limit, a duplicate id is the more specific refusal"
        );
        // Outside the area of influence (0.3 rad on the fixture's ECU).
        let mut fresh = Pin::new(2, TYPE_ECU, OWNER, SurfacePoint::new(1.5, 0.5), T0);
        assert_eq!(
            fresh
                .add_head(
                    &s,
                    Head {
                        head_id: 0,
                        latitude: 1.5,
                        longitude: 1.5,
                    }
                )
                .err(),
            Some(PiError::HeadOutsideAreaOfInfluence)
        );
        // With a program running, every head edit is refused.
        fresh.add_head(&s, near(0.0)).unwrap();
        fresh
            .install_program(&s, Some(TYPE_AQUEOUS_LIQUIDS), 0.02, 1000, T0)
            .unwrap();
        assert_eq!(
            fresh.add_head(&s, near(1.0)).err(),
            Some(PiError::CannotEditHeadsWhileProgramActive)
        );
        assert_eq!(
            fresh.remove_head(&s, 0).err(),
            Some(PiError::CannotEditHeadsWhileProgramActive)
        );
        assert_eq!(
            fresh.move_head(&s, 0, SurfacePoint::new(1.51, 0.5)).err(),
            Some(PiError::CannotEditHeadsWhileProgramActive)
        );
        // And a head that is not there cannot be removed or moved.
        let mut idle = Pin::new(2, TYPE_ECU, OWNER, SurfacePoint::new(1.5, 0.5), T0);
        assert_eq!(
            idle.remove_head(&s, 3).err(),
            Some(PiError::HeadDoesNotExist)
        );
        assert_eq!(
            idle.move_head(&s, 3, SurfacePoint::new(1.5, 0.5)).err(),
            Some(PiError::HeadDoesNotExist)
        );
    }

    /// Only one character may edit their own pins.
    #[test]
    fn another_characters_pins_cannot_be_edited() {
        let s = statics();
        let mut c = armed_colony(4000);
        for p in &mut c.pins {
            p.owner_id = OWNER + 1;
        }
        let mut next_pin = 900_000_000_000_i64;
        let mut next_route = 10_i64;
        let quote = NoResourceField;
        for command in [
            Command::RemovePin {
                pin_id: Id::Real(3),
            },
            Command::SetSchematic {
                pin_id: Id::Real(3),
                schematic_id: SCHEMATIC_WATER,
            },
            Command::UpgradeCommandCenter {
                pin_id: Id::Real(1),
                level: 3,
            },
            Command::KillExtractorHead {
                pin_id: Id::Real(2),
                head_id: 0,
            },
            Command::InstallProgram {
                pin_id: Id::Real(2),
                program_type: None,
                head_radius: 0.02,
            },
        ] {
            let mut cx = ctx(T0, &mut next_pin, &mut next_route, &quote);
            assert_eq!(
                c.apply(&s, std::slice::from_ref(&command), &mut cx).err(),
                Some(PiError::CanOnlyManageOwnPins),
                "{command:?}"
            );
        }
    }

    /// Links are stored endpoint-sorted, so an unordered lookup always
    /// finds them and a colony can never hold both `(a,b)` and `(b,a)`.
    #[test]
    fn a_link_is_the_same_link_whichever_way_round_it_is_asked_for() {
        let s = statics();
        let mut c = armed_colony(4000);
        assert!(c.link(2, 3).is_some() && c.link(3, 2).is_some());
        let mut next_pin = 900_000_000_000_i64;
        let mut next_route = 10_i64;
        let quote = NoResourceField;
        let mut cx = ctx(T0, &mut next_pin, &mut next_route, &quote);
        assert_eq!(
            c.apply(
                &s,
                &[Command::CreateLink {
                    endpoint1: Id::Real(3),
                    endpoint2: Id::Real(2),
                    level: 0,
                }],
                &mut cx,
            )
            .err(),
            Some(PiError::LinkAlreadyExists),
            "the reversed pair is the same link"
        );
        // A pin cannot be linked to itself.
        let mut cx = ctx(T0, &mut next_pin, &mut next_route, &quote);
        assert_eq!(
            c.apply(
                &s,
                &[Command::CreateLink {
                    endpoint1: Id::Real(3),
                    endpoint2: Id::Real(3),
                    level: 0,
                }],
                &mut cx,
            )
            .err(),
            Some(PiError::CannotLinkPinToItself)
        );
    }

    /// Replacing a link is `REMOVELINK` then `CREATELINK`, and the colony is
    /// disconnected in between.
    ///
    /// This is why the whole-colony checks run **once at the end** of a
    /// stream rather than per command: per-command validation would refuse an
    /// edit the client was right to submit.
    #[test]
    fn a_stream_may_pass_through_a_state_it_would_be_refused_in() {
        let s = statics();
        let mut c = armed_colony(4000);
        let mut next_pin = 900_000_000_000_i64;
        let mut next_route = 10_i64;
        let quote = NoResourceField;
        let mut cx = ctx(T0, &mut next_pin, &mut next_route, &quote);
        c.apply(
            &s,
            &[
                // Dropping 2-3 orphans the ECU's route, which this same
                // command deletes with it.
                Command::RemoveLink {
                    endpoint1: Id::Real(2),
                    endpoint2: Id::Real(3),
                },
                Command::CreateLink {
                    endpoint1: Id::Real(2),
                    endpoint2: Id::Real(3),
                    level: 2,
                },
            ],
            &mut cx,
        )
        .expect("replacing a link with a better one is legal");
        assert_eq!(c.link(2, 3).unwrap().level, 2);
        assert_eq!(c.validate(&s, OWNER), Ok(()));
    }

    /// The event cap is resumable rather than lossy.
    ///
    /// A capped pass leaves `current_sim_time` where it stopped, so the next
    /// call continues instead of skipping the gap. A cap that advanced the
    /// clock anyway would silently delete however many cycles were left.
    #[test]
    fn a_capped_pass_stops_without_losing_the_rest_of_the_gap() {
        let s = statics();
        let mut c = armed_colony(4000);
        // A one-tick pass cannot be capped, so assert the shape rather than
        // forcing 20,000 events: the clock must never pass the deadline.
        let report = c.run_simulation(&s, T0 + 50 * HOUR);
        assert!(!report.capped);
        assert_eq!(c.current_sim_time, T0 + 50 * HOUR);
        assert!(report.events > 0 && report.events < MAX_SIMULATION_EVENTS);
        assert!(
            report.touched.contains(&2) && report.touched.contains(&3),
            "the ECU and the factory both ran: {:?}",
            report.touched
        );
        // `touched` is sorted and free of duplicates, because the arm uses it
        // to decide which `OnRefreshPins` to send.
        let mut sorted = report.touched.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, report.touched);
    }

    // -----------------------------------------------------------------
    // P5.3: the static file, CreateProgram, launches, transfers, the
    // world file
    // -----------------------------------------------------------------

    /// The four record kinds `tools/pi_statics.py` writes, read back. The
    /// numbers are a fixture; what is asserted is that each field lands where
    /// the simulation reads it.
    #[test]
    fn the_static_file_reads_every_record_kind() {
        let text = "# a comment\n\
            pintype\t990600012\t1063\t0.0\t45000.0\t0\t15=2600.0;49=400.0;1683=0.012000000104308128\n\
            schematic\t99061\t1800\t990600015,990600017\t990600021=3000\t990600022=20\n\
            resource\t990600001\t990600021=0.75;990600024=1.0\n\
            itemtype\t990600022\t1042\t43\t0.19\t0.0\tFixture Product\n\
            somefuturerecord\tignored\n";
        let s = Statics::parse(text).expect("well formed");
        let ecu = s.pin_type(TYPE_ECU).expect("the pin type");
        assert_eq!(ecu.group_id, GROUP_ECU_PINS);
        assert_eq!(ecu.attr(attr::POWER_LOAD), 2600.0);
        assert_eq!(
            ecu.attr(attr::ECU_DECAY_FACTOR),
            0.012000000104308128,
            "a float32-widened default survives the text bit for bit"
        );
        assert_eq!(s.base_price(TYPE_ECU), 45_000.0);
        let w = s.schematic(SCHEMATIC_WATER).expect("the schematic");
        assert_eq!(w.cycle_time, 1800);
        assert_eq!(w.pin_type_ids, vec![TYPE_BASIC_FACTORY, 990_600_017]);
        assert_eq!(w.inputs, vec![(TYPE_AQUEOUS_LIQUIDS, 3000)]);
        assert_eq!(w.outputs, vec![(TYPE_WATER, 20)]);
        assert_eq!(
            s.resources(PLANET_BARREN)
                .and_then(|r| r.get(&TYPE_AQUEOUS_LIQUIDS)),
            Some(&0.75)
        );
        assert_eq!(s.item_rows().len(), 1);
        assert_eq!(s.item_rows()[0].name, "Fixture Product");
        assert_eq!(
            s.volume(TYPE_WATER),
            0.19,
            "an item row also gives the volume"
        );
        assert_eq!(s.counts(), (1, 1, 1, 1));
    }

    /// A malformed row is an error, not a skip: a dropped schematic input
    /// is a factory that runs for free.
    #[test]
    fn a_malformed_static_row_is_refused() {
        assert!(
            Statics::parse("schematic\t99061\t1800\t990600015\t990600021=lots\t990600022=20\n")
                .is_err()
        );
        assert!(Statics::parse("pintype\tnot-a-number\n").is_err());
    }

    /// A field that is the same everywhere, for testing `create_program`'s
    /// geometry without the assumed one.
    struct Flat(f64);
    impl ResourceField for Flat {
        fn value_at(&self, _: i64, _: SurfacePoint) -> f64 {
            self.0
        }
    }

    fn heads(points: &[(f64, f64)]) -> Vec<Head> {
        points
            .iter()
            .enumerate()
            .map(|(i, (lat, lon))| Head {
                head_id: i as i64,
                latitude: *lat,
                longitude: *lon,
            })
            .collect()
    }

    /// Heads far apart do not overlap: `int(maxVolume * sum(values))`.
    #[test]
    fn heads_far_apart_yield_max_volume_times_the_sum() {
        let s = statics();
        let c = colony();
        let r = create_program(
            &s,
            &c,
            2,
            TYPE_ECU,
            TYPE_AQUEOUS_LIQUIDS,
            &heads(&[(1.0, 0.0), (1.0, 1.0), (2.0, 2.0)]),
            0.01,
            &Flat(10.0),
        );
        assert_eq!(
            r.qty_per_cycle,
            (ecu_default::MAX_VOLUME * 30.0).trunc() as i64
        );
        let length = program_length_from_head_radius(0.01);
        let hours = cycle_time_from_program_length(length);
        assert_eq!(r.cycle_time, (hours * HOUR as f64).trunc() as i64);
        assert_eq!(r.num_cycles, (length / hours).trunc() as i64);
    }

    /// Two of the ECU's own heads on one spot overlap completely: the lens
    /// of two equal circles at distance zero is the whole circle, so each
    /// loses `overlapFactor` of its value.
    #[test]
    fn two_own_heads_on_one_spot_each_lose_the_overlap_factor() {
        let s = statics();
        let c = colony();
        let r = create_program(
            &s,
            &c,
            2,
            TYPE_ECU,
            TYPE_AQUEOUS_LIQUIDS,
            &heads(&[(1.0, 0.0), (1.0, 0.0)]),
            0.01,
            &Flat(10.0),
        );
        let each = 10.0 * (1.0 - ecu_default::OVERLAP_FACTOR);
        assert_eq!(
            r.qty_per_cycle,
            (ecu_default::MAX_VOLUME * 2.0 * each).trunc() as i64
        );
    }

    /// Another ECU's head on the same spot, extracting the same resource,
    /// takes (almost) everything: `1 - 2 * overlap * overlapFactor` with a
    /// full overlap. On a different resource it takes nothing.
    #[test]
    fn another_ecu_on_the_same_resource_competes_and_on_another_does_not() {
        let s = statics();
        let mut c = colony();
        c.pins.push(Pin::new(
            9,
            TYPE_ECU,
            OWNER,
            SurfacePoint::new(1.0, 0.0),
            T0,
        ));
        {
            let other = c.pin_mut(9).unwrap();
            other.heads = heads(&[(1.0, 0.0)]);
            other.head_radius = 0.01;
            other.program_type = Some(TYPE_AQUEOUS_LIQUIDS);
        }
        let mine = heads(&[(1.0, 0.0)]);
        let alone = (ecu_default::MAX_VOLUME * 10.0).trunc() as i64;
        let same = create_program(
            &s,
            &c,
            2,
            TYPE_ECU,
            TYPE_AQUEOUS_LIQUIDS,
            &mine,
            0.01,
            &Flat(10.0),
        );
        assert!(
            same.qty_per_cycle < alone / 50,
            "a full overlap with a competing head leaves next to nothing: {}",
            same.qty_per_cycle
        );
        let other_resource =
            create_program(&s, &c, 2, TYPE_ECU, TYPE_WATER, &mine, 0.01, &Flat(10.0));
        assert_eq!(other_resource.qty_per_cycle, alone);
    }

    /// A launch: only from the command centre, only what is there, and
    /// once a minute.
    #[test]
    fn a_launch_takes_what_is_there_from_the_command_centre_only() {
        let s = statics();
        let mut c = colony();
        c.pin_mut(1).unwrap().contents.insert(TYPE_WATER, 10);
        c.pin_mut(4).unwrap().contents.insert(TYPE_WATER, 10);
        let ask = |n: i64| BTreeMap::from([(TYPE_WATER, n)]);
        assert_eq!(
            c.launch(&s, OWNER, 1, &BTreeMap::new(), T0),
            Err(PiError::PleaseSelectCommoditiesToLaunch)
        );
        assert_eq!(
            c.launch(&s, OWNER, 4, &ask(1), T0),
            Err(PiError::CanOnlyLaunchFromCommandCenters),
            "a launchpad's goods leave through a customs office"
        );
        assert_eq!(
            c.launch(
                &s,
                OWNER,
                1,
                &BTreeMap::from([(TYPE_AQUEOUS_LIQUIDS, 1)]),
                T0
            ),
            Err(PiError::CannotLaunchCommoditiesNotFound)
        );
        let got = c.launch(&s, OWNER, 1, &ask(25), T0).expect("launched");
        assert_eq!(got, ask(10), "asking for more launches what is there");
        assert_eq!(c.pin(1).unwrap().quantity(TYPE_WATER), 0);
        assert_eq!(c.pin(1).unwrap().last_launch_time, T0);
        c.pin_mut(1).unwrap().contents.insert(TYPE_WATER, 10);
        assert_eq!(
            c.launch(&s, OWNER, 1, &ask(1), T0 + LAUNCH_CYCLE_TIME - 1),
            Err(PiError::CannotLaunchCommandPinNotReady)
        );
        assert!(
            c.launch(&s, OWNER, 1, &ask(1), T0 + LAUNCH_CYCLE_TIME)
                .is_ok()
        );
    }

    /// An expedited transfer moves only from storage, along links, and
    /// puts the source on cooldown for the transfer time.
    #[test]
    fn an_expedited_transfer_moves_along_links_and_cools_the_source() {
        let s = statics();
        let mut c = colony();
        c.pin_mut(3)
            .unwrap()
            .install_schematic(&s, SCHEMATIC_WATER)
            .unwrap();
        c.pin_mut(4)
            .unwrap()
            .contents
            .insert(TYPE_AQUEOUS_LIQUIDS, 3000);
        let ask = BTreeMap::from([(TYPE_AQUEOUS_LIQUIDS, 3000)]);
        assert_eq!(
            c.expedited_transfer(&s, OWNER, &[3, 4], &ask, T0),
            Err(PiError::RouteFailedValidationExpeditedSourceNotStorage)
        );
        assert_eq!(
            c.expedited_transfer(&s, OWNER, &[4, 2], &ask, T0),
            Err(PiError::RouteFailedValidationExpeditedDestinationCannotAccept),
            "an ECU accepts nothing"
        );
        let moved = c
            .expedited_transfer(&s, OWNER, &[4, 3], &ask, T0)
            .expect("one link, a factory that wants a batch");
        assert_eq!(moved, ask);
        assert_eq!(c.pin(3).unwrap().quantity(TYPE_AQUEOUS_LIQUIDS), 3000);
        assert_eq!(c.pin(4).unwrap().quantity(TYPE_AQUEOUS_LIQUIDS), 0);
        let volume = 3000.0 * s.volume(TYPE_AQUEOUS_LIQUIDS);
        assert_eq!(
            c.pin(4).unwrap().last_run_time,
            T0 + Colony::expedited_transfer_time(1250.0, volume)
        );
        c.pin_mut(4)
            .unwrap()
            .contents
            .insert(TYPE_AQUEOUS_LIQUIDS, 10);
        // Room at the factory again, so the only thing in the way is the
        // source's cooldown.
        c.pin_mut(3).unwrap().contents.clear();
        assert_eq!(
            c.expedited_transfer(
                &s,
                OWNER,
                &[4, 3],
                &BTreeMap::from([(TYPE_AQUEOUS_LIQUIDS, 1)]),
                T0
            ),
            Err(PiError::RouteFailedValidationExpeditedSourceNotReady)
        );
    }

    /// Render then parse is the identity, for every field a pin can carry.
    #[test]
    fn the_world_file_records_round_trip() {
        let s = statics();
        let mut c = colony();
        {
            let ecu = c.pin_mut(2).unwrap();
            ecu.heads = heads(&[(1.234_567_890_123, 0.1), (1.3, 0.2)]);
        }
        c.pin_mut(2)
            .unwrap()
            .install_program(&s, Some(TYPE_AQUEOUS_LIQUIDS), 0.013, 777, T0)
            .unwrap();
        c.pin_mut(3)
            .unwrap()
            .install_schematic(&s, SCHEMATIC_WATER)
            .unwrap();
        c.pin_mut(4).unwrap().contents.insert(TYPE_WATER, 42);
        c.pin_mut(1).unwrap().last_launch_time = T0 + 5;
        c.routes.push(Route {
            route_id: 3,
            owner_id: OWNER,
            path: vec![2, 3],
            commodity_type_id: TYPE_AQUEOUS_LIQUIDS,
            commodity_quantity: 1000,
        });
        let saved = Saved {
            colonies: vec![c],
            next_pin_id: FIRST_PIN_ID + 7,
            next_route_id: 4,
        };
        let mut text = String::new();
        saved.render(&mut text);
        let mut back = Saved::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            assert!(Saved::RECORDS.contains(&f[0]));
            back.parse_record(&f).expect("our own record");
        }
        assert_eq!(back, saved);
    }

    /// A restore never hands out an id a stored pin already has.
    #[test]
    fn a_restore_keeps_the_allocators_ahead_of_the_colonies() {
        let mut c = colony();
        c.pins[0].pin_id = FIRST_PIN_ID + 50;
        let mut world = Colonies::new();
        world.restore(Saved {
            colonies: vec![c],
            next_pin_id: FIRST_PIN_ID,
            next_route_id: 0,
        });
        assert_eq!(world.saved().next_pin_id, FIRST_PIN_ID + 51);
        assert!(world.saved().next_route_id >= 1);
    }

    // -----------------------------------------------------------------
    // Customs offices (W9 L5)
    // -----------------------------------------------------------------
    //
    // The numbers below are a fixture. What is asserted is CCP's formula
    // (`SpaceportPin.GetImportTax`/`GetExportTax`), CCP's type filter
    // (`CanPutTypeInCustomsOffice`), the slim's [V] attribute names, and the
    // invariants: an office per planet, and no commodity made or lost.

    /// An invented office type; [`FIXTURE`] has none (see its doc).
    const TYPE_OFFICE: i64 = 990_600_031;
    /// The fixture's taxed P1: volume and tax multiplier in [`FIXTURE`].
    const TYPE_P1: i64 = 990_600_023;

    fn customs_statics() -> Statics {
        fixture().with_customs_type(CustomsType {
            type_id: TYPE_OFFICE,
            capacity: 35_000.0,
            radius: 8_000.0,
            tax_rate: 0.17,
        })
    }

    fn colony_with_a_launchpad() -> Colony {
        let mut c = Colony::new(40_000_007, 90_000_001, PLANET_BARREN, 0);
        c.pins.push(Pin::new(
            1,
            TYPE_COMMAND_CENTRE,
            90_000_001,
            SurfacePoint::new(1.5, 0.1),
            0,
        ));
        c.pins.push(Pin::new(
            2,
            TYPE_LAUNCHPAD,
            90_000_001,
            SurfacePoint::new(1.5, 0.2),
            0,
        ));
        c
    }

    #[test]
    fn the_static_file_carries_the_office_type_and_the_tax_multipliers() {
        let s = Statics::parse(
            "customsoffice\t990600031\t35000.0\t8000.0\t0.17\n\
             taxmult\t990600023\t400.0\n",
        )
        .expect("well formed");
        let t = s.customs_type().expect("the office row");
        assert_eq!(t.type_id, TYPE_OFFICE);
        assert_eq!(t.capacity, 35_000.0);
        assert_eq!(t.radius, 8_000.0);
        assert_eq!(t.tax_rate, 0.17);
        assert_eq!(s.import_tax_multiplier(TYPE_P1), 400.0);
        assert!(
            Statics::parse("taxmult\t990600023\n").is_err(),
            "a short row is an error"
        );
        // An older file has neither record: no office, and no guess.
        assert!(Statics::parse("").unwrap().customs_type().is_none());
    }

    /// `GetTypeAttribute`'s default for a type with no row is the
    /// attribute's `defaultValue`, which is 1.0 for 1640 **[V]**.
    #[test]
    fn a_commodity_with_no_multiplier_row_reads_the_attributes_default() {
        assert_eq!(
            customs_statics().import_tax_multiplier(9_999),
            DEFAULT_IMPORT_TAX_MULTIPLIER
        );
    }

    /// `qty * pinRate * importTaxMultiplier(type)`, summed, times the rate --
    /// and the export side multiplies by the **import** multiplier too.
    #[test]
    fn the_customs_tax_is_the_spaceports_own_formula() {
        let s = customs_statics();
        let goods: BTreeMap<i64, i64> = [(TYPE_P1, 10)].into_iter().collect();
        let import = s.import_tax(TYPE_LAUNCHPAD, &goods, 0.17);
        let export = s.export_tax(TYPE_LAUNCHPAD, &goods, 0.17);
        assert!(
            (import - 10.0 * 0.5 * 400.0 * 0.17).abs() < 1e-9,
            "{import}"
        );
        assert!(
            (export - 10.0 * 1.0 * 400.0 * 0.17).abs() < 1e-9,
            "{export}"
        );
        // A pin type with no `importTax` row imports for nothing.
        assert_eq!(s.import_tax(TYPE_COMMAND_CENTRE, &goods, 0.17), 0.0);
    }

    #[test]
    fn only_planetary_goods_go_into_an_office() {
        assert!(can_put_type_in_customs_office(1042, 43), "a P1 commodity");
        assert!(can_put_type_in_customs_office(1032, 42), "a raw resource");
        assert!(
            can_put_type_in_customs_office(280, 17),
            "a general commodity"
        );
        assert!(
            !can_put_type_in_customs_office(1034, 17),
            "another commodity group"
        );
        assert!(!can_put_type_in_customs_office(25, 6), "a ship");
        assert!(
            !can_put_type_in_customs_office(1027, 41),
            "a command centre"
        );
    }

    #[test]
    fn an_office_id_names_its_planet_and_nothing_else_does() {
        for planet in [40_000_002, 40_000_123, 40_999_999] {
            assert_eq!(
                customs_office_planet(customs_office_id(planet)),
                Some(planet)
            );
        }
        assert_eq!(customs_office_planet(40_000_002), None);
        assert_eq!(customs_office_planet(90_000_001), None);
        assert_eq!(customs_office_planet(-1), None);
    }

    /// One office over every served planet of the system, none over a moon
    /// or a belt, none in another system, and none without the type row.
    #[test]
    fn every_planet_has_one_office_and_nothing_else_does() {
        let catalogue = crate::celestial::Catalogue::parse(
            "#system\t30000001\t20000001\t10000001\n\
             #system\t30000003\t20000001\t10000001\n\
             planet\t40000002\t11\t5060000.0\t161891117336.0\t21288951986.0\t-73529712226.0\t30000001\t0\t0\n\
             moon\t40000003\t14\t1000000.0\t161891117336.0\t21288951986.0\t-73500000000.0\t30000001\t0\t0\n\
             belt\t40000004\t15\t0.0\t161000000000.0\t21000000000.0\t-73000000000.0\t30000001\t0\t0\n\
             planet\t40000041\t2016\t5130000.0\t-100000000000.0\t0.0\t50000000000.0\t30000003\t0\t0\n",
        );
        let s = customs_statics();
        let here = customs_offices_in(&catalogue, &s, 30_000_001);
        assert_eq!(here.len(), 1);
        let o = here[0];
        assert_eq!(o.planet_id, 40_000_002);
        assert_eq!(o.solar_system_id, 30_000_001);
        assert_eq!(o.type_id, TYPE_OFFICE);
        assert_eq!(customs_office(&catalogue, &s, o.item_id), Some(o));
        assert_eq!(customs_offices_in(&catalogue, &s, 30_000_003).len(), 1);
        assert_eq!(
            customs_office(&catalogue, &s, customs_office_id(40_000_003)),
            None
        );
        assert!(customs_offices_in(&catalogue, &Statics::new(), 30_000_001).is_empty());
        // Clear of the planet: a ship that reaches it is not inside the body.
        let d = (0..3)
            .map(|i| (o.position[i] - [161891117336.0, 21288951986.0, -73529712226.0][i]).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(d > 5_060_000.0 + o.radius, "{d}");
    }

    /// The slim attributes PI's client code reads off an office **[V]**,
    /// and the ball the slim names.
    #[test]
    fn an_offices_slim_carries_what_the_client_reads_off_it() {
        let catalogue = crate::celestial::Catalogue::parse(
            "#system\t30000001\t20000001\t10000001\n\
             planet\t40000002\t11\t5060000.0\t1.0e11\t0.0\t-7.0e10\t30000001\t0\t0\n",
        );
        let o = customs_offices_in(&catalogue, &customs_statics(), 30_000_001)[0];
        let eve_marshal::PyValue::Instance { state, .. } = o.slim() else {
            panic!("a foo.SlimItem instance")
        };
        let get = |k: &str| crate::pyutil::dict_get(&state, k.as_bytes()).cloned();
        use eve_marshal::PyValue;
        assert_eq!(get("itemID"), Some(PyValue::Int(o.item_id)));
        assert_eq!(get("typeID"), Some(PyValue::Int(TYPE_OFFICE)));
        assert_eq!(get("planetID"), Some(PyValue::Int(40_000_002)));
        assert_eq!(get("ownerID"), Some(PyValue::Int(OWNER_INTERBUS)));
        for key in [
            "level",
            "orbitalState",
            "orbitalTimestamp",
            "orbitalHackerID",
            "orbitalHackerProgress",
        ] {
            assert!(get(key).is_some(), "the slim must carry {key}");
        }
        assert_eq!(o.ball().ball_id, o.item_id);
        assert_eq!(o.ball().radius, 8_000.0);
    }

    /// Import then export of the same goods: the launchpad ends as it began,
    /// and every unit moved one way came back the other.
    #[test]
    fn an_import_and_its_export_conserve_every_unit() {
        let s = customs_statics();
        let mut c = colony_with_a_launchpad();
        let goods: BTreeMap<i64, i64> = [(TYPE_P1, 1_000)].into_iter().collect();
        c.import_commodities(&s, 90_000_001, 2, &goods)
            .expect("room for it");
        assert_eq!(c.pin(2).unwrap().quantity(TYPE_P1), 1_000);
        c.export_commodities(&s, 90_000_001, 2, &goods)
            .expect("it is there");
        assert_eq!(c.pin(2).unwrap().quantity(TYPE_P1), 0);
        assert!(c.pin(2).unwrap().contents.is_empty());
    }

    #[test]
    fn an_import_that_does_not_fit_is_refused_whole() {
        let s = customs_statics();
        let mut c = colony_with_a_launchpad();
        // 10,000 m3 / 0.38 m3 = 26,315 units fit; one more does not.
        let goods: BTreeMap<i64, i64> = [(TYPE_P1, 26_316)].into_iter().collect();
        assert_eq!(
            c.import_commodities(&s, 90_000_001, 2, &goods),
            Err(PiError::CannotImportNotEnoughWarehouseSpace)
        );
        assert_eq!(c.pin(2).unwrap().quantity(TYPE_P1), 0);
    }

    /// `GetImportEndpoints` lists spaceports that are not command centres
    /// **[V]**, so neither direction goes through a command centre.
    #[test]
    fn a_command_centre_is_no_customs_endpoint() {
        let s = customs_statics();
        let mut c = colony_with_a_launchpad();
        let goods: BTreeMap<i64, i64> = [(TYPE_P1, 1)].into_iter().collect();
        assert_eq!(
            c.import_commodities(&s, 90_000_001, 1, &goods),
            Err(PiError::CannotImportEndpointNotSpaceport)
        );
        assert_eq!(
            c.export_commodities(&s, 90_000_001, 1, &goods),
            Err(PiError::CannotExportEndpointNotSpaceport)
        );
        assert_eq!(
            c.import_commodities(&s, 90_000_001, 99, &goods),
            Err(PiError::CannotImportEndpointNotFound)
        );
        assert_eq!(
            c.import_commodities(&s, 90_000_002, 2, &goods),
            Err(PiError::CanOnlyManageOwnPins)
        );
    }

    #[test]
    fn an_export_of_more_than_is_there_names_both_figures() {
        let s = customs_statics();
        let mut c = colony_with_a_launchpad();
        c.pin_mut(2).unwrap().contents.insert(TYPE_P1, 5);
        let want: BTreeMap<i64, i64> = [(TYPE_P1, 6)].into_iter().collect();
        assert_eq!(
            c.export_commodities(&s, 90_000_001, 2, &want),
            Err(PiError::CannotExportCommodityNotEnough {
                type_id: TYPE_P1,
                contained: 5,
                desired: 6
            })
        );
        let other: BTreeMap<i64, i64> = [(9_999, 1)].into_iter().collect();
        assert_eq!(
            c.export_commodities(&s, 90_000_001, 2, &other),
            Err(PiError::CannotExportCommodityNotFound { type_id: 9_999 })
        );
        assert_eq!(c.pin(2).unwrap().quantity(TYPE_P1), 5, "nothing moved");
    }

    // -----------------------------------------------------------------
    // CP60: the static table in `Content`
    // -----------------------------------------------------------------

    /// The registration line (`content_tables::TABLES`) puts the fixture in
    /// every test registry's `Content`, and both views read the same table.
    #[test]
    fn the_fixture_content_carries_the_planet_table() {
        let c = Content::fixture();
        let s = statics_in(&c);
        assert_eq!(s.counts(), fixture().counts());
        assert_eq!(s.pin_kind(TYPE_ECU), Some(PinKind::Ecu));
        assert!(s.schematic(SCHEMATIC_WATER).is_some());
        assert_eq!(s.customs_type(), None, "no office in the fixture");
        let ids: Vec<i64> = item_types_in(&c).iter().map(|t| t.type_id).collect();
        let rows: Vec<i64> = s.item_rows().iter().map(|r| r.type_id).collect();
        assert_eq!(ids, rows, "one item type per item row, in order");
        assert!(ids.contains(&TYPE_COMMAND_CENTRE));
    }

    /// Without a bundle the table's `load` gives what its `fixture` gives.
    #[test]
    fn without_a_bundle_the_planet_table_is_the_fixture() {
        let dir = crate::authored::Dir::compiled_only();
        let src = Sources::new(None, &dir);
        let loaded = (STATICS.load)(&src).expect("loads");
        let loaded = loaded.downcast_ref::<Installed>().expect("an Installed");
        let fixed = (STATICS.fixture)();
        let fixed = fixed.downcast_ref::<Installed>().expect("an Installed");
        assert_eq!(loaded.statics().counts(), fixed.statics().counts());
        assert_eq!(loaded.statics().counts(), fixture().counts());
        assert_eq!(
            loaded.item_types().len(),
            fixed.item_types().len(),
            "the same item types"
        );
    }

    /// Every item row becomes a known, stackable item type with the row's
    /// group, category, volume and name: what the store needs to mint a
    /// command centre or a commodity.
    #[test]
    fn every_item_row_is_a_stackable_item_type() {
        let i = Installed::build(statics());
        assert_eq!(i.item_types().len(), statics().item_rows().len());
        for (t, r) in i.item_types().iter().zip(statics().item_rows()) {
            assert_eq!(
                (t.type_id, t.group_id, t.category_id, t.name),
                (r.type_id, r.group_id, r.category_id, r.name.as_str())
            );
            assert_eq!((t.volume, t.capacity), (r.volume, r.capacity));
            assert!(!t.singleton, "a PI item stacks");
        }
        assert!(Installed::build(Statics::new()).item_types().is_empty());
    }

    /// The file `--planets` and the bundle name: a good one builds, and a
    /// malformed, empty or absent one is an error naming the file. An
    /// override replaces the table in `Content`.
    #[test]
    fn a_static_file_builds_and_a_bad_one_is_an_error_naming_it() {
        let dir = std::env::temp_dir().join(format!("eve-planet-cp60-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.tsv");
        std::fs::write(&good, FIXTURE).unwrap();
        let i = from_file(&good).expect("the fixture file builds");
        assert_eq!(i.statics().counts(), fixture().counts());
        let empty = dir.join("empty.tsv");
        std::fs::write(&empty, "# nothing\n").unwrap();
        let bad = dir.join("bad.tsv");
        std::fs::write(&bad, "pintype\tnot-a-number\n").unwrap();
        for p in [&empty, &bad, &dir.join("absent.tsv")] {
            let e = from_file(p).map(|_| ()).expect_err("refused");
            assert!(e.contains(&p.display().to_string()), "{e}");
        }

        let mut c = content_with(Statics::new());
        assert!(statics_in(&c).is_empty());
        let line = override_in(&mut c, &good).expect("overrides");
        assert!(line.contains(&good.display().to_string()), "{line}");
        assert_eq!(statics_in(&c).counts(), fixture().counts());
        assert!(override_in(&mut c, &bad).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
