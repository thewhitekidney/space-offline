//! **Observation** (R-REL-2, R-RELATIONS §2.3 and §4): what a bot sees
//! through the client's own calls, turned into [`Fact`]s and fed to the
//! R-REL-1 model. Observation and state only: no contact is set, no invitation
//! answered, no price changed here (that is R-REL-3).
//!
//! ## What is observed, and where from
//!
//! | fact | the call | where |
//! |---|---|---|
//! | F1/F2 loss, victory, F13 betrayal | `charMgr.GetRecentShipKillsAndLosses`, every row's `attackers` list (REL-FIX) | [`pass`], [`fold_row`] |
//! | F3 help | the same rows: another pilot on a kill I was on, the victim hostile to me | [`fold_row`] |
//! | F8 trade, F7 scam | `contractProxy.GetContractListForOwner` (finished and failed, as issuer and as acceptor) | [`contract_fact`], [`watch`] |
//! | F10-F12 war, peace, ally | `warRegistry.GetWars` for my corp and alliance | [`war_facts`], [`watch`] |
//! | F14 talk | `mailMgr.SyncMail` headers (the text is never read) | [`crate::mail::heard`], [`inbox`] |
//! | F9 siege, contest | the sov structure rows (`campaignState`) | `sov_bot` |
//! | F5 fleet | the fleet director's roster clock | [`crate::fleet::shared_hours`] |
//!
//! ## How a fact lands
//!
//! The corp matrix still takes today's [`Event::Kill`] for the final blow
//! (`hunt::settle_into`, `corpmind::turn`), now through
//! [`relations::event_deltas`] (so at normal difficulty nothing changed). The
//! facts add what the events lacked: the **personal** level (the holder's own
//! [`Mind::regard`], keyed by the attacker's character), the **assisters**
//! (every attacker with at least [`ASSIST_SHARE`] of the damage), fair-fight
//! respect, betrayal, help, and the group-level kinds (war, peace) into the
//! hall's `alliance_own` store. Each report is folded once per bot
//! ([`Hall::first`]) and its hall-wide parts once per hall.
//!
//! Not observable (no client call shows it to a bot): the rescue F4 (the
//! kill that saved me is not mine), theft F6 (the can-flip needs the
//! overview's suspect flag at the moment of the loot), gifts F15 and the
//! enemy-of-my-enemy F17. They have no emitter here.

use crate::career::Band;
use crate::corpmind::Hall;
use crate::hostile::Class;
use crate::hunt::Ground;
use crate::mind::Mind;
use crate::relations::{self, Ctx, Fact, FactKind, Kill, Level, Temper};
use crate::society::{self, Event};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::group_contacts::Affiliation;
use std::sync::Mutex;

// ---- the observed-set kinds (the first part of the `Hall::first` key) -----------------

/// A kill row folded by one bot (personal level).
pub const KIND_ROW: u8 = 1;
/// A kill row's assisters folded into the matrix (hall-wide, once).
pub const KIND_ASSIST: u8 = 2;
/// A contract settled, seen by one bot.
pub const KIND_CONTRACT: u8 = 3;
/// A mail heard by one bot.
pub const KIND_MAIL: u8 = 4;
/// A war state (`war id * 4 + state`), or an ally (`who` is the ally).
pub const KIND_WAR: u8 = 5;
/// A campaign on a hub (`who` the hub, `id` the campaign's start).
pub const KIND_SIEGE: u8 = 6;
/// REVIEW-FIX: one Talk per sender per bot per game day (`who` the bot, `id`
/// from [`talk_key`]).
pub const KIND_TALK_DAY: u8 = 7;

// ---- policy [A] ------------------------------------------------------------------------

