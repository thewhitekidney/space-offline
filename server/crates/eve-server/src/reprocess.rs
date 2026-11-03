//! Reprocessing: the tables, the yield maths, and the operation.
//!
//! # Why this module is ahead of mining, compression and industry
//!
//! `docs/port-tracker.md` §3 has the ordering and the reason, which no amount
//! of reading service names would have produced: EvEJS's
//! `miningStaticData.js:14` re-exports *everything* from `../reprocessing`,
//! so **mining's compression is reprocessing's compressed-type map**. The map
//! is [`Table::compressed_type`] below. Reprocessing also works at an NPC
//! station with no structure code at all, which is what makes it a usable
//! first milestone.
//!
//! # The rows come from the content bundle, never from this repository
//!
//! W15 packet CP32b (`docs/content/plan.md` §2.2, `docs/port-tracker.md`
//! §66): the generated `reprocess/tables.rs` is gone. [`Table::from_bundle`]
//! reads the player's own extract (`out/content/`, `--content`), and
//! [`Table::fixture`] is a handful of **invented** rows for tests and for a
//! server started without a bundle. The files, all written by
//! `tools/content.py` (`docs/content/schema.md` §2):
//!
//! | what | bundle file | client source |
//! | --- | --- | --- |
//! | fixed materials, per portion | `types/materials.tsv` | `typematerials.materials` |
//! | randomized materials | `types/materials-random.tsv` | `typematerials.randomizedMaterials` |
//! | the compressed form of a type | `types/compress.tsv` | `compressibletypes` |
//! | `portionSize`, `basePrice` | `types/market.tsv` | `types` |
//! | the reference price `P_ref` | `econ/prices.tsv` | C5 §3.2, over the above |
//! | attributes 790 and 379 | `types/typedata.tsv` | `typedogma` |
//! | per-station efficiency and take | `stations/stations.tsv` | `mapObjects.db` `npcStations` |
//!
//! Row counts and real rows live only in the extractor's `--check` output
//! (`docs/lanes/cp12.md`, `cp24.md`, `cp32b.md`), never here.
//!
//! **The compressed map is a table, not a name match.** EvEJS's
//! `buildCompressedTypeMaps` derives it by looking for a type named
//! `"Compressed " + name`, and against our build that heuristic loses a pair
//! whose source name carries a trailing space. The client ships the mapping
//! as `compressibletypes` and that is what is read.
//!
//! **Attribute 790 is the classifier.** [`Table::has_special_skill_bonuses`]
//! is `bool(attribute 790)`, the client's own definition. EvEJS hardcodes
//! ore/moon-ore/ice skill sets (`database-creator.js:290-306`) that miss two
//! of our build's reprocessing skills, both of them attached exactly to the
//! randomized types; reading 790 from the client sidesteps the classifier.
//!
//! ## `reprocessingFamily` is not here, deliberately
//!
//! EvEJS computes a family (`ore` / `moon_ore` / `ice` / `gas` / `general`)
//! for every type. It is read in exactly two places --
//! `resolveStructureRigEfficiency` and the structure yield bonus -- and both
//! are structure-only. At an NPC station the family is never consulted. When
//! structures land, derive the family from the ore's own group through the
//! client's `attributeRefiningYieldNormalOre` (2444) / `MoonOre` (2445) /
//! `Ice` (2448) rig attributes rather than from a skill-id allowlist.
//!
//! # The yield formula, from the client's own bytecode
//!
//! `reprocessing/util.pyc` and `reprocessing/ui/{util,efficiencyCalculator}.pyc`
//! in `out/pyc/` are CCP's own code for this, and they agree on one formula.
//! `reprocessing/ui/util.GetTypeSkillBonuses` is the readable one:
//!
//! ```text
//!     if TypeHasSpecialReprocessingSkillBonuses(typeID):
//!         ret  = [GetAttributeSkillBonusFromTypeID(typeID)]   # the ore skill
//!         ret += GetSpecialReprocessingSkillBonuses()         # 3385, then 3389
//!     else:
//!         ret  = GetScrapMetalReprocessingSkillBonuses()      # 12196 alone
//! ```
//!
//! and `efficiencyCalculator.CalculateTheoreticalEfficiency` folds it:
//!
//! ```text
//!     totalSkillBonus = 1.0
//!     for (_, bonus) in skillBonuses:
//!         totalSkillBonus *= 1 + bonus / 100
//!     totalImplantBonus = GetImplantBonus(typeID, getTypeAttribute)
//!     ...
//!     return min(1.0, efficiency * (100 + avgBonus) / 100)
//! ```
//!
//! with `GetImplantBonus` returning `1.0` unless
//! `TypeHasSpecialReprocessingSkillBonuses(typeID)` -- **implants do not help
//! scrapmetal**. `reprocessing/util.GetRefiningYieldPercentageForType` is the
//! same number reached through dogma instead: it reads character attribute
//! 378 `refiningYieldPercentage`, which is where 3385, 3389 and the implants
//! have already been folded by their own dogma effects, and multiplies the
//! per-ore skill on top.
//!
//! [`character_multiplier`] is the first form, in the client's term order.
//!
//! ## Where EvEJS differs, and which side the client takes
//!
//! 1. **Implants on scrapmetal.** `reprocessingRuntime.js:588` multiplies
//!    `getActiveReprocessingImplantMultiplier` into the character multiplier
//!    *outside* the `typeHasSpecialReprocessingSkillBonuses` branch, so a
//!    refining implant would raise a module's scrapmetal yield. The client's
//!    `GetImplantBonus` returns a flat `1.0` on that branch. We follow the
//!    client.
//! 2. **Flooring.** EvEJS `buildFixedRecoverables` runs the product through
//!    `floorAfterNormalizing`. The client's
//!    `_GetRecoverablesFromFixedMaterials` does a **bare**
//!    `int(floor(quantity * efficiency))`, over the same integer quantity and
//!    the same `efficiency` double we put in the quote. So normalising would
//!    make our payout differ from the preview the player is looking at.
//!    [`floor_yield`] is the bare floor, and
//!    `tests::the_client_s_bare_floor_is_reproduced` pins a case that
//!    distinguishes them.
//! 3. **Tax rounding.** `getStationTaxRate` passes the *rate* through
//!    `round2`, which quantises it to whole percent. Nothing in the client
//!    reads the rate except to format it, so we keep full precision. **[I]**
//!    on the curve itself -- see [`STANDING_TAX_ZERO_POINT`].
//!
//! # What this module is not
//!
//! - **No structures.** No rigs, no `rigSize`, no security-band multipliers,
//!   no refinery yield bonus, no gas decompression bonus. The attribute ids
//!   are recorded in the constants below so the next lane does not have to
//!   find them again, and nothing here reads them.
//! - **No Equinox gateway.** EvEJS's `reprocessingGatewayProto.js` is
//!   protobuf over HTTPS, a transport we cannot speak at all
//!   (`docs/port-tracker.md` §10.4).
//! - **Scrapmetal rows are kept** (lane `scrap`, tracker §82). CP32b left
//!   them out because `Edit::Create` refused any output without an
//!   `item::ItemType`, and in W15 the hand rows were nearly all there was.
//!   `item::find_in` now reads every type in the bundle's typedata, so a
//!   module, a charge or a hull reprocesses on the client's scrapmetal branch
//!   of [`character_multiplier`] (Scrapmetal Processing 12196 alone, no
//!   implants) at the same station efficiency and tax as ore. Whatever
//!   `typematerials` has no row for -- blueprints, for one -- is still not
//!   reprocessable, which is the client's own data and not a list here. The
//!   two rules the table cannot apply are `service::reprocess`'s, because
//!   they need the store and the item types: an item **with contents** (an
//!   assembled ship with fittings or cargo, a container with something in
//!   it) is not offered, and a scrapmetal type with an output no
//!   `item::find_in` knows is not offered either ([`Table::outputs_known`]).
//! - **The tax is on `P_ref`, not on the market.** C5 §5.2: the price each
//!   output is taxed at is `econ/prices.tsv`'s `P_ref`, and `types/market.tsv`
//!   `basePrice` where a material has no `P_ref`. Neither is the client's
//!   `GetAdjustedAveragePrice`, so [`Quote::total_isk_cost`] is the right shape
//!   and an approximate number. **[A]**; falsified by a capture of a real
//!   quote's `iskCost` disagreeing with `P_ref * received * take`.

use crate::item::{self, Edit, Slot};
use crate::skill::Skills;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Constants. Every one of these is **[V]** against our client: the type ids
// from `inventorycommon/const.pyc` and the attribute ids from
// `dogma/const.pyc`, both read with `tools/pyc27.py`.
// ---------------------------------------------------------------------------

/// `inventorycommon.const.typeReprocessing`. +3% yield per level (attr 379).
pub const TYPE_REPROCESSING: i64 = 3385;
/// `inventorycommon.const.typeReprocessingEfficiency`. +2% per level.
pub const TYPE_REPROCESSING_EFFICIENCY: i64 = 3389;
/// `inventorycommon.const.typeScrapmetalProcessing`. +2% per level, and the
/// *only* skill that applies to a type with no attribute 790.
pub const TYPE_SCRAPMETAL_PROCESSING: i64 = 12196;
/// `inventorycommon.const.typeCredits` — ISK itself.
///
/// Both of the client's recoverable builders open with
/// `if invconst.typeCredits == item.typeID: return []`, so a stack of ISK is
/// refused before anything else is looked at. Reproduced in [`quote`].
pub const TYPE_CREDITS: i64 = 29;

/// `dogma.const.attributeReprocessingSkillType`. The source of
/// [`Table::reprocessing_skills`].
pub const ATTR_REPROCESSING_SKILL_TYPE: i64 = 790;
/// `dogma.const.attributeRefiningYieldMutator`. The source of
/// [`Table::yield_mutators`]. Percent per level for a skill, flat percent for an
/// implant.
pub const ATTR_REFINING_YIELD_MUTATOR: i64 = 379;
/// `dogma.const.attributeRefiningYieldPercentage`, the **character**
/// attribute `reprocessing/util.GetRefiningYieldPercentageForType` reads.
///
/// Not used here: we have no dogma engine, so [`character_multiplier`] folds
/// 3385, 3389 and the implants itself, which is what the client's own UI path
/// does. Recorded because EvEJS looks this up by the name
/// `"refiningYieldPercentageChar"` with `|| 378` as a fallback, and **our
/// build has no attribute of that name** — 378 is `refiningYieldPercentage`.
/// So on our build their lookup lands on the fallback by accident rather than
/// by design.
pub const ATTR_REFINING_YIELD_PERCENTAGE: i64 = 378;

/// Structure-only attributes, recorded so the next lane need not re-find
/// them. **Nothing in this module reads these.** All six verified against
/// `dogma/const.pyc`; note that 3260 is spelled with a capital G in our
/// build, where EvEJS looks it up as `"gasDecompressionEfficiencyBonus"`.
pub mod structure_attrs {
    /// `attributeRigSize`.
    pub const RIG_SIZE: i64 = 1547;
    /// `attributeRefiningYieldMultiplier`, the rig's own base multiplier.
    pub const REFINING_YIELD_MULTIPLIER: i64 = 717;
    /// `strRefiningYieldBonus`, the refinery's percentage bonus.
    pub const STR_REFINING_YIELD_BONUS: i64 = 2722;
    /// `GasDecompressionEfficiencyBonus` — **capital G in build 3619879**.
    pub const GAS_DECOMPRESSION_EFFICIENCY_BONUS: i64 = 3260;
    /// `structureGasDecompressionEfficiencyBonus`.
    pub const STRUCTURE_GAS_DECOMPRESSION_BONUS: i64 = 3261;
    /// `gasDecompressionBaseEfficiency`.
    pub const GAS_DECOMPRESSION_BASE_EFFICIENCY: i64 = 3262;
    /// The three rig yield-class attributes, which are the honest basis for
    /// a reprocessing "family" -- see this module's header. Named
    /// `refiningYieldNormalOres` / `MoonOres` / `Ice` in `dogmaattributes`
    /// and `attributeRefiningYieldNormalOre` (singular) in `dogma/const.pyc`;
    /// the ids are what matter.
    pub const REFINING_YIELD_NORMAL_ORE: i64 = 2444;
    pub const REFINING_YIELD_MOON_ORE: i64 = 2445;
    pub const REFINING_YIELD_ICE: i64 = 2448;
    /// The security-band multipliers a reprocessing rig is scaled by:
    /// `hiSecModifier`, `lowSecModifier`, `nullSecModifier`.
    pub const HI_SEC_MODIFIER: i64 = 2355;
    pub const LOW_SEC_MODIFIER: i64 = 2356;
    pub const NULL_SEC_MODIFIER: i64 = 2357;

    /// The six effects that carry the structure behaviour, by their names in
    /// **our** `dogmaeffects`. All six of EvEJS's ids check out unchanged,
    /// which is worth recording because nothing else in this lane did:
    ///
    /// ```text
    ///   6673  structureReprocessingRigSecurityModification
    ///   6674  structureRigAsteroidOreYield
    ///   6675  structureRigMoonOreYield
    ///   6678  structureRigIceYield
    ///   6989  refineryRefiningYieldBonus
    ///   8369  refineryGasDecompressionEfficiencyBonus
    /// ```
    ///
    /// (EvEJS calls 6989 `EFFECT_REFINERY_REPROCESSING_YIELD_BONUS`; the
    /// client's own name for it is `refineryRefiningYieldBonus`.) A seventh,
    /// **8372 `fleetCompressionLogisticsRangeBonus`**, is in-space
    /// compression's reach and belongs to the mining lane, not here.
    pub const EFFECTS: &[(i64, &str)] = &[
        (6673, "structureReprocessingRigSecurityModification"),
        (6674, "structureRigAsteroidOreYield"),
        (6675, "structureRigMoonOreYield"),
        (6678, "structureRigIceYield"),
        (6989, "refineryRefiningYieldBonus"),
        (8369, "refineryGasDecompressionEfficiencyBonus"),
    ];
}

