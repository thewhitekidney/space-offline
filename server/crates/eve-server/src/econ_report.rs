//! **The economy report** (lane X-ECONREPORT): where the ISK came from and
//! went, per region and in total, and what each bot career really earns per
//! active hour, read off the wallet journal ([`crate::ledger`]) and the goods
//! counters ([`crate::econ_ledger`]) over a rolling window.
//!
//! - **ISK by class** ([`isk_class`]): every journal row the ledger counts as
//!   a faucet or sink gets a class by its ref (bounties, mission rewards,
//!   insurance, NPC buy orders, NPC sell orders bought, LP redemptions,
//!   industry fees, taxes, broker fees, ...). Between two characters a
//!   market fill only moves money; its volume is shown as `traded`. A row is
//!   placed in the region of the character whose wallet it is
//!   ([`Entry::wallet`]); a corporation's wallet has no region and shows in
//!   the total only.
//! - **Item flows**: the goods minted and destroyed per class since the last
//!   rollover and over the stored days. The server does not attribute an item
//!   to an activity (explore, kill, mine, build), so the report prints the
//!   activity proxies the journal does have: bounty rows (kill), industry
//!   rows (build), ore minted (mine). **[A]**: falsified by a hook in
//!   `Registry::apply_items` that knows the source, which this lane does not
//!   own.
//! - **Bot progress** ([`BotRow`], [`Group`]): per bot, the wallet delta
//!   (net) and the realised income (gross) over its **active hours**, an
//!   hour of game time in which the bot has at least one journal row.
//!   Grouped by career and by ship tier. A starting grant, a GM transfer and
//!   an escrow are not earnings. The career is the parked agent's when it has
//!   a row ([`crate::econ_agents::agent`]); otherwise it is **inferred** from
//!   the income mix ([`infer_career`]) **[A]**: the server is never told a
//!   live bot's career. Falsified by a bot-side career signal. The tier is
//!   the hull the bot sits in *now* by `rigSize` ([`tier`]) **[A]**, not the
//!   hull it earned in.
//! - **Outputs**: the `/econ [region]` and `/econ report [region] [hours]`
//!   text, `/econ tsv [hours]`, a `[econ] health` line at each rollover, and
//!   a TSV appended at each rollover to the file named by the environment
//!   variable [`TSV_ENV`] (`tools/econ-report.py` reads it).
//!
//! No state of its own: a pure read of the world.

pub mod tape;

use crate::damage::BlueTime;
use crate::econ_ledger::{self as ledger, Class, Flow};
use crate::ledger::{Entry, REF_GM_CASH_TRANSFER};
use crate::service::Registry;
use std::collections::{BTreeMap, BTreeSet};

/// The environment variable naming the TSV the rollover appends to.
pub const TSV_ENV: &str = "EVE_ECON_TSV";

/// `/econ` with no window: the last game day.
pub const DEFAULT_HOURS: f64 = 24.0;

/// The TSV's columns.
pub const TSV_HEADER: &str = "day\tscope\tkind\tkey\tn\thours\tisk\tisk2\tper_h";

/// `refNewCharacterStartingFunds`-style grants are not earnings (the ledger's
/// own ref).
const REF_START_FUNDS: i64 = 10_004;

/// The region key of a wallet with no character (a corporation).
const NO_REGION: i64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dir {
    Source,
    Sink,
}

/// The class of a journal row the ledger counts as a faucet or sink, `None`
/// for one that only moves money.
pub fn isk_class(e: &Entry) -> Option<(Dir, &'static str)> {
    let dir = match ledger::classify(e) {
        Class::Faucet => Dir::Source,
        Class::Sink => Dir::Sink,
        Class::Unclassified if e.amount >= 0.0 => Dir::Source,
        Class::Unclassified => Dir::Sink,
        Class::Moved => return None,
    };
    let credit = dir == Dir::Source;
    Some((
        dir,
        match e.entry_type_id {
            17 | 85 => "bounties",
            33 | 34 => "mission rewards",
            19 if credit => "insurance payouts",
            19 => "insurance premiums",
            26 => "LP redemptions",
            2 if credit => "NPC buy orders",
            2 => "NPC sell orders bought",
            46 => "broker fees",
            54 => "taxes",
            56..=60 | 135 => "industry fees",
            127 => "taxes",
            96..=98 => "taxes",
            13 | 39 | 48 | 49 | 117 => "fees",
            3 | REF_START_FUNDS if credit => "grants",
            3 | REF_START_FUNDS => "upkeep burn",
            _ if credit => "other sources",
            _ => "other sinks",
        },
    ))
}

