//! **One difficulty setting** (DOC-8; R-DOCTRINE 10, R-RELATIONS 5,
//! R-FLUIDECON G5): `eve-bots --difficulty {easy|normal|hard|brutal | 0..4}`
//! (default normal) and `tools/soak.sh --difficulty N`.
//!
//! It scales **bot behaviour only**: what a leader sees ([`SenseKnobs`]),
//! how fast and how accurately it chooses ([`LeadKnobs`]), how much it
//! commits and how well the fleet obeys ([`TheatreKnobs`]), how corps hold
//! grudges ([`Temper`]) and how hungry the traders are ([`TraderKnobs`]).
//! The server, physics, NPCs, CONCORD and prices are the same at every
//! level, and no level gives a bot ISK, a ship or sight the client would
//! not give.
//!
//! **Where the numbers live.** The built-in columns are the five structs'
//! own consts (`Temper::EASY`.., `SenseKnobs::EASY`.., ...), and `normal`
//! is today's constants by test. `content/bots/difficulty.tsv`
//! (`level, constant, value, why`) is the committed copy of the same
//! numbers that a mod may edit: [`Difficulty::open`] reads it and each row
//! overrides one constant at one level; a row naming no constant, a bad
//! level or a bad value is skipped and logged, and a missing row leaves the
//! built-in value. Level 0 is "peaceful": normal, with leaders that never
//! switch doctrine ([`LeadKnobs::frozen`]) and a reserve of 0.6.

use crate::counter::CounterKnobs;
use crate::doctrine_lead::LeadKnobs;
use crate::doctrine_sense::SenseKnobs;
use crate::doctrine_theatre::TheatreKnobs;
use crate::relations::Temper;
use crate::roam_gang::GangKnobs;
use crate::speculate::SpecKnobs;
use crate::trader::{Costs, MIN_PROFIT, TRADE_MARGIN};
use eve_server::authored;

/// The table's path under the authored directory.
pub const REL: &str = "bots/difficulty.tsv";
/// Its header, `schema.md` §4.
pub const HEADER: &str = "level\tconstant\tvalue\twhy";
const SHIPPED: &str = include_str!("../../../../content/bots/difficulty.tsv");

/// The levels: 0 peaceful, 1 easy, 2 normal, 3 hard, 4 brutal.
pub const LEVELS: [u8; 5] = [0, 1, 2, 3, 4];
/// The default level.
pub const NORMAL_LEVEL: u8 = 2;
/// The highest level.
pub const MAX_LEVEL: u8 = 4;

/// **The traders' appetite** [A]: how thin a margin and how small a profit
/// a trader still takes a trip for. Normal is the trader's own consts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraderKnobs {
    /// The fraction of a deal's spend a trip must earn
    /// ([`crate::trader::Costs::margin`]).
    pub margin: f64,
    /// The least ISK a trip must earn ([`crate::trader::Costs::min_profit`]).
    pub min_profit: f64,
    /// FE-TRADE: the most of its worth a trader's speculative holdings may
    /// cost ([`crate::speculate::SpecKnobs::wallet_share`]).
    pub spec_wallet: f64,
    /// FE-TRADE: days of the region's volume one holding may be
    /// ([`crate::speculate::SpecKnobs::share_days`]).
    pub spec_share: f64,
    /// FE-TRADE: the extra margin a cross-region deal must clear
    /// ([`crate::speculate::SpecKnobs::region_risk`]).
    pub region_risk: f64,
}