/// The least share of the damage that makes a non-final attacker a party to
/// the loss **[A]**. Falsified by `[rel]` showing a lone-wolf's victim blaming
/// the whole gang for a few stray shots (too low) or a gank gang's corps
/// walking free (too high).
pub const ASSIST_SHARE: f64 = 0.25;
/// How often a bot syncs its inbox, game seconds **[A]** (R-RELATIONS §4:
/// 30 minutes).
pub const MAIL_EVERY: u64 = 30 * 60;
/// How many new mails one sync turns into facts **[A]**.
pub const MAIL_MAX: usize = 20;
/// How often a bot reads its contracts and wars, game seconds **[A]**.
pub const WATCH_EVERY: u64 = 10 * 60;
/// How many settled contracts one watch reads **[A]**: the client's page.
pub const CONTRACTS_READ: i64 = 25;

// ---- reading a row ---------------------------------------------------------------------

pub fn val<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    let PyValue::Dict(entries) = crate::attributes(v) else {
        return None;
    };
    entries
        .iter()
        .find(|(_, k)| match k {
            PyValue::Bytes(b) => b == key.as_bytes(),
            PyValue::Unicode(s) => s == key,
            _ => false,
        })
        .map(|(value, _)| value)
}

/// An integer attribute of a `KeyVal`.
pub fn int(v: &PyValue, key: &str) -> Option<i64> {
    match val(v, key)? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

/// A numeric attribute of a `KeyVal`.
pub fn real(v: &PyValue, key: &str) -> Option<f64> {
    match val(v, key)? {
        PyValue::Real(r) => Some(*r),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

/// An attribute that is a list.
pub fn list<'a>(v: &'a PyValue, key: &str) -> Option<&'a [PyValue]> {
    match val(v, key)? {
        PyValue::List(l) | PyValue::Tuple(l) => Some(l),
        _ => None,
    }
}

/// A text attribute.
pub fn text(v: &PyValue, key: &str) -> Option<String> {
    match val(v, key)? {
        PyValue::Unicode(s) => Some(s.clone()),
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

/// One player attacker on a killmail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub who: Affiliation,
    pub ship: Option<i64>,
    pub damage: i64,
}

/// A killmail row about the bot, with every attacker.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub id: i64,
    pub system: i64,
    pub victim: Affiliation,
    pub victim_ship: Option<i64>,
    pub isk: f64,
    /// The player attackers, the final blow first when it is a player's.
    pub hits: Vec<Hit>,
    /// The final blow was a player's (not an NPC's).
    pub final_is_player: bool,
}

fn affiliation(v: &PyValue, c: &str, corp: &str, al: &str) -> Option<Affiliation> {
    let character_id = int(v, c).filter(|c| *c > 0)?;
    Some(Affiliation {
        character_id,
        corporation_id: int(v, corp).unwrap_or(0),
        alliance_id: int(v, al).filter(|a| *a > 0),
    })
}

/// The rows of a `GetRecentShipKillsAndLosses` answer that have a player
/// victim and at least one player attacker. A row with no `attackers` list
/// (an older server) reads as its final blow alone.
pub fn seen_of(rows: &[PyValue]) -> Vec<Seen> {
    rows.iter()
        .filter_map(|r| {
            let id = int(r, "killID")?;
            let victim = affiliation(
                r,
                "victimCharacterID",
                "victimCorporationID",
                "victimAllianceID",
            )?;
            let mut hits: Vec<(bool, Hit)> = Vec::new();
            if let Some(list) = list(r, "attackers") {
                for a in list {
                    if let Some(who) = affiliation(a, "characterID", "corporationID", "allianceID")
                    {
                        hits.push((
                            int(a, "finalBlow") == Some(1),
                            Hit {
                                who,
                                ship: int(a, "shipTypeID"),
                                damage: int(a, "damageDone").unwrap_or(0),
                            },
                        ));
                    }
                }
            }
            if hits.is_empty()
                && let Some(who) = affiliation(
                    r,
                    "finalCharacterID",
                    "finalCorporationID",
                    "finalAllianceID",
                )
            {
                hits.push((
                    true,
                    Hit {
                        who,
                        ship: int(r, "finalShipTypeID"),
                        damage: int(r, "finalDamageDone").unwrap_or(0),
                    },
                ));
            }
            // The final blow first, the rest as listed.
            hits.sort_by_key(|(f, _)| !*f);
            let final_is_player = hits.first().is_some_and(|(f, _)| *f);
            let hits: Vec<Hit> = hits
                .into_iter()
                .map(|(_, h)| h)
                .filter(|h| h.who.character_id != victim.character_id)
                .collect();
            if hits.is_empty() {
                return None;
            }
            Some(Seen {
                id,
                system: int(r, "solarSystemID").unwrap_or(0),
                victim,
                victim_ship: int(r, "victimShipTypeID"),
                isk: real(r, "iskLost").unwrap_or(0.0),
                hits,
                final_is_player,
            })
        })
        .collect()
}

// ---- the fairness inputs --------------------------------------------------------------

/// The band a system is in: wormhole space, else its security class.
pub fn band_of(ground: Option<&Ground>, system: i64) -> Band {
    use eve_server::celestial::{SecurityClass, WORMHOLE_SYSTEMS};
    if WORMHOLE_SYSTEMS.contains(&system) {
        return Band::Wormhole;
    }
    match ground
        .and_then(|g| g.security(system))
        .map(SecurityClass::from_level)
    {
        Some(SecurityClass::Zero) => Band::Null,
        Some(SecurityClass::Low) => Band::Low,
        Some(SecurityClass::High | SecurityClass::Safe) => Band::High,
        // Unknown: the middle of the road (no high-sec surcharge).
        None => Band::Low,
    }
}

/// The classifier's input for a row: gang size, summed power, pod, band.
/// Without a `ground` every hull is [`Class::Unknown`] and no victim a pod.
pub fn kill_of(s: &Seen, ground: Option<&Ground>) -> Kill {
    let class = |t: Option<i64>| ground.map_or(Class::Unknown, |g| g.class_of(t));
    let victim = class(s.victim_ship);
    Kill {
        gang: s.hits.len(),
        attacker_power: s
            .hits
            .iter()
            .map(|h| crate::hunt::power(class(h.ship)))
            .sum(),
        victim_power: crate::hunt::power(victim),
        pod: victim == Class::Capsule,
        band: band_of(ground, s.system),
        // Whether the victim shot first is not on a killmail.
        victim_aggressor: false,
    }
}

/// One counted attacker's facts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitFacts {
    pub hit: Hit,
    pub final_blow: bool,
    /// F1 (or F13 when `allied_at_time`): the victim's loss to this attacker.
    pub loss: Fact,
    /// F2: this attacker's victory.
    pub victory: Fact,
}

