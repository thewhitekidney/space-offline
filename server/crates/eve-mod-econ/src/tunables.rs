//! **Every number of the owner's economy** that is a choice, in one place.
//! All of them are **[A]**: starting values, not facts. The falsifier for the
//! drop rows and the budget is one week of `[econ-mod] plex` lines outside
//! the target band (an active human earns about 5 PLEX an hour in high
//! security and 15 to 25 in null or wormhole space,
//! `handback/r-nes-econ/report.md` §2.3); for the seed, the price the PLEX
//! market settles at once the stack is gone.
//!
//! Owner decisions (issue #35) that are not numbers and are kept by the code
//! shape: PLEX gives no power; there is no PLEX in LP stores; injectors do
//! not come from the store (they drop here, `injectors`, or come from
//! extraction); a drop is an item in the wreck, so it is lost with the
//! ship; the vault is safe.

/// A bot's drop chance is this times a human's (owner decision, #35).
pub const BOT_PLEX_MULT: f64 = 0.5;

/// What the daily budget does past its end: every chance times this. A
/// soft cap, not a stop, so a good day still pays something. **[A]**
pub const OVER_BUDGET_MULT: f64 = 0.25;

/// PLEX the whole world may mint from drops in one day before
/// [`OVER_BUDGET_MULT`] applies. **[A]**
pub const DAILY_BUDGET: i64 = 3_000;

/// One drop row: an NPC whose kill bounty is at least `min_bounty` leaves
/// `min..=max` PLEX with probability `chance`. Rows are tried from the top,
/// so list them with the highest `min_bounty` first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropRow {
    /// The ledger name: `drop:<source>` in the `nes` ledger.
    pub source: &'static str,
    /// The rows can tell a kind of NPC only by what it pays: there is no
    /// officer or commander flag on an NPC type. **[A]** Falsifier: a rat
    /// the owner calls an officer paying under this, or a plain one over it.
    pub min_bounty: f64,
    pub chance: f64,
    pub min: i64,
    pub max: i64,
}

/// The drop rows, **[A]** in every number (`r-nes-econ` §2.3 had 40% and
/// 25 to 100 for an officer).
pub const DROPS: &[DropRow] = &[
    DropRow {
        source: "elite",
        min_bounty: 5_000_000.0,
        chance: 0.40,
        min: 25,
        max: 100,
    },
    DropRow {
        source: "heavy",
        min_bounty: 1_000_000.0,
        chance: 0.03,
        min: 1,
        max: 4,
    },
];

/// The row an NPC paying `bounty` rolls, if any.
pub fn row_for(bounty: f64) -> Option<&'static DropRow> {
    DROPS.iter().find(|r| bounty >= r.min_bounty)
}

/// **The PLEX seed**: one stack, listed once at the station corporation's
/// name, never restocked. Size and price are the owner's to set **[A]**;
/// falsifier for the price: the market's own price once the stack is gone.
pub const SEED_UNITS: i64 = 500;
/// ISK a unit, the reference price.
pub const SEED_PRICE: f64 = 3_000_000.0;
/// How long the ask rests, days. Long, so it does not lapse into the
/// corporation's hangar before anyone has had a chance to buy.
pub const SEED_DAYS: i64 = 3_650;

// ---- injectors (lane l4-injectors, issue #30) ----

/// A bot's injector chance is this times a human's (as for PLEX). **[A]**
pub const BOT_INJECTOR_MULT: f64 = 0.5;

/// Skill points the world may mint from injector drops in one day before
/// [`OVER_BUDGET_MULT`] applies: ten large injectors. **[A]** Falsifier: the
/// `sptrade` ledger's `drop:*` rows outrunning a week of the owner's
/// "about one an active hour".
pub const DAILY_SP_BUDGET: i64 = 5_000_000;