impl TraderKnobs {
    pub const EASY: TraderKnobs = TraderKnobs {
        margin: 0.02,
        min_profit: 40_000.0,
        spec_wallet: 0.15,
        spec_share: 2.0,
        region_risk: 0.05,
    };
    pub const NORMAL: TraderKnobs = TraderKnobs {
        margin: TRADE_MARGIN,
        min_profit: MIN_PROFIT,
        spec_wallet: SpecKnobs::NORMAL.wallet_share,
        spec_share: SpecKnobs::NORMAL.share_days,
        region_risk: SpecKnobs::NORMAL.region_risk,
    };
    pub const HARD: TraderKnobs = TraderKnobs {
        margin: 0.005,
        min_profit: 10_000.0,
        spec_wallet: 0.40,
        spec_share: 4.0,
        region_risk: 0.02,
    };
    pub const BRUTAL: TraderKnobs = TraderKnobs {
        margin: 0.0025,
        min_profit: 5_000.0,
        spec_wallet: 0.50,
        spec_share: 5.0,
        region_risk: 0.01,
    };

    /// 1 easy .. 4 brutal; 0 and 2 are normal.
    pub fn at(level: u8) -> TraderKnobs {
        match level {
            1 => Self::EASY,
            3 => Self::HARD,
            4.. => Self::BRUTAL,
            _ => Self::NORMAL,
        }
    }

    /// The speculator's and cross-region trader's knobs at this appetite.
    pub fn spec(&self) -> SpecKnobs {
        SpecKnobs::with(self.spec_wallet, self.spec_share, self.region_risk)
    }

    /// The trader's costs at this appetite: the defaults with the margin and
    /// the least profit replaced.
    pub fn costs(&self) -> Costs {
        Costs {
            margin: self.margin,
            min_profit: self.min_profit,
            ..Costs::default()
        }
    }
}

impl Default for TraderKnobs {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// **Every knob of one level**, handed to the subsystems.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    pub temper: Temper,
    pub sense: SenseKnobs,
    pub lead: LeadKnobs,
    pub theatre: TheatreKnobs,
    pub trader: TraderKnobs,
    pub gang: GangKnobs,
    /// BP-COUNTER: how well the fleets keep the operational counters.
    pub counter: CounterKnobs,
}

impl Knobs {
    /// The built-in column of `level`: the structs' own consts.
    pub fn builtin(level: u8) -> Knobs {
        Knobs {
            temper: Temper::at(level),
            sense: SenseKnobs::at(level),
            lead: LeadKnobs::at(level),
            theatre: TheatreKnobs::at(level),
            trader: TraderKnobs::at(level),
            gang: GangKnobs::at(level),
            counter: CounterKnobs::at(level),
        }
    }
}

impl Default for Knobs {
    fn default() -> Self {
        Self::builtin(NORMAL_LEVEL)
    }
}

/// One named constant: how to read it and write it, and which way is harder.
pub struct Knob {
    /// The name in `difficulty.tsv`.
    pub name: &'static str,
    /// Does a higher value make the bots harder (else a lower one does)?
    pub harder_up: bool,
    pub get: fn(&Knobs) -> f64,
    pub set: fn(&mut Knobs, f64),
}

/// A real-valued knob at a path inside [`Knobs`].
macro_rules! real {
    ($name:literal, $up:literal, $($p:ident).+) => {
        Knob {
            name: $name,
            harder_up: $up,
            get: |k| k.$($p).+,
            set: |k, v| k.$($p).+ = v,
        }
    };
}

/// A whole-number knob.
macro_rules! whole {
    ($name:literal, $up:literal, $($p:ident).+, $ty:ty) => {
        Knob {
            name: $name,
            harder_up: $up,
            get: |k| k.$($p).+ as f64,
            set: |k, v| k.$($p).+ = v.round().max(0.0) as $ty,
        }
    };
}

