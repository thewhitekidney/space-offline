//! **The bot half of wars** (C8-f, R-REL-5): one owner's war pass on its
//! CEO's own connection, the client's own calls ([`crate::coalition`] decides,
//! this module reads and acts).
//!
//! | what | the call |
//! |---|---|
//! | the owner's wars | `warRegistry.GetWars(ownerID)` -- the rows the client's own wars tab reads |
//! | open wars | `warsInfoMgr.GetWarsRequiringAssistance()` |
//! | offers to answer | `warRegistry.GetNegotiations()` (type 0 ally, 2 surrender) |
//! | pending mutual invites | `mutualWarInviteMgr.GetPendingInvitesForSession()` |
//! | the fee's wallet | `account.GetCashBalance(1, accountKey=1000)` -- the corp's Master Wallet |
//! | declare | `structureDirectory.GetValidWarHQs(owner)`, then `corpRegistry` or `allianceRegistry` `.DeclareWarAgainst(againstID, warHQ)` |
//! | mutual war | `mutualWarInviteMgr.SendInviteByPlayer(toOwnerID)` / `RespondToInviteByPlayer(from, accepts)` |
//! | allies | `warRegistry.SetOpenForAllies(warID, true)`, `CreateWarAllyOffer(warID, 0, defenderID, message)`, `AcceptAllyNegotiation(negID)` |
//! | surrender | `warRegistry.CreateSurrenderNegotiation(warID, isk, message)`, `AcceptSurrender(negID)` |
//! | let the bill lapse | `billMgr.SendAutomaticPaySettings({owner: {billType: bool, 'divisionID': 1000}})` |
//!
//! The war is the server's (`docs/lanes/c8-a.md`): a declaration needs the
//! declaring director docked in a high-sec station (the war HQ stand-in is
//! anchored there), so a CEO elsewhere tries again next pass.

use crate::coalition::{self, Cand, Neg, WarAct, WarCtx, WarRow};
use crate::corpmind::Hall;
use crate::mind::Mind;
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use std::sync::{Arc, Mutex};

/// `billTypeID` of a war bill in the auto-pay panel **[V `corpBillsAutoPaySettingsPanel`]**:
/// the server's own `war_deal::BILL_TYPE_WAR`.
const BILL_TYPE_WAR: i64 = eve_server::war_deal::BILL_TYPE_WAR;
/// The Master Wallet's account key.
const MASTER_KEY: i64 = 1000;

