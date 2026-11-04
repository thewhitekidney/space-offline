//! **The corp mind** (R-SOCIETY §3.2-3.4, SOC-C4): one shared [`Hall`] per
//! `eve-bots` process holding every bot corp's mind -- its CEO, profile,
//! alliance and planner -- the relation matrix and the stances
//! ([`crate::society`]), and what each bot last reported of itself.
//!
//! **It acts only through a bot's own connection and the client's own
//! calls** ([`turn`]): a bot's turn (docked, at most once per
//! [`HallState::every`]) reports the bot's session (corp, alliance), its
//! losses (`charMgr.GetRecentShipKillsAndLosses`, SOC-C6), then plans with
//! the hall's lock held (pure) and executes the plan on its connection:
//!
//! - **corpless**: accept an offer (an application the corp accepted, 6, or
//!   an invitation, 8: `UpdateApplicationOffer(.., 2)`, the joiner clicks
//!   last); else found a corp (`acquire::learn_skills` for Corporation
//!   Management, then `corpRegistry.AddCorporation` with a ticker from
//!   `GetSuggestedTickerNames`), or apply to the best bot corp
//!   (`InsertApplication`);
//! - **a member**: its satisfaction ([`crate::society::Satisfaction`]);
//!   leaving is `KickOutMember(me)`, the client's own "quit";
//! - **the CEO** (and only the CEO: roles editing is SOC-C3's, so no bot is
//!   a director without being CEO): answer applications
//!   (`GetApplications`, `UpdateApplicationOffer(.., 6 | 4)`), set the
//!   corp's blue and red contacts from its stances
//!   (`Add/Edit/RemoveCorporateContact(s)`, SOC-C2), found, apply to or
//!   leave an alliance (`CreateAlliance`, `ApplyToJoinAlliance`,
//!   `allianceRegistry.DeleteMember`, SOC-C1); as the executor's CEO, answer
//!   alliance applications (`allianceRegistry.UpdateApplication`) and set
//!   the alliance's contacts (`Add/Edit/RemoveAllianceContact(s)`);
//!   **rent an office** at its home station once the corp's Master Wallet
//!   holds [`OFFICE_RESERVE_PERIODS`] periods of rent (CORP-O,
//!   [`Plan::Office`]: `officeManager.GetPriceQuote` and `RentOffice`).
//!
//! **A plan waits while its CEO is offline**: nothing here acts for a
//! character but on that character's connection. The relation matrix moves
//! with every bot's turn ([`HallState::tick`]): decay, the daily derived
//! rules, the stances.
//!
//! **CEO succession** (§3.2, SOC-C3's `corpRegistry.ResignFromCEO`): a CEO
//! done with its corp asks `ResignFromCEO(None)` for the willing
//! successors, best first, and hands the seat to the first
//! ([`Plan::Resign`]); a lone CEO stays rather than close the corp.
//!
//! The hall persists in `<state>/society.tsv` (`eve-bots --state DIR`):
//! the corps, alliances, relations, stances, the contacts the bots set,
//! and the kills already counted. The members are re-read from the bots.

use crate::career::{Band, Career, Signals};
use crate::effects;
use crate::mind::{Mind, Stint, Trait, Traits};
use crate::relations::{self, Temper};
use crate::society::{
    self, Affiliation, AllianceAct, CorpView, Event, Group, Mood, Offer, Profile, Relations,
    Satisfaction, Stance, Stance5, Stances,
};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// `appConst.corporationStartupCost` (`docs/lanes/corp.md` §2) **[V]**.
pub const CORP_FEE: f64 = 1_599_800.0;
/// `appConst.allianceCreationCost` (SOC-C1) **[V]**, charged to the CEO's
/// wallet (SOC-C1's **[A]**). A bot CEO saves for it.
pub const ALLIANCE_FEE: f64 = eve_server::alliance::ALLIANCE_CREATION_COST;
/// `typeCorporationManagement` (`docs/lanes/corp.md` §2) **[V]**.
pub const CORPORATION_MANAGEMENT: i64 = 3363;
/// How often one bot takes a social turn, game seconds **[A]**: falsified
/// by `[turns]` showing these calls crowding human turns (too short), or a
/// CEO sitting on applications for an evening (too long).
pub const SOCIAL_EVERY: u64 = 10 * 60;
/// A pending application is left alone this many game-days before the bot
/// looks elsewhere **[A]**.
pub const APPLY_PATIENCE_DAYS: f64 = 1.0;
/// How many kills and losses a turn reads: the client's own `num`
/// (`combatLogPanel`, SOC-C6) **[V]**.
pub const KILLS_READ: i64 = 25;
/// How many counted kill ids the hall keeps (the newest, ids rise with
/// time): far above [`KILLS_READ`] times the bots reading **[A]**.
pub const KILLS_KEEP: usize = 512;
/// How many observed-fact keys the hall keeps (R-REL-2) **[A]**: the
/// oldest go first (the set sorts by kind, so a flood of one kind could push
/// another's out; falsified by a double count of an old contract).
pub const OBSERVED_KEEP: usize = 4096;
/// The executor is read again this often, game-days **[A]**: falsified by
/// the server moving the executor faster than this.
pub const EXECUTOR_REREAD_DAYS: f64 = 1.0;
/// How many recent acts the hall keeps for `/society`-style reading.
pub const ACTS_KEEP: usize = 64;
/// A CEO looks at renting an office this often, game-days **[A]**: the rent
/// is a monthly bill, so a daily look is more than it needs. Falsified by a
/// corp that sits a week with the money and no office.
pub const OFFICE_EVERY_DAYS: f64 = 1.0;
/// The CEO rents when the Master Wallet holds this many periods of rent
/// **[A]**: one to pay now and one to pay the next bill, so a corp does not
/// rent an office it loses a month later, and does not spend the alliance
/// fee's savings on a room. Falsified by `corp_offices` rollover lines
/// showing bot offices lapsing.
pub const OFFICE_RESERVE_PERIODS: f64 = 2.0;
/// The state file's name in `--state DIR`.
pub const STATE_FILE: &str = "society.tsv";
const STATE_VERSION: &str = "1";

// `appConst.crpApplication*` (`docs/lanes/corp.md` §2) **[V]**.
const APP_APPLIED: i64 = 0;
const APP_RENEGOTIATED: i64 = 1;
const APP_ACCEPTED_BY_CHARACTER: i64 = 2;
const APP_REJECTED_BY_CORPORATION: i64 = 4;
const APP_ACCEPTED_BY_CORPORATION: i64 = 6;
const APP_INVITED_BY_CORPORATION: i64 = 8;
// `allianceApplication*` (`docs/lanes/soc-c1.md` §2) **[V]**.
const ALLIANCE_APP_NEW: i64 = 1;
const ALLIANCE_APP_ACCEPTED: i64 = 2;
const ALLIANCE_APP_REJECTED: i64 = 4;

/// What one bot last reported of itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub name: String,
    pub traits: Traits,
    pub career: String,
    pub band: Band,
    /// The session's `corpid` (an NPC corp when corpless).
    pub corp: i64,
    pub alliance: Option<i64>,
    pub last_turn: Option<u64>,
    pub satisfaction: Satisfaction,
    /// An open application: `(corp, when)`.
    pub applied: Option<(i64, u64)>,
    /// Bumped on a taken corp name, for the next try.
    pub salt: u32,
}

/// A pilot no bot flies, as last seen (R-REL-3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seen {
    pub corp: i64,
    pub alliance: Option<i64>,
    pub at: u64,
}

/// The humans the hall remembers at most (a state bound).
pub const HUMANS_KEEP: usize = 200;

/// One corp's mind.
#[derive(Debug, Clone, PartialEq)]
pub struct CorpMind {
    pub id: i64,
    /// The CEO when a bot is: the only character this mind acts through.
    pub ceo: Option<i64>,
    pub profile: Profile,
    pub founded: u64,
    pub alliance: Option<i64>,
    pub name: String,
    /// An open alliance application: `(alliance, when)`.
    pub applied_alliance: Option<(i64, u64)>,
    /// Bumped on a taken alliance name.
    pub salt: u32,
}

/// One alliance the bots know.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AllianceMind {
    /// The executor corp, when known.
    pub executor: Option<i64>,
    /// When the executor was last read from the server (not persisted).
    pub read_at: Option<u64>,
    pub name: String,
    /// Alliance applications already answered, by corp.
    pub answered: BTreeSet<i64>,
}

/// **The hall's state** (pure, under [`Hall`]'s lock).
#[derive(Debug, Clone, PartialEq)]
pub struct HallState {
    /// A game-day, in seconds (`eve-bots --day-secs`).
    pub day: u64,
    /// [`SOCIAL_EVERY`]; tests use 0.
    pub every: u64,
    pub members: BTreeMap<i64, Member>,
    pub corps: BTreeMap<i64, CorpMind>,
    pub alliances: BTreeMap<i64, AllianceMind>,
    pub relations: Relations,
    pub stances: Stances,
    /// The group books as last read: `(owner, contact) -> standing`.
    pub books: BTreeMap<(i64, i64), f64>,
    /// REL-FIX: the humans the bots count as friends (`--friends`), by
    /// character id: the starting personal relation, never red. Not stored:
    /// the file is read again each run. The R-REL-1 model lane's seam.
    pub friends: BTreeMap<i64, f64>,
    /// R-REL-2: the alliances' own facts (wars, sieges, peace): holder
    /// alliance, subject group. Persisted as `aown` rows.
    pub alliance_own: Relations,
    /// R-REL-2: the difficulty the facts are weighed at (R-REL-6 sets it;
    /// [`Temper::NORMAL`] reproduces today's constants). Not persisted.
    pub temper: Temper,
    /// R-REL-2: the systems and hulls the fairness classifier reads
    /// (`eve-bots --content`). Not persisted; without it a kill reads as
    /// low-sec between unclassed hulls.
    pub ground: Option<Arc<crate::hunt::Ground>>,
    /// R-REL-2: what was already observed, `(kind, who, id)`: a kill's
    /// assisters, a contract, a mail, a war. Persisted as `obs` rows.
    pub observed: BTreeSet<(u8, i64, i64)>,
    /// R-REL-2: when each bot last synced its inbox. Not persisted.
    pub mail_tried: BTreeMap<i64, u64>,
    /// REVIEW-FIX: the pilots a bot sent a blue/friendly offer to, `(bot,
    /// pilot)`: only a reply from one of these earns the accept bonus. Not
    /// persisted (a restart misses an accept; the talk still counts).
    pub offers: BTreeSet<(i64, i64)>,
    /// REVIEW-FIX: Trade standing already granted, `(bot, partner) -> (game
    /// day, ISK counted)`: one pair earns at most [`society::K_TRADE_CAP`]
    /// of regard per game day however many small contracts it settles. Not
    /// persisted.
    pub trade_spent: BTreeMap<(i64, i64), (u64, f64)>,
    /// R-REL-2: when each bot last read its contracts and wars. Not
    /// persisted.
    pub watch_tried: BTreeMap<i64, u64>,
    /// R-REL-3: when each bot last tended its own contact book. Not
    /// persisted.
    pub personal_tried: BTreeMap<i64, u64>,
    /// R-REL-3: the pilots the bots have seen that no bot flies, with their
    /// group and when last seen. Not persisted.
    pub humans: BTreeMap<i64, Seen>,
    /// R-REL-3 (the R-REL-4 seam): shared fleet game-hours with each human
    /// (F5). Nothing fills it yet; the CEO's invitation reads it. Not
    /// persisted.
    pub fleet_hours: BTreeMap<i64, f64>,
    /// R-REL-4: the presence clocks behind F5, `(bot, human) -> clock`
    /// (`bot` 0 is the hall-wide clock of a human). Not persisted.
    pub fleet_clock: BTreeMap<(i64, i64), crate::human::Clock>,
    /// R-REL-4: the tier each bot corp last told a human, `(corp, human) ->
    /// (tier, when)`. Persisted as `htier` rows.
    pub tiers: BTreeMap<(i64, i64), (Stance5, u64)>,
    /// R-REL-3: when a corp last invited a pilot, `(corp, pilot) -> when`.
    /// Not persisted.
    pub invited: BTreeMap<(i64, i64), u64>,
    /// The contacts a bot set (only these does a bot clear).
    pub managed: BTreeSet<(i64, i64)>,
    /// Kill reports already counted.
    pub kills: BTreeSet<i64>,
    /// Kill reports already folded into the matrix (SOC-C5): the victim's
    /// bot and the killer's bot both read a report, and only one may move
    /// the matrix for it. Apart from `kills`, which also gates a mind's
    /// grudge.
    pub recorded: BTreeSet<i64>,
    pub last_daily: Option<u64>,
    /// CORP-O: when each corp's CEO last looked at renting an office. Not
    /// persisted: a restart looks once more, which is one cheap call.
    pub office_tried: BTreeMap<i64, u64>,
    /// BOT-CORP: each corp's CEO state (tax set, payout mark). Not persisted.
    pub ops: BTreeMap<i64, crate::corp_ops::OpsState>,
    /// BOT-CORP: when each member last looked at the corp hangar. Not
    /// persisted.
    pub stock_tried: BTreeMap<i64, u64>,
    /// BOT-CORP: when each corp's CEO last ran its operations pass. Not
    /// persisted.
    pub ops_tried: BTreeMap<i64, u64>,
    /// CO-FOUND: the co-founding pool.
    pub pool: crate::cofound::Pool,
    /// DOC-4: the catalog and its matrix (`Hall::with_doctrine`). Not
    /// persisted; without it the doctrine layer sleeps.
    pub doctrine: Option<Arc<crate::doctrine_hall::DoctrineWorld>>,
    /// DOC-4: `--difficulty` (0 peaceful .. 4 brutal). Not persisted.
    pub level: u8,
    /// DOC-8: every knob the level hands out (`Hall::with_difficulty`).
    pub knobs: crate::difficulty::Knobs,
    /// DOC-4: the society seed the leaders' fancies are drawn under.
    pub doctrine_seed: u64,
    /// DOC-4: each leader group's doctrine book. `current`, `since`, the
    /// last switch and the retiring doctrine persist (`dbook` rows).
    pub dbooks: BTreeMap<Group, crate::doctrine_lead::DoctrineBook>,
    /// DOC-4: each leader's open theatres. Not persisted.
    pub theatres: BTreeMap<Group, Vec<crate::doctrine_theatre::Theatre>>,
    /// DOC-4: open calls for backup. Not persisted.
    pub calls: Vec<crate::doctrine_theatre::Call>,
    /// DOC-4: the fleet commander's pending actions, by character. Not
    /// persisted.
    pub directives: BTreeMap<i64, Vec<crate::doctrine_theatre::Action>>,
    /// DOC-4: the counter doctrine a leader's `Rebuild` wants stocked
    /// (DOC-6 reads it). Not persisted.
    pub stock_wanted: BTreeMap<Group, String>,
    /// DOC-6: the doctrine stock, SRP and staging (only `stock.paid` is
    /// persisted, as `srp` rows).
    pub stock: crate::doctrine_stock::StockState,
    /// DOC-4/5: what each bot reported for the doctrine layer. Not
    /// persisted.
    pub readiness: BTreeMap<i64, crate::doctrine_hall::Readiness>,
    /// BP-CAMP: role hulls (dictors, HICs) a bot saw on sale or held, by
    /// (doctrine leader's group, hull type), with the game second of the
    /// sighting ([`crate::doctrine_assign::SEEN_DAYS`]). A seat for such a role
    /// opens only on a fresh sighting. Not persisted.
    pub hulls_seen: BTreeMap<(Group, i64), u64>,
    /// BP-CAMP: when each (group, hull) was last looked for, found or not, so
    /// a look that found nothing is not repeated within a game day.
    pub hulls_looked: BTreeMap<(Group, i64), u64>,
    /// R-REL-5: the coalition and war state (`crate::coalition`). Not
    /// persisted.
    pub coal: crate::coalition::State,
    /// THREAT: the guard calls a bot outside a fleet posted, for armed corp
    /// mates in its system ([`crate::threat::claim`]). Not persisted.
    pub distress: Vec<crate::threat::Distress>,
    /// The recent acts, newest last.
    pub acts: Vec<String>,
    /// Where the hall persists.
    pub state: Option<PathBuf>,
    pub dirty: bool,
}

/// **The process's hall**: one per `eve-bots --society`.
#[derive(Debug)]
pub struct Hall {
    inner: Mutex<HallState>,
}

/// One thing a bot's turn does, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Corpless: accept an accepted application or an invitation.
    Offers,
    Found,
    /// CO-FOUND: pay `amount` to the pool's founder.
    Pledge {
        to: i64,
        amount: f64,
    },
    /// CO-FOUND: the founder pays every standing pledge back.
    Refund,
    Apply(i64),
    Leave,
    /// CEO: answer the corp's applications.
    Recruit,
    /// R-REL-3: the CEO invites a friendly pilot to the corp.
    Invite(i64),
    /// CEO, or the executor's CEO for an alliance: the owner's contacts.
    Contacts {
        owner: i64,
        alliance: bool,
    },
    FoundAlliance,
    ApplyAlliance(i64),
    /// The executor's CEO: answer the alliance's applications.
    AnswerAlliance(i64),
    LeaveAlliance(i64),
    /// The CEO hands the corp to the best willing member (SOC-C3).
    Resign,
    /// CEO of a corp in an alliance whose executor is not known.
    ReadExecutor(i64),
    /// CORP-O: the CEO rents an office at the home station when the corp
    /// wallet allows.
    Office,
    /// BOT-CORP: the CEO's tax, roles and pay ([`crate::corp_ops::run_ceo`]).
    Ops,
    /// BOT-CORP: a builder, trader or courier looks at the corp hangar
    /// ([`crate::corp_ops::run_member`]); DOC-6: the procurement member
    /// stocks the doctrine division and a fighter without a hull draws his
    /// kit ([`crate::doctrine_stock`]).
    Stock,
    /// DOC-6: the CEO pays ship replacement for doctrine losses on the
    /// corp's ops ([`crate::doctrine_stock::run_srp`]).
    Srp,
    /// DOC-6: the CEO stages, announces and funds the doctrine stock
    /// ([`crate::doctrine_stock::run_stage`]).
    Stage,
    /// R-REL-5: the owner's war pass ([`crate::war_bot::run`]): answer what
    /// is asked, end what has run its course, help a defender, declare.
    /// `owner` is the corp, or the alliance when its executor's CEO speaks.
    Wars {
        owner: i64,
        alliance: bool,
    },
}

