//! **Offline agents** (E1 = R-ECON E12 = R-SOCIETY SOC-E1): the lazy
//! settlement of a logged-off bot's career, so 50 live connections can stand
//! for hundreds of pilots.
//!
//! Spec: `out/research/R-ECON.md` §1.5 and §3 stage 3, `R-SOCIETY.md` §5 and
//! stage E; lane note `docs/lanes/e1-settle.md`.
//!
//! ## The idea
//!
//! A bot is a character with a wallet, a hangar and orders: all of it real
//! and persisted. While it is **logged on** it is a hot or warm live
//! connection. While it is **logged off** (the `--society` session
//! timetable logs bots off between sessions) nothing happens to it, and the
//! economy loses the bot's output. This module gives that time back **by
//! settlement**: when the bot next logs on (and on a periodic sweep while it
//! is away) the server works out what the bot's career would have produced
//! and consumed over the offline span, by the same pure rules a live bot
//! lives under, and applies it through the real paths.
//!
//! ## What the server knows, and how
//!
//! The server cannot see a bot's mind. At log-off the bot says, once, which
//! career it was playing: `econAgent.Park(career)` ([`wire`]). **Why a new
//! call and not an existing one**: no existing call carries a career; the
//! character description would be a bot hiding state in a human-facing text,
//! and a GM slash would be GM-gated (T9: "GM is for setup only", and the
//! bots' log-off is not setup). The call is bot-only -- a character the
//! server marked a bot (`Character::is_bot`, account-book flag) -- and
//! **it grants nothing**: it names a career, and the server then settles only
//! what the character's real hull, modules, rounds, blueprints, skills and
//! wallet can do ([`rules`]'s header: "a settlement is never a privilege").
//! A bot that never calls it, or a human, is never settled. The row it
//! writes is a delta ([`KIND`]), so a restart keeps it.
//!
//! ## When it runs
//!
//! - **At login** ([`on_login`], a `session_hooks::ON_LOGIN` row): the
//!   first thing a returning bot's session does is settle the span since
//!   the last settlement, then drop the row, so the bot sees exactly the
//!   settled state ("a bot that logs in resumes with exactly the settled
//!   state").
//! - **On a sweep** ([`tick`], a `tick::STAGES` row): every
//!   [`SWEEP_EVERY_H`] game hours a slice ([`SWEEP_SLICE`]) of the parked
//!   agents that are due are settled, so the market sees the output over
//!   the offline day and not as one lump at login, and a bot that never
//!   comes back still works (to the caps).
//! - **At the rollover** ([`on_rollover`], a `rollover_steps::STEPS` row):
//!   every parked agent, so the day's `[econ]` line holds its day.
//!
//! ## The clock
//!
//! Spans are wall FILETIMEs; a settled span is game time,
//! `GameClock::elapsed(wall)`, exactly the lane `clock` rule for a derived
//! span. At scale 1 that is the wall span. Work is [`rules::active_hours`]
//! of it.
//!
//! ## What is settled ([`Career`])
//!
//! - gatherers (`miner`, `ice_miner`, `gas_huffer`): ore, ice or gas cut from
//!   the home system's real rocks, into the hangar; ore refined at the
//!   station's refinery (the real plan, the real tax); raw materials sold
//!   into the bids there ([`gather`]);
//! - fighters (`ratter`, `site_runner`, `fw_plexer`): seeded squads killed by
//!   the ship's real guns; bounties through the dynamic bounty system, loot
//!   from the seeded loot tables, rounds consumed, loot sold into the bids
//!   ([`combat`]);
//! - the builder: finished jobs delivered, products sold, materials bought,
//!   the next jobs installed with what the hangar covers ([`build`]);
//! - the trader: the NPC trade-goods loop's net edge on its wallet
//!   ([`trade`]);
//! - the PI farmer: its colonies simulated to now, extractor programs
//!   restarted, the command centre launched to the hangar, the commodities
//!   sold and listed ([`pi`]).
//!
//! A career without a rule here (`mission_runner`, `explorer`, ...) is **refused at [`park`]** with the reason, so the bot knows to stay
//! live; it is never silently idle. See the lane note's "Not done".
//!
//! ## The ledger stays total
//!
//! Every flow goes through code the live game runs: wallet credits and
//! debits are journal rows of refs the ledger already classifies
//! (`econ_ledger::REFS`; a settled bounty is the ref-85 row, the refinery
//! tax ref 127, a market sale refs 2 and 54, a job fee ref 56), goods are
//! made and destroyed through `Registry::apply_items`, which counts them in
//! the closed-loop `goods` classes. The one new money row, the trader's
//! margin, is a ref-2 row against the NPC order band, which the ledger
//! classifies by sign as it does an NPC fill. No new ref, no unclassified
//! row.

