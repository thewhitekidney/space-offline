//! **Security status** (SEC-1a, SEC-1b, SEC-1d; `docs/lanes/sec-1.md`):
//! what a pilot's status is made of, what moves it, and what the rest of the
//! game reads off it.
//!
//! The status itself is stored by `npc::bounty` (delta kind `secstatus`,
//! `OnSecurityStatusUpdate`); this module **extends** that home and
//! duplicates none of it. A write goes through
//! `npc::bounty::set_security_status`, whose one added line calls
//! [`on_changed`] (the slim refresh). Everything below is pure except the
//! functions that take a `Registry`.
//!
//! # What it owns
//!
//! - **The flags the overview draws** (SEC-1a). `state.CheckOutlaw` is
//!   `IsOutlawStatus(slimItem.securityStatus)`, `state.CheckDangerous` is
//!   `-0.1 > slimItem.securityStatus >= -5` on a slim with a character, and
//!   `CanAttackFreely` reads the outlaw rule off the same key **[V]**. Our
//!   ship slims carried no `securityStatus`, so neither flag ever fired.
//!   [`slim_extras`] is a `slim_extra::Source` and [`on_changed`] sends an
//!   `OnSlimItemChange` when a pilot's status crosses a flag boundary.
//! - **The losses** (SEC-1b). [`penalty`] is the pure rule CONCORD-1 and
//!   CW-1 can call; [`on_crime`] applies it for one [`Crime`] (the seam,
//!   `docs/lanes/sec-1.md` §2), tells the pilot with the client's own
//!   message keys, and writes the history row.
//! - **The history** (`crimewatch.GetSecurityStatusTransactions`): rows in
//!   the capped delta kind [`TX_KIND`], read by `securityStatusPanel`.
//!   Gains from rats (event 378) are folded into one row per bounty period.
//! - **What a status does to a route** (SEC-1d, as far as it needs no
//!   CONCORD): [`band_attacked_in`] is `crimewatchSvc.GetSecurityWhereAttacked`
//!   ported, [`police_attack_here`] asks it of a system, and
//!   [`refuse_dock`] keeps an outlaw's ship out of high-sec stations.
//!
//! The tag office is `security_office.rs`.
//!
//! # Marks
//!
//! `[V]` read in our client (build 3619879); `[I]` inferred; `[A]` assumed,
//! with the falsifier next to the constant. **No `[A]` value is written into
//! a test**: tests name the constant.

use crate::damage::BlueTime;
use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::time::Duration;

// ---------------------------------------------------------------------------
// The client's constants
// ---------------------------------------------------------------------------

/// `appConst.characterSecurityStatusMin` / `Max` **[V]**
/// (`crimewatch/const.pyc`).
pub const MIN_STATUS: f64 = -10.0;
pub const MAX_STATUS: f64 = 10.0;

/// `eveCfg.IsOutlawStatus`: `round(status, 1) <= -5` **[V]**.
pub const OUTLAW_AT: f64 = -5.0;

/// `state.CheckDangerous`: `-0.1 > status >= -5` **[V]**.
pub const DANGEROUS_BELOW: f64 = -0.1;

/// `appLogConst.eventSecStatus*` **[V]**: the `eventTypeID` of a history row.
pub const EVENT_KILL_PIRATE_NPC: i64 = 378;
pub const EVENT_ILLEGAL_AGGRESSION: i64 = 379;
pub const EVENT_GM_MODIFICATION: i64 = 380;
pub const EVENT_HAND_IN_TAGS: i64 = 385;
pub const EVENT_GM_ROLLBACK: i64 = 386;

/// The thresholds of `crimewatchSvc.GetSecurityWhereAttacked` **[V]**: a
/// pilot at or below the first number of a pair is attacked by faction
/// police in systems of at least the second. Lowest status first, as the
/// client tests them.
pub const ATTACK_BANDS: [(f64, f64); 6] = [
    (-4.5, 0.5),
    (-4.0, 0.6),
    (-3.5, 0.7),
    (-3.0, 0.8),
    (-2.5, 0.9),
    (-2.0, 1.0),
];