/// One injector drop row: an NPC whose kill bounty is at least `min_bounty`
/// leaves one large injector with probability `large`, else one small with
/// probability `small`. Rows are tried from the top like [`DropRow`]s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InjectorRow {
    /// The ledger name: `drop:<source>` in the `sptrade` ledger.
    pub source: &'static str,
    pub min_bounty: f64,
    pub large: f64,
    pub small: f64,
}

/// The injector rows, **[A]** in every number. The owner's target is about
/// one large injector for each active hour (`plan-deep-queue`, #30). The
/// arithmetic behind these rows assumes an hour of ratting kills about 3
/// elite and 20 heavy NPCs, a small one counting a fifth of a large:
/// 3 * (0.15 + 0.85 * 0.30 / 5) + 20 * (0.01 + 0.99 * 0.05 / 5) is about 1.0.
/// Falsifier: the real kill mix per hour (the soak's kill count) differing
/// from that by a factor of two; then only these numbers move.
pub const INJECTOR_DROPS: &[InjectorRow] = &[
    InjectorRow {
        source: "elite",
        min_bounty: 5_000_000.0,
        large: 0.15,
        small: 0.30,
    },
    InjectorRow {
        source: "heavy",
        min_bounty: 1_000_000.0,
        large: 0.01,
        small: 0.05,
    },
];

/// The injector row an NPC paying `bounty` rolls, if any.
pub fn injector_row_for(bounty: f64) -> Option<&'static InjectorRow> {
    INJECTOR_DROPS.iter().find(|r| bounty >= r.min_bounty)
}

// ---- Needlejack filaments (lane needlejack, issue #88) ----

/// A bot's filament chance is this times a human's (as for PLEX). **[A]**
pub const BOT_FILAMENT_MULT: f64 = 0.5;

/// Filaments the whole world may drop in one day before
/// [`OVER_BUDGET_MULT`] applies. **[A]** Falsifier: a week of `fday` rows
/// (the `econmod` delta) far from the owner's wish of a roaming gang a
/// session: more than a few dozen a day means players hoard, none means
/// nobody roams.
pub const DAILY_FILAMENTS: i64 = 24;

/// One filament drop row: an NPC whose kill bounty is at least `min_bounty`
/// leaves one filament with probability `chance`. Rows are tried from the top
/// like [`DropRow`]s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilamentRow {
    pub source: &'static str,
    pub min_bounty: f64,
    pub chance: f64,
}

/// The filament rows, **[A]** in every number: an elite is worth one in
/// fifteen kills, a heavy one in a hundred and fifty. The bounty threshold is
/// the same stand-in the PLEX rows use (there is no officer flag on an NPC
/// type). Falsifier: the soak's `[econ-mod]` count of filaments per elite
/// kill, outside a factor of two of this.
pub const FILAMENT_DROPS: &[FilamentRow] = &[
    FilamentRow {
        source: "elite",
        min_bounty: 5_000_000.0,
        chance: 0.07,
    },
    FilamentRow {
        source: "heavy",
        min_bounty: 1_000_000.0,
        chance: 0.007,
    },
];

/// The filament row an NPC paying `bounty` rolls, if any.
pub fn filament_row_for(bounty: f64) -> Option<&'static FilamentRow> {
    FILAMENT_DROPS.iter().find(|r| bounty >= r.min_bounty)
}

// ---- geodes (lane l6a-geodes, issue #32) ----

/// The chance a completed mining cut leaves a geode in the hold. **[A]**
/// (`r-nes-econ` §2.6 A: 0.2%). Falsifier: a miner who clears a belt of a
/// hundred cycles an hour and sees a geode more than once a few hours, or
/// less than once a few days.
pub const GEODE_CHANCE: f64 = 0.002;

/// The chance times this on a cut that crit (`MinedEvent::crit > 0`). **[A]**
pub const GEODE_CRIT_MULT: f64 = 10.0;

/// A bot's geode chance is this times a human's (as for PLEX). **[A]**
pub const BOT_GEODE_MULT: f64 = 0.5;