pub mod rules;
pub mod wire;

mod build;
mod combat;
mod gather;
mod pi;
mod shop;
mod trade;

use crate::damage::BlueTime;
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::BTreeMap;
use std::time::Instant;

/// The delta kind: `character -> career station parked_at settled_to day
/// used_h runs` (`delta_kinds::KINDS`). One row per parked agent; no row
/// means the character is not an agent now.
pub const KIND: &str = "agent";

/// A sweep runs this often, game hours **[A]**. Falsified by a bid that
/// sits unfilled for a settled day while its miner is parked (too seldom),
/// or by `[world]` stage timings showing the sweep (too often).
pub const SWEEP_EVERY_H: f64 = 1.0;

/// At most this many agents settle in one tick of a sweep **[A]**, so a few
/// hundred parked agents never hold the world lock for one long tick (a
/// ratter draws a few dozen seeded squads); the sweep goes on tick after tick
/// until every due agent is settled. Falsified by the `[world]` stage timing
/// of `econ_agents` above the 25 ms budget (lower it), or by a sweep that
/// outlasts its period (raise it).
pub const SWEEP_SLICE: usize = 2;

/// How many `[settle]` reports the world remembers for `/settle`.
pub const KEEP_REPORTS: usize = 20;

// ---- careers ---------------------------------------------------------------------

/// What kind of ore a gatherer cuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stuff {
    Ore,
    Ice,
    Gas,
}

/// Where a fighter's squads come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ground {
    /// A belt's rats: `npc_gen::belt_squad_in`.
    Belt,
    /// An anomaly's: the same pool drawn [`combat::SITE_SEC_DROP`] harder.
    Site,
}

/// What a career does while its pilot is away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Gather(Stuff),
    Fight(Ground),
    Build,
    Trade,
    Colony,
}

/// A career the server settles. The names are the bot's career table's
/// (`eve-bot/src/career.rs`), which a stored mind names its career by and
/// which therefore never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Career {
    Miner,
    IceMiner,
    GasHuffer,
    Ratter,
    SiteRunner,
    FwPlexer,
    Builder,
    Trader,
    PiFarmer,
}

/// Every settled career, in table order.
pub const CAREERS: &[Career] = &[
    Career::Miner,
    Career::IceMiner,
    Career::GasHuffer,
    Career::Ratter,
    Career::SiteRunner,
    Career::FwPlexer,
    Career::Builder,
    Career::Trader,
    Career::PiFarmer,
];

impl Career {
    /// The bot-side name.
    pub fn name(self) -> &'static str {
        match self {
            Career::Miner => "miner",
            Career::IceMiner => "ice_miner",
            Career::GasHuffer => "gas_huffer",
            Career::Ratter => "ratter",
            Career::SiteRunner => "site_runner",
            Career::FwPlexer => "fw_plexer",
            Career::Builder => "builder",
            Career::Trader => "trader",
            Career::PiFarmer => "pi_farmer",
        }
    }

    /// The career a bot names, `None` for one the server has no rule for.
    pub fn parse(name: &str) -> Option<Career> {
        CAREERS.iter().copied().find(|c| c.name() == name)
    }

    /// Which rule settles it.
    pub fn family(self) -> Family {
        match self {
            Career::Miner => Family::Gather(Stuff::Ore),
            Career::IceMiner => Family::Gather(Stuff::Ice),
            Career::GasHuffer => Family::Gather(Stuff::Gas),
            Career::Ratter | Career::FwPlexer => Family::Fight(Ground::Belt),
            Career::SiteRunner => Family::Fight(Ground::Site),
            Career::Builder => Family::Build,
            Career::Trader => Family::Trade,
            Career::PiFarmer => Family::Colony,
        }
    }
}

/// Why a career is not settled, for the bot's `MISSING` line: what the
/// server lacks. Careers not in [`CAREERS`] and not listed here are unknown
/// names.
pub fn unsettled_reason(name: &str) -> Option<&'static str> {
    Some(match name {
        "mission_runner" => "agent missions need a live pilot to talk, accept and fly",
        "explorer" => "hacking cans are a live minigame (HK2); no offline rule",
        "pirate" | "soldier" | "sov_grunt" | "wormhole_diver" => "not a served career",
        _ => return None,
    })
}