/// CONCORD's corporation id, the payee of the tag fee and the police owner
/// of the client (`appConst.ownerCONCORD`) **[V]**.
pub const OWNER_CONCORD: i64 = 1_000_125;

// ---------------------------------------------------------------------------
// The rules we assume
// ---------------------------------------------------------------------------

/// **[A]** The base fraction of a ship-aggression loss: `0.025 x security x
/// (1 + (victim - offender) / 100)` of the distance to -10. Public lore of
/// the 2012 Crimewatch rules and EvEJS's base. Falsified by one retail
/// `SecurityStatusAggressionChange` with a known status before.
pub const SHIP_AGGRESSION_M: f64 = 0.025;

/// **[A]** The same for aggression at a capsule: ten times the ship's.
pub const CAPSULE_AGGRESSION_M: f64 = 0.25;

/// **[A]** Attacking CONCORD, a station or a gate: a flat fraction, with no
/// security or victim factor. Falsified as above.
pub const FACILITY_M: f64 = 0.025;

/// **[A]** Killing CONCORD: the same flat fraction. The client names the
/// message (`SecurityStatusPoliceKillChange`) and gives no number.
pub const POLICE_KILL_M: f64 = 0.025;

/// **[A]** The security used for scaling is clamped to at least this.
pub const MIN_SECURITY: f64 = 0.1;

/// **[A]** The security used for a system the universe table lacks (a
/// server with no bundle). Falsified by a loss in an unlisted system that
/// the owner thinks too large or small.
pub const FALLBACK_SECURITY: f64 = 0.5;

/// **[A]** One penalty per (offender, victim) per this long; repeats are
/// swallowed. EvEJS has the same 60 s. Falsified by retail penalties on
/// every cycle of a gun.
pub const SUPPRESS: Duration = Duration::from_secs(60);

/// The delta kind of the history: `char seq -> eventTypeID eventDate
/// modification newValue otherOwnerID otherTypeID locationID start`, where
/// `seq` is zero-padded so the rows sort in order, an absent
/// `modification` or `start` is `-`, and `start` is the status before the
/// first kill a folded row holds. Claimed in `delta_kinds::KINDS`.
pub const TX_KIND: &str = "sectx";

/// **[A]** How many history rows a character keeps.
pub const TX_KEEP: usize = 50;

const FILETIME_SECOND: i64 = 10_000_000;

// ---------------------------------------------------------------------------
// The flags
// ---------------------------------------------------------------------------

/// Python 2's `round(x, 1)`: half away from zero.
pub fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// `eveCfg.IsOutlawStatus` **[V]**: anyone may shoot an outlaw.
pub fn is_outlaw(status: f64) -> bool {
    round1(status) <= OUTLAW_AT
}

/// `state.CheckDangerous` **[V]** (the "has a character" half is the slim's).
pub fn is_dangerous(status: f64) -> bool {
    (OUTLAW_AT..DANGEROUS_BELOW).contains(&status)
}

/// The two booleans the overview reads off a status. A slim only needs
/// refreshing when this changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    pub outlaw: bool,
    pub dangerous: bool,
}

/// [`Flags`] of `status`.
pub fn flags(status: f64) -> Flags {
    Flags {
        outlaw: is_outlaw(status),
        dangerous: is_dangerous(status),
    }
}

/// **`crimewatchSvc.GetSecurityWhereAttacked`** **[V]**: the lowest system
/// security in which faction police attack a pilot of `status`, or `None`
/// above -2.0. This is C5's "never enter high-sec at a band where police
/// attack".
pub fn band_attacked_in(status: f64) -> Option<f64> {
    ATTACK_BANDS
        .iter()
        .find(|(at_or_below, _)| status <= *at_or_below)
        .map(|(_, sec)| *sec)
}