/// **The table of constants**: every number a level changes, each once.
pub const KNOBS: &[Knob] = &[
    // observation (R-DOCTRINE 10)
    real!("OBS_NOISE", false, sense.obs_noise),
    real!("INTEL_SHARE", true, sense.intel_share),
    whole!("SCAN_EVERY_S", false, sense.scan_every_s, u64),
    whole!("ENEMY_MEMORY", true, sense.enemy_memory, usize),
    real!("ENEMY_HALF_LIFE_DAYS", true, sense.half_life_days),
    real!("BLIND_CONF", true, sense.blind_conf),
    // reaction and counter accuracy
    whole!("TACTICAL_DELAY_S", false, theatre.tactical_delay_s, u64),
    real!("DOCTRINE_MIN_DAYS", false, lead.min_days),
    real!("DOCTRINE_COOLDOWN_DAYS", false, lead.cooldown_days),
    whole!("SWITCH_CONFIRMATIONS", false, lead.confirmations, u32),
    real!("SWITCH_FORCE_DAYS", false, lead.force_days),
    real!("W_COUNTER", true, lead.w_counter),
    real!("COUNTER_PICK", true, lead.counter_pick),
    real!("COUNTER_RISK", true, lead.counter_risk),
    real!("STOCK_DAYS", true, lead.stock_days),
    real!("SRP_SHARE", false, lead.srp_share),
    real!("STOCK_WALLET_SHARE", true, lead.stock_wallet),
    Knob {
        name: "FROZEN",
        harder_up: true,
        get: |k| f64::from(u8::from(k.lead.frozen)),
        set: |k, v| k.lead.frozen = v >= 0.5,
    },
    // commitment and coordination
    real!("COMMIT_STEP", true, theatre.commit_step),
    real!("RESERVE", false, theatre.reserve),
    real!("ENGAGE_ER", false, theatre.engage_er),
    real!("DISENGAGE_ER", false, theatre.disengage_er),
    whole!("MAX_THEATRES", true, theatre.max_theatres, usize),
    real!("P_OBEY", true, theatre.p_obey),
    real!("ARRIVE_SPREAD_S", false, theatre.arrive_spread_s),
    whole!("FC_CALL_S", false, theatre.fc_call_s, u64),
    real!("ESCALATE_ER", false, theatre.escalate_er),
    // relations (R-RELATIONS 5)
    real!("GRUDGE_RATE", true, temper.grudge_rate),
    real!("MEMORY", true, temper.memory),
    real!("HUNT_BIAS", true, temper.hunt_bias),
    real!("COORDINATION", true, temper.coordination),
    real!("HUMAN_PRIOR", false, temper.human_prior),
    real!("GANK_TOLERANCE", false, temper.gank_tolerance),
    real!("FLIP_MIN_DAYS", false, temper.flip_min_days),
    real!("BETRAY_MEMORY_DAYS", true, temper.betray_memory_days),
    // the traders' appetite
    real!("TRADE_MARGIN", false, trader.margin),
    real!("TRADE_MIN_PROFIT", false, trader.min_profit),
    real!("SPEC_WALLET", true, trader.spec_wallet),
    real!("SPEC_SHARE_DAYS", true, trader.spec_share),
    real!("REGION_RISK", false, trader.region_risk),
    // the roaming gangs (ROAM-GANG)
    real!("GANG_AGGRESSION", true, gang.aggression),
    whole!("GANG_SCOUTS", true, gang.scouts, usize),
    whole!("GANG_RADIUS", true, gang.radius, u32),
    real!("GANG_REQUIRED_ODDS", false, gang.required_odds),
    // the operational counters (BP-COUNTER)
    real!("KIT_DISCIPLINE", true, counter.discipline),
    real!("COUNTER_READ", true, lead.counter_read),
];

/// The knob called `name`.
pub fn knob(name: &str) -> Option<&'static Knob> {
    KNOBS.iter().find(|k| k.name == name)
}

/// The parsed `difficulty.tsv`: overrides in file order (a later row wins).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub rows: Vec<(u8, &'static Knob, f64)>,
}

impl PartialEq for Knob {
    fn eq(&self, o: &Knob) -> bool {
        self.name == o.name
    }
}

impl std::fmt::Debug for Knob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

impl Table {
    /// The rows of an authored file's text. A header other than [`HEADER`]
    /// is an error; a row that does not parse is skipped and logged.
    pub fn parse(text: &str) -> Result<Table, String> {
        let (rows, _) = authored::parse(text, HEADER)?;
        Ok(Table::from_rows(&rows))
    }