/// **A killmail's facts** (pure): for the final blow and every attacker
/// with at least [`ASSIST_SHARE`] of the damage, the victim's loss and the
/// attacker's victory. `allied(attacker, victim)` is the victim's books'
/// answer (betrayal, F13).
pub fn kill_facts(
    s: &Seen,
    ground: Option<&Ground>,
    allied: &dyn Fn(&Affiliation, &Affiliation) -> bool,
    at: u64,
) -> Vec<HitFacts> {
    let kill = kill_of(s, ground);
    let total: i64 = s.hits.iter().map(|h| h.damage.max(0)).sum();
    s.hits
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            let final_blow = i == 0 && s.final_is_player;
            let share = if total > 0 {
                h.damage.max(0) as f64 / total as f64
            } else {
                1.0 / s.hits.len() as f64
            };
            if !final_blow && share < ASSIST_SHARE {
                return None;
            }
            let ctx = Ctx {
                kill,
                allied_at_time: allied(&h.who, &s.victim),
                ..Ctx::default()
            };
            let fact = |kind| Fact {
                kind,
                actor: h.who,
                target: s.victim,
                isk: s.isk,
                ctx,
                at,
            };
            Some(HitFacts {
                hit: *h,
                final_blow,
                loss: fact(FactKind::Loss),
                victory: fact(FactKind::Victory),
            })
        })
        .collect()
}

