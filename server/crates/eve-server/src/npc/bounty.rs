//! The bounty an NPC pays whoever killed it, and the security status the
//! kill earns (W17 CP50, C6 N8).
//!
//! The ISK half was moved verbatim out of `npc.rs` by W10.5 seam S4
//! (`docs/content/plan.md` §6.1); `crate::npc::pay_bounty` is a re-export,
//! and `kill_hooks::NPC` its caller. Since wave E's E6 a kill no longer
//! books a journal row: it adds to a **pending payout** ([`PENDING_KIND`]),
//! and [`tick`] pays each pending payout as one aggregated ref-85 row when
//! its [`BOUNTY_PERIOD`] is up. The security-status half is [`on_kill`], a
//! `kill_hooks::NPC` row of its own (CA15), with the status stored as a
//! [`KIND`] delta and pushed with `OnSecurityStatusUpdate`.

use crate::damage::BlueTime;
use crate::kill_hooks::KillEvent;
use crate::notify::Push;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// The bounty
// ---------------------------------------------------------------------------

/// `appConst.refBountyPrize` = 17 **[V]** (`eve/common/lib/appConst.py:843`);
/// `accountingentrytypes` row 17 is "Bounty Prize", "Player got bounty prize
/// for killing someone" **[V-data]**. Since E6 the server books no row of it
/// (a kill is paid through [`REF_BOUNTY_PRIZES`]); the constant stays, the
/// name the client's formatter and the econ ledger know a single kill by.
pub const REF_BOUNTY_PRIZE: i64 = 17;

/// `appConst.refBountyPrizes` = 85 **[V]** (`appConst.pyc`, line 907);
/// `accountingentrytypes` row 85 is "Bounty Prizes" **[V-data]**, with no
/// `entryJournalMessageID` (its presence bitfield is 0 **[V-data]**), so
/// `FmtWalletEntry` takes `_FmtWalletEntryManual`'s ref-85 arm
/// (`walletEntryFormat.py:98-100`) **[V]**: `UI/Generic/FormatReference/
/// bountyPrizes`, "{name1} got bounty prizes for killing pirates in
/// {location}" (message 59877's sibling label, read out of the
/// localisation corpus) **[V]**, with `location = GetLocation(arg1) or
/// cfg.evelocations.Get(arg1).name` and `name1 = GetName(ownerID2)`. So the
/// row's **reference is the solar system** the kills were in, and its
/// wallet (`ownerID2`) the killer.
pub const REF_BOUNTY_PRIZES: i64 = 85;

/// **[A]** How long kills add up before they are paid: **20 minutes**, from
/// the first kill of a payout. The aggregation itself is **[V]**: the
/// client's per-kill line is "{isk} added to next bounty payout"
/// (`UI/Inflight/ActivityLog/BountyAddedToPayoutMessage1`, message 517488)
/// and its push carries a `payoutTime` (`bountyNotificationAdapter.py:12`);
/// the period is in no client file (a grep of every `.pyc` and the en-us
/// corpus for a bounty interval finds only the ESS one) and 20 minutes is
/// the figure players quote. Falsified by a reference server's ref-85 rows
/// for one ratter arriving at another spacing, or on a server-wide clock
/// rather than from the first kill.
pub const BOUNTY_PERIOD: Duration = Duration::from_secs(20 * 60);

/// **[I]** The first party of a ref-85 row: CONCORD (`1000125`), which the
/// public journal API names as the payer of bounty prizes. The client's
/// line reads only `ownerID2` and the reference, so nothing on screen
/// depends on it.
pub const BOUNTY_PAYER: i64 = 1_000_125;

/// **[A]** At most this many NPC types are listed in a payout's
/// description; past it the list is cut and `NBLT` set, which the client
/// renders as a "Truncated" line (`transactionsUtil.py:237-238`,
/// `HintBountyTruncated`) **[V]**. Falsified by a client whose tooltip
/// shows a longer list cut elsewhere.
pub const BOUNTY_LIST_MAX: usize = 10;

/// The delta kind pending payouts live in:
/// `char system -> amount due_at kills`, where `amount` is the ISK owed,
/// `due_at` the FILETIME [`tick`] pays it at, and `kills` the
/// `type:count,type:count` tally the description lists. Claimed in
/// `delta_kinds::KINDS`.
///
/// **A pending payout survives a save**: it is a delta row, written with
/// the same snapshot as the DBS drain its kills made, so a crash loses both
/// or neither. It is **not** flushed on logoff -- the payout is due by the
/// clock whoever is online, as an offline character's other wallet rows
/// are, and a restart pays every row whose `due_at` has passed on its
/// first tick.
pub const PENDING_KIND: &str = "bountypay";

/// `appConst.recDescNpcBountyList` = `'NBL'` and
/// `recDescNpcBountyListTruncated` = `'NBLT'` **[V]** (`appConst.pyc`,
/// lines 1183-1184): the description keys `GetTransactionHint` reads for
/// the tooltip's "{type} x {count}" lines (`transactionsUtil.py:233-238`).
pub const DESC_NPC_BOUNTY_LIST: &str = "NBL";
pub const DESC_NPC_BOUNTY_LIST_TRUNCATED: &str = "NBLT";

/// One FILETIME second.
const FILETIME_SECOND: i64 = 10_000_000;

/// A payout that kills have added to and [`tick`] has not paid yet: one
/// [`PENDING_KIND`] row.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub amount: f64,
    pub due_at: BlueTime,
    /// NPC type id -> how many of it were killed.
    pub kills: BTreeMap<i64, u32>,
}

