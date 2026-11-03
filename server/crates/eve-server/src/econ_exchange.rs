//! The NPC exchange: NPC market orders computed on read, never stored.
//!
//! W15 packet CP35 (`docs/content/plan.md` §6.2), spec C5 §4 and §6 (packet
//! E3), lane note `docs/lanes/cp35.md`; narrowed to the closed economy by
//! wave E's E5 (R-ECON §2.3, owner decisions O5-a and O5-d in
//! `docs/port-tracker.md` §82), lane note `docs/lanes/e5.md`; made TQ-true
//! by E5b (R-NPCTRADE §4 R1-R4, R6; owner §83), lane note
//! `docs/lanes/e5b.md`. `service/market.rs` merges these orders into every
//! reader and fill; nothing else calls here.
//!
//! - **Virtual orders.** An order is a function of the bundle
//!   (`stations/stations.tsv`, `npc/trades.tsv`, `types/market.tsv`,
//!   `agents/schools.tsv`, `industry/blueprints.tsv`), the price oracle
//!   ([`crate::econ_prices`]), `content/econ/hubs.tsv` and two deltas: what
//!   was taken since it was full ([`KIND`]) and a Tier B multiplier off its
//!   start ([`MULT_KIND`]).
//! - **Tier A** (R-NPCTRADE R1, verified on 547,778 TQ NPC orders with no
//!   disagreement): the sign of the station owner's `corporationTrades`
//!   entry is the side. A positive entry is an ask, a negative one a bid, at
//!   **every station the corporation owns**, in [`ASK_CATEGORIES`] and
//!   [`BID_CATEGORIES`] only (blueprints while [`NPC_BPOS`], O5-a;
//!   category 66 while [`NPC_STRUCTURE_ITEMS`]). Ships, modules, charges,
//!   drones, hardwirings and category 40 are never posted, whatever the
//!   table says (TQ posts none). Skillbooks come from every listing
//!   corporation, schools or not (R2); while [`SCHOOL_BOOKS`] a school's
//!   starting stations also sell its corporation's books, and while
//!   [`SCHOOL_INVENTION_BOOKS`] every school station sells the invention
//!   skills nobody lists (the encryption methods, GAPS-B). Trade goods are
//!   both asked and bid (O5-i: a hauling profession, capped by
//!   [`TIER_A_STOCK`] and the trickle). Prices are `basePrice` ×
//!   [`ask_factor`] or × [`TIER_A_BID`] (R3). No NPC bid on any recipe
//!   product (R4, C5 §6 invariant 2), and no NPC ask on one but a blueprint
//!   (O5: never seed what players build).
//! - **Tier B** (C5 §4.3 under O5-d), only while [`LOOT_BIDS`] **[A]**: at
//!   each hub, a floor bid on loot: loot-only modules and charges
//!   ([`LOOT_CATEGORIES`] in [`LOOT_META_GROUPS`]) and tags ([`TAG_GROUPS`]).
//!   TQ has no such bid (R-NPCTRADE §3); it is a stopgap until the loot
//!   tables drop TQ's sink goods. No ceiling ask. Never a bid on anything
//!   with a recipe (invariant 2), nor on a starter type
//!   ([`crate::starter_ship::Table::types`]: minted free, E7).
//! - **The slot index** (E5, R-ECON §1.4 P5): which `(station, side, tier)`
//!   may hold an order of a `(region, type)` is fixed by the content, so
//!   [`Table::new`] computes it once; a reader walks only its
//!   `(region, type)` range. [`stamp`] is what a cached book of one
//!   `(region, type)` depends on (P6).
//! - **Ids** ([`order_id`]) are C5 §4.1's, with no day in them (plan §5).
//! - **Refill** ([`trickle`]) is lazy, so an emptied order is full within a
//!   day with no timer; [`on_rollover`] prunes, moves the multipliers and
//!   folds the day's trades into the index ([`econ_prices::fold_day`]).
//!
//! **With no bundle there are no NPC orders** (the station table is empty),
//! so invented rows never reach a client; tests see [`Table::fixture`].

use crate::content::{Boxed, Bundle, Origin, Sources, TableSpec};
use crate::econ_prices::{self, Source};
use crate::seed::{DAY, HOUR};
use crate::service::Registry;
use std::collections::{BTreeMap, BTreeSet};

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "econ_exchange",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

/// `station_id type_id side -> remaining touched_at` (plan §4.2).
pub const KIND: &str = "npcorder";
/// `station_id type_id side -> m` (plan §4.2), only when `m` is not the start.
pub const MULT_KIND: &str = "npcmult";

/// Bundle files and their header lines, copied from `schema.md` §2.
pub const STATIONS: (&str, &str) = (
    "stations/stations.tsv",
    "#station_id\tsystem_id\tregion_id\towner_id\toperation_id\tservices\treproc_eff\treproc_take",
);
pub const TRADES: (&str, &str) = ("npc/trades.tsv", "#corp_id\ttype_id\tvalue");
pub const MARKET: (&str, &str) = (
    "types/market.tsv",
    "#type_id\tgroup_id\tcategory_id\tmarket_group_id\tbase_price\tmeta_group_id\tmeta_level\ttech_level\tvariation_parent_type_id\tfaction_id\trace_id\twreck_type_id\tportion_size\tpublished",
);
/// The authored hub list under `--authored`, and its header (`schema.md` §4).
pub const HUBS_FILE: &str = "econ/hubs.tsv";
pub const HUBS_HEADER: &str = "station_id\twhy";
const SHIPPED_HUBS: &str = include_str!("../../../../content/econ/hubs.tsv");

/// The NPC order band (plan §3.4), C5 §4.1 **[A]**: three orders of magnitude
/// above the player (5e9), transaction (7e9) and escrow (9e9) bands.
/// Falsified by a client path that truncates an order id above 2^32.
pub const NPC_ORDER_BASE: i64 = 10_000_000_000_000;
/// Ids per station, C5 §4.1's `2^20`: two per type slot (ask, bid).
pub const STATION_STRIDE: i64 = 1 << 20;
/// Stations and type slots the band holds; the bundle has far fewer.
pub const MAX_STATIONS: usize = 1 << 14;
pub const MAX_TYPES: usize = 1 << 19;
pub const NPC_ORDER_END: i64 = NPC_ORDER_BASE + MAX_STATIONS as i64 * STATION_STRIDE;

/// `categoryBlueprint`, `categorySkill` [V]: Tier A sells them at
/// `basePrice`, the retail NPC price (R-NPCTRADE §2: exactly 1.000 on TQ).
pub const CATEGORY_BLUEPRINT: i64 = 9;
pub const CATEGORY_SKILL: i64 = 16;
/// Commodity (trade goods, insignia, data items ...) and planetary command
/// centers [V].
pub const CATEGORY_COMMODITY: i64 = 17;
pub const CATEGORY_COMMAND_CENTER: i64 = 41;
/// Category 66, group 4086 (sov-hub items **[I]**), both sides on TQ.
pub const CATEGORY_STRUCTURE_ITEM: i64 = 66;
/// **O5-a** (owner decision, port-tracker §82): NPCs sell blueprint
/// originals at their corporations' stations, as a knowledge sink, and never
/// buy them back. `false` leaves blueprints to loot.
pub const NPC_BPOS: bool = true;
/// Whether category 66 is posted (R-NPCTRADE R1): TQ does, but sovereignty
/// is not in scope, so off.
pub const NPC_STRUCTURE_ITEMS: bool = false;
/// R1 [V-TQ]: the categories a positive `corporationTrades` entry is asked
/// in (blueprints only while [`NPC_BPOS`]). TQ asks nothing in 6, 7, 8 or 18
/// although the table lists some (the Civilian Armor Repairer aside, left
/// out).
pub const ASK_CATEGORIES: [i64; 4] = [
    CATEGORY_BLUEPRINT,
    CATEGORY_SKILL,
    CATEGORY_COMMODITY,
    CATEGORY_COMMAND_CENTER,
];
/// R1 [V-TQ]: the categories a negative entry is bid in: commodities,
/// asteroid (mutanites, Nephrite), material (Neo-Jadarite) and 2143. TQ bids
/// on nothing in 20 or 40 although the table lists some.
pub const BID_CATEGORIES: [i64; 4] = [CATEGORY_COMMODITY, 25, 4, 2143];
/// R2 alternative kept on: a school's `starting_stations` also sell its
/// corporation's books when another corporation owns them **[A]**, so a new
/// pilot finds them at home. Falsified by a school station whose books a
/// player calls wrong for TQ.
pub const SCHOOL_BOOKS: bool = true;
/// **GAPS-B, the orphan invention books [A]** (coordinator decision until a
/// real source is known): every skill an invention needs
/// (`industry/blueprints.tsv`, activity 8, kind `skill`) that **no**
/// corporation's trade list asks is asked at every school's starting
/// stations, the way [`SCHOOL_BOOKS`] sells the school's own books. In the
/// build-3619879 bundle that is the encryption methods and two engineering
/// skills, none of which `corporationTrades` lists, so without this no
/// racial invention is possible. Only published types with a market group
/// qualify (`types/market.tsv`). Falsified by a TQ school station whose
/// market lacks an encryption-method book, or by the real NPC source of
/// these books once found (then sell them there instead).
pub const SCHOOL_INVENTION_BOOKS: bool = true;
/// The ask of an orphan invention book whose `basePrice` is 0 (the
/// encryption methods' is, in the bundle) **[A]**: Tier A asks nothing at
/// 0, so it needs a price. Falsified by the TQ price of an
/// encryption-method book.
pub const INVENTION_BOOK_ASK: f64 = 1_000_000.0;
/// **The orphan booster formulas [A]** (issue #80, owner decision D1): each
/// reaction formula of the booster chain (`booster_sources::chain_formulas`)
/// that **no** corporation's trade list asks is asked at every school's
/// starting stations, as the invention books are. No TQ source for them is in
/// the client data, and the bots' reactor only buys a formula from an NPC
/// ask. Falsified by the real NPC source of these formulas once found (then
/// sell them there), or by a TQ school station whose market lacks them.
pub const ORPHAN_BOOSTER_FORMULAS: bool = true;
/// The ask of an orphan formula whose `basePrice` is 0 **[A]**. Falsified by
/// the TQ price of a booster reaction formula.
pub const ORPHAN_FORMULA_ASK: f64 = 20_000_000.0;
/// R6 **[A]**: the Tier B hub bids on loot-only modules, charges and tags.
/// TQ has none (0 orders); kept so ratters have a buyer until `npc/loot.tsv`
/// drops TQ's sink goods (insignia, Overseer's Personal Effects). Falsified
/// by a loot economy that runs on player buyers alone; then `false`.
pub const LOOT_BIDS: bool = true;
/// Module and charge, in T1, storyline, faction, officer or deadspace meta
/// groups [V], with no recipe: Tier B's loot-only floor bids (C5 §4.3).
pub const LOOT_CATEGORIES: [i64; 2] = [7, 8];
pub const LOOT_META_GROUPS: [i64; 5] = [1, 3, 4, 5, 6];
/// The tags' group: every tag R7 drops (`npc/factions.tsv` `tags`) lies in it
/// [V, the build-3619879 bundle]; that the group holds tags only is **[I]**.
/// Tier B bids on it (O5-d), recipe types excepted.
pub const TAG_GROUPS: [i64; 1] = [370];
/// Ship, module, charge, drone [V]: C5 §4.3's ceiling-ask categories. **No
/// NPC order reads it under O5**; the bots' builder classifies its goods by
/// it (`eve-bot` `builder.rs`).
pub const CEILING_CATEGORIES: [i64; 4] = [6, 7, 8, 18];
/// C5 §4.3's floor-bid groups [V]: minerals, ice products, moon materials,
/// salvage, PI. **No NPC order reads it under O5-d**; the bots' miner
/// classifies its goods by it (`eve-bot` `builder.rs`).
pub const FLOOR_GROUPS: [i64; 11] = [18, 423, 427, 754, 1032, 1033, 1034, 1035, 1040, 1041, 1042];

