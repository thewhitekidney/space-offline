//! **The daily economy ledger** (W16 packet CP43): at each rollover, sum the
//! wallet journal by ref, split it into faucets and sinks, count the money
//! humans and bots hold, and write one `[econ]` line.
//!
//! Spec: `docs/content/C5-economy.md` §7.2-§7.4 (packet E8) and §5.4, plan
//! §6.2 W16. Lane note: `docs/lanes/cp43.md`. The GM view is
//! `service/gm_econ.rs` (`/econ`).
//!
//! - **Classification** ([`REFS`], [`classify`]). Every journal ref the
//!   server writes has a [`Flow`]. A [`Flow::System`] row moves money
//!   between a wallet and the world, so its sign says faucet or sink (ref 3
//!   is both: a grant is a faucet, the bot upkeep a sink). A
//!   [`Flow::Market`] row (ref 2) is a faucet or sink only when its order is
//!   an NPC order ([`crate::econ_exchange::is_npc`]); between two
//!   characters it only moves money. [`Flow::Player`] and [`Flow::Held`]
//!   (escrow) rows never change the money supply. A ref with no row is
//!   counted as `unclassified` in the line rather than dropped, and the
//!   test `the_ref_classification_is_total_over_the_refs_in_use` reads the
//!   tree for every ref constant.
//! - **The day** is every journal row written since the last rollover's
//!   cursor (the highest transaction id it read, an `econ` delta), so a
//!   forced rollover or an N-day catch-up never counts a row twice.
//! - **What persists** ([`KIND`], `econ`): per day and ref, `day ref ->
//!   credit debit rows` (C5 §7.3's `econ <day> <ref> <sum>`, with the sum
//!   kept as its two signs); per day, `day total -> faucet sink M_h M_bots
//!   npc_fills trades dbs_min unclassified`; `day closed -> index indexed
//!   bot_to_human human_to_bot`; `day goods <class> -> minted destroyed`
//!   (today's running `goods <class>` rows fold into it); `burn -> on|off`;
//!   and `cursor -> last_tx`.
//!   [`KEEP_DAYS`] days are kept.
//! - **The bot burn** ([`burn`], C5 §7.2): a bot's balance above
//!   [`BOT_WALLET_CAP`] is taken at rollover, journalled as ref 3 with
//!   description [`UPKEEP`]. It is **off** ([`BURN_AT_ROLLOVER`]) until O5's
//!   week of `[econ]` ledger drift (CP43); `/econ` shows what it would take.
//!   `/econ burn on|off` overrides it, persisted ([`burn_enabled`]).
//! - **The closed loop** (R-ECON §2.6, packet E11): the `[econ]` line also
//!   carries [`Closed::part`]: the price index of the day's
//!   character-to-character fills against `P_ref` ([`INDEX_TYPES`] types),
//!   ISK bots paid humans and the reverse, and the goods minted and
//!   destroyed per class ([`goods_of`], counted by `Registry::apply_items`).
//! - **The demand budget** (O5-e, [`demand_take`]): a bot's fill on a
//!   human's ask stops at [`BOT_DEMAND_PER_HUMAN`] a day per human; bot-bot
//!   and human fills are never limited.
//! - **The reward sanity line** ([`reward_levels`], C5 §5.4): per agent
//!   level, the median spawn bounty a mission's base is drawn from, the
//!   security multiplier and the ISK a kill mission pays, so "missions and
//!   ratting pay comparably" can be read off the log.
//!
//! **No controller** (C5 §7.3: phase 1 ships the ledger only).

use crate::damage::BlueTime;
use crate::ledger::{Entry, REF_GM_CASH_TRANSFER};
use crate::service::Registry;
use std::collections::BTreeMap;

/// The delta kind this module owns (`delta_kinds::KINDS`, plan §4.2).
pub const KIND: &str = "econ";

/// C5 §7.2's bot wallet cap **[A]**: falsified by bots that cannot replace
/// a lost hull, or by a week of `[econ]` lines in which `M_bots` still
/// dwarfs `M_h`.
pub const BOT_WALLET_CAP: f64 = 50_000_000.0;

/// Whether the rollover burns bot balances above [`BOT_WALLET_CAP`] when
/// no `/econ burn` row says otherwise ([`burn_enabled`]). **Off** until O5's
/// week of `[econ]` ledger drift (CP43, R-ECON §2.6); a GM turns it on with
/// `/econ burn on`, without a rebuild.
pub const BURN_AT_ROLLOVER: bool = false;

/// The journal description of a burn row (C5 §7.2: the GM-transfer ref with
/// description "upkeep"; the real ref is unknown).
pub const UPKEEP: &str = "upkeep";

/// Days of ledger rows kept **[A]**: enough for O5's week and a month's
/// comparison; falsified by an operator who wants a longer history.
pub const KEEP_DAYS: u32 = 90;

/// The journal's "the system" owner, as `/giveisk` writes it.
const SYSTEM_OWNER: i64 = 1;

/// How a journal ref moves money.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Between a wallet and the world: positive is a faucet, negative a sink.
    System,
    /// A market fill: [`Flow::System`] against an NPC order, else
    /// [`Flow::Player`].
    Market,
    /// Between two characters.
    Player,
    /// Into or out of an escrow the owner gets back or spends later.
    Held,
}