/// Whether faction police attack a pilot of `status` in a system of
/// `system_security`. **[I]** The client compares with the system's rounded
/// security; the comparison is done in tenths so `0.5` and the stored
/// `0.4999` agree.
pub fn police_attack_here(status: f64, system_security: f64) -> bool {
    band_attacked_in(status)
        .is_some_and(|min| (system_security * 10.0).round() >= (min * 10.0).round())
}

/// Whether `after` newly puts a pilot inside the net of faction police in a
/// system of `system_security`: police did not attack `before` and do now.
/// This is the "status change that crosses a band, while in space" trigger
/// of the faction police (CONCORD-1c), as a pure rule.
pub fn crossed_into_police_net(before: f64, after: f64, system_security: f64) -> bool {
    !police_attack_here(before, system_security) && police_attack_here(after, system_security)
}

/// **Who faction police would come for, and where** (the hook CONCORD-1c
/// spawns a squad from): an in-space pilot whose status puts them inside
/// the net of the system they fly in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoliceCall {
    pub character: i64,
    pub system: i64,
    /// The system's security, from the universe table.
    pub security: f64,
    /// The faction that owns the system (`owner_faction` of its row), whose
    /// police answer; `None` for a system no faction owns.
    pub faction: Option<i64>,
}

/// The [`PoliceCall`] for `character` right now, if there is one. A pilot in
/// a station, a system the tables lack, or one below the band is not called
/// on.
pub fn police_call(w: &Registry, character: i64) -> Option<PoliceCall> {
    let p = w.roster().get(character)?;
    w.character(character)?;
    let system = w
        .content()
        .get::<crate::celestial::Universe>()
        .system(p.solar_system_id)?;
    let status = crate::npc::bounty::security_status(w, character);
    police_attack_here(status, system.security).then_some(PoliceCall {
        character,
        system: system.id,
        security: system.security,
        faction: system.owner_faction,
    })
}

// ---------------------------------------------------------------------------
// The penalty
// ---------------------------------------------------------------------------

/// What a criminal act was, for the penalty and the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Aggression at a capsuleer's ship.
    ShipAggression,
    /// Aggression at a capsuleer's capsule.
    CapsuleAggression,
    /// Remote assistance to a criminal.
    Assistance,
    /// An offensive act at CONCORD.
    AttackConcord,
    /// An offensive act at a station or a stargate.
    AttackFacility,
    /// Destroying CONCORD.
    KillConcord,
}

impl Act {
    /// The `UserError` key the client names for this act
    /// (`findings/user-errors.tsv`) **[V]**; each takes one `penalty`
    /// argument, a float printed to 4 decimals.
    pub fn message(self) -> &'static str {
        match self {
            Act::ShipAggression | Act::CapsuleAggression => "SecurityStatusAggressionChange",
            Act::Assistance => "SecurityStatusAssistanceChange",
            Act::AttackConcord | Act::AttackFacility => "SecurityStatusPoliceAggressionChange",
            Act::KillConcord => "SecurityStatusPoliceKillChange",
        }
    }
}

/// **The fraction `m`** of the distance to -10 an act costs. `security` is
/// the system's, `victim` and `offender` the two statuses. Never above 1,
/// never negative.
///
/// **[A]** `base x max(security, MIN_SECURITY) x (1 + (victim - offender) /
/// 100)` for aggression and assistance: it falls when the victim is worse
/// than the offender and rises when the victim is better, so shooting an
/// outlaw is cheap and shooting a saint is not. See [`SHIP_AGGRESSION_M`].
pub fn fraction(act: Act, victim: f64, offender: f64, security: f64) -> f64 {
    let scaled = |base: f64| {
        let factor = (1.0 + (victim - offender) / 100.0).max(0.0);
        base * security.max(MIN_SECURITY) * factor
    };
    let m = match act {
        Act::ShipAggression | Act::Assistance => scaled(SHIP_AGGRESSION_M),
        Act::CapsuleAggression => scaled(CAPSULE_AGGRESSION_M),
        Act::AttackConcord | Act::AttackFacility => FACILITY_M,
        Act::KillConcord => POLICE_KILL_M,
    };
    m.clamp(0.0, 1.0)
}