/// R3 [V-TQ]: a Tier A ask on `basePrice`, blueprints and skillbooks
/// (exactly 1.000 on TQ, everywhere).
pub const TIER_A_ASK: f64 = 1.00;
/// R3 [V-TQ]: command centers (0.90 on TQ, everywhere).
pub const COMMAND_CENTER_ASK: f64 = 0.90;
/// R3: trade-good asks, TQ's median (0.70 to 1.30 by type) **[I]**.
/// Falsified by a type whose TQ ask is far off it mattering to a hauler.
pub const TRADE_GOOD_ASK: f64 = 0.94;
/// R3 [V-TQ]: a Tier A bid on `basePrice` (TQ's median 1.00).
pub const TIER_A_BID: f64 = 1.00;
/// The floor under any Tier A ask, on the refine floor (C5 §6 invariant 1)
/// **[A]**: falsified by an NPC-sold type cheap enough to refine at a profit.
pub const ASK_REFINE_GUARD: f64 = 1.10;
/// Tier A stock of an order, either side, in effect unlimited **[A]**; with
/// the trickle it caps a trade-good haul loop at this many units a day per
/// station (O5-i). TQ volumes do not follow the table's magnitude (R7).
pub const TIER_A_STOCK: i64 = 1_000;
/// ISK of Tier B stock per order, and its unit bounds (C5 §4.3) **[A]**:
/// falsified by a stock one pilot empties daily, or never dents.
pub const TIER_B_VALUE: f64 = 20_000_000.0;
pub const TIER_B_UNITS: (i64, i64) = (2, 10_000);
/// Tier B's multipliers `(min, start, max)` (C5 §4.3, §4.5) **[A]**:
/// falsified by loot with no buyer above the floor, or a floor so high that
/// loot outearns bounties. Under O5 only the bid band prices an order; the
/// ask band stays for [`step`], whose arithmetic serves both.
pub const ASK_MULT: (f64, f64, f64) = (1.15, 1.25, 1.60);
pub const BID_MULT: (f64, f64, f64) = (0.40, 0.60, 0.70);
/// C5 §4.5 **[A]**: a day's fill fraction at or above `FILL_HIGH` moves a
/// multiplier against the crowd (ask ×`ASK_UP`, bid ×`BID_DOWN`), at or
/// below `FILL_LOW` it relaxes (ask ÷`ASK_DOWN`, bid ×`BID_UP`). Falsified
/// by a band that visibly oscillates.
pub const FILL_HIGH: f64 = 0.8;
pub const FILL_LOW: f64 = 0.1;
pub const ASK_UP: f64 = 1.05;
pub const ASK_DOWN: f64 = 1.02;
pub const BID_DOWN: f64 = 0.95;
pub const BID_UP: f64 = 1.02;
/// An NPC order's `duration`, days (C5 §4.1) **[A]**.
pub const DURATION_DAYS: i64 = 90;
/// The refill: a `1/TRICKLE_STEPS` of the volume per period (C5 §4.4,
/// hourly, full in 24 h) **[A]**. Falsified by a pilot who finds a
/// bought-out hub refilling too slowly or too fast.
pub const TRICKLE_PERIOD: i64 = HOUR;
pub const TRICKLE_STEPS: i64 = 24;
/// The best refine yield of anything an NPC sells (C5 §6: scrap at max
/// skills) **[I]**. The invariant tests read it: arbitrage is closed while
/// `REFINE_YIELD_MAX × the best bid < the lowest ask`.
pub const REFINE_YIELD_MAX: f64 = 0.55;

// ---- the table -----------------------------------------------------------------

/// One `stations/stations.tsv` row, the columns this module reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Station {
    pub station_id: i64,
    pub system_id: i64,
    pub region_id: i64,
    pub owner_id: i64,
}

/// One `types/market.tsv` row, the columns this module reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeRow {
    pub group_id: i64,
    pub category_id: i64,
    pub base_price: f64,
    pub meta_group_id: Option<i64>,
    pub tech_level: Option<i64>,
}

/// One school that sells skillbooks: the corporation whose trade list it
/// sells, at each of its stations (`agents/schools.tsv`, `crate::start`'s
/// file, `starting_stations`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct School {
    pub corp_id: i64,
    pub stations: Vec<i64>,
}

/// An NPC order's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    Ask,
    Bid,
}

impl Side {
    /// The delta key's spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Ask => "ask",
            Side::Bid => "bid",
        }
    }
}

/// Which rule made an order. On one `(station, type, side)` Tier A wins
/// **[A]**: it is the static NPC world (C5 §4.5), and the id has no tier bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    A,
    B,
}

/// One place an NPC order of a `(region, type)` may stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Slot {
    /// The station's index in [`Table`]'s sorted stations.
    station: usize,
    side: Side,
    tier: Tier,
}

/// The exchange's static half: stations, types and the slot index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    /// Sorted by `station_id`; the position is C5 §4.1's `station_index`.
    stations: Vec<Station>,
    types: BTreeMap<i64, TypeRow>,
    /// Sorted type ids; the position is the order id's type slot.
    type_ids: Vec<i64>,
    /// `(region, type) -> slots`, each `(station, side)` once (P5).
    slots: BTreeMap<(i64, i64), Vec<Slot>>,
    /// The orphan invention books the schools ask ([`SCHOOL_INVENTION_BOOKS`]),
    /// which [`INVENTION_BOOK_ASK`] prices when their `basePrice` is 0.
    invention_books: BTreeSet<i64>,
    /// The orphan booster formulas the schools ask
    /// ([`ORPHAN_BOOSTER_FORMULAS`]), which [`ORPHAN_FORMULA_ASK`] prices
    /// when their `basePrice` is 0.
    orphan_formulas: BTreeSet<i64>,
}

/// Whether Tier B may bid on a type at a hub, before its price row is read
/// (the recipe test needs the oracle, so [`price`] makes it).
fn loot_only(ty: &TypeRow) -> bool {
    if matches!(ty.category_id, CATEGORY_BLUEPRINT | CATEGORY_SKILL) {
        return false;
    }
    let module = LOOT_CATEGORIES.contains(&ty.category_id)
        && ty
            .meta_group_id
            .is_some_and(|m| LOOT_META_GROUPS.contains(&m));
    module || TAG_GROUPS.contains(&ty.group_id)
}

/// R3: a Tier A ask's multiplier on `basePrice` by category, `None` for a
/// category no NPC asks in.
pub fn ask_factor(category_id: i64) -> Option<f64> {
    match category_id {
        CATEGORY_BLUEPRINT if NPC_BPOS => Some(TIER_A_ASK),
        CATEGORY_SKILL => Some(TIER_A_ASK),
        CATEGORY_COMMODITY => Some(TRADE_GOOD_ASK),
        CATEGORY_COMMAND_CENTER => Some(COMMAND_CENTER_ASK),
        CATEGORY_STRUCTURE_ITEM if NPC_STRUCTURE_ITEMS => Some(TIER_A_ASK),
        _ => None,
    }
}

/// R1: whether a negative `corporationTrades` entry in `category_id` is bid.
pub fn bid_category(category_id: i64) -> bool {
    BID_CATEGORIES.contains(&category_id)
        || (NPC_STRUCTURE_ITEMS && category_id == CATEGORY_STRUCTURE_ITEM)
}

/// R1 and R4: the Tier A side of one `corporationTrades` entry, or `None`
/// when no NPC posts it. `product`: the type is a recipe product, which no
/// NPC bids on (invariant 2) and none but a blueprint is asked (O5).
fn tier_a_side(ty: &TypeRow, value: f64, product: bool) -> Option<Side> {
    if value > 0.0 && ask_factor(ty.category_id).is_some() {
        (!product || ty.category_id == CATEGORY_BLUEPRINT).then_some(Side::Ask)
    } else if value < 0.0 && bid_category(ty.category_id) {
        (!product).then_some(Side::Bid)
    } else {
        None
    }
}

/// **The orphan invention books** ([`SCHOOL_INVENTION_BOOKS`]): each of
/// `invention_skills` that is a skillbook in `types` and that no
/// corporation asks (`posts`, R1's Tier A entries).
fn orphan_books(
    posts: &BTreeMap<i64, Vec<(i64, Side)>>,
    types: &BTreeMap<i64, TypeRow>,
    invention_skills: &BTreeSet<i64>,
) -> BTreeSet<i64> {
    let asked: BTreeSet<i64> = posts
        .values()
        .flatten()
        .filter(|&&(_, side)| side == Side::Ask)
        .map(|&(ty, _)| ty)
        .collect();
    invention_skills
        .iter()
        .copied()
        .filter(|ty| types.get(ty).map(|t| t.category_id) == Some(CATEGORY_SKILL))
        .filter(|ty| !asked.contains(ty))
        .collect()
}

/// **The orphan booster formulas** ([`ORPHAN_BOOSTER_FORMULAS`]): each of
/// `formulas` that is a blueprint in `types` which an NPC may ask, and that no
/// corporation asks (`posts`, R1's Tier A entries).
fn orphan_formulas(
    posts: &BTreeMap<i64, Vec<(i64, Side)>>,
    types: &BTreeMap<i64, TypeRow>,
    formulas: &BTreeSet<i64>,
) -> BTreeSet<i64> {
    let asked: BTreeSet<i64> = posts
        .values()
        .flatten()
        .filter(|&&(_, side)| side == Side::Ask)
        .map(|&(ty, _)| ty)
        .collect();
    formulas
        .iter()
        .copied()
        .filter(|ty| types.get(ty).map(|t| t.category_id) == Some(CATEGORY_BLUEPRINT))
        .filter(|ty| ask_factor(CATEGORY_BLUEPRINT).is_some() && !asked.contains(ty))
        .collect()
}

/// [`Table::fixture`]'s invented `corporationTrades` rows.
const FIXTURE_TRADES: [(i64, i64, f64); 8] = [
    (1_999_001, 990_001, 0.083),
    (1_999_001, 990_004, 1.0),
    (1_999_001, 990_005, 1.0),
    (1_999_001, 990_007, 0.5),
    (1_999_001, 990_009, 0.2),
    (1_999_002, 990_002, -0.1),
    (1_999_002, 990_007, -0.2),
    (1_999_002, 990_008, -0.3),
];

impl Table {
    /// [`Self::with_schools`] with no school: no skillbook is NPC-sold.
    pub fn new(
        stations: Vec<Station>,
        trades: &[(i64, i64, f64)],
        types: BTreeMap<i64, TypeRow>,
        hubs: &[i64],
    ) -> Result<Table, String> {
        Table::with_schools(stations, trades, types, hubs, &[])
    }

    /// [`Self::build`] knowing no recipe product.
    pub fn with_schools(
        stations: Vec<Station>,
        trades: &[(i64, i64, f64)],
        types: BTreeMap<i64, TypeRow>,
        hubs: &[i64],
        schools: &[School],
    ) -> Result<Table, String> {
        let none = BTreeSet::new();
        Table::build(stations, trades, types, hubs, schools, &none, &none)
    }

    /// Build the slot index. `hubs` and school stations not in `stations`
    /// are ignored (the authored file names client stations; a smaller
    /// bundle may lack some). `products`: every recipe product
    /// (`industry/blueprints.tsv` kind `product`, R4). `invention_skills`:
    /// every skill an invention requires (kind `skill`, activity 8), of
    /// which the schools ask those no corporation asks
    /// ([`SCHOOL_INVENTION_BOOKS`]).
    pub fn build(
        stations: Vec<Station>,
        trades: &[(i64, i64, f64)],
        types: BTreeMap<i64, TypeRow>,
        hubs: &[i64],
        schools: &[School],
        products: &BTreeSet<i64>,
        invention_skills: &BTreeSet<i64>,
    ) -> Result<Table, String> {
        let none = BTreeSet::new();
        Table::build_with(
            stations,
            trades,
            types,
            hubs,
            schools,
            products,
            invention_skills,
            &none,
        )
    }