/// ISK by class for one scope.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Flows {
    /// `(direction, class) -> (ISK, rows)`.
    pub by: BTreeMap<(Dir, &'static str), (f64, usize)>,
    /// ISK that changed hands between two characters on the market.
    pub traded: f64,
}

impl Flows {
    fn add(&mut self, e: &Entry) {
        match isk_class(e) {
            Some((d, c)) => {
                let s = self.by.entry((d, c)).or_default();
                s.0 += e.amount.abs();
                s.1 += 1;
            }
            None => {
                if e.entry_type_id == 2 && e.amount > 0.0 {
                    self.traded += e.amount;
                }
            }
        }
    }

    pub fn total(&self, d: Dir) -> f64 {
        self.by
            .iter()
            .filter(|((k, _), _)| *k == d)
            .fold(0.0, |a, (_, v)| a + v.0)
    }
}

/// One bot over the window.
#[derive(Debug, Clone, PartialEq)]
pub struct BotRow {
    pub id: i64,
    pub region: i64,
    pub career: String,
    /// The career is the parked agent's, not inferred.
    pub declared: bool,
    pub tier: &'static str,
    /// Game hours with at least one journal row.
    pub hours: u32,
    /// Wallet delta, earnings and spending alike.
    pub net: f64,
    /// Positive earnings.
    pub gross: f64,
}

/// Bots pooled by career or tier.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Group {
    pub bots: usize,
    pub hours: u32,
    pub net: f64,
    pub gross: f64,
}

impl Group {
    fn add(&mut self, b: &BotRow) {
        self.bots += 1;
        self.hours += b.hours;
        self.net += b.net;
        self.gross += b.gross;
    }
    pub fn net_per_h(&self) -> f64 {
        self.net / f64::from(self.hours.max(1))
    }
    pub fn gross_per_h(&self) -> f64 {
        self.gross / f64::from(self.hours.max(1))
    }
}

/// The report over `[from, to)` (wall FILETIMEs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub hours: f64,
    pub all: Flows,
    pub regions: BTreeMap<i64, Flows>,
    pub bots: Vec<BotRow>,
    /// Bots with no journal row in the window.
    pub idle_bots: usize,
}

/// The tier of the ship type a character sits in: `rigSize` 1..4 is
/// S, M, L, C; a pod or an unknown hull is `?`. **[A]** (see the module).
pub fn tier(reg: &Registry, ship_type_id: i64) -> &'static str {
    let size = reg
        .content()
        .try_get::<crate::typedata::Installed>()
        .and_then(|t| {
            t.table()
                .get(ship_type_id)?
                .attribute(crate::rigs::ATTR_RIG_SIZE)
        });
    match size.map(|s| s.round() as i64) {
        Some(1) => "S",
        Some(2) => "M",
        Some(3) => "L",
        Some(4) => "C",
        _ => "?",
    }
}