// ---- applying -------------------------------------------------------------------------

/// Apply the personal deltas of `ds` held by `me` to its mind.
pub fn apply_personal(
    mind: &Mutex<Mind>,
    me: i64,
    ds: &[relations::Delta],
    now: u64,
    day: u64,
    t: &Temper,
) -> u32 {
    let mut m = mind.lock().unwrap_or_else(|p| p.into_inner());
    let half = relations::personal_half_days(m.traits.get(crate::mind::Trait::Loyalty), t);
    let mut n = 0;
    for d in ds
        .iter()
        .filter(|d| d.level == Level::Personal && d.holder == me)
    {
        relations::regard_add(&mut m.regard, d.subject, d.d, now, day, half);
        n += 1;
    }
    n
}

/// What [`fold_row`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folded {
    /// CHATTER-3: the fights this report newly showed the bot was in, as
    /// `(the other pilot, the bot won)`: it won when it was an attacker.
    pub fights: Vec<(i64, bool)>,
    /// Personal regards moved.
    pub personal: u32,
    /// Hall deltas applied (assisters, help).
    pub hall: u32,
}

/// **Fold one killmail** the bot is on into the society, once per bot and
/// report. `me` is the bot's character id.
pub fn fold_row(hall: &Hall, mind: &Mutex<Mind>, me: i64, s: &Seen, now: u64) -> Folded {
    let mut out = Folded::default();
    let involved = s.victim.character_id == me || s.hits.iter().any(|h| h.who.character_id == me);
    if !involved {
        return out;
    }
    let (t, day, ground, facts, helps) = {
        let mut st = hall.lock();
        if !st.first(KIND_ROW, me, s.id) {
            return out;
        }
        let ground = st.ground.clone();
        let facts = kill_facts(
            s,
            ground.as_deref(),
            // The victim's books: is the attacker one of its own?
            &|attacker, victim| !st.hostile_to(*victim, *attacker),
            now,
        );
        // F3: I was an attacker, another pilot was too, and the victim is
        // no friend of mine.
        let mine = s
            .hits
            .iter()
            .find(|h| h.who.character_id == me)
            .map(|h| h.who);
        let helps: Vec<Fact> = match mine {
            Some(me_aff) if st.hostile_to(me_aff, s.victim) => s
                .hits
                .iter()
                .filter(|h| h.who.character_id != me)
                .map(|h| Fact {
                    kind: FactKind::Help,
                    actor: h.who,
                    target: me_aff,
                    isk: s.isk,
                    ctx: Ctx::default(),
                    at: now,
                })
                .collect(),
            _ => Vec::new(),
        };
        (st.temper, st.day, ground, facts, helps)
    };
    let _ = ground;
    if s.victim.character_id == me {
        if let Some(h) = s.hits.first() {
            out.fights.push((h.who.character_id, false));
        }
    } else {
        out.fights.push((s.victim.character_id, true));
    }
    let mut hall_deltas = Vec::new();
    for hf in &facts {
        let loss = relations::deltas(&hf.loss, &t);
        out.personal += apply_personal(mind, me, &loss, now, day, &t);
        // The final blow's corp level is the `Event::Kill` of `pass` (the
        // adapter), every other attacker's is the spilled delta.
        if !hf.final_blow {
            hall_deltas.extend(loss.into_iter().filter(|d| d.level != Level::Personal));
        }
        let win = relations::deltas(&hf.victory, &t);
        out.personal += apply_personal(mind, me, &win, now, day, &t);
    }
    for f in &helps {
        let ds = relations::deltas(f, &t);
        out.personal += apply_personal(mind, me, &ds, now, day, &t);
        hall_deltas.extend(ds.into_iter().filter(|d| d.level != Level::Personal));
    }
    // R-REL-5: a blue that shot a blue flips the victim's stance at once and
    // the bloc takes the spill (once per attacker per report, hall-wide).
    for hf in &facts {
        if relations::is_betrayal(
            hf.loss.ctx.allied_at_time,
            s.isk,
            hf.loss.ctx.kill.victim_aggressor,
        ) {
            let mut st = hall.lock();
            if st.first(crate::coalition::KIND_BETRAY, s.id, hf.hit.who.character_id) {
                st.betray(&s.victim, &hf.hit.who, now);
            }
        }
    }
    if !hall_deltas.is_empty() {
        let mut st = hall.lock();
        // The assisters' corp level once per report, whoever reads it.
        if st.first(KIND_ASSIST, 0, s.id) {
            out.hall = hall_deltas.len() as u32;
            st.apply_deltas(&hall_deltas);
        }
    }
    out
}