    /// [`Self::build`] with `formulas`: the reaction formulas of the booster
    /// chain ([`ORPHAN_BOOSTER_FORMULAS`]), of which the schools ask those no
    /// corporation asks.
    #[allow(clippy::too_many_arguments)]
    pub fn build_with(
        mut stations: Vec<Station>,
        trades: &[(i64, i64, f64)],
        types: BTreeMap<i64, TypeRow>,
        hubs: &[i64],
        schools: &[School],
        products: &BTreeSet<i64>,
        invention_skills: &BTreeSet<i64>,
        formulas: &BTreeSet<i64>,
    ) -> Result<Table, String> {
        stations.sort_by_key(|s| s.station_id);
        if stations
            .windows(2)
            .any(|w| w[0].station_id == w[1].station_id)
        {
            return Err("a station id appears twice".into());
        }
        if stations.len() > MAX_STATIONS || types.len() > MAX_TYPES {
            return Err("more stations or types than the NPC order id band holds".into());
        }
        let index_of = |id: i64| stations.binary_search_by_key(&id, |s| s.station_id).ok();
        // Corporation -> (type, side) for every entry an NPC posts (R1). The
        // client's table is a dict, one value per (corp, type); a repeated
        // pair keeps its first value, so one station never both asks and
        // bids a type.
        let mut posts: BTreeMap<i64, Vec<(i64, Side)>> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for &(corp, type_id, value) in trades {
            let Some(ty) = types.get(&type_id).filter(|_| seen.insert((corp, type_id))) else {
                continue;
            };
            if let Some(side) = tier_a_side(ty, value, products.contains(&type_id)) {
                posts.entry(corp).or_default().push((type_id, side));
            }
        }
        let mut cands: BTreeMap<(i64, i64), BTreeMap<(usize, Side), Tier>> = BTreeMap::new();
        let mut put = |i: usize, ty: i64, side: Side, tier: Tier| {
            let region = stations[i].region_id;
            let at = cands.entry((region, ty)).or_default();
            at.entry((i, side)).or_insert(tier);
        };
        for (i, s) in stations.iter().enumerate() {
            for &(ty, side) in posts.get(&s.owner_id).into_iter().flatten() {
                put(i, ty, side, Tier::A);
            }
        }
        if SCHOOL_BOOKS {
            for school in schools {
                let list = posts.get(&school.corp_id).into_iter().flatten();
                let books: Vec<i64> = list
                    .filter(|&&(ty, side)| {
                        side == Side::Ask
                            && types.get(&ty).map(|t| t.category_id) == Some(CATEGORY_SKILL)
                    })
                    .map(|&(ty, _)| ty)
                    .collect();
                for i in school.stations.iter().filter_map(|&s| index_of(s)) {
                    for &ty in &books {
                        put(i, ty, Side::Ask, Tier::A);
                    }
                }
            }
        }
        let invention_books = if SCHOOL_INVENTION_BOOKS {
            orphan_books(&posts, &types, invention_skills)
        } else {
            BTreeSet::new()
        };
        for school in schools {
            for i in school.stations.iter().filter_map(|&s| index_of(s)) {
                for &ty in &invention_books {
                    put(i, ty, Side::Ask, Tier::A);
                }
            }
        }
        let orphan_formulas = if ORPHAN_BOOSTER_FORMULAS {
            orphan_formulas(&posts, &types, formulas)
        } else {
            BTreeSet::new()
        };
        for school in schools {
            for i in school.stations.iter().filter_map(|&s| index_of(s)) {
                for &ty in &orphan_formulas {
                    put(i, ty, Side::Ask, Tier::A);
                }
            }
        }
        if LOOT_BIDS {
            let loot: Vec<i64> = types
                .iter()
                .filter(|&(id, t)| loot_only(t) && !products.contains(id))
                .map(|(&id, _)| id)
                .collect();
            let mut hub_ix: Vec<usize> = hubs.iter().filter_map(|&h| index_of(h)).collect();
            hub_ix.sort_unstable();
            hub_ix.dedup();
            for &i in &hub_ix {
                for &ty in &loot {
                    put(i, ty, Side::Bid, Tier::B);
                }
            }
        }
        let slots = cands
            .into_iter()
            .map(|(k, at)| {
                let v = at.into_iter().map(|((station, side), tier)| Slot {
                    station,
                    side,
                    tier,
                });
                (k, v.collect())
            })
            .collect();
        Ok(Table {
            stations,
            type_ids: types.keys().copied().collect(),
            types,
            slots,
            invention_books,
            orphan_formulas,
        })
    }

    /// The orphan booster formulas the schools ask
    /// ([`ORPHAN_BOOSTER_FORMULAS`]).
    pub fn orphan_formulas(&self) -> &BTreeSet<i64> {
        &self.orphan_formulas
    }

    /// The orphan invention books the schools ask
    /// ([`SCHOOL_INVENTION_BOOKS`]).
    pub fn invention_books(&self) -> &BTreeSet<i64> {
        &self.invention_books
    }

    /// **Every type some NPC station asks (sells)**, any region (MN-F): what
    /// a bot reads to leave out a reaction formula nobody sells, so it never
    /// spends a market read on a book that is empty by the data.
    pub fn asked_types(&self) -> BTreeSet<i64> {
        self.slots
            .iter()
            .filter(|(_, v)| v.iter().any(|s| s.side == Side::Ask))
            .map(|(&(_, t), _)| t)
            .collect()
    }

    /// The bundle's five files; absent files give an empty table.
    pub fn from_bundle(b: &Bundle, hubs: &[i64]) -> Result<Table, String> {
        let stations = parse_lines(&b.data_lines(STATIONS.0, STATIONS.1)?, parse_station)
            .map_err(|e| format!("{}: {e}", STATIONS.0))?;
        let trades = parse_lines(&b.data_lines(TRADES.0, TRADES.1)?, parse_trade)
            .map_err(|e| format!("{}: {e}", TRADES.0))?;
        let types = parse_lines(&b.data_lines(MARKET.0, MARKET.1)?, parse_type)
            .map_err(|e| format!("{}: {e}", MARKET.0))?;
        let (file, header) = crate::start::FILE;
        let schools = parse_lines(&b.data_lines(file, header)?, parse_school)
            .map_err(|e| format!("{file}: {e}"))?;
        let schools: Vec<School> = schools.into_iter().flatten().collect();
        // Published types with a market group: an orphan invention book
        // must be one the client's market can show.
        let listed: BTreeSet<i64> = parse_lines(&b.data_lines(MARKET.0, MARKET.1)?, parse_listed)
            .map_err(|e| format!("{}: {e}", MARKET.0))?
            .into_iter()
            .flatten()
            .collect();
        let (file, header) = crate::industry::BLUEPRINTS;
        let rows = parse_lines(&b.data_lines(file, header)?, parse_recipe_row)
            .map_err(|e| format!("{file}: {e}"))?;
        let mut products = BTreeSet::new();
        let mut invention_skills = BTreeSet::new();
        for row in rows {
            match row {
                RecipeRow::Product(t) => {
                    products.insert(t);
                }
                RecipeRow::InventionSkill(t) if listed.contains(&t) => {
                    invention_skills.insert(t);
                }
                RecipeRow::InventionSkill(_) | RecipeRow::Other => {}
            }
        }
        Table::build_with(
            stations,
            &trades,
            types.into_iter().collect(),
            hubs,
            &schools,
            &products,
            &invention_skills,
            &crate::booster_sources::formulas_in_bundle(b)?,
        )
    }

    /// Invented rows (stations `69000001+`, corps `1999001+`, types
    /// `990001+`, `schema.md` §1.1), never a client value. They line up with
    /// [`econ_prices::Table::fixture`]'s types; the skillbook (990005), the
    /// loot module (990006) and the commodities (990007-990009) have no price
    /// row there.
    ///
    /// Corp 1999001 owns station 69000001 and lists the blueprint, the
    /// skillbook, the trade good 990007 (+) and a module 990009 (+); corp
    /// 1999002 owns 69000002, the hub, and lists the material 990002 (-), the
    /// trade good (-) and the recipe product 990008 (-). The school is corp
    /// 1999001's, at 69000002. So 69000001 asks the blueprint, the book and
    /// the trade good; 69000002 asks the book (school), bids the material and
    /// the trade good, and bids the loot module once priced; nobody posts the
    /// module 990009 or the product 990008.
    pub fn fixture() -> Table {
        let station = |(station_id, system_id, owner_id)| Station {
            station_id,
            system_id,
            region_id: 19_000_001,
            owner_id,
        };
        let stations = [
            (69_000_001, 39_000_001, 1_999_001),
            (69_000_002, 39_000_002, 1_999_002),
        ];
        let ty = |(id, category_id, base_price, meta, tech)| {
            let (group_id, meta_group_id, tech_level) = (id - 891_000, meta, tech);
            (
                id,
                TypeRow {
                    group_id,
                    category_id,
                    base_price,
                    meta_group_id,
                    tech_level,
                },
            )
        };
        let types = [
            (990_001, 6, 250_000.0, Some(1), Some(1)),
            (990_002, 4, 10.0, None, None),
            (990_003, 25, 0.0, None, None),
            (990_004, CATEGORY_BLUEPRINT, 1_000_000.0, None, None),
            (990_005, CATEGORY_SKILL, 30_000.0, None, None),
            (990_006, 7, 5_000.0, Some(4), Some(1)),
            (990_007, CATEGORY_COMMODITY, 100.0, None, None),
            (990_008, CATEGORY_COMMODITY, 50.0, None, None),
            (990_009, 7, 2_000.0, Some(1), Some(1)),
        ];
        let school = School {
            corp_id: 1_999_001,
            stations: vec![69_000_002],
        };
        let types = types.into_iter().map(ty).collect();
        Table::build(
            stations.map(station).to_vec(),
            &FIXTURE_TRADES,
            types,
            &[69_000_002],
            &[school],
            &BTreeSet::from([990_008]),
            &BTreeSet::new(),
        )
        .unwrap_or_else(|e| unreachable!("the fixture builds: {e}"))
    }

    fn index_of(&self, station_id: i64) -> Option<usize> {
        self.stations
            .binary_search_by_key(&station_id, |s| s.station_id)
            .ok()
    }

    fn type_slot(&self, type_id: i64) -> Option<usize> {
        self.type_ids.binary_search(&type_id).ok()
    }
}

/// The registration's load: the bundle's files and the authored hubs, or an
/// empty table with no bundle (see the module doc).
fn load_table(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(HUBS_FILE, SHIPPED_HUBS, HUBS_HEADER)?;
    let hubs = file.parse_rows(|r| r.i64(0));
    Ok(Box::new(match src.bundle {
        Some(b) => {
            src.note(Origin::Bundle);
            Table::from_bundle(b, &hubs)?
        }
        None => Table::default(),
    }))
}

fn parse_lines<S: AsRef<str>, T>(
    lines: &[S],
    f: impl Fn(&[&str]) -> Option<T>,
) -> Result<Vec<T>, String> {
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let fields: Vec<&str> = l.as_ref().split('\t').collect();
            f(&fields).ok_or_else(|| format!("data line {}: {:?}", i + 1, l.as_ref()))
        })
        .collect()
}

fn opt(s: &str) -> Option<Option<i64>> {
    match s {
        "" => Some(None),
        v => v.parse().ok().map(Some),
    }
}

fn parse_station(f: &[&str]) -> Option<Station> {
    let [id, system, region, owner, _op, _services, _eff, _take] = f[..] else {
        return None;
    };
    Some(Station {
        station_id: id.parse().ok()?,
        system_id: system.parse().ok()?,
        region_id: region.parse().ok()?,
        owner_id: owner.parse().ok()?,
    })
}

fn parse_trade(f: &[&str]) -> Option<(i64, i64, f64)> {
    let [corp, type_id, value] = f[..] else {
        return None;
    };
    let value: f64 = value.parse().ok()?;
    value
        .is_finite()
        .then_some((corp.parse().ok()?, type_id.parse().ok()?, value))
}

fn parse_type(f: &[&str]) -> Option<(i64, TypeRow)> {
    if f.len() != 14 {
        return None;
    }
    Some((
        f[0].parse().ok()?,
        TypeRow {
            group_id: f[1].parse().ok()?,
            category_id: f[2].parse().ok()?,
            base_price: f[4].parse().ok().filter(|p: &f64| p.is_finite())?,
            meta_group_id: opt(f[5])?,
            tech_level: opt(f[7])?,
        },
    ))
}