// ---- the agent row ---------------------------------------------------------------

/// One parked agent: the [`KIND`] row, read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Agent {
    pub career: Career,
    /// The station the pilot is docked in (its hangar is where work lands).
    pub station: i64,
    /// When the bot parked, FILETIME.
    pub parked_at: BlueTime,
    /// Settled up to here, FILETIME.
    pub settled_to: BlueTime,
    /// The game day `used_h` counts.
    pub day: u32,
    /// Work hours already settled in `day`.
    pub used_h: f64,
    /// Settlements so far: the seed's counter, so no two draw alike.
    pub runs: u64,
}

fn key(character: i64) -> Vec<String> {
    vec![character.to_string()]
}

impl Agent {
    fn to_row(self) -> Vec<String> {
        vec![
            self.career.name().to_owned(),
            self.station.to_string(),
            self.parked_at.to_string(),
            self.settled_to.to_string(),
            self.day.to_string(),
            self.used_h.to_string(),
            self.runs.to_string(),
        ]
    }

    fn from_row(v: &[String]) -> Option<Agent> {
        let [career, station, parked, settled, day, used, runs] = v else {
            return None;
        };
        Some(Agent {
            career: Career::parse(career)?,
            station: station.parse().ok()?,
            parked_at: parked.parse().ok()?,
            settled_to: settled.parse().ok()?,
            day: day.parse().ok()?,
            used_h: used.parse::<f64>().ok().filter(|u| u.is_finite())?,
            runs: runs.parse().ok()?,
        })
    }
}

/// `character`'s agent row, if it is parked.
pub fn agent(reg: &Registry, character: i64) -> Option<Agent> {
    reg.deltas()
        .of(KIND)
        .get(&key(character))
        .and_then(|v| Agent::from_row(v))
}

fn write(reg: &mut Registry, character: i64, a: Agent) {
    reg.deltas_mut().put(KIND, key(character), a.to_row());
    reg.mark_dirty();
}

fn forget(reg: &mut Registry, character: i64) {
    if reg.deltas_mut().del(KIND, &key(character)).is_some() {
        reg.mark_dirty();
    }
}

/// Every parked agent, by character id.
pub fn parked(reg: &Registry) -> Vec<(i64, Agent)> {
    reg.deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| Some((k.first()?.parse().ok()?, Agent::from_row(v)?)))
        .collect()
}

// ---- parking ---------------------------------------------------------------------

/// Why [`park`] said no.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not a character the server marked a bot: only bots are agents.
    NotABot,
    /// A career the server has no rule for, with why.
    NoRule(String),
    /// Not docked: an agent works from a hangar, and a ship left in space is
    /// not settled.
    NotDocked,
    /// No such character.
    NoCharacter,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotABot => write!(f, "only a bot character can be parked"),
            Refusal::NoRule(why) => write!(f, "no offline rule: {why}"),
            Refusal::NotDocked => write!(f, "only a docked character can be parked"),
            Refusal::NoCharacter => write!(f, "no such character"),
        }
    }
}

/// **Park `character` as an offline agent of `career`** (the bot's
/// `econAgent.Park`, at its log-off). Docked, bot, a career with a rule.
/// Parking again replaces the career and keeps the settled-to mark, so a
/// bot that changed career is settled for the old span at the new rule only
/// from now (the old one was settled at the login that cleared it).
pub fn park(
    reg: &mut Registry,
    character: i64,
    career: &str,
    now: BlueTime,
) -> Result<Career, Refusal> {
    park_as(reg, character, career, now, true)
}

/// [`park`] with the bot check optional: the GM's `/settle park` is a test
/// hook for a character the server did not mark.
pub(crate) fn park_as(
    reg: &mut Registry,
    character: i64,
    career: &str,
    now: BlueTime,
    bots_only: bool,
) -> Result<Career, Refusal> {
    let c = reg.character(character).ok_or(Refusal::NoCharacter)?;
    if bots_only && !c.is_bot {
        return Err(Refusal::NotABot);
    }
    let Some(parsed) = Career::parse(career) else {
        return Err(Refusal::NoRule(
            unsettled_reason(career)
                .unwrap_or("unknown career")
                .to_owned(),
        ));
    };
    let station = reg.docked_station(character).ok_or(Refusal::NotDocked)?;
    let day = reg.world().day;
    let prev = agent(reg, character);
    write(
        reg,
        character,
        Agent {
            career: parsed,
            station,
            parked_at: prev.map_or(now, |p| p.parked_at),
            settled_to: prev.map_or(now, |p| p.settled_to),
            day: prev.map_or(day, |p| p.day),
            used_h: prev.map_or(0.0, |p| p.used_h),
            runs: prev.map_or(0, |p| p.runs),
        },
    );
    Ok(parsed)
}