/// Every journal ref the server writes, with its flow and what it is.
/// Refs are `appConst` [V] (each owning module cites its source). Sorted by
/// ref; a new ref constant anywhere in the tree fails
/// `the_ref_classification_is_total_over_the_refs_in_use` until it has a row.
pub const REFS: &[(i64, Flow, &str)] = &[
    // GAP-TRADE: ISK offered in a direct trade (`crate::trade`).
    (1, Flow::Player, "player trading"),
    (2, Flow::Market, "market transaction"),
    (
        3,
        Flow::System,
        "GM cash transfer, starting funds, bot upkeep",
    ),
    (10, Flow::Player, "player donation"),
    // CORP-W: one corporation paying another.
    (11, Flow::Player, "corporation payment"),
    // CORP-O: an office's rent and an impound release, paid by the corp to
    // the station's owner: ISK out of the player economy.
    (13, Flow::System, "office rental"),
    // GAP-BOUNTY: ISK given to a player bounty pool (the prize, 17, is paid out of it).
    // GAP-REPAIR: the station repair shop's bill (`refRepairBill`).
    (15, Flow::System, "repair bill"),
    (16, Flow::Held, "bounty pool"),
    (17, Flow::System, "bounty prize"),
    // CP44: C5 §5.5 pays and charges insurance on ref 19.
    (19, Flow::System, "insurance"),
    (26, Flow::System, "LP store payment"),
    (33, Flow::System, "agent mission reward"),
    (34, Flow::System, "agent mission time bonus"),
    // CORP-W: a member taking ISK out of a corporation division, or moving it
    // between two of its divisions.
    (37, Flow::Player, "corporation account withdrawal"),
    // S7 (#178): a CEO pays the corp wallet out to members or shareholders.
    (38, Flow::Player, "corporation dividend payment"),
    (39, Flow::System, "corporation registration fee"),
    (42, Flow::Held, "market escrow"),
    (46, Flow::System, "broker fee"),
    // SOC-C1: founding an alliance.
    (48, Flow::System, "alliance registration fee"),
    // C8: a war declaration and its weekly bill, and CONCORD's cut of an ally
    // contract, are fees to CONCORD.
    (49, Flow::System, "war fee"),
    // C5 §5.1: sales tax, not charged yet.
    (54, Flow::System, "transaction tax"),
    (56, Flow::System, "manufacturing"),
    (57, Flow::System, "invention"),
    (58, Flow::System, "time efficiency research"),
    (59, Flow::System, "material efficiency research"),
    (60, Flow::System, "copying"),
    // Contracts move money between characters, or hold it for one
    // (`crate::contracts`); neither changes the supply.
    (63, Flow::Held, "contract auction bid"),
    (64, Flow::Held, "contract auction bid refund"),
    (65, Flow::Held, "contract collateral"),
    (66, Flow::Held, "contract reward refund"),
    (67, Flow::Player, "contract auction sold"),
    (68, Flow::Player, "contract reward"),
    (69, Flow::Held, "contract collateral refund"),
    (70, Flow::Player, "contract collateral payout"),
    (71, Flow::Player, "contract price"),
    (83, Flow::Held, "contract reward added"),
    // E6: a kill's bounty is paid in periodic lump rows (`npc::bounty`).
    (85, Flow::System, "bounty prizes"),
    // CORP-W: the corporation tax on a payout. The payout's row carries the
    // net amount and names the tax in its description (the client derives the
    // tax line), so these never head a row. The tax itself is a hidden
    // faucet row under the corporation's id (`service/corp_wallet.rs`).
    (92, Flow::Player, "corporation tax, NPC bounties"),
    (93, Flow::Player, "corporation tax, agent rewards"),
    (94, Flow::Player, "corporation tax, agent bonus rewards"),
    (96, Flow::System, "planetary import tax"),
    (97, Flow::System, "planetary export tax"),
    (98, Flow::System, "planetary construction"),
    // C8: ISK between two sides of a war -- a surrender offer's price (113)
    // and an ally's contract (114) -- changes hands, not the supply.
    (113, Flow::Player, "war surrender fee"),
    (114, Flow::Player, "war ally contract"),
    // CW-1: a kill right bought, paid by the buyer to the holder.
    (116, Flow::Player, "kill right fee"),
    // SEC-1: the security office's per-tag processing fee, paid to CONCORD.
    (117, Flow::System, "security tag processing fee"),
    (127, Flow::System, "reprocessing tax"),
    (135, Flow::System, "reactions"),
    // #89 ESS-1: a share of the ESS main bank paid back to a ratter
    // (`crate::ess`); the bank's cut was never a journal row, so this is a
    // faucet when it lands.
    (155, Flow::System, "ESS main bank autopayment"),
    // P-START-2: a new character's starting funds (`crate::start`).
    (10_004, Flow::System, "new character starting funds"),
];

/// The flow of `entry_type`, `None` for a ref with no [`REFS`] row.
pub fn flow(entry_type: i64) -> Option<Flow> {
    REFS.binary_search_by_key(&entry_type, |r| r.0)
        .ok()
        .map(|i| REFS[i].1)
}

/// What one journal row did to the money supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Faucet,
    Sink,
    /// Moved between wallets, or into or out of escrow.
    Moved,
    /// A ref [`REFS`] does not list.
    Unclassified,
}

/// Classify one journal row.
pub fn classify(e: &Entry) -> Class {
    let by_sign = || {
        if e.amount >= 0.0 {
            Class::Faucet
        } else {
            Class::Sink
        }
    };
    match flow(e.entry_type_id) {
        Some(Flow::System) => by_sign(),
        Some(Flow::Market) if crate::econ_exchange::is_npc(e.reference_id) => by_sign(),
        Some(Flow::Market | Flow::Player | Flow::Held) => Class::Moved,
        None => Class::Unclassified,
    }
}

// ---- the summary ------------------------------------------------------------------

/// One ref's journal rows over a window.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RefSum {
    /// ISK in, positive.
    pub credit: f64,
    /// ISK out, positive.
    pub debit: f64,
    pub rows: usize,
}

/// The journal over a window, classified.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub by_ref: BTreeMap<i64, RefSum>,
    /// ISK into wallets from the world.
    pub faucet: f64,
    /// ISK out of wallets to the world, positive.
    pub sink: f64,
    pub unclassified: usize,
    /// The highest transaction id read, 0 for none.
    pub last_tx: i64,
}

/// Sum `entries` by ref and by class.
pub fn summarize<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> Summary {
    let mut s = Summary::default();
    for e in entries {
        let r = s.by_ref.entry(e.entry_type_id).or_default();
        if e.amount >= 0.0 {
            r.credit += e.amount;
        } else {
            r.debit -= e.amount;
        }
        r.rows += 1;
        match classify(e) {
            Class::Faucet => s.faucet += e.amount,
            Class::Sink => s.sink -= e.amount,
            Class::Moved => {}
            Class::Unclassified => s.unclassified += 1,
        }
        s.last_tx = s.last_tx.max(e.transaction_id);
    }
    s
}

/// The journal rows after `cursor` (a transaction id), summarized.
pub fn since(reg: &Registry, cursor: i64) -> Summary {
    summarize(
        reg.journal()
            .entries()
            .iter()
            .filter(|e| e.transaction_id > cursor),
    )
}

/// The cursor the last rollover left, 0 before the first.
pub fn cursor(reg: &Registry) -> i64 {
    reg.deltas()
        .of(KIND)
        .get(&vec!["cursor".to_owned()])
        .and_then(|v| v.first()?.parse().ok())
        .unwrap_or(0)
}

/// `(M_h, M_bots)`: the balances of every human and every bot character.
pub fn money(reg: &Registry) -> (f64, f64) {
    // CORP-W: the corporations' wallets count too, by whether the CEO is a bot.
    let (corp_h, corp_bots) = crate::service::corp_wallet::corp_money(reg);
    reg.config()
        .characters
        .iter()
        .fold((corp_h, corp_bots), |(h, b), c| {
            if c.is_bot {
                (h, b + c.balance)
            } else {
                (h + c.balance, b)
            }
        })
}

/// `(npc_fills, trades)`: market fills at or after `since` with an NPC on
/// one side, and those between two characters.
pub fn fills(reg: &Registry, since: BlueTime) -> (usize, usize) {
    let fills = reg.market_fills_since(since);
    let trades = fills
        .iter()
        .filter(|f| reg.character(f.buyer_id).is_some() && reg.character(f.seller_id).is_some())
        .count();
    (fills.len() - trades, trades)
}