/// **The status after a loss of fraction `m`**: `SS - (SS + 10) x m`, so a
/// loss shrinks toward -10 and a pilot at -10 loses nothing. **[A]**, the
/// direction the spec chose over EvEJS's `(10 - SS)`.
pub fn after_loss(offender: f64, m: f64) -> f64 {
    (offender - (offender + 10.0) * m).clamp(MIN_STATUS, MAX_STATUS)
}

/// **The change in status points** one act costs, `<= 0`. This is the
/// `penalty` argument of the client's message and the pure function CW-1 and
/// CONCORD-1 call.
pub fn penalty(act: Act, victim: f64, offender: f64, security: f64) -> f64 {
    after_loss(offender, fraction(act, victim, offender, security)) - offender
}

/// **The status after a gain of fraction `m`** of the distance to +10.
pub fn after_gain(current: f64, m: f64) -> f64 {
    (current + (MAX_STATUS - current) * m).clamp(MIN_STATUS, MAX_STATUS)
}

/// The history's "Change" for a move from `before` to `after`: the signed
/// fraction of the distance it covered, which the panel prints times 100 as
/// a percentage **[V]**. A loss is a fraction of the distance to -10 (the
/// `m` of [`after_loss`]), a gain of the distance to +10.
pub fn modification(before: f64, after: f64) -> Option<f64> {
    if after == before {
        return None;
    }
    if after < before {
        let span = before - MIN_STATUS;
        (span > 0.0).then(|| -((before - after) / span))
    } else {
        let span = MAX_STATUS - before;
        (span > 0.0).then(|| (after - before) / span)
    }
}

// ---------------------------------------------------------------------------
// One crime, as this module takes it
// ---------------------------------------------------------------------------

/// **One criminal act** CW-1 classified, as SEC-1 takes it. The adapter from
/// `crimewatch::CrimeEvent` is in `docs/lanes/sec-1.md` §2 (coordinator): a
/// `Criminal` outcome only, `Attack` at a capsule is
/// [`Act::CapsuleAggression`], `Assist` is [`Act::Assistance`], a theft or a
/// suspect act costs nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crime {
    pub offender: i64,
    /// The capsuleer wronged, if any. `None` for CONCORD and facilities.
    pub victim: Option<i64>,
    pub act: Act,
    pub system: i64,
    /// The victim's ship type, for the history row (0 when unknown).
    pub victim_type: i64,
    pub at: BlueTime,
}

/// A system's security, from the universe table.
pub fn system_security(w: &Registry, system: i64) -> Option<f64> {
    w.content()
        .get::<crate::celestial::Universe>()
        .system(system)
        .map(|s| s.security)
}

/// **Apply one crime**: the loss, the story and the message. Answers the
/// change in points when anything moved, `None` for a swallowed repeat
/// ([`SUPPRESS`]) or a pilot with nothing left to lose.
pub fn on_crime(w: &mut Registry, c: &Crime) -> Option<f64> {
    w.character(c.offender)?;
    if let Some(victim) = c.victim
        && suppressed(w, c.offender, victim, c.at)
    {
        return None;
    }
    let before = crate::npc::bounty::security_status(w, c.offender);
    let victim_status = c
        .victim
        .map_or(before, |v| crate::npc::bounty::security_status(w, v));
    let security = system_security(w, c.system).unwrap_or(FALLBACK_SECURITY);
    let m = fraction(c.act, victim_status, before, security);
    let after = after_loss(before, m);
    if after == before {
        return None;
    }
    crate::npc::bounty::set_security_status(w, c.offender, after);
    record(
        w,
        c.offender,
        Tx {
            event: EVENT_ILLEGAL_AGGRESSION,
            date: c.at,
            modification: modification(before, after),
            new_value: after,
            other_owner: c.victim.unwrap_or(OWNER_CONCORD),
            other_type: c.victim_type,
            location: c.system,
            start: None,
        },
    );
    let change = after - before;
    w.schedule(
        Duration::ZERO,
        RemoteMessage {
            to: c.offender,
            key: c.act.message(),
            args: vec![("penalty", PyValue::Real(change))],
        },
    );
    Some(change)
}