/// What one `industry/blueprints.tsv` row tells the exchange.
enum RecipeRow {
    /// Some activity's product (R4).
    Product(i64),
    /// A skill an invention (activity 8) requires.
    InventionSkill(i64),
    Other,
}

/// One `industry/blueprints.tsv` row.
fn parse_recipe_row(f: &[&str]) -> Option<RecipeRow> {
    let [_bp, activity, kind, type_id, _q, _p, _time, _runs] = f[..] else {
        return None;
    };
    Some(match (kind, activity) {
        ("product", _) => RecipeRow::Product(type_id.parse().ok()?),
        ("skill", a) if a.parse::<i64>().ok()? == crate::industry::INVENTION => {
            RecipeRow::InventionSkill(type_id.parse().ok()?)
        }
        _ => RecipeRow::Other,
    })
}

/// One `types/market.tsv` row: its type when it is published with a market
/// group.
fn parse_listed(f: &[&str]) -> Option<Option<i64>> {
    if f.len() != 14 {
        return None;
    }
    let id: i64 = f[0].parse().ok()?;
    Some((!f[3].is_empty() && f[13] == "1").then_some(id))
}

/// One `agents/schools.tsv` row (`crate::start::FILE`'s nine columns): the
/// school, or `Some(None)` for a starter-space school, which places nobody
/// (`start::Table::schools_of_race`) and so sells nothing **[A]**.
fn parse_school(f: &[&str]) -> Option<Option<School>> {
    let [
        _id,
        corp,
        _race,
        _career,
        _system,
        stations,
        _agents,
        _name,
        starter,
    ] = f[..]
    else {
        return None;
    };
    let stations = match stations {
        "" => Vec::new(),
        s => s
            .split(',')
            .map(|x| x.parse().ok())
            .collect::<Option<Vec<i64>>>()?,
    };
    let school = School {
        corp_id: corp.parse().ok()?,
        stations,
    };
    match starter {
        "0" => Some(Some(school)),
        "1" => Some(None),
        _ => None,
    }
}

// ---- ids --------------------------------------------------------------------------

/// C5 §4.1: `NPC_ORDER_BASE + station_index · 2^20 + type_slot · 2 +
/// is_bid`. `station_index` is the station's position in the bundle's
/// sorted station file, `type_slot` the type's in `types/market.tsv`'s sorted
/// ids: both recomputed on load, never persisted (plan §3.4).
pub fn order_id(station_index: usize, type_slot: usize, side: Side) -> i64 {
    NPC_ORDER_BASE
        + station_index as i64 * STATION_STRIDE
        + type_slot as i64 * 2
        + i64::from(side == Side::Bid)
}

/// Whether `id` is in the NPC order band.
pub fn is_npc(id: i64) -> bool {
    (NPC_ORDER_BASE..NPC_ORDER_END).contains(&id) || is_dev(id)
}

// ---- the dev seller (DEV-MODE) ------------------------------------------------------

/// First id of the dev seller's orders: `DEV_ORDER_BASE + station_id · 2^24 +
/// type_id`. Far above [`NPC_ORDER_END`] and below 2^53. Never stored: the
/// order is synthesised at read time.
pub const DEV_ORDER_BASE: i64 = 1 << 52;
const DEV_STATION_STRIDE: i64 = 1 << 24;
/// What the dev seller stocks and asks, per station and type (`--dev` only).
pub const DEV_STOCK: i64 = 10_000_000;
/// The dev ask, ISK a unit.
pub const DEV_PRICE: f64 = 100.0;

/// Whether `id` is a dev seller order.
pub fn is_dev(id: i64) -> bool {
    id >= DEV_ORDER_BASE
}

/// The dev seller's ask on `type_id` at `station`: always [`DEV_STOCK`]
/// units at [`DEV_PRICE`] (a buy takes from it and it is full again).
fn dev_order(st: &Station, type_id: i64, issue_date: i64) -> NpcOrder {
    NpcOrder {
        order_id: DEV_ORDER_BASE + st.station_id * DEV_STATION_STRIDE + type_id,
        corp_id: st.owner_id,
        type_id,
        station_id: st.station_id,
        system_id: st.system_id,
        region_id: st.region_id,
        price: DEV_PRICE,
        side: Side::Ask,
        tier: Tier::A,
        vol: DEV_STOCK,
        remaining: DEV_STOCK,
        issue_date,
    }
}

/// The dev seller's asks (call only under `--dev`): every market type at
/// every station of `region_id`, narrowed to `only` (a type) and/or
/// `station` (a station). With neither it is the whole region's catalogue,
/// so callers pass one.
pub fn dev_orders(
    reg: &Registry,
    region_id: i64,
    station: Option<i64>,
    only: Option<i64>,
) -> Vec<NpcOrder> {
    let t = reg.content().get::<Table>();
    let issue_date = reg.world().last_rollover;
    let stations = t
        .stations
        .iter()
        .filter(|s| s.region_id == region_id && station.is_none_or(|id| id == s.station_id));
    let mut out = Vec::new();
    for st in stations {
        match only {
            Some(ty) if t.type_slot(ty).is_some() => out.push(dev_order(st, ty, issue_date)),
            Some(_) => {}
            None => out.extend(t.type_ids.iter().map(|&ty| dev_order(st, ty, issue_date))),
        }
    }
    out
}

// ---- the refill -------------------------------------------------------------------

/// Units back after `k` whole [`TRICKLE_PERIOD`]s: `k ×
/// ceil(vol / TRICKLE_STEPS)`, capped at `vol`. The quantum is rounded up so
/// a small order refills too, and an empty order is full within
/// `TRICKLE_STEPS` periods (C5 §4.4).
pub fn trickle(vol: i64, k: i64) -> i64 {
    let per = (vol + TRICKLE_STEPS - 1) / TRICKLE_STEPS;
    per.saturating_mul(k.max(0)).min(vol)
}

/// Whole periods between two times, counted as boundaries crossed, so that
/// touching an order does not throw away the part-period since the last.
///
/// Lane clock: the period is `span(TRICKLE_PERIOD)`, so an emptied order is
/// full again in a compressed day. The boundaries are counted on the wall
/// clock from both stored times, so a restart at another scale counts the
/// time since the last take at the new period.
fn periods(from: i64, to: i64, period: i64) -> i64 {
    (to.div_euclid(period) - from.div_euclid(period)).max(0)
}

/// [`TRICKLE_PERIOD`] at the registry's game clock (lane `clock`).
fn trickle_period(reg: &Registry) -> i64 {
    crate::game_clock::span(reg, TRICKLE_PERIOD)
}

fn key(station_id: i64, type_id: i64, side: Side) -> Vec<String> {
    vec![
        station_id.to_string(),
        type_id.to_string(),
        side.as_str().to_owned(),
    ]
}

/// What an order of `vol` has left at `now`: the stored row refilled, or
/// `vol` with no row (an unreadable row is no row).
fn remaining(reg: &Registry, k: &[String], vol: i64, now: i64) -> i64 {
    let row = reg.deltas().of(KIND).get(k).map(|v| &v[..]);
    let Some([r, t]) = row else { return vol };
    match (r.parse::<i64>(), t.parse::<i64>()) {
        (Ok(r), Ok(t)) => (r.max(0) + trickle(vol, periods(t, now, trickle_period(reg)))).min(vol),
        _ => vol,
    }
}

/// Take `quantity` from the NPC order at `at`, `(station_id, type_id, side)`, which
/// had `before` left at `now` (the order the caller read this turn).
pub fn take(reg: &mut Registry, at: (i64, i64, Side), before: i64, quantity: i64, now: i64) {
    let left = (before - quantity).max(0);
    let k = key(at.0, at.1, at.2);
    reg.deltas_mut()
        .put(KIND, k, vec![left.to_string(), now.to_string()]);
}

// ---- the orders --------------------------------------------------------------------

/// One NPC order as a reader sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct NpcOrder {
    pub order_id: i64,
    /// The station's owner, the order's `charID`.
    pub corp_id: i64,
    pub type_id: i64,
    pub station_id: i64,
    pub system_id: i64,
    pub region_id: i64,
    pub price: f64,
    pub side: Side,
    pub tier: Tier,
    /// Full stock.
    pub vol: i64,
    pub remaining: i64,
    /// The day's rollover time.
    pub issue_date: i64,
}

/// A Tier B multiplier: the stored row clamped into its band, or the start.
pub fn multiplier(reg: &Registry, station_id: i64, type_id: i64, side: Side) -> f64 {
    let (lo, start, hi) = band(side);
    reg.deltas()
        .of(MULT_KIND)
        .get(&key(station_id, type_id, side))
        .and_then(|v| v.first()?.parse::<f64>().ok())
        .filter(|m| m.is_finite())
        .map_or(start, |m| m.clamp(lo, hi))
}

fn band(side: Side) -> (f64, f64, f64) {
    match side {
        Side::Ask => ASK_MULT,
        Side::Bid => BID_MULT,
    }
}

fn has_recipe(p: &econ_prices::Price) -> bool {
    matches!(p.source, Source::Recipe | Source::Reaction)
}

/// Round an ask up and a bid down to the cent, so rounding never favours
/// the pilot; `None` for a bid that rounds to nothing.
fn to_cents(x: f64, side: Side) -> Option<f64> {
    let c = match side {
        Side::Ask => (x * 100.0).ceil(),
        Side::Bid => (x * 100.0).floor(),
    } / 100.0;
    (c.is_finite() && c >= 0.01).then_some(c)
}

/// An order's price (R3 for Tier A, C5 §4.3 for Tier B), `None` when it has
/// none. The slot index only makes the pairs R1 and [`LOOT_BIDS`] allow;
/// any other pair is `None`, so a hand-built slot can never price an order
/// the rules forbid.
fn price(
    reg: &Registry,
    t: &Table,
    prices: &econ_prices::Table,
    station_id: i64,
    type_id: i64,
    side: Side,
    tier: Tier,
) -> Option<f64> {
    let ty = t.types.get(&type_id)?;
    let p = prices.prices.get(&type_id);
    let base = (ty.base_price > 0.0).then_some(ty.base_price).or_else(|| {
        // An orphan invention book with no `basePrice` (the encryption
        // methods): [`INVENTION_BOOK_ASK`], an ask only.
        (tier == Tier::A && side == Side::Ask && t.invention_books.contains(&type_id))
            .then_some(INVENTION_BOOK_ASK)
    });
    let base = base.or_else(|| {
        (tier == Tier::A && side == Side::Ask && t.orphan_formulas.contains(&type_id))
            .then_some(ORPHAN_FORMULA_ASK)
    });
    let raw = match (tier, side) {
        (Tier::A, Side::Ask) => {
            // Retail at `basePrice` (0 is no order), never below the
            // refine floor, so §6 invariant 1 holds for these too [A].
            let ask = base? * ask_factor(ty.category_id)?;
            ask.max(ASK_REFINE_GUARD * p.map_or(0.0, |p| p.floor_price))
        }
        // No NPC bid on a buildable type (§6 invariant 2): the slot index
        // already left out the recipe products; the oracle's source is the
        // second check.
        (Tier::A, Side::Bid) if bid_category(ty.category_id) => {
            if p.is_some_and(has_recipe) {
                return None;
            }
            base? * TIER_A_BID
        }
        (Tier::B, Side::Bid) if LOOT_BIDS => {
            let p = p.filter(|p| !has_recipe(p))?;
            multiplier(reg, station_id, type_id, side) * p.ref_price
        }
        _ => return None,
    };
    to_cents(raw, side)
}

/// An order's full stock (C5 §4.2, §4.3).
fn volume(tier: Tier, price: f64) -> i64 {
    match tier {
        Tier::A => TIER_A_STOCK,
        Tier::B => {
            let (lo, hi) = TIER_B_UNITS;
            ((TIER_B_VALUE / price).ceil() as i64).clamp(lo, hi)
        }
    }
}