    /// `./content/bots/difficulty.tsv` when there, else the shipped copy.
    pub fn load(dir: &authored::Dir) -> Result<Table, String> {
        let file = dir.load(REL, SHIPPED, HEADER)?;
        Ok(Table::from_rows(&file.rows))
    }

    fn from_rows(rows: &[authored::Row]) -> Table {
        let mut t = Table::default();
        for row in rows {
            match Table::row(row) {
                Ok(r) => t.rows.push(r),
                Err(e) => println!("[authored] {REL}: line {}: skipped: {e}", row.line),
            }
        }
        t
    }

    fn row(r: &authored::Row) -> Result<(u8, &'static Knob, f64), String> {
        let level = r.i64(0)?;
        let level = u8::try_from(level)
            .ok()
            .filter(|l| *l <= MAX_LEVEL)
            .ok_or_else(|| format!("level {level} is not 0..={MAX_LEVEL}"))?;
        let knob = knob(r.field(1)).ok_or_else(|| format!("no constant {:?}", r.field(1)))?;
        let value = r.f64(2)?;
        if value.is_nan() {
            return Err("value is NaN".to_owned());
        }
        Ok((level, knob, value))
    }
}

/// **The run's difficulty**: a level and the table that may override it.
#[derive(Debug, Clone, PartialEq)]
pub struct Difficulty {
    level: u8,
    table: Table,
}

impl Default for Difficulty {
    fn default() -> Self {
        Difficulty::built_in(NORMAL_LEVEL)
    }
}

impl Difficulty {
    /// `level` with no overrides: the built-in columns.
    pub fn built_in(level: u8) -> Difficulty {
        Difficulty {
            level: level.min(MAX_LEVEL),
            table: Table::default(),
        }
    }

    /// `level` with `table`'s overrides.
    pub fn with_table(level: u8, table: Table) -> Difficulty {
        Difficulty {
            level: level.min(MAX_LEVEL),
            table,
        }
    }

    /// `level` with the table from `dir` (or the shipped copy).
    pub fn open(level: u8, dir: &authored::Dir) -> Result<Difficulty, String> {
        Ok(Difficulty::with_table(level, Table::load(dir)?))
    }

    /// The `--difficulty` value: `easy`, `normal`, `hard`, `brutal`,
    /// `peaceful` or a level `0..=4`.
    pub fn parse_level(word: &str) -> Result<u8, String> {
        match word {
            "peaceful" => Ok(0),
            "easy" => Ok(1),
            "normal" => Ok(NORMAL_LEVEL),
            "hard" => Ok(3),
            "brutal" => Ok(MAX_LEVEL),
            w => match w.parse::<u8>() {
                Ok(n) if n <= MAX_LEVEL => Ok(n),
                _ => Err(format!(
                    "--difficulty {w:?}: want easy, normal, hard, brutal or 0..={MAX_LEVEL}"
                )),
            },
        }
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    /// The level's name, for the startup line.
    pub fn name(&self) -> &'static str {
        match self.level {
            0 => "peaceful",
            1 => "easy",
            2 => "normal",
            3 => "hard",
            _ => "brutal",
        }
    }