impl HallState {
    pub fn new(day: u64) -> HallState {
        HallState {
            day: day.max(1),
            every: SOCIAL_EVERY,
            members: BTreeMap::new(),
            corps: BTreeMap::new(),
            alliances: BTreeMap::new(),
            relations: Relations::default(),
            stances: Stances::default(),
            books: BTreeMap::new(),
            friends: BTreeMap::new(),
            alliance_own: Relations::default(),
            temper: Temper::NORMAL,
            ground: None,
            observed: BTreeSet::new(),
            mail_tried: BTreeMap::new(),
            offers: BTreeSet::new(),
            trade_spent: BTreeMap::new(),
            watch_tried: BTreeMap::new(),
            personal_tried: BTreeMap::new(),
            humans: BTreeMap::new(),
            fleet_hours: BTreeMap::new(),
            fleet_clock: BTreeMap::new(),
            tiers: BTreeMap::new(),
            invited: BTreeMap::new(),
            managed: BTreeSet::new(),
            kills: BTreeSet::new(),
            recorded: BTreeSet::new(),
            last_daily: None,
            office_tried: BTreeMap::new(),
            ops: BTreeMap::new(),
            stock_tried: BTreeMap::new(),
            ops_tried: BTreeMap::new(),
            pool: crate::cofound::Pool::default(),
            doctrine: None,
            level: 2,
            knobs: crate::difficulty::Knobs::builtin(2),
            doctrine_seed: 0,
            dbooks: BTreeMap::new(),
            theatres: BTreeMap::new(),
            calls: Vec::new(),
            directives: BTreeMap::new(),
            stock_wanted: BTreeMap::new(),
            stock: Default::default(),
            readiness: BTreeMap::new(),
            hulls_seen: BTreeMap::new(),
            hulls_looked: BTreeMap::new(),
            coal: crate::coalition::State::default(),
            distress: Vec::new(),
            acts: Vec::new(),
            state: None,
            dirty: false,
        }
    }

    pub(crate) fn act(&mut self, line: String) {
        if self.acts.len() >= ACTS_KEEP {
            self.acts.remove(0);
        }
        self.acts.push(line);
        self.dirty = true;
    }

    /// The corps a bot runs.
    pub fn bot_corps(&self) -> Vec<i64> {
        self.corps
            .values()
            .filter(|c| c.ceo.is_some_and(|ceo| self.members.contains_key(&ceo)))
            .map(|c| c.id)
            .collect()
    }

    /// Bots in `corp`.
    pub fn size(&self, corp: i64) -> usize {
        self.members.values().filter(|m| m.corp == corp).count()
    }

    /// The group a corp counts as: its alliance, or itself.
    pub fn group_of_corp(&self, corp: i64) -> Group {
        self.corps
            .get(&corp)
            .and_then(|c| c.alliance)
            .unwrap_or(corp)
    }

    /// The bot corps a group's view is held by: the alliance's bot member
    /// corps, or the corp itself if a bot runs it.
    pub(crate) fn viewers(&self, g: Group) -> Vec<i64> {
        let bots = self.bot_corps();
        if society::PLAYER_ALLIANCES.contains(&g) {
            bots.into_iter()
                .filter(|c| self.corps.get(c).and_then(|m| m.alliance) == Some(g))
                .collect()
        } else {
            bots.into_iter().filter(|c| *c == g).collect()
        }
    }

    /// A target as the matrix keys it: a corp in an alliance is its
    /// alliance.
    pub(crate) fn target(&self, t: Group) -> Group {
        if society::is_player_corp(t) {
            self.group_of_corp(t)
        } else {
            t
        }
    }

    /// **Fold an event in**: each side's view is held by its bot corps
    /// (an alliance's by each member corp), toward the other side's group.
    ///
    /// R-REL-2: through the relations model's adapter
    /// ([`relations::event_deltas`]); at [`Temper::NORMAL`] the outcome is
    /// the old `Relations::apply` into a scratch matrix, by test.
    pub fn record(&mut self, e: &Event) {
        // R-REL-5: the ISK each side lost to the other, for the surrender rule.
        if let Event::Kill {
            victim,
            killer,
            isk,
        } = *e
        {
            self.coal.lose(victim, killer, isk);
        }
        for d in relations::event_deltas(e, &self.temper) {
            if d.holder == d.subject || !d.d.is_finite() {
                continue;
            }
            let d_clamped = d.d.clamp(-society::R_MAX, society::R_MAX);
            let t = self.target(d.subject);
            for viewer in self.viewers(d.holder) {
                self.relations.add(viewer, t, d_clamped);
            }
        }
        self.dirty = true;
    }

    /// **Apply the model's deltas that this hall holds** (R-REL-2): the
    /// corp level into the matrix when a bot runs the holder corp, the
    /// alliance level into `alliance_own`. Personal deltas belong to the
    /// holder's own mind and are skipped here.
    pub fn apply_deltas(&mut self, ds: &[relations::Delta]) {
        for d in ds {
            if !d.d.is_finite() {
                continue;
            }
            match d.level {
                relations::Level::Personal => {}
                relations::Level::Corp => {
                    let t = self.target(d.subject);
                    // A corp has no view of its own alliance.
                    if self.bot_corps().contains(&d.holder) && self.group_of_corp(d.holder) != t {
                        self.relations.add(d.holder, t, d.d);
                        self.dirty = true;
                    }
                }
                relations::Level::Alliance => {
                    let t = self.target(d.subject);
                    self.alliance_own.add(d.holder, t, d.d);
                    self.dirty = true;
                }
            }
        }
    }

    /// REVIEW-FIX: the ISK of a finished contract between `me` and `them`
    /// that may still count toward Trade regard at `now`: the pair's day total
    /// is held to what [`society::K_TRADE_CAP`] buys at [`society::K_TRADE`]
    /// (the cap the model already puts on one contract, now per pair per game
    /// day). `0.0` once the day's room is spent.
    pub fn trade_allow(&mut self, me: i64, them: i64, now: u64, isk: f64) -> f64 {
        let day = now / self.day.max(1);
        let room_isk = society::K_TRADE_CAP / society::K_TRADE;
        let key = (me.min(them), me.max(them));
        let e = self.trade_spent.entry(key).or_insert((day, 0.0));
        if e.0 != day {
            *e = (day, 0.0);
        }
        let take = isk.max(0.0).min((room_isk - e.1).max(0.0));
        e.1 += take;
        take
    }

    /// **First time?** `true` once per `(kind, who, id)`; the set keeps the
    /// newest [`OBSERVED_KEEP`].
    pub fn first(&mut self, kind: u8, who: i64, id: i64) -> bool {
        if !self.observed.insert((kind, who, id)) {
            return false;
        }
        while self.observed.len() > OBSERVED_KEEP {
            self.observed.pop_first();
        }
        self.dirty = true;
        true
    }

    /// **Fold a kill report in, once** (SOC-C5): `false`, and nothing
    /// changes, when report `id` already moved the matrix.
    pub fn record_kill(&mut self, id: i64, e: &Event) -> bool {
        match self.record_kill_line(id, e) {
            None => false,
            Some(line) => {
                // SOC-WIRE: the line `tools/society-report.py` reads.
                if let Some(line) = line {
                    println!("{line}");
                }
                true
            }
        }
    }

    /// [`HallState::record_kill`] without the printing: `None` when report
    /// `id` was already folded in; else the [`society::loss_line`] of the
    /// victim group's view of the killer before and after (`None` inside
    /// when the event is not a kill).
    pub fn record_kill_line(&mut self, id: i64, e: &Event) -> Option<Option<String>> {
        if !self.recorded.insert(id) {
            return None;
        }
        let Event::Kill { victim, killer, .. } = *e else {
            self.record(e);
            return Some(None);
        };
        let t = self.target(killer);
        let before = self.alliance_relation(victim, t);
        self.record(e);
        let after = self.alliance_relation(victim, t);
        Some(Some(society::loss_line(victim, t, before, after)))
    }

    /// The mean loyalty of a corp's bots (0.5 when none).
    fn loyalty(&self, corp: i64) -> f64 {
        let l: Vec<f64> = self
            .members
            .values()
            .filter(|m| m.corp == corp)
            .map(|m| m.traits.get(Trait::Loyalty))
            .collect();
        if l.is_empty() {
            0.5
        } else {
            l.iter().sum::<f64>() / l.len() as f64
        }
    }

    /// An alliance's relation toward `t`: its bot member corps' mean.
    pub fn alliance_relation(&self, al: i64, t: Group) -> f64 {
        let v = self.viewers(al);
        if v.is_empty() {
            return 0.0;
        }
        v.iter().map(|c| self.relations.get(*c, t)).sum::<f64>() / v.len() as f64
    }