// ---- the report ------------------------------------------------------------------

/// What one settlement did, for the `[settle]` line and `/settle`.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub character: i64,
    pub name: String,
    pub career: Career,
    pub station: i64,
    /// The offline span settled, game hours.
    pub span_h: f64,
    /// The work in it, game hours.
    pub work_h: f64,
    /// Wallet change.
    pub isk: f64,
    /// Hangar change at the station, `type -> quantity`.
    pub goods: BTreeMap<i64, i64>,
    /// What the rule did or could not do, in words.
    pub notes: Vec<String>,
}

impl Report {
    /// The `[settle]` log line.
    pub fn line(&self) -> String {
        let goods: Vec<String> = self
            .goods
            .iter()
            .filter(|(_, n)| **n != 0)
            .map(|(t, n)| format!("{t}:{n:+}"))
            .collect();
        format!(
            "[settle] {} ({}) {} at {} span={:.1}h work={:.2}h isk={:+.2} goods=[{}]{}",
            self.name,
            self.character,
            self.career.name(),
            self.station,
            self.span_h,
            self.work_h,
            self.isk,
            goods.join(","),
            if self.notes.is_empty() {
                String::new()
            } else {
                format!(" notes: {}", self.notes.join("; "))
            }
        )
    }
}

/// The agents' world slot: the sweep's clock, the recent reports and the
/// running totals. Not persisted (the rows are).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Book {
    swept_at: BlueTime,
    /// A sweep is under way: it began at `swept_at` and has due agents left.
    sweeping: bool,
    recent: Vec<Report>,
    settled: u64,
    isk: f64,
}

impl Book {
    /// The recent reports, oldest first.
    pub fn recent(&self) -> &[Report] {
        &self.recent
    }

    /// Settlements since the server started.
    pub fn settled(&self) -> u64 {
        self.settled
    }

    /// The ISK they moved into wallets, net.
    pub fn isk(&self) -> f64 {
        self.isk
    }

    fn keep(&mut self, r: &Report) {
        self.settled += 1;
        self.isk += r.isk;
        self.recent.push(r.clone());
        if self.recent.len() > KEEP_REPORTS {
            self.recent.remove(0);
        }
    }
}

/// The world slot (`world_ext::SLOTS`).
pub const SLOT: SlotSpec = SlotSpec {
    name: "econ_agents",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Book::default())
}

// ---- the world as an agent sees it ------------------------------------------------

/// The hangar of `character` at `station`: `type -> quantity` of the packaged
/// stacks. What a report diffs.
fn hangar(reg: &Registry, character: i64, station: i64) -> BTreeMap<i64, i64> {
    let mut out: BTreeMap<i64, i64> = BTreeMap::new();
    let slot = crate::item::Slot::new(character, station, crate::model::FLAG_HANGAR);
    for i in reg.store().at(slot).filter(|i| !i.singleton) {
        *out.entry(i.type_id).or_default() += i.quantity;
    }
    out
}

fn balance(reg: &Registry, character: i64) -> f64 {
    reg.character(character).map_or(0.0, |c| c.balance)
}

// ---- settling --------------------------------------------------------------------

/// **Settle `character` up to `now`**, if it is parked and the span is worth
/// settling ([`rules::MIN_SPAN_S`]). The rule of its career runs against the
/// real world; the row moves to `now`; the `[settle]` line is printed and the
/// report kept. `None`: nothing to settle (not parked, or too soon). The
/// row stays: [`on_login`] drops it, a sweep does not.
pub fn settle(reg: &mut Registry, character: i64, now: BlueTime) -> Option<Report> {
    let a = agent(reg, character)?;
    let clock = crate::game_clock::of(reg);
    let span_s = clock.elapsed((now - a.settled_to).max(0)) as f64 / 1.0e7;
    if span_s < rules::MIN_SPAN_S {
        return None;
    }
    settle_span(reg, character, a, span_s, now)
}

