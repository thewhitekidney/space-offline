//! **Duels** (CW-1): a consensual limited engagement between two
//! capsuleers.
//!
//! What the client fixes **[V]** (build 3619879):
//!
//! - the offer: `crimewatch.StartDuelChallenge(charID)` from the "Invite to
//!   Duel" menu entry, which the client offers only outside zero sec, never
//!   on an NPC, oneself, or a pilot one is already engaged with
//!   (`characterCheckers.OfferDuelMenuEntry`);
//! - the challenged pilot's window: `OnDuelChallenge(fromCharID, fromCorpID,
//!   fromAllianceID, expiryTime)` opens `DuelInviteWindow` with a countdown
//!   to `expiryTime`; the answer is `RespondToDuelChallenge(fromCharID,
//!   expiryTime, accept)` (`crimewatchSvc.OnDuelChallenge`);
//! - "Accept this challenge and we will enter a **Limited Engagement**"
//!   (286563) -- acceptance is an engagement, and the engagement's own
//!   pushes close the window (`OnCrimewatchEngagementCreated` closes
//!   `duelWindow` for that character);
//! - `duelOfferExpiryTimeout` = 30 s (`crimewatch/const.py`);
//! - the refusals and notices, the client's keys and argument names
//!   (`findings/user-errors.tsv`): `DuelChallengeFailNoSesssionForTarget
//!   {targetCharID}`, `DuelChallengeFailBlockedByTarget {targetCharID}`,
//!   `DuelChallengeFailTooManyOutgoingRequests {newTargetCharID,
//!   outstandingTargetCharID}`, `DuelChallengeFailTooManyIncomingRequests
//!   {targetCharID}`, `DuelChallengeAcceptFail {sourceCharID}`,
//!   `DuelChallengeDeclined {targetCharID}` and
//!   `DuelChallengeAcceptedParkNotify {sourceCharID, targetCharID}`, all
//!   `raise_sites` 0.
//!
//! `[A]`: one outstanding offer per challenger, [`MAX_INCOMING`] per
//! challenged pilot; the challenged pilot must be online and in space in the
//! challenger's system; the notices go as `OnRemoteMessage(key, dict)`.

use super::{Notice, SEC, Space};
use crate::damage::BlueTime;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::time::Duration;

/// `duelOfferExpiryTimeout` **[V]**.
pub const OFFER_TIMEOUT: BlueTime = 30 * SEC;

/// How many offers a pilot may have waiting on them at once `[A]`.
pub const MAX_INCOMING: usize = 3;