/// The `(region, type)` keys [`Table::slots`] holds for `region_id`, of
/// `only` that type.
fn slot_range(region_id: i64, only: Option<i64>) -> std::ops::RangeInclusive<(i64, i64)> {
    match only {
        Some(ty) => (region_id, ty)..=(region_id, ty),
        None => (region_id, i64::MIN)..=(region_id, i64::MAX),
    }
}

/// Every NPC order in `region_id` (of `only` that type), empty ones
/// included, read off the slot index. A starter type gets no bid, and a type
/// with an NPC ask anywhere in the region has no Tier B bid there (C5 §6
/// invariant 3). A Tier A bid stands beside a Tier A ask: TQ's trade goods,
/// both sides at once (O5-i).
fn all_orders(reg: &Registry, region_id: i64, only: Option<i64>, now: i64) -> Vec<NpcOrder> {
    let t = reg.content().get::<Table>();
    let prices = reg.content().get::<econ_prices::Table>();
    let starter = reg.content().get::<crate::starter_ship::Table>().types();
    let issue_date = reg.world().last_rollover;
    let mut out = Vec::new();
    for (&(_, ty), slots) in t.slots.range(slot_range(region_id, only)) {
        let Some(type_slot) = t.type_slot(ty) else {
            continue;
        };
        for s in slots {
            if s.side == Side::Bid && starter.binary_search(&ty).is_ok() {
                continue;
            }
            let st = t.stations[s.station];
            let Some(price) = price(reg, t, prices, st.station_id, ty, s.side, s.tier) else {
                continue;
            };
            let vol = volume(s.tier, price);
            out.push(NpcOrder {
                order_id: order_id(s.station, type_slot, s.side),
                corp_id: st.owner_id,
                type_id: ty,
                station_id: st.station_id,
                system_id: st.system_id,
                region_id,
                price,
                side: s.side,
                tier: s.tier,
                vol,
                remaining: remaining(reg, &key(st.station_id, ty, s.side), vol, now),
                issue_date,
            });
        }
    }
    let asked: BTreeSet<i64> = out
        .iter()
        .filter(|o| o.side == Side::Ask)
        .map(|o| o.type_id)
        .collect();
    out.retain(|o| o.side == Side::Ask || o.tier == Tier::A || !asked.contains(&o.type_id));
    out
}

/// The NPC orders a reader shows in `region_id`, of `only` that type: every
/// one with stock left at `now`.
pub fn orders(reg: &Registry, region_id: i64, only: Option<i64>, now: i64) -> Vec<NpcOrder> {
    let mut v = all_orders(reg, region_id, only, now);
    v.retain(|o| o.remaining > 0);
    v
}

/// What the NPC orders of one `(region, type)` are computed from beyond the
/// content, which is fixed at boot: the `npcorder` and `npcmult` rows of its
/// slots, `last_rollover` (each order's `issueDate`) and the trickle period
/// `now` falls in. Equal stamps give equal orders, so a `GetOrders` cache
/// entry keyed by `(region, type)` holds while its stamp does (P6): a fill on
/// another type's NPC order no longer drops it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    period: i64,
    last_rollover: i64,
    rows: Vec<Option<Vec<String>>>,
}

/// The [`Stamp`] of `(region_id, type_id)` at `now`.
pub fn stamp(reg: &Registry, region_id: i64, type_id: i64, now: i64) -> Stamp {
    let t = reg.content().get::<Table>();
    let (orders, mults) = (reg.deltas().of(KIND), reg.deltas().of(MULT_KIND));
    let mut rows = Vec::new();
    for s in t.slots.get(&(region_id, type_id)).into_iter().flatten() {
        let k = key(t.stations[s.station].station_id, type_id, s.side);
        rows.push(orders.get(&k).cloned());
        rows.push(mults.get(&k).cloned());
    }
    Stamp {
        period: now.div_euclid(trickle_period(reg)),
        last_rollover: reg.world().last_rollover,
        rows,
    }
}

/// `(system_id, region_id)` of a station the exchange knows: where a remote
/// buy from an NPC ask happens.
pub fn place(reg: &Registry, station_id: i64) -> Option<(i64, i64)> {
    let t = reg.content().get::<Table>();
    t.index_of(station_id)
        .map(|i| (t.stations[i].system_id, t.stations[i].region_id))
}

// ---- the rollover -------------------------------------------------------------------

/// One market trade, as the rollover reads it (`service/market.rs` hands
/// these over; NPC fills carry the station's corporation as the NPC side).
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub date: i64,
    pub station_id: i64,
    pub type_id: i64,
    pub quantity: i64,
    pub price: f64,
    pub buyer_id: i64,
    pub seller_id: i64,
}

/// One resting player or bot order, as the economy report reads the book
/// (FE-MEASURE; `service/market.rs` hands these over). Never an NPC order.
#[derive(Debug, Clone, PartialEq)]
pub struct BookRow {
    pub char_id: i64,
    pub type_id: i64,
    pub region_id: i64,
    pub price: f64,
    pub remaining: i64,
    pub bid: bool,
}

/// The NPC side a fill took from, if it was an NPC fill.
fn npc_side(t: &Table, f: &Fill) -> Option<Side> {
    let owner = t.stations[t.index_of(f.station_id)?].owner_id;
    let sides = [(f.seller_id, Side::Ask), (f.buyer_id, Side::Bid)];
    sides
        .into_iter()
        .find(|&(id, _)| id == owner)
        .map(|(_, s)| s)
}

/// C5 §4.5's step for one Tier B multiplier, given the window's fill
/// fraction. Bounded by [`ASK_MULT`] and [`BID_MULT`], deterministic.
pub fn step(m: f64, side: Side, f: f64) -> f64 {
    let (lo, _, hi) = band(side);
    let m = match side {
        Side::Ask if f >= FILL_HIGH => m * ASK_UP,
        Side::Ask if f <= FILL_LOW => m / ASK_DOWN,
        Side::Bid if f >= FILL_HIGH => m * BID_DOWN,
        Side::Bid if f <= FILL_LOW => m * BID_UP,
        _ => m,
    };
    m.clamp(lo, hi)
}

/// The rollover step (plan §4.4 step 4), over the `days` just ended: fold
/// each traded type's volume-weighted mean into the index (NPC fills count,
/// C5 §3.4); step each traded Tier B multiplier by its fill fraction
/// `filled / (vol × days)`; delete every [`KIND`] row that has refilled or
/// lost its order (plan §4.2). A part-empty order keeps refilling: the
/// rollover is no restock, so a hub bought out at 10:59 is not full at 11:00
/// [A]. Returns the rows pruned.
pub fn on_rollover(reg: &mut Registry, d: &crate::rollover::Day) -> usize {
    let now = d.now;
    let days = i64::from(d.days.max(1));
    // Lane clock: the days just ended are compressed days.
    let fills = reg.market_fills_since(now - crate::game_clock::span(reg, days * DAY));

    let mut by_type: BTreeMap<i64, (f64, i64)> = BTreeMap::new();
    // FE-GUARD-A: a wash trade or an outlier moves no index (R-FLUIDECON §5
    // G2); the multipliers below, and `mkthist`, still see every fill.
    for f in fills
        .iter()
        .filter(|f| econ_prices::counts_for_index(reg, f.type_id, f.buyer_id, f.seller_id, f.price))
    {
        let e = by_type.entry(f.type_id).or_default();
        e.0 += f.price * f.quantity as f64;
        e.1 += f.quantity;
    }
    for (ty, (value, qty)) in by_type {
        if qty > 0 {
            econ_prices::fold_day(reg, ty, value / qty as f64, d.day);
        }
    }

    let mut filled: BTreeMap<(i64, i64, Side), i64> = BTreeMap::new();
    {
        let t = reg.content().get::<Table>();
        for f in &fills {
            if let Some(side) = npc_side(t, f) {
                *filled.entry((f.station_id, f.type_id, side)).or_default() += f.quantity;
            }
        }
    }
    for ((station_id, ty, side), qty) in filled {
        let Some(o) = order_at(reg, (station_id, ty, side), now).filter(|o| o.tier == Tier::B)
        else {
            continue;
        };
        let f = qty as f64 / (o.vol * days) as f64;
        let m = step(multiplier(reg, station_id, ty, side), side, f);
        let k = key(station_id, ty, side);
        if m == band(side).1 {
            reg.deltas_mut().del(MULT_KIND, &k);
        } else {
            reg.deltas_mut().put(MULT_KIND, k, vec![m.to_string()]);
        }
    }

    let rows: Vec<Vec<String>> = reg.deltas().of(KIND).keys().cloned().collect();
    let mut pruned = 0;
    for k in rows {
        let side = |x: &String| [Side::Ask, Side::Bid].into_iter().find(|s| s.as_str() == x);
        let at = match &k[..] {
            [s, ty, x] => s.parse().ok().zip(ty.parse().ok()).zip(side(x)),
            _ => None,
        };
        let order = at.and_then(|((s, ty), side)| order_at(reg, (s, ty, side), now));
        if order.is_none_or(|o| o.remaining >= o.vol) {
            reg.deltas_mut().del(KIND, &k);
            pruned += 1;
        }
    }
    pruned
}