/// The *fallback* efficiency an NPC station with no row in the table refines
/// at, before skills.
///
/// **[V] as the modal value, and only as that.** The client ships
/// per-station terms in `bin64/staticdata/mapObjects.db`, table
/// `npcStations`, columns `reprocessingEfficiency` and
/// `reprocessingStationsTake`, and they vary: a minority of stations refine
/// below this value and none above it. The extract carries every station's
/// pair in `stations/stations.tsv` (`reproc_eff`, `reproc_take`), so this
/// constant answers only for a station the bundle does not list (or for a
/// server with no bundle). EvEJS's `DEFAULT_STATION_REPROCESSING_EFFICIENCY`
/// agrees with the fallback (**[V2]**) and mistakes it for a universal.
pub const NPC_STATION_EFFICIENCY: f64 = 0.5;

/// The *fallback* base take, at zero standing, for a station with no row.
///
/// **[V] as the modal value**, same source as [`NPC_STATION_EFFICIENCY`]. The
/// take and the efficiency are independent columns and must be looked up
/// together, which is why [`StationTerms`] carries both. EvEJS's
/// `DEFAULT_STATION_REPROCESSING_TAX` agrees with the fallback (**[V2]**).
pub const NPC_STATION_TAX: f64 = 0.05;

/// The standing at which an NPC station's reprocessing tax reaches zero.
///
/// **Still [I], and the weakest number in this file.** `npcStations` has a
/// base take per station and no standing curve; nothing else client-side
/// carries one (`npcstationtaxmodifier.fsdbinary` is a factional-warfare
/// *industry* modifier that no client file calls).
/// `reprocessing/ui/quotes.Quotes.GetQuotes` unpacks `stationTax,
/// stationEfficiency, rawQuotes` straight out of our answer and never
/// recomputes either, so the client cannot contradict us here.
///
/// EvEJS's `NPC_STANDING_TAX_ZERO_POINT` is the same value (**[V2]**). The
/// curve is a function of the station's own base take, which is what
/// [`station_tax`] takes as an argument. It would be settled by a capture of
/// a real `GetQuotes` at a known standing.
pub const STANDING_TAX_ZERO_POINT: f64 = 6.67;

/// `const.refReprocessingTax`: the wallet journal's entry type for the tax.
///
/// **[V]**, and read rather than remembered: `eve/common/lib/appConst.pyc`
/// stores `refReprocessingTax = 127` (the `LOAD_CONST`/`STORE_NAME` pair, the
/// way `industry::ACCOUNT_ENTRY_TYPES` was read). It is the one number the
/// debit needed that this module did not already have, and
/// `docs/port-tracker.md` §15.6 named it as the missing half of
/// "the reprocess tax is quoted and not charged".
///
/// Cross-checked downstream, which is the part that makes it safe to send:
/// `crate::ledger::ENTRY_TYPE_NAME_IDS` maps 127 to message **312131**,
/// `UI/Wallet/EntryTypeNames/ReprocessingTax`, 'Reprocessing Tax' -- so
/// `account.GetEntryTypes` already ships a row for it and the Journal tab
/// renders a name instead of a blank or a `[no messageID: N]`. An entry type
/// the client cannot name is the failure mode `crate::ledger` is built to
/// avoid; this one it can.
pub const ENTRY_TYPE_REPROCESSING_TAX: i64 = 127;

/// One NPC station's reprocessing terms, as `npcStations` states them.
///
/// `tax` is the **base** take -- the raw `reprocessingStationsTake` column,
/// before standing. Run it through [`station_tax`] to get what the station
/// actually charges a given pilot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StationTerms {
    /// `npcStations.stationID`.
    pub station_id: i64,
    /// `npcStations.reprocessingEfficiency` (`reproc_eff`).
    pub efficiency: f64,
    /// `npcStations.reprocessingStationsTake` (`reproc_take`), before
    /// standing.
    pub tax: f64,
}

// ---------------------------------------------------------------------------
// The tables.
// ---------------------------------------------------------------------------

/// One randomized output: a uniform integer in `min..=max`, rolled **once per
/// portion**.
///
/// `random.randint(quantityMin, quantityMax)` in the client's
/// `_GetRecoverablesFromRandomizedMaterials`, summed over the portions picked
/// for this material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomMaterial {
    pub type_id: i64,
    pub min: i64,
    pub max: i64,
}

/// `types/materials.tsv`: fixed materials per portion (`schema.md` §2).
pub const MATERIALS: (&str, &str) = (
    "types/materials.tsv",
    "#type_id\tmaterial_type_id\tquantity",
);
/// `types/materials-random.tsv`: randomized materials.
pub const RANDOM: (&str, &str) = (
    "types/materials-random.tsv",
    "#type_id\tmaterial_type_id\tqmin\tqmax",
);
/// `types/compress.tsv`: `(source, compressed)`.
pub const COMPRESS: (&str, &str) = ("types/compress.tsv", "#type_id\tcompressed_type_id");
/// `types/market.tsv`: `portion_size` and `base_price` are read.
pub const MARKET: (&str, &str) = (
    "types/market.tsv",
    "#type_id\tgroup_id\tcategory_id\tmarket_group_id\tbase_price\tmeta_group_id\t\
     meta_level\ttech_level\tvariation_parent_type_id\tfaction_id\trace_id\t\
     wreck_type_id\tportion_size\tpublished",
);
/// `stations/stations.tsv`: `reproc_eff` and `reproc_take` are read.
pub const STATIONS: (&str, &str) = (
    "stations/stations.tsv",
    "#station_id\tsystem_id\tregion_id\towner_id\toperation_id\tservices\t\
     reproc_eff\treproc_take",
);

/// The data lines [`Table::build`] reads, one field per bundle file.
///
/// `prices` is `econ/prices.tsv` ([`crate::econ_prices::PRICES`]), parsed by
/// that module's own parser. `typedata` holds only the `type` records of
/// `types/typedata.tsv` that mention attribute 790 or 379: [`Files::read`]
/// filters the file as it reads it, so the large file is not kept.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Files {
    pub materials: Vec<String>,
    pub random: Vec<String>,
    pub compress: Vec<String>,
    pub market: Vec<String>,
    pub prices: Vec<String>,
    pub typedata: Vec<String>,
    pub stations: Vec<String>,
}

impl Files {
    /// Every file from the bundle. An absent file is no lines; a file with
    /// the wrong header is an error naming it.
    pub fn read(b: &crate::content::Bundle) -> Result<Files, String> {
        let typedata = match b.path(crate::content::TYPEDATA) {
            None => Vec::new(),
            Some(p) => std::fs::read_to_string(&p)
                .map_err(|e| format!("{}: {e}", p.display()))?
                .lines()
                .filter(|l| l.starts_with("type\t") && (l.contains("790=") || l.contains("379=")))
                .map(str::to_owned)
                .collect(),
        };
        let (prices, prices_header) = crate::econ_prices::PRICES;
        Ok(Files {
            materials: b.data_lines(MATERIALS.0, MATERIALS.1)?,
            random: b.data_lines(RANDOM.0, RANDOM.1)?,
            compress: b.data_lines(COMPRESS.0, COMPRESS.1)?,
            market: b.data_lines(MARKET.0, MARKET.1)?,
            prices: b.data_lines(prices, prices_header)?,
            typedata,
            stations: b.data_lines(STATIONS.0, STATIONS.1)?,
        })
    }

    /// The invented rows behind [`Table::fixture`]. **No client value**:
    /// every id is in an invented range ([`fixture`]) except the protocol
    /// skill ids [`TYPE_REPROCESSING`], [`TYPE_REPROCESSING_EFFICIENCY`] and
    /// [`TYPE_SCRAPMETAL_PROCESSING`], whose attribute-379 values here are
    /// invented too.
    pub fn fixture() -> Files {
        use fixture::*;
        let lines = |rows: &[String]| rows.to_vec();
        let td = |id: i64, cat: i64, attrs: String| {
            format!(
                "type\t{id}\t99001\t{cat}\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t{attrs}\tInvented {id}"
            )
        };
        let market = |id: i64, base: f64, portion: i64| {
            format!("{id}\t99001\t4\t99501\t{base:?}\t\t\t1\t\t\t\t\t{portion}\t1")
        };
        Files {
            materials: lines(&[
                format!("{ORE}\t{MINERAL}\t400"),
                format!("{ORE_MIXED}\t{MINERAL}\t175"),
                format!("{ORE_MIXED}\t{MINERAL_DEAR}\t70"),
                format!("{ORE_MIXED_COMPRESSED}\t{MINERAL}\t175"),
                format!("{ORE_MIXED_COMPRESSED}\t{MINERAL_DEAR}\t70"),
                format!("{GAS_COMPRESSED}\t{GAS}\t1"),
                format!("{MODULE}\t{MINERAL}\t50"),
                format!("{MODULE_RARE}\t{MINERAL}\t20"),
                format!("{MODULE_RARE}\t{MINERAL_DEAR}\t5"),
            ]),
            random: lines(&[format!("{ROLL}\t{MINERAL}\t300\t500")]),
            compress: lines(&[
                format!("{ORE_MIXED}\t{ORE_MIXED_COMPRESSED}"),
                format!("{GAS}\t{GAS_COMPRESSED}"),
            ]),
            market: lines(&[
                market(MINERAL, 2.5, 1),
                market(MINERAL_DEAR, 20.0, 1),
                market(ORE, 10.0, 100),
                market(ORE_MIXED, 10.0, 100),
                market(ORE_MIXED_COMPRESSED, 1000.0, 100),
                market(GAS, 0.0, 10),
                market(GAS_COMPRESSED, 0.0, 1),
                market(ROLL, 5.0, 1),
                market(MODULE, 500.0, 1),
                market(MODULE_RARE, 800.0, 1),
                market(INERT, 100.0, 1),
            ]),
            // `P_ref` for the cheap mineral (it wins over its `basePrice`)
            // and for the gas, whose `basePrice` is 0. The dear mineral has
            // no `P_ref` and falls back to `basePrice`.
            prices: lines(&[
                format!("{MINERAL}\t3.0\tbase\t2.0\t4\t99001\t\t1"),
                format!("{GAS}\t40.0\trecipe\t10.0\t4\t99001\t\t10"),
            ]),
            typedata: lines(&[
                td(ORE, 25, format!("790={ORE_SKILL}.0")),
                td(ORE_MIXED, 25, format!("790={ORE_SKILL}.0")),
                td(ORE_MIXED_COMPRESSED, 25, format!("790={ORE_SKILL}.0")),
                td(GAS_COMPRESSED, 25, format!("790={ORE_SKILL}.0")),
                td(ROLL, 25, format!("790={ROLL_SKILL}.0")),
                td(ORE_SKILL, 16, "379=2.0".into()),
                td(ROLL_SKILL, 16, "379=1.5".into()),
                td(TYPE_REPROCESSING, 16, "379=4.0".into()),
                td(TYPE_REPROCESSING_EFFICIENCY, 16, "379=1.0".into()),
                td(TYPE_SCRAPMETAL_PROCESSING, 16, "379=5.0".into()),
                td(IMPLANT, 20, "379=4.0".into()),
            ]),
            stations: lines(&[
                format!("{STATION_PLAIN}\t39000001\t19000001\t1999001\t99\t7,21\t0.5\t0.05"),
                format!("{STATION_POOR}\t39000001\t19000001\t1999001\t99\t7,21\t0.25\t0.05"),
                format!("{STATION_CHEAP}\t39000001\t19000001\t1999001\t99\t7,21\t0.5\t0.025"),
            ]),
        }
    }
}

/// The invented ids [`Files::fixture`] uses, for tests here and in
/// `service::reprocess`. None is a client id.
pub mod fixture {
    /// The cheap output: `basePrice` 2.5 and `P_ref` 3.0.
    pub const MINERAL: i64 = 990_001;
    /// The dear output: `basePrice` 20.0 and no `P_ref`.
    pub const MINERAL_DEAR: i64 = 990_003;
    /// 400 [`MINERAL`] per 100 units; attribute 790 [`ORE_SKILL`].
    pub const ORE: i64 = 990_101;
    /// 175 [`MINERAL`] and 70 [`MINERAL_DEAR`] per 100 units; compresses.
    pub const ORE_MIXED: i64 = 990_102;
    /// [`ORE_MIXED`]'s compressed form, with the same refine.
    pub const ORE_MIXED_COMPRESSED: i64 = 990_112;
    /// A gas: no fixed yield, `basePrice` 0, `P_ref` 40.0; compresses.
    pub const GAS: i64 = 990_201;
    /// [`GAS`]'s compressed form, which refines 1:1 back into [`GAS`].
    pub const GAS_COMPRESSED: i64 = 990_211;
    /// Randomized only: 300..=500 [`MINERAL`] per portion of 1.
    pub const ROLL: i64 = 990_301;
    /// A scrapmetal type: 50 [`MINERAL`] per unit, no attribute 790 and no
    /// compression pair.
    pub const MODULE: i64 = 990_401;
    /// A scrapmetal type with two outputs, 20 [`MINERAL`] and 5
    /// [`MINERAL_DEAR`] per unit.
    pub const MODULE_RARE: i64 = 990_402;
    /// A type with a market row and no materials at all, as a blueprint has:
    /// not reprocessable.
    pub const INERT: i64 = 990_701;
    /// The ore skill, attribute 379 = 2.0.
    pub const ORE_SKILL: i64 = 990_501;
    /// [`ROLL`]'s skill, attribute 379 = 1.5.
    pub const ROLL_SKILL: i64 = 990_502;
    /// A refining implant, attribute 379 = 4.0.
    pub const IMPLANT: i64 = 990_601;
    /// A station at (0.5, 0.05).
    pub const STATION_PLAIN: i64 = 69_000_001;
    /// A station at (0.25, 0.05).
    pub const STATION_POOR: i64 = 69_000_002;
    /// A station at (0.5, 0.025).
    pub const STATION_CHEAP: i64 = 69_000_003;
}