fn refuse(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

fn notify(text: &str) -> ServiceError {
    refuse("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// `OnRemoteMessage(key, dict)` to one pilot: how a server notice with the
/// client's own text reaches its message box `[A]`.
fn remote_message(w: &mut Registry, to: i64, key: &str, args: Vec<(&str, PyValue)>) {
    w.schedule(
        Duration::ZERO,
        Notice::new(
            to,
            "OnRemoteMessage",
            vec![crate::pyutil::bstr(key), crate::pyutil::pydict(args)],
        ),
    );
}

/// `crimewatch.StartDuelChallenge(charID)`.
pub fn start(
    w: &mut Registry,
    actor: Option<i64>,
    target: i64,
    now: BlueTime,
) -> Result<PyValue, ServiceError> {
    let Some(me) = actor else {
        return Err(notify("No character."));
    };
    if target == me || w.character(target).is_none() {
        return Err(notify("You can only challenge another capsuleer."));
    }
    let Some(here) = w.roster().get(me).map(|p| p.solar_system_id) else {
        return Err(notify(
            "You must be in space to challenge a pilot to a duel.",
        ));
    };
    if super::space_of(w, here) == Space::Zero {
        return Err(notify("Duels are not offered in zero-security space."));
    }
    if w.combat().crimewatch().engaged(me, target, now) {
        return Ok(PyValue::None);
    }
    if !w.sessions().is_online(target) {
        return Err(refuse(
            "DuelChallengeFailNoSesssionForTarget",
            vec![("targetCharID", PyValue::Int(target))],
        ));
    }
    if w.roster()
        .get(target)
        .is_none_or(|p| p.solar_system_id != here)
    {
        return Err(notify("That pilot is not in space in your solar system."));
    }
    if w.contacts.blocked_by(target).contains(&me) {
        return Err(refuse(
            "DuelChallengeFailBlockedByTarget",
            vec![("targetCharID", PyValue::Int(target))],
        ));
    }
    let cw = w.combat().crimewatch();
    if let Some(&(_, other)) = cw
        .duels
        .iter()
        .find(|((from, to), until)| *from == me && *to != target && now < **until)
        .map(|(k, _)| k)
    {
        return Err(refuse(
            "DuelChallengeFailTooManyOutgoingRequests",
            vec![
                ("newTargetCharID", PyValue::Int(target)),
                ("outstandingTargetCharID", PyValue::Int(other)),
            ],
        ));
    }
    let incoming = cw
        .duels
        .iter()
        .filter(|((from, to), until)| *to == target && *from != me && now < **until)
        .count();
    if incoming >= MAX_INCOMING {
        return Err(refuse(
            "DuelChallengeFailTooManyIncomingRequests",
            vec![("targetCharID", PyValue::Int(target))],
        ));
    }
    let expiry = now + OFFER_TIMEOUT;
    w.combat_mut()
        .crimewatch_mut()
        .duels
        .insert((me, target), expiry);
    let (corp, alliance) = w.character(me).map_or((PyValue::None, PyValue::None), |c| {
        (
            PyValue::Int(c.corporation_id),
            c.alliance_id.map_or(PyValue::None, PyValue::Int),
        )
    });
    w.schedule(
        Duration::ZERO,
        Notice::new(
            target,
            "OnDuelChallenge",
            vec![PyValue::Int(me), corp, alliance, PyValue::Int(expiry)],
        ),
    );
    Ok(PyValue::None)
}

/// `crimewatch.RespondToDuelChallenge(fromCharID, expiryTime, accept)`.
///
/// An acceptance of a live offer opens the engagement (both told by its own
/// pushes) and tells the system (`DuelChallengeAcceptedParkNotify`); one of
/// a stale or unknown offer is `DuelChallengeAcceptFail`. A refusal tells
/// the challenger (`DuelChallengeDeclined`).
pub fn respond(
    w: &mut Registry,
    actor: Option<i64>,
    from: i64,
    expiry: Option<BlueTime>,
    accept: bool,
    now: BlueTime,
) -> Result<PyValue, ServiceError> {
    let Some(me) = actor else {
        return Err(notify("No character."));
    };
    let offer = w.combat_mut().crimewatch_mut().duels.remove(&(from, me));
    let live = offer.is_some_and(|until| now < until && expiry.is_none_or(|e| e == until));
    if !accept {
        if live {
            remote_message(
                w,
                from,
                "DuelChallengeDeclined",
                vec![("targetCharID", PyValue::Int(me))],
            );
        }
        return Ok(PyValue::None);
    }
    if !live {
        return Err(refuse(
            "DuelChallengeAcceptFail",
            vec![("sourceCharID", PyValue::Int(from))],
        ));
    }
    crate::outln!("[crimewatch] duel: {me} accepted {from}'s challenge");
    super::engage(w, from, me, now, now);
    let system = w.roster().get(me).map(|p| p.solar_system_id);
    let to = system.map_or_else(Vec::new, |s| super::pilots_in(w, s));
    for c in to {
        remote_message(
            w,
            c,
            "DuelChallengeAcceptedParkNotify",
            vec![
                ("sourceCharID", PyValue::Int(from)),
                ("targetCharID", PyValue::Int(me)),
            ],
        );
    }
    Ok(PyValue::None)
}