/// A bot's career from its income, when it declared none **[A]**.
pub fn infer_career(income: &BTreeMap<&'static str, f64>, industry_rows: usize) -> &'static str {
    let get = |k: &str| income.get(k).copied().unwrap_or(0.0);
    let market = get("NPC buy orders") + get("player sales");
    let best = [
        ("pve", get("bounties")),
        ("missions", get("mission rewards")),
        ("market", market),
    ]
    .into_iter()
    .max_by(|a, b| a.1.total_cmp(&b.1))
    .filter(|b| b.1 > 0.0);
    match best {
        Some(("market", _)) if industry_rows > 0 => "builder",
        Some((name, _)) => name,
        None => "idle",
    }
}

/// Is this row an earning or an expense of the wallet's owner, rather than a
/// grant or an escrow.
fn counts(e: &Entry) -> bool {
    !matches!(ledger::flow(e.entry_type_id), Some(Flow::Held))
        && e.entry_type_id != REF_GM_CASH_TRANSFER
        && e.entry_type_id != REF_START_FUNDS
}

impl Report {
    pub fn of(reg: &Registry, from: BlueTime, to: BlueTime) -> Report {
        let clock = crate::game_clock::of(reg);
        let hour = |date: i64| (clock.elapsed((date - from).max(0)) / crate::seed::HOUR) as i64;
        let mut r = Report {
            hours: clock.elapsed((to - from).max(0)) as f64 / crate::seed::HOUR as f64,
            ..Report::default()
        };
        #[derive(Default)]
        struct Acc {
            hours: BTreeSet<i64>,
            net: f64,
            gross: f64,
            income: BTreeMap<&'static str, f64>,
            industry: usize,
        }
        let mut accs: BTreeMap<i64, Acc> = BTreeMap::new();
        for e in reg
            .journal()
            .entries()
            .iter()
            .filter(|e| e.date >= from && e.date < to)
        {
            let who = reg.character(e.wallet());
            let region = who.map_or(NO_REGION, |c| c.region_id);
            r.all.add(e);
            r.regions.entry(region).or_default().add(e);
            let Some(c) = who.filter(|c| c.is_bot) else {
                continue;
            };
            let a = accs.entry(c.character_id).or_default();
            a.hours.insert(hour(e.date));
            if !counts(e) {
                continue;
            }
            a.net += e.amount;
            if e.amount > 0.0 {
                a.gross += e.amount;
                let class = match isk_class(e) {
                    Some((_, c)) => c,
                    None if e.entry_type_id == 2 => "player sales",
                    None => "other sources",
                };
                *a.income.entry(class).or_default() += e.amount;
            } else if matches!(isk_class(e), Some((_, "industry fees"))) {
                a.industry += 1;
            }
        }
        for c in reg.config().characters.iter().filter(|c| c.is_bot) {
            let Some(a) = accs.get(&c.character_id) else {
                r.idle_bots += 1;
                continue;
            };
            let declared = crate::econ_agents::agent(reg, c.character_id).map(|g| g.career.name());
            r.bots.push(BotRow {
                id: c.character_id,
                region: c.region_id,
                career: declared
                    .unwrap_or_else(|| infer_career(&a.income, a.industry))
                    .to_owned(),
                declared: declared.is_some(),
                tier: tier(reg, c.ship_type_id),
                hours: a.hours.len() as u32,
                net: a.net,
                gross: a.gross,
            });
        }
        r
    }

    /// The bots of `region` (all with `None`) pooled by `key`.
    pub fn groups<'a>(
        &'a self,
        region: Option<i64>,
        key: impl Fn(&'a BotRow) -> String,
    ) -> BTreeMap<String, Group> {
        let mut g: BTreeMap<String, Group> = BTreeMap::new();
        for b in self
            .bots
            .iter()
            .filter(|b| region.is_none_or(|r| r == b.region))
        {
            g.entry(key(b)).or_default().add(b);
        }
        g
    }
}

fn flows_lines(f: &Flows, out: &mut Vec<String>) {
    for (d, label) in [(Dir::Source, "sources"), (Dir::Sink, "sinks")] {
        let mut v: Vec<_> = f.by.iter().filter(|((k, _), _)| *k == d).collect();
        v.sort_by(|a, b| b.1.0.total_cmp(&a.1.0));
        let list = v
            .iter()
            .map(|((_, c), (isk, rows))| format!("{c} {isk:.0} ({rows})"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!(
            "ISK {label} {:.0}: {}",
            f.total(d),
            if list.is_empty() { "none" } else { &list }
        ));
    }
    out.push(format!("ISK traded between characters: {:.0}", f.traded));
}

fn group_line(label: &str, g: &BTreeMap<String, Group>) -> String {
    if g.is_empty() {
        return format!("{label}: no bot active");
    }
    let list = g
        .iter()
        .map(|(k, v)| {
            format!(
                "{k} {} bots {}h net {:.0}/h gross {:.0}/h",
                v.bots,
                v.hours,
                v.net_per_h(),
                v.gross_per_h()
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("{label}: {list}")
}

/// The item-flow line: goods minted and destroyed since the last rollover
/// and the activity proxies of the window.
fn items_line(reg: &Registry, f: &Flows) -> String {
    let g = ledger::goods_today(reg);
    let ore = g.0.get("ore").map_or(0, |p| p.0);
    let rows = |c: &str| f.by.get(&(Dir::Source, c)).map_or(0, |v| v.1);
    format!(
        "items since the last rollover (minted/destroyed by class): {}; activity proxies: \
         mine {ore} ore minted, kill {} bounty rows, build {} industry rows, explore not attributed; \
         ESS main banks hold {:.0} ISK (the cut, unpaid until ESS-2)",
        g.part(),
        rows("bounties"),
        f.by.get(&(Dir::Sink, "industry fees")).map_or(0, |v| v.1),
        crate::ess::held(reg),
    )
}

/// `/econ [region]` and `/econ report [region] [hours]`.
pub fn text(reg: &Registry, now: BlueTime, region: Option<i64>, hours: f64) -> String {
    let from = now - crate::game_clock::span(reg, (hours * crate::seed::HOUR as f64) as i64);
    let r = Report::of(reg, from, now);
    let mut out = vec![format!(
        "[econ] report {} over the last {hours:.0} game hours ({} bots active, {} idle)",
        region.map_or("total".to_owned(), |x| format!("region {x}")),
        r.bots.len(),
        r.idle_bots
    )];
    match region {
        Some(x) => match r.regions.get(&x) {
            Some(f) => flows_lines(f, &mut out),
            None => out.push(format!("no ISK moved in region {x} in the window")),
        },
        None => {
            flows_lines(&r.all, &mut out);
            let list = r
                .regions
                .iter()
                .map(|(x, f)| {
                    format!(
                        "{x}: +{:.0} -{:.0}",
                        f.total(Dir::Source),
                        f.total(Dir::Sink)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            out.push(format!("by region (source/sink): {list}"));
        }
    }
    out.push(items_line(reg, &r.all));
    out.push(group_line(
        "ISK/h by career",
        &r.groups(region, |b| b.career.clone()),
    ));
    out.push(group_line(
        "ISK/h by ship tier",
        &r.groups(region, |b| b.tier.to_owned()),
    ));
    out.join("\n")
}

/// The report as TSV rows (no header) for day `day`.
pub fn tsv_rows(r: &Report, goods: &ledger::Goods, day: u32) -> Vec<String> {
    let mut v = Vec::new();
    let mut scopes: Vec<(String, Option<i64>, &Flows)> = vec![("all".to_owned(), None, &r.all)];
    scopes.extend(r.regions.iter().map(|(x, f)| (x.to_string(), Some(*x), f)));
    for (scope, region, f) in scopes {
        for ((d, c), (isk, rows)) in &f.by {
            let kind = if *d == Dir::Source {
                "isk_src"
            } else {
                "isk_sink"
            };
            v.push(format!(
                "{day}\t{scope}\t{kind}\t{c}\t{rows}\t0\t{isk:.0}\t0\t0"
            ));
        }
        for (kind, g) in [
            ("career", r.groups(region, |b| b.career.clone())),
            ("tier", r.groups(region, |b| b.tier.to_owned())),
        ] {
            for (k, g) in g {
                v.push(format!(
                    "{day}\t{scope}\t{kind}\t{k}\t{}\t{}\t{:.0}\t{:.0}\t{:.0}",
                    g.bots,
                    g.hours,
                    g.net,
                    g.gross,
                    g.net_per_h()
                ));
            }
        }
    }
    for (class, (m, d)) in &goods.0 {
        v.push(format!("{day}\tall\tgoods\t{class}\t0\t0\t{m}\t{d}\t0"));
    }
    v
}

/// The tape (FE-MEASURE) over the last `hours` game hours before `now`, with
/// `goods` for its churn.
pub fn tape(reg: &Registry, now: BlueTime, hours: f64, goods: &ledger::Goods) -> tape::Tape {
    let from = now - crate::game_clock::span(reg, (hours * crate::seed::HOUR as f64) as i64);
    let r = Report::of(reg, from, now);
    tape::Tape::of(
        reg,
        now,
        hours,
        goods,
        &r,
        &tape::PRICE_BASKETS,
        &tape::watch_env(),
    )
}

/// `/econ tsv [hours]`: the header and the rows.
pub fn tsv_text(reg: &Registry, now: BlueTime, hours: f64) -> String {
    let from = now - crate::game_clock::span(reg, (hours * crate::seed::HOUR as f64) as i64);
    let r = Report::of(reg, from, now);
    let mut out = vec![TSV_HEADER.to_owned()];
    let (goods, day) = (ledger::goods_today(reg), reg.world().day.saturating_add(1));
    out.extend(tsv_rows(&r, &goods, day));
    out.extend(tape(reg, now, hours, &goods).tsv_rows(day));
    out.join("\n")
}

/// `/econ price-tape [region] [hours]`: the tape as text.
pub fn tape_text(reg: &Registry, now: BlueTime, region: Option<i64>, hours: f64) -> String {
    tape(reg, now, hours, &ledger::goods_today(reg)).text(region)
}

/// `/econ audit [hours]`: the manipulation detector as text.
pub fn audit_text(reg: &Registry, now: BlueTime, hours: f64) -> String {
    tape(reg, now, hours, &ledger::Goods::default()).audit_text()
}

/// The market-health line of a report.
pub fn health_line(day: u32, r: &Report) -> String {
    let (src, sink) = (r.all.total(Dir::Source), r.all.total(Dir::Sink));
    let top = |d: Dir| {
        r.all
            .by
            .iter()
            .filter(|((k, _), _)| *k == d)
            .max_by(|a, b| a.1.0.total_cmp(&b.1.0))
            .map_or("none".to_owned(), |((_, c), v)| format!("{c} {:.0}", v.0))
    };
    let mut per_h: Vec<f64> = r
        .bots
        .iter()
        .filter(|b| b.hours > 0)
        .map(|b| b.net / f64::from(b.hours))
        .collect();
    per_h.sort_by(f64::total_cmp);
    let median = per_h.get(per_h.len() / 2).copied().unwrap_or(0.0);
    format!(
        "[econ] health day {day} sources={src:.0} sinks={sink:.0} net={:.0} top_source=({}) \
         top_sink=({}) traded={:.0} bots_active={} bots_idle={} bot_median_isk_h={median:.0}",
        src - sink,
        top(Dir::Source),
        top(Dir::Sink),
        r.all.traded,
        r.bots.len(),
        r.idle_bots,
    )
}

/// Append `rows` to the TSV at `path`, writing the header first when the file
/// is new or empty.
#[cfg(test)]
pub fn append_tsv(path: &std::path::Path, rows: &[String]) -> std::io::Result<()> {
    use std::io::Write;
    let fresh = std::fs::metadata(path).is_ok_and(|m| m.len() > 0);
    let mut text = String::new();
    if !fresh {
        text.push_str(TSV_HEADER);
        text.push('\n');
    }
    for l in rows {
        text.push_str(l);
        text.push('\n');
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())
}

/// [`append_tsv`] on the log writer (inline when it is not running): the
/// header goes first when the file is new or empty, as there.
pub fn append_tsv_queued(path: std::path::PathBuf, rows: &[String]) {
    crate::logline::append(
        path,
        Some(TSV_HEADER.to_owned()),
        rows.iter().fold(String::new(), |mut t, l| {
            t.push_str(l);
            t.push('\n');
            t
        }),
    );
}

/// The rollover step: print the health line and append the day's rows to the
/// [`TSV_ENV`] file. Its count is the bots active. After `econ_ledger`.
pub fn on_rollover(reg: &mut Registry, d: &crate::rollover::Day) -> usize {
    let from = d.now - crate::game_clock::span(reg, i64::from(d.days.max(1)) * crate::seed::DAY);
    let r = Report::of(reg, from, d.now);
    crate::outln!("{}", health_line(d.day, &r));
    if let Some(path) = std::env::var_os(TSV_ENV) {
        let goods = ledger::goods_of_day(reg, d.day);
        let mut rows = tsv_rows(&r, &goods, d.day);
        let hours = f64::from(d.days.max(1)) * 24.0;
        rows.extend(tape(reg, d.now, hours, &goods).tsv_rows(d.day));
        // TT-B C4: the writer thread does the file work, not this lock.
        append_tsv_queued(path.into(), &rows);
    }
    r.bots.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::World;
    use crate::model::{Character, SandboxConfig};

    const HUMAN: i64 = 90_000_001;
    const MINER: i64 = 90_000_002;
    const RATTER: i64 = 90_000_003;
    const SYSTEM: i64 = 1;
    const NPC_ORDER: i64 = crate::econ_exchange::NPC_ORDER_BASE;
    const REGION_A: i64 = 10_000_901;
    const REGION_B: i64 = 10_000_902;

    fn world() -> Registry {
        let mut w = Registry::new(SandboxConfig::default());
        w.deltas_mut().set_world(World::FIXTURE);
        w.character_mut(HUMAN).unwrap().region_id = REGION_A;
        for (id, ship, region) in [
            (MINER, 100_000_002, REGION_A),
            (RATTER, 100_000_003, REGION_B),
        ] {
            w.seed_character(Character {
                character_id: id,
                ship_id: ship,
                region_id: region,
                is_bot: true,
                ..Character::default()
            });
        }
        w
    }

    fn t0() -> BlueTime {
        World::FIXTURE.birth
    }

    fn pay(w: &mut Registry, at: BlueTime, kind: i64, to: i64, amount: f64, order: i64) {
        let (from, owner) = if amount >= 0.0 {
            (SYSTEM, to)
        } else {
            (to, SYSTEM)
        };
        w.journal_mut()
            .record(at, kind, from, owner, order, amount, 0.0, "");
    }

    #[test]
    fn isk_is_classed_by_ref_and_placed_in_the_wallets_region() {
        let mut w = world();
        pay(&mut w, t0(), 85, RATTER, 400.0, 0);
        pay(&mut w, t0(), 33, HUMAN, 100.0, 0);
        pay(&mut w, t0(), 2, MINER, 50.0, NPC_ORDER);
        pay(&mut w, t0(), 2, MINER, -20.0, NPC_ORDER);
        pay(&mut w, t0(), 46, MINER, -3.0, 0);
        pay(&mut w, t0(), 19, HUMAN, 7.0, 0);
        let r = Report::of(&w, t0(), t0() + crate::seed::HOUR);
        let get = |f: &Flows, d, c| f.by.get(&(d, c)).map_or(0.0, |v| v.0);
        assert_eq!(get(&r.all, Dir::Source, "bounties"), 400.0);
        assert_eq!(get(&r.all, Dir::Source, "NPC buy orders"), 50.0);
        assert_eq!(get(&r.all, Dir::Sink, "NPC sell orders bought"), 20.0);
        assert_eq!(get(&r.all, Dir::Sink, "broker fees"), 3.0);
        assert_eq!(r.all.total(Dir::Source), 557.0);
        let (a, b) = (&r.regions[&REGION_A], &r.regions[&REGION_B]);
        assert_eq!(get(a, Dir::Source, "mission rewards"), 100.0);
        assert_eq!(get(a, Dir::Source, "insurance payouts"), 7.0);
        assert_eq!(get(b, Dir::Source, "bounties"), 400.0);
        assert_eq!(get(b, Dir::Source, "mission rewards"), 0.0);
    }

    #[test]
    fn a_trade_between_characters_is_volume_not_faucet() {
        let mut w = world();
        // A fill between two characters: the order id is not an NPC's.
        pay(&mut w, t0(), 2, MINER, 90.0, 5);
        pay(&mut w, t0(), 2, HUMAN, -90.0, 5);
        let r = Report::of(&w, t0(), t0() + crate::seed::HOUR);
        assert!(r.all.by.is_empty());
        assert_eq!(r.all.traded, 90.0);
    }

    #[test]
    fn the_window_excludes_rows_outside_it() {
        let mut w = world();
        pay(&mut w, t0() - 1, 85, RATTER, 1.0, 0);
        pay(&mut w, t0() + crate::seed::HOUR, 85, RATTER, 2.0, 0);
        pay(&mut w, t0() + 5, 85, RATTER, 4.0, 0);
        let r = Report::of(&w, t0(), t0() + crate::seed::HOUR);
        assert_eq!(r.all.total(Dir::Source), 4.0);
    }

    #[test]
    fn a_bots_rate_is_its_wallet_delta_over_its_active_hours() {
        let mut w = world();
        let h = crate::seed::HOUR;
        // The miner sells in hours 0 and 2 and pays a fee in hour 2; a grant
        // in hour 5 is not an earning and does not make the hour active
        // for the rate (it does count as a row in the hour).
        pay(&mut w, t0() + 10, 2, MINER, 300.0, NPC_ORDER);
        pay(&mut w, t0() + 2 * h + 10, 2, MINER, 500.0, NPC_ORDER);
        pay(&mut w, t0() + 2 * h + 20, 46, MINER, -100.0, 0);
        pay(&mut w, t0() + 5 * h, 3, MINER, 9_999.0, 0);
        let r = Report::of(&w, t0(), t0() + 8 * h);
        let m = r.bots.iter().find(|b| b.id == MINER).expect("the miner");
        assert_eq!(m.net, 700.0);
        assert_eq!(m.gross, 800.0);
        assert_eq!(m.hours, 3);
        assert_eq!(m.career, "market");
        assert!(!m.declared);
        // The ratter earned nothing: idle, not in the rows.
        assert!(r.bots.iter().all(|b| b.id != RATTER));
        assert_eq!(r.idle_bots, 1);
        let g = r.groups(None, |b| b.career.clone());
        assert!((g["market"].net_per_h() - 700.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_career_is_inferred_from_the_income_mix() {
        let mix = |v: &[(&'static str, f64)]| v.iter().copied().collect();
        assert_eq!(
            infer_career(&mix(&[("bounties", 9.0), ("NPC buy orders", 3.0)]), 0),
            "pve"
        );
        assert_eq!(
            infer_career(&mix(&[("mission rewards", 9.0)]), 0),
            "missions"
        );
        assert_eq!(infer_career(&mix(&[("NPC buy orders", 9.0)]), 0), "market");
        assert_eq!(infer_career(&mix(&[("NPC buy orders", 9.0)]), 2), "builder");
        assert_eq!(infer_career(&mix(&[]), 0), "idle");
    }

    #[test]
    fn the_tsv_gets_one_header_and_appends() {
        let dir = std::env::temp_dir().join(format!("econ-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("e.tsv");
        append_tsv(&f, &["a".to_owned()]).unwrap();
        append_tsv(&f, &["b".to_owned()]).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert_eq!(text, format!("{TSV_HEADER}\na\nb\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn game_hours_follow_the_clock() {
        let mut w = world();
        crate::game_clock::set(
            &mut w,
            crate::game_clock::GameClock::new(10).expect("scale"),
        );
        // At 10x, one wall hour is ten game hours: rows 7 wall minutes apart
        // fall in different game hours.
        let wall_min = crate::seed::HOUR / 60;
        pay(&mut w, t0(), 85, RATTER, 1.0, 0);
        pay(&mut w, t0() + 7 * wall_min, 85, RATTER, 1.0, 0);
        let r = Report::of(&w, t0(), t0() + crate::seed::HOUR);
        assert_eq!(r.hours, 10.0);
        assert_eq!(r.bots[0].hours, 2);
    }

    #[test]
    fn the_text_the_health_line_and_the_tsv_agree() {
        let mut w = world();
        pay(&mut w, t0() + 1, 85, RATTER, 400.0, 0);
        pay(&mut w, t0() + 2, 2, MINER, -40.0, NPC_ORDER);
        let r = Report::of(&w, t0(), t0() + crate::seed::HOUR);
        let health = health_line(7, &r);
        assert!(
            health.starts_with("[econ] health day 7 sources=400 sinks=40 net=360 "),
            "{health}"
        );
        assert!(health.contains("top_source=(bounties 400)"), "{health}");
        let rows = tsv_rows(&r, &ledger::Goods::default(), 7);
        assert!(
            rows.contains(&"7\tall\tisk_src\tbounties\t1\t0\t400\t0\t0".to_owned()),
            "{rows:?}"
        );
        let ratter_row = rows
            .iter()
            .find(|l| l.starts_with(&format!("7\t{REGION_B}\tcareer\tpve")))
            .expect("the ratter's region has a pve row");
        assert!(ratter_row.ends_with("\t400\t400\t400"), "{ratter_row}");
        for l in &rows {
            assert_eq!(l.split('\t').count(), TSV_HEADER.split('\t').count(), "{l}");
        }
        let now = t0() + crate::seed::HOUR;
        let all = text(&w, now, None, 1.0);
        assert!(all.contains("ISK sources 400: bounties 400 (1)"), "{all}");
        assert!(all.contains("ISK/h by career: "), "{all}");
        let one = text(&w, now, Some(REGION_A), 1.0);
        assert!(one.contains("region 10000901"), "{one}");
        assert!(one.contains("ISK sinks 40"), "{one}");
        assert!(!one.contains("bounties 400"), "{one}");
    }
}