/// **The `crimewatch::ON_CRIME` row** (CW-1's seam, `docs/lanes/sec-1.md`
/// §2): a **criminal** act costs status; a suspect act or a theft costs
/// none. An attack at a capsule is the capsule's price.
pub fn on_crime_event(w: &mut Registry, ev: &crate::crimewatch::CrimeEvent) {
    use crate::crimewatch::{ActKind, CrimeKind};
    if ev.kind != CrimeKind::Criminal {
        return;
    }
    let victim_type = ev
        .victim
        .and_then(|v| w.pilot(v))
        .map_or(0, |p| p.ship_type_id);
    let act = match ev.act {
        ActKind::Attack if victim_type == crate::damage::CAPSULE_TYPE_ID => Act::CapsuleAggression,
        ActKind::Attack => Act::ShipAggression,
        ActKind::Assist => Act::Assistance,
        ActKind::Theft => return,
    };
    on_crime(
        w,
        &Crime {
            offender: ev.offender,
            victim: ev.victim,
            act,
            system: ev.solar_system_id,
            victim_type,
            at: ev.at,
        },
    );
}

/// Whether `offender` was already penalised for `victim` inside
/// [`SUPPRESS`] of `at`: the newest 379 row naming the victim.
fn suppressed(w: &Registry, offender: i64, victim: i64, at: BlueTime) -> bool {
    let window = SUPPRESS.as_secs() as i64 * FILETIME_SECOND;
    transactions(w, offender)
        .iter()
        .filter(|t| t.event == EVENT_ILLEGAL_AGGRESSION && t.other_owner == victim)
        .any(|t| at >= t.date && at - t.date < window)
}

/// `OnRemoteMessage(key, {arg: value})`: the client's `gameui` handler
/// shows a named message with its arguments. **[A]** that the pushed form
/// shows what the raised `UserError` of the same key shows
/// (`shutdown::ShutdownMessage` makes the same assumption); falsified by a
/// pilot who loses status and sees nothing, or an error.
#[derive(Debug, Clone)]
pub struct RemoteMessage {
    pub to: i64,
    pub key: &'static str,
    pub args: Vec<(&'static str, PyValue)>,
}

impl Push for RemoteMessage {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnRemoteMessage",
            vec![
                crate::pyutil::bstr(self.key),
                crate::pyutil::pydict(self.args.clone()),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---------------------------------------------------------------------------
// The history
// ---------------------------------------------------------------------------

/// One history row, as `securityStatusPanel.LoadPanel` reads it **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct Tx {
    pub event: i64,
    pub date: BlueTime,
    /// A fraction; the panel prints it times 100. `None` prints nothing.
    pub modification: Option<f64>,
    pub new_value: f64,
    pub other_owner: i64,
    pub other_type: i64,
    pub location: i64,
    /// The status before the first kill a folded 378 row holds.
    pub start: Option<f64>,
}

fn opt_field(v: Option<f64>) -> String {
    v.map_or_else(|| "-".to_owned(), |f| f.to_string())
}

fn parse_opt(s: Option<&String>) -> Option<Option<f64>> {
    match s.map(String::as_str) {
        None | Some("-") => Some(None),
        Some(t) => t.parse::<f64>().ok().filter(|f| f.is_finite()).map(Some),
    }
}

impl Tx {
    fn to_row(&self) -> Vec<String> {
        vec![
            self.event.to_string(),
            self.date.to_string(),
            opt_field(self.modification),
            self.new_value.to_string(),
            self.other_owner.to_string(),
            self.other_type.to_string(),
            self.location.to_string(),
            opt_field(self.start),
        ]
    }

