//! Player wars: the registry, the war HQ stand-in, the war permit, declaring,
//! the lifecycle clock, and the one question crimewatch and CONCORD ask --
//! "are these two at war?" (C8-a/b; R-WAR; lane note `docs/lanes/c8-a.md`).
//!
//! **The deltas are the only copy.** A war is a few rows (`KINDS`), changed
//! one war at a time:
//!
//! ```text
//! war       <warID>          -> declaredBy against timeDeclared timeStarted timeFinished
//!                               retracted retractedBy billID mutual createdFrom
//!                               openForAllies reasonEnded warHQ reasonStarted notified
//! warally   <warID allyID>   -> timeStarted timeFinished told
//! warhq     <owner>          -> itemID stationID systemID
//! warpermit <corp>           -> 0|1            (a GM override of the permit rule)
//! warkill   <warID killID>   -> (nothing)      (a kill that was a war kill)
//! warnext   <name>           -> n              (the id counters)
//! ```
//!
//! `-` is an absent id. Times are blue FILETIMEs and are **real** times: the
//! game clock (`--time-scale N`, `crate::game_clock`) compresses a *span*
//! (the 24 h spool-up, the 24 h cooldown, the 4 h ally spool-up, the weeks)
//! at the moment a deadline is made from it, so the client's own wall-clock
//! comparisons (`evewar.util`) agree with the server's.
//!
//! Negotiations, mutual invites, peace treaties and bills are `war_deal.rs`;
//! the notifications are `war_notice.rs`; the wire is `service/war.rs`.
//!
//! # What the client defines [V]
//!
//! `evewar.const` carries the numbers (`COST_OF_WAR` 500 M ISK,
//! `BILLING_PERIOD` a week, `WAR_SPOOLUP` and `WAR_COOLDOWN` 24 h,
//! `ALLY_SPOOLUP` 4 h, `ALLY_COMMITMENT_PERIOD` and `FORCED_PEACE_PERIOD`
//! two weeks) and the reason codes. `evewar.util` carries the predicates,
//! ported verbatim below: [`in_hostile_state`], [`is_at_war`],
//! [`in_cooldown`], [`is_finished`], [`in_spoolup`], [`ally_active`].
//! **Cooldown still counts as war** (`IsWarInHostileState`): a war that has
//! a `timeFinished` in the future stays hostile until it passes.
//!
//! # The war HQ stand-in and the permit [A]
//!
//! The client will not send a declaration without a war HQ item
//! ([`Hq`]). It must pass `idCheckers.IsPlayerItem`, or
//! `structureDirectory.GetStructureInfo` answers `None` and the wizard
//! crashes. There is no Upwell and this build's corporations have no HQ
//! station, so an HQ is a **virtual type-27 item** ([`TYPE_WAR_HQ`]) per
//! owner, in a band no allocator reaches ([`HQ_ITEM_BASE`]), anchored to the
//! station the declaring director is docked in when they open the wizard
//! ([`valid_hqs`]), and valid only while that system is high-sec
//! ([`high_sec`]). `allowWar` ([`permit`]) is 1 for every player corp whose
//! anchor is not low or null. Both are `[A]`s with falsifiers in the lane
//! note.

use crate::alliance::{Alliances, MAX_PLAYER_ALLIANCE_ID, MIN_PLAYER_ALLIANCE_ID};
use crate::damage::BlueTime;
use crate::delta::Deltas;
use crate::seed::{DAY, HOUR};
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

// ---------------------------------------------------------------------------
// The client's numbers [V evewar.const]
// ---------------------------------------------------------------------------

/// The delta kinds this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "war";
pub const KIND_ALLY: &str = "warally";
pub const KIND_HQ: &str = "warhq";
pub const KIND_PERMIT: &str = "warpermit";
pub const KIND_KILL: &str = "warkill";
pub const KIND_NEXT: &str = "warnext";
pub const KINDS: [&str; 6] = [KIND, KIND_ALLY, KIND_HQ, KIND_PERMIT, KIND_KILL, KIND_NEXT];

/// `WEEK` of `carbon.common.lib.const`.
pub const WEEK: BlueTime = 7 * DAY;

/// `evewar.const.COST_OF_WAR` **[V]**.
pub const COST_OF_WAR: f64 = 500_000_000.0;
/// `BILLING_PERIOD = WEEK` **[V]**.
pub const BILLING_PERIOD: BlueTime = WEEK;
/// `WAR_SPOOLUP = 24 h` **[V]**: a war can be fought this long after it is
/// declared, a mutual war this long after the invite is accepted
/// (`WarDeclareSummaryMutualWarStartTime` **[V-data]**).
pub const WAR_SPOOLUP: BlueTime = 24 * HOUR;
/// `WAR_COOLDOWN = 24 h` **[V]**: a retracted, surrendered or invalidated
/// war stays hostile this long.
pub const WAR_COOLDOWN: BlueTime = 24 * HOUR;
/// `WAR_MUTUAL_WAR_EXPIRY_DAYS = 7` **[V]**.
pub const MUTUAL_INVITE_EXPIRY: BlueTime = 7 * DAY;
/// `ALLY_SPOOLUP = 4 h` **[V]**.
pub const ALLY_SPOOLUP: BlueTime = 4 * HOUR;
/// `ALLY_COMMITMENT_PERIOD = 2 weeks` **[V]**.
pub const ALLY_COMMITMENT_PERIOD: BlueTime = 2 * WEEK;
/// `FORCED_PEACE_PERIOD = 2 weeks` **[V]**.
pub const FORCED_PEACE_PERIOD: BlueTime = 2 * WEEK;
/// `WAR_NEGOTIATION_EXPIRY = 1 day` **[V]**.
pub const NEGOTIATION_EXPIRY: BlueTime = DAY;

/// `evewar.const.ENDED_*` **[V]**.
pub mod ended {
    pub const UNLICENSED_AGGRESSOR: i64 = 1;
    pub const UNLICENSED_DEFENDER: i64 = 2;
    pub const WARHQ_GONE: i64 = 3;
    pub const WAR_ADOPTED: i64 = 4;
    pub const CORP_DELETED: i64 = 5;
    pub const RETRACTED: i64 = 6;
    pub const SURRENDER: i64 = 7;
    pub const CONCORD: i64 = 8;
    pub const HQ_OWNER_LEFT: i64 = 9;
    pub const UNPAID_BILL: i64 = 10;
    pub const MUTUAL_END: i64 = 11;
    pub const LEFT_ALLIANCE: i64 = 12;
    pub const ALLIANCE_DELETED: i64 = 13;
    pub const GM_ACTION: i64 = 14;
    pub const WAR_HQ_SYSTEM_SECURITY_DROP: i64 = 15;
}

/// `evewar.const.STARTED_*` **[V]**.
pub mod started {
    pub const PLAYER_DECLARED: i64 = 1;
    pub const ADOPTION: i64 = 2;
    pub const INHERITED: i64 = 3;
    pub const GM_ACTION: i64 = 4;
}

/// `evewar.const.PEACE_REASON_*` **[V]**.
pub mod peace {
    pub const UNDEFINED: i64 = 0;
    pub const HQ_REMOVED: i64 = 1;
    pub const HQ_OWNER_LEFT_ALLIANCE: i64 = 2;
    pub const CORP_LEFT_ALLIANCE: i64 = 3;
    pub const WAR_SURRENDER: i64 = 4;
    pub const UNPAID_BILL: i64 = 5;
}

/// `appConst.refWarFee = 49` **[V]**: the declaration fee and the weekly
/// bill, and (**[A]**, so a ref has one economic class) CONCORD's cut of an
/// ally contract.
pub const REF_WAR_FEE: i64 = 49;
/// `appConst.refWarSurrenderFee = 113` **[V]**: the offerer's ISK on an
/// accepted surrender.
pub const REF_WAR_SURRENDER_FEE: i64 = 113;
/// `appConst.refWarAllyContract = 114` **[V]**: the ISK an ally is paid.
pub const REF_WAR_ALLY_CONTRACT: i64 = 114;
/// CONCORD, the creditor of a war fee and a bill (`1000125`, **[V2]** an
/// owner the client names itself).
pub const CONCORD_OWNER: i64 = 1_000_125;

/// `inventorycommon/const.py` `typeOffice = 27` **[V-data]** (group 16,
/// category 3): the war HQ stand-in's type, because it has an icon and an
/// ordinary info sheet. **[A]**; falsified by the info window, the icon or
/// the war report misbehaving on it (human check H-W2), in which case an
/// Upwell type id replaces this one constant.
pub const TYPE_WAR_HQ: i64 = 27;

/// **[A]** the first war HQ item id. Player items count up from
/// `item::MIN_PLAYER_ITEM`, NPCs from 7e9, sites 8.1e9, instances 8.7e9, a
/// sov stand-in hub 8.9e9, escrow 9e9: this band sits between instances and
/// hubs and is far below `minFakeItem`. Falsified by a client path that
/// mishandles an item id above 2^31 (then `item::MIN_PLAYER_ITEM`'s band
/// with a counter row replaces it).
pub const HQ_ITEM_BASE: i64 = 8_800_000_000;

/// **[A]** the most aggressive wars an owner may have running. Falsified by
/// the owner wanting more or fewer (`CrpTooManyWarDeclarations`).
pub const MAX_WARS: usize = 5;

/// **[A]** how long a finished war stays on the books: 90 game days after
/// its cooldown ends. Falsified by a war report wanted older than this.
pub const KEEP_FINISHED: BlueTime = 90 * DAY;

/// A war's `notified` bits: the "you can fight now" notice went out, and the
/// cooldown's end was told to the windows.
pub const TOLD_STARTED: i64 = 1;
pub const TOLD_FINISHED: i64 = 2;

/// How a push leaves: from a call (alongside its answer) or from the world
/// tick (on the clock, to nobody's call).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Call,
    Clock,
}