/// The NPC fills at or after `since`, by type: `(type_id, value, quantity)`,
/// highest value first, at most `n`.
pub fn top_npc_fills(reg: &Registry, since: BlueTime, n: usize) -> Vec<(i64, f64, i64)> {
    let mut by_type: BTreeMap<i64, (f64, i64)> = BTreeMap::new();
    for f in reg.market_fills_since(since) {
        if reg.character(f.buyer_id).is_some() && reg.character(f.seller_id).is_some() {
            continue;
        }
        let e = by_type.entry(f.type_id).or_default();
        e.0 += f.price * f.quantity as f64;
        e.1 += f.quantity;
    }
    let mut v: Vec<(i64, f64, i64)> = by_type.into_iter().map(|(t, (v, q))| (t, v, q)).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

/// The lowest dynamic-bounty multiplier of any system with a `dbs` row at
/// `now`; the default when none has one.
pub fn dbs_min(reg: &Registry, now: BlueTime) -> f64 {
    reg.deltas()
        .of(crate::econ_dbs::KIND)
        .keys()
        .filter_map(|k| k.first()?.parse::<i64>().ok())
        .map(|system| crate::econ_dbs::multiplier(reg, system, now))
        .fold(crate::econ_dbs::DEFAULT, f64::min)
}

/// One day's `[econ]` figures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Totals {
    pub day: u32,
    pub faucet: f64,
    pub sink: f64,
    pub m_h: f64,
    pub m_bots: f64,
    pub npc_fills: usize,
    pub trades: usize,
    pub dbs_min: f64,
    pub unclassified: usize,
}

impl Totals {
    /// The figures of `s` and the world at `now`, fills counted from
    /// `fills_since`.
    pub fn of(
        reg: &Registry,
        day: u32,
        s: &Summary,
        fills_since: BlueTime,
        now: BlueTime,
    ) -> Totals {
        let (m_h, m_bots) = money(reg);
        let (npc_fills, trades) = fills(reg, fills_since);
        Totals {
            day,
            faucet: s.faucet,
            sink: s.sink,
            m_h,
            m_bots,
            npc_fills,
            trades,
            dbs_min: dbs_min(reg, now),
            unclassified: s.unclassified,
        }
    }

    /// C5 §7.4's line. `label` follows the day (`""`, or `" so far"`).
    pub fn line(&self, label: &str) -> String {
        format!(
            "[econ] day {}{label} faucet={:.0} sink={:.0} net={:.0} M_h={:.0} M_bots={:.0} \
             npc_fills={} trades={} dbs_min={:.2} unclassified={}",
            self.day,
            self.faucet,
            self.sink,
            self.faucet - self.sink,
            self.m_h,
            self.m_bots,
            self.npc_fills,
            self.trades,
            self.dbs_min,
            self.unclassified,
        )
    }

    fn to_row(self) -> Vec<String> {
        vec![
            self.faucet.to_string(),
            self.sink.to_string(),
            self.m_h.to_string(),
            self.m_bots.to_string(),
            self.npc_fills.to_string(),
            self.trades.to_string(),
            self.dbs_min.to_string(),
            self.unclassified.to_string(),
        ]
    }

    fn from_row(day: u32, v: &[String]) -> Option<Totals> {
        let [f, s, h, b, n, t, d, u] = v else {
            return None;
        };
        Some(Totals {
            day,
            faucet: f.parse().ok()?,
            sink: s.parse().ok()?,
            m_h: h.parse().ok()?,
            m_bots: b.parse().ok()?,
            npc_fills: n.parse().ok()?,
            trades: t.parse().ok()?,
            dbs_min: d.parse().ok()?,
            unclassified: u.parse().ok()?,
        })
    }
}

/// Every stored day's totals, oldest first (an unreadable row is skipped).
pub fn history(reg: &Registry) -> Vec<Totals> {
    let mut v: Vec<Totals> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| match &k[..] {
            [day, t] if t == "total" => Totals::from_row(day.parse().ok()?, v),
            _ => None,
        })
        .collect();
    v.sort_by_key(|t| t.day);
    v
}

// ---- the bot burn ---------------------------------------------------------------

/// Every bot over [`BOT_WALLET_CAP`]: `(character_id, excess)`.
pub fn over_cap(reg: &Registry) -> Vec<(i64, f64)> {
    reg.config()
        .characters
        .iter()
        .filter(|c| c.is_bot && c.balance > BOT_WALLET_CAP)
        .map(|c| (c.character_id, c.balance - BOT_WALLET_CAP))
        .collect()
}

/// Take every bot's balance above [`BOT_WALLET_CAP`] (C5 §7.2): the balance
/// becomes the cap, a ref-3 [`UPKEEP`] row records the debit, and the bot is
/// told. A human is never touched. Returns the ISK burned.
pub fn burn(reg: &mut Registry, now: BlueTime) -> f64 {
    let mut total = 0.0;
    for (id, excess) in over_cap(reg) {
        let Some(c) = reg.character_mut(id) else {
            continue;
        };
        c.balance = BOT_WALLET_CAP;
        reg.mark_dirty();
        reg.journal_mut().record(
            now,
            REF_GM_CASH_TRANSFER,
            id,
            SYSTEM_OWNER,
            0,
            -excess,
            BOT_WALLET_CAP,
            UPKEEP,
        );
        reg.push(crate::notify::Addressed {
            to: Some(vec![id]),
            inner: Box::new(crate::notify::AccountChange {
                owner_id: id,
                balance: BOT_WALLET_CAP,
            }),
        });
        total += excess;
    }
    total
}

// ---- the reward sanity line -----------------------------------------------------

/// What a kill mission at one level pays against what its pockets' rats
/// carry (C5 §5.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RewardLevel {
    pub level: i64,
    /// Kill missions whose pocket bounty the base is the median of; 0 means
    /// the base is the dungeon-budget fallback.
    pub pockets: usize,
    /// `mission_rewards::isk_base`: the median spawn bounty.
    pub base: f64,
    /// The security row's `k_isk`.
    pub k_isk: f64,
    /// The ISK a kill mission at this level pays, before the bonus.
    pub reward: i64,
}

/// [`RewardLevel`] for every agent level.
pub fn reward_levels(reg: &Registry) -> Vec<RewardLevel> {
    use crate::mission_rewards as mr;
    let table = reg.content().get::<mr::Table>();
    mr::LEVELS
        .map(|level| {
            let base = mr::isk_base(reg, level);
            let k_isk = table.row(mr::Scale::Security, level).k_isk;
            RewardLevel {
                level,
                pockets: mr::spawn_bounty_sums(reg, level).len(),
                base,
                k_isk,
                // `mission_rewards::reward`'s own rounding.
                reward: (k_isk * base).round() as i64,
            }
        })
        .collect()
}