    fn from_row(v: &[String]) -> Option<Tx> {
        Some(Tx {
            event: v.first()?.parse().ok()?,
            date: v.get(1)?.parse().ok()?,
            modification: parse_opt(v.get(2))?,
            new_value: v.get(3)?.parse::<f64>().ok().filter(|f| f.is_finite())?,
            other_owner: v.get(4)?.parse().ok()?,
            other_type: v.get(5)?.parse().ok()?,
            location: v.get(6)?.parse().ok()?,
            start: parse_opt(v.get(7))?,
        })
    }

    /// The row as the client reads it: a `KeyVal`, one attribute per field.
    pub fn to_pyvalue(&self) -> PyValue {
        crate::pyutil::keyval(vec![
            ("eventTypeID", PyValue::Int(self.event)),
            ("eventDate", PyValue::Int(self.date)),
            (
                "modification",
                self.modification.map_or(PyValue::None, PyValue::Real),
            ),
            ("newValue", PyValue::Real(self.new_value)),
            ("otherOwnerID", PyValue::Int(self.other_owner)),
            ("otherTypeID", PyValue::Int(self.other_type)),
            ("locationID", PyValue::Int(self.location)),
        ])
    }
}

fn tx_key(character: i64, seq: u64) -> Vec<String> {
    vec![character.to_string(), format!("{seq:010}")]
}

/// `(seq, row)` of every history row of `character`, oldest first.
fn tx_rows(w: &Registry, character: i64) -> Vec<(u64, Vec<String>)> {
    let c = character.to_string();
    w.deltas()
        .of(TX_KIND)
        .range(vec![c.clone()]..)
        .take_while(|(k, _)| k.first() == Some(&c))
        .filter_map(|(k, v)| Some((k.get(1)?.parse::<u64>().ok()?, v.clone())))
        .collect()
}

/// **`character`'s history, newest first** (a long list that scrolls: the
/// freshest change is the one a player looks for). **[A]** that the order
/// does not matter to the panel, which sorts on its header.
pub fn transactions(w: &Registry, character: i64) -> Vec<Tx> {
    let mut out: Vec<Tx> = tx_rows(w, character)
        .iter()
        .filter_map(|(_, v)| Tx::from_row(v))
        .collect();
    out.reverse();
    out
}

/// Append `tx` to `character`'s history and drop the oldest past
/// [`TX_KEEP`].
pub fn record(w: &mut Registry, character: i64, tx: Tx) {
    let rows = tx_rows(w, character);
    let next = rows.last().map_or(0, |(s, _)| s + 1);
    w.deltas_mut()
        .put(TX_KIND, tx_key(character, next), tx.to_row());
    let excess = (rows.len() + 1).saturating_sub(TX_KEEP);
    for (seq, _) in rows.iter().take(excess) {
        w.deltas_mut().del(TX_KIND, &tx_key(character, *seq));
    }
    w.mark_dirty();
}

/// `crimewatch.GetSecurityStatusTransactions()`: the history as a list of
/// `KeyVal`s, newest first.
pub fn transactions_value(w: &Registry, character: i64) -> PyValue {
    PyValue::List(
        transactions(w, character)
            .iter()
            .map(Tx::to_pyvalue)
            .collect(),
    )
}

/// **A rat kill's gain, as a 378 row** (the `npc::bounty::on_kill` line).
/// Gains inside one `BOUNTY_PERIOD` of the row's date fold into it when they
/// come from the same owner, so a ratter's history does not churn: the row
/// keeps its first date and start, and takes the new value.
pub fn record_kill_gain(
    w: &mut Registry,
    character: i64,
    before: f64,
    after: f64,
    npc_owner: i64,
    system: i64,
    now: BlueTime,
) {
    let window = crate::npc::bounty::BOUNTY_PERIOD.as_secs() as i64 * FILETIME_SECOND;
    let rows = tx_rows(w, character);
    if let Some((seq, row)) = rows.last()
        && let Some(last) = Tx::from_row(row)
        && last.event == EVENT_KILL_PIRATE_NPC
        && last.other_owner == npc_owner
        && now >= last.date
        && now - last.date < window
    {
        let start = last.start.unwrap_or(before);
        let folded = Tx {
            modification: modification(start, after),
            new_value: after,
            start: Some(start),
            ..last
        };
        w.deltas_mut()
            .put(TX_KIND, tx_key(character, *seq), folded.to_row());
        w.mark_dirty();
        return;
    }
    record(
        w,
        character,
        Tx {
            event: EVENT_KILL_PIRATE_NPC,
            date: now,
            modification: modification(before, after),
            new_value: after,
            other_owner: npc_owner,
            other_type: 0,
            location: system,
            start: Some(before),
        },
    );
}

// ---------------------------------------------------------------------------
// The slim
// ---------------------------------------------------------------------------

/// **A slim-extra source** (`slim_extra::SOURCES`, coordinator row): every
/// character's ship or capsule carries `securityStatus`, a float
/// **[V key name: `state.CheckOutlaw`, `CheckDangerous`;
/// `crimewatchSvc.GetSlimItemDataForCharID` builds the same attribute]**.
/// An NPC's ball carries none.
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    ids.iter()
        .filter_map(|id| {
            let p = w.roster().by_ship(*id)?;
            w.character(p.character_id)?;
            Some((
                *id,
                vec![(
                    "securityStatus",
                    PyValue::Real(crate::npc::bounty::security_status(w, p.character_id)),
                )],
            ))
        })
        .collect()
}

/// **The refresh** (`npc::bounty::set_security_status` calls it after the
/// store write): when `character`'s status crossed a flag boundary
/// ([`flags`]), everyone in their system gets an `OnSlimItemChange` with the
/// ship's new slim. The client stores the new slim as it is
/// (`michelle.py:2006`) and scatters the change, which re-colours the
/// overview. A pilot docked at the time has nothing to refresh: the next
/// `AddBalls2` or state carries the status.
pub fn on_changed(w: &mut Registry, character: i64, before: f64, after: f64) {
    if flags(before) == flags(after) {
        return;
    }
    let Some(p) = w.roster().get(character).copied() else {
        return;
    };
    let Some(c) = w.character(character) else {
        return;
    };
    let (type_id, corp) = (c.ship_type_id, c.corporation_id);
    let to = crate::dungeon::audience(w, p.solar_system_id);
    if to.is_empty() {
        return;
    }
    let mut pairs: Vec<(&'static str, PyValue)> = vec![
        ("itemID", PyValue::Int(p.ship_id)),
        ("typeID", PyValue::Int(type_id)),
        ("ownerID", PyValue::Int(character)),
        ("charID", PyValue::Int(character)),
        ("corpID", PyValue::Int(corp)),
    ];
    let fitted = crate::modules::slim_modules(w.store(), p.ship_id);
    if !fitted.is_empty() {
        pairs.push((
            "modules",
            PyValue::List(
                fitted
                    .iter()
                    .map(|m| {
                        PyValue::Tuple(vec![
                            PyValue::Int(m.item_id),
                            PyValue::Int(m.type_id),
                            PyValue::Int(m.flag_id),
                        ])
                    })
                    .collect(),
            ),
        ));
    }
    for (id, extra) in crate::slim_extra::for_slims(w, &[p.ship_id]) {
        if id == p.ship_id {
            pairs.extend(extra);
        }
    }
    let stamp = w.next_destiny_stamp();
    w.schedule(
        Duration::ZERO,
        ShipSlimChange {
            to,
            stamp,
            item_id: p.ship_id,
            pairs,
        },
    );
}

/// `OnSlimItemChange(itemID, newSlim)` in a `DoDestinyUpdate`, for a pilot's
/// ship; the sibling of `destiny::SlimItemChange` (a wreck's) and
/// `hacking::CanSlimChange` (a can's). In `nonDestinyCriticalFunctions`, so
/// it is applied without resynchronising the park **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct ShipSlimChange {
    pub to: Vec<i64>,
    pub stamp: u32,
    pub item_id: i64,
    pub pairs: Vec<(&'static str, PyValue)>,
}

impl Push for ShipSlimChange {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("OnSlimItemChange"),
                PyValue::Tuple(vec![
                    PyValue::Int(self.item_id),
                    crate::pyutil::instance("foo.SlimItem", self.pairs.clone()),
                ]),
            ]),
        ]);
        (
            "DoDestinyUpdate",
            vec![PyValue::List(vec![entry]), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.item_id])
    }
}