/// The game clock's compression of a span at `reg`'s scale.
pub fn span(reg: &Registry, game: BlueTime) -> BlueTime {
    crate::game_clock::span(reg, game)
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Why something was refused, as the client's own message key
/// (`findings/user-errors.tsv`) or one of the three shapes that carry
/// arguments.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// A bare `UserError` key.
    Key(&'static str),
    /// `CrpAccessDenied {reason}`.
    Access(&'static str),
    /// `NotEnoughMoney {amount, balance}`.
    Money { amount: f64, balance: f64 },
    /// A key with named arguments, ids and amounts.
    With(&'static str, Vec<(&'static str, PyValue)>),
}

impl From<&'static str> for Refusal {
    fn from(k: &'static str) -> Refusal {
        Refusal::Key(k)
    }
}

// ---------------------------------------------------------------------------
// The war
// ---------------------------------------------------------------------------

/// An ally's row of a war: `KeyVal(allyID, timeStarted, timeFinished)` on
/// the wire **[V `IsAtWar`, `allyWnd`]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ally {
    pub ally_id: i64,
    pub time_started: BlueTime,
    pub time_finished: BlueTime,
    /// The join notices went out (the tick sends them at `time_started`).
    pub told: bool,
}

/// One war: the client's 17 fields **[V `evewar.util.War`]**, and the
/// server's `notified` bits.
#[derive(Debug, Clone, PartialEq)]
pub struct War {
    pub war_id: i64,
    pub declared_by: i64,
    pub against: i64,
    pub time_declared: BlueTime,
    /// `timeDeclared + WAR_SPOOLUP`, **for mutual wars too**.
    pub time_started: BlueTime,
    /// `None` while running. Once set, the war is in cooldown until it
    /// passes ([`in_cooldown`]), then finished ([`is_finished`]).
    pub time_finished: Option<BlueTime>,
    pub retracted: Option<BlueTime>,
    pub retracted_by: Option<i64>,
    pub bill_id: Option<i64>,
    pub mutual: bool,
    pub created_from: Option<i64>,
    pub open_for_allies: bool,
    pub reason_ended: Option<i64>,
    pub war_hq: Option<i64>,
    pub reason_started: i64,
    pub notified: i64,
    pub allies: BTreeMap<i64, Ally>,
}

impl War {
    /// A fresh aggressive war declared at `now`.
    pub fn new(
        war_id: i64,
        declared_by: i64,
        against: i64,
        now: BlueTime,
        spoolup: BlueTime,
        war_hq: Option<i64>,
    ) -> War {
        War {
            war_id,
            declared_by,
            against,
            time_declared: now,
            time_started: now.saturating_add(spoolup),
            time_finished: None,
            retracted: None,
            retracted_by: None,
            bill_id: None,
            mutual: false,
            created_from: None,
            open_for_allies: false,
            reason_ended: None,
            war_hq,
            reason_started: started::PLAYER_DECLARED,
            notified: 0,
            allies: BTreeMap::new(),
        }
    }

    /// Whether `owner` is a principal or an ally (finished allies too).
    pub fn involves(&self, owner: i64) -> bool {
        self.declared_by == owner || self.against == owner || self.allies.contains_key(&owner)
    }

    /// Every owner id in the war: the principals, then the allies.
    pub fn owners(&self) -> Vec<i64> {
        let mut v = vec![self.declared_by, self.against];
        v.extend(self.allies.keys().copied());
        v
    }

    /// `IsWarActive`: running, or in cooldown.
    pub fn active(&self, now: BlueTime) -> bool {
        self.time_finished.is_none_or(|f| now < f)
    }

    /// `canBeRetracted` on the wire **[I]**: a mutual war that has not
    /// finished.
    pub fn can_be_retracted(&self, now: BlueTime) -> bool {
        self.mutual && !is_finished(self, now)
    }

    fn to_row(&self) -> Vec<String> {
        vec![
            self.declared_by.to_string(),
            self.against.to_string(),
            self.time_declared.to_string(),
            self.time_started.to_string(),
            id_field(self.time_finished),
            id_field(self.retracted),
            id_field(self.retracted_by),
            id_field(self.bill_id),
            u8::from(self.mutual).to_string(),
            id_field(self.created_from),
            u8::from(self.open_for_allies).to_string(),
            id_field(self.reason_ended),
            id_field(self.war_hq),
            self.reason_started.to_string(),
            self.notified.to_string(),
        ]
    }

    fn from_row(war_id: i64, v: &[String]) -> Result<War, String> {
        Ok(War {
            war_id,
            declared_by: num(v, 0)?,
            against: num(v, 1)?,
            time_declared: num(v, 2)?,
            time_started: num(v, 3)?,
            time_finished: opt(v, 4)?,
            retracted: opt(v, 5)?,
            retracted_by: opt(v, 6)?,
            bill_id: opt(v, 7)?,
            mutual: num(v, 8)? != 0,
            created_from: opt(v, 9)?,
            open_for_allies: num(v, 10)? != 0,
            reason_ended: opt(v, 11)?,
            war_hq: opt(v, 12)?,
            reason_started: num(v, 13)?,
            // Older rows have no `notified`.
            notified: opt(v, 14)?.unwrap_or(0),
            allies: BTreeMap::new(),
        })
    }
}

/// `-` for an absent id.
pub(crate) fn id_field(v: Option<i64>) -> String {
    v.map_or_else(|| "-".to_owned(), |n| n.to_string())
}

pub(crate) fn num(v: &[String], i: usize) -> Result<i64, String> {
    v.get(i)
        .ok_or_else(|| format!("field {i} is missing"))?
        .parse::<i64>()
        .map_err(|e| format!("field {i}: {e}"))
}

pub(crate) fn opt(v: &[String], i: usize) -> Result<Option<i64>, String> {
    match v.get(i).map(String::as_str) {
        None | Some("-" | "") => Ok(None),
        Some(s) => s
            .parse::<i64>()
            .map(Some)
            .map_err(|e| format!("field {i}: {e}")),
    }
}

/// **The war registry**: every war, read from the deltas. A war is
/// `put`/`remove`d one at a time by the functions below.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Wars {
    wars: BTreeMap<i64, War>,
}

impl Wars {
    /// Read every war and its allies. A row that does not parse is skipped
    /// with a log line: it is one war lost, not the world.
    pub fn load(d: &Deltas) -> Wars {
        let mut wars = BTreeMap::new();
        for (key, v) in d.of(KIND) {
            let parsed = num(key, 0).and_then(|id| War::from_row(id, v));
            match parsed {
                Ok(w) => {
                    wars.insert(w.war_id, w);
                }
                Err(e) => crate::errln!("[war] skipping war row {key:?}: {e}"),
            }
        }
        for (key, v) in d.of(KIND_ALLY) {
            let parsed = (|| -> Result<(i64, Ally), String> {
                let war = num(key, 0)?;
                let ally_id = num(key, 1)?;
                Ok((
                    war,
                    Ally {
                        ally_id,
                        time_started: num(v, 0)?,
                        time_finished: num(v, 1)?,
                        told: opt(v, 2)?.unwrap_or(0) != 0,
                    },
                ))
            })();
            match parsed {
                Ok((war, a)) => {
                    if let Some(w) = wars.get_mut(&war) {
                        w.allies.insert(a.ally_id, a);
                    }
                }
                Err(e) => crate::errln!("[war] skipping ally row {key:?}: {e}"),
            }
        }
        Wars { wars }
    }

    pub fn get(&self, war_id: i64) -> Option<&War> {
        self.wars.get(&war_id)
    }

    /// Every war, in id order.
    pub fn all(&self) -> impl Iterator<Item = &War> + '_ {
        self.wars.values()
    }

    /// Every war `owner` is in: as aggressor, defender or ally.
    pub fn of_owner(&self, owner: i64) -> impl Iterator<Item = &War> + '_ {
        self.wars.values().filter(move |w| w.involves(owner))
    }

    pub fn is_empty(&self) -> bool {
        self.wars.is_empty()
    }

    /// The war currently running between two principals, either way round.
    pub fn between(&self, a: i64, b: i64, now: BlueTime) -> Option<&War> {
        self.wars.values().find(|w| {
            w.active(now)
                && ((w.declared_by == a && w.against == b)
                    || (w.declared_by == b && w.against == a))
        })
    }

    /// Insert or replace (in memory only; [`put`] persists).
    pub fn insert(&mut self, w: War) {
        self.wars.insert(w.war_id, w);
    }
}

/// One war and its allies, from the deltas.
pub fn load_war(d: &Deltas, war_id: i64) -> Option<War> {
    let key = vec![war_id.to_string()];
    let mut w = War::from_row(war_id, d.of(KIND).get(&key)?).ok()?;
    for (k, v) in d
        .of(KIND_ALLY)
        .range(vec![war_id.to_string()]..)
        .take_while(|(k, _)| k.first() == Some(&war_id.to_string()))
    {
        if let (Ok(ally_id), Ok(started), Ok(finished)) = (num(k, 1), num(v, 0), num(v, 1)) {
            w.allies.insert(
                ally_id,
                Ally {
                    ally_id,
                    time_started: started,
                    time_finished: finished,
                    told: opt(v, 2).ok().flatten().unwrap_or(0) != 0,
                },
            );
        }
    }
    Some(w)
}

/// Persist one war and its allies; ally rows the war no longer holds are
/// removed.
pub fn put(d: &mut Deltas, w: &War) {
    d.put(KIND, vec![w.war_id.to_string()], w.to_row());
    let prefix = w.war_id.to_string();
    let stale: Vec<Vec<String>> = d
        .of(KIND_ALLY)
        .range(vec![prefix.clone()]..)
        .take_while(|(k, _)| k.first() == Some(&prefix))
        .filter(|(k, _)| num(k, 1).is_ok_and(|a| !w.allies.contains_key(&a)))
        .map(|(k, _)| k.clone())
        .collect();
    for k in stale {
        d.del(KIND_ALLY, &k);
    }
    for a in w.allies.values() {
        d.put(
            KIND_ALLY,
            vec![w.war_id.to_string(), a.ally_id.to_string()],
            vec![
                a.time_started.to_string(),
                a.time_finished.to_string(),
                u8::from(a.told).to_string(),
            ],
        );
    }
}