/// **The kill pass** (pure over the rows): fold every row, and put the
/// final blow into the matrix as an [`Event::Kill`] when the bot is neither
/// the victim nor the final blow (an assist) -- the other two cases have their
/// emitters (`corpmind::turn`, `hunt::settle_into`).
pub fn pass(hall: &Hall, mind: &Mutex<Mind>, me: i64, rows: &[Seen], now: u64) -> Folded {
    let mut out = Folded::default();
    for s in rows {
        if s.victim.character_id != me
            && s.final_is_player
            && s.hits.first().is_some_and(|h| h.who.character_id != me)
        {
            let (victim, killer) = (
                society::group_of(&s.victim),
                society::group_of(&s.hits[0].who),
            );
            if victim != killer {
                hall.record_kill(
                    s.id,
                    &Event::Kill {
                        victim,
                        killer,
                        isk: s.isk,
                    },
                );
            }
        }
        let f = fold_row(hall, mind, me, s, now);
        out.personal += f.personal;
        out.hall += f.hall;
        out.fights.extend(f.fights);
    }
    out
}

// ---- contracts (F7, F8) ----------------------------------------------------------------

/// `conStatusFinished`, `conStatusFailed` (the server's `contracts.rs`).
const STATUS_FINISHED: i64 = 4;
const STATUS_FAILED: i64 = 7;

/// A settled contract row as a fact for `me` (pure): finished is a trade with
/// the other party (F8), a courier the acceptor let fail is a scam against
/// the issuer (F7). `other` is the counterparty's affiliation.
pub fn contract_fact(
    row: &PyValue,
    me: &Affiliation,
    other: impl Fn(i64) -> Affiliation,
    at: u64,
) -> Option<(i64, Fact)> {
    let id = int(row, "contractID")?;
    let (issuer, acceptor) = (int(row, "issuerID")?, int(row, "acceptorID").unwrap_or(0));
    let isk = real(row, "price").unwrap_or(0.0) + real(row, "reward").unwrap_or(0.0);
    let (kind, them) = match int(row, "status")? {
        STATUS_FINISHED if issuer == me.character_id && acceptor > 0 => (FactKind::Trade, acceptor),
        STATUS_FINISHED if acceptor == me.character_id && issuer > 0 => (FactKind::Trade, issuer),
        STATUS_FAILED if issuer == me.character_id && acceptor > 0 => (FactKind::Scam, acceptor),
        _ => return None,
    };
    if them == me.character_id {
        return None;
    }
    let fact = match kind {
        // The acceptor let it fail: the actor is the acceptor.
        FactKind::Scam => Fact {
            kind,
            actor: other(them),
            target: *me,
            isk,
            ctx: Ctx::default(),
            at,
        },
        _ => Fact {
            kind,
            actor: *me,
            target: other(them),
            isk,
            ctx: Ctx::default(),
            at,
        },
    };
    Some((id, fact))
}