/// Every reprocessing table, as one value (`docs/content/plan.md` §1.5,
/// ruling 3).
///
/// Production reads it through `reg.content().get::<Table>()`; the pure
/// functions below ([`quote`], [`plan`], [`character_multiplier`]) take
/// `&Table`, so a test can hand them any rows it likes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    /// Built from a bundle's `types/materials.tsv`. `false` for
    /// [`Table::fixture`]: a type the table does not know is then refused
    /// with [`Refusal::NoContent`], because the server has no real rows at
    /// all rather than no row for that type.
    loaded: bool,
    /// `typematerials` fixed materials per portion, by type, ascending by
    /// material.
    materials: BTreeMap<i64, Vec<(i64, i64)>>,
    /// `typematerials` randomized materials, by type.
    randomized: BTreeMap<i64, Vec<RandomMaterial>>,
    /// `types.portionSize` for every type in either map above.
    portion: BTreeMap<i64, i64>,
    /// Attribute 790 `reprocessingSkillType`: type -> skill.
    skill: BTreeMap<i64, i64>,
    /// Attribute 379 `refiningYieldMutator`: skill or implant -> percent.
    mutator: BTreeMap<i64, f64>,
    /// `compressibletypes`: source -> compressed.
    compressed: BTreeMap<i64, i64>,
    /// The tax price of every output material: `P_ref`, else `basePrice`.
    price: BTreeMap<i64, f64>,
    /// `npcStations` reprocessing terms, by station.
    stations: BTreeMap<i64, StationTerms>,
}

/// This module's registration row, one line in `TABLES` in
/// `content_tables.rs` (coordinator-only).
pub const TABLE: crate::content::TableSpec = crate::content::TableSpec {
    name: "reprocess",
    load: boxed_load,
    fixture: boxed_fixture,
};

/// The bundle's rows; the fixture without a bundle, or with one that has no
/// `types/materials.tsv` (an extract from before the `types2` section).
fn boxed_load(src: &crate::content::Sources) -> Result<crate::content::Boxed, String> {
    let table = match src.bundle {
        Some(b) => Table::from_bundle(b)?,
        None => Table::fixture(),
    };
    if src.bundle.is_some() && !table.is_loaded() {
        src.note(crate::content::Origin::Fixture);
    }
    Ok(Box::new(table))
}

fn boxed_fixture() -> crate::content::Boxed {
    Box::new(Table::fixture())
}

/// The message a reprocess of an unknown type answers on a server with no
/// reprocessing rows (plan §2.2).
pub fn no_content_message(type_id: i64) -> String {
    format!(
        "Reprocessing has no data for type {type_id}: this server has no content \
         bundle. Extract one from your client with tools/up.sh and start the \
         server with --content <dir>."
    )
}

/// One file's data line split into exactly `n` TAB-separated columns.
fn columns<'a>(file: &str, i: usize, line: &'a str, n: usize) -> Result<Vec<&'a str>, String> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() != n {
        return Err(format!(
            "{file}: data line {}: {} columns, expected {n}: {line:?}",
            i + 1,
            f.len()
        ));
    }
    Ok(f)
}

/// One column parsed, or an error naming the file, line and column.
fn field<T: std::str::FromStr>(file: &str, i: usize, f: &[&str], c: usize) -> Result<T, String> {
    f[c].trim()
        .parse()
        .map_err(|_| format!("{file}: data line {}: column {c}: {:?}", i + 1, f[c]))
}

/// A finite number, else an error naming the file, line and column.
fn real(file: &str, i: usize, f: &[&str], c: usize) -> Result<f64, String> {
    let v: f64 = field(file, i, f, c)?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!(
            "{file}: data line {}: column {c} is not finite",
            i + 1
        ))
    }
}

impl Table {
    /// The bundle's rows ([`Files::read`], then [`Table::build`]). A bundle
    /// with no `types/materials.tsv` answers [`Table::fixture`].
    pub fn from_bundle(b: &crate::content::Bundle) -> Result<Table, String> {
        if b.path(MATERIALS.0).is_none() {
            return Ok(Table::fixture());
        }
        Table::build(&Files::read(b)?, true)
    }

    /// The invented rows ([`Files::fixture`]), not loaded: what tests and a
    /// server with no bundle run on.
    pub fn fixture() -> Table {
        Table::build(&Files::fixture(), false)
            .unwrap_or_else(|e| unreachable!("the invented rows parse: {e}"))
    }

    /// Build the table from data lines.
    ///
    /// **Every row is kept**, fixed and randomized: the ore, ice and gas rows
    /// mining feeds and the scrapmetal rows (modules, charges, hulls) that
    /// loot feeds. CP32b's selection rule (attribute 790 or a compression
    /// pair) is gone with the reason it had; see the module header. Which
    /// branch of the yield a type takes is the client's
    /// [`Table::has_special_skill_bonuses`], not which rows were kept.
    ///
    /// Prices are kept for the output materials of the kept rows only.
    pub fn build(files: &Files, loaded: bool) -> Result<Table, String> {
        let mut t = Table {
            loaded,
            ..Table::default()
        };

        for (i, line) in files.typedata.iter().enumerate() {
            let file = crate::content::TYPEDATA;
            let f: Vec<&str> = line.split('\t').collect();
            if f.first() != Some(&"type") || f.len() < 13 {
                return Err(format!("{file}: line {}: not a type record", i + 1));
            }
            let type_id: i64 = field(file, i, &f, 1)?;
            for kv in f[12].split(';').filter(|s| !s.trim().is_empty()) {
                let Some((k, v)) = kv.split_once('=') else {
                    return Err(format!("{file}: type {type_id}: expected id=value: {kv:?}"));
                };
                let (Ok(k), Ok(v)) = (k.trim().parse::<i64>(), v.trim().parse::<f64>()) else {
                    return Err(format!("{file}: type {type_id}: bad attribute {kv:?}"));
                };
                if !v.is_finite() {
                    return Err(format!("{file}: type {type_id}: bad attribute {kv:?}"));
                }
                match k {
                    ATTR_REPROCESSING_SKILL_TYPE => {
                        t.skill.insert(type_id, v as i64);
                    }
                    ATTR_REFINING_YIELD_MUTATOR => {
                        t.mutator.insert(type_id, v);
                    }
                    _ => {}
                }
            }
        }

        for (i, line) in files.compress.iter().enumerate() {
            let f = columns(COMPRESS.0, i, line, 2)?;
            t.compressed
                .insert(field(COMPRESS.0, i, &f, 0)?, field(COMPRESS.0, i, &f, 1)?);
        }

        let mut materials: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
        for (i, line) in files.materials.iter().enumerate() {
            let f = columns(MATERIALS.0, i, line, 3)?;
            materials
                .entry(field(MATERIALS.0, i, &f, 0)?)
                .or_default()
                .push((field(MATERIALS.0, i, &f, 1)?, field(MATERIALS.0, i, &f, 2)?));
        }
        for (type_id, mut ms) in materials {
            ms.sort_unstable();
            t.materials.insert(type_id, ms);
        }

        for (i, line) in files.random.iter().enumerate() {
            let f = columns(RANDOM.0, i, line, 4)?;
            t.randomized
                .entry(field(RANDOM.0, i, &f, 0)?)
                .or_default()
                .push(RandomMaterial {
                    type_id: field(RANDOM.0, i, &f, 1)?,
                    min: field(RANDOM.0, i, &f, 2)?,
                    max: field(RANDOM.0, i, &f, 3)?,
                });
        }

        let outputs: std::collections::BTreeSet<i64> = t
            .materials
            .values()
            .flatten()
            .map(|(m, _)| *m)
            .chain(t.randomized.values().flatten().map(|m| m.type_id))
            .collect();
        for (i, line) in files.market.iter().enumerate() {
            let f = columns(MARKET.0, i, line, 14)?;
            let type_id: i64 = field(MARKET.0, i, &f, 0)?;
            if t.materials.contains_key(&type_id) || t.randomized.contains_key(&type_id) {
                t.portion.insert(type_id, field(MARKET.0, i, &f, 12)?);
            }
            if outputs.contains(&type_id) {
                t.price.insert(type_id, real(MARKET.0, i, &f, 4)?);
            }
        }
        // `P_ref` wins over `basePrice` (C5 §5.2).
        let prices = crate::econ_prices::Table::parse(&files.prices)
            .map_err(|e| format!("{}: {e}", crate::econ_prices::PRICES.0))?;
        for (type_id, p) in prices.prices {
            if outputs.contains(&type_id) {
                t.price.insert(type_id, p.ref_price);
            }
        }

        for (i, line) in files.stations.iter().enumerate() {
            let f = columns(STATIONS.0, i, line, 8)?;
            let s = StationTerms {
                station_id: field(STATIONS.0, i, &f, 0)?,
                efficiency: real(STATIONS.0, i, &f, 6)?,
                tax: real(STATIONS.0, i, &f, 7)?,
            };
            t.stations.insert(s.station_id, s);
        }
        Ok(t)
    }

    /// Built from a bundle's rows, rather than the invented fixture.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Every fixed-material type, ascending.
    pub fn reprocessable_types(&self) -> impl Iterator<Item = i64> + '_ {
        self.materials.keys().copied()
    }

    /// Every randomized type, ascending.
    pub fn randomized_types(&self) -> impl Iterator<Item = i64> + '_ {
        self.randomized.keys().copied()
    }

    /// Every compression pair, `(source, compressed)`, ascending by source.
    pub fn compressed_pairs(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.compressed.iter().map(|(s, c)| (*s, *c))
    }

    /// Every attribute-790 pair, `(type, skill)`, ascending by type.
    pub fn reprocessing_skills(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.skill.iter().map(|(t, s)| (*t, *s))
    }

    /// The fixed materials one portion of this type yields, or `&[]`.
    pub fn materials(&self, type_id: i64) -> &[(i64, i64)] {
        self.materials.get(&type_id).map_or(&[], Vec::as_slice)
    }

    /// `portionSize`, which is **1 and not 0** for a type we do not know.
    ///
    /// Zero would be a division by zero in [`quote`]; 1 means "every unit is
    /// a portion", which is what `portionSize` means for most rows.
    pub fn portion_size(&self, type_id: i64) -> i64 {
        self.portion.get(&type_id).map_or(1, |p| (*p).max(1))
    }

    /// The randomized outputs for a type, or `&[]`.
    pub fn randomized(&self, type_id: i64) -> &[RandomMaterial] {
        self.randomized.get(&type_id).map_or(&[], Vec::as_slice)
    }

    /// Does reprocessing this type roll dice?
    pub fn has_randomized_materials(&self, type_id: i64) -> bool {
        !self.randomized(type_id).is_empty()
    }

    /// Anything this module can produce an output for.
    pub fn is_reprocessable(&self, type_id: i64) -> bool {
        !self.materials(type_id).is_empty() || self.has_randomized_materials(type_id)
    }

    /// Attribute 790, the type's own reprocessing skill.
    pub fn reprocessing_skill(&self, type_id: i64) -> Option<i64> {
        self.skill.get(&type_id).copied()
    }

    /// `reprocessing/util.TypeHasSpecialReprocessingSkillBonuses`, which is
    /// `bool(get_type_attribute(typeID, attributeReprocessingSkillType))` and
    /// nothing more.
    ///
    /// It decides three things at once: which skills apply, whether implants
    /// apply at all, and -- in the client's UI -- which colour the row is.
    pub fn has_special_skill_bonuses(&self, type_id: i64) -> bool {
        self.reprocessing_skill(type_id).is_some()
    }

    /// A type the client reprocesses on its **scrapmetal** branch: it has
    /// fixed materials and no attribute 790
    /// (`reprocessing/ui/util.GetTypeSkillBonuses`'s `else`). Modules,
    /// charges and hulls; their yield is [`character_multiplier`]'s `None`
    /// arm.
    pub fn is_scrapmetal(&self, type_id: i64) -> bool {
        !self.materials(type_id).is_empty() && !self.has_special_skill_bonuses(type_id)
    }

    /// Does `known` answer for every fixed output of this type? Vacuously
    /// true for a type with no fixed rows.
    ///
    /// The seam `service::reprocess` passes `item::find_in` through, so a
    /// scrapmetal type whose outputs include one the server cannot create
    /// (`Edit::Create` refuses an unknown type and with it the whole plan)
    /// is not offered rather than quoted and then refused.
    pub fn outputs_known(&self, type_id: i64, known: &dyn Fn(i64) -> bool) -> bool {
        self.materials(type_id).iter().all(|&(m, _)| known(m))
    }

    /// Attribute 379 on a skill or an implant. `0.0` when the type has none,
    /// which makes `1 + 0/100` a no-op rather than a special case.
    pub fn yield_mutator(&self, type_id: i64) -> f64 {
        self.mutator.get(&type_id).copied().unwrap_or(0.0)
    }

    /// The compressed form of an ore, ice or gas type.
    ///
    /// **This is the function mining and compression are waiting on.** It is
    /// a table lookup and not a name match -- see the module header.
    pub fn compressed_type(&self, source_type_id: i64) -> Option<i64> {
        self.compressed.get(&source_type_id).copied()
    }

    /// Can this type be compressed?
    pub fn is_compressible(&self, type_id: i64) -> bool {
        self.compressed_type(type_id).is_some()
    }

    /// Every source type that compresses into this one.
    ///
    /// A `Vec` rather than a single id because the table is keyed the other
    /// way, and EvEJS's map is many-to-one; the shape does not promise what
    /// one build's data happens to be.
    pub fn compression_sources(&self, compressed_type_id: i64) -> Vec<i64> {
        self.compressed
            .iter()
            .filter(|(_, c)| **c == compressed_type_id)
            .map(|(s, _)| *s)
            .collect()
    }

    /// Is this the compressed form of something?
    pub fn is_compressed_type(&self, type_id: i64) -> bool {
        self.compressed.values().any(|c| *c == type_id)
    }

    /// The price an output is taxed at: `econ/prices.tsv` `P_ref`, else
    /// `types/market.tsv` `basePrice`, else 0.0 (C5 §5.2).
    ///
    /// `P_ref` is what charges a gas refine: a gas's `basePrice` is 0, and
    /// the client's own `if price is None: price = 0` would make it free.
    pub fn price(&self, type_id: i64) -> f64 {
        self.price.get(&type_id).copied().unwrap_or(0.0)
    }

    /// Is this material priced at all, whatever the value? Separate from
    /// [`Table::price`] because 0.0 is a real price and "no row" is a gap.
    pub fn is_priced(&self, type_id: i64) -> bool {
        self.price.contains_key(&type_id)
    }

    /// One station's terms, or `None` when the table has no row for it.
    ///
    /// `None` is the honest answer and the callers below turn it into the
    /// fallback; a lookup that silently substituted the fallback would make
    /// a poor station indistinguishable from a good one.
    pub fn station_terms(&self, station_id: i64) -> Option<&StationTerms> {
        self.stations.get(&station_id)
    }

    /// What this station refines at, before skills, falling back to
    /// [`NPC_STATION_EFFICIENCY`].
    pub fn station_efficiency(&self, station_id: i64) -> f64 {
        self.station_terms(station_id)
            .map_or(NPC_STATION_EFFICIENCY, |s| s.efficiency)
    }

    /// What this station takes at zero standing, falling back to
    /// [`NPC_STATION_TAX`]. Run it through [`station_tax`] for a real pilot.
    pub fn station_base_tax(&self, station_id: i64) -> f64 {
        self.station_terms(station_id)
            .map_or(NPC_STATION_TAX, |s| s.tax)
    }
}