    /// R-REL-3: **the stored layers** (corp matrix, the alliances' own facts,
    /// the alliances' bot corps and the derived blocs) handed to `f` for
    /// composing a view ([`relations::r_eff`], [`relations::c_eff`],
    /// [`relations::a_eff_k`]).
    pub fn with_layers<R>(&self, now: u64, f: impl FnOnce(&relations::Layers<'_>) -> R) -> R {
        let bots = self.bot_corps();
        let mut members: BTreeMap<Group, Vec<Group>> = BTreeMap::new();
        let mut weights: BTreeMap<Group, f64> = BTreeMap::new();
        for c in &bots {
            if let Some(al) = self.corps[c].alliance {
                members.entry(al).or_default().push(*c);
            }
            *weights.entry(self.group_of_corp(*c)).or_default() += self.size(*c) as f64;
        }
        let blocs = relations::blocs(&self.stances, &weights, now, self.day);
        f(&relations::Layers {
            corp: &self.relations,
            alliance_own: &self.alliance_own,
            members: &members,
            blocs: &blocs,
        })
    }

    /// R-REL-3: **a bot's effective view of a pilot** ([`relations::r_eff`]):
    /// its own `regard` list, its corp, its alliance and its coalition. A
    /// pilot no bot flies starts at the temper's human prior; a `--friends`
    /// pilot is never below the file's standing (the seam R-REL-4 widens).
    pub fn r_eff_of(
        &self,
        viewer_corp: i64,
        regard: &[crate::mind::Regard],
        loyalty: f64,
        target: &Affiliation,
        now: u64,
    ) -> f64 {
        let human = !self.members.contains_key(&target.character_id);
        let v = relations::Viewer {
            corp: viewer_corp,
            alliance: self.corps.get(&viewer_corp).and_then(|c| c.alliance),
        };
        let (r, held) = self.with_layers(now, |l| {
            let r = relations::r_eff(
                l,
                &v,
                regard,
                loyalty,
                target,
                human,
                now,
                self.day,
                &self.temper,
            );
            // R-REL-5: the coalition's word on an enemy it named only a
            // moment ago reaches the members after the lag.
            let g = society::group_of(target);
            let bloc = v
                .alliance
                .and_then(|a| l.bloc_of(a))
                .or_else(|| l.bloc_of(v.corp));
            let held = bloc
                .filter(|b| self.lagging(b.lead, g, now))
                .map_or(0.0, |b| self.temper.d_kp() * l.k(b, g));
            (r, held)
        });
        let r = (r - held).clamp(-society::R_MAX, society::R_MAX);
        match self.friends.get(&target.character_id) {
            Some(s) => r.max(s.min(society::R_MAX)),
            None => r,
        }
    }

    /// THREAT: post a guard call ([`crate::threat::post`]).
    pub fn distress_post(&mut self, d: crate::threat::Distress) {
        crate::threat::post(&mut self.distress, d);
    }

    /// THREAT: let `guard` take a call of `corp` in `system`
    /// ([`crate::threat::claim`]); the caller's character.
    pub fn distress_claim(
        &mut self,
        guard: i64,
        corp: i64,
        system: i64,
        now: std::time::Instant,
    ) -> Option<i64> {
        crate::threat::claim(&mut self.distress, guard, corp, system, now)
    }

    /// R-REL-3: remember a pilot no bot flies (seen on grid), with the
    /// group it flies for. Bots are never recorded.
    pub fn see_human(&mut self, a: &Affiliation, now: u64) {
        if self.members.contains_key(&a.character_id) || a.character_id <= 0 {
            return;
        }
        self.humans.insert(
            a.character_id,
            Seen {
                corp: a.corporation_id,
                alliance: a.alliance_id,
                at: now,
            },
        );
        while self.humans.len() > HUMANS_KEEP {
            let oldest = self
                .humans
                .iter()
                .min_by_key(|(id, s)| (s.at, **id))
                .map(|(id, _)| *id);
            match oldest {
                Some(id) => self.humans.remove(&id),
                None => break,
            };
        }
    }

    /// R-REL-3: **the humans a bot counts friendly or better**: seen pilots
    /// whose `R_eff` reaches [`society::FRIEND_AT`] (what a fleet commander
    /// invites whatever they fly).
    pub fn friendly_humans(
        &self,
        corp: i64,
        regard: &[crate::mind::Regard],
        loyalty: f64,
        now: u64,
    ) -> BTreeSet<i64> {
        self.humans
            .iter()
            .filter(|(id, s)| {
                let a = Affiliation {
                    character_id: **id,
                    corporation_id: s.corp,
                    alliance_id: s.alliance,
                };
                self.r_eff_of(corp, regard, loyalty, &a, now) >= society::FRIEND_AT
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// R-REL-3 (the R-REL-4 seam): `hours` more game-hours flown with
    /// `pilot` (F5).
    pub fn note_fleet_hours(&mut self, pilot: i64, hours: f64) {
        if hours.is_finite() && hours > 0.0 {
            *self.fleet_hours.entry(pilot).or_default() += hours;
        }
    }

    /// **The matrix's clock** at `now`: decay, the daily rules (common
    /// enemies, the hegemon, neighbours), then every owner's stance. One
    /// line per stance change.
    pub fn tick(&mut self, now: u64) -> Vec<String> {
        let loyalty: BTreeMap<i64, f64> =
            self.corps.keys().map(|c| (*c, self.loyalty(*c))).collect();
        self.relations
            .decay(now, self.day, |g| loyalty.get(&g).copied().unwrap_or(0.5));
        self.alliance_own.decay(now, self.day, |_| 0.5);
        // CO-FOUND: a pledger that joined a corp has done applying.
        let joined: Vec<i64> = self
            .pool
            .pledged
            .keys()
            .copied()
            .filter(|p| {
                self.members
                    .get(p)
                    .is_none_or(|m| society::is_player_corp(m.corp))
            })
            .collect();
        for p in joined {
            self.pool.pledged.remove(&p);
            self.dirty = true;
        }
        let bots = self.bot_corps();
        let mut daily_doc = false;
        match self.last_daily {
            None => self.last_daily = Some(now),
            Some(at) if now.saturating_sub(at) >= self.day => {
                let days = (now - at) as f64 / self.day as f64;
                self.last_daily = Some(now);
                let groups: BTreeMap<i64, Group> =
                    bots.iter().map(|c| (*c, self.group_of_corp(*c))).collect();
                let of = |c: Group| groups.get(&c).copied().unwrap_or(c);
                self.relations.shared_enemies(&bots, of, days);
                let total: usize = bots.iter().map(|c| self.size(*c)).sum();
                let mut shares: BTreeMap<Group, f64> = BTreeMap::new();
                for c in &bots {
                    *shares.entry(of(*c)).or_default() +=
                        self.size(*c) as f64 / total.max(1) as f64;
                }
                self.relations.hegemon(&bots, &shares, of, days);
                for (i, a) in bots.iter().enumerate() {
                    for b in &bots[i + 1..] {
                        let (pa, pb) = (self.corps[a].profile, self.corps[b].profile);
                        if pa == pb && of(*a) != of(*b) {
                            self.relations.add(*a, of(*b), society::K_NEIGHBOUR * days);
                            self.relations.add(*b, of(*a), society::K_NEIGHBOUR * days);
                        }
                    }
                }
                self.dirty = true;
                daily_doc = true;
            }
            Some(_) => {}
        }
        // The owners: every bot corp, and every alliance with a bot member.
        let mut lines = Vec::new();
        if daily_doc {
            lines.extend(self.doctrine_daily(now));
        }
        let mut pairs: Vec<(i64, i64, f64)> = Vec::new();
        for c in &bots {
            let own = self.group_of_corp(*c);
            let targets: BTreeSet<i64> = self
                .relations
                .r
                .keys()
                .filter(|(v, _)| v == c)
                .map(|(_, t)| *t)
                .chain(
                    self.stances
                        .held
                        .keys()
                        .filter(|(o, _)| o == c)
                        .map(|(_, t)| *t),
                )
                .chain(
                    self.stances
                        .graded
                        .keys()
                        .filter(|(o, _)| o == c)
                        .map(|(_, t)| *t),
                )
                .filter(|t| *t != *c && *t != own)
                .collect();
            for t in targets {
                pairs.push((*c, t, self.relations.get(*c, t)));
            }
        }
        let alliances: BTreeSet<i64> = bots.iter().filter_map(|c| self.corps[c].alliance).collect();
        for &al in &alliances {
            let members = self.viewers(al);
            let targets: BTreeSet<i64> = self
                .relations
                .r
                .keys()
                .filter(|(v, _)| members.contains(v))
                .map(|(_, t)| *t)
                .chain(
                    self.stances
                        .held
                        .keys()
                        .filter(|(o, _)| *o == al)
                        .map(|(_, t)| *t),
                )
                .chain(
                    self.stances
                        .graded
                        .keys()
                        .filter(|(o, _)| *o == al)
                        .map(|(_, t)| *t),
                )
                .filter(|t| *t != al && !members.contains(t))
                .collect();
            for t in targets {
                pairs.push((al, t, self.alliance_relation(al, t)));
            }
        }
        let mut owners: BTreeSet<i64> = bots.iter().copied().collect();
        owners.extend(alliances.iter().copied());
        // R-REL-3: the graded reading of the same pairs, from the composed
        // views (a corp's C_eff, an alliance's A_eff + K).
        let pairs5: Vec<(i64, i64, f64)> = self.with_layers(now, |l| {
            pairs
                .iter()
                .map(|&(o, t, _)| {
                    let r5 = if society::PLAYER_ALLIANCES.contains(&o) {
                        relations::a_eff_k(l, o, t)
                    } else {
                        let v = relations::Viewer {
                            corp: o,
                            alliance: self.corps.get(&o).and_then(|c| c.alliance),
                        };
                        relations::c_eff(l, &v, t)
                    };
                    (o, t, r5)
                })
                .collect()
        });
        let flip = self.temper.flip_min_days;
        let temper = self.temper;
        for (o, t, r) in pairs {
            if let Some((from, to)) = self.stances.settle(o, t, r, now, self.day) {
                let line = format!(
                    "stance {o} -> {t}: {} -> {} (R {r:.1})",
                    from.name(),
                    to.name()
                );
                self.act(line.clone());
                lines.push(line);
            }
        }
        for (o, t, r) in pairs5 {
            if let Some((from, to)) = self
                .stances
                .settle5_with(o, t, r, now, self.day, flip, &temper)
            {
                let line = format!(
                    "stance5 {o} -> {t}: {} -> {} (R {r:.1})",
                    from.name(),
                    to.name()
                );
                self.act(line.clone());
                lines.push(line);
            }
        }
        if self.stances.prune(now, self.day, |o| owners.contains(&o)) > 0 {
            self.dirty = true;
        }
        // R-REL-5: the blocs' enemy lists, courting and expulsions.
        lines.extend(self.coalition_tick(now));
        lines
    }

    /// CO-FOUND: the pool's candidates: corpless bots whose traits and the
    /// corp count would have them found, were their wallet at the line.
    pub fn pool_cands(&self) -> Vec<crate::cofound::Cand> {
        let bots = self.bot_corps();
        self.members
            .iter()
            .filter(|(_, m)| !society::is_player_corp(m.corp))
            .filter(|(_, m)| {
                let mine = Profile::of(&m.career, m.band);
                let matching = bots.iter().any(|c| self.corps[c].profile == mine);
                society::wants_to_found(
                    &m.traits,
                    crate::cofound::line(),
                    CORP_FEE,
                    matching,
                    bots.len(),
                )
            })
            .map(|(&id, m)| crate::cofound::Cand {
                id,
                diligence: m.traits.get(Trait::Diligence),
                aggression: m.traits.get(Trait::Aggression),
            })
            .collect()
    }

    /// **What `me` does this turn** (pure). `wallet` is `None` when it
    /// could not be read; `grudges` the bot's own (group -> score).
    pub fn plan(
        &self,
        me: i64,
        now: u64,
        wallet: Option<f64>,
        grudges: &BTreeMap<i64, f64>,
    ) -> Vec<Plan> {
        let Some(m) = self.members.get(&me).cloned() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // CO-FOUND: the one decision point. A refund goes first, from
        // wherever the founder now is; a pledge is the corpless bot's
        // turn (below, after its patience), and a pledger of a founded
        // corp applies where it paid.
        let pool_step =
            crate::cofound::step(&self.pool, &self.pool_cands(), me, wallet, now, self.day);
        if pool_step == Some(crate::cofound::Step::Refund) {
            out.push(Plan::Refund);
        }
        if !society::is_player_corp(m.corp) {
            out.push(Plan::Offers);
            let patience = (society::ALLY_DAYS.min(APPLY_PATIENCE_DAYS) * self.day as f64) as u64;
            if m.applied
                .is_some_and(|(_, at)| now.saturating_sub(at) < patience)
            {
                return out;
            }
            if let Some(&corp) = self.pool.pledged.get(&me)
                && self.corps.contains_key(&corp)
            {
                out.push(Plan::Apply(corp));
                return out;
            }
            if let Some(crate::cofound::Step::Pledge { to, amount }) = pool_step {
                out.push(Plan::Pledge { to, amount });
                return out;
            }
            let bots = self.bot_corps();
            let mine = Profile::of(&m.career, m.band);
            let matching = bots.iter().any(|c| self.corps[c].profile == mine);
            if wallet.is_some_and(|w| {
                society::wants_to_found(&m.traits, w, CORP_FEE, matching, bots.len())
            }) {
                out.push(Plan::Found);
            } else if society::wants_to_join(&m.traits) {
                let offers: Vec<Offer> = bots
                    .iter()
                    .map(|c| {
                        let corp = &self.corps[c];
                        Offer {
                            corp: *c,
                            fit: society::fit(corp.profile, &m.career, m.band),
                            grudge: grudges.get(&self.group_of_corp(*c)).copied().unwrap_or(0.0),
                            size: self.size(*c),
                            enemy: self.stances.get(*c, me) == Stance::Red,
                        }
                    })
                    .collect();
                if let Some(c) = society::best_corp(&offers) {
                    out.push(Plan::Apply(c));
                }
            }
            return out;
        }
        let Some(corp) = self.corps.get(&m.corp).cloned() else {
            return out;
        };
        if corp.ceo != Some(me) {
            // A member: its satisfaction is [`HallState::sample`]'s. BOT-CORP:
            // a builder, trader or courier also tends the corp hangar.
            if (crate::corp_ops::has_stock_pass(&m.career) || self.draws_kit(me, &m.career))
                && crate::corp_ops::due(
                    self.stock_tried.get(&me).copied(),
                    now,
                    crate::corp_ops::STOCK_EVERY_DAYS,
                    self.day,
                )
            {
                out.push(Plan::Stock);
            }
            return out;
        }
        out.push(Plan::Recruit);
        out.push(Plan::Contacts {
            owner: corp.id,
            alliance: false,
        });
        if corp.alliance.is_none() && self.war_pass_due(corp.id, now) {
            out.push(Plan::Wars {
                owner: corp.id,
                alliance: false,
            });
        }
        out.extend(self.invitations(corp.id, now).into_iter().map(Plan::Invite));
        // CORP-O: the one decision point for an office. The CEO asks the
        // server (a quote, the corp wallet, the corp's offices) on its own
        // connection; the hall only keeps the cadence.
        let office_wait = (OFFICE_EVERY_DAYS * self.day as f64) as u64;
        if self
            .office_tried
            .get(&corp.id)
            .is_none_or(|t| now.saturating_sub(*t) >= office_wait)
        {
            out.push(Plan::Office);
        }
        // BOT-CORP: the one decision point for tax, roles and pay.
        if crate::corp_ops::due(
            self.ops_tried.get(&corp.id).copied(),
            now,
            crate::corp_ops::OPS_EVERY_DAYS,
            self.day,
        ) {
            out.push(Plan::Ops);
        }
        // DOC-6: with a doctrine, ship replacement, staging and the stock.
        if self.doctrine_key_of(corp.id).is_some() {
            let srp = &self.stock.srp_tried;
            if crate::corp_ops::due(
                srp.get(&corp.id).copied(),
                now,
                crate::doctrine_stock::SRP_EVERY_DAYS,
                self.day,
            ) {
                out.push(Plan::Srp);
            }
            let stage = &self.stock.stage_tried;
            if crate::corp_ops::due(
                stage.get(&corp.id).copied(),
                now,
                crate::doctrine_stock::STAGE_EVERY_DAYS,
                self.day,
            ) {
                out.push(Plan::Stage);
            }
        }
        let views: Vec<CorpView> = self
            .bot_corps()
            .iter()
            .map(|c| CorpView {
                id: *c,
                members: self.size(*c),
                alliance: self.corps[c].alliance,
                aggression: self.corps[c]
                    .ceo
                    .and_then(|ceo| self.members.get(&ceo))
                    .map_or(0.5, |m| m.traits.get(Trait::Aggression)),
            })
            .collect();
        let known = self.alliances.len();
        for a in society::alliance_acts(&views, &self.stances, known, now, self.day) {
            match a {
                AllianceAct::Found { corp: c } if c == corp.id => out.push(Plan::FoundAlliance),
                AllianceAct::Apply { corp: c, alliance }
                    if c == corp.id
                        && corp.applied_alliance.is_none_or(|(al, at)| {
                            al != alliance
                                || now.saturating_sub(at)
                                    >= (APPLY_PATIENCE_DAYS * self.day as f64) as u64
                        }) =>
                {
                    out.push(Plan::ApplyAlliance(alliance));
                }
                AllianceAct::Leave { corp: c, alliance } if c == corp.id => {
                    out.push(Plan::LeaveAlliance(alliance));
                }
                _ => {}
            }
        }
        if let Some(al) = corp.alliance {
            let reread = (EXECUTOR_REREAD_DAYS * self.day as f64) as u64;
            let ex = self.alliances.get(&al).and_then(|a| a.executor);
            // A cached executor that is no longer a member, or is old, is
            // read again.
            let stale = ex.is_some_and(|e| {
                self.corps.get(&e).is_some_and(|c| c.alliance != Some(al))
                    || self
                        .alliances
                        .get(&al)
                        .and_then(|a| a.read_at)
                        .is_none_or(|at| now.saturating_sub(at) >= reread)
            });
            if stale {
                out.push(Plan::ReadExecutor(al));
            }
            match ex {
                Some(ex) if ex == corp.id && !stale => {
                    out.push(Plan::AnswerAlliance(al));
                    out.push(Plan::Contacts {
                        owner: al,
                        alliance: true,
                    });
                    if self.war_pass_due(al, now) {
                        out.push(Plan::Wars {
                            owner: al,
                            alliance: true,
                        });
                    }
                }
                Some(_) => {}
                None => out.push(Plan::ReadExecutor(al)),
            }
        }
        out
    }

    /// **A member's satisfaction** this turn, and what it does when it is
    /// done with its corp: a member leaves ([`Plan::Leave`]); a CEO hands
    /// the seat on ([`Plan::Resign`], SOC-C3's `ResignFromCEO`), but only
    /// while another member is there to take it -- a bot CEO never closes
    /// its corp **[A]** (falsified by the owner wanting empty corps closed).
    pub fn sample(&mut self, me: i64, mood: &Mood, now: u64) -> Option<Plan> {
        let day = self.day;
        let corp = self.members.get(&me)?.corp;
        if !society::is_player_corp(corp) {
            return None;
        }
        let ceo = self.corps.get(&corp).and_then(|c| c.ceo) == Some(me);
        let size = self.size(corp);
        let m = self.members.get_mut(&me)?;
        let loyalty = m.traits.get(Trait::Loyalty);
        if !m
            .satisfaction
            .update(society::mood_sample(mood), loyalty, now, day)
        {
            return None;
        }
        match (ceo, size) {
            (false, _) => Some(Plan::Leave),
            (true, n) if n >= 2 => Some(Plan::Resign),
            (true, _) => None,
        }
    }

    /// The contacts `owner` should hold: its stances that are not neutral,
    /// toward anyone outside its own corp and alliance.
    pub fn desired_contacts(&self, owner: i64) -> BTreeMap<i64, f64> {
        let own_alliance = if society::PLAYER_ALLIANCES.contains(&owner) {
            Some(owner)
        } else {
            self.corps.get(&owner).and_then(|c| c.alliance)
        };
        // R-REL-3: the graded stance where there is one, else the coarse.
        let targets: BTreeSet<i64> = self
            .stances
            .held
            .keys()
            .chain(self.stances.graded.keys())
            .filter(|(o, _)| *o == owner)
            .map(|(_, t)| *t)
            .collect();
        targets
            .into_iter()
            .filter(|t| !self.inside(*t, own_alliance))
            .filter_map(|t| self.stances.get5(owner, t).standing().map(|s| (t, s)))
            .collect()
    }

    /// Is `t` the alliance or one of its member corps?
    fn inside(&self, t: i64, al: Option<i64>) -> bool {
        al.is_some_and(|al| t == al || self.corps.get(&t).and_then(|c| c.alliance) == Some(al))
    }

    /// REVIEW-FIX2: **the claims on `owner`'s book** as one [`ContactPlan`]:
    /// the graded tier stances ([`HallState::desired_contacts`]), those among
    /// them with a coalition partner as [`Source::Coalition`] (a bloc's
    /// members hold each other at +10, R-REL-5), and, for a pilot's own
    /// book, the `--friends` floor.
    pub fn contact_plan(&self, owner: i64, now: u64) -> crate::contact_plan::ContactPlan {
        use crate::contact_plan::{ContactPlan, Source};
        let mut plan = ContactPlan::new();
        let blocs = self.blocs(now);
        let partner = |t: i64| {
            blocs
                .iter()
                .any(|b| b.members.contains_key(&owner) && b.members.contains_key(&t))
        };
        for (t, s) in self.desired_contacts(owner) {
            let source = if s > 0.0 && partner(t) {
                Source::Coalition
            } else {
                Source::Tier
            };
            plan.claim(owner, t, s, source);
        }
        if crate::effects::is_character(owner) {
            for (t, s) in &self.friends {
                plan.claim(owner, *t, *s, Source::Friends);
            }
        }
        plan
    }

    /// **The contact calls** that bring `actual` (the book as read) to
    /// what `owner` should hold: `(contact, Some(standing))` to add or edit,
    /// `(contact, None)` to remove. Only a contact a bot set is removed, and
    /// never one on the owner's own alliance.
    pub fn contact_diff(
        &self,
        owner: i64,
        actual: &BTreeMap<i64, f64>,
        now: u64,
    ) -> Vec<(i64, Option<f64>)> {
        let own_alliance = if society::PLAYER_ALLIANCES.contains(&owner) {
            Some(owner)
        } else {
            self.corps.get(&owner).and_then(|c| c.alliance)
        };
        self.contact_plan(owner, now).diff(owner, actual, &|t| {
            self.managed.contains(&(owner, t)) && !self.inside(t, own_alliance)
        })
    }

    /// The CEO accepts `applicant` into `corp` (§3.2): a bot by its fit,
    /// anyone else at [`society::ACCEPT_FIT`]; never one of the corp's
    /// enemies, never past the cap.
    pub fn accepts(&self, corp: i64, applicant: i64) -> bool {
        self.accepts_with(corp, applicant, None)
    }

    /// [`HallState::accepts`] with the CEO's own `R_eff` toward the
    /// applicant when it is known (R-REL-3, R-RELATIONS §3.2): a pilot the
    /// CEO has come to like is taken at the friendly bar, one it distrusts
    /// at the wary one, an enemy never.
    pub fn accepts_with(&self, corp: i64, applicant: i64, r_eff: Option<f64>) -> bool {
        let Some(c) = self.corps.get(&corp) else {
            return false;
        };
        let paid = self.pool.pledged.get(&applicant) == Some(&corp);
        let (fit, group) = match self.members.get(&applicant) {
            Some(m) => (
                // CO-FOUND: a pledger paid for this corp; its fit is full.
                if paid {
                    1.0
                } else {
                    society::fit(c.profile, &m.career, m.band)
                },
                if society::is_player_corp(m.corp) {
                    self.group_of_corp(m.corp)
                } else {
                    applicant
                },
            ),
            None => (society::ACCEPT_FIT, applicant),
        };
        let red = self.stances.get(corp, group) == Stance::Red
            || c.alliance
                .is_some_and(|al| self.stances.get(al, group) == Stance::Red);
        // The settled stance of the corp and of its alliance, the worse of
        // the two; the CEO's own reading can only move a neutral stance.
        let seen: Vec<Stance5> = [Some(corp), c.alliance]
            .into_iter()
            .flatten()
            .map(|o| self.stances.get5(o, group))
            .collect();
        let held = if seen.contains(&Stance5::Hostile) {
            Stance5::Hostile
        } else if seen.contains(&Stance5::Wary) {
            Stance5::Wary
        } else {
            seen.into_iter().max().unwrap_or(Stance5::Neutral)
        };
        let stance = match (held, r_eff.map(effects::stance_of)) {
            (Stance5::Neutral, Some(s)) => s,
            // Only distrust overrides a friendlier settled stance.
            (h, Some(s)) if s < h && s <= Stance5::Wary => s,
            (h, _) => h,
        };
        if red || stance == Stance5::Hostile {
            return false;
        }
        let base = society::ceo_accepts(fit, self.size(corp), false);
        if !matches!(stance, Stance5::Friendly | Stance5::Allied) {
            return effects::accepts(stance, fit, base);
        }
        // A friend still finds the door shut on a full corp.
        effects::accepts(stance, fit, base) && self.size(corp) < society::CORP_SIZE_CAP
    }

    /// The executor accepts `corp` into `al` when the executor corp is
    /// blue toward it.
    pub fn alliance_accepts(&self, al: i64, corp: i64) -> bool {
        let Some(ex) = self.alliances.get(&al).and_then(|a| a.executor) else {
            return false;
        };
        self.stances.get(ex, corp) == Stance::Blue || self.stances.get(al, corp) == Stance::Blue
    }

    /// R-REL-3: **the pilots `corp`'s CEO may invite this turn**: seen
    /// humans with a corp that is no player corp, at least
    /// [`effects::INVITE_FLEET_HOURS`] of shared fleet time, not invited in
    /// the last [`effects::INVITE_EVERY_DAYS`]. At most one; the CEO checks
    /// its own `R_eff` ([`effects::may_invite`]) before it sends.
    pub fn invitations(&self, corp: i64, now: u64) -> Vec<i64> {
        let wait = (effects::INVITE_EVERY_DAYS * self.day as f64) as u64;
        self.humans
            .iter()
            .filter(|(_, s)| !society::is_player_corp(s.corp))
            .filter(|(id, _)| {
                self.fleet_hours.get(id).copied().unwrap_or(0.0) >= effects::INVITE_FLEET_HOURS
            })
            .filter(|(id, _)| {
                self.invited
                    .get(&(corp, **id))
                    .is_none_or(|at| now.saturating_sub(*at) >= wait)
            })
            .map(|(id, _)| *id)
            .take(1)
            .collect()
    }

    /// R-REL-3: [`HallState::alliance_accepts`], and also a corp no bot
    /// runs (a friend's) whose standing with the executor reaches
    /// [`effects::alliance_admits_human`] -- so a friend corp can join a bot
    /// alliance and a hostile one cannot.
    pub fn alliance_accepts_at(&self, al: i64, corp: i64, now: u64) -> bool {
        if self.alliance_accepts(al, corp) {
            return true;
        }
        let Some(ex) = self.alliances.get(&al).and_then(|a| a.executor) else {
            return false;
        };
        if self.bot_corps().contains(&corp) {
            return false;
        }
        if matches!(
            self.stances.get5(ex, corp).min(self.stances.get5(al, corp)),
            Stance5::Hostile | Stance5::Wary
        ) {
            return false;
        }
        let v = relations::Viewer {
            corp: ex,
            alliance: Some(al),
        };
        let c = self.with_layers(now, |l| relations::c_eff(l, &v, corp));
        effects::alliance_admits_human(c)
    }

    /// **The planner** (§3.4): the corp need of every career in `table`
    /// for `me`'s corp; empty when corpless.
    pub fn corp_need(&self, me: i64, table: &[Career]) -> BTreeMap<String, f64> {
        let Some(m) = self.members.get(&me) else {
            return BTreeMap::new();
        };
        let Some(c) = self.corps.get(&m.corp) else {
            return BTreeMap::new();
        };
        let careers: Vec<&str> = self
            .members
            .values()
            .filter(|x| x.corp == c.id)
            .map(|x| x.career.as_str())
            .collect();
        let names: Vec<&str> = table.iter().map(|c| c.name).collect();
        society::corp_need(c.profile.kind, &careers, &names)
    }

    /// The `[soc]` fields: bot corps, alliances with a bot corp, and the
    /// blue and red pairs.
    pub fn summary(&self) -> String {
        let bots = self.bot_corps();
        let alliances: BTreeSet<i64> = bots.iter().filter_map(|c| self.corps[c].alliance).collect();
        society::summary_fields(
            bots.len(),
            alliances.len(),
            self.stances.count(Stance::Blue),
            self.stances.count(Stance::Red),
        ) + &society::day_field(self.game_day())
    }

    /// SOC-WIRE: the game day the matrix was last advanced to (`0` before
    /// its first tick). [I] `Relations::at` is the unix second the bots'
    /// `tick` passed; falsified by a `day` field that does not advance once
    /// a soak runs past a game day.
    pub fn game_day(&self) -> u64 {
        self.relations.at.map_or(0, |t| t / self.day.max(1))
    }

    /// [`society::hostile_to`] over the books the bots read.
    pub fn hostile_to(&self, me: Affiliation, other: Affiliation) -> bool {
        society::hostile_to_with(
            me,
            other,
            |o, t| self.books.get(&(o, t)).copied(),
            |c| self.friends.get(&c).copied(),
        )
    }

    // ---- the state file ----------------------------------------------------------

    /// The file's text.
    pub fn render(&self) -> String {
        let mut s = format!("soc\t{STATE_VERSION}\n");
        let mut rows = 0usize;
        let opt = |v: Option<i64>| v.map_or("-".to_owned(), |v| v.to_string());
        for c in self.corps.values() {
            s += &format!(
                "corp\t{}\t{}\t{}\t{}\t{}\t{}\n",
                c.id,
                opt(c.ceo),
                c.profile.name(),
                c.founded,
                opt(c.alliance),
                c.name.replace(['\t', '\n'], " ")
            );
            rows += 1;
        }
        for (id, a) in &self.alliances {
            s += &format!("alliance\t{id}\t{}\n", opt(a.executor));
            rows += 1;
        }
        for (&(v, t), r) in &self.relations.r {
            s += &format!("rel\t{v}\t{t}\t{r}\n");
            rows += 1;
        }
        for (&(o, t), h) in &self.stances.held {
            s += &format!(
                "stance\t{o}\t{t}\t{}\t{}\n",
                h.stance.name(),
                h.since.map_or("-".to_owned(), |v| v.to_string())
            );
            rows += 1;
        }
        for (&(o, t), h) in &self.stances.graded {
            s += &format!(
                "stance5\t{o}\t{t}\t{}\t{}\n",
                h.stance.name(),
                h.since.map_or("-".to_owned(), |v| v.to_string())
            );
            rows += 1;
        }
        for &(o, t) in &self.managed {
            s += &format!("managed\t{o}\t{t}\n");
            rows += 1;
        }
        for (&(v, t), r) in &self.alliance_own.r {
            s += &format!("aown\t{v}\t{t}\t{r}\n");
            rows += 1;
        }
        for &(k, w, i) in &self.observed {
            s += &format!("obs\t{k}\t{w}\t{i}\n");
            rows += 1;
        }
        // R-REL-4: the humans the bots know, the hours flown with them and
        // the tier each corp last told them.
        for (id, h) in &self.humans {
            s += &format!("human\t{id}\t{}\t{}\t{}\n", h.corp, opt(h.alliance), h.at);
            rows += 1;
        }
        for (id, h) in &self.fleet_hours {
            s += &format!("fhours\t{id}\t{h}\n");
            rows += 1;
        }
        for (&(c, h), (tier, at)) in &self.tiers {
            s += &format!("htier\t{c}\t{h}\t{}\t{at}\n", tier.name());
            rows += 1;
        }
        for k in &self.recorded {
            s += &format!("rec\t{k}\n");
            rows += 1;
        }
        for k in &self.kills {
            s += &format!("kill\t{k}\n");
            rows += 1;
        }
        // REVIEW-FIX2: the day's SRP total and the procurement float survive
        // a restart (a restart no longer resets the daily cap or strands a
        // float in a member's wallet).
        for (c, (d, isk)) in &self.stock.paid_today {
            s += &format!("srpday\t{c}\t{d}\t{isk}\n");
            rows += 1;
        }
        for (m, isk) in &self.stock.float {
            s += &format!("float\t{m}\t{isk}\n");
            rows += 1;
        }
        for k in &self.stock.paid {
            s += &format!("srp\t{k}\n");
            rows += 1;
        }
        for (g, b) in &self.dbooks {
            let (rk, ru) = b
                .retiring
                .as_ref()
                .map_or(("-".to_owned(), 0), |(k, u)| (k.clone(), *u));
            s += &format!(
                "dbook\t{g}\t{}\t{}\t{}\t{rk}\t{ru}\t{}\n",
                b.current.as_deref().unwrap_or("-"),
                b.since,
                opt(b.last_switch.map(|v| v as i64)),
                b.fancy.line(),
            );
            rows += 1;
        }
        let pool = &self.pool;
        s += &format!("pool\t{}\t{}\n", opt(pool.founder), pool.since);
        rows += 1;
        for p in &pool.pledges {
            s += &format!("pledge\t{}\t{}\t{}\t{}\n", p.from, p.to, p.amount, p.at);
            rows += 1;
        }
        for (who, corp) in &pool.pledged {
            s += &format!("pledged\t{who}\t{corp}\n");
            rows += 1;
        }
        for e in &pool.journal {
            s += &format!(
                "pjournal\t{}\t{}\t{}\t{}\t{}\n",
                e.at,
                e.kind.name(),
                e.from,
                e.to,
                e.amount
            );
            rows += 1;
        }
        s += &format!(
            "clock\t{}\t{}\n",
            opt(self.relations.at.map(|v| v as i64)),
            opt(self.last_daily.map(|v| v as i64))
        );
        rows += 1;
        s += &format!("end\t{rows}\n");
        s
    }

    /// Read a file [`HallState::render`] wrote into a hall of `day`;
    /// `Err(why)` when torn.
    pub fn parse(text: &str, day: u64) -> std::result::Result<HallState, String> {
        let mut st = HallState::new(day);
        let mut rows = 0usize;
        let mut ended = false;
        let int = |s: &str| s.parse::<i64>().map_err(|e| format!("{s}: {e}"));
        let opt = |s: &str| {
            if s == "-" {
                Ok(None)
            } else {
                s.parse::<i64>().map(Some).map_err(|e| format!("{s}: {e}"))
            }
        };
        for (i, line) in text.lines().enumerate() {
            let f: Vec<&str> = line.split('\t').collect();
            let need = |n: usize| {
                if f.len() < n {
                    Err(format!("line {}: {} fields, want {n}", i + 1, f.len()))
                } else {
                    Ok(())
                }
            };
            match f[0] {
                "soc" if i == 0 => {
                    need(2)?;
                    if f[1] != STATE_VERSION {
                        return Err(format!("version {}", f[1]));
                    }
                    continue;
                }
                "corp" => {
                    need(7)?;
                    let id = int(f[1])?;
                    st.corps.insert(
                        id,
                        CorpMind {
                            id,
                            ceo: opt(f[2])?,
                            profile: Profile::parse(f[3]).ok_or(format!("profile {}", f[3]))?,
                            founded: int(f[4])? as u64,
                            alliance: opt(f[5])?,
                            name: f[6].to_owned(),
                            applied_alliance: None,
                            salt: 0,
                        },
                    );
                }
                "alliance" => {
                    need(3)?;
                    st.alliances.insert(
                        int(f[1])?,
                        AllianceMind {
                            executor: opt(f[2])?,
                            ..AllianceMind::default()
                        },
                    );
                }
                "rel" => {
                    need(4)?;
                    let r: f64 = f[3].parse().map_err(|e| format!("{}: {e}", f[3]))?;
                    st.relations.r.insert((int(f[1])?, int(f[2])?), r);
                }
                "aown" => {
                    need(4)?;
                    let r: f64 = f[3].parse().map_err(|e| format!("{}: {e}", f[3]))?;
                    st.alliance_own.r.insert((int(f[1])?, int(f[2])?), r);
                }
                "obs" => {
                    need(4)?;
                    st.observed
                        .insert((int(f[1])? as u8, int(f[2])?, int(f[3])?));
                }
                "human" => {
                    need(5)?;
                    st.humans.insert(
                        int(f[1])?,
                        Seen {
                            corp: int(f[2])?,
                            alliance: opt(f[3])?,
                            at: int(f[4])? as u64,
                        },
                    );
                }
                "fhours" => {
                    need(3)?;
                    let h: f64 = f[2].parse().map_err(|e| format!("{}: {e}", f[2]))?;
                    st.fleet_hours.insert(int(f[1])?, h);
                }
                "htier" => {
                    need(5)?;
                    let tier = Stance5::parse(f[3]).ok_or(format!("htier {}", f[3]))?;
                    st.tiers
                        .insert((int(f[1])?, int(f[2])?), (tier, int(f[4])? as u64));
                }
                "stance" => {
                    need(5)?;
                    let stance = [Stance::Neutral, Stance::Blue, Stance::Red]
                        .into_iter()
                        .find(|s| s.name() == f[3])
                        .ok_or(format!("stance {}", f[3]))?;
                    st.stances.held.insert(
                        (int(f[1])?, int(f[2])?),
                        society::Held {
                            stance,
                            since: opt(f[4])?.map(|v| v as u64),
                        },
                    );
                }
                "stance5" => {
                    need(5)?;
                    let stance = Stance5::parse(f[3]).ok_or(format!("stance5 {}", f[3]))?;
                    st.stances.graded.insert(
                        (int(f[1])?, int(f[2])?),
                        society::Held5 {
                            stance,
                            since: opt(f[4])?.map(|v| v as u64),
                        },
                    );
                }
                "managed" => {
                    need(3)?;
                    st.managed.insert((int(f[1])?, int(f[2])?));
                }
                "dbook" => {
                    need(8)?;
                    let fancy = crate::doctrine_lead::Fancy::parse(f[7])
                        .ok_or(format!("dbook fancy {}", f[7]))?;
                    let mut b = crate::doctrine_lead::DoctrineBook::new(fancy);
                    b.current = (f[2] != "-").then(|| f[2].to_owned());
                    b.since = int(f[3])? as u64;
                    b.last_switch = opt(f[4])?.map(|v| v as u64);
                    if f[5] != "-" {
                        b.retiring = Some((f[5].to_owned(), int(f[6])? as u64));
                    }
                    st.dbooks.insert(int(f[1])?, b);
                }
                "kill" => {
                    need(2)?;
                    st.kills.insert(int(f[1])?);
                    while st.kills.len() > KILLS_KEEP {
                        st.kills.pop_first();
                    }
                }
                "srp" => {
                    need(2)?;
                    st.stock.paid.insert(int(f[1])?);
                    while st.stock.paid.len() > crate::doctrine_stock::SRP_KEEP {
                        st.stock.paid.pop_first();
                    }
                }
                "srpday" => {
                    need(4)?;
                    let isk: f64 = f[3].parse().map_err(|e| format!("{}: {e}", f[3]))?;
                    st.stock
                        .paid_today
                        .insert(int(f[1])?, (int(f[2])? as u64, isk));
                }
                "float" => {
                    need(3)?;
                    let isk: f64 = f[2].parse().map_err(|e| format!("{}: {e}", f[2]))?;
                    st.stock.float.insert(int(f[1])?, isk);
                }
                "rec" => {
                    need(2)?;
                    st.recorded.insert(int(f[1])?);
                }
                "pool" => {
                    need(3)?;
                    st.pool.founder = opt(f[1])?;
                    st.pool.since = int(f[2])? as u64;
                }
                "pledge" => {
                    need(5)?;
                    st.pool.pledges.push(crate::cofound::Pledge {
                        from: int(f[1])?,
                        to: int(f[2])?,
                        amount: f[3].parse().map_err(|e| format!("{}: {e}", f[3]))?,
                        at: int(f[4])? as u64,
                    });
                }
                "pledged" => {
                    need(3)?;
                    st.pool.pledged.insert(int(f[1])?, int(f[2])?);
                }
                "pjournal" => {
                    need(6)?;
                    st.pool.journal.push(crate::cofound::Entry {
                        at: int(f[1])? as u64,
                        kind: crate::cofound::Kind::parse(f[2]).ok_or(format!("kind {}", f[2]))?,
                        from: int(f[3])?,
                        to: int(f[4])?,
                        amount: f[5].parse().map_err(|e| format!("{}: {e}", f[5]))?,
                    });
                }
                "clock" => {
                    need(3)?;
                    st.relations.at = opt(f[1])?.map(|v| v as u64);
                    st.last_daily = opt(f[2])?.map(|v| v as u64);
                }
                "end" => {
                    need(2)?;
                    if int(f[1])? as usize != rows {
                        return Err(format!("end says {}, read {rows}", f[1]));
                    }
                    ended = true;
                    break;
                }
                other => return Err(format!("line {}: unknown row {other:?}", i + 1)),
            }
            rows += 1;
        }
        if !ended {
            return Err("no end row".to_owned());
        }
        Ok(st)
    }
}

impl Hall {
    /// A hall for game-days of `day` seconds, in memory only.
    pub fn new(day: u64) -> Hall {
        Hall {
            inner: Mutex::new(HallState::new(day)),
        }
    }

    /// A hall read back from `dir/society.tsv`, or new when it is missing
    /// or torn (`Err(why)` beside the new hall says which).
    pub fn open(dir: &Path, day: u64) -> (Hall, std::result::Result<(), String>) {
        let path = dir.join(STATE_FILE);
        let (mut st, said) = match std::fs::read_to_string(&path) {
            Ok(text) => match HallState::parse(&text, day) {
                Ok(st) => (st, Ok(())),
                Err(why) => {
                    // REVIEW-FIX: never overwrite what cannot be read (a
                    // newer file, a torn write): keep it as `.bad`.
                    let mut bad = dir.join(format!("{STATE_FILE}.bad"));
                    let mut n = 1;
                    while bad.exists() {
                        bad = dir.join(format!("{STATE_FILE}.bad{n}"));
                        n += 1;
                    }
                    let kept = match std::fs::rename(&path, &bad) {
                        Ok(()) => format!("kept as {}", bad.display()),
                        Err(e) => format!("could not keep it: {e}"),
                    };
                    (HallState::new(day), Err(format!("{why} ({kept})")))
                }
            },
            Err(_) => (HallState::new(day), Ok(())),
        };
        st.state = Some(dir.to_path_buf());
        (
            Hall {
                inner: Mutex::new(st),
            },
            said,
        )
    }

    /// DOC-4: the same hall with the doctrine layer awake: the catalog and
    /// its matrix, the difficulty `level` (0 peaceful .. 4 brutal) and the
    /// society seed the leaders' fancies are drawn under.
    pub fn with_doctrine(
        self,
        world: Arc<crate::doctrine_hall::DoctrineWorld>,
        level: u8,
        seed: u64,
    ) -> Hall {
        {
            let mut st = self.lock();
            st.doctrine = Some(world);
            st.level = level;
            st.knobs = crate::difficulty::Knobs::builtin(level);
            st.doctrine_seed = seed;
        }
        self
    }

    /// DOC-8: the run's difficulty (`eve-bots --difficulty`): the level, the
    /// relation temper and the doctrine layer's knobs, one set from
    /// [`crate::difficulty::Difficulty`]. After [`Hall::with_doctrine`].
    pub fn with_difficulty(self, d: &crate::difficulty::Difficulty) -> Hall {
        {
            let mut st = self.lock();
            let k = d.knobs();
            st.level = d.level();
            st.temper = k.temper;
            st.knobs = k;
        }
        self
    }

    /// DOC-4: one battle tick (`BATTLE_TICK_S`) for every leader; the `[doc]`
    /// lines. See [`HallState::theatre_tick`].
    pub fn theatre_tick(&self, now: u64, board: &crate::doctrine_sense::Board) -> Vec<String> {
        self.lock().theatre_tick(now, board)
    }

    /// DOC-4: the commander's pending actions (cleared).
    pub fn take_directives(&self, commander: i64) -> Vec<crate::doctrine_theatre::Action> {
        self.lock().take_directives(commander)
    }

    /// DOC-4/5: a bot reports its readiness.
    pub fn report_readiness(&self, char_id: i64, r: crate::doctrine_hall::Readiness) {
        self.lock().report_readiness(char_id, r);
    }

    /// The same hall with turns `every` game seconds (tests use 0).
    pub fn with_every(self, every: u64) -> Hall {
        self.lock().every = every;
        self
    }

    /// How often a bot takes a social turn.
    pub fn every(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.lock().every)
    }

    /// The lock; a poisoned one is taken anyway (the state is plain data).
    pub fn lock(&self) -> MutexGuard<'_, HallState> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A copy of the state, for tests and `[soc]`.
    pub fn snapshot(&self) -> HallState {
        self.lock().clone()
    }

    /// Fold an event in (C7's fights, tests).
    pub fn record(&self, e: &Event) {
        self.lock().record(e);
    }

    /// Fold a kill report in once, whoever reads it first (SOC-C5,
    /// [`HallState::record_kill`]).
    pub fn record_kill(&self, id: i64, e: &Event) -> bool {
        self.lock().record_kill(id, e)
    }

    /// The `[soc]` line's society fields.
    pub fn summary(&self) -> String {
        self.lock().summary()
    }

    /// REL-FIX: count `char_id` a friend at `standing` (the `--friends`
    /// file, [`crate::friends`]).
    pub fn befriend(&self, char_id: i64, standing: f64) {
        self.lock().friends.insert(char_id, standing);
    }

    /// R-REL-2: the hall reads kills against `ground` (systems and hulls).
    pub fn with_ground(self, ground: Arc<crate::hunt::Ground>) -> Hall {
        self.lock().ground = Some(ground);
        self
    }

    /// R-REL-5: `--society-wars` (bots declare and invite) and `--hostile`
    /// (a group of humans may be declared on).
    pub fn with_wars(self, enabled: bool, humans: bool) -> Hall {
        {
            let mut st = self.lock();
            st.coal.enabled = enabled;
            st.coal.humans = humans;
        }
        self
    }

    /// R-REL-5: the coalitions now (`Bloc { members, weight, lead }`).
    pub fn blocs(&self, now: u64) -> Vec<relations::Bloc> {
        self.lock().blocs(now)
    }

    /// R-REL-2: the difficulty the facts are weighed at.
    pub fn with_temper(self, temper: Temper) -> Hall {
        self.lock().temper = temper;
        self
    }

    /// **For SOC-C5**: may `me` hunt `other`? Anyone not allied.
    pub fn hostile_to(&self, me: Affiliation, other: Affiliation) -> bool {
        self.lock().hostile_to(me, other)
    }

    /// `signals` with `me`'s corp need multiplied in (§3.4).
    pub fn with_need(&self, me: i64, table: &[Career], mut signals: Signals) -> Signals {
        for (c, n) in self.lock().corp_need(me, table) {
            *signals.corp_need.entry(c).or_insert(1.0) *= n;
        }
        signals
    }

    /// Write the state file when something changed and there is a
    /// directory: temp and rename.
    pub fn save(&self) -> std::io::Result<()> {
        let (dir, text) = {
            let mut st = self.lock();
            let Some(dir) = st.state.clone().filter(|_| st.dirty) else {
                return Ok(());
            };
            st.dirty = false;
            (dir, st.render())
        };
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!("{STATE_FILE}.tmp"));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, dir.join(STATE_FILE))
    }
}

// ---- names -------------------------------------------------------------------------

const FIRST: [&str; 16] = [
    "Iron", "Silent", "Amber", "Deep", "Bright", "Hollow", "Frozen", "Distant", "Brave", "Quiet",
    "Golden", "Ashen", "Lost", "Wild", "Pale", "Steady",
];
const SECOND: [&str; 16] = [
    "Drifters",
    "Haulers",
    "Forge",
    "Wardens",
    "Prospectors",
    "Lanterns",
    "Rangers",
    "Collective",
    "Syndicate",
    "Union",
    "Guild",
    "Company",
    "Works",
    "Outfit",
    "Cartel",
    "Circle",
];
const PACT: [&str; 8] = [
    "Pact",
    "Coalition",
    "Accord",
    "League",
    "Compact",
    "Covenant",
    "Assembly",
    "Alignment",
];

fn pick(h: u64, salt: u32) -> u64 {
    let mut x = h ^ u64::from(salt).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    crate::brain::xorshift(&mut x)
}

/// A corp name for `founder` (pure; `salt` after a taken name).
pub fn corp_name(founder: &str, salt: u32) -> String {
    let h = pick(crate::brain::name_hash(founder) | 1, salt);
    let base = format!(
        "{} {}",
        FIRST[(h % 16) as usize],
        SECOND[((h >> 8) % 16) as usize]
    );
    if salt >= 4 {
        format!("{base} {salt}")
    } else {
        base
    }
}

/// An alliance name for a corp named `corp` (pure).
pub fn alliance_name(corp: &str, salt: u32) -> String {
    let first = corp.split(' ').next().unwrap_or("Free");
    let h = pick(crate::brain::name_hash(corp) | 1, salt);
    let base = format!("{first} {}", PACT[(h % 8) as usize]);
    if salt >= 4 {
        format!("{base} {salt}")
    } else {
        base
    }
}

/// A ticker when the server suggests none: the name's initials and digits,
/// at most five.
fn fallback_ticker(name: &str, salt: u32) -> String {
    let mut t: String = name
        .split(' ')
        .filter_map(|w| w.chars().next())
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    t.push_str(&format!(
        "{}",
        (crate::brain::name_hash(name) + u64::from(salt)) % 100
    ));
    t.chars().take(5).collect()
}

// ---- the wire ----------------------------------------------------------------------

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    match crate::attributes(v) {
        PyValue::Dict(entries) => entries
            .iter()
            .find(|(_, k)| match k {
                PyValue::Bytes(b) => b == name.as_bytes(),
                PyValue::Unicode(s) => s == name,
                _ => false,
            })
            .map(|(value, _)| value),
        _ => None,
    }
}