/// Read the settled contracts of `me` (as issuer, then as acceptor) and
/// fold the new ones. Returns the act lines.
fn contracts(
    conn: &mut Conn,
    hall: &Hall,
    mind: &Mutex<Mind>,
    me: Affiliation,
    now: u64,
) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for issued_by_me in [true, false] {
        let out = match conn.call(
            "contractProxy",
            "GetContractListForOwner",
            vec![
                PyValue::Int(me.character_id),
                PyValue::Int(STATUS_FINISHED),
                PyValue::None,
                // IssuedBy is False, IssuedTo True [V contractPanels].
                PyValue::Bool(!issued_by_me),
            ],
        ) {
            Ok(o) => o,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => continue,
        };
        let Some(v) = out.value() else { continue };
        let Some(rows) = list(&v, "contracts") else {
            continue;
        };
        for row in rows.iter().take(CONTRACTS_READ as usize) {
            let Some(id) = int(row, "contractID") else {
                continue;
            };
            // Cheap checks first: only a row that makes a fact costs the
            // counterparty lookup.
            let probe = contract_fact(row, &me, bare, now);
            let Some((_, probe)) = probe else { continue };
            if !hall.lock().first(KIND_CONTRACT, me.character_id, id) {
                continue;
            }
            let them_id = if probe.actor.character_id == me.character_id {
                probe.target.character_id
            } else {
                probe.actor.character_id
            };
            let them = match conn.affiliation_of(them_id) {
                Ok(a) => Affiliation {
                    character_id: them_id,
                    corporation_id: a.corp,
                    alliance_id: a.alliance.filter(|x| *x > 0),
                },
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(_) => bare(them_id),
            };
            let Some((_, mut fact)) = contract_fact(row, &me, |_| them, now) else {
                continue;
            };
            if fact.kind == FactKind::Trade {
                // REVIEW-FIX: one pair earns a day's worth of Trade regard at most.
                fact.isk = hall
                    .lock()
                    .trade_allow(me.character_id, them_id, now, fact.isk);
                if fact.isk <= 0.0 {
                    continue;
                }
            }
            let (t, day) = {
                let st = hall.lock();
                (st.temper, st.day)
            };
            let ds = relations::deltas(&fact, &t);
            apply_personal(mind, me.character_id, &ds, now, day, &t);
            let mut st = hall.lock();
            if fact.kind == FactKind::Trade {
                // The matrix hears the trade as the event it always was.
                st.record(&Event::Trade {
                    a: society::group_of(&me),
                    b: society::group_of(&them),
                    isk: fact.isk,
                });
            } else {
                st.apply_deltas(&ds);
            }
            lines.push(format!(
                "contract {id}: {:?} with {}",
                fact.kind, them.character_id
            ));
        }
    }
    Ok(lines)
}

fn bare(character_id: i64) -> Affiliation {
    Affiliation {
        character_id,
        corporation_id: 0,
        alliance_id: None,
    }
}

// ---- wars (F10, F11, F12) --------------------------------------------------------------

/// An owner id as an affiliation: an alliance, else a corp.
pub fn owner_affiliation(owner: i64) -> Affiliation {
    if society::PLAYER_ALLIANCES.contains(&owner) {
        Affiliation {
            character_id: 0,
            corporation_id: 0,
            alliance_id: Some(owner),
        }
    } else {
        Affiliation {
            character_id: 0,
            corporation_id: owner,
            alliance_id: None,
        }
    }
}