/// Remove a war, its allies and its kills.
pub fn remove(d: &mut Deltas, war_id: i64) {
    let prefix = war_id.to_string();
    for kind in [KIND_ALLY, KIND_KILL] {
        let keys: Vec<Vec<String>> = d
            .of(kind)
            .range(vec![prefix.clone()]..)
            .take_while(|(k, _)| k.first() == Some(&prefix))
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            d.del(kind, &k);
        }
    }
    d.del(KIND, &[prefix]);
}

/// Take the next id of counter `name` (`war`, `neg`, `bill`, `hq`): the
/// first is 1, and ids are never reused.
pub fn take_id(d: &mut Deltas, name: &str) -> i64 {
    let key = vec![name.to_owned()];
    let next = d
        .of(KIND_NEXT)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    d.put(KIND_NEXT, key, vec![(next + 1).to_string()]);
    next
}

// ---------------------------------------------------------------------------
// The client's predicates [V evewar.util], ported
// ---------------------------------------------------------------------------

/// `DoesWarHaveFutureFinishTimeSet`.
pub fn future_finish(w: &War, now: BlueTime) -> bool {
    w.time_finished.is_some_and(|f| now < f)
}

/// `IsWarInHostileState`: running or cooling down, and past its spool-up.
pub fn in_hostile_state(w: &War, now: BlueTime) -> bool {
    (w.time_finished.is_none() || future_finish(w, now)) && now > w.time_started
}

/// `IsWarInCooldown`.
pub fn in_cooldown(w: &War, now: BlueTime) -> bool {
    w.time_finished
        .is_some_and(|f| w.time_started < now && now < f)
}

/// `IsWarFinished`.
pub fn is_finished(w: &War, now: BlueTime) -> bool {
    w.time_finished.is_some_and(|f| f < now)
}

/// `IsWarInSpoolup`.
pub fn in_spoolup(w: &War, now: BlueTime) -> bool {
    now < w.time_started
}

/// `IsAllyActive`.
pub fn ally_active(a: &Ally, now: BlueTime) -> bool {
    a.time_started < now && now < a.time_finished
}

/// `IsAllyPendingOrActive`.
pub fn ally_pending_or_active(a: &Ally, now: BlueTime) -> bool {
    now < a.time_finished
}

/// `IsAtWar(wars, entities, now)` **[V]**, verbatim: the first war whose
/// aggressor is among `entities`, that is in a hostile state, and whose
/// defender -- or the first of its allies that is among `entities` -- is
/// among them too. Like the client's, the ally loop looks at the first ally
/// found in `entities` and stops there, active or not.
pub fn is_at_war<'a>(
    wars: impl IntoIterator<Item = &'a War>,
    entities: &[i64],
    now: BlueTime,
) -> Option<i64> {
    for w in wars {
        if !entities.contains(&w.declared_by) || !in_hostile_state(w, now) {
            continue;
        }
        if entities.contains(&w.against) {
            return Some(w.war_id);
        }
        for (ally_id, row) in &w.allies {
            if entities.contains(ally_id) {
                if row.time_started < now && now < row.time_finished {
                    return Some(w.war_id);
                }
                break;
            }
        }
    }
    None
}