/// The sanity line: each level's base, multiplier and reward, then what
/// missions (refs 33, 34) and bounties (refs 17, 85) paid in `s`.
pub fn reward_line(reg: &Registry, s: &Summary) -> String {
    let levels = reward_levels(reg)
        .iter()
        .map(|l| {
            let from = if l.pockets > 0 {
                format!("median of {}", l.pockets)
            } else {
                "budget".to_owned()
            };
            format!(
                "L{} base={:.0} ({from}) x{} reward={}",
                l.level, l.base, l.k_isk, l.reward
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let paid = |r: i64| s.by_ref.get(&r).copied().unwrap_or_default();
    let (m33, m34, b17, b85) = (paid(33), paid(34), paid(17), paid(85));
    format!(
        "[econ] rewards {levels} | paid: missions {:.0} over {} rows, bounties {:.0} over {} rows",
        m33.credit + m34.credit,
        m33.rows + m34.rows,
        b17.credit + b85.credit,
        b17.rows + b85.rows
    )
}

/// The burn switch's row (`econ burn -> on|off`), written by `/econ burn`.
const BURN_KEY: &str = "burn";

/// Whether the rollover burns: the `/econ burn` row when there is one, else
/// [`BURN_AT_ROLLOVER`].
pub fn burn_enabled(reg: &Registry) -> bool {
    match reg
        .deltas()
        .of(KIND)
        .get(&vec![BURN_KEY.to_owned()])
        .and_then(|v| v.first().cloned())
        .as_deref()
    {
        Some("on") => true,
        Some("off") => false,
        _ => BURN_AT_ROLLOVER,
    }
}

/// Turn the rollover burn on or off (`/econ burn on|off`), persisted.
pub fn set_burn(reg: &mut Registry, on: bool) {
    let v = if on { "on" } else { "off" };
    reg.deltas_mut()
        .put(KIND, vec![BURN_KEY.to_owned()], vec![v.to_owned()]);
    reg.mark_dirty();
}

// ---- E11: goods minted and destroyed --------------------------------------------

/// `categoryCelestial` [V]: wrecks and cargo containers, which are not goods
/// (their contents are counted on their own).
const CATEGORY_CELESTIAL: i64 = 2;
/// `appConst` categories [V] the goods classes are cut on.
const CATEGORY_MATERIAL: i64 = 4;
const CATEGORY_MODULE: i64 = 7;
const CATEGORY_CHARGE: i64 = 8;
const CATEGORY_DRONE: i64 = 18;
const CATEGORY_ASTEROID: i64 = 25;

/// The goods class of an item category, `None` for what is not a good
/// (wrecks and containers). The cut is **[A]**: one class per category the
/// loop of R-ECON §2.2 moves, the rest `other`; falsified by a glut the
/// `[econ]` line cannot place.
pub fn goods_class(category_id: i64) -> Option<&'static str> {
    use crate::econ_exchange::{CATEGORY_BLUEPRINT, CATEGORY_COMMODITY, CATEGORY_SKILL};
    Some(match category_id {
        CATEGORY_CELESTIAL => return None,
        crate::model::CATEGORY_SHIP => "ship",
        CATEGORY_MODULE => "module",
        CATEGORY_CHARGE => "charge",
        CATEGORY_DRONE => "drone",
        CATEGORY_ASTEROID => "ore",
        CATEGORY_MATERIAL => "material",
        CATEGORY_BLUEPRINT => "blueprint",
        CATEGORY_SKILL => "skill",
        CATEGORY_COMMODITY => "commodity",
        _ => "other",
    })
}

/// Units minted and destroyed, by goods class.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Goods(pub BTreeMap<&'static str, (i64, i64)>);

impl Goods {
    /// Add `units` (positive minted, negative destroyed) of `class`.
    fn add(&mut self, class: &'static str, units: i64) {
        let e = self.0.entry(class).or_default();
        if units >= 0 {
            e.0 += units;
        } else {
            e.1 -= units;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.values().all(|&(m, d)| m == 0 && d == 0)
    }

    /// `(minted, destroyed)` over every class.
    pub fn total(&self) -> (i64, i64) {
        self.0
            .values()
            .fold((0, 0), |(m, d), (a, b)| (m + a, d + b))
    }

    /// `class:minted/destroyed,...`, `-` for none.
    pub fn part(&self) -> String {
        let v: Vec<String> = self
            .0
            .iter()
            .filter(|(_, (m, d))| *m != 0 || *d != 0)
            .map(|(c, (m, d))| format!("{c}:{m}/{d}"))
            .collect();
        if v.is_empty() {
            "-".to_owned()
        } else {
            v.join(",")
        }
    }
}

/// What an item plan mints and destroys, read **before** it runs (a
/// destroyed item is gone after). Per type the plan is netted first, so a
/// move written as consume-here-create-there (a reload) counts nothing, and
/// a refine counts its ore destroyed and its minerals minted. Moves, splits
/// and merges never count. `Registry::apply_items` adds the result once the
/// plan applied ([`add_goods`]).
pub fn goods_of(
    store: &crate::item::Store,
    content: &crate::content::Content,
    edits: &[crate::item::Edit],
) -> Goods {
    use crate::item::Edit;
    let mut by_type: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
    for e in edits {
        let (type_id, category_id, units) = match *e {
            Edit::Create {
                type_id, quantity, ..
            } => match crate::item::find_in(content, type_id) {
                Some(t) => (type_id, t.category_id, quantity),
                None => continue,
            },
            Edit::Grow { item_id, quantity } => match store.get(item_id) {
                Some(i) => (i.type_id, i.category_id, quantity),
                None => continue,
            },
            Edit::Consume { item_id, quantity } => match store.get(item_id) {
                Some(i) => (i.type_id, i.category_id, -quantity),
                None => continue,
            },
            Edit::Destroy { item_id } => match store.get(item_id) {
                Some(i) => (i.type_id, i.category_id, -i.quantity),
                None => continue,
            },
            _ => continue,
        };
        let e = by_type.entry(type_id).or_insert((category_id, 0));
        e.1 += units;
    }
    let mut g = Goods::default();
    for (category_id, net) in by_type.into_values() {
        if let Some(class) = goods_class(category_id).filter(|_| net != 0) {
            g.add(class, net);
        }
    }
    g
}

/// The running row of `class` (`econ goods <class> -> minted destroyed`),
/// folded into the day at rollover.
fn goods_key(class: &str) -> Vec<String> {
    vec!["goods".to_owned(), class.to_owned()]
}

fn read_pair(v: &[String]) -> Option<(i64, i64)> {
    let [m, d] = v else { return None };
    Some((m.parse().ok()?, d.parse().ok()?))
}

/// Add `g` to today's running goods rows.
pub fn add_goods(reg: &mut Registry, g: &Goods) {
    if g.is_empty() {
        return;
    }
    let deltas = reg.deltas_mut();
    for (class, (m, d)) in &g.0 {
        let key = goods_key(class);
        let (m0, d0) = deltas
            .of(KIND)
            .get(&key)
            .and_then(|v| read_pair(v))
            .unwrap_or_default();
        deltas.put(KIND, key, vec![(m0 + m).to_string(), (d0 + d).to_string()]);
    }
}

/// The goods minted and destroyed since the last rollover.
pub fn goods_today(reg: &Registry) -> Goods {
    let mut g = Goods::default();
    for (k, v) in reg.deltas().of(KIND) {
        if let ([goods, class], Some(pair)) = (&k[..], read_pair(v))
            && goods == "goods"
            && let Some(c) = static_class(class)
        {
            g.0.insert(c, pair);
        }
    }
    g
}

/// One stored day's goods.
pub fn goods_of_day(reg: &Registry, day: u32) -> Goods {
    let day = day.to_string();
    let mut g = Goods::default();
    for (k, v) in reg.deltas().of(KIND) {
        if let ([d, goods, class], Some(pair)) = (&k[..], read_pair(v))
            && *d == day
            && goods == "goods"
            && let Some(c) = static_class(class)
        {
            g.0.insert(c, pair);
        }
    }
    g
}

/// Every class name [`goods_class`] gives, for reading a row back.
const CLASSES: [&str; 10] = [
    "ship",
    "module",
    "charge",
    "drone",
    "ore",
    "material",
    "blueprint",
    "skill",
    "commodity",
    "other",
];

fn static_class(name: &str) -> Option<&'static str> {
    CLASSES.iter().copied().find(|c| *c == name)
}

/// Move today's running goods rows into `day`'s and clear them.
fn fold_goods(reg: &mut Registry, day: u32) -> Goods {
    let g = goods_today(reg);
    let deltas = reg.deltas_mut();
    for (class, (m, d)) in &g.0 {
        deltas.del(KIND, &goods_key(class));
        deltas.put(
            KIND,
            vec![day.to_string(), "goods".to_owned(), (*class).to_owned()],
            vec![m.to_string(), d.to_string()],
        );
    }
    g
}

// ---- E11: the price index and the bot-human flow --------------------------------

/// How many types the price index weighs (R-ECON §2.6: ten).
pub const INDEX_TYPES: usize = 10;

/// R-ECON §2.6's band **[A]**: the index outside `[lo, hi] × P_ref` for
/// [`INDEX_DAYS`] days in a row is the falsifier of the closed loop.
pub const INDEX_BAND: (f64, f64) = (0.30, 4.0);
pub const INDEX_DAYS: usize = 3;

/// One window's closed-loop figures beside the ISK ledger.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Closed {
    /// `Σ value / Σ P_ref × units` over the [`INDEX_TYPES`] types with the
    /// most character-to-character value; `None` with no such trade.
    pub index: Option<f64>,
    /// How many types the index weighed.
    pub indexed: usize,
    /// `(type_id, average price / P_ref)` of each indexed type, by value;
    /// empty for a stored day (only the index is kept).
    pub index_types: Vec<(i64, f64)>,
    /// ISK bots paid humans (a bot bought from a human), and the reverse.
    pub bot_to_human: f64,
    pub human_to_bot: f64,
    pub goods: Goods,
}

/// Which side a character trades on: `Some(true)` a bot, `Some(false)` a
/// human, `None` not a character (an NPC).
fn is_bot(reg: &Registry, id: i64) -> Option<bool> {
    reg.character(id).map(|c| c.is_bot)
}

impl Closed {
    /// The index and the flow from fills at or after `since`, with `goods`.
    pub fn of(reg: &Registry, since: BlueTime, goods: Goods) -> Closed {
        let mut by_type: BTreeMap<i64, (f64, i64)> = BTreeMap::new();
        let (mut b2h, mut h2b) = (0.0, 0.0);
        for f in reg.market_fills_since(since) {
            let (Some(buyer), Some(seller)) = (is_bot(reg, f.buyer_id), is_bot(reg, f.seller_id))
            else {
                continue;
            };
            let value = f.price * f.quantity as f64;
            match (buyer, seller) {
                (true, false) => b2h += value,
                (false, true) => h2b += value,
                _ => {}
            }
            let e = by_type.entry(f.type_id).or_default();
            e.0 += value;
            e.1 += f.quantity;
        }
        let mut priced: Vec<(i64, f64, f64)> = by_type
            .into_iter()
            .filter_map(|(t, (v, q))| {
                let p = crate::econ_prices::p_ref(reg, t).filter(|p| *p > 0.0)?;
                Some((t, v, p * q as f64))
            })
            .collect();
        priced.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        priced.truncate(INDEX_TYPES);
        let (v, r) = priced
            .iter()
            .fold((0.0, 0.0), |(v, r), (_, a, b)| (v + a, r + b));
        Closed {
            index: (r > 0.0).then(|| v / r),
            indexed: priced.len(),
            index_types: priced.iter().map(|(t, a, b)| (*t, a / b)).collect(),
            bot_to_human: b2h,
            human_to_bot: h2b,
            goods,
        }
    }

    /// What the `[econ]` line appends.
    pub fn part(&self) -> String {
        let index = self.index.map_or("-".to_owned(), |i| format!("{i:.3}"));
        format!(
            "index={index}/{} bot>human={:.0} human>bot={:.0} goods={}",
            self.indexed,
            self.bot_to_human,
            self.human_to_bot,
            self.goods.part()
        )
    }

    fn to_row(&self) -> Vec<String> {
        vec![
            self.index.map_or(String::new(), |i| i.to_string()),
            self.indexed.to_string(),
            self.bot_to_human.to_string(),
            self.human_to_bot.to_string(),
        ]
    }
}

/// One stored day's closed-loop figures, with its goods.
pub fn closed_of_day(reg: &Registry, day: u32) -> Option<Closed> {
    let v = reg
        .deltas()
        .of(KIND)
        .get(&vec![day.to_string(), "closed".to_owned()])?;
    let [i, n, b2h, h2b] = &v[..] else {
        return None;
    };
    Some(Closed {
        index: i.parse().ok(),
        indexed: n.parse().ok()?,
        index_types: Vec::new(),
        bot_to_human: b2h.parse().ok()?,
        human_to_bot: h2b.parse().ok()?,
        goods: goods_of_day(reg, day),
    })
}

// ---- E11: the demand budget (O5-e) ----------------------------------------------

/// C5 §7.2's `BOT_DEMAND_BUDGET`, kept **per human seller** **[A]**: what
/// bots together may pay one human for goods off that human's asks in one
/// day. Bots summed over every active human gives C5's `4 M × humans`
/// without a count of "humans active today" the server does not keep.
/// Falsified by a human who cannot sell a bot the goods it wants on an
/// ordinary day, or by `M_h` growing on `bot>human` alone.
pub const BOT_DEMAND_PER_HUMAN: f64 = 4_000_000.0;

/// What bots may still pay `seller` today, `None` when the budget does not
/// apply: the buyer is not a bot, or the seller is not a human (another bot,
/// or an NPC order). **Bot-bot fills are never limited** (O5-e).
pub fn demand_left(reg: &Registry, buyer: i64, seller: i64) -> Option<f64> {
    if is_bot(reg, buyer) != Some(true) || is_bot(reg, seller) != Some(false) {
        return None;
    }
    let paid: f64 = reg
        .market_fills_since(reg.world().last_rollover)
        .iter()
        .filter(|f| f.seller_id == seller && is_bot(reg, f.buyer_id) == Some(true))
        .map(|f| f.price * f.quantity as f64)
        .sum();
    Some((BOT_DEMAND_PER_HUMAN - paid).max(0.0))
}

/// **The fill-refusal hook** (`service/market.rs`'s buy plan): how many of
/// `take` units at `price` from `seller`'s ask a `buyer` may fill, given
/// `planned` ISK already planned to the same seller in this order. `take`
/// unchanged when [`demand_left`] does not apply; 0 refuses the fill.
pub fn demand_take(
    reg: &Registry,
    buyer: i64,
    seller: i64,
    price: f64,
    take: i64,
    planned: f64,
) -> i64 {
    match demand_left(reg, buyer, seller) {
        None => take,
        Some(_) if price <= 0.0 => take,
        Some(left) => take.min(((left - planned).max(0.0) / price).floor() as i64),
    }
}

/// `(human, paid today)` for every human bots bought from since the last
/// rollover, highest first.
pub fn demand_spent(reg: &Registry) -> Vec<(i64, f64)> {
    let mut by: BTreeMap<i64, f64> = BTreeMap::new();
    for f in reg.market_fills_since(reg.world().last_rollover) {
        if is_bot(reg, f.buyer_id) == Some(true) && is_bot(reg, f.seller_id) == Some(false) {
            *by.entry(f.seller_id).or_default() += f.price * f.quantity as f64;
        }
    }
    let mut v: Vec<(i64, f64)> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

// ---- the rollover step ----------------------------------------------------------

/// **The rollover step** (`rollover_steps::STEPS`, plan §4.4 step 7):
/// [`step`] with the burn as [`burn_enabled`] says.
pub fn on_rollover(reg: &mut Registry, d: &crate::rollover::Day) -> usize {
    let burn = burn_enabled(reg);
    step(reg, d, burn)
}

/// Burn (when `burn_bots`), summarize the journal since the cursor, store
/// the day's rows (the ISK totals, the closed-loop figures and the goods),
/// prune days older than [`KEEP_DAYS`], and print the `[econ]` line and the
/// reward line. Returns the journal rows read.
pub fn step(reg: &mut Registry, d: &crate::rollover::Day, burn_bots: bool) -> usize {
    if burn_bots {
        burn(reg, d.now);
    }
    let s = since(reg, cursor(reg));
    // Lane clock: the days just ended are compressed days.
    let fills_since =
        d.now - crate::game_clock::span(reg, i64::from(d.days.max(1)) * crate::seed::DAY);
    let totals = Totals::of(reg, d.day, &s, fills_since, d.now);
    let goods = fold_goods(reg, d.day);
    let closed = Closed::of(reg, fills_since, goods);
    let day = d.day.to_string();
    let deltas = reg.deltas_mut();
    deltas.put(
        KIND,
        vec![day.clone(), "closed".to_owned()],
        closed.to_row(),
    );
    for (r, sum) in &s.by_ref {
        deltas.put(
            KIND,
            vec![day.clone(), r.to_string()],
            vec![
                sum.credit.to_string(),
                sum.debit.to_string(),
                sum.rows.to_string(),
            ],
        );
    }
    deltas.put(KIND, vec![day, "total".to_owned()], totals.to_row());
    if s.last_tx > 0 {
        deltas.put(KIND, vec!["cursor".to_owned()], vec![s.last_tx.to_string()]);
    }
    prune(reg, d.day);
    crate::outln!("{} {}", totals.line(""), closed.part());
    crate::outln!("{}", reward_line(reg, &s));
    s.by_ref.values().map(|r| r.rows).sum()
}

/// Drop every day row more than [`KEEP_DAYS`] before `day`.
fn prune(reg: &mut Registry, day: u32) {
    let old: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| {
            k.len() >= 2
                && k[0]
                    .parse::<u32>()
                    .is_ok_and(|d| d.saturating_add(KEEP_DAYS) < day)
        })
        .cloned()
        .collect();
    if old.is_empty() {
        return;
    }
    let deltas = reg.deltas_mut();
    for k in old {
        deltas.del(KIND, &k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::World;
    use crate::model::{Character, SandboxConfig};
    use crate::rollover::Day;
    use std::collections::BTreeSet;
    use std::path::Path;

    const HUMAN: i64 = 90_000_001;
    const BOT: i64 = 90_000_002;
    const POOR_BOT: i64 = 90_000_003;

    fn reg() -> Registry {
        let mut w = Registry::new(SandboxConfig::default());
        w.deltas_mut().set_world(World::FIXTURE);
        w
    }

    /// The default human plus two invented bots.
    fn with_bots(human: f64, bot: f64, poor: f64) -> Registry {
        let mut w = reg();
        w.character_mut(HUMAN).unwrap().balance = human;
        for (id, ship, balance) in [(BOT, 100_000_002, bot), (POOR_BOT, 100_000_003, poor)] {
            w.seed_character(Character {
                character_id: id,
                ship_id: ship,
                balance,
                is_bot: true,
                ..Character::default()
            });
        }
        w
    }

    fn day(day: u32) -> Day {
        Day {
            day,
            days: 1,
            now: World::FIXTURE.birth + i64::from(day) * crate::seed::DAY,
        }
    }

    fn entry(entry_type_id: i64, amount: f64, reference_id: i64) -> Entry {
        Entry {
            transaction_id: 1,
            date: World::FIXTURE.birth,
            entry_type_id,
            owner_id_1: SYSTEM_OWNER,
            owner_id_2: HUMAN,
            reference_id,
            amount,
            balance: 0.0,
            description: String::new(),
        }
    }

    /// Every `const REF_*` / `const ENTRY_TYPE_*: i64 = N` in this crate's
    /// source, by value, with where it was found.
    fn ref_consts() -> BTreeMap<i64, String> {
        fn walk(dir: &Path, out: &mut BTreeMap<i64, String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    for line in text.lines() {
                        let t = line.trim_start();
                        let t = t.strip_prefix("pub(crate) ").unwrap_or(t);
                        let t = t.strip_prefix("pub ").unwrap_or(t);
                        let Some(rest) = t.strip_prefix("const ") else {
                            continue;
                        };
                        let Some((name, value)) = rest.split_once(": i64 =") else {
                            continue;
                        };
                        let is_ref = name.starts_with("REF_")
                            || (name.starts_with("ENTRY_TYPE_")
                                && !name.starts_with("ENTRY_TYPE_NAME_ID"));
                        if !is_ref {
                            continue;
                        }
                        let n: i64 = value
                            .trim()
                            .trim_end_matches(';')
                            .replace('_', "")
                            .parse()
                            .unwrap_or_else(|e| panic!("{name} in {p:?}: {e}"));
                        out.insert(n, format!("{name} ({})", p.display()));
                    }
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
        out
    }

    #[test]
    fn the_ref_classification_is_total_over_the_refs_in_use() {
        let mut in_use = ref_consts();
        // The scan sees the constants it must: one per kind of writer.
        for r in [
            crate::npc::REF_BOUNTY_PRIZE,
            crate::mission_rewards::REF_AGENT_MISSION_REWARD,
            crate::contracts::REF_PRICE,
            crate::reprocess::ENTRY_TYPE_REPROCESSING_TAX,
            crate::loyalty::REF_PAYMENT_TO_LP_STORE,
            REF_GM_CASH_TRANSFER,
        ] {
            assert!(in_use.contains_key(&r), "the scan missed ref {r}");
        }
        // Industry spends its install fees through a table, not a const.
        for (_, r) in crate::industry::ACCOUNT_ENTRY_TYPES {
            in_use.insert(r, "industry::ACCOUNT_ENTRY_TYPES".to_owned());
        }
        let missing: Vec<String> = in_use
            .iter()
            .filter(|(r, _)| flow(**r).is_none())
            .map(|(r, at)| format!("{r}: {at}"))
            .collect();
        assert!(missing.is_empty(), "refs with no REFS row: {missing:?}");
    }

    #[test]
    fn refs_are_sorted_and_unique() {
        for w in REFS.windows(2) {
            assert!(w[0].0 < w[1].0, "{} then {}", w[0].0, w[1].0);
        }
        let ids: BTreeSet<i64> = REFS.iter().map(|r| r.0).collect();
        assert_eq!(ids.len(), REFS.len());
        for (r, f, _) in REFS {
            assert_eq!(flow(*r), Some(*f));
        }
    }

    #[test]
    fn a_market_fill_is_a_faucet_or_sink_only_against_an_npc_order() {
        let npc = crate::econ_exchange::NPC_ORDER_BASE + 7;
        let player = 5_000_000_001;
        assert_eq!(
            classify(&entry(2, -10.0, npc)),
            Class::Sink,
            "bought from an NPC ask"
        );
        assert_eq!(
            classify(&entry(2, 10.0, npc)),
            Class::Faucet,
            "sold to an NPC bid"
        );
        assert_eq!(classify(&entry(2, 10.0, player)), Class::Moved);
        assert_eq!(classify(&entry(2, -10.0, player)), Class::Moved);
        assert_eq!(classify(&entry(42, -10.0, player)), Class::Moved, "escrow");
        assert_eq!(classify(&entry(17, 10.0, 0)), Class::Faucet);
        assert_eq!(classify(&entry(46, -10.0, 0)), Class::Sink);
        assert_eq!(classify(&entry(3, 10.0, 0)), Class::Faucet, "a grant");
        assert_eq!(classify(&entry(3, -10.0, 0)), Class::Sink, "the upkeep");
        assert_eq!(classify(&entry(-5, 10.0, 0)), Class::Unclassified);
    }

    #[test]
    fn the_summary_splits_faucets_sinks_and_unclassified_rows() {
        let npc = crate::econ_exchange::NPC_ORDER_BASE;
        let rows = [
            entry(17, 100.0, 0),
            entry(33, 50.0, 0),
            entry(2, -30.0, npc),
            entry(2, 40.0, 5_000_000_001),
            entry(46, -5.0, 0),
            entry(-5, 1.0, 0),
        ];
        let s = summarize(&rows);
        assert_eq!(s.faucet, 150.0);
        assert_eq!(s.sink, 35.0);
        assert_eq!(s.unclassified, 1);
        let two = s.by_ref[&2];
        assert_eq!((two.credit, two.debit, two.rows), (40.0, 30.0, 2));
    }

    #[test]
    fn the_bot_burn_caps_at_bot_wallet_cap_and_journals_it() {
        let over = BOT_WALLET_CAP * 3.0;
        let under = BOT_WALLET_CAP / 2.0;
        let mut w = with_bots(over, over, under);
        let rows_before = w.journal().entries().len();
        let burned = burn(&mut w, day(1).now);
        assert_eq!(burned, over - BOT_WALLET_CAP);
        assert_eq!(w.character(BOT).unwrap().balance, BOT_WALLET_CAP);
        assert_eq!(
            w.character(POOR_BOT).unwrap().balance,
            under,
            "under the cap"
        );
        assert_eq!(w.character(HUMAN).unwrap().balance, over, "never a human");
        let new = &w.journal().entries()[rows_before..];
        assert_eq!(new.len(), 1, "{new:?}");
        let e = &new[0];
        assert_eq!(e.entry_type_id, REF_GM_CASH_TRANSFER);
        assert_eq!(e.description, UPKEEP);
        assert_eq!(e.wallet(), BOT);
        assert_eq!(e.amount, -(over - BOT_WALLET_CAP));
        assert_eq!(e.balance, BOT_WALLET_CAP);
        assert_eq!(classify(e), Class::Sink);
        assert_eq!(burn(&mut w, day(1).now), 0.0, "a second burn takes nothing");
    }

    #[test]
    fn the_day_records_the_burn_as_a_sink_and_the_money_by_side() {
        let over = BOT_WALLET_CAP * 2.0;
        let mut w = with_bots(1_000.0, over, 0.0);
        let read = step(&mut w, &day(1), true);
        assert_eq!(read, 1);
        let t = history(&w);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].day, 1);
        assert_eq!(t[0].sink, over - BOT_WALLET_CAP);
        assert_eq!(t[0].faucet, 0.0);
        assert_eq!((t[0].m_h, t[0].m_bots), (1_000.0, BOT_WALLET_CAP));
        let r3 = w.deltas().of(KIND)[&vec!["1".to_owned(), "3".to_owned()]].clone();
        assert_eq!(r3[1].parse::<f64>().unwrap(), over - BOT_WALLET_CAP);
    }

    #[test]
    fn the_rollover_step_burns_only_when_burn_at_rollover_says() {
        let over = BOT_WALLET_CAP * 2.0;
        let mut w = with_bots(0.0, over, 0.0);
        on_rollover(&mut w, &day(1));
        let want = if BURN_AT_ROLLOVER {
            BOT_WALLET_CAP
        } else {
            over
        };
        assert_eq!(w.character(BOT).unwrap().balance, want);
    }

    #[test]
    fn the_cursor_counts_each_journal_row_once() {
        let mut w = reg();
        let now = day(1).now;
        w.journal_mut()
            .record(now, 17, SYSTEM_OWNER, HUMAN, 0, 10.0, 10.0, "");
        assert_eq!(step(&mut w, &day(1), false), 1);
        assert_eq!(step(&mut w, &day(2), false), 0, "nothing new");
        w.journal_mut()
            .record(now, 33, SYSTEM_OWNER, HUMAN, 0, 5.0, 15.0, "");
        assert_eq!(step(&mut w, &day(3), false), 1);
        let faucets: Vec<f64> = history(&w).iter().map(|t| t.faucet).collect();
        assert_eq!(faucets, vec![10.0, 0.0, 5.0]);
        assert!(since(&w, cursor(&w)).by_ref.is_empty());
    }

    #[test]
    fn days_older_than_keep_days_are_pruned() {
        let mut w = reg();
        step(&mut w, &day(1), false);
        step(&mut w, &day(1 + KEEP_DAYS), false);
        assert_eq!(history(&w).len(), 2, "KEEP_DAYS back is kept");
        step(&mut w, &day(2 + KEEP_DAYS), false);
        let days: Vec<u32> = history(&w).iter().map(|t| t.day).collect();
        assert_eq!(days, vec![1 + KEEP_DAYS, 2 + KEEP_DAYS]);
    }

    #[test]
    fn the_line_holds_every_field_c5_names() {
        let w = with_bots(10.0, 20.0, 0.0);
        let s = since(&w, 0);
        let line = Totals::of(&w, 4, &s, 0, day(4).now).line("");
        for field in [
            "[econ] day 4 ",
            "faucet=",
            "sink=",
            "net=",
            "M_h=10 ",
            "M_bots=20 ",
            "npc_fills=",
            "trades=",
            "dbs_min=",
        ] {
            assert!(line.contains(field), "{field:?} missing from {line}");
        }
    }

    // ---- E11 ------------------------------------------------------------------

    /// A fixture type by name: `(type_id, category_id)`.
    fn ty(w: &Registry, name: &str) -> (i64, i64) {
        let t = crate::item::find_type_in(w.content(), name).expect("a fixture type");
        (t.type_id, t.category_id)
    }

    fn hangar() -> crate::item::Slot {
        crate::item::Slot::new(HUMAN, 60_000_001, crate::model::FLAG_HANGAR)
    }

    #[test]
    fn goods_are_counted_per_class_netted_per_type_and_never_for_a_move() {
        use crate::item::Edit;
        let mut w = reg();
        let (ammo, ammo_cat) = ty(&w, "EMP S");
        let (ship, ship_cat) = ty(&w, "Rifter");
        let (ammo_c, ship_c) = (
            goods_class(ammo_cat).unwrap(),
            goods_class(ship_cat).unwrap(),
        );
        assert_ne!(ammo_c, ship_c);
        let made = w
            .apply_items(&[
                Edit::Create {
                    type_id: ammo,
                    quantity: 100,
                    to: hangar(),
                },
                Edit::Create {
                    type_id: ship,
                    quantity: 1,
                    to: hangar(),
                },
            ])
            .unwrap();
        let ammo_id = made
            .iter()
            .map(|c| c.item_id())
            .find(|id| w.store().get(*id).is_some_and(|i| i.type_id == ammo))
            .unwrap();
        // A move written as consume-and-create (a reload) mints nothing.
        let g = goods_of(
            w.store(),
            w.content(),
            &[
                Edit::Consume {
                    item_id: ammo_id,
                    quantity: 10,
                },
                Edit::Create {
                    type_id: ammo,
                    quantity: 10,
                    to: hangar(),
                },
            ],
        );
        assert!(g.is_empty(), "{g:?}");
        // Fired: thirty rounds destroyed.
        w.apply_items_quiet(&[Edit::Consume {
            item_id: ammo_id,
            quantity: 30,
        }])
        .unwrap();
        let today = goods_today(&w);
        assert_eq!(today.0[ammo_c], (100, 30));
        assert_eq!(today.0[ship_c], (1, 0));
        // A refused plan counts nothing.
        let before = goods_today(&w);
        assert!(
            w.apply_items(&[Edit::Consume {
                item_id: ammo_id,
                quantity: 1_000,
            }])
            .is_err()
        );
        assert_eq!(goods_today(&w), before);
        // The rollover folds the running rows into the day and clears them.
        step(&mut w, &day(1), false);
        assert!(goods_today(&w).is_empty());
        assert_eq!(goods_of_day(&w, 1), before);
        let c = closed_of_day(&w, 1).unwrap();
        assert_eq!(c.goods, before);
        assert!(
            c.part().contains(&format!("{ammo_c}:100/30")),
            "{}",
            c.part()
        );
        assert_eq!(goods_class(CATEGORY_CELESTIAL), None, "a wreck is no good");
    }

    /// The fixture's priced type (`econ_prices::Table::fixture`).
    fn priced(w: &Registry) -> (i64, f64) {
        let (t, p) = w
            .content()
            .get::<crate::econ_prices::Table>()
            .prices
            .iter()
            .next()
            .expect("the fixture prices a type");
        (*t, p.ref_price)
    }

    fn fill(type_id: i64, price: f64, quantity: i64, buyer: i64, seller: i64) -> Fill {
        Fill {
            date: day(1).now,
            station_id: 60_000_001,
            type_id,
            quantity,
            price,
            buyer_id: buyer,
            seller_id: seller,
        }
    }

    use crate::econ_exchange::Fill;

    #[test]
    fn the_closed_figures_carry_the_index_and_the_bot_human_flow() {
        let mut w = with_bots(0.0, 0.0, 0.0);
        let (t, p_ref) = priced(&w);
        w.seed_market_fills(vec![
            fill(t, p_ref * 2.0, 3, BOT, HUMAN),
            fill(t, p_ref, 1, HUMAN, BOT),
            fill(t, p_ref, 2, BOT, POOR_BOT),
            // An NPC side is not in the index or the flow.
            fill(
                t,
                p_ref * 100.0,
                5,
                BOT,
                crate::econ_exchange::NPC_ORDER_BASE,
            ),
        ]);
        let c = Closed::of(&w, 0, Goods::default());
        assert_eq!(c.bot_to_human, p_ref * 6.0);
        assert_eq!(c.human_to_bot, p_ref);
        assert_eq!(c.indexed, 1);
        let want = (p_ref * 6.0 + p_ref + p_ref * 2.0) / (p_ref * 6.0);
        assert!((c.index.unwrap() - want).abs() < 1e-12, "{c:?}");
        let line = c.part();
        for field in ["index=", "bot>human=", "human>bot=", "goods=-"] {
            assert!(line.contains(field), "{field:?} missing from {line}");
        }
        assert_eq!(Closed::of(&reg(), 0, Goods::default()).index, None);
    }

    #[test]
    fn the_demand_budget_limits_only_bots_buying_from_humans() {
        let mut w = with_bots(0.0, 0.0, 0.0);
        let (t, _) = priced(&w);
        let price = BOT_DEMAND_PER_HUMAN / 10.0;
        assert_eq!(demand_take(&w, BOT, HUMAN, price, 50, 0.0), 10);
        assert_eq!(
            demand_take(&w, BOT, HUMAN, price, 50, price * 4.0),
            6,
            "what this order already planned counts"
        );
        assert_eq!(
            demand_take(&w, BOT, POOR_BOT, price, 50, 0.0),
            50,
            "bot-bot"
        );
        assert_eq!(demand_take(&w, HUMAN, BOT, price, 50, 0.0), 50, "a human");
        let npc = crate::econ_exchange::NPC_ORDER_BASE;
        assert_eq!(demand_take(&w, BOT, npc, price, 50, 0.0), 50, "an NPC");
        // Today's fills from any bot spend the human's budget.
        w.seed_market_fills(vec![fill(t, price, 7, POOR_BOT, HUMAN)]);
        assert_eq!(demand_take(&w, BOT, HUMAN, price, 50, 0.0), 3);
        assert_eq!(demand_spent(&w), vec![(HUMAN, price * 7.0)]);
        w.seed_market_fills(vec![fill(t, price, 3, BOT, HUMAN)]);
        assert_eq!(demand_take(&w, BOT, HUMAN, price, 1, 0.0), 0, "refused");
    }

    #[test]
    fn the_burn_switch_is_off_by_default_and_persists() {
        let over = BOT_WALLET_CAP * 2.0;
        let mut w = with_bots(0.0, over, 0.0);
        assert_eq!(burn_enabled(&w), BURN_AT_ROLLOVER);
        set_burn(&mut w, true);
        assert!(burn_enabled(&w));
        on_rollover(&mut w, &day(1));
        assert_eq!(w.character(BOT).unwrap().balance, BOT_WALLET_CAP);
        set_burn(&mut w, false);
        w.character_mut(BOT).unwrap().balance = over;
        on_rollover(&mut w, &day(2));
        assert_eq!(w.character(BOT).unwrap().balance, over);
        assert!(history(&w).iter().all(|t| t.day > 0), "the row is no day");
    }

    /// The C5 §5.4 reward sanity line agrees with what a kill mission
    /// actually pays at every level, and says where each base came from.
    #[test]
    fn the_reward_sanity_line_agrees_with_what_a_kill_mission_pays() {
        use crate::mission_pool::{Agent, Mission};
        let w = reg();
        let levels = reward_levels(&w);
        assert_eq!(levels.len(), crate::mission_rewards::LEVELS.count());
        for l in &levels {
            let m = Mission {
                id: 999_900 + l.level,
                template: "BasicKill".into(),
                family: "kill".into(),
                level: Some(l.level),
                ..Mission::default()
            };
            let pays = crate::mission_rewards::reward(&w, &Agent::default(), &m).isk(false);
            assert_eq!(l.reward, pays, "level {}", l.level);
            assert_eq!(l.base, crate::mission_rewards::isk_base(&w, l.level));
        }
        let line = reward_line(&w, &Summary::default());
        assert!(line.starts_with("[econ] rewards L1 base="), "{line}");
        for l in &levels {
            assert!(line.contains(&format!("reward={}", l.reward)), "{line}");
        }
        assert!(line.contains("paid: missions 0 over 0 rows"), "{line}");
    }
}