/// **A war row's facts** (pure) for a bot of `mine` (its corp and alliance
/// ids): `(key, fact)` pairs, `key` for [`KIND_WAR`] as `(who, id)`.
/// - declared on me: [`FactKind::WarOn`] (F10), once;
/// - finished (not retracted by the declarer alone is not told apart: a war
///   that ended is peace, F11), once;
/// - an ally listed on a war that is mine: [`FactKind::WarAlly`] (F12),
///   once per ally.
pub fn war_facts(war: &PyValue, mine: &[i64], at: u64) -> Vec<((i64, i64), Fact)> {
    let Some(id) = int(war, "warID") else {
        return Vec::new();
    };
    let (by, against) = (
        int(war, "declaredByID").unwrap_or(0),
        int(war, "againstID").unwrap_or(0),
    );
    if by == 0 || against == 0 {
        return Vec::new();
    }
    let (a, b) = (owner_affiliation(by), owner_affiliation(against));
    let mk = |kind, actor, target| Fact {
        kind,
        actor,
        target,
        isk: 0.0,
        ctx: Ctx::default(),
        at,
    };
    let mut out = Vec::new();
    let finished = matches!(val(war, "timeFinished"), Some(PyValue::Int(_)));
    if mine.contains(&against) {
        out.push(((0, id * 4), mk(FactKind::WarOn, a, b)));
    }
    if finished && (mine.contains(&against) || mine.contains(&by)) {
        out.push(((0, id * 4 + 1), mk(FactKind::Peace, a, b)));
    }
    if (mine.contains(&against) || mine.contains(&by))
        && let Some(PyValue::Dict(allies)) = val(war, "allies")
    {
        for (_, k) in allies {
            if let PyValue::Int(ally) = k {
                let side = if mine.contains(&by) { a } else { b };
                out.push((
                    (*ally, id * 4 + 2),
                    mk(FactKind::WarAlly, owner_affiliation(*ally), side),
                ));
            }
        }
    }
    out
}

fn wars(conn: &mut Conn, hall: &Hall, me: Affiliation, now: u64) -> Result<Vec<String>> {
    let mut mine = vec![me.corporation_id];
    mine.extend(me.alliance_id);
    let mut lines = Vec::new();
    for owner in mine.clone() {
        if owner <= 0 {
            continue;
        }
        let out = match conn.call("warRegistry", "GetWars", vec![PyValue::Int(owner)]) {
            Ok(o) => o,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => continue,
        };
        let Some(PyValue::Dict(entries)) = out.value() else {
            continue;
        };
        for (war, _) in &entries {
            // R-REL-5: the hall keeps the wars (hunters read them).
            if let Some(row) = crate::coalition::WarRow::from_value(war, now) {
                hall.lock().note_war(row);
            }
            for ((who, id), fact) in war_facts(war, &mine, now) {
                let mut st = hall.lock();
                if !st.first(KIND_WAR, who, id) {
                    continue;
                }
                let ds = relations::deltas(&fact, &st.temper);
                st.apply_deltas(&ds);
                lines.push(format!("war {}: {:?}", id / 4, fact.kind));
            }
        }
    }
    Ok(lines)
}

// ---- mail (F14) ------------------------------------------------------------------------

/// The dedupe id of one sender's Talk on one game day.
pub fn talk_key(sender: i64, day_index: u64) -> i64 {
    sender
        .wrapping_mul(4096)
        .wrapping_add((day_index & 0xfff) as i64)
}

/// REVIEW-FIX: **may this mail count as a Talk?** `None` when `sender`
/// already talked to `me` today (one Talk per sender per game day, so a
/// mailbox flood buys nothing); else `Some(accepted)`: the reply bonus only
/// when the reply reads as one *and* this bot really sent `sender` an offer
/// ([`HallState::offers`]).
pub fn talk_gate(
    hall: &Hall,
    me: i64,
    sender: i64,
    reply_to_offer: bool,
    now: u64,
) -> Option<bool> {
    let mut st = hall.lock();
    let day_index = now / st.day.max(1);
    if !st.first(KIND_TALK_DAY, me, talk_key(sender, day_index)) {
        return None;
    }
    Some(reply_to_offer && st.offers.contains(&(me, sender)))
}