    /// **Every knob at this level**: the built-in column, then the table's
    /// rows for this level in file order.
    pub fn knobs(&self) -> Knobs {
        let mut k = Knobs::builtin(self.level);
        for (level, knob, value) in &self.table.rows {
            if *level == self.level {
                (knob.set)(&mut k, *value);
            }
        }
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(level: u8) -> Knobs {
        Difficulty::built_in(level).knobs()
    }

    /// Why each constant is what it is, for the generated table (the
    /// `[A]` and its falsifier; R-DOCTRINE 10, R-RELATIONS 5).
    fn why(name: &str) -> &'static str {
        match name {
            "OBS_NOISE" => {
                "[A] only the direction is physical; falsified by easy leaders still counting enemies to within 10%"
            }
            "INTEL_SHARE" => {
                "[A] falsified by an easy leader whose picture is as full as a brutal one"
            }
            "SCAN_EVERY_S" => "[A] falsified by d-scan calls crowding human turns at hard",
            "ENEMY_MEMORY" => "[A] falsified by a leader that forgets a recurring enemy at normal",
            "ENEMY_HALF_LIFE_DAYS" => {
                "[A] falsified by a stale enemy still steering a counter after a game month"
            }
            "BLIND_CONF" => "[A] falsified by a leader with no scout planning as if it had one",
            "TACTICAL_DELAY_S" => {
                "[A] falsified by easy fleets answering a changed picture as fast as brutal ones"
            }
            "DOCTRINE_MIN_DAYS" => {
                "[A] falsified by a doctrine switch soak where hard flips more than daily"
            }
            "DOCTRINE_COOLDOWN_DAYS" => "[A] falsified as DOCTRINE_MIN_DAYS is",
            "SWITCH_CONFIRMATIONS" => "[A] falsified by easy leaders switching on one noisy day",
            "SWITCH_FORCE_DAYS" => {
                "[A] falsified by a counter that stays unanswered past this at normal"
            }
            "W_COUNTER" => "[A] falsified by taste never losing to a counter at brutal",
            "COUNTER_PICK" => "[A] falsified by easy leaders always taking the best counter",
            "COUNTER_RISK" => {
                "[A] falsified by brutal leaders picking a doctrine a single enemy fit beats"
            }
            "STOCK_DAYS" => {
                "[A] falsified by a corp out of doctrine ships mid-fight at normal, or hoarding at easy"
            }
            "FROZEN" => {
                "[A] level 0 only: the owner wants a quiet world with one doctrine per leader"
            }
            "COMMIT_STEP" => {
                "[A] falsified by easy fleets committing as much per step as brutal ones"
            }
            "RESERVE" => {
                "[A] falsified by a brutal fleet that still holds a third back, or a peaceful one that commits more than 40%"
            }
            "ENGAGE_ER" => "[A] falsified by easy leaders starting fights they lose",
            "DISENGAGE_ER" => "[A] falsified by brutal fleets that run from an even fight",
            "MAX_THEATRES" => "[A] falsified by normal leaders fighting in two systems at once",
            "P_OBEY" => "[A] falsified by easy fleets that all follow one order",
            "ARRIVE_SPREAD_S" => "[A] falsified by brutal fleets that still arrive a minute apart",
            "FC_CALL_S" => {
                "[A] falsified by primary calls changing faster than the client can lock"
            }
            "ESCALATE_ER" => "[A] falsified by easy leaders sending more ships into a lost fight",
            "GRUDGE_RATE" => "[A] falsified by easy corps staying at war after a single loss",
            "MEMORY" => "[A] falsified by brutal corps forgiving inside a game week",
            "HUNT_BIAS" => "[A] falsified by easy bots hunting a corp they rate only Wary",
            "COORDINATION" => "[A] falsified by a coalition at brutal taking a day to answer",
            "HUMAN_PRIOR" => {
                "[A] falsified by easy bots attacking an unlisted human, or brutal ones befriending one"
            }
            "GANK_TOLERANCE" => "[A] falsified by easy bots condemning a 2:1 fight",
            "FLIP_MIN_DAYS" => "[A] falsified by stances flapping at hard, or never moving at easy",
            "BETRAY_MEMORY_DAYS" => {
                "[A] falsified by a betrayer re-trusted inside a game month at hard"
            }
            "TRADE_MARGIN" => {
                "[A] falsified by hard traders with a negative ledger or easy ones idle while spreads exceed 5%"
            }
            "TRADE_MIN_PROFIT" => "[A] falsified as TRADE_MARGIN is",
            "SPEC_WALLET" => {
                "[A] falsified by a brutal speculator ending 40% under its start, or an easy one that never moves the tape's spec row"
            }
            "SPEC_SHARE_DAYS" => {
                "[A] falsified by a speculator holding a type's whole ask side for a day"
            }
            "REGION_RISK" => {
                "[A] falsified by arbitrage legs aborting on arrival over a third of the time at brutal, or a spread over twice the round trip that no easy trader closes"
            }
            "GANG_AGGRESSION" => {
                "[A] falsified by easy gangs that take a fight at long odds, or brutal ones that break off at the first loss"
            }
            "GANG_SCOUTS" => {
                "[A] falsified by a normal gang that needs minutes to find a pilot two jumps from its staging"
            }
            "GANG_RADIUS" => {
                "[A] falsified by easy gangs that find a human three jumps away, or brutal ones that miss one at five"
            }
            "GANG_REQUIRED_ODDS" => {
                "[A] falsified by gangs that engage at even odds at easy, or that never engage at brutal"
            }
            "KIT_DISCIPLINE" => {
                "[A] falsified by easy bots that answer a bomber or a drive as fast as brutal ones, or by level 0 bots that still kill the tackler first"
            }
            "COUNTER_READ" => {
                "[A] falsified by an easy leader whose counter score moves as much on a seen smartbomb as a brutal one's, or by a level 0 score that changes at all"
            }
            _ => "[A] unexplained",
        }
    }