fn soft<T>(r: Result<T>) -> Result<std::result::Result<T, String>> {
    match r {
        Ok(v) => Ok(Ok(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(Err(e.to_string())),
    }
}

fn bound(
    conn: &mut Conn,
    service: &str,
    owner: i64,
    method: &str,
    args: Vec<PyValue>,
) -> Result<crate::Answer> {
    let bind = PyValue::Tuple(vec![PyValue::Int(owner), PyValue::Int(1)]);
    conn.moniker_call(service, bind, method, args, vec![])
}

fn wallet(conn: &mut Conn) -> Result<f64> {
    let a = soft(conn.call_kw(
        "account",
        "GetCashBalance",
        vec![PyValue::Int(1)],
        vec![(
            PyValue::Int(MASTER_KEY),
            PyValue::Bytes(b"accountKey".to_vec()),
        )],
    ))?;
    Ok(a.ok()
        .and_then(|o| match o.value() {
            Some(PyValue::Real(r)) => Some(r),
            Some(PyValue::Int(n)) => Some(n as f64),
            _ => None,
        })
        .unwrap_or(0.0))
}

/// The `KeyVal`s of a list answer.
fn items(v: Option<PyValue>) -> Vec<PyValue> {
    match v {
        Some(PyValue::List(l) | PyValue::Tuple(l)) => l,
        _ => Vec::new(),
    }
}

/// **The owner's wars** read from the registry into the hall; the rows.
fn read_wars(conn: &mut Conn, hall: &Hall, owner: i64, now: u64) -> Result<Vec<WarRow>> {
    let a = match soft(conn.call("warRegistry", "GetWars", vec![PyValue::Int(owner)]))? {
        Ok(a) => a,
        Err(_) => return Ok(Vec::new()),
    };
    let rows: Vec<WarRow> = match a.value() {
        Some(PyValue::Dict(entries)) => entries
            .iter()
            .filter_map(|(w, _)| WarRow::from_value(w, now))
            .collect(),
        _ => Vec::new(),
    };
    let mut st = hall.lock();
    for r in &rows {
        st.note_war(r.clone());
    }
    // The rows as the hall now holds them (the first sighting's time kept).
    Ok(rows
        .iter()
        .filter_map(|r| st.coal.wars.get(&r.id).cloned())
        .collect())
}

fn read_negs(conn: &mut Conn, owner: i64) -> Result<Vec<Neg>> {
    let a = match soft(conn.call("warRegistry", "GetNegotiations", vec![]))? {
        Ok(a) => a,
        Err(_) => return Ok(Vec::new()),
    };
    let _ = owner;
    Ok(items(a.value())
        .iter()
        .filter_map(|n| {
            Some(Neg {
                id: crate::observe::int(n, "warNegotiationID")?,
                war: crate::observe::int(n, "warID")?,
                kind: crate::observe::int(n, "warNegotiationTypeID")?,
                from: crate::observe::int(n, "ownerID1")?,
                to: crate::observe::int(n, "ownerID2")?,
                state: crate::observe::int(n, "negotiationState")?,
                isk: crate::observe::real(n, "iskValue").unwrap_or(0.0),
            })
        })
        .collect())
}

fn read_invites(conn: &mut Conn, owner: i64) -> Result<Vec<i64>> {
    let a = match soft(conn.call("mutualWarInviteMgr", "GetPendingInvitesForSession", vec![]))? {
        Ok(a) => a,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(items(a.value())
        .iter()
        .filter(|i| crate::observe::int(i, "toOwnerID") == Some(owner))
        .filter_map(|i| crate::observe::int(i, "fromOwnerID"))
        .collect())
}

fn read_open(conn: &mut Conn, now: u64) -> Result<Vec<WarRow>> {
    let a = match soft(conn.call("warsInfoMgr", "GetWarsRequiringAssistance", vec![]))? {
        Ok(a) => a,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(items(a.value())
        .iter()
        .filter_map(|w| WarRow::from_value(w, now))
        .collect())
}

/// The war HQ the declaration names: the first of `GetValidWarHQs(owner)`.
fn hq_for(conn: &mut Conn, owner: i64) -> Result<Option<i64>> {
    let a = match soft(conn.call(
        "structureDirectory",
        "GetValidWarHQs",
        vec![PyValue::Int(owner)],
    ))? {
        Ok(a) => a,
        Err(_) => return Ok(None),
    };
    Ok(items(a.value())
        .iter()
        .find_map(|h| crate::observe::int(h, "structureID")))
}

/// **One owner's war pass** ([`crate::corpmind::Plan::Wars`]): read, decide
/// ([`coalition::war_acts`]), act. A refusal is a line; only a broken
/// connection is an error.
#[allow(clippy::too_many_arguments)] // the plan's whole context, flat for the caller
pub fn run(
    conn: &mut Conn,
    hall: &Hall,
    mind: &Arc<Mutex<Mind>>,
    me: i64,
    corp: i64,
    owner: i64,
    alliance: bool,
    now: u64,
) -> Result<Vec<String>> {
    let _ = mind;
    let mut out = Vec::new();
    let (enabled, humans_ok, day) = {
        let st = hall.lock();
        (st.coal.enabled, st.coal.humans, st.day)
    };
    let ft = eve_server::pyutil::filetime_now();
    let wars = read_wars(conn, hall, owner, now)?;
    if wars.is_empty() && !enabled {
        return Ok(out);
    }
    let negs = read_negs(conn, owner)?;
    let invites = if enabled || !wars.is_empty() {
        read_invites(conn, owner)?
    } else {
        Vec::new()
    };
    let open = read_open(conn, now)?;
    let money = wallet(conn)?;
    let (aggression, cands, has_mates, autopay) = {
        let st = hall.lock();
        let cands: Vec<Cand> = st
            .war_candidates(owner, now)
            .into_iter()
            .filter(|c| {
                !st.coal
                    .declare_tried
                    .get(&(owner, c.group))
                    .is_some_and(|t| {
                        now.saturating_sub(*t) < (coalition::DECLARE_RETRY_DAYS * day as f64) as u64
                    })
            })
            .collect();
        (
            st.ceo_aggression(corp),
            cands,
            st.has_mates(owner),
            st.coal.autopay.get(&owner).copied().unwrap_or(true),
        )
    };
    let acts = {
        let st = hall.lock();
        let view = |g: i64| st.owner_view(owner, g, now);
        let blue = |g: i64| st.owner_blue(owner, g);
        let lost = |g: i64| st.war_ledger(owner, g).0;
        let destroyed = |g: i64| st.war_ledger(owner, g).1;
        // A bloc member joins a war after the difficulty's lag from when a
        // bot first read it.
        let lag = st.temper.lag_hours();
        let lag_ok = |w: &WarRow| {
            let age_h = now.saturating_sub(w.seen) as f64 / day.max(1) as f64 * 24.0;
            age_h >= lag
        };
        coalition::war_acts(&WarCtx {
            owner,
            ft,
            now,
            day,
            aggression,
            wallet: money,
            enabled,
            humans_ok,
            wars: &wars,
            open_wars: &open,
            negs: &negs,
            invites: &invites,
            cands: &cands,
            view: &view,
            blue: &blue,
            lost: &lost,
            destroyed: &destroyed,
            autopay,
            has_mates,
            lag_ok: &lag_ok,
            offered: &st.coal.offered,
        })
    };
    for act in acts {
        out.extend(perform(conn, hall, me, owner, alliance, now, &act)?);
    }
    Ok(out)
}

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

/// Do one act; its line(s).
fn perform(
    conn: &mut Conn,
    hall: &Hall,
    me: i64,
    owner: i64,
    alliance: bool,
    now: u64,
    act: &WarAct,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let line = |st: &mut crate::corpmind::HallState, s: String| {
        st.act(s.clone());
        s
    };
    match act {
        WarAct::Declare(target) => {
            hall.lock().coal.declare_tried.insert((owner, *target), now);
            let Some(hq) = hq_for(conn, owner)? else {
                out.push(format!(
                    "war {owner} -> {target}: no war HQ here (a director must dock in high sec)"
                ));
                return Ok(out);
            };
            let args = vec![PyValue::Int(*target), PyValue::Int(hq)];
            let a = if alliance {
                bound(conn, "allianceRegistry", owner, "DeclareWarAgainst", args)?
            } else {
                bound(conn, "corpRegistry", owner, "DeclareWarAgainst", args)?
            };
            match a.refusal() {
                Some(key) => out.push(format!("war {owner} -> {target}: refused {key}")),
                None => {
                    let mut st = hall.lock();
                    out.push(line(
                        &mut st,
                        format!("war: {owner} declares on {target} (CEO {me})"),
                    ));
                    drop(st);
                    out.extend(tell(conn, hall, owner, *target, true)?);
                }
            }
        }
        WarAct::Mutual(target) => {
            hall.lock().coal.declare_tried.insert((owner, *target), now);
            let a = conn.call(
                "mutualWarInviteMgr",
                "SendInviteByPlayer",
                vec![PyValue::Int(*target)],
            )?;
            match a.refusal() {
                Some(key) => out.push(format!("war {owner} ~ {target}: invite refused {key}")),
                None => {
                    let mut st = hall.lock();
                    out.push(line(
                        &mut st,
                        format!("war: {owner} invites {target} to a mutual war"),
                    ));
                }
            }
        }
        WarAct::Answer { from, accept } => {
            let a = conn.call(
                "mutualWarInviteMgr",
                "RespondToInviteByPlayer",
                vec![PyValue::Int(*from), PyValue::Bool(*accept)],
            )?;
            match a.refusal() {
                Some(key) => out.push(format!("war: invite from {from}: refused {key}")),
                None => {
                    let mut st = hall.lock();
                    out.push(line(
                        &mut st,
                        format!(
                            "war: {owner} {} {from}'s mutual-war invite",
                            if *accept { "accepts" } else { "declines" }
                        ),
                    ));
                }
            }
        }
        WarAct::OpenAllies(war) => {
            let a = bound(
                conn,
                "warRegistry",
                owner,
                "SetOpenForAllies",
                vec![PyValue::Int(*war), PyValue::Bool(true)],
            )?;
            match a.refusal() {
                Some(key) => out.push(format!("war {war}: open for allies refused {key}")),
                None => {
                    let mut st = hall.lock();
                    if let Some(w) = st.coal.wars.get_mut(war) {
                        w.open = true;
                    }
                    out.push(line(&mut st, format!("war {war}: {owner} asks for allies")));
                }
            }
        }
        WarAct::OfferAlly { war, defender } => {
            hall.lock().coal.offered.insert((owner, *war), now);
            let a = bound(
                conn,
                "warRegistry",
                owner,
                "CreateWarAllyOffer",
                vec![
                    PyValue::Int(*war),
                    PyValue::Real(0.0),
                    PyValue::Int(*defender),
                    uni("We stand with our friends."),
                ],
            )?;
            match a.refusal() {
                Some(key) => out.push(format!("war {war}: ally offer refused {key}")),
                None => {
                    let mut st = hall.lock();
                    out.push(line(
                        &mut st,
                        format!("war {war}: {owner} offers to ally with {defender}"),
                    ));
                }
            }
        }
        WarAct::AcceptAlly(neg) => {
            let a = bound(
                conn,
                "warRegistry",
                owner,
                "AcceptAllyNegotiation",
                vec![PyValue::Int(*neg)],
            )?;
            match a.refusal() {
                Some(key) => out.push(format!("ally offer {neg}: accept refused {key}")),
                None => {
                    let mut st = hall.lock();
                    out.push(line(
                        &mut st,
                        format!("war: {owner} takes ally offer {neg}"),
                    ));
                }
            }
        }
        WarAct::OfferSurrender { war, isk, to } => {
            hall.lock().coal.offered.insert((owner, *war), now);
            let a = bound(
                conn,
                "warRegistry",
                owner,
                "CreateSurrenderNegotiation",
                vec![
                    PyValue::Int(*war),
                    PyValue::Real(*isk),
                    uni("Enough. We ask for peace."),
                ],
            )?;
            match a.refusal() {
                Some(key) => out.push(format!("war {war}: surrender refused {key}")),
                None => {
                    let mut st = hall.lock();
                    out.push(line(
                        &mut st,
                        format!("war {war}: {owner} offers to surrender for {isk:.0}"),
                    ));
                    drop(st);
                    out.extend(tell(conn, hall, owner, *to, false)?);
                }
            }
        }
        WarAct::AcceptSurrender { neg, from } => {
            let a = bound(
                conn,
                "warRegistry",
                owner,
                "AcceptSurrender",
                vec![PyValue::Int(*neg)],
            )?;
            match a.refusal() {
                Some(key) => out.push(format!("surrender {neg}: accept refused {key}")),
                None => {
                    let mut st = hall.lock();
                    out.push(line(
                        &mut st,
                        format!("war: {owner} accepts surrender {neg}"),
                    ));
                    drop(st);
                    out.extend(tell(conn, hall, owner, *from, false)?);
                }
            }
        }
        WarAct::AutoPay(on) => {
            let inner = PyValue::Dict(vec![
                (PyValue::Bool(*on), PyValue::Int(BILL_TYPE_WAR)),
                (
                    PyValue::Int(MASTER_KEY),
                    PyValue::Bytes(b"divisionID".to_vec()),
                ),
            ]);
            let settings = PyValue::Dict(vec![(inner, PyValue::Int(owner))]);
            let a = soft(conn.call("billMgr", "SendAutomaticPaySettings", vec![settings]))?;
            match a {
                Ok(o) => match o.refusal() {
                    Some(key) => out.push(format!("war bills {owner}: auto-pay refused {key}")),
                    None => {
                        let mut st = hall.lock();
                        st.coal.autopay.insert(owner, *on);
                        out.push(line(
                            &mut st,
                            format!(
                                "war: {owner} {} its war bills",
                                if *on { "pays" } else { "lets lapse" }
                            ),
                        ));
                    }
                },
                Err(why) => out.push(format!("war bills {owner}: {why}")),
            }
        }
    }
    Ok(out)
}

/// Mail the target's CEO that a war was declared (a bot group only: a human
/// group's CEO is not known to the hall).
fn tell(conn: &mut Conn, hall: &Hall, owner: i64, target: i64, war: bool) -> Result<Vec<String>> {
    let (to, ours) = {
        let st = hall.lock();
        let ceo_corp = if crate::society::PLAYER_ALLIANCES.contains(&target) {
            st.alliances.get(&target).and_then(|a| a.executor)
        } else {
            Some(target)
        };
        let ceo = ceo_corp.and_then(|c| st.corps.get(&c)).and_then(|c| c.ceo);
        let ours = st
            .corps
            .get(&owner)
            .map(|c| c.name.clone())
            .or_else(|| st.alliances.get(&owner).map(|a| a.name.clone()))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "A corporation".to_owned());
        (ceo, ours)
    };
    let Some(to) = to else {
        return Ok(Vec::new());
    };
    if Some(to) == conn.session().char_id() {
        return Ok(Vec::new());
    }
    let (title, body) = if war {
        crate::mail::war_text(&ours)
    } else {
        crate::mail::peace_text(&ours)
    };
    Ok(vec![match crate::mail::send(conn, &[to], &title, &body)? {
        Ok(_) => format!(
            "mail {to}: {} from {owner}: ok",
            if war { "war" } else { "peace" }
        ),
        Err(key) => format!("mail {to}: refused {key}"),
    }])
}