/// Sync the inbox and fold each new mail as a [`FactKind::Talk`].
fn inbox(
    conn: &mut Conn,
    hall: &Hall,
    mind: &Mutex<Mind>,
    me: Affiliation,
    now: u64,
) -> Result<Vec<String>> {
    let heard = crate::mail::sync_inbox(conn, me.character_id)?;
    let mut lines = Vec::new();
    for h in heard.into_iter().take(MAIL_MAX) {
        if !hall.lock().first(KIND_MAIL, me.character_id, h.id) {
            continue;
        }
        let Some(accepted) = talk_gate(hall, me.character_id, h.sender, h.reply_to_offer, now)
        else {
            continue;
        };
        let from = match conn.affiliation_of(h.sender) {
            Ok(a) => Affiliation {
                character_id: h.sender,
                corporation_id: a.corp,
                alliance_id: a.alliance.filter(|x| *x > 0),
            },
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => bare(h.sender),
        };
        let fact = Fact {
            kind: FactKind::Talk,
            actor: from,
            target: me,
            isk: 0.0,
            ctx: Ctx {
                accepted_offer: accepted,
                ..Ctx::default()
            },
            at: now,
        };
        let (t, day) = {
            let st = hall.lock();
            (st.temper, st.day)
        };
        let ds = relations::deltas(&fact, &t);
        apply_personal(mind, me.character_id, &ds, now, day, &t);
        hall.lock().apply_deltas(&ds);
        lines.push(format!("mail {}: talk with {}", h.id, h.sender));
    }
    Ok(lines)
}

// ---- the turn --------------------------------------------------------------------------

/// **One observation turn** for the bot on `conn` (called from
/// `corpmind::turn`): the killmails now, the contracts and wars every
/// [`WATCH_EVERY`], the inbox every [`MAIL_EVERY`]. A refusal is skipped; only a
/// broken connection is an error. Returns act lines.
pub fn turn(conn: &mut Conn, hall: &Hall, mind: &Mutex<Mind>, now: u64) -> Result<Vec<String>> {
    let Some(me_id) = conn.session().char_id() else {
        return Ok(Vec::new());
    };
    let me = Affiliation {
        character_id: me_id,
        corporation_id: conn.session().get_i64("corpid").unwrap_or(0),
        alliance_id: conn.session().get_i64("allianceid").filter(|a| *a > 0),
    };
    let mut lines = Vec::new();
    // The killmails.
    let rows = match conn.call(
        "charMgr",
        "GetRecentShipKillsAndLosses",
        vec![PyValue::Int(crate::corpmind::KILLS_READ), PyValue::None],
    ) {
        Ok(o) => o,
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(_) => return Ok(lines),
    };
    if let Some(PyValue::List(rows)) = rows.value() {
        let f = pass(hall, mind, me_id, &seen_of(&rows), now);
        crate::chatter::note_fights(me_id, &f.fights);
        if f.personal + f.hall > 0 {
            lines.push(format!(
                "rel: {} personal regards, {} hall deltas from killmails",
                f.personal, f.hall
            ));
        }
    }
    let (watch, mail) = {
        let mut st = hall.lock();
        let due = |m: &mut std::collections::BTreeMap<i64, u64>, every: u64| {
            let go = m
                .get(&me_id)
                .is_none_or(|t| now.saturating_sub(*t) >= every);
            if go {
                m.insert(me_id, now);
            }
            go
        };
        let watch = due(&mut st.watch_tried, WATCH_EVERY);
        let mail = due(&mut st.mail_tried, MAIL_EVERY);
        (watch, mail)
    };
    if watch {
        lines.extend(contracts(conn, hall, mind, me, now)?);
        lines.extend(wars(conn, hall, me, now)?);
    }
    if mail {
        lines.extend(inbox(conn, hall, mind, me, now)?);
    }
    Ok(lines)
}

// ---- tests ----------------------------------------------------------------------------

#[cfg(test)]
mod tests;