/// The settlement itself, for a span of `span_s` game seconds ending at
/// `now`. Split from [`settle`] so `/settle` can name the span.
pub(crate) fn settle_span(
    reg: &mut Registry,
    character: i64,
    a: Agent,
    span_s: f64,
    now: BlueTime,
) -> Option<Report> {
    let day = reg.world().day;
    let used = if a.day == day { a.used_h } else { 0.0 };
    let work_h = rules::active_hours(span_s, used);
    let name = reg.character(character)?.name.clone();
    let before = (balance(reg, character), hangar(reg, character, a.station));
    let mut notes = Vec::new();
    match a.career.family() {
        Family::Gather(stuff) => gather::settle(reg, character, &a, stuff, work_h, now, &mut notes),
        Family::Fight(ground) => {
            combat::settle(reg, character, &a, ground, work_h, now, &mut notes)
        }
        Family::Build => build::settle(reg, character, &a, now, &mut notes),
        Family::Trade => trade::settle(reg, character, &a, work_h, now, &mut notes),
        Family::Colony => pi::settle(reg, character, &a, now, &mut notes),
    }
    let after = (balance(reg, character), hangar(reg, character, a.station));
    let mut goods: BTreeMap<i64, i64> = BTreeMap::new();
    for t in before.1.keys().chain(after.1.keys()) {
        let d = after.1.get(t).copied().unwrap_or(0) - before.1.get(t).copied().unwrap_or(0);
        if d != 0 {
            goods.insert(*t, d);
        }
    }
    write(
        reg,
        character,
        Agent {
            settled_to: now,
            day,
            used_h: used + work_h,
            runs: a.runs + 1,
            ..a
        },
    );
    let report = Report {
        character,
        name,
        career: a.career,
        station: a.station,
        span_h: span_s.min(rules::MAX_SPAN_H * rules::HOUR_S) / rules::HOUR_S,
        work_h,
        isk: after.0 - before.0,
        goods,
        notes,
    };
    crate::outln!("{}", report.line());
    reg.ext_mut::<Book>().keep(&report);
    Some(report)
}

// ---- the three entry points -------------------------------------------------------

/// **Login** (`session_hooks::ON_LOGIN`): settle the span since the last
/// settlement and drop the row; the bot is live again.
pub fn on_login(reg: &mut Registry, character: i64) {
    if agent(reg, character).is_none() {
        return;
    }
    settle(reg, character, crate::pyutil::filetime_now());
    forget(reg, character);
}

/// **The sweep** (`tick::STAGES`): every [`SWEEP_EVERY_H`] game hours a sweep
/// begins, and each tick of it settles the [`SWEEP_SLICE`] longest-unsettled
/// parked agents that are not online, until none is due. An agent whose
/// character is online (a row a crash left) loses its row.
pub fn tick(reg: &mut Registry, _: Instant, ft: BlueTime) {
    let every = crate::game_clock::of(reg).span((SWEEP_EVERY_H * 3600.0 * 1.0e7) as i64);
    let book = reg.ext_mut::<Book>();
    if !book.sweeping {
        if ft - book.swept_at < every {
            return;
        }
        book.sweeping = true;
        book.swept_at = ft;
    }
    if sweep(reg, ft, SWEEP_SLICE) == 0 {
        reg.ext_mut::<Book>().sweeping = false;
    }
}

/// Settle up to `max` due parked agents at `now`; answers how many settled.
pub(crate) fn sweep(reg: &mut Registry, now: BlueTime, max: usize) -> usize {
    let mut due: Vec<(BlueTime, i64)> = Vec::new();
    for (ch, a) in parked(reg) {
        // A row for a character that is online (a crash left it) or gone
        // is no agent.
        if reg.sessions().is_online(ch) || reg.character(ch).is_none() {
            forget(reg, ch);
            continue;
        }
        due.push((a.settled_to, ch));
    }
    due.sort_unstable();
    let mut n = 0;
    for (_, ch) in due.into_iter().take(max) {
        if settle(reg, ch, now).is_some() {
            n += 1;
        }
    }
    n
}

/// **The rollover step** (`rollover_steps::STEPS`): settle every parked
/// agent, so the day's `[econ]` line holds the day. Its count is the agents
/// settled.
pub fn on_rollover(reg: &mut Registry, d: &crate::rollover::Day) -> usize {
    sweep(reg, d.now, usize::MAX)
}

#[cfg(test)]
mod tests;