impl Pending {
    fn key(character: i64, system: i64) -> Vec<String> {
        vec![character.to_string(), system.to_string()]
    }

    fn from_row(v: &[String]) -> Option<Pending> {
        let amount = v.first()?.parse::<f64>().ok().filter(|a| a.is_finite())?;
        let due_at = v.get(1)?.parse().ok()?;
        let mut kills = BTreeMap::new();
        for pair in v.get(2).map_or("", String::as_str).split(',') {
            if pair.is_empty() {
                continue;
            }
            let (t, n) = pair.split_once(':')?;
            kills.insert(t.parse().ok()?, n.parse().ok()?);
        }
        Some(Pending {
            amount,
            due_at,
            kills,
        })
    }

    fn to_row(&self) -> Vec<String> {
        let kills = self
            .kills
            .iter()
            .map(|(t, n)| format!("{t}:{n}"))
            .collect::<Vec<_>>()
            .join(",");
        vec![self.amount.to_string(), self.due_at.to_string(), kills]
    }

    /// **The ref-85 row's description**: a YAML flow mapping
    /// `{NBL: {type: count, ...}}`, which `GetTransactionHint` loads with
    /// `yaml.CSafeLoader` and lists as "{type name} x {count}" **[V]**; past
    /// [`BOUNTY_LIST_MAX`] types the biggest counts are kept and
    /// `NBLT: 1` added.
    pub fn description(&self) -> String {
        let mut kills: Vec<(i64, u32)> = self.kills.iter().map(|(t, n)| (*t, *n)).collect();
        let truncated = kills.len() > BOUNTY_LIST_MAX;
        if truncated {
            kills.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            kills.truncate(BOUNTY_LIST_MAX);
            kills.sort_unstable();
        }
        let list = kills
            .iter()
            .map(|(t, n)| format!("{t}: {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        if truncated {
            format!("{{{DESC_NPC_BOUNTY_LIST}: {{{list}}}, {DESC_NPC_BOUNTY_LIST_TRUNCATED}: 1}}")
        } else {
            format!("{{{DESC_NPC_BOUNTY_LIST}: {{{list}}}}}")
        }
    }
}

/// `character`'s pending payout for kills in `system`, if any.
pub fn pending(reg: &Registry, character: i64, system: i64) -> Option<Pending> {
    reg.deltas()
        .of(PENDING_KIND)
        .get(&Pending::key(character, system))
        .and_then(|v| Pending::from_row(v))
}

/// **Pay NPC `npc_id`'s bounty to whoever killed it**: its type's
/// `entityKillBounty` (attribute 481) **[V-data]**, owed to the character
/// behind `killer` -- a pilot, or the owner of a drone (both are
/// `Pilot::owner_id`; an NPC's owner is a corporation, so an NPC killing an
/// NPC pays nobody). Call it **before** the kill takes the NPC out of the
/// table. Answers `(character, amount)` when something was owed.
///
/// **The dynamic bounty system** (CP38, C5 §5.3): the amount is the
/// listed bounty times the NPC's system's multiplier
/// (`econ_dbs::payout`), and what was owed drains that system **per kill**
/// (`econ_dbs::drain`), which may push the new rate to the pilots there.
///
/// **The ISK is not credited here** (E6): the amount and the kill are added
/// to the killer's [`Pending`] payout for that system, which [`tick`] pays
/// as one ref-85 row when [`BOUNTY_PERIOD`] has passed since the payout's
/// first kill. The killer is told at once with
/// [`OnBountyAddedToPayout`], on the clock and addressed, because a kill
/// happens inside the world tick, which is nobody's turn.
pub fn pay_bounty(w: &mut Registry, killer: i64, npc_id: i64) -> Option<(i64, f64)> {
    let npc = w.npcs().get(npc_id)?;
    let (kind, system) = (npc.kind, npc.solar_system_id);
    if kind.bounty.is_nan() || kind.bounty <= 0.0 {
        return None;
    }
    let to = w.pilot(killer)?.owner_id;
    w.character(to)?;
    let amount = book_kill(w, to, system, kind.type_id, kind.bounty, BOUNTY_PERIOD);
    Some((to, amount))
}

/// **Book one kill's bounty** (E1 factored this out of [`pay_bounty`], which
/// calls it unchanged): `listed` ISK for an NPC of `type_id` killed in
/// `system`, owed to `to` through the dynamic bounty system -- the amount is
/// `econ_dbs::payout`, the system drains by it, and it joins `to`'s
/// [`Pending`] payout for that system, due `period` after the payout's first
/// kill. Answers the ISK booked. The offline settlement (`econ_agents`) books
/// a window's kills through this and then [`flush`]es with `period` zero, so
/// a settled bounty is a ref-85 row with the same description a live one has.
pub fn book_kill(
    w: &mut Registry,
    to: i64,
    system: i64,
    type_id: i64,
    listed: f64,
    period: Duration,
) -> f64 {
    let now = crate::pyutil::filetime_now();
    let full = crate::econ_dbs::payout(w, system, listed, now);
    // #89 ESS-1: in a system with an ESS the bank takes its cut first; the
    // pilot's pending payout gets the rest. No ESS, no change.
    let amount = if crate::ess::has_ess_in(w, system) {
        let (pilot, bank) = crate::ess::split(full, crate::ess::BOUNTIES_CUT);
        crate::ess::deposit(w, system, to, bank, now);
        pilot
    } else {
        full
    };
    let mut p = pending(w, to, system).unwrap_or_else(|| Pending {
        amount: 0.0,
        due_at: now + period.as_secs() as i64 * FILETIME_SECOND,
        kills: BTreeMap::new(),
    });
    p.amount += amount;
    *p.kills.entry(type_id).or_default() += 1;
    let due_at = p.due_at;
    w.deltas_mut()
        .put(PENDING_KIND, Pending::key(to, system), p.to_row());
    w.schedule(
        Duration::ZERO,
        OnBountyAddedToPayout {
            character: to,
            amount,
            payout_time: due_at,
            enemy_type_id: type_id,
            is_modified: full != listed,
        },
    );
    // The pool drains by what the system paid out, the bank's part too.
    crate::econ_dbs::drain(w, system, full, now);
    amount
}

/// **The world-clock stage** (E6): pay every pending payout whose `due_at`
/// is at or before `ft`. Exported for `tick::STAGES`.
pub fn tick(w: &mut Registry, _: Instant, ft: BlueTime) {
    flush(w, ft);
    // #89 ESS-1: the main banks' autopayments, on the same slow cadence.
    crate::ess::tick(w, ft);
}

/// **Pay every pending payout due by `now`**, as one ref-85 row each
/// ([`pay_one`]), and drop its row. A row whose character is gone, or that
/// does not parse, is dropped unpaid. Answers how many were paid.
pub fn flush(w: &mut Registry, now: BlueTime) -> usize {
    let rows = w.deltas().of(PENDING_KIND);
    if rows.is_empty() {
        return 0;
    }
    let due: Vec<(Vec<String>, Option<Pending>)> = rows
        .iter()
        .filter_map(|(k, v)| {
            let p = Pending::from_row(v);
            match &p {
                Some(p) if p.due_at > now => None,
                _ => Some((k.clone(), p)),
            }
        })
        .collect();
    let mut paid = 0;
    for (key, p) in due {
        w.deltas_mut().del(PENDING_KIND, &key);
        let parsed = (|| Some((key.first()?.parse().ok()?, key.get(1)?.parse().ok()?)))();
        if let (Some(p), Some((character, system))) = (p, parsed)
            && pay_one(w, character, system, &p, now).is_some()
        {
            paid += 1;
        }
    }
    paid
}

/// **Credit one payout**: the balance, one [`REF_BOUNTY_PRIZES`] row dated
/// `now` (first party [`BOUNTY_PAYER`], wallet `character`, reference the
/// system, description [`Pending::description`]), and `OnAccountChange`
/// scheduled to the character. Answers the new balance.
fn pay_one(
    w: &mut Registry,
    character: i64,
    system: i64,
    p: &Pending,
    now: BlueTime,
) -> Option<f64> {
    w.character(character)?;
    // CORP-W: the corporation's cut goes to its Master Wallet; the row is the
    // net, with the tax stamped in for the client's derived tax line.
    let skim = crate::service::corp_wallet::skim(
        w,
        crate::service::corp_wallet::Via::Clock,
        character,
        p.amount,
        REF_BOUNTY_PRIZES,
        system,
    );
    let description =
        crate::service::corp_wallet::stamped(&p.description(), REF_BOUNTY_PRIZES, skim);
    let c = w.character_mut(character)?;
    c.balance += p.amount - skim.map_or(0.0, |s| s.tax);
    let balance = c.balance;
    w.journal_mut().record(
        now,
        REF_BOUNTY_PRIZES,
        BOUNTY_PAYER,
        character,
        system,
        p.amount - skim.map_or(0.0, |s| s.tax),
        balance,
        &description,
    );
    w.mark_dirty();
    w.schedule(
        Duration::ZERO,
        crate::notify::Addressed {
            to: Some(vec![character]),
            inner: Box::new(crate::notify::AccountChange {
                owner_id: character,
                balance,
            }),
        },
    );
    Some(balance)
}

/// `OnBountyAddedToPayout(dataDict)` **[V]**: in
/// `BountyNotificationAdapter.__notifyevents__`
/// (`bountyNotificationAdapter.py:4`), registered by `notificationUI`
/// (`notificationUI.py:47-53`); the handler (`:10-15`) reads
/// `dataDict['amount']`, `['payoutTime']`, `['enemyTypeID']` and
/// `['isModified']` and hands them to `logger.AddBountyMessage`, which
/// prints "{isk} added to next bounty payout", or "... (payment adjusted)"
/// when `isModified` (`uilog.py:365-380`). **[I]** `isModified` is "the
/// dynamic bounty system changed the amount", and `payoutTime` a FILETIME
/// (the logger does not read it).
#[derive(Debug, Clone, PartialEq)]
pub struct OnBountyAddedToPayout {
    pub character: i64,
    pub amount: f64,
    pub payout_time: BlueTime,
    pub enemy_type_id: i64,
    pub is_modified: bool,
}

impl Push for OnBountyAddedToPayout {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnBountyAddedToPayout",
            vec![crate::pyutil::pydict(vec![
                ("amount", PyValue::Real(self.amount)),
                ("payoutTime", PyValue::Int(self.payout_time)),
                ("enemyTypeID", PyValue::Int(self.enemy_type_id)),
                ("isModified", PyValue::Bool(self.is_modified)),
            ])],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

// ---------------------------------------------------------------------------
// Security status (W17 CP50, C6 N8)
// ---------------------------------------------------------------------------

/// The delta kind a character's security status lives in:
/// `char -> status`, one row per character whose status is not the clean
/// default ([`CLEAN`]). Claimed in `delta_kinds::KINDS` (plan §4.2, CA16).
pub const KIND: &str = "secstatus";

/// The dogma attributes a kill's security credit is read from
/// (`dogmaattributes` ids and names, as the type extract carries them)
/// **[V-data]**: `entitySecurityStatusKillBonus` (252) and
/// `entitySecurityMaxGain` (563).
pub mod attr {
    pub const ENTITY_SECURITY_STATUS_KILL_BONUS: i64 = 252;
    pub const ENTITY_SECURITY_MAX_GAIN: i64 = 563;
}

/// A character's status with no row: **`0.0`**, the client's own default
/// (`baseDogmaLocation.GetCharacterSecurityStatus` is `return 0.0`) **[V]**,
/// and what `crimewatch.GetMySecurityStatus` answered before CP50.
pub const CLEAN: f64 = 0.0;

/// What killing one NPC type is worth to its killer's security status.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SecGain {
    /// Attribute 252, `entitySecurityStatusKillBonus`.
    pub bonus: f64,
    /// Attribute 563, `entitySecurityMaxGain`: the status a kill of this
    /// type can raise a character to and no further.
    pub cap: f64,
}

/// **The security credit of NPC type `type_id`**, from `content`'s type
/// table (the bundle's, or `--typedata`), defaults applied. `None` when no table is loaded,
/// the type is not in it, or its bonus is not positive (a type whose kill
/// is worth nothing). The hand row `ANGEL_HIJACKER` carries no 252/563, so
/// with no extract a kill earns no status.
pub fn sec_gain_of(content: &crate::content::Content, type_id: i64) -> Option<SecGain> {
    let bonus =
        crate::typedata::attribute_in(content, type_id, attr::ENTITY_SECURITY_STATUS_KILL_BONUS)?;
    let cap = crate::typedata::attribute_in(content, type_id, attr::ENTITY_SECURITY_MAX_GAIN)?;
    (bonus.is_finite() && bonus > 0.0 && cap.is_finite()).then_some(SecGain { bonus, cap })
}

/// **The status after one kill worth `g`**, from `current`: raised by the
/// type's bonus, never past its cap, and never lowered -- a character
/// already at or above the cap keeps what they have.
///
/// `[A]` **The bonus is added as it stands**, in status points, as plan §6.2
/// CP50 states the rule. CCP's server computes this and the client never
/// does (the only client reader of 252, `parklife/consider.py:90`, tests for
/// its presence) **[V]**, so the formula is not in the client; a
/// percentage-of-the-remainder rule is the live alternative. Falsified by a
/// kill on a reference server whose gain differs from the type's 252.
pub fn raised(current: f64, g: SecGain) -> f64 {
    if current >= g.cap {
        return current;
    }
    (current + g.bonus).min(g.cap)
}

/// **`character`'s security status**: their [`KIND`] row, else [`CLEAN`].
/// What `crimewatch.GetMySecurityStatus` and `GetCharacterSecurityStatus`
/// answer.
pub fn security_status(reg: &Registry, character: i64) -> f64 {
    reg.deltas()
        .of(KIND)
        .get(&vec![character.to_string()])
        .and_then(|v| v.first())
        .and_then(|f| f.parse::<f64>().ok())
        .filter(|s| s.is_finite())
        .unwrap_or(CLEAN)
}

/// **Set `character`'s status to `status`** and tell them: the [`KIND`]
/// row (dropped at [`CLEAN`], so a clean record and no row are one state),
/// and an [`OnSecurityStatusUpdate`] scheduled to the character. The push
/// leaves on the clock, addressed, for the reason [`pay_bounty`] gives: a
/// kill is nobody's turn.
pub fn set_security_status(w: &mut Registry, character: i64, status: f64) {
    let before = security_status(w, character);
    let key = vec![character.to_string()];
    if status == CLEAN {
        w.deltas_mut().del(KIND, &key);
    } else {
        w.deltas_mut().put(KIND, key, vec![status.to_string()]);
    }
    w.schedule(Duration::ZERO, OnSecurityStatusUpdate { character, status });
    // SEC-1a: a flag boundary crossed refreshes the pilot's slim.
    crate::sec_status::on_changed(w, character, before, status);
}

/// **Credit `character` with one kill worth `g`** ([`raised`]), storing and
/// pushing the new status only when it moved. Answers the new status, or
/// `None` when the character was already at the cap.
pub fn credit_kill(w: &mut Registry, character: i64, g: SecGain) -> Option<f64> {
    let before = security_status(w, character);
    let after = raised(before, g);
    if after == before {
        return None;
    }
    set_security_status(w, character, after);
    Some(after)
}

/// **The `kill_hooks::NPC` row** (CA15): the character behind the killing
/// volley -- a pilot, or a drone's owner, as [`pay_bounty`] pays -- gains
/// the dead type's security credit ([`sec_gain_of`]). An NPC killing an
/// NPC credits nobody.
pub fn on_kill(w: &mut Registry, ev: &KillEvent) {
    on_kill_with(w, ev, sec_gain_of);
}

/// [`on_kill`] with the type's credit looked up by `gain`: the type table
/// is the world's, so a test hands its own invented credit here.
fn on_kill_with(
    w: &mut Registry,
    ev: &KillEvent,
    gain: fn(&crate::content::Content, i64) -> Option<SecGain>,
) {
    if ev.npc.is_none() {
        return;
    }
    let Some(killer) = ev.killer else { return };
    let Some(to) = w.pilot(killer).map(|p| p.owner_id) else {
        return;
    };
    if w.character(to).is_none() {
        return;
    }
    if let Some(g) = gain(w.content(), ev.ship_type) {
        let before = security_status(w, to);
        if let Some(after) = credit_kill(w, to, g) {
            // SEC-1b: the gain is a 378 row of the history.
            let owner = ev.npc.map_or(0, |n| n.owner_corp_id);
            crate::sec_status::record_kill_gain(w, to, before, after, owner, ev.system, ev.when);
        }
    }
}

/// `OnSecurityStatusUpdate(newSecurityStatus)` **[V]**: in
/// `crimewatchSvc.__notifyevents__` (`crimewatchSvc.py:69`), and the
/// handler (`:749-750`, two arguments `self, newSecurityStatus`) stores it
/// as `mySecurityStatus`, the cache `GetMySecurityStatus` (`:739-742`)
/// answers from -- so the next reader (the character sheet,
/// `GetSecurityWhereAttacked`) sees the new value with no call.
#[derive(Debug, Clone, PartialEq)]
pub struct OnSecurityStatusUpdate {
    pub character: i64,
    pub status: f64,
}

impl Push for OnSecurityStatusUpdate {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnSecurityStatusUpdate", vec![PyValue::Real(self.status)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::{Damage, Shot};
    use crate::npc::ANGEL_HIJACKER;
    use crate::npc::tests::{ALICE, BOB, due, hijacker_near, ship_of, world};
    use eve_marshal::PyValue;