fn as_i64(v: &PyValue) -> Option<i64> {
    match v {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn as_f64(v: &PyValue) -> Option<f64> {
    match v {
        PyValue::Real(r) => Some(*r),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

fn as_text(v: &PyValue) -> Option<String> {
    match v {
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        PyValue::Unicode(s) => Some(s.clone()),
        _ => None,
    }
}

fn attr_i64(v: &PyValue, name: &str) -> Option<i64> {
    attr(v, name).and_then(as_i64)
}

/// The rows of a `{key: [row, ...]}` answer, flattened.
fn grouped_rows(v: &PyValue) -> Vec<&PyValue> {
    match v {
        PyValue::Dict(entries) => entries
            .iter()
            .flat_map(|(value, _)| match value {
                PyValue::List(rows) | PyValue::Tuple(rows) => rows.iter().collect::<Vec<_>>(),
                other => vec![other],
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A column of an `eve_rowset::Row` (`Row(header, line)`, the shape of an
/// office in `officeManager.GetMyCorporationsOffices`), as an integer.
fn row_i64(v: &PyValue, column: &str) -> Option<i64> {
    let PyValue::ObjectEx { header, .. } = v else {
        return None;
    };
    let PyValue::Tuple(h) = header.as_ref() else {
        return None;
    };
    let PyValue::Tuple(args) = h.get(1)? else {
        return None;
    };
    let (PyValue::List(names) | PyValue::Tuple(names), PyValue::List(line)) =
        (args.first()?, args.get(1)?)
    else {
        return None;
    };
    let i = names
        .iter()
        .position(|n| as_text(n).as_deref() == Some(column))?;
    as_i64(line.get(i)?)
}

/// A column of a `DbRow`, as an integer.
fn db_i64(row: &eve_marshal::DbRow, column: &str) -> Option<i64> {
    let i = row.descriptor().index_of(column.as_bytes())?;
    match row.values().get(i)? {
        eve_marshal::DbValue::Int(n) => Some(*n),
        _ => None,
    }
}

/// The `DbRow`s anywhere in `v` (an indexed rowset's entries, a row).
fn db_rows(v: &PyValue) -> Vec<&eve_marshal::DbRow> {
    match v {
        PyValue::DbRow(r) => vec![r.as_ref()],
        PyValue::ObjectEx { dict, list, .. } => dict
            .iter()
            .flat_map(|(a, b)| [a, b])
            .chain(list.iter())
            .flat_map(db_rows)
            .collect(),
        PyValue::Dict(entries) => entries
            .iter()
            .flat_map(|(a, b)| [a, b])
            .flat_map(db_rows)
            .collect(),
        PyValue::List(items) | PyValue::Tuple(items) => items.iter().flat_map(db_rows).collect(),
        _ => Vec::new(),
    }
}

fn soft<T>(r: Result<T>) -> Result<std::result::Result<T, String>> {
    match r {
        Ok(v) => Ok(Ok(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(Err(e.to_string())),
    }
}

fn corp_call(
    conn: &mut Conn,
    corp: i64,
    method: &str,
    args: Vec<PyValue>,
) -> Result<crate::Answer> {
    conn.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
}

fn alliance_call(
    conn: &mut Conn,
    al: i64,
    method: &str,
    args: Vec<PyValue>,
) -> Result<crate::Answer> {
    let bind = PyValue::Tuple(vec![PyValue::Int(al), PyValue::Int(1)]);
    conn.moniker_call("allianceRegistry", bind, method, args, vec![])
}

/// One loss of mine: `(killID, the killer, ISK lost)`.
type Loss = (i64, Affiliation, f64);

/// My losses to characters among the recent reports (SOC-C6's
/// `GetRecentShipKillsAndLosses(num, startIndex)`); NPC kills carry no
/// final character and are skipped. The second argument is a kill id upper
/// bound on the server (`killmail::involving`), so the newest page is
/// `None` (SOC-C5: `Int(0)` was an empty page).
fn read_losses(conn: &mut Conn, me: i64) -> Result<Vec<Loss>> {
    let outcome = match soft(conn.call(
        "charMgr",
        "GetRecentShipKillsAndLosses",
        vec![PyValue::Int(KILLS_READ), PyValue::None],
    ))? {
        Ok(o) => o,
        Err(_) => return Ok(Vec::new()),
    };
    let Some(PyValue::List(rows)) = outcome.value() else {
        return Ok(Vec::new());
    };
    Ok(rows
        .iter()
        .filter(|r| attr_i64(r, "victimCharacterID") == Some(me))
        .filter_map(|r| {
            let id = attr_i64(r, "killID")?;
            let killer = attr_i64(r, "finalCharacterID")?;
            let corp = attr_i64(r, "finalCorporationID").unwrap_or(0);
            let alliance = attr_i64(r, "finalAllianceID");
            let isk = attr(r, "iskLost").and_then(as_f64).unwrap_or(0.0);
            (killer != me).then_some((
                id,
                Affiliation {
                    character_id: killer,
                    corporation_id: corp,
                    alliance_id: alliance,
                },
                isk,
            ))
        })
        .collect())
}

/// Am I my corp's CEO? `CanLeaveCurrentCorporation` says
/// `CrpCantQuitCEO` for exactly the CEO (`docs/lanes/corp.md` §2).
fn read_ceo(conn: &mut Conn, corp: i64) -> Result<bool> {
    let a = corp_call(conn, corp, "CanLeaveCurrentCorporation", vec![])?;
    Ok(match a.value {
        Some(PyValue::Tuple(items)) => {
            items.get(1).and_then(as_text).as_deref() == Some("CrpCantQuitCEO")
        }
        _ => false,
    })
}

/// **One social turn on this connection** (module header). Docked; at most
/// once per [`HallState::every`] per bot. Returns the act lines; a refusal
/// is a line, only a broken connection an error.
pub fn turn(
    conn: &mut Conn,
    hall: &Hall,
    mind: &Arc<Mutex<Mind>>,
    table: &[Career],
    now: u64,
) -> Result<Vec<String>> {
    let Some(me) = conn.session().char_id() else {
        return Ok(Vec::new());
    };
    {
        let mut st = hall.lock();
        let every = st.every;
        if let Some(m) = st.members.get_mut(&me) {
            if m.last_turn.is_some_and(|t| now.saturating_sub(t) < every) {
                return Ok(Vec::new());
            }
            m.last_turn = Some(now);
        }
    }
    let corp = conn.session().get_i64("corpid").unwrap_or(0);
    let alliance = conn.session().get_i64("allianceid");
    let day = hall.lock().day;
    let (name, traits, career, income_ratio, grudges) = {
        let m = mind.lock().unwrap_or_else(|p| p.into_inner());
        let c = crate::career::find(table, &m.career);
        let prior = c.map_or(0.0, |c| c.prior_isk_h);
        let ratio = match m.income.get(&m.career) {
            Some(l) if prior > 0.0 && l.hours >= crate::mind::LEDGER_MIN_HOURS => l.isk_h / prior,
            _ => 1.0,
        };
        let half = relations::personal_half_days(m.traits.get(Trait::Loyalty), &Temper::NORMAL);
        let grudges = relations::grudges(&m.regard, now, day, half);
        (m.name.clone(), m.traits, m.career.clone(), ratio, grudges)
    };
    let band = crate::career::find(table, &career).map_or(Band::High, |c| c.band);
    let losses = read_losses(conn, me)?;
    let mut out = Vec::new();

    // Who runs my corp, when the hall does not know.
    let unknown_ceo = society::is_player_corp(corp)
        && hall.lock().corps.get(&corp).is_none_or(|c| c.ceo.is_none());
    let i_am_ceo = if unknown_ceo {
        soft(read_ceo(conn, corp))?.unwrap_or_default()
    } else {
        false
    };
    let wallet = soft(conn.cash_balance())?.ok();

    let (plans, new_losses, leave) = {
        let mut st = hall.lock();
        let prev = st.members.get(&me).cloned();
        let member = Member {
            name: name.clone(),
            traits,
            career: career.clone(),
            band,
            corp,
            alliance,
            last_turn: Some(now),
            satisfaction: prev.as_ref().map(|p| p.satisfaction).unwrap_or_default(),
            applied: prev
                .as_ref()
                .and_then(|p| p.applied)
                .filter(|_| !society::is_player_corp(corp)),
            salt: prev.as_ref().map_or(0, |p| p.salt),
        };
        st.members.insert(me, member);
        if let Some(w) = wallet {
            st.pool.wallets.insert(me, (w, now));
        }
        let ids: Vec<i64> = st.pool_cands().iter().map(|c| c.id).collect();
        st.pool.observe(ids, now);
        if society::is_player_corp(corp) {
            if let std::collections::btree_map::Entry::Vacant(e) = st.corps.entry(corp) {
                e.insert(CorpMind {
                    id: corp,
                    ceo: None,
                    profile: Profile::of(&career, band),
                    founded: now,
                    alliance,
                    name: String::new(),
                    applied_alliance: None,
                    salt: 0,
                });
                st.dirty = true;
            }
            let c = st.corps.get_mut(&corp).expect("inserted");
            if i_am_ceo && c.ceo != Some(me) {
                c.ceo = Some(me);
                c.profile = Profile::of(&career, band);
            }
            if c.alliance != alliance {
                c.alliance = alliance;
                if let Some(al) = alliance {
                    c.applied_alliance = None;
                    st.alliances.entry(al).or_default();
                }
                st.dirty = true;
            }
        }
        // Losses into the matrix (once per report) and my grudges.
        let my_group = if society::is_player_corp(corp) {
            Some(st.group_of_corp(corp))
        } else {
            None
        };
        let mut new_losses = Vec::new();
        for (id, killer, isk) in &losses {
            if !st.kills.insert(*id) {
                continue;
            }
            while st.kills.len() > KILLS_KEEP {
                st.kills.pop_first();
            }
            let k = society::group_of(killer);
            if let Some(g) = my_group {
                st.record_kill(
                    *id,
                    &Event::Kill {
                        victim: g,
                        killer: k,
                        isk: *isk,
                    },
                );
            }
            st.act(format!(
                "{name} lost a ship to {} (group {k})",
                killer.character_id
            ));
            new_losses.push((k, *isk));
        }
        out.extend(st.tick(now));
        let fit = st
            .corps
            .get(&corp)
            .map_or(0.0, |c| society::fit(c.profile, &career, band));
        let leave = st.sample(
            me,
            &Mood {
                income_ratio,
                losses: new_losses.len() as u32,
                fit,
                at_war: false,
                aggression: traits.get(Trait::Aggression),
            },
            now,
        );
        (st.plan(me, now, wallet, &grudges), new_losses, leave)
    };
    if !new_losses.is_empty() {
        let mut m = mind.lock().unwrap_or_else(|p| p.into_inner());
        let half = relations::personal_half_days(m.traits.get(Trait::Loyalty), &Temper::NORMAL);
        for (k, isk) in new_losses {
            // Keyed by the killer's group, as the grudge was; R-REL-2 keys
            // by character with the full killmail.
            let d = -society::K_KILL * society::value_factor(isk);
            relations::regard_add(&mut m.regard, k, d, now, day, half);
        }
    }
    // DOC-5: the readiness the leader's doctrine layer reads (skills,
    // system, whether the bot holds the doctrine hull).
    crate::doctrine_assign::report(conn, hall, me, now)?;
    // R-REL-2: what this bot saw (killmails with every attacker, contracts,
    // wars, mail) into the relations model.
    out.extend(crate::observe::turn(conn, hall, mind, now)?);
    // R-REL-3: the bot's own contact book follows its personal regard.
    out.extend(crate::effects::personal_contacts(conn, hall, mind, now)?);
    // R-REL-4: a CEO tells a human when their standing crosses a tier.
    out.extend(crate::human::turn(conn, hall, mind, now)?);
    let mut plans = plans;
    plans.extend(leave);
    for p in plans {
        out.extend(execute(conn, hall, mind, me, corp, &p, now)?);
        // A join, a founding or a leave moved me: the rest waits a turn.
        if conn.session().get_i64("corpid").unwrap_or(0) != corp {
            break;
        }
    }
    if let Err(e) = hall.save() {
        out.push(format!("MISSING: a society state file that saves ({e})"));
    }
    Ok(out)
}

/// R-REL-3: `corp`'s CEO's effective view (`R_eff`) of the pilot `who`: the
/// bot's own regard, its corp, alliance and coalition, and the human prior.
fn ceo_view(hall: &Hall, mind: &Arc<Mutex<Mind>>, corp: i64, who: i64, now: u64) -> f64 {
    let (regard, loyalty) = {
        let m = mind.lock().unwrap_or_else(|p| p.into_inner());
        (m.regard.clone(), m.traits.get(Trait::Loyalty))
    };
    let st = hall.lock();
    let seen = st.humans.get(&who);
    let aff = Affiliation {
        character_id: who,
        corporation_id: seen.map_or(0, |s| s.corp),
        alliance_id: seen.and_then(|s| s.alliance),
    };
    st.r_eff_of(corp, &regard, loyalty, &aff, now)
}

/// Note a corp change in the mind's history.
fn stint(mind: &Arc<Mutex<Mind>>, from: i64, to: Option<i64>, now: u64) {
    let mut m = mind.lock().unwrap_or_else(|p| p.into_inner());
    for s in m
        .corps
        .iter_mut()
        .filter(|s| s.corp == from && s.left.is_none())
    {
        s.left = Some(now);
    }
    if let Some(to) = to {
        m.corps.push(Stint {
            corp: to,
            joined: now,
            left: None,
        });
    }
}

fn execute(
    conn: &mut Conn,
    hall: &Hall,
    mind: &Arc<Mutex<Mind>>,
    me: i64,
    corp: i64,
    plan: &Plan,
    now: u64,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    match *plan {
        Plan::Offers => {
            let a = corp_call(conn, corp, "GetMyApplications", vec![])?;
            let Some(v) = a.value else {
                return Ok(out);
            };
            let wants = {
                let st = hall.lock();
                st.members
                    .get(&me)
                    .is_some_and(|m| society::wants_to_join(&m.traits))
            };
            for row in grouped_rows(&v) {
                let status = attr_i64(row, "status");
                if !matches!(
                    status,
                    Some(APP_ACCEPTED_BY_CORPORATION | APP_INVITED_BY_CORPORATION)
                ) || !wants
                {
                    continue;
                }
                let (Some(id), Some(to)) = (
                    attr_i64(row, "applicationID"),
                    attr_i64(row, "corporationID"),
                ) else {
                    continue;
                };
                let enemy = hall.lock().stances.get(to, me) == Stance::Red;
                if enemy {
                    continue;
                }
                let text = attr(row, "applicationText")
                    .and_then(as_text)
                    .unwrap_or_default();
                let a = corp_call(
                    conn,
                    corp,
                    "UpdateApplicationOffer",
                    vec![
                        PyValue::Int(id),
                        PyValue::Int(me),
                        PyValue::Int(to),
                        uni(&text),
                        PyValue::Int(APP_ACCEPTED_BY_CHARACTER),
                    ],
                )?;
                match a.refusal() {
                    Some(key) => out.push(format!("join {to}: refused {key}")),
                    None => {
                        let joined = conn.session().get_i64("corpid") == Some(to);
                        let mut st = hall.lock();
                        if let Some(m) = st.members.get_mut(&me) {
                            m.applied = None;
                            m.satisfaction = Satisfaction::default();
                            if joined {
                                m.corp = to;
                            }
                        }
                        st.act(format!("{me} joined corp {to}"));
                        drop(st);
                        stint(mind, corp, Some(to), now);
                        out.push(format!("join {to}: ok"));
                        break;
                    }
                }
            }
        }
        Plan::Found => {
            let got = match soft(crate::acquire::learn_skills(
                conn,
                &[(CORPORATION_MANAGEMENT, 1)],
                None,
            ))? {
                Ok(g) => g,
                Err(why) => {
                    out.push(format!("MISSING: Corporation Management ({why})"));
                    return Ok(out);
                }
            };
            if !got.missing.is_empty() {
                out.push(format!(
                    "MISSING: Corporation Management ({})",
                    got.missing.join("; ")
                ));
                return Ok(out);
            }
            let (founder, salt) = {
                let st = hall.lock();
                let m = &st.members[&me];
                (m.name.clone(), m.salt)
            };
            let name = corp_name(&founder, salt);
            let suggested = corp_call(conn, corp, "GetSuggestedTickerNames", vec![uni(&name)])?;
            let ticker = match suggested.value {
                Some(PyValue::List(rows)) => rows
                    .iter()
                    .find_map(|r| attr(r, "tickerName").and_then(as_text)),
                _ => None,
            }
            .unwrap_or_else(|| fallback_ticker(&name, salt));
            let a = corp_call(
                conn,
                corp,
                "AddCorporation",
                vec![uni(&name), uni(&ticker), uni(""), uni("")],
            )?;
            if let Some(key) = a.refusal() {
                if key.contains("Taken")
                    && let Some(m) = hall.lock().members.get_mut(&me)
                {
                    m.salt += 1;
                }
                out.push(format!("found {name} [{ticker}]: refused {key}"));
                return Ok(out);
            }
            let Some(new) = a
                .outcome
                .session_change
                .as_ref()
                .and_then(|s| s.new_i64("corpid"))
                .or_else(|| conn.session().get_i64("corpid"))
                .filter(|c| society::is_player_corp(*c))
            else {
                out.push(format!("found {name}: no corp in the answer"));
                return Ok(out);
            };
            {
                let mut st = hall.lock();
                let (career, band) = {
                    let m = st.members.get_mut(&me).expect("observed");
                    m.corp = new;
                    m.applied = None;
                    (m.career.clone(), m.band)
                };
                st.corps.insert(
                    new,
                    CorpMind {
                        id: new,
                        ceo: Some(me),
                        profile: Profile::of(&career, band),
                        founded: now,
                        alliance: None,
                        name: name.clone(),
                        applied_alliance: None,
                        salt: 0,
                    },
                );
                let p = st.corps[&new].profile.name();
                st.act(format!("{founder} founded {name} [{ticker}] {new}, {p}"));
                if st.pool.founder == Some(me) {
                    let n = st.pool.pledges.len();
                    st.pool.founded(me, new, now);
                    st.act(format!(
                        "co-founding: {n} pledge(s) to {founder} became corp {new}'s; the pledgers apply"
                    ));
                }
            }
            stint(mind, corp, Some(new), now);
            out.push(format!("found {name} [{ticker}] {new}: ok"));
        }
        Plan::Pledge { to, amount } => {
            // The founder may have founded since the plan was made.
            if !hall.lock().pool_cands().iter().any(|c| c.id == to) {
                out.push(format!("pledge to {to}: no longer founding"));
                return Ok(out);
            }
            let a = conn.call(
                "account",
                "GiveCash",
                vec![
                    PyValue::Int(to),
                    PyValue::Real(amount),
                    uni(crate::cofound::PLEDGE_REASON),
                ],
            )?;
            match a.refusal() {
                Some(key) => out.push(format!("pledge {amount:.0} to {to}: refused {key}")),
                None => {
                    let mut st = hall.lock();
                    st.pool.pledge(me, to, amount, now);
                    st.act(format!("{me} pledged {amount:.0} ISK to {to} for a corp"));
                    out.push(format!("pledge {amount:.0} to {to}: ok"));
                }
            }
        }
        Plan::Refund => {
            let pledges: Vec<crate::cofound::Pledge> = hall
                .lock()
                .pool
                .pledges
                .iter()
                .filter(|p| p.to == me)
                .cloned()
                .collect();
            let mut held = soft(conn.cash_balance())?.unwrap_or(0.0);
            for p in pledges {
                // The founder may have spent some: pay back what it holds.
                let pay = p.amount.min(held).floor();
                if pay < 1.0 {
                    out.push(format!("refund {} to {}: wallet short", p.amount, p.from));
                    continue;
                }
                let a = conn.call(
                    "account",
                    "GiveCash",
                    vec![
                        PyValue::Int(p.from),
                        PyValue::Real(pay),
                        uni(crate::cofound::REFUND_REASON),
                    ],
                )?;
                match a.refusal() {
                    Some(key) => out.push(format!("refund {pay:.0} to {}: refused {key}", p.from)),
                    None => {
                        held -= pay;
                        let mut st = hall.lock();
                        if let Some(i) = st.pool.pledges.iter().position(|q| q == &p) {
                            st.pool.refunded(i, pay, now);
                        }
                        st.act(format!("{me} refunded {pay:.0} ISK to {}", p.from));
                        out.push(format!("refund {pay:.0} to {}: ok", p.from));
                    }
                }
            }
        }
        Plan::Apply(to) => {
            let a = corp_call(
                conn,
                corp,
                "InsertApplication",
                vec![PyValue::Int(to), uni("I would like to fly with you.")],
            )?;
            let mut st = hall.lock();
            if let Some(m) = st.members.get_mut(&me) {
                m.applied = Some((to, now));
            }
            match a.refusal() {
                Some(key) => out.push(format!("apply {to}: refused {key}")),
                None => {
                    st.act(format!("{me} applied to {to}"));
                    out.push(format!("apply {to}: ok"));
                }
            }
        }
        Plan::Leave => {
            let a = corp_call(conn, corp, "KickOutMember", vec![PyValue::Int(me)])?;
            match a.refusal() {
                Some(key) => out.push(format!("leave {corp}: refused {key}")),
                None => {
                    let mut st = hall.lock();
                    if let Some(m) = st.members.get_mut(&me) {
                        m.satisfaction = Satisfaction::default();
                        m.corp = conn.session().get_i64("corpid").unwrap_or(0);
                    }
                    st.act(format!("{me} left corp {corp}"));
                    drop(st);
                    stint(mind, corp, None, now);
                    out.push(format!("leave {corp}: ok"));
                }
            }
        }
        Plan::Recruit => {
            let a = corp_call(conn, corp, "GetApplications", vec![])?;
            let Some(v) = a.value else {
                return Ok(out);
            };
            for row in grouped_rows(&v) {
                if !matches!(
                    attr_i64(row, "status"),
                    Some(APP_APPLIED | APP_RENEGOTIATED)
                ) {
                    continue;
                }
                let (Some(id), Some(who)) =
                    (attr_i64(row, "applicationID"), attr_i64(row, "characterID"))
                else {
                    continue;
                };
                // R-REL-3: the CEO's own view of the applicant counts.
                let r = ceo_view(hall, mind, corp, who, now);
                let yes = hall.lock().accepts_with(corp, who, Some(r));
                let status = if yes {
                    APP_ACCEPTED_BY_CORPORATION
                } else {
                    APP_REJECTED_BY_CORPORATION
                };
                let text = attr(row, "applicationText")
                    .and_then(as_text)
                    .unwrap_or_default();
                let a = corp_call(
                    conn,
                    corp,
                    "UpdateApplicationOffer",
                    vec![
                        PyValue::Int(id),
                        PyValue::Int(who),
                        PyValue::Int(corp),
                        uni(&text),
                        PyValue::Int(status),
                    ],
                )?;
                let verb = if yes { "accept" } else { "reject" };
                match a.refusal() {
                    Some(key) => out.push(format!("{verb} {who}: refused {key}")),
                    None => {
                        hall.lock().act(format!("corp {corp} {verb}s {who}"));
                        out.push(format!("{verb} {who}: ok"));
                        // MAIL-1: the offer is waiting; say so.
                        if yes && let Some(line) = crate::mail::recruit(conn, hall, corp, who)? {
                            out.push(line);
                        }
                    }
                }
            }
        }
        Plan::Contacts { owner, alliance } => {
            let (get, add, edit, remove) = if alliance {
                (
                    "GetAllianceContacts",
                    "AddAllianceContact",
                    "EditAllianceContact",
                    "RemoveAllianceContacts",
                )
            } else {
                (
                    "GetCorporateContacts",
                    "AddCorporateContact",
                    "EditCorporateContact",
                    "RemoveCorporateContacts",
                )
            };
            let call = |conn: &mut Conn, m: &str, args: Vec<PyValue>| {
                if alliance {
                    alliance_call(conn, owner, m, args)
                } else {
                    corp_call(conn, owner, m, args)
                }
            };
            let a = call(conn, get, vec![])?;
            if let Some(key) = a.refusal() {
                out.push(format!("contacts {owner}: refused {key}"));
                return Ok(out);
            }
            let actual: BTreeMap<i64, f64> = match &a.value {
                Some(PyValue::Dict(entries)) => entries
                    .iter()
                    // Each entry's row names its own contact, whichever
                    // half of the pair the row is.
                    .flat_map(|(x, y)| [x, y])
                    .filter_map(|row| {
                        Some((
                            attr_i64(row, "contactID")?,
                            attr(row, "relationshipID").and_then(as_f64)?,
                        ))
                    })
                    .collect(),
                _ => BTreeMap::new(),
            };
            let diff = {
                let mut st = hall.lock();
                st.books.retain(|(o, _), _| *o != owner);
                for (t, r) in &actual {
                    st.books.insert((owner, *t), *r);
                }
                st.contact_diff(owner, &actual, now)
            };
            let removals: Vec<i64> = diff
                .iter()
                .filter(|(_, s)| s.is_none())
                .map(|(t, _)| *t)
                .collect();
            for (t, s) in diff {
                let Some(s) = s else { continue };
                let m = if actual.contains_key(&t) { edit } else { add };
                let a = call(conn, m, vec![PyValue::Int(t), PyValue::Real(s)])?;
                match a.refusal() {
                    Some(key) => out.push(format!("contact {owner} -> {t} {s:+}: refused {key}")),
                    None => {
                        let mut st = hall.lock();
                        st.books.insert((owner, t), s);
                        st.managed.insert((owner, t));
                        st.act(format!("contact {owner} -> {t} {s:+}"));
                        drop(st);
                        out.push(format!("contact {owner} -> {t} {s:+}: ok"));
                        // MAIL-1: tell the other side.
                        if !alliance
                            && let Some(line) = crate::mail::diplomacy(conn, hall, owner, t, s)?
                        {
                            out.push(line);
                        }
                    }
                }
            }
            if !removals.is_empty() {
                let ids = PyValue::List(removals.iter().map(|t| PyValue::Int(*t)).collect());
                let a = call(conn, remove, vec![ids])?;
                match a.refusal() {
                    Some(key) => out.push(format!(
                        "contacts {owner} clear {removals:?}: refused {key}"
                    )),
                    None => {
                        let mut st = hall.lock();
                        for t in &removals {
                            st.books.remove(&(owner, *t));
                            st.managed.remove(&(owner, *t));
                        }
                        st.act(format!("contacts {owner} cleared {removals:?}"));
                        out.push(format!("contacts {owner} clear {removals:?}: ok"));
                    }
                }
            }
        }
        Plan::Invite(who) => {
            let r = ceo_view(hall, mind, corp, who, now);
            let hours = hall.lock().fleet_hours.get(&who).copied().unwrap_or(0.0);
            if !effects::may_invite(r, hours, false) {
                out.push(format!("invite {who}: not yet (R {r:.1}, {hours:.1} h)"));
                return Ok(out);
            }
            let a = corp_call(conn, corp, "InsertInvitation", vec![PyValue::Int(who)])?;
            match a.refusal() {
                Some(key) => out.push(format!("invite {who}: refused {key}")),
                None => {
                    {
                        let mut st = hall.lock();
                        st.invited.insert((corp, who), now);
                        st.act(format!("corp {corp} invites {who}"));
                    }
                    out.push(format!("invite {who}: ok"));
                    if let Some(line) = crate::mail::invited(conn, hall, corp, who)? {
                        out.push(line);
                    }
                }
            }
        }
        Plan::FoundAlliance => {
            let wallet = soft(conn.cash_balance())?.unwrap_or(0.0);
            if wallet < ALLIANCE_FEE {
                out.push(format!(
                    "found an alliance: saving, {wallet:.0} of {ALLIANCE_FEE:.0} ISK"
                ));
                return Ok(out);
            }
            let (cname, salt) = {
                let st = hall.lock();
                let c = &st.corps[&corp];
                let n = if c.name.is_empty() {
                    corp_name(&st.members[&me].name, 0)
                } else {
                    c.name.clone()
                };
                (n, c.salt)
            };
            let name = alliance_name(&cname, salt);
            let s = corp_call(
                conn,
                corp,
                "GetSuggestedAllianceShortNames",
                vec![uni(&name)],
            )?;
            let short = match s.value {
                Some(PyValue::List(rows)) => rows
                    .iter()
                    .find_map(|r| attr(r, "shortName").and_then(as_text)),
                _ => None,
            }
            .unwrap_or_else(|| fallback_ticker(&name, salt));
            // CORP-W: the fee is the corporation's to pay. Top the Master
            // Wallet up to it from the CEO's own, as the Wallet window's
            // Deposit ISK does.
            let held = conn
                .call_kw(
                    "account",
                    "GetCashBalance",
                    vec![PyValue::Int(1)],
                    vec![(PyValue::Int(1000), PyValue::Bytes(b"accountKey".to_vec()))],
                )
                .ok()
                .and_then(|o| match o.value() {
                    Some(PyValue::Real(r)) => Some(r),
                    _ => None,
                })
                .unwrap_or(0.0);
            if held < ALLIANCE_FEE {
                let gift = conn.call_kw(
                    "account",
                    "GiveCash",
                    vec![
                        PyValue::Int(corp),
                        PyValue::Real(ALLIANCE_FEE - held),
                        uni(""),
                    ],
                    vec![(PyValue::Int(1000), PyValue::Bytes(b"toAccountKey".to_vec()))],
                );
                if let Some(key) = gift.ok().and_then(|o| o.refusal()) {
                    out.push(format!("found an alliance: funding the corp refused {key}"));
                    return Ok(out);
                }
            }
            let a = corp_call(
                conn,
                corp,
                "CreateAlliance",
                vec![uni(&name), uni(&short), uni(""), uni("")],
            )?;
            if let Some(key) = a.refusal() {
                if key.contains("Taken")
                    && let Some(c) = hall.lock().corps.get_mut(&corp)
                {
                    c.salt += 1;
                }
                out.push(format!("found alliance {name} <{short}>: refused {key}"));
                return Ok(out);
            }
            let al = a
                .outcome
                .session_change
                .as_ref()
                .and_then(|s| s.new_i64("allianceid"))
                .or_else(|| conn.session().get_i64("allianceid"));
            let Some(al) = al else {
                out.push(format!("found alliance {name}: no alliance in the answer"));
                return Ok(out);
            };
            let mut st = hall.lock();
            st.alliances.insert(
                al,
                AllianceMind {
                    executor: Some(corp),
                    read_at: Some(now),
                    name: name.clone(),
                    answered: BTreeSet::new(),
                },
            );
            if let Some(c) = st.corps.get_mut(&corp) {
                c.alliance = Some(al);
            }
            if let Some(m) = st.members.get_mut(&me) {
                m.alliance = Some(al);
            }
            st.act(format!(
                "corp {corp} founded alliance {name} <{short}> {al}"
            ));
            out.push(format!("found alliance {name} <{short}> {al}: ok"));
        }
        Plan::ApplyAlliance(al) => {
            let a = corp_call(
                conn,
                corp,
                "ApplyToJoinAlliance",
                vec![PyValue::Int(al), uni("We stand with you.")],
            )?;
            let mut st = hall.lock();
            if let Some(c) = st.corps.get_mut(&corp) {
                c.applied_alliance = Some((al, now));
            }
            match a.refusal() {
                Some(key) => out.push(format!("apply to alliance {al}: refused {key}")),
                None => {
                    st.act(format!("corp {corp} applied to alliance {al}"));
                    out.push(format!("apply to alliance {al}: ok"));
                }
            }
        }
        Plan::AnswerAlliance(al) => {
            let a = alliance_call(conn, al, "GetApplications", vec![])?;
            let Some(v) = a.value else {
                return Ok(out);
            };
            let open: Vec<i64> = db_rows(&v)
                .into_iter()
                .filter(|r| db_i64(r, "state") == Some(ALLIANCE_APP_NEW))
                .filter_map(|r| db_i64(r, "corporationID"))
                .collect();
            for c in open {
                let (done, yes) = {
                    let st = hall.lock();
                    (
                        st.alliances
                            .get(&al)
                            .is_some_and(|a| a.answered.contains(&c)),
                        st.alliance_accepts_at(al, c, now),
                    )
                };
                if done {
                    continue;
                }
                let state = if yes {
                    ALLIANCE_APP_ACCEPTED
                } else {
                    ALLIANCE_APP_REJECTED
                };
                let a = alliance_call(
                    conn,
                    al,
                    "UpdateApplication",
                    vec![PyValue::Int(c), uni(""), PyValue::Int(state)],
                )?;
                let verb = if yes { "accept" } else { "reject" };
                let mut st = hall.lock();
                st.alliances.entry(al).or_default().answered.insert(c);
                match a.refusal() {
                    Some(key) => out.push(format!("alliance {al} {verb} {c}: refused {key}")),
                    None => {
                        if yes && let Some(m) = st.corps.get_mut(&c) {
                            m.alliance = Some(al);
                            m.applied_alliance = None;
                        }
                        st.act(format!("alliance {al} {verb}s corp {c}"));
                        out.push(format!("alliance {al} {verb} {c}: ok"));
                    }
                }
            }
        }
        Plan::LeaveAlliance(al) => {
            let a = alliance_call(conn, al, "DeleteMember", vec![PyValue::Int(corp)])?;
            match a.refusal() {
                Some(key) => out.push(format!("leave alliance {al}: refused {key}")),
                None => {
                    let mut st = hall.lock();
                    if let Some(c) = st.corps.get_mut(&corp) {
                        c.alliance = None;
                    }
                    if let Some(a) = st
                        .alliances
                        .get_mut(&al)
                        .filter(|a| a.executor == Some(corp))
                    {
                        a.executor = None;
                        a.read_at = None;
                    }
                    st.act(format!("corp {corp} left alliance {al}"));
                    out.push(format!("leave alliance {al}: ok"));
                }
            }
        }
        Plan::Resign => {
            // `ResignFromCEO(None)` answers the willing successors, best
            // first (directors, then the longest-serving); the client's
            // `SelectReplacementCEO` pick goes to a second call (SOC-C3).
            let a = corp_call(conn, corp, "ResignFromCEO", vec![PyValue::None])?;
            if let Some(key) = a.refusal() {
                out.push(format!("resign {corp}: refused {key}"));
                return Ok(out);
            }
            let Some(next) = (match &a.value {
                Some(PyValue::List(ids) | PyValue::Tuple(ids)) => ids.iter().find_map(as_i64),
                _ => None,
            }) else {
                out.push(format!("resign {corp}: nobody willing, staying"));
                return Ok(out);
            };
            let a = corp_call(conn, corp, "ResignFromCEO", vec![PyValue::Int(next)])?;
            match a.refusal() {
                Some(key) => out.push(format!("resign {corp} to {next}: refused {key}")),
                None => {
                    let mut st = hall.lock();
                    if let Some(c) = st.corps.get_mut(&corp) {
                        c.ceo = Some(next);
                    }
                    if let Some(m) = st.members.get_mut(&me) {
                        m.satisfaction = Satisfaction::default();
                    }
                    st.act(format!("corp {corp}: {me} resigned, {next} is CEO"));
                    out.push(format!("resign {corp} to {next}: ok"));
                }
            }
        }
        Plan::ReadExecutor(al) => {
            let a = alliance_call(conn, al, "GetAlliance", vec![])?;
            let ex = a
                .value
                .as_ref()
                .map(db_rows)
                .and_then(|rows| rows.first().and_then(|r| db_i64(r, "executorCorpID")));
            if let Some(ex) = ex {
                let mut st = hall.lock();
                let a = st.alliances.entry(al).or_default();
                a.executor = Some(ex);
                a.read_at = Some(now);
                out.push(format!("alliance {al} executor {ex}"));
            }
        }
        // BOT-CORP: tax, roles and pay (the CEO), the hangar (a member).
        Plan::Ops => {
            hall.lock().ops_tried.insert(corp, now);
            out.extend(crate::corp_ops::run_ceo(conn, hall, me, corp, now)?);
        }
        Plan::Wars { owner, alliance } => {
            hall.lock().coal.war_tried.insert(owner, now);
            out.extend(crate::war_bot::run(
                conn, hall, mind, me, corp, owner, alliance, now,
            )?);
        }
        Plan::Stock => {
            hall.lock().stock_tried.insert(me, now);
            let career = hall
                .lock()
                .members
                .get(&me)
                .map(|m| m.career.clone())
                .unwrap_or_default();
            if crate::corp_ops::has_stock_pass(&career) {
                out.extend(crate::corp_ops::run_member(conn, hall, me, corp, &career)?);
            }
            // DOC-6: the funded procurement member stocks the doctrine
            // division; a fighter with no hull draws his kit from it.
            out.extend(crate::doctrine_stock::run_procure(
                conn, hall, me, corp, now,
            )?);
            if hall.lock().draws_kit(me, &career) {
                out.extend(crate::doctrine_stock::run_draw(conn, hall, me, corp)?);
            }
        }
        Plan::Srp => {
            hall.lock().stock.srp_tried.insert(corp, now);
            out.extend(crate::doctrine_stock::run_srp(conn, hall, me, corp, now)?);
        }
        Plan::Stage => {
            hall.lock().stock.stage_tried.insert(corp, now);
            out.extend(crate::doctrine_stock::run_stage(conn, hall, me, corp, now)?);
        }
        // CORP-O: rent an office at the home station when the corp wallet
        // allows. The calls are the client's own: `officeManager`'s
        // `GetMyCorporationsOffices` (remote), then bound on the station
        // `GetPriceQuote(corpID)` and `RentOffice(cost)`
        // (`OfficesPanel._rent_office`), and the corp's Master Wallet read as
        // the Wallet window does (`GetCashBalance(1, accountKey=1000)`).
        Plan::Office => {
            hall.lock().office_tried.insert(corp, now);
            let home = mind.lock().unwrap_or_else(|p| p.into_inner()).home;
            let station = conn.session().get_i64("stationid");
            let (Some(home), Some(station)) = (home, station) else {
                return Ok(out);
            };
            if home != station {
                return Ok(out);
            }
            let mine = match soft(conn.call("officeManager", "GetMyCorporationsOffices", vec![]))? {
                Ok(o) => o,
                Err(why) => {
                    out.push(format!("office: {why}"));
                    return Ok(out);
                }
            };
            if let Some(PyValue::List(rows)) = mine.value()
                && rows
                    .iter()
                    .any(|r| row_i64(r, "stationID") == Some(station))
            {
                return Ok(out);
            }
            let wallet = match soft(conn.call_kw(
                "account",
                "GetCashBalance",
                vec![PyValue::Int(1)],
                vec![(
                    PyValue::Int(eve_server::corp_wallet::MASTER_KEY),
                    PyValue::Bytes(b"accountKey".to_vec()),
                )],
            ))? {
                Ok(o) => o.value().as_ref().and_then(as_f64),
                Err(why) => {
                    out.push(format!("office: {why}"));
                    return Ok(out);
                }
            };
            let quote = match soft(conn.moniker_call(
                "officeManager",
                PyValue::Int(station),
                "GetPriceQuote",
                vec![PyValue::Int(corp)],
                vec![],
            ))? {
                Ok(a) => match (a.refusal(), a.value.as_ref().and_then(as_f64)) {
                    (None, Some(price)) => price,
                    (key, _) => {
                        out.push(format!("office quote: {}", key.unwrap_or_default()));
                        return Ok(out);
                    }
                },
                Err(why) => {
                    out.push(format!("office quote: {why}"));
                    return Ok(out);
                }
            };
            if wallet.is_none_or(|w| w < quote * OFFICE_RESERVE_PERIODS) {
                return Ok(out);
            }
            let a = soft(conn.moniker_call(
                "officeManager",
                PyValue::Int(station),
                "RentOffice",
                vec![PyValue::Real(quote)],
                vec![],
            ))?;
            match a {
                Ok(a) => match a.refusal() {
                    Some(key) => out.push(format!("rent office at {station}: refused {key}")),
                    None => {
                        hall.lock().act(format!(
                            "{me} rented an office for corp {corp} at {station}"
                        ));
                        out.push(format!("rent office at {station}: ok"));
                    }
                },
                Err(why) => out.push(format!("rent office at {station}: {why}")),
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mind::TRAIT_COUNT;

    const DAY: u64 = 1_000;
    const ANN: i64 = 90_000_201;
    const BEN: i64 = 90_000_202;
    const CAS: i64 = 90_000_203;
    const NPC: i64 = 1_000_044;
    const C1: i64 = 98_000_201;
    const C2: i64 = 98_000_202;
    const AL: i64 = 99_000_201;
    const MAL: i64 = 90_000_209;

    fn member(name: &str, career: &str, corp: i64, set: &[(Trait, f64)]) -> Member {
        let mut t = Traits([0.5; TRAIT_COUNT]);
        for &(k, v) in set {
            t.0[k as usize] = v;
        }
        Member {
            name: name.to_owned(),
            traits: t,
            career: career.to_owned(),
            band: Band::High,
            corp,
            alliance: None,
            last_turn: None,
            satisfaction: Satisfaction::default(),
            applied: None,
            salt: 0,
        }
    }

    fn corp(id: i64, ceo: i64, career: &str) -> CorpMind {
        CorpMind {
            id,
            ceo: Some(ceo),
            profile: Profile::of(career, Band::High),
            founded: 0,
            alliance: None,
            name: format!("Invented {id}"),
            applied_alliance: None,
            salt: 0,
        }
    }

    fn two_corps() -> HallState {
        let mut st = HallState::new(DAY);
        st.members.insert(ANN, member("Ann", "miner", C1, &[]));
        st.members.insert(CAS, member("Cas", "ratter", C2, &[]));
        st.corps.insert(C1, corp(C1, ANN, "miner"));
        st.corps.insert(C2, corp(C2, CAS, "ratter"));
        st
    }

    #[test]
    fn a_corpless_founder_founds_and_a_joiner_applies_to_the_fitting_corp() {
        let mut st = two_corps();
        let social = (society::FOUND_SOCIAL + 1.0) / 2.0;
        let drive = (society::FOUND_DRIVE / 2.0 + 1.0) / 2.0;
        st.members.insert(
            BEN,
            member(
                "Ben",
                "miner",
                NPC,
                &[
                    (Trait::Sociability, social),
                    (Trait::Aggression, 0.0),
                    (Trait::Diligence, 0.0),
                ],
            ),
        );
        let rich = Some(CORP_FEE + society::FOUND_RESERVE);
        assert_eq!(
            st.plan(BEN, 0, rich, &BTreeMap::new()),
            vec![Plan::Offers, Plan::Apply(C1)]
        );
        // A pending application waits.
        st.members.get_mut(&BEN).unwrap().applied = Some((C1, 0));
        assert_eq!(st.plan(BEN, 1, rich, &BTreeMap::new()), vec![Plan::Offers]);
        // A driven, sociable, rich bot with no matching corp founds.
        st.members.insert(
            BEN,
            member(
                "Ben",
                "explorer",
                NPC,
                &[
                    (Trait::Sociability, social),
                    (Trait::Aggression, drive),
                    (Trait::Diligence, drive),
                ],
            ),
        );
        assert_eq!(
            st.plan(BEN, 0, rich, &BTreeMap::new()),
            vec![Plan::Offers, Plan::Found]
        );
        // ...but not without the fee.
        assert_ne!(
            st.plan(BEN, 0, Some(CORP_FEE), &BTreeMap::new()).last(),
            Some(&Plan::Found)
        );
    }

    #[test]
    fn the_pool_pledges_refunds_applies_and_persists_through_the_hall() {
        use crate::cofound::{self, Step};
        let social = (society::FOUND_SOCIAL + 1.0) / 2.0;
        let set = |d: f64| {
            [
                (Trait::Sociability, social),
                (Trait::Aggression, 1.0),
                (Trait::Diligence, d),
            ]
        };
        // No bot corps: every driven corpless bot is a candidate.
        let mut st = HallState::new(DAY);
        st.members
            .insert(ANN, member("Ann", "miner", NPC, &set(1.0)));
        st.members
            .insert(BEN, member("Ben", "miner", NPC, &set(0.9)));
        st.members
            .insert(CAS, member("Cas", "miner", NPC, &set(0.8)));
        assert_eq!(st.pool_cands().len(), 3);
        st.pool.wallets.insert(ANN, (1_000_000.0, 0));
        let w = Some(cofound::PLEDGE_FLOOR + 800_000.0);
        // Ann (most diligent) is the founder; Ben pledges; Ann does not.
        let ben = st.plan(BEN, 1, w, &BTreeMap::new());
        let Some(&Plan::Pledge { to, amount }) = ben.last() else {
            panic!("{ben:?}")
        };
        assert_eq!(to, ANN);
        assert!(amount > 0.0);
        assert_eq!(st.plan(ANN, 1, w, &BTreeMap::new()), vec![Plan::Offers]);
        // The pledge is paid; the pool survives the state file.
        st.pool.pledge(BEN, ANN, amount, 1);
        let back = HallState::parse(&st.render(), DAY).expect("parses");
        assert_eq!(back.pool.pledges, st.pool.pledges);
        assert_eq!(back.pool.journal, st.pool.journal);
        assert_eq!(back.pool.founder, Some(ANN));
        // The deadline on the game clock sends the founder to refund.
        let due = 1 + (cofound::POOL_DEADLINE_DAYS * DAY as f64) as u64;
        assert_eq!(
            cofound::step(&st.pool, &st.pool_cands(), ANN, w, due, DAY),
            Some(Step::Refund)
        );
        assert_eq!(st.plan(ANN, due, w, &BTreeMap::new())[0], Plan::Refund);
        // A founding makes Ben an applicant that the CEO accepts whatever
        // its fit.
        st.members.get_mut(&ANN).unwrap().corp = C1;
        st.corps.insert(C1, corp(C1, ANN, "ratter"));
        st.pool.founded(ANN, C1, 2);
        assert_eq!(
            st.plan(BEN, 3, w, &BTreeMap::new()),
            vec![Plan::Offers, Plan::Apply(C1)]
        );
        assert!(st.accepts(C1, BEN));
        // Once Ben is in a corp the entry goes.
        st.members.get_mut(&BEN).unwrap().corp = C1;
        st.tick(4);
        assert!(st.pool.pledged.is_empty());
    }

    #[test]
    fn the_ceo_alone_plans_for_the_corp() {
        let mut st = two_corps();
        st.members.insert(BEN, member("Ben", "miner", C1, &[]));
        let ceo = st.plan(ANN, 0, Some(0.0), &BTreeMap::new());
        assert_eq!(
            ceo,
            vec![
                Plan::Recruit,
                Plan::Contacts {
                    owner: C1,
                    alliance: false
                },
                Plan::Office,
                Plan::Ops
            ]
        );
        assert!(st.plan(BEN, 0, Some(0.0), &BTreeMap::new()).is_empty());
    }

    /// CORP-O: the CEO looks at an office once per [`OFFICE_EVERY_DAYS`],
    /// and only the CEO does.
    #[test]
    fn the_ceo_looks_at_renting_an_office_once_a_day() {
        let mut st = two_corps();
        st.members.insert(BEN, member("Ben", "miner", C1, &[]));
        assert!(
            st.plan(ANN, 0, Some(0.0), &BTreeMap::new())
                .contains(&Plan::Office)
        );
        assert!(
            !st.plan(BEN, 0, Some(0.0), &BTreeMap::new())
                .contains(&Plan::Office)
        );
        // It tried at 0: not again until a day has passed.
        st.office_tried.insert(C1, 0);
        let wait = (OFFICE_EVERY_DAYS * st.day as f64) as u64;
        assert!(
            !st.plan(ANN, wait - 1, Some(0.0), &BTreeMap::new())
                .contains(&Plan::Office)
        );
        assert!(
            st.plan(ANN, wait, Some(0.0), &BTreeMap::new())
                .contains(&Plan::Office)
        );
        // Another corp's try is its own.
        st.office_tried.insert(C2, wait);
        assert!(
            st.plan(ANN, wait, Some(0.0), &BTreeMap::new())
                .contains(&Plan::Office)
        );
    }

    /// BOT-CORP: the CEO runs its operations pass twice a game-day and a
    /// builder, trader or courier member tends the hangar four times; other
    /// careers have no pass.
    #[test]
    fn the_ceo_runs_operations_and_a_builder_tends_the_hangar_on_their_cadence() {
        let mut st = two_corps();
        st.members.insert(BEN, member("Ben", "builder", C1, &[]));
        let ops_wait = (crate::corp_ops::OPS_EVERY_DAYS * st.day as f64) as u64;
        let stock_wait = (crate::corp_ops::STOCK_EVERY_DAYS * st.day as f64) as u64;
        let none = BTreeMap::new();
        assert!(st.plan(ANN, 0, Some(0.0), &none).contains(&Plan::Ops));
        st.ops_tried.insert(C1, 0);
        assert!(
            !st.plan(ANN, ops_wait - 1, Some(0.0), &none)
                .contains(&Plan::Ops)
        );
        assert!(
            st.plan(ANN, ops_wait, Some(0.0), &none)
                .contains(&Plan::Ops)
        );
        assert_eq!(st.plan(BEN, 0, Some(0.0), &none), vec![Plan::Stock]);
        st.stock_tried.insert(BEN, 0);
        assert!(st.plan(BEN, stock_wait - 1, Some(0.0), &none).is_empty());
        assert_eq!(
            st.plan(BEN, stock_wait, Some(0.0), &none),
            vec![Plan::Stock]
        );
        st.members.get_mut(&BEN).unwrap().career = "hunter".to_owned();
        assert!(st.plan(BEN, stock_wait, Some(0.0), &none).is_empty());
    }

    /// The rows `GetMyCorporationsOffices` sends are `Row(header, line)`.
    #[test]
    fn an_office_row_is_read_by_column() {
        let names = ["officeID", "stationID", "solarsystemID", "stationTypeID"];
        let row = PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Global(b"carbon.common.script.sys.row.Row".to_vec()),
                PyValue::Tuple(vec![
                    PyValue::List(
                        names
                            .iter()
                            .map(|n| PyValue::Bytes(n.as_bytes().to_vec()))
                            .collect(),
                    ),
                    PyValue::List(vec![
                        PyValue::Int(100_000_007),
                        PyValue::Int(69_000_001),
                        PyValue::Int(39_000_001),
                        PyValue::Int(1),
                    ]),
                ]),
            ])),
            list: Vec::new(),
            dict: Vec::new(),
        };
        assert_eq!(row_i64(&row, "stationID"), Some(69_000_001));
        assert_eq!(row_i64(&row, "officeID"), Some(100_000_007));
        assert_eq!(row_i64(&row, "nope"), None);
        assert_eq!(row_i64(&PyValue::Int(1), "stationID"), None);
    }

    #[test]
    fn mutual_blue_turns_into_contacts_then_an_alliance_and_a_kill_into_a_red() {
        let mut st = two_corps();
        st.tick(0);
        for _ in 0..=((society::ALLY_AT / society::K_ALLY_FIGHT).ceil() as usize) {
            st.record(&Event::AllyFight { a: C1, b: C2 });
        }
        let n = (society::ENEMY_AT.abs() / (society::K_KILL * society::VALUE_FACTOR_MIN)).ceil()
            as usize;
        for _ in 0..n {
            st.record(&Event::Kill {
                victim: C1,
                killer: MAL,
                isk: 0.0,
            });
        }
        st.tick(1);
        assert_eq!(st.stances.get(C1, C2), Stance::Blue);
        assert_eq!(st.stances.get(C2, C1), Stance::Blue);
        assert_eq!(st.stances.get(C1, MAL), Stance::Red);
        let want = st.desired_contacts(C1);
        assert_eq!(want.get(&C2), Some(&society::BLUE_STANDING));
        assert_eq!(want.get(&MAL), Some(&society::RED_STANDING));
        assert_eq!(st.contact_diff(C1, &BTreeMap::new(), 0).len(), 2);
        // A human's own contact is never cleared; a bot-set one is.
        let mut actual = want.clone();
        actual.insert(NPC, -5.0);
        assert!(st.contact_diff(C1, &actual, 0).is_empty());
        st.managed.insert((C1, NPC));
        assert_eq!(st.contact_diff(C1, &actual, 0), vec![(NPC, None)]);
        // Hostility follows the books read back.
        st.books.insert((C1, C2), society::BLUE_STANDING);
        let ann = Affiliation {
            character_id: ANN,
            corporation_id: C1,
            alliance_id: None,
        };
        let cas = Affiliation {
            character_id: CAS,
            corporation_id: C2,
            alliance_id: None,
        };
        let mal = Affiliation {
            character_id: MAL,
            corporation_id: NPC,
            alliance_id: None,
        };
        assert!(!st.hostile_to(ann, cas));
        assert!(st.hostile_to(ann, mal));
        // After ALLY_DAYS of mutual blue, the founder founds (equal sizes:
        // the more aggressive CEO).
        st.members.get_mut(&ANN).unwrap().traits.0[Trait::Aggression as usize] = 1.0;
        st.members.get_mut(&CAS).unwrap().traits.0[Trait::Aggression as usize] = 0.0;
        let later = 1 + (society::ALLY_DAYS * DAY as f64) as u64;
        st.tick(later);
        assert!(
            st.plan(ANN, later, Some(0.0), &BTreeMap::new())
                .contains(&Plan::FoundAlliance)
        );
        assert!(
            !st.plan(CAS, later, Some(0.0), &BTreeMap::new())
                .contains(&Plan::FoundAlliance)
        );
        // Once it exists, the other applies and the executor answers.
        st.corps.get_mut(&C1).unwrap().alliance = Some(AL);
        st.alliances.insert(
            AL,
            AllianceMind {
                executor: Some(C1),
                read_at: Some(later),
                ..AllianceMind::default()
            },
        );
        st.stances.held.insert(
            (C2, AL),
            society::Held {
                stance: Stance::Blue,
                since: Some(0),
            },
        );
        assert!(
            st.plan(CAS, later, Some(0.0), &BTreeMap::new())
                .contains(&Plan::ApplyAlliance(AL))
        );
        let ann_plan = st.plan(ANN, later, Some(0.0), &BTreeMap::new());
        assert!(ann_plan.contains(&Plan::AnswerAlliance(AL)), "{ann_plan:?}");
        assert!(st.alliance_accepts(AL, C2));
    }

    /// SOC-C3's succession: an unhappy CEO hands the seat on only while
    /// someone is there to take it; an unhappy member leaves.
    #[test]
    fn an_unhappy_ceo_resigns_to_a_member_and_never_closes_the_corp() {
        let mut st = two_corps();
        let bad = Mood {
            income_ratio: 0.0,
            losses: 3,
            fit: 0.0,
            at_war: false,
            aggression: 0.5,
        };
        for id in [ANN, CAS] {
            st.members.get_mut(&id).unwrap().traits.0[Trait::Loyalty as usize] = 0.0;
        }
        st.members
            .insert(BEN, member("Ben", "miner", C1, &[(Trait::Loyalty, 0.0)]));
        let run = |st: &mut HallState, id: i64| {
            (0..1_000u64).find_map(|i| st.sample(id, &bad, i * DAY / 10))
        };
        assert_eq!(run(&mut st, ANN), Some(Plan::Resign));
        assert_eq!(run(&mut st, BEN), Some(Plan::Leave));
        // Cas is alone in C2: no successor, so no plan.
        assert_eq!(run(&mut st, CAS), None);
    }

    #[test]
    fn the_ceo_rejects_an_enemy_and_accepts_a_fitting_bot() {
        let mut st = two_corps();
        st.members.insert(BEN, member("Ben", "miner", NPC, &[]));
        assert!(st.accepts(C1, BEN));
        st.stances.held.insert(
            (C1, BEN),
            society::Held {
                stance: Stance::Red,
                since: Some(0),
            },
        );
        assert!(!st.accepts(C1, BEN));
        // A stranger (a human) is taken at the accept line, unless an enemy.
        assert!(st.accepts(C1, MAL));
    }

    #[test]
    fn the_planner_need_reaches_the_signals() {
        let mut st = two_corps();
        st.members.insert(BEN, member("Ben", "ratter", C1, &[]));
        st.members.insert(CAS, member("Cas", "ratter", C1, &[]));
        let hall = Hall {
            inner: Mutex::new(st),
        };
        let s = hall.with_need(ANN, crate::career::CAREERS, Signals::default());
        assert!(
            s.corp_need["miner"] > s.corp_need["ratter"],
            "{:?}",
            s.corp_need
        );
    }

    #[test]
    fn the_hall_state_file_round_trips_and_a_torn_one_is_refused() {
        let mut st = two_corps();
        st.corps.get_mut(&C1).unwrap().alliance = Some(AL);
        st.alliances.insert(
            AL,
            AllianceMind {
                executor: Some(C1),
                ..AllianceMind::default()
            },
        );
        st.record(&Event::Kill {
            victim: C1,
            killer: MAL,
            isk: 1.0,
        });
        st.tick(5);
        st.stances.settle(C1, MAL, society::ENEMY_AT, 7, DAY);
        st.managed.insert((C1, MAL));
        st.kills.insert(42);
        st.stock.paid_today.insert(C1, (9, 1234.5));
        st.stock.float.insert(MAL, 777.0);
        let text = st.render();
        let back = HallState::parse(&text, DAY).expect("parses");
        assert_eq!(back.corps, st.corps);
        assert_eq!(back.alliances[&AL].executor, Some(C1));
        assert_eq!(back.relations, st.relations);
        assert_eq!(back.stances, st.stances);
        assert_eq!(back.managed, st.managed);
        assert_eq!(back.kills, st.kills);
        // REVIEW-FIX2: the day's SRP total and the float survive a restart.
        assert_eq!(back.stock.paid_today, st.stock.paid_today);
        assert_eq!(back.stock.float, st.stock.float);
        assert_eq!(back.last_daily, st.last_daily);
        let torn: String = text.lines().take(2).map(|l| format!("{l}\n")).collect();
        assert!(HallState::parse(&torn, DAY).is_err());
        assert!(HallState::parse("soc\t0\nend\t0\n", DAY).is_err());
    }

    #[test]
    fn names_are_pure_valid_and_change_with_the_salt() {
        assert_eq!(corp_name("Ann", 0), corp_name("Ann", 0));
        assert_ne!(corp_name("Ann", 0), corp_name("Ann", 4));
        for s in 0..8 {
            let n = corp_name("Invented Founder", s);
            assert!(n.len() >= 3 && n.len() <= 50, "{n}");
            let a = alliance_name(&n, s);
            assert!(a.len() >= 3 && a.len() <= 50, "{a}");
            let t = fallback_ticker(&n, s);
            assert!(
                !t.is_empty() && t.len() <= 5 && t.chars().all(|c| c.is_ascii_alphanumeric()),
                "{t}"
            );
        }
    }

    /// C4-fix 4: an applicant whose group the corp's alliance is red to is
    /// refused though the corp itself holds no stance.
    #[test]
    fn the_ceo_refuses_a_group_its_alliance_is_red_to() {
        let mut st = two_corps();
        st.members.insert(BEN, member("Ben", "miner", NPC, &[]));
        st.corps.get_mut(&C1).unwrap().alliance = Some(AL);
        assert!(st.accepts(C1, BEN));
        st.stances.held.insert(
            (AL, BEN),
            society::Held {
                stance: Stance::Red,
                since: Some(0),
            },
        );
        assert!(!st.accepts(C1, BEN));
    }

    /// C4-fix 1: an executor that left, or an old read, is read again.
    #[test]
    fn a_stale_executor_is_read_again() {
        let mut st = two_corps();
        st.corps.get_mut(&C1).unwrap().alliance = Some(AL);
        st.corps.get_mut(&C2).unwrap().alliance = Some(AL);
        st.alliances.insert(
            AL,
            AllianceMind {
                executor: Some(C1),
                read_at: Some(0),
                ..AllianceMind::default()
            },
        );
        let now = DAY / 2;
        let fresh = st.plan(CAS, now, Some(0.0), &BTreeMap::new());
        assert!(!fresh.contains(&Plan::ReadExecutor(AL)), "{fresh:?}");
        // The executor corp leaves the alliance.
        st.corps.get_mut(&C1).unwrap().alliance = None;
        let left = st.plan(CAS, now, Some(0.0), &BTreeMap::new());
        assert!(left.contains(&Plan::ReadExecutor(AL)), "{left:?}");
        // Or the read grows old while the executor looks the same.
        st.corps.get_mut(&C1).unwrap().alliance = Some(AL);
        let old = (EXECUTOR_REREAD_DAYS * DAY as f64) as u64;
        let aged = st.plan(CAS, old, Some(0.0), &BTreeMap::new());
        assert!(aged.contains(&Plan::ReadExecutor(AL)), "{aged:?}");
        // A stale executor does not answer on its own say-so.
        let ann = st.plan(ANN, old, Some(0.0), &BTreeMap::new());
        assert!(!ann.contains(&Plan::AnswerAlliance(AL)), "{ann:?}");
    }

    // ---- SOC-WIRE ----------------------------------------------------------

    #[test]
    fn a_kill_prints_the_victims_view_before_and_after() {
        let mut st = two_corps();
        let e = Event::Kill {
            victim: C1,
            killer: MAL,
            isk: 25_000_000.0,
        };
        let line = st.record_kill_line(7, &e).unwrap().unwrap();
        let after = st.relations.get(C1, MAL);
        assert!(after < 0.0, "{after}");
        assert_eq!(
            line,
            format!("[soc] relation loss victim={C1} killer={MAL} before=0.00 after={after:.2}")
        );
        // The same report again prints nothing and moves nothing.
        assert_eq!(st.record_kill_line(7, &e), None);
        assert_eq!(st.relations.get(C1, MAL), after);
    }

    #[test]
    fn the_summary_ends_with_the_game_day() {
        let mut st = two_corps();
        assert!(st.summary().ends_with(" day 0"), "{}", st.summary());
        st.tick(3 * st.day + 5);
        assert!(st.summary().ends_with(" day 3"), "{}", st.summary());
        assert!(
            st.summary().starts_with("corps 2 alliances"),
            "{}",
            st.summary()
        );
    }

    // ---- R-REL-3 -----------------------------------------------------------

    /// A graded stance, by driving the pair's R through the clock.
    fn graded_at(st: &mut HallState, r: f64, at: u64) {
        st.relations.add(C1, MAL, r);
        st.tick(at);
    }

    #[test]
    fn a_friendly_stance_sets_plus_five_and_a_wary_one_minus_five() {
        let mut st = two_corps();
        // C1's view of MAL at the friendly bar: +5 on the wire, not +10.
        graded_at(&mut st, society::FRIEND_AT + 0.5, 1);
        assert_eq!(st.stances.get5(C1, MAL), Stance5::Friendly);
        assert_eq!(st.stances.get(C1, MAL), Stance::Neutral, "the coarse view");
        assert_eq!(st.desired_contacts(C1).get(&MAL), Some(&5.0));
        // And toward CAS at the wary bar: -5.
        st.relations.add(C1, C2, society::WARY_AT - 0.5);
        st.tick(2);
        assert_eq!(st.stances.get5(C1, C2), Stance5::Wary);
        assert_eq!(st.desired_contacts(C1).get(&C2), Some(&-5.0));
        let diff = st.contact_diff(C1, &BTreeMap::new(), 0);
        assert!(diff.contains(&(MAL, Some(5.0))), "{diff:?}");
        assert!(diff.contains(&(C2, Some(-5.0))), "{diff:?}");
    }

    #[test]
    fn a_graded_stance_does_not_flap_inside_its_band_or_its_window() {
        let mut st = two_corps();
        graded_at(&mut st, society::FRIEND_AT + 0.5, 1);
        assert_eq!(st.stances.get5(C1, MAL), Stance5::Friendly);
        // Dips between the leave point and the enter point: still friendly,
        // and nothing flips inside the window whatever R does.
        let dip = society::FRIEND_AT + 0.5 - (society::FRIEND_AT - society::FRIEND_LEAVE) / 2.0;
        st.relations.r.insert((C1, MAL), dip);
        st.tick(2 + DAY);
        assert_eq!(st.stances.get5(C1, MAL), Stance5::Friendly);
        st.relations.r.insert((C1, MAL), 0.0);
        st.tick(3);
        assert_eq!(
            st.stances.get5(C1, MAL),
            Stance5::Friendly,
            "inside the window"
        );
        // Past the window and under the leave point it goes, and a pair
        // that is neutral again sets no contact.
        st.relations.r.insert((C1, MAL), 0.0);
        st.tick(2 * DAY + 10);
        assert_eq!(st.stances.get5(C1, MAL), Stance5::Neutral);
        assert!(!st.desired_contacts(C1).contains_key(&MAL));
    }

    #[test]
    fn the_graded_stances_round_trip_through_the_state_file() {
        let mut st = two_corps();
        graded_at(&mut st, society::FRIEND_AT + 0.5, 1);
        let back = HallState::parse(&st.render(), DAY).expect("parses");
        assert_eq!(back.stances.graded, st.stances.graded);
        assert_eq!(back.stances.get5(C1, MAL), Stance5::Friendly);
    }

    #[test]
    fn a_ceo_takes_a_friend_at_a_low_fit_and_refuses_a_wary_one() {
        let st = two_corps();
        // A human no bot flies: strangers are taken at the accept line.
        assert!(st.accepts(C1, MAL));
        // A pilot the CEO distrusts needs a much better fit than a
        // stranger has; an enemy is refused whatever the fit.
        assert!(!st.accepts_with(C1, MAL, Some(society::WARY_AT)));
        assert!(!st.accepts_with(C1, MAL, Some(society::ENEMY_AT)));
        // A friend is taken even where the corp's own bar would say no.
        let mut full = two_corps();
        full.members
            .insert(BEN, member("Ben", "ratter", NPC, &[(Trait::Loyalty, 0.5)]));
        assert!(full.accepts_with(C1, BEN, Some(society::FRIEND_AT)));
        // A settled hostile stance of the corp cannot be talked round by a
        // friendly personal view.
        full.stances.graded.insert(
            (C1, BEN),
            society::Held5 {
                stance: Stance5::Hostile,
                since: Some(0),
            },
        );
        assert!(!full.accepts_with(C1, BEN, Some(society::ALLY_AT)));
    }

    #[test]
    fn a_friend_corp_joins_a_bot_alliance_and_a_hostile_one_does_not() {
        let mut st = two_corps();
        st.corps.get_mut(&C1).unwrap().alliance = Some(AL);
        st.alliances.insert(
            AL,
            AllianceMind {
                executor: Some(C1),
                ..AllianceMind::default()
            },
        );
        const HUMAN_CORP: i64 = 98_000_299;
        assert!(!st.alliance_accepts_at(AL, HUMAN_CORP, 1), "a stranger");
        st.relations.add(C1, HUMAN_CORP, society::ALLY_AT * 2.0);
        assert!(st.alliance_accepts_at(AL, HUMAN_CORP, 1), "a friend corp");
        st.relations.add(C1, HUMAN_CORP, -3.0 * society::ALLY_AT);
        assert!(!st.alliance_accepts_at(AL, HUMAN_CORP, 1), "an enemy");
    }

    #[test]
    fn the_ceo_invites_a_friend_after_two_fleet_hours() {
        let mut st = two_corps();
        let seen = Affiliation {
            character_id: MAL,
            corporation_id: NPC,
            alliance_id: None,
        };
        st.see_human(&seen, 5);
        assert!(st.invitations(C1, 10).is_empty(), "no shared fleet time");
        st.note_fleet_hours(MAL, effects::INVITE_FLEET_HOURS);
        assert_eq!(st.invitations(C1, 10), vec![MAL]);
        let plans = st.plan(ANN, 10, Some(0.0), &BTreeMap::new());
        assert!(plans.contains(&Plan::Invite(MAL)), "{plans:?}");
        // Once invited, not again for a while; a pilot in a player corp is
        // never poached; a bot is not a human.
        st.invited.insert((C1, MAL), 10);
        assert!(st.invitations(C1, 11).is_empty());
        assert_eq!(
            st.invitations(C1, 11 + (effects::INVITE_EVERY_DAYS * DAY as f64) as u64),
            vec![MAL]
        );
        st.see_human(
            &Affiliation {
                character_id: CAS,
                corporation_id: C2,
                alliance_id: None,
            },
            5,
        );
        assert!(!st.humans.contains_key(&CAS));
        st.humans.get_mut(&MAL).unwrap().corp = C2;
        assert!(st.invitations(C1, 100 * DAY).is_empty());
    }

    #[test]
    fn r_eff_of_a_pilot_reads_the_corp_and_the_pilots_own_regard() {
        let mut st = two_corps();
        let seen = Affiliation {
            character_id: MAL,
            corporation_id: NPC,
            alliance_id: None,
        };
        let none = st.r_eff_of(C1, &[], 0.5, &seen, 0);
        assert_eq!(none, 0.0);
        let regard = [crate::mind::Regard {
            who: MAL,
            score: -4.0,
            at: 0,
        }];
        assert!(st.r_eff_of(C1, &regard, 0.5, &seen, 0) < none);
        // A --friends pilot is never below the file's standing.
        st.friends.insert(MAL, 3.0);
        assert!(st.r_eff_of(C1, &regard, 0.5, &seen, 0) >= 3.0);
    }

    #[test]
    fn friendly_humans_are_those_a_bot_rates_at_the_friend_bar() {
        let mut st = two_corps();
        let seen = |id| Affiliation {
            character_id: id,
            corporation_id: NPC,
            alliance_id: None,
        };
        st.see_human(&seen(MAL), 1);
        st.see_human(&seen(BEN), 1);
        st.friends.insert(MAL, society::FRIEND_AT + 1.0);
        assert_eq!(
            st.friendly_humans(C1, &[], 0.5, 1),
            [MAL].into_iter().collect()
        );
    }

    /// C4-fix 2: the counted kills stay bounded, newest kept.
    #[test]
    fn the_kill_set_is_bounded() {
        let mut st = HallState::new(DAY);
        for id in 0..(KILLS_KEEP as i64 + 50) {
            st.kills.insert(id);
            while st.kills.len() > KILLS_KEEP {
                st.kills.pop_first();
            }
        }
        assert_eq!(st.kills.len(), KILLS_KEEP);
        assert!(st.kills.contains(&(KILLS_KEEP as i64 + 49)));
        assert!(!st.kills.contains(&0));
    }

    #[test]
    fn review_fix_an_unreadable_society_file_is_kept_as_bad_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("rfix-hall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(STATE_FILE);
        std::fs::write(&path, "soc\t999\nfrom a newer build\n").unwrap();
        let (hall, said) = Hall::open(&dir, DAY);
        assert!(said.is_err());
        hall.lock().dirty = true;
        hall.save().unwrap();
        let bad = dir.join(format!("{STATE_FILE}.bad"));
        assert_eq!(
            std::fs::read_to_string(&bad).unwrap(),
            "soc\t999\nfrom a newer build\n"
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with("soc\t1")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review_fix_trade_regard_is_capped_per_pair_per_day() {
        let mut st = HallState::new(DAY);
        let room = society::K_TRADE_CAP / society::K_TRADE;
        // Many small contracts in one day add up to the cap and no more.
        let small = room / 10.0;
        let got: f64 = (0..30).map(|_| st.trade_allow(ANN, BEN, 5, small)).sum();
        assert!((got - room).abs() < 1e-6 * room, "{got} of {room}");
        assert_eq!(st.trade_allow(BEN, ANN, 6, small), 0.0, "either order");
        // Another pair, and the next game day, are fresh.
        assert!(st.trade_allow(ANN, CAS, 5, small) > 0.0);
        assert!(st.trade_allow(ANN, BEN, 5 + DAY, small) > 0.0);
    }
}