/// Which tier a geode that dropped is, as relative weights of tiers 1, 2 and
/// 3. A tier a world's bundle has no crate type for is passed over (the next
/// one down is used). **[A]**
pub const TIER_WEIGHTS: [f64; 3] = [80.0, 17.0, 3.0];

/// The same on a crit cut: a crit finds a better geode. **[A]**
pub const TIER_WEIGHTS_CRIT: [f64; 3] = [50.0, 35.0, 15.0];

/// Prizes a geode of tier 1, 2 and 3 holds. **[A]**
pub const PRIZES_BY_TIER: [usize; 3] = [1, 2, 3];

/// Quantities of tier 1, 2 and 3 are times this. **[A]**
pub const QTY_BY_TIER: [f64; 3] = [1.0, 1.5, 2.5];

/// Ore units one cycle yields, for "the ore of N cycles". **[A]** Falsifier:
/// the soak's mean `OnOreMined` amount.
pub const CYCLE_UNITS: i64 = 80;

/// A geode's mineral prize is the materials of this many to that many
/// cycles of ore, refined at [`REFINE_RATE`]. **[A]**
pub const CYCLES: (u64, u64) = (5, 20);

/// The fraction of an ore's materials a geode pays, the NPC station rate
/// (`reprocess::NPC_STATION_EFFICIENCY`), so a geode never beats mining the
/// ore and refining it. **[A]**
pub const REFINE_RATE: f64 = 0.5;

/// The "rare" prize's ore is drawn from the dearest this share of the ores
/// the reprocessing table holds, by price. **[A]**
pub const RARE_SHARE: f64 = 0.15;

/// PLEX a geode prize holds (times the tier's [`QTY_BY_TIER`]). **[A]**
pub const GEODE_PLEX: (u64, u64) = (5, 25);

/// Groups of the hulls the SKIN prize is drawn for: the mining barges and the
/// exhumers. **[A]** Falsifier: the Procurer's `groupID` in the bundle's
/// typedata differing from the first, the Skiff's from the second.
pub const SKIN_HULL_GROUPS: [i64; 2] = [463, 543];

/// What one geode prize is. The names are the `role` column of
/// `content/econ/geode-loot.tsv`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The materials of the ore of some cycles.
    Minerals,
    /// The same, of a dear ore.
    Rare,
    /// PLEX paid to the vault.
    Plex,
    InjectorSmall,
    InjectorLarge,
    /// A SKIN licence for a mining hull.
    Skin,
    /// A mining crystal.
    Crystal,
}

impl Role {
    pub const ALL: [Role; 7] = [
        Role::Minerals,
        Role::Rare,
        Role::Plex,
        Role::InjectorSmall,
        Role::InjectorLarge,
        Role::Skin,
        Role::Crystal,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Role::Minerals => "minerals",
            Role::Rare => "rare",
            Role::Plex => "plex",
            Role::InjectorSmall => "injector_small",
            Role::InjectorLarge => "injector_large",
            Role::Skin => "skin",
            Role::Crystal => "crystal",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.name() == s)
    }
}

// ---- booster blueprint copies (lane b3-sources, issue #80) ----

/// A bot's booster-blueprint chance is this times a human's (as for PLEX). **[A]**
pub const BOT_BPC_MULT: f64 = 0.5;

/// Booster blueprint copies the whole world may drop in one day before
/// [`OVER_BUDGET_MULT`] applies. **[A]** Falsifier: a week of `bday` rows (the
/// `econmod` delta) with more than a handful a day (the drug trade is flooded)
/// or none (nobody ever builds a booster).
pub const DAILY_BPCS: i64 = 6;

/// One booster-blueprint drop row: an NPC whose kill bounty is at least
/// `min_bounty` leaves one copy with probability `chance`, with `min` to `max`
/// runs. Rows are tried from the top like [`DropRow`]s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BpcRow {
    pub source: &'static str,
    pub min_bounty: f64,
    pub chance: f64,
    pub min: i64,
    pub max: i64,
}