    /// The shipped table, rebuilt from the built-in columns:
    /// `tools/dev.sh cargo test -p eve-bot dump_the_table -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_the_table() {
        println!(
            "# The bots' difficulty (eve-bots --difficulty; eve-bot/src/difficulty.rs, DOC-8;"
        );
        println!("# header in docs/content/schema.md §4). Read at eve-bots startup from");
        println!("# ./content/bots/difficulty.tsv when it is there, else this compiled-in copy.");
        println!("# One row, one constant at one level; edit the value and restart the bots.");
        println!("# Level 0 peaceful, 1 easy, 2 normal, 3 hard, 4 brutal. It scales bot");
        println!("# behaviour only; the server, NPCs, CONCORD and prices do not change.");
        println!("{HEADER}");
        for level in LEVELS {
            let k = at(level);
            for knob in KNOBS {
                println!(
                    "{level}\t{}\t{}\t{}",
                    knob.name,
                    (knob.get)(&k),
                    why(knob.name)
                );
            }
        }
    }

    #[test]
    fn knobs_are_monotone_in_level() {
        for knob in KNOBS {
            let v: Vec<f64> = (1..=MAX_LEVEL).map(|l| (knob.get)(&at(l))).collect();
            for w in v.windows(2) {
                let ok = if knob.harder_up {
                    w[0] <= w[1]
                } else {
                    w[0] >= w[1]
                };
                assert!(ok, "{} is not monotone: {v:?}", knob.name);
            }
            if knob.name != "FROZEN" {
                assert_ne!(v[0], v[3], "{} does not change with the level", knob.name);
            }
        }
    }

    #[test]
    fn normal_is_the_base() {
        let k = at(NORMAL_LEVEL);
        assert_eq!(k.temper, Temper::NORMAL);
        assert_eq!(k.sense, SenseKnobs::NORMAL);
        assert_eq!(k.lead, LeadKnobs::NORMAL);
        assert_eq!(k.theatre, TheatreKnobs::NORMAL);
        assert_eq!(k.trader, TraderKnobs::NORMAL);
        assert_eq!(k.trader.costs(), Costs::default());
        assert_eq!(Difficulty::default().knobs(), k);
        assert_eq!(Knobs::default(), k);
        // The other columns are the structs' own.
        assert_eq!(at(1).lead, LeadKnobs::EASY);
        assert_eq!(at(3).sense, SenseKnobs::HARD);
        assert_eq!(at(4).theatre, TheatreKnobs::BRUTAL);
        assert_eq!(at(4).temper, Temper::BRUTAL);
        // The committed table reproduces the built-in columns, so editing it
        // is the only way to move a level.
        let shipped = Table::parse(SHIPPED).expect("the shipped table parses");
        for level in LEVELS {
            assert_eq!(
                Difficulty::with_table(level, shipped.clone()).knobs(),
                at(level),
                "level {level}"
            );
        }
    }

    #[test]
    fn the_shipped_table_is_well_formed() {
        let (rows, skipped) = authored::parse(SHIPPED, HEADER).expect("header");
        assert!(skipped.is_empty(), "{skipped:?}");
        let t = Table::from_rows(&rows);
        assert_eq!(t.rows.len(), rows.len(), "every row names a known constant");
        assert_eq!(
            rows.len(),
            LEVELS.len() * KNOBS.len(),
            "one row per level and constant"
        );
        for r in &rows {
            assert!(r.field(3).starts_with("[A]"), "line {}: why", r.line);
        }
    }

    #[test]
    fn a_tsv_row_overrides_a_knob() {
        let text = format!(
            "{HEADER}\n3\tOBS_NOISE\t0.07\tinvented\n3\tMAX_THEATRES\t5\tinvented\n\
             3\tFROZEN\t1\tinvented\n3\tTRADE_MARGIN\t0.4\tinvented\n\
             3\tBETRAY_MEMORY_DAYS\tinf\tinvented\n9\tOBS_NOISE\t1\tbad level\n\
             3\tNO_SUCH\t1\tbad name\n3\tCOUNTER_PICK\tx\tbad value\n"
        );
        let t = Table::parse(&text).expect("parses");
        assert_eq!(t.rows.len(), 5, "three bad rows are skipped");
        let hard = Difficulty::with_table(3, t.clone()).knobs();
        assert_eq!(hard.sense.obs_noise, 0.07);
        assert_eq!(hard.theatre.max_theatres, 5);
        assert!(hard.lead.frozen);
        assert_eq!(hard.trader.margin, 0.4);
        assert_eq!(hard.temper.betray_memory_days, f64::INFINITY);
        // Everything else at level 3 and every other level are untouched.
        assert_eq!(hard.sense.intel_share, SenseKnobs::HARD.intel_share);
        assert_eq!(hard.lead.counter_pick, LeadKnobs::HARD.counter_pick);
        assert_eq!(Difficulty::with_table(2, t).knobs(), at(2));
        // A wrong header is an error, never a silent fallback.
        assert!(Table::parse("level\tname\tvalue\twhy\n").is_err());
    }

    #[test]
    fn level_zero_never_switches() {
        let k = at(0);
        assert!(k.lead.frozen);
        assert!(!at(1).lead.frozen && !at(2).lead.frozen && !at(4).lead.frozen);
        assert_eq!(
            k.theatre.reserve, 0.6,
            "the peaceful world commits at most 40%"
        );
        // Everything else is normal.
        assert_eq!(k.sense, SenseKnobs::NORMAL);
        assert_eq!(k.temper, Temper::NORMAL);
        assert_eq!(
            LeadKnobs {
                frozen: false,
                counter_read: LeadKnobs::NORMAL.counter_read,
                ..k.lead
            },
            LeadKnobs::NORMAL
        );
    }

    #[test]
    fn the_flag_reads_names_and_numbers() {
        for (w, l) in [
            ("easy", 1),
            ("normal", 2),
            ("hard", 3),
            ("brutal", 4),
            ("peaceful", 0),
            ("0", 0),
            ("1", 1),
            ("2", 2),
            ("3", 3),
            ("4", 4),
        ] {
            assert_eq!(Difficulty::parse_level(w), Ok(l), "{w}");
        }
        for w in ["", "5", "-1", "EASY", "adaptive", "2.5"] {
            assert!(Difficulty::parse_level(w).is_err(), "{w:?}");
        }
        assert_eq!(Difficulty::default().level(), NORMAL_LEVEL);
        assert_eq!(Difficulty::built_in(9).level(), MAX_LEVEL);
    }
}