// ---------------------------------------------------------------------------
// The yield maths.
// ---------------------------------------------------------------------------

/// A trained level, clamped to the 0..=5 the client's skill bar can draw.
///
/// `reprocessing/ui/util.GetSkillLevel` is `skillLevel or 0` -- `None`
/// becomes 0 -- so an unknown skill contributes a `1 + 0` factor and not a
/// missing one.
fn level_of(skills: Option<&Skills>, type_id: i64) -> i64 {
    skills
        .and_then(|s| s.entries().find(|e| e.type_id == type_id))
        .map_or(0, |e| e.level.clamp(0, 5))
}

/// The character's multiplier on the station's base efficiency, for one type.
///
/// `level` answers "what level is this skill trained to", so the maths can be
/// tested without a `Skills` and without the reprocessing skills existing in
/// `crate::skill::SKILLS` -- which they do not, see
/// [`multiplier_for`]'s note.
///
/// **The term order is the client's**, from `GetTypeSkillBonuses`: the type's
/// own skill first, then Reprocessing, then Reprocessing Efficiency, then the
/// implants. Float multiplication is not associative, this product is about
/// to be floored, and a reordering is therefore occasionally worth one unit
/// of mineral. That is the whole reason the order is written down.
pub fn character_multiplier<F>(table: &Table, type_id: i64, level: F, implants: &[i64]) -> f64
where
    F: Fn(i64) -> i64,
{
    match table.reprocessing_skill(type_id) {
        // The scrapmetal branch: one skill, and **no implants**.
        // `GetImplantBonus` returns a flat 1.0 here. EvEJS applies implants
        // on this branch too; see the module header, divergence 1.
        None => {
            1.0 + (level(TYPE_SCRAPMETAL_PROCESSING) as f64
                * table.yield_mutator(TYPE_SCRAPMETAL_PROCESSING))
                / 100.0
        }
        Some(ore_skill) => {
            let mut m = 1.0;
            for skill in [ore_skill, TYPE_REPROCESSING, TYPE_REPROCESSING_EFFICIENCY] {
                m *= 1.0 + (level(skill) as f64 * table.yield_mutator(skill)) / 100.0;
            }
            // `GetImplantBonus`: `if implantBonus > 0.0`, so an implant with
            // no attribute 379 is skipped rather than multiplied by 1 -- the
            // same answer, but the guard is there in the client and the
            // reason is that a *negative* mutator would otherwise apply.
            for &implant in implants {
                let bonus = table.yield_mutator(implant);
                if bonus > 0.0 {
                    m *= 1.0 + bonus / 100.0;
                }
            }
            m
        }
    }
}

/// [`character_multiplier`] against a character's trained skills.
///
/// **This used to be unreachable, and the comment saying so outlived it.**
/// `crate::skill::SKILLS` held 30 skills and none of 3385, 3389, 12196 or the
/// fifteen ore skills; `Skills::grant` refuses a type id it does not know, so
/// this function could only ever see level 0 and return 1.0. All eighteen
/// landed in `07a58d9` -- ids and names from `types.fsdbinary`, ranks from
/// `typedogma` attribute 275, the rows quoted in `docs/lanes/reprocess.md`
/// §6.4 -- so a trained character now gets the yield they trained for,
/// through `Registry::skills_for`.
pub fn multiplier_for(
    table: &Table,
    type_id: i64,
    skills: Option<&Skills>,
    implants: &[i64],
) -> f64 {
    character_multiplier(table, type_id, |s| level_of(skills, s), implants)
}

/// The production multiplier closure: a character's skills and implants.
///
/// ```ignore
/// let table = reg.content().get::<reprocess::Table>();
/// let m = reprocess::pilot(table, reg.skills_for(actor), &[]);
/// let plan = reprocess::plan(table, actor, docked, &inputs, &facility, &m, ship, None, balance)?;
/// ```
pub fn pilot<'a>(
    table: &'a Table,
    skills: Option<&'a Skills>,
    implants: &'a [i64],
) -> impl Fn(i64) -> f64 + 'a {
    move |type_id| multiplier_for(table, type_id, skills, implants)
}

/// The effective yield: `min(1.0, stationEfficiency * characterMultiplier)`.
///
/// `reprocessing/util._GetEfficiency`, exactly:
///
/// ```text
///     efficiency = min(stationEfficiency * GetCharRefiningYieldPercentageForType(...), 1.0)
/// ```
///
/// The cap is at the top only. A station efficiency of 0 is a station that
/// refines nothing, and no amount of skill lifts it -- which is correct, and
/// is why there is no `max(0.0, ..)` pretending otherwise.
pub fn effective_yield(station_efficiency: f64, character_multiplier: f64) -> f64 {
    (station_efficiency * character_multiplier).min(1.0)
}

/// What an NPC station takes, after standing.
///
/// `base * (1 - min(standing, zero_point) / zero_point)`, floored at zero.
/// Negative standing does not *raise* the tax: the client has no UI for a
/// penalty here and EvEJS clamps the standing at zero, so a hostile station
/// charges the base rate.
pub fn station_tax(base_tax: f64, standing: f64) -> f64 {
    let standing = standing.max(0.0);
    if standing <= 0.0 {
        return base_tax;
    }
    let scale = (1.0 - standing.min(STANDING_TAX_ZERO_POINT) / STANDING_TAX_ZERO_POINT).max(0.0);
    base_tax * scale
}

/// `int(floor(quantity * efficiency))`, and nothing cleverer.
///
/// The client's own expression. See the module header, divergence 2, for why
/// EvEJS's normalised variant is the wrong answer *here* even though it is
/// the arithmetically nicer one.
pub fn floor_yield(quantity: i64, efficiency: f64) -> i64 {
    if quantity <= 0 || efficiency <= 0.0 {
        return 0;
    }
    (quantity as f64 * efficiency).floor() as i64
}

// ---------------------------------------------------------------------------
// The operation.
// ---------------------------------------------------------------------------

/// One output line of a quote.
///
/// The four field names are the client's, from the `Bundle(...)` call in
/// `_GetRecoverablesFromFixedMaterials`: `typeID`, `client`, `unrecoverable`,
/// `iskCost`. `client` is what the player receives and `unrecoverable` is
/// what the refining process loses -- *not* what the station takes as tax,
/// which is paid in ISK.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recoverable {
    pub type_id: i64,
    pub client: i64,
    pub unrecoverable: i64,
    pub isk_cost: f64,
}

/// One item's reprocessing quote.
///
/// Field names match the client's reads. `reprocessing/ui/quotes.pyc` reads
/// `typeID` and `recoverables[].{typeID, client}`;
/// `reprocessing/ui/outputItemAdder.pyc` adds `unrecoverable`, `iskCost` and
/// `totalISKCost`; `reprocessing/ui/randomizedOutputContainer.AddOutputsFromQuote`
/// adds **`efficiency` and `numPortions`**. All **[V]** from the bytecode --
/// `numPortions` in particular is not an EvEJS invention, the window needs it
/// to draw the dice-roll panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Quote {
    pub item_id: i64,
    pub type_id: i64,
    /// `stacksize - stacksize % portionSize`: whole portions only.
    pub quantity_to_process: i64,
    /// `stacksize % portionSize`, left in the hangar untouched.
    pub left_overs: i64,
    pub portions: i64,
    pub efficiency: f64,
    pub recoverables: Vec<Recoverable>,
    pub total_isk_cost: f64,
}

/// One stack offered for reprocessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Input {
    pub item_id: i64,
    pub type_id: i64,
    pub quantity: i64,
    pub slot: Slot,
}

impl Input {
    /// The stack as the store has it.
    pub fn of(item: &item::Item) -> Self {
        Input {
            item_id: item.item_id,
            type_id: item.type_id,
            // A singleton is one object however the row reads, which is the
            // client's `item.singleton == 1 -> 1` rule in EvEJS's
            // `getInventoryQuantity`.
            quantity: if item.singleton { 1 } else { item.quantity },
            slot: item.slot(),
        }
    }
}

/// Why a reprocess was refused.
///
/// Named after the `UserError` the client raises for each, from
/// `reprocessing/util.pyc` and `reprocessing/ui/itemReprocessor.pyc`, so the
/// dispatch arm can map them without a second table. `NotDocked` and
/// `NoActor` have no client-side `UserError`: the client never gets that far
/// because `eveMoniker.GetReprocessingManager` raises
/// `RuntimeError('asked for reprocessing manager in a weird place')` locally
/// when neither `session.stationid` nor `session.structureid` is set.
/// `Eq` is deliberately **not** derived, for the reason
/// `industry::Refusal`'s doc gives: [`Refusal::AccountFunds`] carries ISK as
/// the `f64` the wallet keeps it in, and an `Eq` on a float would be a
/// promise this type cannot make.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// There is no acting character. **Not an error to report** -- the
    /// no-character branch answers empty, see `service::reprocess`.
    NoActor,
    /// The character is in space.
    NotDocked,
    /// `UserError('CannotReprocessActive')`. The client checks this itself in
    /// `_CheckIsRefiningShip`; we check it again because the client's check
    /// is advice, not enforcement.
    ActiveShip(i64),
    /// Nothing in the selection could be reprocessed here.
    NothingToDo,
    /// `UserError('NotEnoughMoney')`: the station's tax is more than the
    /// character has. `(required, balance)`, both in ISK.
    ///
    /// **The refusal exists because the debit must not check.**
    /// `Registry::charge`'s own doc is explicit that it subtracts without
    /// looking at the balance, on the grounds that the caller has already
    /// refused with something the client has a message for -- so this is that
    /// refusal, in the same dispatcher turn and under the same world write
    /// lock, which is the pattern `industry::plan_install` established for
    /// the install fee.
    ///
    /// `NotEnoughMoney` is dialog **1347** in `findings/user-errors.tsv`,
    /// `info`/modal, with loc args `[numeric]amount.isk` and
    /// `[numeric]balance.isk` -- so both numbers are sent and the player is
    /// told the shortfall rather than that nothing happened.
    ///
    /// **[A] that the real server refuses rather than overdrawing.** Nothing
    /// client-side settles it: `reprocessing/util.pyc` bills the tax without
    /// a visible balance check, because on CCP's side that check is in the
    /// accounting layer the client package does not ship. A refusal is the
    /// choice that cannot corrupt a wallet, and it is the same choice
    /// industry made for the install fee. What it costs is visible and
    /// deliberate: **the fixture character's balance is 0.0**, so a fresh
    /// character cannot refine anything with a priced output until somebody
    /// runs `/giveisk`.
    AccountFunds(f64, f64),
    /// The table is not loaded from a bundle ([`Table::is_loaded`]) and this
    /// type is not in its invented rows: the server has no reprocessing data
    /// at all. `service::reprocess` answers [`no_content_message`], which
    /// names `--content` (plan §2.2), instead of `MrNothingToRefine`, which
    /// would tell the player the type cannot be refined.
    NoContent(i64),
}