/// The booster-blueprint rows, **[A]** in every number: an elite is worth one
/// in thirty kills, a heavy one in three hundred, with a short licence. The
/// bounty threshold is the same stand-in the PLEX rows use. Falsifier: the
/// soak's `[econ-mod]` count of copies per elite kill outside a factor of two
/// of this, or a booster price that never leaves the formula's cost.
pub const BPC_DROPS: &[BpcRow] = &[
    BpcRow {
        source: "elite",
        min_bounty: 5_000_000.0,
        chance: 0.035,
        min: 5,
        max: 20,
    },
    BpcRow {
        source: "heavy",
        min_bounty: 1_000_000.0,
        chance: 0.0035,
        min: 2,
        max: 10,
    },
];

/// The booster-blueprint row an NPC paying `bounty` rolls, if any.
pub fn bpc_row_for(bounty: f64) -> Option<&'static BpcRow> {
    BPC_DROPS.iter().find(|r| bounty >= r.min_bounty)
}

// ---- motherlodes, ambushes and rich sites (lane l6b-mining-events, issue #33) ----
//
// `r-nes-econ` §2.6 C-E. Every number is **[A]**; the falsifier for the lot
// is the owner's "the announcement draws bots and humans" check: an event
// nobody turns up for, or a belt that is a queue, moves the rates here and
// nothing else.

/// How often the scheduler starts a motherlode in each region that has an
/// active mining system. **[A]**
pub const ML_INTERVAL_S: i64 = 2 * 3600;

/// How long one lasts. **[A]**
pub const ML_DURATION_S: i64 = 45 * 60;

/// The first event of a fresh world waits this long, so a restart does not
/// open one at once. **[A]**
pub const ML_FIRST_DELAY_S: i64 = 15 * 60;

/// A system is active for the scheduler when a belt rock in it was cut within
/// this long. **[A]**
pub const ML_ACTIVE_WINDOW_S: i64 = 3600;

/// The systems within this many gates of a motherlode hear it. **[A]**
pub const ML_ECHO_JUMPS: u32 = 3;

/// They hear it this long after the system itself. **[A]**
pub const ML_ECHO_DELAY_S: i64 = 5 * 60;

/// Extra ore a cut yields in a motherlode, as a fraction of its normal yield,
/// minted into the hold. The spec says "rocks x3 quantity"; rocks are left
/// alone (their sizes are persisted and pushed to every client), and a belt
/// pays three times what it would by this. **[A]** Falsifier: miners saying
/// the rocks look the same size, or the minted ore (`motherlode:ore:*` rows)
/// swamping the ore sold in a day.
pub const ML_BONUS_YIELD: f64 = 2.0;

/// The same in a rich site (x2 what it would pay). **[A]**
pub const RICH_BONUS_YIELD: f64 = 1.0;

/// The geode chance times this in a motherlode and in a rich site. **[A]**
pub const ML_GEODE_MULT: f64 = 5.0;
pub const RICH_GEODE_MULT: f64 = 3.0;

/// A site drawn by a `population.tsv` row whose key contains this is a rich
/// site. **[A]**
pub const RICH_MARK: &str = "rich";

/// A rich site is being worked, for ambushes, when a rock in it was cut this
/// recently. **[A]**
pub const RICH_ACTIVE_S: i64 = 10 * 60;

/// The first ambush of an event or a worked site comes this long after it
/// begins. **[A]**
pub const AMBUSH_FIRST_S: i64 = 10 * 60;

/// Seconds between ambushes by band: high, low, null and wormhole. **[A]**
pub const AMBUSH_EVERY_S: [i64; 3] = [15 * 60, 10 * 60, 7 * 60];

/// The ambush leader's wreck: PLEX with this chance and this many units,
/// a small injector, a large injector, a geode. **[A]**
pub const AMBUSH_PLEX: (f64, u64, u64) = (0.25, 5, 20);
pub const AMBUSH_SMALL: f64 = 0.05;
pub const AMBUSH_LARGE: f64 = 0.01;
pub const AMBUSH_GEODE: f64 = 0.30;