/// The first war in which some owner of `a` fights some owner of `b`, either
/// way round (aggressor against defender, or aggressor against an **active**
/// ally of the defender). `a` and `b` are *sets of owner ids* (a character's
/// corp and its alliance): the pair semantics of the client's two-element
/// `entities` extended to both sides. The defender's allies never fight each
/// other, nor their principal.
pub fn war_between<'a>(
    wars: impl IntoIterator<Item = &'a War>,
    a: &[i64],
    b: &[i64],
    now: BlueTime,
) -> Option<i64> {
    for w in wars {
        if !in_hostile_state(w, now) {
            continue;
        }
        for (aggr, other) in [(a, b), (b, a)] {
            if !aggr.contains(&w.declared_by) {
                continue;
            }
            if other.contains(&w.against) {
                return Some(w.war_id);
            }
            if w.allies
                .iter()
                .any(|(id, row)| other.contains(id) && ally_active(row, now))
            {
                return Some(w.war_id);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Owners
// ---------------------------------------------------------------------------

/// `idCheckers.IsAlliance`: `minPlayerAlliance <= id <= maxPlayerAlliance`.
pub fn is_alliance_id(id: i64) -> bool {
    (MIN_PLAYER_ALLIANCE_ID..=MAX_PLAYER_ALLIANCE_ID).contains(&id)
}

/// `idCheckers.IsNPCCorporation`: 1,000,000 to 1,999,999 **[V]**.
pub fn is_npc_corporation(id: i64) -> bool {
    (1_000_000..=1_999_999).contains(&id)
}

/// A player corporation that is open, or a player alliance that is open.
pub fn owner_exists(reg: &Registry, id: i64) -> bool {
    if is_alliance_id(id) {
        Alliances::load(reg.deltas())
            .get(id)
            .is_some_and(|a| !a.deleted)
    } else {
        reg.corporations().get(id).is_some_and(|c| !c.deleted)
    }
}

/// The corporation that speaks for an owner: a corp itself, an alliance's
/// executor.
pub fn owner_corp(reg: &Registry, owner: i64) -> Option<i64> {
    if is_alliance_id(owner) {
        Alliances::load(reg.deltas())
            .get(owner)
            .and_then(|a| a.executor_corp_id)
    } else {
        reg.corporations().get(owner).map(|c| c.corporation_id)
    }
}

/// The owner a corporation fights as: its alliance, else itself
/// (`evewar.util.GetWarEntity`).
pub fn entity_of_corp(reg: &Registry, corp: i64) -> i64 {
    crate::alliance::of_corp(reg.deltas(), corp).unwrap_or(corp)
}

/// The owner a character fights as.
pub fn entity_of_character(reg: &Registry, character: i64) -> Option<i64> {
    let c = reg.character(character)?;
    Some(c.alliance_id.unwrap_or(c.corporation_id))
}

/// Every owner id a corp stands for: itself and its alliance.
fn owner_ids_of_corp(reg: &Registry, corp: i64) -> Vec<i64> {
    let mut v = vec![corp];
    v.extend(crate::alliance::of_corp(reg.deltas(), corp));
    v
}

/// The owner ids `id` stands for when it is a corp (itself and its
/// alliance) or an alliance (itself).
fn owner_ids(reg: &Registry, id: i64) -> Vec<i64> {
    if is_alliance_id(id) {
        vec![id]
    } else {
        owner_ids_of_corp(reg, id)
    }
}

/// Every character of an owner: a corp's members, or all of an alliance's
/// member corps'.
pub fn owner_characters(reg: &Registry, owner: i64) -> Vec<i64> {
    if is_alliance_id(owner) {
        let st = Alliances::load(reg.deltas());
        st.members_of(owner)
            .into_iter()
            .flat_map(|(corp, _)| reg.corporations().members_of(corp))
            .collect()
    } else {
        reg.corporations().members_of(owner)
    }
}

/// A corp's CEO and every member with the Director role.
pub fn corp_directors(reg: &Registry, corp: i64) -> Vec<i64> {
    let ceo = reg.corporations().get(corp).map(|c| c.ceo_id);
    reg.corporations()
        .members_of(corp)
        .into_iter()
        .filter(|id| {
            Some(*id) == ceo
                || reg
                    .corporations()
                    .membership(*id)
                    .is_some_and(|m| m.roles & crate::corp_roles::CORP_ROLE_DIRECTOR != 0)
        })
        .collect()
}

/// The directors who speak for an owner (an alliance: its executor's).
pub fn owner_directors(reg: &Registry, owner: i64) -> Vec<i64> {
    owner_corp(reg, owner).map_or_else(Vec::new, |c| corp_directors(reg, c))
}

/// An owner's short name for the war HQ's label: the corp's ticker or the
/// alliance's short name.
fn owner_tag(reg: &Registry, owner: i64) -> String {
    if is_alliance_id(owner) {
        crate::alliance::short_name_of(reg.deltas(), owner).unwrap_or_default()
    } else {
        reg.corporations()
            .get(owner)
            .map(|c| c.ticker.clone())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// The treasury: the one place a war fee leaves a wallet
// ---------------------------------------------------------------------------

/// The pilot who speaks for an owner's money: the CEO of the corp (an
/// alliance: of its executor). Money itself is the corp's Master Wallet
/// (CORP-W, [`balance_of`], [`pay`], [`receive`]); this names the executor in
/// the surrender refusal.
pub fn payer_of(reg: &Registry, owner: i64) -> Option<i64> {
    let corp = owner_corp(reg, owner)?;
    reg.corporations().get(corp).map(|c| c.ceo_id)
}

/// What an owner can spend: its (an alliance's: its executor's) corp
/// Master Wallet.
pub fn balance_of(reg: &Registry, owner: i64) -> f64 {
    owner_corp(reg, owner).map_or(0.0, |c| {
        crate::corp_wallet::to_isk(crate::corp_wallet::balance(
            reg.deltas(),
            c,
            crate::corp_wallet::MASTER_KEY,
        ))
    })
}

/// `NotEnoughMoney` as the client shows it for a corp's money:
/// `NotEnoughMoneyCorp {amount, balance, division, owner}` naming the
/// owner's Master Wallet. Any other refusal passes through.
pub fn corp_money(reg: &Registry, owner: i64, r: Refusal) -> Refusal {
    let (Refusal::Money { amount, balance }, Some(corp)) = (&r, owner_corp(reg, owner)) else {
        return r;
    };
    use crate::service::corp_wallet as sw;
    Refusal::With(
        "NotEnoughMoneyCorp",
        vec![
            ("amount", PyValue::Real(*amount)),
            ("balance", PyValue::Real(*balance)),
            (
                "division",
                PyValue::Unicode(sw::division_label(
                    reg,
                    corp,
                    crate::corp_wallet::MASTER_KEY,
                )),
            ),
            ("owner", PyValue::Unicode(sw::corp_name(reg, corp))),
        ],
    )
}

/// Take `amount` from an owner's Master Wallet as journal ref `ref_type`,
/// with `counterparty` on the row (CONCORD: a sink in the economy ledger).
/// `false` (and nothing taken) when it cannot pay.
pub fn pay(
    reg: &mut Registry,
    owner: i64,
    amount: f64,
    ref_type: i64,
    counterparty: i64,
    reference: i64,
) -> bool {
    if !amount.is_finite() || amount < 0.0 || balance_of(reg, owner) < amount {
        return false;
    }
    let Some(corp) = owner_corp(reg, owner) else {
        return false;
    };
    crate::service::corp_wallet::transfer(
        reg,
        corp,
        crate::corp_wallet::MASTER_KEY,
        -amount,
        ref_type,
        (corp, counterparty),
        reference,
        counterparty == CONCORD_OWNER,
    )
    .is_some()
}

/// Pay `amount` into an owner's Master Wallet.
pub fn receive(
    reg: &mut Registry,
    owner: i64,
    amount: f64,
    ref_type: i64,
    counterparty: i64,
    reference: i64,
) -> bool {
    if !amount.is_finite() || amount < 0.0 {
        return false;
    }
    let Some(corp) = owner_corp(reg, owner) else {
        return false;
    };
    crate::service::corp_wallet::transfer(
        reg,
        corp,
        crate::corp_wallet::MASTER_KEY,
        amount,
        ref_type,
        (counterparty, corp),
        reference,
        false,
    )
    .is_some()
}

// ---------------------------------------------------------------------------
// Security
// ---------------------------------------------------------------------------

/// Whether `system` is high-sec (`HighSec` or `SafeSec`, the client's
/// classes, `SecurityClassFromLevel`) **[V]**. **[A]** that a system the
/// universe table lacks counts as high-sec, as `crimewatch::flags_apply` does
/// for a server with no bundle (falsified by a war HQ anchored in null-sec
/// with no bundle loaded; load one).
pub fn high_sec(reg: &Registry, system: i64) -> bool {
    reg.content()
        .get::<crate::celestial::Universe>()
        .security_class(system)
        .is_none_or(|c| c >= crate::celestial::SecurityClass::High)
}

// ---------------------------------------------------------------------------
// The war HQ stand-in
// ---------------------------------------------------------------------------

/// A war HQ: the virtual item a declaration names **[A]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hq {
    /// The owner it belongs to: a corp or an alliance.
    pub owner: i64,
    pub item_id: i64,
    pub station_id: i64,
    pub system_id: i64,
}

/// An owner's war HQ row, if it has one.
pub fn hq_of(d: &Deltas, owner: i64) -> Option<Hq> {
    let v = d.of(KIND_HQ).get(&vec![owner.to_string()])?;
    Some(Hq {
        owner,
        item_id: num(v, 0).ok()?,
        station_id: num(v, 1).ok()?,
        system_id: num(v, 2).ok()?,
    })
}

/// The HQ with this item id (`structureDirectory.GetStructureInfo`).
pub fn hq_by_item(d: &Deltas, item_id: i64) -> Option<Hq> {
    d.of(KIND_HQ).iter().find_map(|(k, v)| {
        if num(v, 0).ok()? != item_id {
            return None;
        }
        Some(Hq {
            owner: num(k, 0).ok()?,
            item_id,
            station_id: num(v, 1).ok()?,
            system_id: num(v, 2).ok()?,
        })
    })
}

fn put_hq(d: &mut Deltas, hq: &Hq) {
    d.put(
        KIND_HQ,
        vec![hq.owner.to_string()],
        vec![
            hq.item_id.to_string(),
            hq.station_id.to_string(),
            hq.system_id.to_string(),
        ],
    );
}

/// The HQ's label on the wire: `<tag> War HQ` **[A]**.
pub fn hq_name(reg: &Registry, hq: &Hq) -> String {
    format!("{} War HQ", owner_tag(reg, hq.owner))
}

/// The `KeyVal` of `GetValidWarHQs` and `GetStructureInfo`: `structureID`,
/// `typeID`, `itemName`, `ownerID`, `solarSystemID` **[V reads `structureID`,
/// `typeID`, `itemName`; I the rest]**.
pub fn hq_value(reg: &Registry, hq: &Hq) -> PyValue {
    crate::pyutil::keyval(vec![
        ("structureID", PyValue::Int(hq.item_id)),
        ("typeID", PyValue::Int(TYPE_WAR_HQ)),
        ("itemName", PyValue::Unicode(hq_name(reg, hq))),
        (
            "ownerID",
            PyValue::Int(owner_corp(reg, hq.owner).unwrap_or(hq.owner)),
        ),
        ("solarSystemID", PyValue::Int(hq.system_id)),
    ])
}

/// Whether a running war of `owner` uses the HQ `item_id`.
fn hq_in_use(wars: &Wars, owner: i64, item_id: i64, now: BlueTime) -> bool {
    wars.all()
        .any(|w| w.declared_by == owner && w.war_hq == Some(item_id) && w.active(now))
}

/// **`structureDirectory.GetValidWarHQs`** for `owner`, asked by the
/// director `actor`: the owner's HQ, anchored at the station `actor` is
/// docked in if that system is high-sec and no running war uses the HQ (a
/// new item id the first time), else the HQ it already has if that is
/// high-sec. Empty when there is none (the wizard then cannot declare).
pub fn valid_hqs(reg: &mut Registry, owner: i64, actor: i64) -> Vec<Hq> {
    let now = crate::pyutil::filetime_now();
    let here = reg
        .docked_station(actor)
        .zip(reg.character(actor).and_then(|c| c.solar_system_id))
        .filter(|(_, system)| high_sec(reg, *system));
    let existing = hq_of(reg.deltas(), owner);
    let wars = Wars::load(reg.deltas());
    let movable = existing.is_none_or(|h| !hq_in_use(&wars, owner, h.item_id, now));
    if let (Some((station, system)), true) = (here, movable) {
        let hq = match existing {
            Some(h) => Hq {
                station_id: station,
                system_id: system,
                ..h
            },
            None => Hq {
                owner,
                item_id: HQ_ITEM_BASE + take_id(reg.deltas_mut(), "hq"),
                station_id: station,
                system_id: system,
            },
        };
        if existing != Some(hq) {
            put_hq(reg.deltas_mut(), &hq);
        }
        return vec![hq];
    }
    existing
        .filter(|h| high_sec(reg, h.system_id))
        .into_iter()
        .collect()
}

/// **An owner's HQ moves** to `station`/`system` (a corporation changing its
/// HQ; nothing in this build does it yet, so GM tools and later packets
/// call it). A move into low or null sec ends every war the HQ carries with
/// `ENDED_WAR_HQ_SYSTEM_SECURITY_DROP`, tells the parties (notification
/// 246), and turns the permit off.
pub fn move_hq(reg: &mut Registry, owner: i64, station: i64, system: i64, via: Via) {
    let now = crate::pyutil::filetime_now();
    let before_permit = owner_corp(reg, owner).map(|c| corp_permit(reg, c));
    let hq = match hq_of(reg.deltas(), owner) {
        Some(h) => Hq {
            station_id: station,
            system_id: system,
            ..h
        },
        None => Hq {
            owner,
            item_id: HQ_ITEM_BASE + take_id(reg.deltas_mut(), "hq"),
            station_id: station,
            system_id: system,
        },
    };
    put_hq(reg.deltas_mut(), &hq);
    if !high_sec(reg, system) {
        let wars: Vec<i64> = Wars::load(reg.deltas())
            .all()
            .filter(|w| w.war_hq == Some(hq.item_id) && w.active(now))
            .map(|w| w.war_id)
            .collect();
        for id in wars {
            finish(
                reg,
                id,
                FinishWith {
                    reason: ended::WAR_HQ_SYSTEM_SECURITY_DROP,
                    peace: Some(peace::HQ_REMOVED),
                    ..FinishWith::default()
                },
                now,
                via,
            );
        }
    }
    if let (Some(corp), Some(before)) = (owner_corp(reg, owner), before_permit) {
        permit_changed(reg, corp, before, via);
    }
}

// ---------------------------------------------------------------------------
// The war permit (`allowWar`)
// ---------------------------------------------------------------------------

/// A corp's **war permit** `[A]`: 1 for a player corp that is open, unless a
/// GM override says otherwise, or its owner's anchored HQ is in low or null
/// sec. NPC corps are always 0. Falsified by the owner wanting opt-in (then
/// the GM override is the switch, defaulting to 0).
pub fn corp_permit(reg: &Registry, corp: i64) -> bool {
    if reg.corporations().get(corp).is_none_or(|c| c.deleted) {
        return false;
    }
    if let Some(v) = reg
        .deltas()
        .of(KIND_PERMIT)
        .get(&vec![corp.to_string()])
        .and_then(|v| v.first())
    {
        return v == "1";
    }
    hq_of(reg.deltas(), corp).is_none_or(|h| high_sec(reg, h.system_id))
}

/// The permit of an owner: a corp's own, an alliance's executor's.
pub fn permit(reg: &Registry, owner: i64) -> bool {
    owner_corp(reg, owner).is_some_and(|c| corp_permit(reg, c))
}

/// `allowWar` as the corp row and the public info carry it.
pub fn permit_value(reg: &Registry, corp: i64) -> PyValue {
    PyValue::Int(i64::from(corp_permit(reg, corp)))
}

/// Set (or with `None` clear) the GM override of a corp's permit.
pub fn set_permit(reg: &mut Registry, corp: i64, to: Option<bool>, via: Via) {
    let before = corp_permit(reg, corp);
    match to {
        Some(v) => {
            reg.deltas_mut().put(
                KIND_PERMIT,
                vec![corp.to_string()],
                vec![u8::from(v).to_string()],
            );
        }
        None => {
            reg.deltas_mut().del(KIND_PERMIT, &[corp.to_string()]);
        }
    }
    permit_changed(reg, corp, before, via);
}

/// After a permit may have flipped: tell the corp (`OnAllowWarUpdated`) and
/// its directors (notifications 221 and 222).
fn permit_changed(reg: &mut Registry, corp: i64, before: bool, via: Via) {
    let now_allowed = corp_permit(reg, corp);
    if now_allowed == before {
        return;
    }
    let to = reg.corporations().members_of(corp);
    emit(
        reg,
        via,
        AllowWarUpdated {
            to,
            corp,
            allow: now_allowed,
        },
    );
    crate::war_notice::permit_changed(reg, corp, now_allowed);
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// Send a push from a call (alongside its answer) or from the clock.
pub(crate) fn emit(reg: &mut Registry, via: Via, p: impl crate::notify::Push + 'static) {
    match via {
        Via::Call => reg.push(p),
        Via::Clock => reg.schedule(Duration::ZERO, p),
    }
}

/// `OnWarChanged(war, ownerIDs, change)` **[V `war_cso.OnWarChanged`]**: the
/// war's 17-field `KeyVal`, every owner in it, and `{field: (old, new)}`.
/// The client's handler special-cases `change['warID'] == (old, None)` as a
/// delete.
#[derive(Debug, Clone)]
pub struct WarChanged {
    pub to: Vec<i64>,
    pub war: PyValue,
    pub owners: Vec<i64>,
    pub change: PyValue,
}

impl crate::notify::Push for WarChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnWarChanged",
            vec![
                self.war.clone(),
                crate::fleet_finder::py_set(&self.owners.iter().copied().collect()),
                self.change.clone(),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnAllowWarUpdated(corpID, allowWar)` **[V `warPermit`]**.
#[derive(Debug, Clone)]
pub struct AllowWarUpdated {
    pub to: Vec<i64>,
    pub corp: i64,
    pub allow: bool,
}

impl crate::notify::Push for AllowWarUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnAllowWarUpdated",
            vec![PyValue::Int(self.corp), PyValue::Int(i64::from(self.allow))],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// A push with a fixed name, arguments and audience: the mutual-war and
/// bill pushes.
#[derive(Debug, Clone)]
pub struct Notice {
    pub to: Vec<i64>,
    pub event: &'static str,
    pub args: Vec<PyValue>,
}

impl crate::notify::Push for Notice {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// The allies of a war as the client holds them: `{allyID: KeyVal(allyID,
/// timeStarted, timeFinished)}` **[V `IsAtWar`, `allyWnd`]**.
pub fn allies_value(w: &War) -> PyValue {
    PyValue::Dict(
        w.allies
            .values()
            .map(|a| {
                (
                    crate::pyutil::keyval(vec![
                        ("allyID", PyValue::Int(a.ally_id)),
                        ("timeStarted", PyValue::Int(a.time_started)),
                        ("timeFinished", PyValue::Int(a.time_finished)),
                    ]),
                    PyValue::Int(a.ally_id),
                )
            })
            .collect(),
    )
}

/// The fields of a war as the client's 17 attributes, in the wire's order
/// **[V `evewar.util.War`]**.
fn war_fields(w: &War, now: BlueTime) -> Vec<(&'static str, PyValue)> {
    let opt = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
    let allies = allies_value(w);
    vec![
        ("warID", PyValue::Int(w.war_id)),
        ("declaredByID", PyValue::Int(w.declared_by)),
        ("againstID", PyValue::Int(w.against)),
        ("timeDeclared", PyValue::Int(w.time_declared)),
        ("timeFinished", opt(w.time_finished)),
        ("retracted", opt(w.retracted)),
        ("timeStarted", PyValue::Int(w.time_started)),
        ("retractedBy", opt(w.retracted_by)),
        ("billID", opt(w.bill_id)),
        ("mutual", PyValue::Bool(w.mutual)),
        ("allies", allies),
        ("createdFromWarID", opt(w.created_from)),
        ("openForAllies", PyValue::Bool(w.open_for_allies)),
        ("canBeRetracted", PyValue::Bool(w.can_be_retracted(now))),
        ("reasonEnded", opt(w.reason_ended)),
        ("warHQ", opt(w.war_hq)),
        ("reasonStarted", PyValue::Int(w.reason_started)),
    ]
}

/// The wire form of a war: a `KeyVal` of the client's 17 fields **[V]**.
/// `War.__new__` reads every one by `getattr`, and logs an exception for any
/// missing but `allies`, so all are sent.
pub fn war_value(w: &War, now: BlueTime) -> PyValue {
    crate::pyutil::keyval(war_fields(w, now))
}

/// The `change` dict of `OnWarChanged`: every field `(old, new)` for a new
/// war (`old` `None`), only the fields that differ for an update, and always
/// `warID` as `(id, id)` so an update never reads as an add. A war removed
/// is `warID: (id, None)`.
pub fn war_change(old: Option<&War>, new: &War, now: BlueTime) -> PyValue {
    let new_v = war_fields(new, now);
    let old_v = old.map(|o| war_fields(o, now));
    let mut out = Vec::new();
    for (i, (name, nv)) in new_v.iter().enumerate() {
        let (o, n) = match old_v.as_ref().map(|o| &o[i].1) {
            None => (PyValue::None, nv.clone()),
            Some(ov) if *name == "warID" => (ov.clone(), nv.clone()),
            Some(ov) if ov == nv => continue,
            Some(ov) => (ov.clone(), nv.clone()),
        };
        out.push((*name, PyValue::Tuple(vec![o, n])));
    }
    crate::pyutil::pydict(out)
}

/// Tell every pilot of every owner in `new` that the war changed
/// (`OnWarChanged`). `old` is `None` for a new war.
pub fn tell_change(reg: &mut Registry, old: Option<&War>, new: &War, via: Via) {
    let now = crate::pyutil::filetime_now();
    let mut to = Vec::new();
    let mut owners = new.owners();
    if let Some(o) = old {
        for id in o.owners() {
            if !owners.contains(&id) {
                owners.push(id);
            }
        }
    }
    for owner in &owners {
        to.extend(owner_characters(reg, *owner));
    }
    to.sort_unstable();
    to.dedup();
    if to.is_empty() {
        return;
    }
    let push = WarChanged {
        to,
        war: war_value(new, now),
        owners: new.owners(),
        change: war_change(old, new, now),
    };
    emit(reg, via, push);
}

// ---------------------------------------------------------------------------
// Asking: are they at war?
// ---------------------------------------------------------------------------

/// **Are these two at war right now?** `a_owner` and `b_owner` are corp or
/// alliance ids; a corp stands for itself *and* its alliance, as a pilot's
/// war entity does. This is the question crimewatch and CONCORD ask
/// (lanes cw-1 and CONCORD-1) and the bots' hunting rule asks (SOC-C5).
/// Hot-path cheap: no war rows, no work.
pub fn at_war(reg: &Registry, a_owner: i64, b_owner: i64) -> bool {
    at_war_at(reg, a_owner, b_owner, crate::pyutil::filetime_now()).is_some()
}

/// [`at_war`] at `now`, answering the war's id.
pub fn at_war_at(reg: &Registry, a_owner: i64, b_owner: i64, now: BlueTime) -> Option<i64> {
    if reg.deltas().of(KIND).is_empty() {
        return None;
    }
    let (a, b) = (owner_ids(reg, a_owner), owner_ids(reg, b_owner));
    war_between(Wars::load(reg.deltas()).all(), &a, &b, now)
}

/// **Is a hostile act between these two pilots legal?** The first war in
/// which the attacker's war entity fights the victim's: `Some(warID)` means
/// CW-1's classifier treats a high-sec act as legal (no suspect or criminal
/// flag, no crime hook, so CONCORD never sees it). Cooldown still counts.
pub fn legal(reg: &Registry, attacker: i64, victim: i64, now: BlueTime) -> Option<i64> {
    let (ac, vc) = (reg.character(attacker)?, reg.character(victim)?);
    legal_between(
        reg,
        (ac.corporation_id, ac.alliance_id),
        (vc.corporation_id, vc.alliance_id),
        now,
    )
}

/// [`legal`] between two `(corp, alliance)` pairs: a pilot's, or a corp's
/// property.
pub fn legal_between(
    reg: &Registry,
    a: (i64, Option<i64>),
    b: (i64, Option<i64>),
    now: BlueTime,
) -> Option<i64> {
    if reg.deltas().of(KIND).is_empty() {
        return None;
    }
    let side = |(corp, alliance): (i64, Option<i64>)| {
        let mut v = vec![corp];
        v.extend(alliance);
        v
    };
    war_between(Wars::load(reg.deltas()).all(), &side(a), &side(b), now)
}

// ---------------------------------------------------------------------------
// Declaring
// ---------------------------------------------------------------------------

/// What [`refusal`] weighs: the facts about one declaration, gathered by
/// the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclareFacts {
    /// The caller holds the Director role in the declaring corp (an
    /// alliance: its executor).
    pub director: bool,
    /// The target is the declarer itself.
    pub same_owner: bool,
    /// The target is an NPC corporation.
    pub target_npc: bool,
    /// The target is no open player corp or alliance.
    pub target_unknown: bool,
    /// The target is a corporation inside an alliance.
    pub target_in_alliance: bool,
    /// The declarer is a corporation inside an alliance (it must leave, or
    /// the alliance must declare).
    pub declarer_in_alliance: bool,
    /// A war between them is already running.
    pub already_at_war: bool,
    /// Both sides hold a war permit.
    pub permits: bool,
    /// A live peace treaty forbids it.
    pub treaty: bool,
    /// The named HQ is the declarer's valid one.
    pub hq_valid: bool,
    /// Aggressive wars the declarer is running now.
    pub active_wars: usize,
    pub balance: f64,
}

/// **The refusal a declaration earns, if any**, in R-WAR §3.1's order, with
/// the client's own keys: role, self, NPC target, alliance membership,
/// already at war, permit and treaty, HQ, too many wars, money.
pub fn refusal(f: &DeclareFacts) -> Option<Refusal> {
    if !f.director {
        return Some(Refusal::Access(
            "Only a director can declare war for a corporation.",
        ));
    }
    if f.same_owner {
        return Some(Refusal::Key("CanNotDeclareWarAgainstSelf"));
    }
    if f.target_npc {
        return Some(Refusal::Key("CrpCantDeclareWarOnNPCCorp"));
    }
    if f.target_unknown {
        return Some(Refusal::Key("CanNotDeclareWarAgainstThem"));
    }
    if f.target_in_alliance {
        return Some(Refusal::Key("CrpCanNotSanctionWarAgainstOwner"));
    }
    if f.declarer_in_alliance {
        return Some(Refusal::Key("CrpCanNotSanctionWarOwner"));
    }
    if f.already_at_war {
        return Some(Refusal::Key("AlreadyAtWarWithThem"));
    }
    if !f.permits || f.treaty {
        return Some(Refusal::Key("CanNotDeclareWarAgainstThem"));
    }
    if !f.hq_valid {
        return Some(Refusal::Key("CanNotDeclareWarInvalidHQ"));
    }
    if f.active_wars >= MAX_WARS {
        return Some(Refusal::Key("CrpTooManyWarDeclarations"));
    }
    if f.balance < COST_OF_WAR {
        return Some(Refusal::Money {
            amount: COST_OF_WAR,
            balance: f.balance,
        });
    }
    None
}

/// **`DeclareWarAgainst(againstID, warHQ)`** for the character `actor`:
/// through `corpRegistry` (`as_alliance` false: the declarer is the caller's
/// corp) or `allianceRegistry` (true: the caller's alliance, the caller a
/// director of its executor). Re-checks everything the wizard checked, then
/// charges [`COST_OF_WAR`] (ref 49, to CONCORD), creates the war with its
/// spool-up, pushes `OnWarChanged` and sends the notices. Answers the war id.
pub fn declare(
    reg: &mut Registry,
    actor: i64,
    as_alliance: bool,
    against: i64,
    war_hq: Option<i64>,
) -> Result<i64, Refusal> {
    let now = crate::pyutil::filetime_now();
    let Some(member) = reg.corporations().membership(actor) else {
        return Err(Refusal::Access(
            "Only a director can declare war for a corporation.",
        ));
    };
    let my_corp = member.corporation_id;
    let my_alliance = crate::alliance::of_corp(reg.deltas(), my_corp);
    let director = reg
        .corporations()
        .get(my_corp)
        .is_some_and(|c| c.ceo_id == actor)
        || member.roles & crate::corp_roles::CORP_ROLE_DIRECTOR != 0;
    // The declaring owner, and whether the caller speaks for it.
    let (owner, speaks) = if as_alliance {
        let st = Alliances::load(reg.deltas());
        match my_alliance.and_then(|a| st.get(a).map(|al| (a, al.executor_corp_id))) {
            Some((a, exec)) => (a, exec == Some(my_corp)),
            None => (my_corp, false),
        }
    } else {
        (my_corp, true)
    };
    let wars = Wars::load(reg.deltas());
    let target_corp = if is_alliance_id(against) {
        None
    } else {
        reg.corporations().get(against).filter(|c| !c.deleted)
    };
    let facts = DeclareFacts {
        director: director && speaks,
        same_owner: against == owner || against == my_corp || Some(against) == my_alliance,
        target_npc: is_npc_corporation(against),
        target_unknown: !is_npc_corporation(against) && !owner_exists(reg, against),
        target_in_alliance: target_corp
            .is_some_and(|c| crate::alliance::of_corp(reg.deltas(), c.corporation_id).is_some()),
        declarer_in_alliance: !as_alliance && my_alliance.is_some(),
        already_at_war: wars.between(owner, against, now).is_some(),
        permits: permit(reg, owner) && permit(reg, against),
        treaty: crate::war_deal::treaty_between(reg.deltas(), owner, against, now),
        hq_valid: war_hq.is_some_and(|item| {
            hq_of(reg.deltas(), owner)
                .is_some_and(|h| h.item_id == item && high_sec(reg, h.system_id))
        }),
        active_wars: wars
            .all()
            .filter(|w| w.declared_by == owner && !w.mutual && w.active(now))
            .count(),
        balance: balance_of(reg, owner),
    };
    if let Some(r) = refusal(&facts) {
        return Err(corp_money(reg, owner, r));
    }
    let war_id = take_id(reg.deltas_mut(), "war");
    let mut war = War::new(war_id, owner, against, now, span(reg, WAR_SPOOLUP), war_hq);
    if !pay(reg, owner, COST_OF_WAR, REF_WAR_FEE, CONCORD_OWNER, war_id) {
        return Err(corp_money(
            reg,
            owner,
            Refusal::Money {
                amount: COST_OF_WAR,
                balance: balance_of(reg, owner),
            },
        ));
    }
    war.bill_id = crate::war_deal::issue_first_bill(reg, &war, now);
    put(reg.deltas_mut(), &war);
    tell_change(reg, None, &war, Via::Call);
    crate::war_notice::declared(reg, &war, actor);
    Ok(war_id)
}

// ---------------------------------------------------------------------------
// Ending a war
// ---------------------------------------------------------------------------

/// How a war ends: the reason, who ended it, when it stops being hostile
/// (default: the cooldown from now; `Some(now)` for no cooldown), and the
/// peace treaty it leaves.
#[derive(Debug, Clone, Default)]
pub struct FinishWith {
    pub reason: i64,
    pub by: Option<i64>,
    /// `Some(t)`: finished at exactly `t`. `None`: `now` plus the cooldown.
    pub at: Option<BlueTime>,
    /// `Some(reason)`: forbid the aggressor to declare on the defender again
    /// for [`FORCED_PEACE_PERIOD`] (both directions when `peace_both`).
    pub peace: Option<i64>,
    pub peace_both: bool,
    /// The war is `retracted` now (a retract or an accepted surrender).
    pub retract: bool,
}

/// **End a war**: set `timeFinished`, the reason and (if asked) `retracted`,
/// leave the treaty, push `OnWarChanged`, and tell the parties. A war that
/// already finished is left alone. Answers whether it ended.
pub fn finish(reg: &mut Registry, war_id: i64, how: FinishWith, now: BlueTime, via: Via) -> bool {
    let Some(old) = load_war(reg.deltas(), war_id) else {
        return false;
    };
    if old.time_finished.is_some() {
        return false;
    }
    let mut w = old.clone();
    let cooldown = span(reg, WAR_COOLDOWN);
    w.time_finished = Some(how.at.unwrap_or_else(|| now.saturating_add(cooldown)));
    w.reason_ended = Some(how.reason);
    if how.retract {
        w.retracted = Some(now);
        w.retracted_by = how.by;
    }
    // Allies stand down with the war.
    let end = w.time_finished.unwrap_or(now);
    for a in w.allies.values_mut() {
        a.time_finished = a.time_finished.min(end);
    }
    put(reg.deltas_mut(), &w);
    if let Some(reason) = how.peace {
        let until = now.saturating_add(span(reg, FORCED_PEACE_PERIOD));
        crate::war_deal::make_peace(
            reg.deltas_mut(),
            w.declared_by,
            w.against,
            until,
            reason,
            war_id,
        );
        if how.peace_both {
            crate::war_deal::make_peace(
                reg.deltas_mut(),
                w.against,
                w.declared_by,
                until,
                reason,
                war_id,
            );
        }
    }
    crate::war_deal::war_ended(reg, &w);
    tell_change(reg, Some(&old), &w, via);
    crate::war_notice::ended(reg, &w, &how, now);
    true
}

/// A corporation closed (its last CEO resigned): every war it is a principal
/// of ends, `ENDED_CORP_DELETED`, with no cooldown.
pub fn on_corp_closed(reg: &mut Registry, corp: i64) {
    end_all_of(reg, corp, ended::CORP_DELETED);
}

/// An alliance closed: every war it is a principal of ends,
/// `ENDED_ALLIANCE_DELETED`.
pub fn on_alliance_closed(reg: &mut Registry, alliance: i64) {
    end_all_of(reg, alliance, ended::ALLIANCE_DELETED);
}

fn end_all_of(reg: &mut Registry, owner: i64, reason: i64) {
    let now = crate::pyutil::filetime_now();
    let ids: Vec<i64> = Wars::load(reg.deltas())
        .of_owner(owner)
        .filter(|w| w.active(now))
        .map(|w| w.war_id)
        .collect();
    for id in ids {
        finish(
            reg,
            id,
            FinishWith {
                reason,
                at: Some(now),
                ..FinishWith::default()
            },
            now,
            Via::Call,
        );
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The world-clock stage** (`tick::STAGES`): at the moment a war can be
/// fought (and when an ally joins, and when a war's cooldown ends) the
/// parties' windows are told, so the overview recolours without waiting for
/// the client's ten-second poll; the start is announced (notification 28);
/// deals that expired and bills that fell due are `war_deal`'s; finished wars
/// past their keep are dropped. Cheap when there is nothing: it reads the
/// war rows' times and nothing else.
pub fn tick(reg: &mut Registry, _: std::time::Instant, now: BlueTime) {
    if !reg.deltas().of(KIND).is_empty() {
        let due = wars_due(reg.deltas(), now);
        for id in due {
            tick_one(reg, id, now);
        }
    }
    crate::war_deal::tick(reg, now);
}

/// The wars with something to do at `now`: a start or end to tell, an ally
/// to bring in, or a keep that ran out.
fn wars_due(d: &Deltas, now: BlueTime) -> Vec<i64> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let id = num(k, 0).ok()?;
            let started = num(v, 3).ok()?;
            let finished = opt(v, 4).ok()?;
            let notified = opt(v, 14).ok()?.unwrap_or(0);
            let start_due = started < now && notified & TOLD_STARTED == 0;
            let finish_due = finished.is_some_and(|f| f < now) && notified & TOLD_FINISHED == 0;
            let prune_due = finished.is_some_and(|f| f.saturating_add(KEEP_FINISHED) < now);
            let prefix = id.to_string();
            let ally_due = d
                .of(KIND_ALLY)
                .range(vec![prefix.clone()]..)
                .take_while(|(ak, _)| ak.first() == Some(&prefix))
                .any(|(_, av)| {
                    num(av, 0).is_ok_and(|s| s < now) && opt(av, 2).ok().flatten().unwrap_or(0) == 0
                });
            (start_due || finish_due || ally_due || prune_due).then_some(id)
        })
        .collect()
}

fn tick_one(reg: &mut Registry, id: i64, now: BlueTime) {
    let Some(mut w) = load_war(reg.deltas(), id) else {
        return;
    };
    if w.time_finished
        .is_some_and(|f| f.saturating_add(KEEP_FINISHED) < now)
    {
        remove(reg.deltas_mut(), id);
        return;
    }
    let before = w.clone();
    let mut retell = false;
    if w.time_started < now && w.notified & TOLD_STARTED == 0 {
        w.notified |= TOLD_STARTED;
        retell = true;
        // A war that ended before it could be fought says nothing.
        if w.reason_ended.is_none() {
            crate::war_notice::fighting_legal(reg, &w);
        }
    }
    if w.time_finished.is_some_and(|f| f < now) && w.notified & TOLD_FINISHED == 0 {
        w.notified |= TOLD_FINISHED;
        retell = true;
    }
    let starts: Vec<i64> = w
        .allies
        .values()
        .filter(|a| a.time_started < now && !a.told)
        .map(|a| a.ally_id)
        .collect();
    for ally in starts {
        if let Some(a) = w.allies.get_mut(&ally) {
            a.told = true;
        }
        retell = true;
    }
    if w != before {
        put(reg.deltas_mut(), &w);
    }
    if retell {
        // `OnWarChanged` makes the client drop its overview colours
        // (`RemoveWarOwners`, `InvalidateFlags`): a war that became
        // fightable, gained an ally or ended is recoloured now.
        tell_change(reg, Some(&w), &w, Via::Clock);
    }
}

// ---------------------------------------------------------------------------
// War kills
// ---------------------------------------------------------------------------

/// **The `kill_hooks::PLAYER` row** after the killmail is written: a kill
/// whose final blow (or another attacker) fought the victim in a war is
/// filed under it (`warkill`), so the war report and the killmail's `warID`
/// can find it.
pub fn on_kill(w: &mut Registry, ev: &crate::kill_hooks::KillEvent) {
    if w.deltas().of(KIND).is_empty() {
        return;
    }
    let Some(report) = crate::killmail::involving(w, ev.victim_character, None, 1)
        .into_iter()
        .find(|r| r.victim == ev.victim_character && r.time == ev.when)
    else {
        return;
    };
    if let Some(war) = war_of_report(w, &report) {
        w.deltas_mut().put(
            KIND_KILL,
            vec![war.to_string(), report.kill_id.to_string()],
            vec![],
        );
    }
}

/// The war a report belongs to: the first attacker (the final blow first)
/// that fought the victim at the time of the kill.
pub fn war_of_report(reg: &Registry, r: &crate::killmail::Report) -> Option<i64> {
    let wars = Wars::load(reg.deltas());
    let victim = {
        let mut v = vec![r.victim_corporation];
        v.extend(r.victim_alliance);
        v
    };
    std::iter::once(&r.final_blow)
        .chain(r.attackers.iter())
        .find_map(|a| {
            let mut side = Vec::new();
            side.extend(a.corporation);
            side.extend(a.alliance);
            war_between(wars.all(), &side, &victim, r.time)
        })
}

/// The war a kill was filed under, for the killmail's `warID`.
pub fn war_of_kill(reg: &Registry, kill_id: i64) -> Option<i64> {
    let kill = kill_id.to_string();
    reg.deltas()
        .of(KIND_KILL)
        .keys()
        .find(|k| k.get(1) == Some(&kill))
        .and_then(|k| num(k, 0).ok())
}

/// Every kill filed under a war, in kill id order.
pub fn kills_of_war(d: &Deltas, war_id: i64) -> Vec<i64> {
    let prefix = war_id.to_string();
    let mut v: Vec<i64> = d
        .of(KIND_KILL)
        .range(vec![prefix.clone()]..)
        .take_while(|(k, _)| k.first() == Some(&prefix))
        .filter_map(|(k, _)| num(k, 1).ok())
        .collect();
    v.sort_unstable();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented owners; the times are invented too (the rules are about the
    /// order of times, not their size).
    const AGG: i64 = 98_000_001;
    const DEF: i64 = 98_000_002;
    const ALLY: i64 = 98_000_003;
    const OTHER: i64 = 98_000_004;
    const ALLIANCE: i64 = 99_000_001;

    fn war() -> War {
        War::new(7, AGG, DEF, 1_000, 500, Some(42))
    }

    fn with_ally(mut w: War, from: i64, to: i64) -> War {
        w.allies.insert(
            ALLY,
            Ally {
                ally_id: ALLY,
                time_started: from,
                time_finished: to,
                told: false,
            },
        );
        w
    }

    /// `IsWarInHostileState` and its three neighbours, ported verbatim.
    #[test]
    fn the_clients_state_predicates_hold() {
        let mut w = war(); // declared 1000, can be fought after 1500
        assert!(in_spoolup(&w, 1_499));
        assert!(!in_spoolup(&w, 1_500));
        assert!(!in_hostile_state(&w, 1_500), "strictly after the start");
        assert!(in_hostile_state(&w, 1_501));
        assert!(!in_cooldown(&w, 2_000), "a running war is not cooling down");
        assert!(!is_finished(&w, 2_000));
        // Retracted at 2000 with a cooldown to 3000: still hostile.
        w.time_finished = Some(3_000);
        assert!(in_cooldown(&w, 2_500));
        assert!(in_hostile_state(&w, 2_999), "cooldown still counts as war");
        assert!(!is_finished(&w, 2_999));
        assert!(!in_hostile_state(&w, 3_000), "over at the finish time");
        assert!(!in_hostile_state(&w, 3_001));
        assert!(is_finished(&w, 3_001));
        assert!(!in_cooldown(&w, 3_001));
        // A war finished before it started was never fought.
        w.time_finished = Some(1_200);
        assert!(!in_hostile_state(&w, 1_501));
        // Allies.
        let a = Ally {
            ally_id: ALLY,
            time_started: 10,
            time_finished: 20,
            told: false,
        };
        assert!(!ally_active(&a, 10) && ally_active(&a, 11) && !ally_active(&a, 20));
        assert!(ally_pending_or_active(&a, 5) && !ally_pending_or_active(&a, 20));
    }

    /// `IsAtWar`: the aggressor against the defender or an active ally of it,
    /// never the defender against an ally.
    #[test]
    fn is_at_war_is_the_clients() {
        let w = with_ally(war(), 2_000, 4_000);
        let wars = [w];
        let at = |entities: &[i64], now| is_at_war(wars.iter(), entities, now);
        assert_eq!(at(&[AGG, DEF], 1_400), None, "spooling up");
        assert_eq!(at(&[AGG, DEF], 1_501), Some(7));
        assert_eq!(at(&[DEF, AGG], 1_501), Some(7), "the pair has no direction");
        assert_eq!(at(&[AGG, OTHER], 3_000), None);
        // The ally before and during its window, and after.
        assert_eq!(at(&[AGG, ALLY], 1_999), None);
        assert_eq!(at(&[AGG, ALLY], 3_000), Some(7));
        assert_eq!(at(&[AGG, ALLY], 4_000), None);
        // The defender and its ally are friends; the aggressor is not
        // anybody's ally's friend.
        assert_eq!(at(&[DEF, ALLY], 3_000), None);
        assert_eq!(at(&[ALLY, OTHER], 3_000), None);
    }

    /// `war_between` is the same over sets of owner ids (a pilot's corp and
    /// its alliance), in either order.
    #[test]
    fn war_between_takes_each_sides_corp_and_alliance() {
        let w = with_ally(war(), 2_000, 4_000);
        let wars = [w];
        let between = |a: &[i64], b: &[i64], now| war_between(wars.iter(), a, b, now);
        // The aggressor corp is in an alliance: either id works.
        assert_eq!(between(&[OTHER, AGG], &[DEF], 1_501), Some(7));
        assert_eq!(between(&[DEF], &[AGG, ALLIANCE], 1_501), Some(7));
        assert_eq!(
            between(&[ALLIANCE], &[DEF], 1_501),
            None,
            "the alliance is not the war's owner"
        );
        // Allies fight the aggressor in their window only.
        assert_eq!(between(&[ALLY], &[AGG], 1_999), None);
        assert_eq!(between(&[ALLY], &[AGG], 2_001), Some(7));
        assert_eq!(between(&[AGG], &[ALLY, OTHER], 2_001), Some(7));
        // Allies and the defender are on one side.
        assert_eq!(between(&[ALLY], &[DEF], 2_001), None);
        assert_eq!(between(&[ALLY], &[ALLY], 2_001), None);
        // The aggressor is never at war with itself.
        assert_eq!(between(&[AGG], &[AGG], 2_001), None);
        // An ended war is no war.
        let mut over = war();
        over.time_finished = Some(1_800);
        assert_eq!(war_between([&over], &[AGG], &[DEF], 1_900), None);
    }

    /// The refusal for a declaration is the first of R-WAR §3.1's list that
    /// applies, with the client's own keys.
    #[test]
    fn a_declaration_is_refused_by_the_first_rule_it_breaks() {
        let ok = DeclareFacts {
            director: true,
            same_owner: false,
            target_npc: false,
            target_unknown: false,
            target_in_alliance: false,
            declarer_in_alliance: false,
            already_at_war: false,
            permits: true,
            treaty: false,
            hq_valid: true,
            active_wars: 0,
            balance: COST_OF_WAR,
        };
        assert_eq!(refusal(&ok), None, "exactly the fee is enough");
        type Break = fn(&mut DeclareFacts);
        let ladder: [(Break, Refusal); 11] = [
            (
                |f| f.director = false,
                Refusal::Access("Only a director can declare war for a corporation."),
            ),
            (
                |f| f.same_owner = true,
                Refusal::Key("CanNotDeclareWarAgainstSelf"),
            ),
            (
                |f| f.target_npc = true,
                Refusal::Key("CrpCantDeclareWarOnNPCCorp"),
            ),
            (
                |f| f.target_unknown = true,
                Refusal::Key("CanNotDeclareWarAgainstThem"),
            ),
            (
                |f| f.target_in_alliance = true,
                Refusal::Key("CrpCanNotSanctionWarAgainstOwner"),
            ),
            (
                |f| f.declarer_in_alliance = true,
                Refusal::Key("CrpCanNotSanctionWarOwner"),
            ),
            (
                |f| f.already_at_war = true,
                Refusal::Key("AlreadyAtWarWithThem"),
            ),
            (
                |f| f.permits = false,
                Refusal::Key("CanNotDeclareWarAgainstThem"),
            ),
            (
                |f| f.treaty = true,
                Refusal::Key("CanNotDeclareWarAgainstThem"),
            ),
            (
                |f| f.hq_valid = false,
                Refusal::Key("CanNotDeclareWarInvalidHQ"),
            ),
            (
                |f| f.active_wars = MAX_WARS,
                Refusal::Key("CrpTooManyWarDeclarations"),
            ),
        ];
        for (i, (breaks, want)) in ladder.iter().enumerate() {
            let mut f = ok.clone();
            breaks(&mut f);
            assert_eq!(refusal(&f), Some(want.clone()), "rule {i}");
            // Every rule after it is broken too, and it still wins.
            for (later, _) in &ladder[i..] {
                later(&mut f);
            }
            f.balance = 0.0;
            assert_eq!(
                refusal(&f),
                Some(want.clone()),
                "rule {i} outranks the later ones"
            );
        }
        let mut poor = ok;
        poor.balance = COST_OF_WAR - 0.01;
        assert_eq!(
            refusal(&poor),
            Some(Refusal::Money {
                amount: COST_OF_WAR,
                balance: COST_OF_WAR - 0.01
            })
        );
        assert_eq!(
            MAX_WARS,
            ladder.len() - 6,
            "the cap is a named assumption, not a number here"
        );
    }

    /// Rows round-trip, an ally dropped from a war drops its row, and ids
    /// count up and are never reused.
    #[test]
    fn a_war_and_its_allies_round_trip_through_the_deltas() {
        let mut d = Deltas::default();
        let mut w = with_ally(war(), 2_000, 4_000);
        w.time_finished = Some(9_000);
        w.retracted = Some(8_000);
        w.retracted_by = Some(DEF);
        w.bill_id = Some(3);
        w.mutual = false;
        w.created_from = Some(2);
        w.open_for_allies = true;
        w.reason_ended = Some(ended::SURRENDER);
        w.notified = TOLD_STARTED | TOLD_FINISHED;
        put(&mut d, &w);
        assert_eq!(load_war(&d, 7), Some(w.clone()));
        assert_eq!(Wars::load(&d).get(7), Some(&w));
        // Dropping the ally removes its row; a prefix-sharing id is untouched.
        let other = with_ally(War::new(70, AGG, OTHER, 1, 1, None), 5, 6);
        put(&mut d, &other);
        w.allies.clear();
        put(&mut d, &w);
        assert_eq!(d.of(KIND_ALLY).len(), 1, "war 70's ally stays");
        assert_eq!(load_war(&d, 7).map(|x| x.allies.len()), Some(0));
        assert_eq!(load_war(&d, 70).map(|x| x.allies.len()), Some(1));
        // Removing a war takes its allies and kills.
        d.put(KIND_KILL, vec!["70".into(), "5".into()], vec![]);
        remove(&mut d, 70);
        assert!(d.of(KIND_ALLY).is_empty() && d.of(KIND_KILL).is_empty());
        assert!(load_war(&d, 70).is_none() && load_war(&d, 7).is_some());
        // Counters.
        assert_eq!(take_id(&mut d, "war"), 1);
        assert_eq!(take_id(&mut d, "war"), 2);
        assert_eq!(take_id(&mut d, "neg"), 1);
        // A row from an older build without `notified` still reads.
        let mut row = w.to_row();
        row.pop();
        d.put(KIND, vec!["9".into()], row);
        assert_eq!(load_war(&d, 9).map(|x| x.notified), Some(0));
        // A damaged row is skipped, not fatal.
        d.put(KIND, vec!["10".into()], vec!["x".into()]);
        assert!(Wars::load(&d).get(10).is_none());
    }

    /// The wire form: all 17 of the client's attributes, the allies as a dict
    /// of rows, and the change dict shaped for `HandleWarChange`.
    #[test]
    fn the_wire_form_has_the_clients_seventeen_fields() {
        let w = with_ally(war(), 2_000, 4_000);
        let fields = war_fields(&w, 1_600);
        let names: Vec<&str> = fields.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "warID",
                "declaredByID",
                "againstID",
                "timeDeclared",
                "timeFinished",
                "retracted",
                "timeStarted",
                "retractedBy",
                "billID",
                "mutual",
                "allies",
                "createdFromWarID",
                "openForAllies",
                "canBeRetracted",
                "reasonEnded",
                "warHQ",
                "reasonStarted",
            ]
        );
        assert_eq!(names.len(), 17);
        // A new war: every field `(None, value)`.
        let add = war_change(None, &w, 1_600);
        let pairs = add.dict_as_kv().expect("a dict");
        assert_eq!(pairs.len(), 17);
        assert!(
            pairs
                .iter()
                .all(|(_, v)| matches!(v, PyValue::Tuple(t) if t[0] == PyValue::None))
        );
        // An update: only what changed, and the id as `(id, id)`.
        let mut later = w.clone();
        later.open_for_allies = true;
        let upd = war_change(Some(&w), &later, 1_600);
        let keys: Vec<String> = upd
            .dict_as_kv()
            .expect("a dict")
            .iter()
            .map(|(k, _)| {
                String::from_utf8_lossy(crate::pyutil::as_bytes(k).unwrap_or(b"")).into_owned()
            })
            .collect();
        assert_eq!(keys, vec!["warID", "openForAllies"]);
        let id = upd
            .dict_as_kv()
            .and_then(|kv| kv.into_iter().next())
            .map(|(_, v)| v.clone());
        assert_eq!(
            id,
            Some(PyValue::Tuple(vec![PyValue::Int(7), PyValue::Int(7)]))
        );
        // A mutual war may be retracted until it is finished; others never.
        let mut m = w.clone();
        assert!(!m.can_be_retracted(1_600));
        m.mutual = true;
        assert!(m.can_be_retracted(1_600));
        m.time_finished = Some(1_700);
        assert!(m.can_be_retracted(1_650), "cooling down is not finished");
        assert!(!m.can_be_retracted(1_800));
    }

    #[test]
    fn owner_id_ranges_are_the_clients() {
        assert!(is_alliance_id(99_000_000) && is_alliance_id(99_999_999));
        assert!(!is_alliance_id(98_999_999) && !is_alliance_id(100_000_000));
        assert!(is_npc_corporation(1_000_000) && is_npc_corporation(1_999_999));
        assert!(!is_npc_corporation(98_000_001));
    }

    /// The scaled spans: the game clock compresses what a deadline is made
    /// from, never below one tick.
    #[test]
    fn spans_are_compressed_by_the_game_clock() {
        let mut reg = crate::testkit::registry();
        assert_eq!(span(&reg, WAR_SPOOLUP), WAR_SPOOLUP);
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(24).unwrap());
        assert_eq!(span(&reg, WAR_SPOOLUP), HOUR);
        assert_eq!(span(&reg, WAR_COOLDOWN), HOUR);
        assert_eq!(span(&reg, ALLY_SPOOLUP), HOUR / 6);
        assert_eq!(span(&reg, BILLING_PERIOD), 7 * HOUR);
        assert_eq!(span(&reg, FORCED_PEACE_PERIOD), 14 * HOUR);
        let w = War::new(1, AGG, DEF, 100, span(&reg, WAR_SPOOLUP), None);
        assert_eq!(w.time_started, 100 + HOUR);
    }

    /// The war pushes are the handlers' signatures: `OnWarChanged(war,
    /// ownerIDs, change)` and `OnAllowWarUpdated(corpID, allowWar)`.
    #[test]
    fn the_pushes_are_the_handlers_signatures() {
        use crate::notify::Push;
        let w = war();
        let p = WarChanged {
            to: vec![1, 2],
            war: war_value(&w, 0),
            owners: w.owners(),
            change: war_change(None, &w, 0),
        };
        let (event, args) = p.to_push();
        assert_eq!((event, args.len()), ("OnWarChanged", 3));
        assert_eq!(
            args[1],
            crate::fleet_finder::py_set(&[AGG, DEF].into_iter().collect()),
            "a set: the client intersects it with other sets"
        );
        assert_eq!(p.audience(), Some(vec![1, 2]));
        let q = AllowWarUpdated {
            to: vec![3],
            corp: AGG,
            allow: true,
        };
        assert_eq!(
            q.to_push(),
            (
                "OnAllowWarUpdated",
                vec![PyValue::Int(AGG), PyValue::Int(1)]
            )
        );
    }
}