/// A reprocess, resolved but not yet performed.
///
/// Two halves, because `item::Edit` can express only one of them.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The consume half, ready for `item::Store::apply`.
    ///
    /// One edit per input: `Edit::Destroy` when the whole stack divides into
    /// portions, `Edit::Consume` when there is a remainder to leave behind.
    /// `apply`'s phase one refuses the whole plan if any of them does not add
    /// up, which is exactly the atomicity this operation needs -- a reprocess
    /// that consumed three of five stacks and then failed would be a
    /// disaster with no undo.
    pub consume: Vec<Edit>,
    /// The produce half: `(typeID, quantity)`, merged across every input and
    /// ascending by type id.
    ///
    /// **This cannot be an `item::Edit` today.** `Edit` has `Move`, `Split`,
    /// `Merge`, `Consume` and `Destroy` -- every one of which needs an
    /// existing item -- and reprocessing creates items out of nothing.
    /// `Store::spawn` is not a substitute: it is clamped to 1..=1000, forces
    /// `FLAG_HANGAR`, and sits outside `apply`, so using it would split this
    /// operation across two non-atomic calls and cap an ore refine at
    /// 1,000 units of output. `docs/lanes/reprocess.md` has the
    /// `Edit::Create { type_id, quantity, at }` variant and both of its
    /// `apply` phases, verbatim, for the coordinator; the moment it exists
    /// this becomes `produce_edits()` and `consume` absorbs it.
    pub produce: Vec<(i64, i64)>,
    /// Where the output goes.
    pub output: Slot,
    /// The input item ids actually processed, in selection order. This is
    /// element 0 of `Reprocess`'s return value.
    pub processed: Vec<i64>,
    /// Total station tax, in ISK.
    pub total_isk_cost: f64,
    /// The per-item quotes this plan was built from, so the caller can push
    /// notifications and bill without recomputing.
    pub quotes: Vec<Quote>,
}

impl Plan {
    /// The whole operation as one `Edit` list: consume, then produce.
    ///
    /// **One list, because it has to be one plan.** `Store::apply`'s phase
    /// one validates everything before phase two changes a byte, so a refusal
    /// leaves the ore where it was. Running the halves as two calls is the
    /// non-atomic saga EvEJS's `reprocessingSettlementState.js` spends 1,277
    /// lines compensating for.
    pub fn edits(&self) -> Vec<Edit> {
        let mut out = self.consume.clone();
        out.extend(
            self.produce
                .iter()
                .map(|&(type_id, quantity)| Edit::Create {
                    type_id,
                    quantity,
                    to: self.output,
                }),
        );
        out
    }
}

/// The station's side of the quote.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Facility {
    /// `GetStationEfficiencyForTypeID`. Per-type in principle -- a structure
    /// with an ore rig refines ore better than ice -- which is why the quote
    /// carries it per type even though an NPC station answers one number.
    pub efficiency: f64,
    /// `GetStationTaxRate`, after standing.
    pub tax: f64,
}

impl Facility {
    /// An NPC station we have no id for, at the given standing with its
    /// owner: the [`NPC_STATION_EFFICIENCY`] / [`NPC_STATION_TAX`] fallback.
    ///
    /// Prefer [`Facility::npc_station_at`] wherever a station id is in hand:
    /// stations differ in both efficiency and take, and this constructor is
    /// the modal pair only.
    pub fn npc_station(standing: f64) -> Self {
        Facility {
            efficiency: NPC_STATION_EFFICIENCY,
            tax: station_tax(NPC_STATION_TAX, standing),
        }
    }

    /// An NPC station **by id**, at the given standing with its owner.
    ///
    /// Both numbers come from the station's own row in the [`Table`]
    /// (`stations/stations.tsv`), and the standing curve is applied to
    /// *that* station's base take -- not to [`NPC_STATION_TAX`] -- which is
    /// the whole point: at a station with half the usual take the curve
    /// sheds half as much per standing point.
    ///
    /// ```
    /// use eve_server::reprocess::{fixture, Facility, Table};
    /// let table = Table::fixture();
    /// // The invented poor station refines at its own rate, not the fallback.
    /// assert_eq!(
    ///     Facility::npc_station_at(&table, fixture::STATION_POOR, 0.0).efficiency,
    ///     table.station_efficiency(fixture::STATION_POOR)
    /// );
    /// // An id with no row gets the fallback, not a panic and not a zero.
    /// assert_eq!(
    ///     Facility::npc_station_at(&table, 1, 0.0),
    ///     Facility::npc_station(0.0)
    /// );
    /// ```
    pub fn npc_station_at(table: &Table, station_id: i64, standing: f64) -> Self {
        Facility {
            efficiency: table.station_efficiency(station_id),
            tax: station_tax(table.station_base_tax(station_id), standing),
        }
    }
}

/// Quote one stack.
///
/// `None` -- not an error -- when there is nothing to quote: a type we have
/// no materials for, ISK, an empty stack, or a stack smaller than one
/// portion. The client's `GetQuotes` is called with whatever the player
/// dragged in and expects the un-refinable entries to simply be absent from
/// the answer, which is why this is `Option` and not `Result`.
///
/// `multiplier` is "the character's multiplier for this type", i.e.
/// [`multiplier_for`] bound to a pilot. A closure rather than a
/// `(skills, implants)` pair because this module has no business reading a
/// skill sheet, and because the server has no implant state at all -- the
/// closure is the seam both arrive through. (It was also the only way to
/// exercise a non-zero level when `crate::skill::SKILLS` held none of the
/// reprocessing skills; it holds all eighteen now, and the production path
/// goes through `Registry::skills_for`.) [`pilot`] builds the production one.
///
/// `include_randomized` mirrors the client's
/// `includeRecoverablesFromRandomizedOutputs`: the *preview* leaves the
/// randomized lines out (the window draws its own dice-roll panel from its
/// own `typematerials` copy) while a caller that wants the expected outputs
/// can ask for them. The randomized lines this produces are the **average**
/// roll and are never what [`plan`] pays out.
pub fn quote(
    table: &Table,
    input: Input,
    facility: &Facility,
    multiplier: &dyn Fn(i64) -> f64,
    include_randomized: bool,
) -> Option<Quote> {
    // `if invconst.typeCredits == item.typeID: return []` opens both of the
    // client's recoverable builders.
    if input.type_id == TYPE_CREDITS || input.quantity <= 0 {
        return None;
    }
    if !table.is_reprocessable(input.type_id) {
        return None;
    }
    let portion_size = table.portion_size(input.type_id);
    let portions = input.quantity / portion_size;
    let left_overs = input.quantity % portion_size;
    if portions <= 0 {
        return None;
    }
    let efficiency = effective_yield(facility.efficiency, multiplier(input.type_id));

    let mut recoverables = Vec::new();
    let mut total_isk_cost = 0.0;
    for &(material, per_portion) in table.materials(input.type_id) {
        let quantity = per_portion * portions;
        let client = floor_yield(quantity, efficiency);
        // `int(round(quantity - receivedQuantity))`. Both are already
        // integers here, so the round is the client's defensiveness about a
        // float subtraction and not arithmetic we need.
        let unrecoverable = (quantity - client).max(0);
        // `round(price * (recoverable * stationsTake), 2)`: the tax is on
        // what the player *receives*, not on the raw yield.
        let isk_cost = round2(table.price(material) * (client as f64 * facility.tax));
        total_isk_cost += isk_cost;
        recoverables.push(Recoverable {
            type_id: material,
            client,
            unrecoverable,
            isk_cost,
        });
    }

    // `if recoverables: ... else: randomized`. The fixed list wins when it
    // exists; in build 3619879 no type has both, but the client's order is
    // the one to keep.
    if recoverables.is_empty() {
        // `round(portions * expectedMarketValuePerBatch * stationsTake * efficiency, 2)`
        // -- note the efficiency is applied to the *ISK* here, where the
        // fixed path applies it to the quantity and taxes the result.
        let expected: f64 = table
            .randomized(input.type_id)
            .iter()
            .map(|m| table.price(m.type_id) * average_roll(m))
            .sum();
        total_isk_cost = round2(portions as f64 * expected * facility.tax * efficiency);
        if include_randomized {
            for m in table.randomized(input.type_id) {
                let quantity = (average_roll(m) * portions as f64).floor() as i64;
                let client = floor_yield(quantity, efficiency);
                recoverables.push(Recoverable {
                    type_id: m.type_id,
                    client,
                    unrecoverable: (quantity - client).max(0),
                    // Zero, in the client too: the randomized path bills the
                    // whole batch in `totalISKCost` and attributes none of it
                    // per line.
                    isk_cost: 0.0,
                });
            }
        }
    }

    Some(Quote {
        item_id: input.item_id,
        type_id: input.type_id,
        quantity_to_process: input.quantity - left_overs,
        left_overs,
        portions,
        efficiency,
        recoverables,
        total_isk_cost: round2(total_isk_cost),
    })
}

/// The mean of a uniform `randint(min, max)`.
///
/// `(min + max) / 2`, which is EvEJS's
/// `buildAverageRandomizedOutputs`. **There is no `weight`/
/// `relativeProbability` term**: their static layer carries one and divides
/// by its sum, and in our build every randomized entry is twelve bytes with
/// no room for it -- their own generated `data.json` agrees, reporting
/// `relativeProbabilityTypeCount: 0`. So the weighting machinery is dead code
/// for build 3619879 and the probability is uniform over the entries.
fn average_roll(m: &RandomMaterial) -> f64 {
    (m.min + m.max) as f64 / 2.0
}

/// `round(x, 2)`, Python's banker-free `round` being close enough at two
/// decimal places on ISK.
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Resolve a whole reprocess.
///
/// `active_ship` is the item id the client passed to `GetQuotes` and checked
/// itself; we check it again because a client-side check is advice.
/// `output` is where the results go -- the client resolves it from the sample
/// item when the player picked nothing, so by the time the call reaches us it
/// is concrete, and `None` falls back to the first input's own slot for the
/// same reason.
///
/// `balance` is the character's wallet, in ISK, and the plan is refused with
/// [`Refusal::AccountFunds`] when the station's tax exceeds it. It is a
/// parameter rather than something this module reads because `crate::reprocess`
/// knows nothing about `Registry` and should not -- the same seam
/// `multiplier` uses for skills.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    table: &Table,
    actor: Option<i64>,
    docked: Option<i64>,
    inputs: &[Input],
    facility: &Facility,
    multiplier: &dyn Fn(i64) -> f64,
    active_ship: Option<i64>,
    output: Option<Slot>,
    balance: f64,
) -> Result<Plan, Refusal> {
    if actor.is_none() {
        return Err(Refusal::NoActor);
    }
    if docked.is_none() {
        return Err(Refusal::NotDocked);
    }
    // Before anything else, and over the *whole* selection: the client's
    // `Reprocess` runs `_CheckIsRefiningShip` across every item before it
    // locks or sends anything, so one active ship in the list refuses the
    // batch rather than reprocessing the rest.
    if let Some(ship) = active_ship
        && let Some(i) = inputs.iter().find(|i| i.item_id == ship)
    {
        return Err(Refusal::ActiveShip(i.item_id));
    }

    let mut quotes = Vec::new();
    let mut consume = Vec::new();
    let mut processed = Vec::new();
    let mut produced: Vec<(i64, i64)> = Vec::new();
    let mut total_isk_cost = 0.0;

    for &input in inputs {
        // `includeRecoverablesFromRandomizedOutputs: false` on the execute
        // path in both EvEJS and the client: the averages are a preview and
        // the real roll happens below.
        let Some(q) = quote(table, input, facility, multiplier, false) else {
            if !table.is_loaded()
                && input.type_id != TYPE_CREDITS
                && !table.is_reprocessable(input.type_id)
            {
                return Err(Refusal::NoContent(input.type_id));
            }
            continue;
        };
        if q.quantity_to_process <= 0 || q.portions <= 0 {
            continue;
        }

        for r in &q.recoverables {
            if r.client > 0 {
                merge(&mut produced, r.type_id, r.client);
            }
        }
        // The dice. **Not rolled here**: this module has no randomness and
        // `crate::` has no RNG (`server/deps.allow` is empty, so no `rand`),
        // and a plan that rolled differently every time it was inspected
        // would be untestable. The randomized types therefore yield the
        // *expected* value, deterministically, and the shortfall is recorded
        // rather than hidden: see `docs/lanes/reprocess.md`.
        for m in table.randomized(input.type_id) {
            let quantity = (average_roll(m) * q.portions as f64).floor() as i64;
            let client = floor_yield(quantity, q.efficiency);
            if client > 0 {
                merge(&mut produced, m.type_id, client);
            }
        }

        consume.push(if q.left_overs == 0 {
            Edit::Destroy {
                item_id: input.item_id,
            }
        } else {
            Edit::Consume {
                item_id: input.item_id,
                quantity: q.quantity_to_process,
            }
        });
        processed.push(input.item_id);
        total_isk_cost += q.total_isk_cost;
        quotes.push(q);
    }

    if processed.is_empty() {
        return Err(Refusal::NothingToDo);
    }

    // The tax, settled before anything is consumed. **After
    // `NothingToDo`**, deliberately: a selection that refines nothing costs
    // nothing, so a broke character dragging a Rifter into the window must
    // still be told `MrNothingToRefine` and not that they cannot afford it.
    //
    // `>` and not `>=`: a free refine -- an output with no price, see
    // [`Table::price`] -- costs 0.0 and a wallet
    // of 0.0 covers it.
    let total_isk_cost = round2(total_isk_cost);
    if total_isk_cost > balance {
        return Err(Refusal::AccountFunds(total_isk_cost, balance));
    }

    let output = output
        .or_else(|| inputs.first().map(|i| i.slot))
        .ok_or(Refusal::NothingToDo)?;

    Ok(Plan {
        consume,
        produce: produced,
        output,
        processed,
        total_isk_cost,
        quotes,
    })
}