// ---------------------------------------------------------------------------
// The outlaw at a station door
// ---------------------------------------------------------------------------

/// **[A]** High-sec stations refuse the **ship** of an outlaw (status
/// `<= -5`, [`is_outlaw`]); the capsule is let in. The refusal is a
/// `UserError` with the key `CantDockInHiSecOutlaw`, the `fsd` modal "Cannot
/// Dock" (row 7803, title 639211) whose body 639210 carries the retail words
/// ("Security status low ... you are only allowed to dock in your capsule")
/// and takes no arguments **[V-data]**. That an outlaw's ship is refused in
/// every high-sec station is the assumption. Falsified by the owner, or by a
/// retail outlaw who docks a ship in high-sec.
pub const DENIED_CRIMINAL: &str = "CantDockInHiSecOutlaw";

/// Whether `station`'s system is high-sec or better. `false` for a station
/// the tables do not place (a sandbox station), so nothing is refused where
/// nothing is known.
fn station_is_high_sec(w: &Registry, station: i64) -> bool {
    let Some(r) = w.station_ref(station) else {
        return false;
    };
    w.content()
        .get::<crate::celestial::Universe>()
        .security_class(r.solar_system_id)
        .is_some_and(|c| c >= crate::celestial::SecurityClass::High)
}

/// **The outlaw's dock refusal**: `Err` when `actor` is an outlaw flying a
/// ship (not the capsule) and `station` is a high-sec station. Call it
/// before the dock (the `CmdDock` arm, coordinator line).
pub fn refuse_dock(
    w: &Registry,
    actor: Option<i64>,
    station: Option<i64>,
) -> Result<(), ServiceError> {
    let (Some(actor), Some(station)) = (actor, station) else {
        return Ok(());
    };
    let Some(c) = w.character(actor) else {
        return Ok(());
    };
    if c.ship_type_id == crate::damage::CAPSULE_TYPE_ID {
        return Ok(());
    }
    if is_outlaw(crate::npc::bounty::security_status(w, actor)) && station_is_high_sec(w, station) {
        return Err(ServiceError::Raise(eve_proto::user_error(
            DENIED_CRIMINAL,
            vec![],
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The GM console
// ---------------------------------------------------------------------------

/// `/secstatus [characterID] <value>`: set a status by hand (history event
/// 380, `GmModification`, whose panel line is "set by a slash command").
/// A `slash::COMMANDS` row (coordinator).
pub fn slash(
    w: &mut Registry,
    actor: Option<i64>,
    words: &[&str],
) -> Result<PyValue, ServiceError> {
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    let (who, value) = match words {
        [v] => (actor, *v),
        [c, v] => (c.parse::<i64>().ok(), *v),
        _ => return say("usage: /secstatus [characterID] <value>"),
    };
    let Some(who) = who.filter(|id| w.character(*id).is_some()) else {
        return say("no such character");
    };
    let Ok(value) = value.parse::<f64>() else {
        return say(&format!("not a number: {value}"));
    };
    if !value.is_finite() || !(MIN_STATUS..=MAX_STATUS).contains(&value) {
        return say("the status is between -10 and 10");
    }
    let before = crate::npc::bounty::security_status(w, who);
    crate::npc::bounty::set_security_status(w, who, value);
    record(
        w,
        who,
        Tx {
            event: EVENT_GM_MODIFICATION,
            date: crate::pyutil::filetime_now(),
            modification: None,
            new_value: value,
            other_owner: 0,
            other_type: 0,
            location: 0,
            start: None,
        },
    );
    say(&format!("security status {before:.4} -> {value:.4}"))
}

#[cfg(test)]
mod tests;