/// The NPC order at `(station_id, type_id, side)`, empty or not.
fn order_at(
    reg: &Registry,
    (station_id, ty, side): (i64, i64, Side),
    now: i64,
) -> Option<NpcOrder> {
    let (_, region) = place(reg, station_id)?;
    let all = all_orders(reg, region, Some(ty), now);
    all.into_iter()
        .find(|o| o.station_id == station_id && o.side == side)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SandboxConfig;

    const NOW: i64 = 134_200_000_000_000_000;
    const REGION: i64 = 19_000_001;
    /// The fixture's types and stations (see [`Table::fixture`]).
    const BPO: i64 = 990_004;
    const SKILLBOOK: i64 = 990_005;
    const LOOT: i64 = 990_006;
    const CORP_STATION: i64 = 69_000_001;
    const SCHOOL_STATION: i64 = 69_000_002;

    fn with(t: Table, p: econ_prices::Table) -> Registry {
        let mut c = crate::content::Content::fixture();
        c.insert("econ_exchange", t);
        c.insert("econ_prices", p);
        Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(c))
    }

    /// The price fixture plus an invented row for the loot module, which has
    /// no recipe.
    fn prices() -> econ_prices::Table {
        let mut p = econ_prices::Table::fixture();
        let line = format!("{LOOT}\t6000.0\tparent\t2000.0\t7\t99006\t4\t1");
        let loot = econ_prices::Table::parse(&[line]).expect("parses");
        p.prices.extend(loot.prices);
        p
    }

    fn fixture() -> Registry {
        with(Table::fixture(), prices())
    }

    fn find(reg: &Registry, f: impl Fn(&NpcOrder) -> bool) -> NpcOrder {
        let all = orders(reg, REGION, None, NOW);
        all.into_iter().find(|o| f(o)).expect("the fixture has it")
    }

    /// **CP35**: ids are disjoint from the player order (5e9 up),
    /// transaction (7e9 up) and escrow (9e9 up) bands, and dense.
    #[test]
    fn ids_are_disjoint_from_the_5e9_and_7e9_bands() {
        let (s, t) = (MAX_STATIONS - 1, MAX_TYPES - 1);
        for id in [order_id(0, 0, Side::Ask), order_id(s, t, Side::Bid)] {
            assert!(is_npc(id) && id > 9_000_000_000 * 1_000, "{id}");
        }
        for id in [5_000_000_000, 7_000_000_000, 9_000_000_000, NPC_ORDER_END] {
            assert!(!is_npc(id));
        }
        assert_eq!(
            order_id(1, 0, Side::Ask) - order_id(0, MAX_TYPES - 1, Side::Bid),
            1
        );
    }

    /// **CP35**: after `TRICKLE_PERIOD·k` the refill equals `trickle(k)`,
    /// capped at full; an empty order is not shown, and full within a day.
    #[test]
    fn after_trickle_period_k_the_refill_equals_trickle_k_capped_at_full() {
        let mut reg = fixture();
        let o = find(&reg, |o| o.side == Side::Ask && o.vol > TRICKLE_STEPS);
        assert_eq!(o.remaining, o.vol, "no row is full");
        let at = (o.station_id, o.type_id, o.side);
        take(&mut reg, at, o.vol, o.vol, NOW);
        assert!(
            orders(&reg, REGION, None, NOW)
                .iter()
                .all(|x| x.order_id != o.order_id)
        );
        for k in 0..=TRICKLE_STEPS + 2 {
            let at = all_orders(&reg, REGION, Some(o.type_id), NOW + k * TRICKLE_PERIOD);
            let left = at
                .iter()
                .find(|x| x.order_id == o.order_id)
                .expect("listed");
            assert_eq!(left.remaining, trickle(o.vol, k).min(o.vol));
        }
        assert!(
            [1, 23, 25, 1000]
                .iter()
                .all(|&v| trickle(v, TRICKLE_STEPS) == v)
        );
    }

    /// Lane clock: at scale N an emptied NPC order refills N times faster,
    /// one `trickle` step per `TRICKLE_PERIOD / N`.
    #[test]
    fn at_scale_n_an_npc_order_refills_n_times_faster() {
        let mut reg = fixture();
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(4).unwrap());
        let o = find(&reg, |o| o.side == Side::Ask && o.vol > TRICKLE_STEPS);
        take(
            &mut reg,
            (o.station_id, o.type_id, o.side),
            o.vol,
            o.vol,
            NOW,
        );
        for k in 0..=TRICKLE_STEPS + 1 {
            let at = all_orders(&reg, REGION, Some(o.type_id), NOW + k * TRICKLE_PERIOD / 4);
            let left = at
                .iter()
                .find(|x| x.order_id == o.order_id)
                .expect("listed");
            assert_eq!(left.remaining, trickle(o.vol, k).min(o.vol), "k = {k}");
        }
    }

    /// A tiny deterministic generator, so the property tests need no crate.
    struct Gen(u64);
    impl Gen {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
            xs[(self.next() % xs.len() as u64) as usize]
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// What a catalogue was built from, for the linear reference.
    struct Inputs {
        stations: Vec<Station>,
        trades: Vec<(i64, i64, f64)>,
        types: BTreeMap<i64, TypeRow>,
        hubs: Vec<i64>,
        schools: Vec<School>,
        products: BTreeSet<i64>,
    }

    /// A random invented catalogue (`P_ref ≥ floor`, CP24's guarantee), with
    /// schools (one naming a station nobody has), hubs and multiplier rows
    /// some outside their band, as a hand edit might leave.
    fn catalogue(seed: u64) -> (Registry, Inputs) {
        use Source::*;
        let mut g = Gen(seed | 1);
        let (mut types, mut prices) = (BTreeMap::new(), BTreeMap::new());
        for ty in 990_100..990_220 {
            let row = TypeRow {
                group_id: g.pick(&[99_001, 18, 370, 754, 1032, 1042]),
                category_id: g.pick(&[4, 6, 7, 8, 9, 16, 17, 18, 20, 25, 40, 41, 66, 2143]),
                base_price: g.pick(&[0.0, 0.5, 100.0, 3e6]),
                meta_group_id: g.pick(&[None, Some(1), Some(2), Some(4), Some(5)]),
                tech_level: g.pick(&[None, Some(1), Some(2)]),
            };
            types.insert(ty, row);
            if g.unit() < 0.85 {
                let ref_price = 0.001 + g.unit() * g.pick(&[1.0, 1e3, 1e7]);
                let source = g.pick(&[Recipe, Reaction, Pi, Base, Parent, Override]);
                let (category_id, group_id) = (row.category_id, row.group_id);
                let floor_price = ref_price * g.unit();
                let meta_group_id = row.meta_group_id;
                let p = econ_prices::Price {
                    ref_price,
                    source,
                    floor_price,
                    category_id,
                    group_id,
                    meta_group_id,
                    portion_size: 1,
                };
                prices.insert(ty, p);
            }
        }
        let ids: Vec<i64> = types.keys().copied().collect();
        let stations: Vec<Station> = (0..18)
            .rev()
            .map(|s| Station {
                station_id: 69_000_001 + s,
                system_id: 39_000_001 + s / 2,
                region_id: REGION + s % 3,
                owner_id: 1_999_001 + s % 5,
            })
            .collect();
        let values = [-1.0, -0.1, 0.0, 0.083, 1.0];
        let trades: Vec<_> = (0..300)
            .map(|i| (1_999_001 + i % 6, g.pick(&ids), g.pick(&values)))
            .collect();
        let hubs = vec![69_000_001, 69_000_005, 69_000_009, 69_999_999];
        let schools = vec![
            School {
                corp_id: 1_999_002,
                stations: vec![69_000_003, 69_000_004, 69_999_998],
            },
            School {
                corp_id: 1_999_006,
                stations: vec![69_000_010, 69_000_011],
            },
        ];
        let products: BTreeSet<i64> = ids.iter().copied().filter(|_| g.unit() < 0.2).collect();
        let table = Table::build(
            stations.clone(),
            &trades,
            types.clone(),
            &hubs,
            &schools,
            &products,
            &BTreeSet::new(),
        )
        .expect("builds");
        let mut reg = with(table, econ_prices::Table { prices });
        for _ in 0..80 {
            let k = key(g.pick(&hubs), g.pick(&ids), g.pick(&[Side::Ask, Side::Bid]));
            let m = g.pick(&[0.0, 0.3, 0.65, 1.0, 1.3, 2.5, f64::NAN]);
            reg.deltas_mut().put(MULT_KIND, k, vec![m.to_string()]);
        }
        for _ in 0..40 {
            let at = (g.pick(&[69_000_001, 69_000_003, 69_000_010]), g.pick(&ids));
            let left = g.pick(&[0, 1, 7, 900]);
            take(
                &mut reg,
                (at.0, at.1, g.pick(&[Side::Ask, Side::Bid])),
                left,
                0,
                NOW - HOUR,
            );
        }
        let inputs = Inputs {
            stations,
            trades,
            types,
            hubs,
            schools,
            products,
        };
        (reg, inputs)
    }

    /// Every order of 39 catalogues, with its seed, price row and inputs'
    /// type row.
    fn every_order() -> Vec<(u64, NpcOrder, Option<econ_prices::Price>, TypeRow)> {
        let mut out = Vec::new();
        for seed in 1..40 {
            let (reg, inputs) = catalogue(seed);
            let prices = &reg.content().get::<econ_prices::Table>().prices;
            for r in REGION..REGION + 3 {
                for o in all_orders(&reg, r, None, NOW) {
                    let p = prices.get(&o.type_id).cloned();
                    out.push((seed, o.clone(), p, inputs.types[&o.type_id]));
                }
            }
        }
        out
    }

    /// The highest price a Tier B bid can pay, as a multiple of `P_ref`.
    const MAX_BID: f64 = BID_MULT.2;

    /// **CP35, C5 §6 invariant 1**: an NPC ask then a refine loses money: the
    /// output sells for at most `REFINE_YIELD_MAX × MAX_BID × floor`. A Tier
    /// A bid pays at most `TIER_A_BID × basePrice` (R3).
    #[test]
    fn invariant_1_an_npc_ask_then_reprocess_loses_money() {
        const { assert!(REFINE_YIELD_MAX * MAX_BID < ASK_REFINE_GUARD) };
        let mut seen = BTreeSet::new();
        for (_, o, p, row) in every_order() {
            seen.insert((o.tier, o.side));
            match (o.tier, o.side) {
                (Tier::A, Side::Ask) => {
                    let floor = p.map_or(0.0, |p| p.floor_price);
                    assert!(o.price > REFINE_YIELD_MAX * MAX_BID * floor, "{o:?}");
                }
                (Tier::A, Side::Bid) => {
                    assert!(o.price <= TIER_A_BID * row.base_price + 1e-9, "{o:?}");
                }
                (Tier::B, Side::Bid) => {
                    let p = p.expect("a bid is priced").ref_price;
                    assert!(o.price <= MAX_BID * p + 1e-9, "{o:?}");
                }
                (Tier::B, Side::Ask) => panic!("no ceiling ask under O5: {o:?}"),
            }
        }
        let mut all = BTreeSet::from([(Tier::A, Side::Ask), (Tier::A, Side::Bid)]);
        if LOOT_BIDS {
            all.insert((Tier::B, Side::Bid));
        }
        assert_eq!(seen, all, "every allowed kind exercised, nothing else");
    }

    /// **E5b (R1)**: every ask is Tier A, in [`ASK_CATEGORIES`] (blueprints
    /// iff `NPC_BPOS`), from a positive entry of the station owner (or, for a
    /// book, a school's corporation); every Tier A bid is in
    /// [`BID_CATEGORIES`] from a negative entry. Ships, modules, charges,
    /// drones, hardwirings and category 40 are never posted at Tier A.
    #[test]
    fn the_trades_sign_is_the_side_within_the_category_whitelist() {
        let mut categories = BTreeSet::new();
        for seed in 1..40 {
            let (reg, x) = catalogue(seed);
            for r in REGION..REGION + 3 {
                for o in all_orders(&reg, r, None, NOW) {
                    let cat = x.types[&o.type_id].category_id;
                    let listed = |corp: i64, pos: bool| {
                        x.trades.iter().any(|&(c, t, v)| {
                            c == corp && t == o.type_id && (v > 0.0) == pos && v != 0.0
                        })
                    };
                    if o.side == Side::Ask {
                        assert_eq!(o.tier, Tier::A, "{o:?}");
                        assert!(ask_factor(cat).is_some(), "{o:?}");
                        let school = |s: &School| {
                            cat == CATEGORY_SKILL
                                && s.stations.contains(&o.station_id)
                                && listed(s.corp_id, true)
                        };
                        assert!(
                            listed(o.corp_id, true) || x.schools.iter().any(school),
                            "{o:?}"
                        );
                    } else if o.tier == Tier::A {
                        assert!(bid_category(cat) && listed(o.corp_id, false), "{o:?}");
                    }
                    if o.tier == Tier::A {
                        assert!(![6, 7, 8, 18, 20, 40].contains(&cat), "{o:?}");
                        categories.insert((cat, o.side));
                    }
                }
            }
        }
        for want in [
            (CATEGORY_SKILL, Side::Ask),
            (CATEGORY_COMMODITY, Side::Ask),
            (CATEGORY_COMMODITY, Side::Bid),
            (CATEGORY_COMMAND_CENTER, Side::Ask),
            (25, Side::Bid),
            (4, Side::Bid),
            (2143, Side::Bid),
        ] {
            assert!(categories.contains(&want), "{want:?} exercised");
        }
    }

    /// **E5b (R1)**, the packet's own case: a corp with +0.5 on a blueprint
    /// and -0.1 on a commodity gets an ask on the first and a bid on the
    /// second at both of its stations; its +0.5 on a module posts nothing.
    #[test]
    fn a_corps_signs_post_an_ask_and_a_bid_at_both_its_stations() {
        let (corp, bpo, goods, module) = (1_999_301, 990_301, 990_302, 990_303);
        let station = |station_id| Station {
            station_id,
            system_id: 39_000_301,
            region_id: REGION,
            owner_id: corp,
        };
        let row = |category_id, meta_group_id| TypeRow {
            group_id: 99_301,
            category_id,
            base_price: 1_000.0,
            meta_group_id,
            tech_level: None,
        };
        let types = BTreeMap::from([
            (bpo, row(CATEGORY_BLUEPRINT, None)),
            (goods, row(CATEGORY_COMMODITY, None)),
            (module, row(7, Some(1))),
        ]);
        let trades = [(corp, bpo, 0.5), (corp, goods, -0.1), (corp, module, 0.5)];
        let stations = vec![station(69_000_301), station(69_000_302)];
        let t = Table::new(stations, &trades, types, &[]).expect("builds");
        let reg = with(t, econ_prices::Table::default());
        let all = orders(&reg, REGION, None, NOW);
        let at = |ty, side| {
            let mut v: Vec<i64> = all
                .iter()
                .filter(|o| o.type_id == ty && o.side == side)
                .map(|o| o.station_id)
                .collect();
            v.sort_unstable();
            v
        };
        let both = [69_000_301, 69_000_302];
        assert_eq!(at(bpo, Side::Ask), if NPC_BPOS { &both[..] } else { &[] });
        assert_eq!(at(goods, Side::Bid), both);
        assert!(at(goods, Side::Ask).is_empty() && at(bpo, Side::Bid).is_empty());
        assert!(all.iter().all(|o| o.type_id != module), "{all:?}");
    }

    /// **E5b (R2)**: a skillbook is asked at every station of a corporation
    /// that lists it, school or not, and (while `SCHOOL_BOOKS`) at a school's
    /// starting station that another corporation owns.
    #[test]
    fn a_skillbook_is_asked_at_every_station_of_a_listing_corp() {
        let reg = fixture();
        let mut at: Vec<i64> = orders(&reg, REGION, Some(SKILLBOOK), NOW)
            .iter()
            .map(|o| o.station_id)
            .collect();
        at.sort_unstable();
        let mut want = vec![CORP_STATION];
        if SCHOOL_BOOKS {
            want.push(SCHOOL_STATION);
        }
        assert_eq!(at, want);
        let bpo: Vec<i64> = orders(&reg, REGION, Some(BPO), NOW)
            .iter()
            .map(|o| o.station_id)
            .collect();
        assert_eq!(bpo, [CORP_STATION], "a blueprint at its corp's station");
    }

    /// **GAPS-B** ([`SCHOOL_INVENTION_BOOKS`]): an invention skill no
    /// corporation lists is asked at every school station, priced at
    /// [`INVENTION_BOOK_ASK`] when its `basePrice` is 0; an invention skill
    /// a corporation lists stays at that corporation's stations only, and a
    /// book no invention needs is not sold. Invented ids.
    #[test]
    fn an_invention_book_no_corp_lists_is_asked_at_every_school_station() {
        let (lister, school_corp, owner) = (1_999_601, 1_999_602, 1_999_603);
        let (encryption, science, other, blueprint) = (990_601, 990_602, 990_603, 990_604);
        let (corp_station, school_a, school_b) = (69_000_601, 69_000_602, 69_000_603);
        let station = |station_id, owner_id| Station {
            station_id,
            system_id: 39_000_601,
            region_id: REGION,
            owner_id,
        };
        let row = |category_id, base_price| TypeRow {
            group_id: 99_601,
            category_id,
            base_price,
            meta_group_id: None,
            tech_level: None,
        };
        let types = BTreeMap::from([
            (encryption, row(CATEGORY_SKILL, 0.0)),
            (science, row(CATEGORY_SKILL, 2_000.0)),
            (other, row(CATEGORY_SKILL, 0.0)),
            (blueprint, row(CATEGORY_BLUEPRINT, 0.0)),
        ]);
        let trades = [(lister, science, 1.0)];
        let school = School {
            corp_id: school_corp,
            stations: vec![school_a, school_b, 69_999_999],
        };
        let t = Table::build(
            vec![
                station(corp_station, lister),
                station(school_a, owner),
                station(school_b, owner),
            ],
            &trades,
            types,
            &[],
            &[school],
            &BTreeSet::new(),
            &BTreeSet::from([encryption, science, blueprint]),
        )
        .expect("builds");
        assert_eq!(t.invention_books(), &BTreeSet::from([encryption]));
        let reg = with(t, econ_prices::Table::default());
        let at = |ty| {
            let mut v: Vec<(i64, f64)> = orders(&reg, REGION, Some(ty), NOW)
                .iter()
                .map(|o| (o.station_id, o.price))
                .collect();
            v.sort_by_key(|&(s, _)| s);
            v
        };
        if SCHOOL_INVENTION_BOOKS {
            assert_eq!(
                at(encryption),
                [
                    (school_a, INVENTION_BOOK_ASK),
                    (school_b, INVENTION_BOOK_ASK)
                ]
            );
        } else {
            assert!(at(encryption).is_empty());
        }
        assert_eq!(at(science), [(corp_station, 2_000.0)]);
        assert!(at(other).is_empty() && at(blueprint).is_empty());
    }

    /// **Orphan booster formulas** ([`ORPHAN_BOOSTER_FORMULAS`], issue #80): a
    /// chain formula no corporation lists is asked at every school station at
    /// [`ORPHAN_FORMULA_ASK`] when its `basePrice` is 0; one a corporation
    /// lists is not doubled and keeps its own price; a name that is no
    /// blueprint is not sold. Invented ids.
    #[test]
    fn a_booster_formula_no_corp_lists_is_asked_at_every_school_station() {
        let (lister, school_corp, owner) = (1_999_701, 1_999_702, 1_999_703);
        let (orphan, priced, listed, not_bp) = (990_701, 990_702, 990_703, 990_704);
        let (corp_station, school_a, school_b) = (69_000_701, 69_000_702, 69_000_703);
        let station = |station_id, owner_id| Station {
            station_id,
            system_id: 39_000_701,
            region_id: REGION,
            owner_id,
        };
        let row = |category_id, base_price| TypeRow {
            group_id: 99_701,
            category_id,
            base_price,
            meta_group_id: None,
            tech_level: None,
        };
        let types = BTreeMap::from([
            (orphan, row(CATEGORY_BLUEPRINT, 0.0)),
            (priced, row(CATEGORY_BLUEPRINT, 3_000.0)),
            (listed, row(CATEGORY_BLUEPRINT, 4_000.0)),
            (not_bp, row(CATEGORY_COMMODITY, 5.0)),
        ]);
        let trades = [(lister, listed, 1.0)];
        let school = School {
            corp_id: school_corp,
            stations: vec![school_a, school_b],
        };
        let t = Table::build_with(
            vec![
                station(corp_station, lister),
                station(school_a, owner),
                station(school_b, owner),
            ],
            &trades,
            types,
            &[],
            &[school],
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([orphan, priced, listed, not_bp]),
        )
        .expect("builds");
        assert_eq!(t.orphan_formulas(), &BTreeSet::from([orphan, priced]));
        let reg = with(t, econ_prices::Table::default());
        let at = |ty| {
            let mut v: Vec<(i64, f64)> = orders(&reg, REGION, Some(ty), NOW)
                .iter()
                .map(|o| (o.station_id, o.price))
                .collect();
            v.sort_by_key(|&(s, _)| s);
            v
        };
        assert_eq!(
            at(orphan),
            [
                (school_a, ORPHAN_FORMULA_ASK),
                (school_b, ORPHAN_FORMULA_ASK)
            ]
        );
        assert_eq!(at(priced), [(school_a, 3_000.0), (school_b, 3_000.0)]);
        assert_eq!(at(listed), [(corp_station, 4_000.0)]);
        assert!(at(not_bp).is_empty());
    }

    /// **E5b (R3)**: Tier A prices are `basePrice` times the category's
    /// factor: an invented book and blueprint at 1,000 ask 1,000.00, a
    /// command center 900.00, a trade good 940.00, and a bid pays 1,000.00.
    #[test]
    fn tier_a_prices_are_base_times_the_category_factor() {
        let corp = 1_999_401;
        let cats = [
            (990_401, CATEGORY_SKILL, 1.0, TIER_A_ASK),
            (990_402, CATEGORY_BLUEPRINT, 1.0, TIER_A_ASK),
            (990_403, CATEGORY_COMMAND_CENTER, 1.0, COMMAND_CENTER_ASK),
            (990_404, CATEGORY_COMMODITY, 1.0, TRADE_GOOD_ASK),
            (990_405, CATEGORY_COMMODITY, -1.0, TIER_A_BID),
        ];
        let types = cats
            .iter()
            .map(|&(id, category_id, _, _)| {
                let row = TypeRow {
                    group_id: 99_401,
                    category_id,
                    base_price: 1_000.0,
                    meta_group_id: None,
                    tech_level: None,
                };
                (id, row)
            })
            .collect();
        let trades: Vec<_> = cats.iter().map(|&(id, _, v, _)| (corp, id, v)).collect();
        let station = Station {
            station_id: 69_000_401,
            system_id: 39_000_401,
            region_id: REGION,
            owner_id: corp,
        };
        let t = Table::new(vec![station], &trades, types, &[]).expect("builds");
        let reg = with(t, econ_prices::Table::default());
        for (id, cat, v, factor) in cats {
            if cat == CATEGORY_BLUEPRINT && !NPC_BPOS {
                continue;
            }
            let o = orders(&reg, REGION, Some(id), NOW);
            assert_eq!(o.len(), 1, "{id}: {o:?}");
            assert_eq!(o[0].side == Side::Bid, v < 0.0);
            assert_eq!(
                o[0].price,
                to_cents(1_000.0 * factor, o[0].side).expect("priced")
            );
            assert_eq!(o[0].vol, TIER_A_STOCK);
        }
        assert_eq!(TIER_A_ASK, 1.0, "R3: books and blueprints at exactly base");
    }

    /// **E5b (R4)**, C5 §6 invariant 2: a negative-listed recipe product
    /// gets no NPC bid (fixture type 990008), and no product but a
    /// blueprint is ever asked; a negative-listed module posts nothing.
    #[test]
    fn a_negative_listed_recipe_product_gets_no_bid() {
        let reg = fixture();
        assert!(orders(&reg, REGION, Some(990_008), NOW).is_empty());
        assert!(orders(&reg, REGION, Some(990_009), NOW).is_empty());
        assert!(
            orders(&reg, REGION, Some(990_001), NOW).is_empty(),
            "a ship"
        );
        let goods = orders(&reg, REGION, Some(990_007), NOW);
        let sides: BTreeSet<_> = goods.iter().map(|o| (o.station_id, o.side)).collect();
        let want = BTreeSet::from([(CORP_STATION, Side::Ask), (SCHOOL_STATION, Side::Bid)]);
        assert_eq!(sides, want, "O5-i: a trade good on both sides");
        for seed in 1..40 {
            let (reg, x) = catalogue(seed);
            for r in REGION..REGION + 3 {
                for o in all_orders(&reg, r, None, NOW) {
                    let cat = x.types[&o.type_id].category_id;
                    if x.products.contains(&o.type_id) {
                        assert!(o.side == Side::Ask && cat == CATEGORY_BLUEPRINT, "{o:?}");
                    }
                }
            }
        }
    }

    /// **E5b (R6)**: the hub's loot bid stands iff [`LOOT_BIDS`].
    #[test]
    fn the_hub_loot_bid_stands_only_behind_loot_bids() {
        let reg = fixture();
        let bids = orders(&reg, REGION, Some(LOOT), NOW);
        assert_eq!(bids.len(), usize::from(LOOT_BIDS), "{bids:?}");
        assert!(
            bids.iter()
                .all(|o| o.tier == Tier::B && o.side == Side::Bid)
        );
    }

    /// **CP35, C5 §6 invariant 2**, kept by E5 and E5b: no NPC bid on a
    /// recipe or reaction product, so "buy, build, sell to the NPC" has no
    /// last step; a Tier B bid is on loot or a tag only, and a Tier A bid in
    /// [`BID_CATEGORIES`] only.
    #[test]
    fn invariant_2_no_npc_bid_on_a_buildable_type() {
        for (_, o, p, row) in every_order() {
            if o.side == Side::Bid {
                assert!(!p.as_ref().is_some_and(has_recipe), "{o:?}");
                match o.tier {
                    Tier::A => assert!(bid_category(row.category_id), "{o:?} {row:?}"),
                    Tier::B => assert!(loot_only(&row), "{o:?} {row:?}"),
                }
            }
        }
    }

    /// **CP35, C5 §6 invariant 3**, as E5b leaves it: no Tier B bid meets
    /// an NPC ask in its region; the only types NPCs both ask and bid are
    /// Tier A commodities (O5-i, TQ's haul trade), and never at one station.
    #[test]
    fn invariant_3_no_npc_ask_meets_an_npc_bid() {
        let all = every_order();
        let mut asked: BTreeSet<(u64, i64, i64)> = BTreeSet::new();
        let mut at: BTreeSet<(u64, i64, i64)> = BTreeSet::new();
        for (seed, o, _, _) in all.iter().filter(|(_, o, _, _)| o.side == Side::Ask) {
            asked.insert((*seed, o.region_id, o.type_id));
            at.insert((*seed, o.station_id, o.type_id));
        }
        let mut both = 0;
        for (seed, o, _, row) in all.iter().filter(|(_, o, _, _)| o.side == Side::Bid) {
            assert!(!at.contains(&(*seed, o.station_id, o.type_id)), "{o:?}");
            if asked.contains(&(*seed, o.region_id, o.type_id)) {
                assert_eq!(o.tier, Tier::A, "{o:?}");
                let pair = [CATEGORY_COMMODITY, CATEGORY_STRUCTURE_ITEM];
                assert!(pair.contains(&row.category_id), "{o:?}");
                both += 1;
            }
        }
        assert!(both > 0, "the haul trade is exercised");
    }

    /// The E5b rules walked the plain way, over every station, trade row,
    /// school and hub, with no index: the reference the slot index must
    /// answer like.
    fn linear(reg: &Registry, x: &Inputs, region: i64, only: Option<i64>) -> Vec<NpcOrder> {
        let t = reg.content().get::<Table>();
        let prices = reg.content().get::<econ_prices::Table>();
        let mut stations = x.stations.clone();
        stations.sort_by_key(|s| s.station_id);
        let ix = |id: i64| stations.iter().position(|s| s.station_id == id);
        let wanted = |ty: i64| only.is_none_or(|o| o == ty);
        let side_of = |ty: i64, v: f64| {
            let row = x.types.get(&ty)?;
            let product = x.products.contains(&ty);
            if v > 0.0 && ask_factor(row.category_id).is_some() {
                (!product || row.category_id == CATEGORY_BLUEPRINT).then_some(Side::Ask)
            } else if v < 0.0 && bid_category(row.category_id) {
                (!product).then_some(Side::Bid)
            } else {
                None
            }
        };
        let mut first: BTreeMap<(i64, i64), f64> = BTreeMap::new();
        for &(c, ty, v) in &x.trades {
            first.entry((c, ty)).or_insert(v);
        }
        let trades: Vec<(i64, i64, f64)> = first.into_iter().map(|((c, t), v)| (c, t, v)).collect();
        let mut cands: BTreeMap<(usize, i64, Side), Tier> = BTreeMap::new();
        for (i, s) in stations.iter().enumerate() {
            for &(c, ty, v) in &trades {
                if c != s.owner_id || s.region_id != region || !wanted(ty) {
                    continue;
                }
                if let Some(side) = side_of(ty, v) {
                    cands.entry((i, ty, side)).or_insert(Tier::A);
                }
            }
        }
        for school in x.schools.iter().filter(|_| SCHOOL_BOOKS) {
            for i in school.stations.iter().filter_map(|&s| ix(s)) {
                for &(c, ty, v) in &trades {
                    let skill = x.types.get(&ty).map(|r| r.category_id) == Some(CATEGORY_SKILL);
                    let here = stations[i].region_id == region;
                    let sold = c == school.corp_id && side_of(ty, v) == Some(Side::Ask);
                    if here && wanted(ty) && skill && sold {
                        cands.entry((i, ty, Side::Ask)).or_insert(Tier::A);
                    }
                }
            }
        }
        for i in x.hubs.iter().filter_map(|&h| ix(h)).filter(|_| LOOT_BIDS) {
            for (&ty, row) in &x.types {
                let loot = loot_only(row) && !x.products.contains(&ty);
                if stations[i].region_id == region && wanted(ty) && loot {
                    cands.entry((i, ty, Side::Bid)).or_insert(Tier::B);
                }
            }
        }
        let mut out = Vec::new();
        for ((i, ty, side), tier) in cands {
            let s = stations[i];
            let slot = t.type_slot(ty).expect("a known type");
            let Some(price) = price(reg, t, prices, s.station_id, ty, side, tier) else {
                continue;
            };
            let vol = volume(tier, price);
            out.push(NpcOrder {
                order_id: order_id(i, slot, side),
                corp_id: s.owner_id,
                type_id: ty,
                station_id: s.station_id,
                system_id: s.system_id,
                region_id: region,
                price,
                side,
                tier,
                vol,
                remaining: remaining(reg, &key(s.station_id, ty, side), vol, NOW),
                issue_date: reg.world().last_rollover,
            });
        }
        let asked: BTreeSet<i64> = out
            .iter()
            .filter(|o| o.side == Side::Ask)
            .map(|o| o.type_id)
            .collect();
        out.retain(|o| o.side == Side::Ask || o.tier == Tier::A || !asked.contains(&o.type_id));
        out.sort_by_key(|o| o.order_id);
        out
    }

    /// **E5 (P5)**: the `(region, type) → NPC slots` index answers like the
    /// linear walk, for a whole region and for one type, over the fixture and
    /// 39 generated catalogues.
    #[test]
    fn the_slot_index_answers_like_all_orders_over_the_fixture() {
        let by_id = |mut v: Vec<NpcOrder>| {
            v.sort_by_key(|o| o.order_id);
            v
        };
        let fixture_inputs = Inputs {
            stations: Table::fixture().stations,
            trades: FIXTURE_TRADES.to_vec(),
            types: Table::fixture().types,
            hubs: vec![SCHOOL_STATION],
            schools: vec![School {
                corp_id: 1_999_001,
                stations: vec![SCHOOL_STATION],
            }],
            products: BTreeSet::from([990_008]),
        };
        let mut cases = vec![(fixture(), fixture_inputs)];
        cases.extend((1..40).map(catalogue));
        let mut seen = 0;
        for (reg, x) in &cases {
            for r in REGION..REGION + 3 {
                let all = by_id(all_orders(reg, r, None, NOW));
                assert_eq!(all, linear(reg, x, r, None), "region {r}");
                seen += all.len();
                for &ty in x.types.keys().step_by(7) {
                    let one = by_id(all_orders(reg, r, Some(ty), NOW));
                    assert_eq!(one, linear(reg, x, r, Some(ty)), "{r} {ty}");
                }
            }
        }
        assert!(seen > 100, "the catalogues exercise the index: {seen}");
    }

    /// **E5 (E7's finding)**: a starter type, minted free, gets no NPC bid,
    /// however loot-like its row.
    #[test]
    fn a_starter_type_gets_no_npc_bid() {
        let reg = fixture();
        assert_eq!(
            orders(&reg, REGION, Some(LOOT), NOW).len(),
            1,
            "loot is bid"
        );
        let mut starter = crate::starter_ship::Table::default();
        starter.add_fit(&[(990_001, LOOT)]);
        let mut c = crate::content::Content::fixture();
        c.insert("econ_exchange", Table::fixture());
        c.insert("econ_prices", prices());
        c.insert(crate::starter_ship::TABLE.name, starter);
        let reg = Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(c));
        assert!(orders(&reg, REGION, Some(LOOT), NOW).is_empty());
    }

    /// **E5 (P6)**: a type's stamp moves with its own NPC rows, the rollover
    /// and the trickle period, and not with another type's fill.
    #[test]
    fn a_stamp_moves_with_its_own_rows_only() {
        let mut reg = fixture();
        let book = |reg: &Registry, ty| stamp(reg, REGION, ty, NOW);
        let (skill, loot) = (book(&reg, SKILLBOOK), book(&reg, LOOT));
        take(
            &mut reg,
            (SCHOOL_STATION, SKILLBOOK, Side::Ask),
            1_000,
            1,
            NOW,
        );
        assert_ne!(book(&reg, SKILLBOOK), skill);
        assert_eq!(book(&reg, LOOT), loot, "another type's fill");
        let mult = key(SCHOOL_STATION, LOOT, Side::Bid);
        reg.deltas_mut().put(MULT_KIND, mult, vec!["0.5".into()]);
        assert_ne!(book(&reg, LOOT), loot, "its multiplier");
        let later = stamp(&reg, REGION, LOOT, NOW + TRICKLE_PERIOD);
        assert_ne!(later, book(&reg, LOOT), "a new trickle period");
    }

    /// **CP35, C5 §4.5**: whatever the fills, a multiplier stays in its
    /// band, so `m_bid ≤ BID_MULT.2 < ASK_MULT.0 ≤ m_ask` always.
    #[test]
    fn the_multipliers_stay_in_their_bands() {
        assert!(BID_MULT.2 < ASK_MULT.0);
        let mut g = Gen(7);
        for side in [Side::Ask, Side::Bid] {
            let (lo, mut m, hi) = band(side);
            for _ in 0..500 {
                m = step(m, side, g.pick(&[0.0, 0.05, 0.1, 0.5, 0.8, 1.0, 3.0]));
                assert!((lo..=hi).contains(&m), "{side:?} {m}");
            }
        }
    }

    /// **CP35**: the rollover prunes a refilled row, keeps a part-empty one,
    /// moves a sold-out hub bid's multiplier down, and folds the index.
    #[test]
    fn the_rollover_prunes_refilled_rows_and_moves_a_sold_out_floor() {
        let mut reg = fixture();
        let hub = find(&reg, |o| o.tier == Tier::B);
        let corp = find(&reg, |o| o.tier == Tier::A && o.side == Side::Ask);
        let at = |o: &NpcOrder| (o.station_id, o.type_id, o.side);
        take(&mut reg, at(&hub), hub.vol, hub.vol, NOW - DAY / 2);
        take(&mut reg, at(&corp), corp.vol, 1, NOW - 2 * DAY);
        let (date, station_id, type_id, quantity) = (NOW - 1, hub.station_id, hub.type_id, hub.vol);
        let (price, buyer_id, seller_id) = (hub.price, hub.corp_id, 90_000_001);
        let fill = Fill {
            date,
            station_id,
            type_id,
            quantity,
            price,
            buyer_id,
            seller_id,
        };
        reg.seed_market_fills(vec![fill]);
        let day = crate::rollover::Day {
            day: 3,
            days: 1,
            now: NOW,
        };
        assert_eq!(on_rollover(&mut reg, &day), 1, "only the corp ask refilled");
        assert!(
            reg.deltas()
                .of(KIND)
                .contains_key(&key(hub.station_id, hub.type_id, hub.side))
        );
        let m = multiplier(&reg, hub.station_id, hub.type_id, hub.side);
        assert_eq!(m, step(BID_MULT.1, Side::Bid, 1.0));
        assert_eq!(
            reg.deltas().of(econ_prices::KIND).len(),
            1,
            "one type folded"
        );
    }

    /// No bundle loads no stations, so no NPC order reaches a client; the
    /// shipped hub file parses with its pinned header; a bad bundle row
    /// names its line; a school row reads its stations, and a starter-space
    /// school sells nothing.
    #[test]
    fn no_bundle_no_orders_and_bad_rows_are_refused() {
        let dir = crate::authored::Dir::compiled_only();
        let loaded = (TABLE.load)(&Sources::new(None, &dir)).expect("loads");
        assert_eq!(loaded.downcast_ref::<Table>(), Some(&Table::default()));
        let (rows, skipped) = crate::authored::parse(SHIPPED_HUBS, HUBS_HEADER).expect("header");
        assert!(skipped.is_empty() && rows.iter().all(|r| r.i64(0).is_ok()));
        let e = parse_lines(&["x\t2\t3\t4\t5\t\t0.5\t0.05"], parse_station).expect_err("bad");
        assert!(e.contains("data line 1"), "{e}");
        assert!(parse_lines(&["1\t2\t3\t4\t5\t\t0.5\t0.05"], parse_station).is_ok());
        assert!(parse_lines(&["1\t2\tnan?"], parse_trade).is_err());
        assert!(parse_lines(&["1\t2\t3\t\t1.0\t\t\t\t\t\t\t\t1"], parse_type).is_err());
        let school = |stations: &str, starter: &str| {
            let line = format!("11\t1999001\t1\t2\t\t{stations}\t\t5\t{starter}");
            parse_school(&line.split('\t').collect::<Vec<_>>())
        };
        let read = School {
            corp_id: 1_999_001,
            stations: vec![69_000_001, 69_000_002],
        };
        assert_eq!(school("69000001,69000002", "0"), Some(Some(read)));
        assert_eq!(school("69000001", "1"), Some(None));
        assert_eq!(school("x", "0"), None);
        assert_eq!(school("69000001", "yes"), None);
    }
}