/// Add to a `(type_id, quantity)` list kept sorted by type id.
///
/// Sorted rather than insertion-ordered because the output dict crosses the
/// wire and a stable order makes a capture diff readable.
fn merge(into: &mut Vec<(i64, i64)>, type_id: i64, quantity: i64) {
    match into.binary_search_by_key(&type_id, |(t, _)| *t) {
        Ok(i) => into[i].1 += quantity,
        Err(i) => into.insert(i, (type_id, quantity)),
    }
}

/// [`Table::fixture`] with its cheap output [`fixture::MINERAL`] swapped for
/// `output`, loaded: for a test that applies a plan, because `Edit::Create`
/// refuses a type `item::find_in` does not know and every fixture id is
/// invented.
#[cfg(test)]
pub(crate) fn fixture_with_output(output: i64) -> Table {
    let from = fixture::MINERAL.to_string();
    let to = output.to_string();
    let swap = |rows: &[String]| -> Vec<String> {
        rows.iter()
            .map(|l| {
                l.split('\t')
                    .map(|c| if c == from { to.as_str() } else { c })
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect()
    };
    let f = Files::fixture();
    let files = Files {
        materials: swap(&f.materials),
        random: swap(&f.random),
        market: swap(&f.market),
        prices: swap(&f.prices),
        ..f
    };
    Table::build(&files, true).unwrap_or_else(|e| unreachable!("the swapped rows parse: {e}"))
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;

    /// Skill levels for a test, by level rather than through a `Skills`
    /// sheet, so these tests pin [`character_multiplier`]'s arithmetic and
    /// not `crate::skill::SKILLS`'s contents.
    fn levels(pairs: &[(i64, i64)]) -> impl Fn(i64) -> i64 + '_ {
        move |id| pairs.iter().find(|(t, _)| *t == id).map_or(0, |(_, l)| *l)
    }

    /// A wallet that covers any tax in this module's tests. The one test
    /// about the refusal passes its own number.
    const RICH: f64 = 1_000_000_000.0;

    /// The acting character in these tests (an invented id).
    const PILOT: i64 = 90_000_001;

    fn station() -> Facility {
        Facility::npc_station(0.0)
    }

    fn ore(item_id: i64, type_id: i64, quantity: i64) -> Input {
        Input {
            item_id,
            type_id,
            quantity,
            slot: Slot::new(PILOT, STATION_PLAIN, crate::model::FLAG_HANGAR),
        }
    }

    /// [`plan`] at the fallback station with no skills.
    fn plan_of(table: &Table, inputs: &[Input], balance: f64) -> Result<Plan, Refusal> {
        plan(
            table,
            Some(PILOT),
            Some(STATION_PLAIN),
            inputs,
            &station(),
            &pilot_at(table, &[], &[]),
            None,
            None,
            balance,
        )
    }

    // -- the bundle ---------------------------------------------------------

    /// The packet's own check: the generated client table is gone, and
    /// nothing in this module can reach it again.
    #[test]
    fn no_generated_table_remains() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/reprocess");
        assert!(
            !dir.join("tables.rs").exists(),
            "src/reprocess/tables.rs is client data (port-tracker §66)"
        );
    }

    /// The registered spec's fixture is [`Table::fixture`], and it is not
    /// loaded: a server with no bundle knows it has no real rows.
    #[test]
    fn the_registered_fixture_is_the_invented_rows_and_is_not_loaded() {
        let built = (TABLE.fixture)()
            .downcast::<Table>()
            .expect("the spec builds a reprocess::Table");
        assert_eq!(*built, Table::fixture());
        assert!(!built.is_loaded());
        let authored = crate::authored::Dir::compiled_only();
        let loaded = (TABLE.load)(&crate::content::Sources::new(None, &authored))
            .expect("no bundle is not an error")
            .downcast::<Table>()
            .expect("a reprocess::Table");
        assert_eq!(*loaded, Table::fixture());
    }

    /// Write `files` as a bundle with the schema headers.
    fn write_bundle(dir: &std::path::Path, files: &Files) {
        let (prices, prices_header) = crate::econ_prices::PRICES;
        let write = |rel: &str, header: Option<&str>, rows: &[String]| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
            let mut text = header.map(|h| format!("{h}\n")).unwrap_or_default();
            for r in rows {
                text.push_str(r);
                text.push('\n');
            }
            std::fs::write(p, text).expect("write");
        };
        write(MATERIALS.0, Some(MATERIALS.1), &files.materials);
        write(RANDOM.0, Some(RANDOM.1), &files.random);
        write(COMPRESS.0, Some(COMPRESS.1), &files.compress);
        write(MARKET.0, Some(MARKET.1), &files.market);
        write(prices, Some(prices_header), &files.prices);
        write(STATIONS.0, Some(STATIONS.1), &files.stations);
        // typedata.tsv has no header; its comments and the type records with
        // neither attribute are filtered out on read.
        let mut td = vec![
            "# a comment".to_owned(),
            "attribute\t790\t0.0\treprocessingSkillType".to_owned(),
            "type\t990999\t99001\t6\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t37=300.0\tA Hull".to_owned(),
        ];
        td.extend(files.typedata.iter().cloned());
        write(crate::content::TYPEDATA, None, &td);
    }

    /// The bundle side reads every file through its schema header, keeps
    /// only the typedata rows it needs, and is loaded; a bundle with no
    /// `types/materials.tsv` answers the fixture; a wrong header is an error
    /// naming the file.
    #[test]
    fn the_bundle_rows_load_through_their_headers() {
        let dir = crate::testkit::scratch_dir("cp32b-reprocess");
        let b = crate::content::Bundle {
            dir: dir.path().to_path_buf(),
        };
        assert_eq!(
            Table::from_bundle(&b).expect("an empty bundle loads"),
            Table::fixture(),
            "no materials file is no reprocessing data"
        );

        write_bundle(dir.path(), &Files::fixture());
        let read = Files::read(&b).expect("reads");
        assert_eq!(
            read,
            Files::fixture(),
            "the typedata filter keeps exactly the 790/379 rows"
        );
        let t = Table::from_bundle(&b).expect("loads");
        assert!(t.is_loaded());
        assert_eq!(t, Table::build(&Files::fixture(), true).expect("builds"));

        std::fs::write(dir.path().join(COMPRESS.0), "#source\ttarget\n1\t2\n").expect("write");
        let e = Table::from_bundle(&b).expect_err("a wrong header refuses");
        assert!(e.contains(COMPRESS.0) && e.contains("header"), "{e}");
    }

    /// A row that is there but wrong is an error naming its file, never a
    /// skip and never a panic.
    #[test]
    fn a_malformed_row_is_an_error_naming_its_file() {
        let bad = |edit: fn(&mut Files)| {
            let mut f = Files::fixture();
            edit(&mut f);
            Table::build(&f, true).expect_err("refused")
        };
        let e = bad(|f| f.materials.push("990101\tx\t1".into()));
        assert!(e.contains(MATERIALS.0), "{e}");
        let e = bad(|f| f.random.push("990301\t990001\t1".into()));
        assert!(e.contains(RANDOM.0), "{e}");
        let e = bad(|f| f.stations.push("1\t2\t3\t4\t5\t\tnan\t0.05".into()));
        assert!(e.contains(STATIONS.0), "{e}");
        let e = bad(|f| {
            f.typedata
                .push("type\t1\t2\t3\t1\t1\t1\t1\t1\t1\t0\t\t790\tx".into())
        });
        assert!(e.contains(crate::content::TYPEDATA), "{e}");
        let e = bad(|f| f.prices.push("1\t-1\tbase\t0\t4\t1\t\t1".into()));
        assert!(e.contains(crate::econ_prices::PRICES.0), "{e}");
    }

    /// Every row is kept: the mining rows, the randomized row and the
    /// scrapmetal rows. A type with no materials (a blueprint's shape) is
    /// still not reprocessable.
    #[test]
    fn every_row_is_kept_mining_and_scrapmetal_alike() {
        let t = Table::fixture();
        for kept in [
            ORE,
            ORE_MIXED,
            ORE_MIXED_COMPRESSED,
            GAS_COMPRESSED,
            ROLL,
            MODULE,
            MODULE_RARE,
        ] {
            assert!(t.is_reprocessable(kept), "type {kept}");
        }
        assert_eq!(
            t.reprocessable_types().collect::<Vec<_>>(),
            vec![
                ORE,
                ORE_MIXED,
                ORE_MIXED_COMPRESSED,
                GAS_COMPRESSED,
                MODULE,
                MODULE_RARE
            ]
        );
        assert_eq!(t.randomized_types().collect::<Vec<_>>(), vec![ROLL]);
        assert!(!t.is_reprocessable(INERT), "no materials row");
        assert_eq!(t.portion_size(INERT), 1);
        for scrap in [MODULE, MODULE_RARE] {
            assert!(t.is_scrapmetal(scrap), "type {scrap}");
        }
        for not_scrap in [ORE, ORE_MIXED_COMPRESSED, GAS_COMPRESSED, ROLL, INERT] {
            assert!(!t.is_scrapmetal(not_scrap), "type {not_scrap}");
        }
        assert!(t.is_priced(MINERAL_DEAR), "a scrapmetal output is priced");
    }

    /// [`Table::outputs_known`] asks about every fixed output, and is true
    /// for a type with none.
    #[test]
    fn outputs_known_asks_about_every_fixed_output() {
        let t = Table::fixture();
        let only_cheap = |m: i64| m == MINERAL;
        assert!(t.outputs_known(MODULE, &only_cheap));
        assert!(!t.outputs_known(MODULE_RARE, &only_cheap));
        assert!(t.outputs_known(MODULE_RARE, &|m| m == MINERAL || m == MINERAL_DEAR));
        assert!(t.outputs_known(INERT, &|_| false));
    }

    /// A scrapmetal type refines on the client's scrapmetal branch:
    /// `min(1, station * (1 + L * m / 100))` with Scrapmetal Processing
    /// alone, floored per output, and taxed at the station's take like ore.
    /// The ore skills and an implant change nothing.
    #[test]
    fn a_module_reprocesses_at_the_scrapmetal_yield() {
        let t = Table::fixture();
        let level = 4;
        let skills = [
            (TYPE_SCRAPMETAL_PROCESSING, level),
            (TYPE_REPROCESSING, 5),
            (TYPE_REPROCESSING_EFFICIENCY, 5),
        ];
        let efficiency = (NPC_STATION_EFFICIENCY
            * (1.0 + (level as f64 * t.yield_mutator(TYPE_SCRAPMETAL_PROCESSING)) / 100.0))
            .min(1.0);
        let units = 3;
        let q = quote(
            &t,
            ore(7, MODULE_RARE, units),
            &station(),
            &pilot_at(&t, &skills, &[IMPLANT]),
            false,
        )
        .expect("a module quotes");
        assert_eq!(q.portions, units);
        assert_eq!(q.efficiency, efficiency);
        let mut tax = 0.0;
        for (r, (m, per)) in q.recoverables.iter().zip(t.materials(MODULE_RARE)) {
            let quantity = per * units;
            let client = (quantity as f64 * efficiency).floor() as i64;
            assert_eq!((r.type_id, r.client), (*m, client));
            assert_eq!(r.unrecoverable, quantity - client);
            let isk = (t.price(*m) * (client as f64 * NPC_STATION_TAX) * 100.0).round() / 100.0;
            assert_eq!(r.isk_cost, isk);
            tax += isk;
        }
        assert_eq!(q.recoverables.len(), 2);
        assert_eq!(q.total_isk_cost, (tax * 100.0).round() / 100.0);

        let p = plan(
            &t,
            Some(PILOT),
            Some(STATION_PLAIN),
            &[ore(7, MODULE_RARE, units)],
            &station(),
            &pilot_at(&t, &skills, &[IMPLANT]),
            None,
            None,
            RICH,
        )
        .expect("a module plans");
        assert_eq!(p.processed, vec![7]);
        assert_eq!(p.consume, vec![Edit::Destroy { item_id: 7 }]);
        let want: Vec<(i64, i64)> = q
            .recoverables
            .iter()
            .filter(|r| r.client > 0)
            .map(|r| (r.type_id, r.client))
            .collect();
        assert_eq!(p.produce, want);
    }

    /// The quote reads the table it is handed and nothing else.
    #[test]
    fn the_quote_reads_only_the_table_it_is_given() {
        let empty = Table::build(&Files::default(), true).expect("builds");
        let input = ore(1, ORE, 1000);
        let none = |_: i64| 1.0;
        assert!(quote(&Table::fixture(), input, &station(), &none, false).is_some());
        assert!(quote(&empty, input, &station(), &none, false).is_none());
        assert_eq!(
            plan_of(&empty, &[input], RICH).map(|p| p.processed),
            Err(Refusal::NothingToDo),
            "a loaded table without the type: nothing to refine"
        );
        assert_eq!(empty.portion_size(ORE), 1);
        assert_eq!(empty.yield_mutator(TYPE_REPROCESSING), 0.0);
    }

    /// Plan §2.2: with no bundle, a type outside the fixture is refused with
    /// a message naming `--content`, not `MrNothingToRefine` and not a panic.
    #[test]
    fn a_type_outside_the_fixture_is_refused_naming_content() {
        let t = Table::fixture();
        assert_eq!(
            plan_of(&t, &[ore(1, ORE, 1000), ore(2, 1, 5)], RICH).map(|p| p.processed),
            Err(Refusal::NoContent(1))
        );
        assert!(no_content_message(1).contains("--content"));
        // ISK is still declined quietly, as the client does.
        assert_eq!(
            plan_of(&t, &[ore(1, TYPE_CREDITS, 5)], RICH),
            Err(Refusal::NothingToDo)
        );
        // A loaded table has real rows: an unknown type is just not refinable.
        assert_eq!(
            plan_of(&fixture_with_output(MINERAL), &[ore(2, 1, 5)], RICH),
            Err(Refusal::NothingToDo)
        );
    }

    // -- per-station reprocessing terms -------------------------------------

    /// A station's terms are its own row; a station with no row falls back to
    /// the constants and says so with `None`.
    #[test]
    fn station_terms_come_from_the_table_and_an_unknown_station_falls_back() {
        let t = Table::fixture();
        for id in [STATION_PLAIN, STATION_POOR, STATION_CHEAP] {
            let s = t.station_terms(id).expect("a row");
            assert_eq!(t.station_efficiency(id), s.efficiency);
            assert_eq!(t.station_base_tax(id), s.tax);
        }
        assert!(t.station_terms(1).is_none());
        assert_eq!(t.station_efficiency(1), NPC_STATION_EFFICIENCY);
        assert_eq!(t.station_base_tax(1), NPC_STATION_TAX);
        assert_eq!(
            Facility::npc_station_at(&t, 1, 0.0),
            Facility::npc_station(0.0)
        );
    }

    /// `npc_station_at` applies the standing curve to the **station's own**
    /// base take, not to the fallback.
    #[test]
    fn the_standing_curve_scales_the_stations_own_base_take() {
        let t = Table::fixture();
        let half = STANDING_TAX_ZERO_POINT / 2.0;
        for id in [STATION_PLAIN, STATION_CHEAP] {
            let f = Facility::npc_station_at(&t, id, half);
            let want = t.station_base_tax(id) / 2.0;
            assert!(
                (f.tax - want).abs() < 1e-12,
                "station {id}: {} vs {want}",
                f.tax
            );
            assert_eq!(
                Facility::npc_station_at(&t, id, STANDING_TAX_ZERO_POINT).tax,
                0.0
            );
        }
        assert!(
            Facility::npc_station_at(&t, STATION_CHEAP, half).tax
                < Facility::npc_station_at(&t, STATION_PLAIN, half).tax
        );
    }

    /// Per-station efficiency reaches the quote's output: the same stack at
    /// the poorer station pays strictly less of every material.
    #[test]
    fn a_poor_station_pays_out_less_for_the_same_stack() {
        let t = Table::fixture();
        assert!(t.station_efficiency(STATION_POOR) < t.station_efficiency(STATION_PLAIN));
        let input = ore(1, ORE_MIXED, 1000);
        let none = |_: i64| 1.0;
        let at = |id| {
            quote(
                &t,
                input,
                &Facility::npc_station_at(&t, id, 0.0),
                &none,
                false,
            )
            .expect("quotes")
        };
        let (good, poor) = (at(STATION_PLAIN), at(STATION_POOR));
        assert_eq!(good.recoverables.len(), poor.recoverables.len());
        assert!(!poor.recoverables.is_empty());
        for (g, p) in good.recoverables.iter().zip(&poor.recoverables) {
            assert_eq!(g.type_id, p.type_id);
            assert!(p.client < g.client, "material {}", g.type_id);
        }
    }

    // -- the static layer ---------------------------------------------------

    /// What a compressed type refines into, for every pair: the source's own
    /// refine (same materials, same portion), a 1:1 decompression back into
    /// a source with no fixed yield (gas), or both sides randomized.
    #[test]
    fn compression_preserves_the_refine() {
        let t = Table::fixture();
        let mut seen = 0;
        for (source, compressed) in t.compressed_pairs() {
            if !t.materials(source).is_empty() {
                assert_eq!(t.materials(source), t.materials(compressed));
                assert_eq!(t.portion_size(source), t.portion_size(compressed));
            } else if t.materials(compressed) != [(source, 1)] {
                assert!(
                    t.has_randomized_materials(source) && t.has_randomized_materials(compressed),
                    "pair ({source}, {compressed}) is neither a refine, a \
                     decompression nor a randomized pair"
                );
            }
            seen += 1;
        }
        assert!(seen > 0, "the fixture has pairs to check");
        assert_eq!(t.compressed_type(GAS), Some(GAS_COMPRESSED));
        assert_eq!(t.materials(GAS_COMPRESSED), &[(GAS, 1)]);
    }

    /// The compressed map answers in both directions.
    #[test]
    fn the_compressed_map_answers_both_ways() {
        let t = Table::fixture();
        assert_eq!(t.compression_sources(ORE_MIXED_COMPRESSED), vec![ORE_MIXED]);
        assert!(t.is_compressed_type(ORE_MIXED_COMPRESSED));
        assert!(!t.is_compressed_type(ORE_MIXED));
        assert!(t.is_compressible(ORE_MIXED));
        assert!(!t.is_compressible(ORE_MIXED_COMPRESSED));
    }

    /// Attribute 790 and 379 close into each other: every skill a type
    /// points at has a mutator, or the yield would silently be `1 + L * 0`.
    #[test]
    fn every_ore_skill_has_a_yield_mutator() {
        let t = Table::fixture();
        assert!(t.reprocessing_skills().count() > 0);
        for (type_id, skill) in t.reprocessing_skills() {
            assert!(
                t.yield_mutator(skill) > 0.0,
                "type {type_id} -> skill {skill}"
            );
        }
        assert!(
            t.has_special_skill_bonuses(ROLL),
            "randomized types carry 790 too"
        );
    }

    /// Every output a kept row can produce has a price row, so a missing
    /// price is a table gap rather than a silent zero.
    #[test]
    fn every_output_material_has_a_price_row() {
        let t = Table::fixture();
        for type_id in t.reprocessable_types().chain(t.randomized_types()) {
            for &(m, _) in t.materials(type_id) {
                assert!(t.is_priced(m), "type {type_id} yields unpriced {m}");
            }
            for m in t.randomized(type_id) {
                assert!(t.is_priced(m.type_id));
            }
        }
    }

    /// C5 §5.2: `P_ref` wins over `basePrice`, and `basePrice` answers for a
    /// material with no `P_ref`.
    #[test]
    fn p_ref_wins_and_base_price_is_the_fallback() {
        let f = Files::fixture();
        let t = Table::fixture();
        let p_ref = crate::econ_prices::Table::parse(&f.prices).expect("parses");
        assert_eq!(t.price(MINERAL), p_ref.prices[&MINERAL].ref_price);
        assert!(!p_ref.prices.contains_key(&MINERAL_DEAR));
        let without = Table::build(
            &Files {
                prices: vec![],
                ..f
            },
            true,
        )
        .expect("builds");
        assert_ne!(
            without.price(MINERAL),
            t.price(MINERAL),
            "basePrice differs"
        );
        assert_eq!(without.price(MINERAL_DEAR), t.price(MINERAL_DEAR));
        assert_eq!(without.price(GAS), 0.0, "a gas's basePrice is 0");
    }

    // -- the yield maths ----------------------------------------------------

    /// The client's product, in its term order: the ore skill, then 3385,
    /// then 3389, then each implant.
    #[test]
    fn the_character_multiplier_is_the_client_s_product() {
        let t = Table::fixture();
        let term = |skill: i64| 1.0 + (5.0 * t.yield_mutator(skill)) / 100.0;
        let all = levels(&[
            (ORE_SKILL, 5),
            (TYPE_REPROCESSING, 5),
            (TYPE_REPROCESSING_EFFICIENCY, 5),
        ]);
        let product =
            term(ORE_SKILL) * term(TYPE_REPROCESSING) * term(TYPE_REPROCESSING_EFFICIENCY);
        assert_eq!(character_multiplier(&t, ORE, &all, &[]), product);
        assert_eq!(character_multiplier(&t, ORE, levels(&[]), &[]), 1.0);
        assert_eq!(
            character_multiplier(&t, ORE, levels(&[(ORE_SKILL, 5)]), &[]),
            term(ORE_SKILL)
        );
        assert_eq!(
            character_multiplier(&t, ORE, &all, &[IMPLANT]),
            product * (1.0 + t.yield_mutator(IMPLANT) / 100.0)
        );
    }

    /// A type with no attribute 790 takes Scrapmetal Processing and nothing
    /// else -- in particular **not** the implant (the EvEJS divergence in
    /// this module's header).
    #[test]
    fn scrapmetal_takes_one_skill_and_no_implant() {
        let t = Table::fixture();
        assert!(!t.has_special_skill_bonuses(MODULE));
        let trained = levels(&[
            (TYPE_SCRAPMETAL_PROCESSING, 5),
            (TYPE_REPROCESSING, 5),
            (TYPE_REPROCESSING_EFFICIENCY, 5),
            (ORE_SKILL, 5),
        ]);
        let want = 1.0 + (5.0 * t.yield_mutator(TYPE_SCRAPMETAL_PROCESSING)) / 100.0;
        assert_eq!(character_multiplier(&t, MODULE, &trained, &[]), want);
        assert_eq!(character_multiplier(&t, MODULE, &trained, &[IMPLANT]), want);
    }

    /// `min(1.0, ...)` caps at the top and not at the bottom.
    #[test]
    fn the_effective_yield_is_capped_at_one() {
        assert_eq!(effective_yield(0.5, 1.5), 0.75);
        assert_eq!(effective_yield(0.9, 1.5), 1.0);
        assert_eq!(effective_yield(1.0, 1.0), 1.0);
        assert_eq!(effective_yield(0.0, 1.5), 0.0);
    }

    /// The standing curve, at its interesting points.
    #[test]
    fn the_station_tax_falls_to_zero_at_the_zero_point() {
        let base = NPC_STATION_TAX;
        assert_eq!(station_tax(base, 0.0), base);
        assert_eq!(station_tax(base, STANDING_TAX_ZERO_POINT), 0.0);
        assert_eq!(station_tax(base, STANDING_TAX_ZERO_POINT + 1.0), 0.0);
        let half = station_tax(base, STANDING_TAX_ZERO_POINT / 2.0);
        assert!((half - base / 2.0).abs() < 1e-12, "{half}");
        assert_eq!(station_tax(base, -5.0), base, "negative does not raise it");
    }

    /// A bare floor, not EvEJS's normalised one: `400 * 0.6325` is just
    /// under 253 as a double, and the client's `int(floor(...))` pays 252.
    #[test]
    fn the_client_s_bare_floor_is_reproduced() {
        let efficiency = 0.5 * 1.15 * 1.10;
        assert!(400.0 * efficiency < 253.0, "the product is just under 253");
        assert_eq!(floor_yield(400, efficiency), 252);
        assert_eq!(floor_yield(0, 1.0), 0);
        assert_eq!(floor_yield(100, 0.0), 0);
    }

    // -- the quote ----------------------------------------------------------

    /// The plan row's [V] check: the fixture's rows through the client's
    /// formula, written out here from `reprocessing/ui/util` and
    /// `_GetRecoverablesFromFixedMaterials`:
    /// `efficiency = min(1, station * prod(1 + L*m/100))`,
    /// `client = int(floor(quantity * efficiency))`,
    /// `iskCost = round(price * (client * take), 2)`.
    #[test]
    fn fixture_materials_give_the_client_s_yield_formula() {
        let t = Table::fixture();
        let skills = [
            (ORE_SKILL, 5),
            (TYPE_REPROCESSING, 5),
            (TYPE_REPROCESSING_EFFICIENCY, 5),
        ];
        let mut multiplier = 1.0;
        for (skill, level) in skills {
            multiplier *= 1.0 + (level as f64 * t.yield_mutator(skill)) / 100.0;
        }
        let efficiency = (NPC_STATION_EFFICIENCY * multiplier).min(1.0);
        let portion = t.portion_size(ORE);
        let per_portion = t.materials(ORE)[0].1;
        let stack = 10 * portion;

        let q = quote(
            &t,
            ore(100, ORE, stack),
            &station(),
            &pilot_at(&t, &skills, &[]),
            false,
        )
        .expect("quotes");
        assert_eq!(q.portions, 10);
        assert_eq!(q.left_overs, 0);
        assert_eq!(q.quantity_to_process, stack);
        assert_eq!(q.efficiency, efficiency);
        let quantity = 10 * per_portion;
        let client = (quantity as f64 * efficiency).floor() as i64;
        let r = q.recoverables[0];
        assert_eq!((r.type_id, r.client), (MINERAL, client));
        assert_eq!(r.unrecoverable, quantity - client);
        let isk = (t.price(MINERAL) * (client as f64 * NPC_STATION_TAX) * 100.0).round() / 100.0;
        assert_eq!(r.isk_cost, isk);
        assert_eq!(q.total_isk_cost, isk);
    }

    /// The remainder is left behind and is not refined.
    #[test]
    fn a_partial_portion_is_left_over() {
        let t = Table::fixture();
        let portion = t.portion_size(ORE);
        let q = quote_with(ore(100, ORE, 10 * portion + 49), &[], &[]).expect("quotes");
        assert_eq!(q.portions, 10);
        assert_eq!(q.left_overs, 49);
        assert_eq!(q.quantity_to_process, 10 * portion);
        let raw = 10 * t.materials(ORE)[0].1;
        assert_eq!(
            q.recoverables[0].client,
            floor_yield(raw, NPC_STATION_EFFICIENCY)
        );
    }

    /// Fewer units than one portion is no quote at all.
    #[test]
    fn less_than_one_portion_does_not_quote() {
        let portion = Table::fixture().portion_size(ORE);
        assert!(quote_with(ore(100, ORE, portion - 1), &[], &[]).is_none());
        assert!(quote_with(ore(100, ORE, 0), &[], &[]).is_none());
    }

    /// ISK, a type with no row (a blueprint's shape) and an unknown type all
    /// decline quietly.
    #[test]
    fn unrefinable_inputs_decline_quietly() {
        assert!(quote_with(ore(100, TYPE_CREDITS, 1_000_000), &[], &[]).is_none());
        assert!(quote_with(ore(100, INERT, 1), &[], &[]).is_none());
        assert!(quote_with(ore(100, 999_999, 1000), &[], &[]).is_none());
    }

    /// A randomized type quotes with no output lines by default, and with
    /// average ones when asked (`includeRecoverablesFromRandomizedOutputs`).
    #[test]
    fn a_randomized_quote_hides_its_outputs_unless_asked() {
        let t = Table::fixture();
        let input = ore(100, ROLL, 1);
        let preview = quote(&t, input, &station(), &pilot_at(&t, &[], &[]), false).expect("quotes");
        assert!(preview.recoverables.is_empty());
        assert!(preview.total_isk_cost > 0.0, "the batch is still taxed");
        let full = quote(&t, input, &station(), &pilot_at(&t, &[], &[]), true).expect("quotes");
        let m = t.randomized(ROLL)[0];
        assert_eq!(full.recoverables.len(), 1);
        assert_eq!(full.recoverables[0].type_id, m.type_id);
        assert_eq!(
            full.recoverables[0].client,
            floor_yield((m.min + m.max) / 2, NPC_STATION_EFFICIENCY)
        );
        assert_eq!(full.recoverables[0].isk_cost, 0.0);
    }

    // -- the plan -----------------------------------------------------------

    /// `Destroy` for a whole stack, `Consume` for one with a remainder, and
    /// the produce half merged across inputs.
    #[test]
    fn a_plan_destroys_whole_stacks_and_consumes_partial_ones() {
        let t = Table::fixture();
        let portion = t.portion_size(ORE);
        let inputs = [
            ore(101, ORE, 10 * portion),
            ore(102, ORE, 10 * portion + 49),
            ore(103, ORE_MIXED, 5 * t.portion_size(ORE_MIXED)),
        ];
        let p = plan_of(&t, &inputs, RICH).expect("plans");
        assert_eq!(
            p.consume,
            vec![
                Edit::Destroy { item_id: 101 },
                Edit::Consume {
                    item_id: 102,
                    quantity: 10 * portion
                },
                Edit::Destroy { item_id: 103 },
            ]
        );
        assert_eq!(p.processed, vec![101, 102, 103]);
        let y = |q| floor_yield(q, NPC_STATION_EFFICIENCY);
        let ore_raw = 10 * t.materials(ORE)[0].1;
        let mixed = t.materials(ORE_MIXED);
        assert_eq!(
            p.produce,
            vec![
                (MINERAL, 2 * y(ore_raw) + y(5 * mixed[0].1)),
                (MINERAL_DEAR, y(5 * mixed[1].1)),
            ]
        );
        assert_eq!(p.output, inputs[0].slot);
        let edits = p.edits();
        assert_eq!(edits.len(), p.consume.len() + p.produce.len());
        for e in &edits {
            if let Edit::Create { to, .. } = e {
                assert_eq!(*to, p.output);
            }
        }
    }

    /// End to end: ore in, outputs out, atomically, against a real store.
    #[test]
    fn a_whole_reprocess_applies_as_one_plan() {
        let cx = crate::content::Content::fixture();
        let output = known_stackable();
        let t = fixture_with_output(output.type_id);
        let inputs = vec![ore(101, ORE, 10 * t.portion_size(ORE))];
        let p = plan_of(&t, &inputs, RICH).expect("plans");
        let mut store = crate::item::Store::new();
        for i in &inputs {
            store.insert(crate::item::Item {
                item_id: i.item_id,
                type_id: i.type_id,
                owner_id: i.slot.owner_id,
                location_id: i.slot.location_id,
                flag_id: i.slot.flag_id,
                quantity: i.quantity,
                group_id: 99_001,
                category_id: crate::typedata::CATEGORY_ASTEROID,
                singleton: false,
            });
        }
        let changes = store.apply_in(&cx, &p.edits()).expect("the plan applies");
        assert!(!changes.is_empty());
        assert!(store.get(101).is_none(), "the ore was consumed");
        for &(type_id, quantity) in &p.produce {
            let have: i64 = store
                .items()
                .iter()
                .filter(|i| i.type_id == type_id)
                .map(|i| i.quantity)
                .sum();
            assert_eq!(have, quantity, "type {type_id} yield must match the quote");
        }
    }

    /// A stackable type `item::find_in` knows, chosen by rule.
    fn known_stackable() -> &'static crate::item::ItemType {
        crate::item::SPAWNABLE
            .iter()
            .find(|t| !t.singleton)
            .expect("SPAWNABLE has a stackable type")
    }

    /// The consume half is accepted by `apply`'s phase one as written.
    #[test]
    fn the_consume_half_applies_against_a_real_store() {
        let cx = crate::content::Content::fixture();
        let t = Table::fixture();
        let stack = 10 * t.portion_size(ORE);
        let mut store = item::Store::new();
        let (ids, _) = store
            .spawn_in(&cx, known_stackable().name, stack, PILOT, STATION_PLAIN)
            .expect("a stack to stand in for ore");
        let it = store.get(ids[0]).expect("spawned").clone();
        let inputs = [Input {
            item_id: it.item_id,
            type_id: ORE,
            quantity: stack,
            slot: it.slot(),
        }];
        let p = plan_of(&t, &inputs, RICH).expect("plans");
        let changes = store
            .apply_in(&cx, &p.consume)
            .expect("phase one accepts the plan");
        assert_eq!(changes.len(), 1);
        assert!(matches!(changes[0], item::Change::Destroyed { .. }));
        assert!(store.get(it.item_id).is_none());
    }

    /// A partial stack leaves its remainder in place after `apply`.
    #[test]
    fn a_remainder_survives_the_reprocess() {
        let cx = crate::content::Content::fixture();
        let t = Table::fixture();
        let portion = t.portion_size(ORE);
        let mut store = item::Store::new();
        let (ids, _) = store
            .spawn_in(
                &cx,
                known_stackable().name,
                portion + 49,
                PILOT,
                STATION_PLAIN,
            )
            .expect("stack");
        let it = store.get(ids[0]).expect("spawned").clone();
        let inputs = [Input {
            item_id: it.item_id,
            type_id: ORE,
            quantity: portion + 49,
            slot: it.slot(),
        }];
        let p = plan_of(&t, &inputs, RICH).expect("plans");
        store.apply_in(&cx, &p.consume).expect("applies");
        assert_eq!(store.get(it.item_id).map(|i| i.quantity), Some(49));
    }

    /// Every refusal but `AccountFunds` (its own test, below) and
    /// `NoContent` (`a_type_outside_the_fixture_is_refused_naming_content`).
    #[test]
    fn every_refusal_is_reachable() {
        let t = Table::fixture();
        let inputs = [ore(101, ORE, 1000)];
        let f = station();
        let m = pilot_at(&t, &[], &[]);
        let run = |actor, docked, inputs: &[Input], active| {
            plan(&t, actor, docked, inputs, &f, &m, active, None, RICH)
        };
        assert_eq!(
            run(None, Some(STATION_PLAIN), &inputs, None),
            Err(Refusal::NoActor)
        );
        assert_eq!(
            run(Some(PILOT), None, &inputs, None),
            Err(Refusal::NotDocked)
        );
        assert_eq!(
            run(Some(PILOT), Some(STATION_PLAIN), &inputs, Some(101)),
            Err(Refusal::ActiveShip(101)),
            "one active ship in the selection refuses the whole batch"
        );
        assert_eq!(
            run(Some(PILOT), Some(STATION_PLAIN), &[], None),
            Err(Refusal::NothingToDo)
        );
    }

    /// The tax is on what the player receives, the plan's total is the
    /// quotes' total, and a wallet one ISK short refuses the whole plan.
    #[test]
    fn the_tax_is_on_what_is_received_and_short_wallets_refuse() {
        let t = Table::fixture();
        let inputs = [ore(101, ORE, 10 * t.portion_size(ORE))];
        let received = floor_yield(10 * t.materials(ORE)[0].1, NPC_STATION_EFFICIENCY);
        let tax = round2(t.price(MINERAL) * (received as f64 * NPC_STATION_TAX));
        assert!(
            tax > 1.0,
            "the fixture's tax is big enough to fall short of"
        );
        let p = plan_of(&t, &inputs, RICH).expect("plans");
        assert_eq!(p.total_isk_cost, tax);
        assert_eq!(p.quotes.iter().map(|q| q.total_isk_cost).sum::<f64>(), tax);
        assert_eq!(
            plan_of(&t, &inputs, tax - 1.0),
            Err(Refusal::AccountFunds(tax, tax - 1.0))
        );
        assert!(plan_of(&t, &inputs, tax).is_ok(), "`>`, not `>=`");
    }

    /// The plan row: the gas refine is charged. A gas's `basePrice` is 0, so
    /// before `P_ref` a decompression cost nothing; with `P_ref` it costs
    /// the take on what is received.
    #[test]
    fn the_gas_refine_is_charged() {
        let t = Table::fixture();
        let p = plan_of(&t, &[ore(101, GAS_COMPRESSED, 10)], RICH).expect("plans");
        let received = floor_yield(10, NPC_STATION_EFFICIENCY);
        assert_eq!(p.produce, vec![(GAS, received)]);
        assert!(p.total_isk_cost > 0.0);
        assert_eq!(
            p.total_isk_cost,
            round2(t.price(GAS) * (received as f64 * NPC_STATION_TAX))
        );
        assert_eq!(
            plan_of(&t, &[ore(101, GAS_COMPRESSED, 10)], 0.0).map(|p| p.processed),
            Err(Refusal::AccountFunds(p.total_isk_cost, 0.0)),
            "an empty wallet no longer refines gas for free"
        );
    }

    /// An output with no price costs nothing, and an empty wallet covers it.
    #[test]
    fn an_unpriced_output_costs_nothing_and_an_empty_wallet_can_refine_it() {
        let t = Table::build(
            &Files {
                prices: vec![],
                ..Files::fixture()
            },
            true,
        )
        .expect("builds");
        let p = plan_of(&t, &[ore(101, GAS_COMPRESSED, 10)], 0.0)
            .expect("a free refine must not be refused for funds");
        assert_eq!(p.total_isk_cost, 0.0);
    }

    /// The skilled yield, and the tax that follows it.
    #[test]
    fn skills_raise_both_the_yield_and_the_tax() {
        let t = Table::fixture();
        let trained = [
            (ORE_SKILL, 5),
            (TYPE_REPROCESSING, 5),
            (TYPE_REPROCESSING_EFFICIENCY, 5),
        ];
        let stack = 10 * t.portion_size(ORE);
        let p = plan(
            &t,
            Some(PILOT),
            Some(STATION_PLAIN),
            &[ore(101, ORE, stack)],
            &station(),
            &pilot_at(&t, &trained, &[]),
            None,
            None,
            RICH,
        )
        .expect("plans");
        let raw = 10 * t.materials(ORE)[0].1;
        let efficiency = effective_yield(
            NPC_STATION_EFFICIENCY,
            character_multiplier(&t, ORE, levels(&trained), &[]),
        );
        let received = floor_yield(raw, efficiency);
        assert_eq!(p.produce, vec![(MINERAL, received)]);
        assert!(received > floor_yield(raw, NPC_STATION_EFFICIENCY));
        assert_eq!(
            p.total_isk_cost,
            round2(t.price(MINERAL) * (received as f64 * NPC_STATION_TAX))
        );
    }

    /// An explicit output slot is honoured over the sample item's.
    #[test]
    fn an_explicit_output_slot_wins() {
        let t = Table::fixture();
        let to = Slot::new(1_000_001, STATION_PLAIN, 115);
        let p = plan(
            &t,
            Some(PILOT),
            Some(STATION_PLAIN),
            &[ore(101, ORE, 1000)],
            &station(),
            &pilot_at(&t, &[], &[]),
            None,
            Some(to),
            RICH,
        )
        .expect("plans");
        assert_eq!(p.output, to);
    }

    /// A randomized input produces its expected value deterministically --
    /// the documented departure from the client, which rolls dice.
    #[test]
    fn a_randomized_input_produces_its_expected_value() {
        let t = Table::fixture();
        let p = plan_of(&t, &[ore(101, ROLL, 1)], RICH).expect("plans");
        let m = t.randomized(ROLL)[0];
        assert_eq!(
            p.produce,
            vec![(
                m.type_id,
                floor_yield((m.min + m.max) / 2, NPC_STATION_EFFICIENCY)
            )]
        );
    }

    // -- helpers ------------------------------------------------------------

    /// The multiplier closure a test drives: explicit skill levels and the
    /// module's own [`character_multiplier`].
    fn pilot_at<'a>(
        table: &'a Table,
        skills: &'a [(i64, i64)],
        implants: &'a [i64],
    ) -> impl Fn(i64) -> f64 + 'a {
        move |type_id| character_multiplier(table, type_id, levels(skills), implants)
    }

    /// [`quote`] at the fallback station, through [`Table::fixture`].
    fn quote_with(input: Input, skills: &[(i64, i64)], implants: &[i64]) -> Option<Quote> {
        let t = Table::fixture();
        quote(
            &t,
            input,
            &station(),
            &pilot_at(&t, skills, implants),
            false,
        )
    }
}