    /// Kill NPC `npc` with one overwhelming volley from `who`'s ship.
    fn kill(reg: &mut Registry, who: i64, npc: i64) {
        let ship = ship_of(reg, who);
        let t0 = crate::pyutil::filetime_now();
        let victim = reg.roster().get(npc).copied().expect("in space");
        let shot = Shot {
            at: t0,
            attacker_ship_id: ship,
            weapon_item_id: 1,
            weapon_type_id: 3640,
            target_ship_id: npc,
            damage: Damage {
                em: 1.0e6,
                explosive: 1.0e6,
                kinetic: 1.0e6,
                thermal: 1.0e6,
            },
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(reg, shot, who, victim, t0);
        assert!(reg.roster().get(npc).is_none(), "one volley kills it");
    }

    /// Every bounty row in the journal, of either ref.
    fn bounty_rows(reg: &Registry) -> Vec<crate::ledger::Entry> {
        reg.journal()
            .entries()
            .iter()
            .filter(|e| [REF_BOUNTY_PRIZE, REF_BOUNTY_PRIZES].contains(&e.entry_type_id))
            .cloned()
            .collect()
    }

    /// One key of a pushed dict, whichever string type carries the key.
    fn field(d: &PyValue, key: &str) -> PyValue {
        let PyValue::Dict(pairs) = d else {
            panic!("a dict")
        };
        pairs
            .iter()
            .find(|(_, k)| match k {
                PyValue::Bytes(b) => b == key.as_bytes(),
                PyValue::Unicode(s) => s == key,
                _ => false,
            })
            .map(|(v, _)| v.clone())
            .unwrap_or_else(|| panic!("no {key}"))
    }

    /// **E6: kills within one [`BOUNTY_PERIOD`] are paid as one ref-85
    /// row** whose amount is the sum of the per-kill payouts, while the
    /// DBS is drained per kill as before. Until the payout is due the
    /// wallet and journal are untouched and the killer alone is told, per
    /// kill, with `OnBountyAddedToPayout`; at the due time the balance, the
    /// row (wallet the killer, reference the system, the kill tally as its
    /// description) and an `OnAccountChange` to the killer land together.
    #[test]
    fn kills_within_one_period_are_paid_as_one_ref_85_row() {
        let mut reg = world(&[ALICE, BOB]);
        let a = hijacker_near(&mut reg, ALICE, 3_000.0);
        let b = hijacker_near(&mut reg, ALICE, 4_000.0);
        let system = reg.npcs().get(a).expect("spawned").solar_system_id;
        let now = crate::pyutil::filetime_now();
        let pool = crate::econ_dbs::pool(&reg, system);
        crate::econ_dbs::drain(&mut reg, system, pool / 2.0, now);
        let d0 = crate::econ_dbs::multiplier(&reg, system, now);
        let before = reg.character(ALICE).expect("Alice").balance;
        let bob_before = reg.character(BOB).expect("Bob").balance;
        let _ = due(&mut reg);

        kill(&mut reg, ALICE, a);
        let d1 = crate::econ_dbs::multiplier(&reg, system, now);
        assert!(d1 < d0, "the first kill drains the system");
        kill(&mut reg, ALICE, b);
        assert!(
            crate::econ_dbs::multiplier(&reg, system, now) < d1,
            "and so does the second"
        );

        assert_eq!(reg.character(ALICE).expect("Alice").balance, before);
        assert!(bounty_rows(&reg).is_empty(), "nothing booked yet");
        let p = pending(&reg, ALICE, system).expect("a pending payout");
        assert_eq!(p.kills, BTreeMap::from([(ANGEL_HIJACKER.type_id, 2)]));
        assert!(p.amount > 0.0 && p.amount < 2.0 * ANGEL_HIJACKER.bounty);

        let told: Vec<_> = due(&mut reg)
            .into_iter()
            .filter(|(_, n, _)| *n == "OnBountyAddedToPayout")
            .collect();
        assert_eq!(told.len(), 2, "one line per kill");
        let mut sum = 0.0;
        for (to, _, args) in &told {
            assert_eq!(to, &vec![ALICE]);
            assert_eq!(args.len(), 1, "one argument: dataDict");
            let PyValue::Real(amount) = field(&args[0], "amount") else {
                panic!("a float amount")
            };
            sum += amount;
            assert_eq!(field(&args[0], "payoutTime"), PyValue::Int(p.due_at));
            assert_eq!(
                field(&args[0], "enemyTypeID"),
                PyValue::Int(ANGEL_HIJACKER.type_id)
            );
            assert_eq!(field(&args[0], "isModified"), PyValue::Bool(true));
        }
        assert!((sum - p.amount).abs() < 1e-6, "the payout is the sum");

        assert_eq!(flush(&mut reg, p.due_at - 1), 0, "not due yet");
        assert!(bounty_rows(&reg).is_empty());
        assert_eq!(flush(&mut reg, p.due_at), 1);

        let after = reg.character(ALICE).expect("Alice").balance;
        assert_eq!(after, before + p.amount);
        assert_eq!(reg.character(BOB).expect("Bob").balance, bob_before);
        let rows = bounty_rows(&reg);
        assert_eq!(rows.len(), 1, "one row for both kills");
        let row = &rows[0];
        assert_eq!(row.entry_type_id, REF_BOUNTY_PRIZES);
        assert_eq!(row.amount, p.amount);
        assert_eq!(row.wallet(), ALICE);
        assert_eq!(row.owner_id_2, ALICE, "name1 of the client's line");
        assert_eq!(row.owner_id_1, BOUNTY_PAYER);
        assert_eq!(row.reference_id, system, "{{location}}: the system");
        assert_eq!(row.date, p.due_at);
        assert_eq!(row.balance, after);
        assert_eq!(
            row.description,
            format!(
                "{{{DESC_NPC_BOUNTY_LIST}: {{{}: 2}}}}",
                ANGEL_HIJACKER.type_id
            )
        );
        assert!(reg.deltas().of(PENDING_KIND).is_empty(), "the row is gone");
        assert!(reg.take_pending().is_empty(), "nothing waits for a turn");
        let change = due(&mut reg)
            .into_iter()
            .find(|(_, n, _)| *n == "OnAccountChange")
            .expect("the wallet is told");
        assert_eq!(change.0, vec![ALICE]);
        assert_eq!(change.2[2], PyValue::Real(after));
        assert_eq!(flush(&mut reg, p.due_at), 0, "paid once");
    }

    /// A kill after a payout starts the next one, due a whole
    /// [`BOUNTY_PERIOD`] after that kill; and a payout owed to a character
    /// who no longer exists is dropped, not paid to anyone.
    #[test]
    fn a_kill_after_a_payout_starts_the_next_period() {
        let mut reg = world(&[ALICE]);
        let a = hijacker_near(&mut reg, ALICE, 3_000.0);
        let b = hijacker_near(&mut reg, ALICE, 4_000.0);
        let system = reg.npcs().get(a).expect("spawned").solar_system_id;
        let t0 = crate::pyutil::filetime_now();
        pay_bounty(&mut reg, ALICE, a).expect("owed");
        let first = pending(&reg, ALICE, system).expect("pending");
        let period = BOUNTY_PERIOD.as_secs() as i64 * FILETIME_SECOND;
        assert!(first.due_at >= t0 + period);
        assert!(first.due_at <= crate::pyutil::filetime_now() + period);
        assert_eq!(flush(&mut reg, first.due_at), 1);

        let t1 = crate::pyutil::filetime_now();
        pay_bounty(&mut reg, ALICE, b).expect("owed");
        let second = pending(&reg, ALICE, system).expect("a new payout");
        assert_eq!(second.kills.values().sum::<u32>(), 1);
        assert!(second.due_at >= t1 + period);

        let ghost = Pending {
            amount: 1.0,
            due_at: 0,
            kills: BTreeMap::new(),
        };
        reg.deltas_mut().put(
            PENDING_KIND,
            Pending::key(999_999_999, system),
            ghost.to_row(),
        );
        let rows = bounty_rows(&reg).len();
        assert_eq!(flush(&mut reg, 1), 0, "nobody to pay");
        assert_eq!(bounty_rows(&reg).len(), rows);
        assert!(pending(&reg, 999_999_999, system).is_none(), "dropped");
        assert!(pending(&reg, ALICE, system).is_some(), "Alice's not due");
    }

    /// An NPC that kills an NPC is owed nothing, and a dead NPC has no
    /// bounty left to pay.
    #[test]
    fn an_npc_kill_pays_no_one_and_a_bounty_is_paid_once() {
        let mut reg = world(&[ALICE]);
        let a = hijacker_near(&mut reg, ALICE, 3_000.0);
        let b = hijacker_near(&mut reg, ALICE, 4_000.0);
        assert_eq!(pay_bounty(&mut reg, a, b), None);
        assert!(reg.deltas().of(PENDING_KIND).is_empty());
        assert_eq!(
            pay_bounty(&mut reg, ALICE, b),
            Some((ALICE, ANGEL_HIJACKER.bounty))
        );
        reg.npcs_mut().remove(b);
        assert_eq!(pay_bounty(&mut reg, ALICE, b), None);
    }

    /// **A pending payout survives a save**: it is a [`PENDING_KIND`]
    /// row, so a snapshot taken before it is due carries it, and the world
    /// that loads the snapshot pays it.
    #[test]
    fn a_pending_payout_survives_a_save() {
        let mut reg = world(&[ALICE]);
        let a = hijacker_near(&mut reg, ALICE, 3_000.0);
        let system = reg.npcs().get(a).expect("spawned").solar_system_id;
        let (_, owed) = pay_bounty(&mut reg, ALICE, a).expect("owed");
        let p = pending(&reg, ALICE, system).expect("pending");
        let snap = reg.to_snapshot();

        let mut fresh = world(&[]);
        let before = fresh.character(ALICE).expect("Alice").balance;
        fresh.apply_snapshot(snap);
        assert_eq!(pending(&fresh, ALICE, system), Some(p.clone()));
        assert_eq!(flush(&mut fresh, p.due_at), 1);
        assert_eq!(
            fresh.character(ALICE).expect("Alice").balance,
            before + owed
        );
    }

    /// The row round-trips exactly, and the description lists every type
    /// up to [`BOUNTY_LIST_MAX`] and past it keeps the biggest counts and
    /// says it was cut.
    #[test]
    fn the_pending_row_round_trips_and_the_description_truncates() {
        let p = Pending {
            amount: 1_234.567_891_234,
            due_at: 133_000_000_000_000_000,
            kills: BTreeMap::from([(990_100_001, 3), (990_100_002, 1)]),
        };
        assert_eq!(Pending::from_row(&p.to_row()), Some(p.clone()));
        assert_eq!(
            p.description(),
            format!("{{{DESC_NPC_BOUNTY_LIST}: {{990100001: 3, 990100002: 1}}}}")
        );

        let many = Pending {
            kills: (0..=BOUNTY_LIST_MAX as i64)
                .map(|i| (990_200_000 + i, if i == 0 { 1 } else { 2 }))
                .collect(),
            ..p
        };
        let d = many.description();
        assert!(d.ends_with(&format!(", {DESC_NPC_BOUNTY_LIST_TRUNCATED}: 1}}")));
        assert!(!d.contains("990200000:"), "the smallest count is cut");
        assert_eq!(d.matches(": 2").count(), BOUNTY_LIST_MAX);
        assert_eq!(Pending::from_row(&["x".to_owned()]), None);
    }

    /// [`tick`] is the clock stage: it pays what is due by the tick's blue
    /// time and nothing else.
    #[test]
    fn the_stage_pays_what_is_due() {
        let mut reg = world(&[ALICE]);
        let a = hijacker_near(&mut reg, ALICE, 3_000.0);
        let system = reg.npcs().get(a).expect("spawned").solar_system_id;
        pay_bounty(&mut reg, ALICE, a).expect("owed");
        let p = pending(&reg, ALICE, system).expect("pending");
        tick(&mut reg, Instant::now(), p.due_at - 1);
        assert!(bounty_rows(&reg).is_empty());
        tick(&mut reg, Instant::now(), p.due_at);
        assert_eq!(bounty_rows(&reg).len(), 1);
    }

    // -- Security status (CP50) ------------------------------------------

    /// An invented credit: not any type's numbers, only a bonus that takes
    /// more than one kill to reach the cap.
    const GAIN: SecGain = SecGain {
        bonus: 0.25,
        cap: 0.6,
    };

    fn invented(_: &crate::content::Content, _: i64) -> Option<SecGain> {
        Some(GAIN)
    }

    /// A death of an NPC of an invented type, shot by `killer`.
    fn npc_death(killer: Option<i64>) -> KillEvent {
        KillEvent {
            victim_character: crate::npc::MIN_NPC_ID,
            victim_ship: crate::npc::MIN_NPC_ID,
            ship_type: 990_100_001,
            killer,
            system: 30_000_001,
            npc: Some(&ANGEL_HIJACKER),
            when: 0,
            park: Vec::new(),
        }
    }

    /// Every `OnSecurityStatusUpdate` on the clock, as `(audience, status)`.
    fn updates(reg: &mut Registry) -> Vec<(Vec<i64>, PyValue)> {
        due(reg)
            .into_iter()
            .filter(|(_, n, _)| *n == "OnSecurityStatusUpdate")
            .map(|(to, _, args)| {
                assert_eq!(args.len(), 1, "one argument: newSecurityStatus");
                (to, args[0].clone())
            })
            .collect()
    }

    /// **A kill raises security status by the type's bonus, capped by
    /// attribute 563** (plan §6.2 CP50): each kill adds the bonus and
    /// tells the killer alone, the last step stops at the cap exactly, and
    /// a kill at the cap changes and pushes nothing.
    #[test]
    fn a_kill_raises_security_status_by_the_types_bonus_capped() {
        let mut reg = world(&[ALICE, BOB]);
        let _ = due(&mut reg);
        assert_eq!(security_status(&reg, ALICE), CLEAN);

        on_kill_with(&mut reg, &npc_death(Some(ALICE)), invented);
        let first = CLEAN + GAIN.bonus;
        assert_eq!(security_status(&reg, ALICE), first);
        assert_eq!(updates(&mut reg), vec![(vec![ALICE], PyValue::Real(first))]);
        assert_eq!(security_status(&reg, BOB), CLEAN, "Bob did not shoot");

        let mut last = first;
        while last < GAIN.cap {
            on_kill_with(&mut reg, &npc_death(Some(ALICE)), invented);
            let now = security_status(&reg, ALICE);
            assert!(
                now > last && now <= GAIN.cap,
                "each kill raises, up to the cap"
            );
            assert_eq!(now, raised(last, GAIN));
            last = now;
        }
        assert_eq!(last, GAIN.cap, "the last step stops at the cap");
        let _ = due(&mut reg);

        on_kill_with(&mut reg, &npc_death(Some(ALICE)), invented);
        assert_eq!(security_status(&reg, ALICE), GAIN.cap);
        assert!(updates(&mut reg).is_empty(), "at the cap: nothing to tell");
    }

    /// The rule against itself: below the cap a kill adds the bonus or
    /// reaches the cap; at or above it the status is kept, never lowered.
    #[test]
    fn a_status_at_or_above_the_cap_is_never_lowered() {
        assert_eq!(raised(GAIN.cap - 1.0, GAIN), GAIN.cap - 1.0 + GAIN.bonus);
        assert_eq!(raised(GAIN.cap - GAIN.bonus / 2.0, GAIN), GAIN.cap);
        assert_eq!(raised(GAIN.cap, GAIN), GAIN.cap);
        assert_eq!(raised(GAIN.cap + 1.0, GAIN), GAIN.cap + 1.0);

        let mut reg = world(&[ALICE]);
        set_security_status(&mut reg, ALICE, GAIN.cap + 1.0);
        let _ = due(&mut reg);
        assert_eq!(credit_kill(&mut reg, ALICE, GAIN), None);
        assert_eq!(security_status(&reg, ALICE), GAIN.cap + 1.0);
        assert!(updates(&mut reg).is_empty());
    }

    /// An NPC that kills an NPC credits nobody, and neither does a death
    /// with no killer, nor a player's death (no `npc`).
    #[test]
    fn an_npc_kill_or_no_killer_credits_nobody() {
        let mut reg = world(&[ALICE]);
        let rat = hijacker_near(&mut reg, ALICE, 3_000.0);
        let _ = due(&mut reg);
        on_kill_with(&mut reg, &npc_death(Some(rat)), invented);
        on_kill_with(&mut reg, &npc_death(None), invented);
        let mut player = npc_death(Some(ALICE));
        player.npc = None;
        on_kill_with(&mut reg, &player, invented);
        assert!(reg.deltas().of(KIND).is_empty());
        assert!(updates(&mut reg).is_empty());
    }

    /// The status is a [`KIND`] row keyed by the character, read back as
    /// written; [`CLEAN`] drops the row, so clean and absent are one state.
    #[test]
    fn the_status_round_trips_through_its_delta_row() {
        let mut reg = world(&[ALICE]);
        set_security_status(&mut reg, ALICE, -2.75);
        assert_eq!(reg.deltas().of(KIND).len(), 1);
        assert_eq!(
            reg.deltas().of(KIND).get(&vec![ALICE.to_string()]),
            Some(&vec!["-2.75".to_owned()])
        );
        assert_eq!(security_status(&reg, ALICE), -2.75);
        set_security_status(&mut reg, ALICE, CLEAN);
        assert!(reg.deltas().of(KIND).is_empty());
        assert_eq!(security_status(&reg, ALICE), CLEAN);
    }

    /// **The client reads what a kill stored**: `crimewatch`'s
    /// `GetMySecurityStatus` (the caller's) and `GetCharacterSecurityStatus`
    /// (the argument's) answer the row as a float, and a character with no
    /// row still gets the float [`CLEAN`].
    #[test]
    fn crimewatch_answers_the_stored_status() {
        let mut reg = world(&[ALICE]);
        on_kill_with(&mut reg, &npc_death(Some(ALICE)), invented);
        let ask = |reg: &mut Registry, method: &'static str, args: &[PyValue]| {
            reg.dispatch(&crate::service::Request {
                service: "crimewatch",
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(BOB),
            })
            .expect("crimewatch answers")
        };
        let mine = ask(&mut reg, "GetMySecurityStatus", &[]);
        assert_eq!(mine, PyValue::Real(CLEAN), "Bob's own: clean");
        let alice = ask(
            &mut reg,
            "GetCharacterSecurityStatus",
            &[PyValue::Int(ALICE)],
        );
        assert_eq!(alice, PyValue::Real(GAIN.bonus));
        let as_alice = reg
            .dispatch(&crate::service::Request {
                service: "crimewatch",
                method: "GetMySecurityStatus",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            })
            .expect("crimewatch answers");
        assert_eq!(as_alice, PyValue::Real(GAIN.bonus));
    }

    /// [`on_kill`] is a `kill_hooks::NPC` row, and a real kill runs it: a
    /// type with no credit in the type table (the fixture rat has no
    /// 252/563) leaves the killer clean and tells nobody.
    #[test]
    fn on_kill_is_an_npc_kill_hook_and_a_creditless_kill_changes_nothing() {
        assert!(
            crate::kill_hooks::NPC
                .iter()
                .any(|r| r.name == "security_status_on_kill"),
            "registered (CA15)"
        );
        let mut reg = world(&[ALICE]);
        let alice_ship = ship_of(&reg, ALICE);
        let npc = hijacker_near(&mut reg, ALICE, 3_000.0);
        let _ = due(&mut reg);
        let t0 = crate::pyutil::filetime_now();
        let victim = reg.roster().get(npc).copied().expect("in space");
        let shot = Shot {
            at: t0,
            attacker_ship_id: alice_ship,
            weapon_item_id: 1,
            weapon_type_id: 3640,
            target_ship_id: npc,
            damage: Damage {
                em: 1.0e6,
                explosive: 1.0e6,
                kinetic: 1.0e6,
                thermal: 1.0e6,
            },
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(&mut reg, shot, ALICE, victim, t0);
        assert!(reg.roster().get(npc).is_none(), "one volley kills it");
        let kind = reg.npcs().get(npc).map(|n| n.kind.type_id);
        assert!(kind.is_none(), "the record left after the hooks");
        assert_eq!(security_status(&reg, ALICE), CLEAN);
        assert!(updates(&mut reg).is_empty());
    }
}
