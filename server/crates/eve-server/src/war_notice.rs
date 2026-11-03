//! The war's notifications, through NOTIF-1's `notify` (C8-e; R-WAR §8; lane
//! note `docs/lanes/c8-a.md`).
//!
//! Who hears: the **directors** (the CEO and every member with the Director
//! role) of each party; for an alliance, its executor corporation's. A war
//! window is theirs, and fifty bot pilots in a corp would otherwise each fill
//! their 250 slots with the same declaration. The type ids are
//! `notificationconst` and the data keys are the formatters' --
//! `notifications/common/formatters/war.py` and the war entries of
//! `notificationUtil.formatters` -- **[V]**.
//!
//! One message, one type: R-WAR's table lists the declaration three times
//! (`CorpWarDeclaredMsg` 27 / `AllWarDeclaredMsg` 5, their version 2 at 224 /
//! 225, and `WarDeclared` 232). They are the same message across client
//! versions, and `FormatWarDeclared` reads 232's `delayHours` and `warHQ` to
//! pick its newest wording, so only 232 is sent; sending the others would
//! show a declaration two or three times.

use crate::damage::BlueTime;
use crate::notifications::{Data, Value, data, notify};
use crate::service::Registry;
use crate::war::{
    self, COST_OF_WAR, FinishWith, TYPE_WAR_HQ, WAR_SPOOLUP, War, ended, owner_directors,
};
use crate::war_deal::{BILL_TYPE_WAR, Bill, Negotiation};

/// `notificationconst` type ids **[V]**.
pub mod kind {
    pub const BILL_OUT_OF_MONEY: i64 = 11;
    pub const CORP_BILL: i64 = 10;
    pub const CORP_WAR_FIGHTING_LEGAL: i64 = 28;
    pub const ALL_WAR_SURRENDER: i64 = 6;
    pub const CORP_WAR_SURRENDER: i64 = 29;
    pub const ALLY_JOINED_DEFENDER: i64 = 99;
    pub const ALLY_JOINED_AGGRESSOR: i64 = 100;
    pub const ALLY_JOINED_ALLY: i64 = 101;
    pub const MERC_OFFERED: i64 = 102;
    pub const SURRENDER_OFFER: i64 = 103;
    pub const SURRENDER_DECLINED: i64 = 104;
    pub const ALLY_OFFER_DECLINED: i64 = 110;
    pub const DECLARE_WAR: i64 = 121;
    pub const OFFERED_SURRENDER: i64 = 122;
    pub const ACCEPTED_SURRENDER: i64 = 123;
    pub const MADE_WAR_MUTUAL: i64 = 124;
    pub const RETRACTS_WAR: i64 = 125;
    pub const OFFERED_TO_ALLY: i64 = 126;
    pub const ACCEPTED_ALLY: i64 = 127;
    pub const CORP_BECAME_WAR_ELIGIBLE: i64 = 221;
    pub const CORP_NO_LONGER_WAR_ELIGIBLE: i64 = 222;
    pub const MUTUAL_INVITE_SENT: i64 = 229;
    pub const MUTUAL_INVITE_REJECTED: i64 = 230;
    pub const MUTUAL_INVITE_ACCEPTED: i64 = 231;
    pub const WAR_DECLARED: i64 = 232;
    pub const WAR_ADOPTED: i64 = 233;
    pub const MUTUAL_EXPIRED: i64 = 234;
    pub const WAR_INHERITED: i64 = 235;
    pub const WAR_RETRACTED: i64 = 238;
    pub const WAR_RETRACTED_BY_CONCORD: i64 = 239;
    pub const WAR_INVALID: i64 = 240;
    pub const MERC_OFFER_RETRACTED: i64 = 241;
    pub const OFFER_TO_ALLY_RETRACTED: i64 = 242;
    pub const WAR_ENDED_HQ_SECURITY_DROP: i64 = 246;
}

fn int(n: i64) -> Value {
    Value::Int(n)
}

/// Tell every director of `owner`.
fn tell(reg: &mut Registry, owner: i64, type_id: i64, d: &Data) {
    for ch in owner_directors(reg, owner) {
        notify(reg, ch, type_id, d.clone());
    }
}

/// Tell every director of `owner`, signed by CONCORD **[A]**: the
/// formatters of 239, 240 and 246 sign off as CONCORD (`bodyConcordSignOff`),
/// and an NPC corporation is a sender the client can name.
fn tell_concord(reg: &mut Registry, owner: i64, type_id: i64, d: &Data) {
    for ch in owner_directors(reg, owner) {
        crate::notifications::notify_from(reg, ch, type_id, war::CONCORD_OWNER, d.clone());
    }
}

/// Tell both principals' directors, once each.
fn tell_parties(reg: &mut Registry, w: &War, type_id: i64, d: &Data) {
    tell(reg, w.declared_by, type_id, d);
    if w.against != w.declared_by {
        tell(reg, w.against, type_id, d);
    }
}

fn owners_data(w: &War) -> Vec<(&'static str, Value)> {
    vec![
        ("declaredByID", int(w.declared_by)),
        ("againstID", int(w.against)),
    ]
}

/// A corp's permit flipped: 221 or 222, no data **[V `warPermit`]**.
pub fn permit_changed(reg: &mut Registry, corp: i64, allowed: bool) {
    let kind = if allowed {
        kind::CORP_BECAME_WAR_ELIGIBLE
    } else {
        kind::CORP_NO_LONGER_WAR_ELIGIBLE
    };
    for ch in war::corp_directors(reg, corp) {
        notify(reg, ch, kind, Vec::new());
    }
}

/// A war was declared: 232 to both parties' directors with the fee, the
/// spool-up and the HQ, and 121 to the aggressor's directors naming who did
/// it **[V formatters]**.
pub fn declared(reg: &mut Registry, w: &War, actor: i64) {
    let cost = if w.mutual { 0.0 } else { COST_OF_WAR };
    let hq = w.war_hq.and_then(|i| war::hq_by_item(reg.deltas(), i));
    let hq_name = hq.map(|h| war::hq_name(reg, &h));
    let mut fields = owners_data(w);
    fields.extend([
        ("cost", Value::Real(cost)),
        ("hostileState", Value::Bool(false)),
        ("timeDeclared", int(w.time_declared)),
        ("timeStarted", int(w.time_started)),
        (
            "delayHours",
            int(war::span(reg, WAR_SPOOLUP) / crate::seed::HOUR),
        ),
    ]);
    if let (Some(item), Some(name)) = (w.war_hq, hq_name) {
        fields.push(("warHQ", Value::Text(name)));
        fields.push((
            "warHQ_IdType",
            Value::List(vec![int(item), int(TYPE_WAR_HQ)]),
        ));
    }
    let d: Data = fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    tell_parties(reg, w, kind::WAR_DECLARED, &d);
    tell(
        reg,
        w.declared_by,
        kind::DECLARE_WAR,
        &data([
            ("defenderID", int(w.against)),
            ("entityID", int(w.declared_by)),
            ("charID", int(actor)),
        ]),
    );
}

/// The war can be fought now: 28 to the defender's directors **[V]**.
pub fn fighting_legal(reg: &mut Registry, w: &War) {
    let cost = if w.mutual { 0.0 } else { COST_OF_WAR };
    let mut fields = owners_data(w);
    fields.push(("cost", Value::Real(cost)));
    let d: Data = fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    tell(reg, w.against, kind::CORP_WAR_FIGHTING_LEGAL, &d);
}

/// A war ended for a reason the parties should hear: unpaid bill (239), HQ
/// system security drop (246), or CONCORD finding it invalid (240). A
/// surrender or retraction has its own notices; an adoption, a GM action and
/// a closed owner say nothing more.
pub fn ended(reg: &mut Registry, w: &War, how: &FinishWith, now: BlueTime) {
    let end = w.time_finished.unwrap_or(now);
    let mut fields = owners_data(w);
    fields.push(("endDate", int(end)));
    match how.reason {
        ended::UNPAID_BILL => {
            let d: Data = fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
            tell_concord(reg, w.declared_by, kind::WAR_RETRACTED_BY_CONCORD, &d);
            tell_concord(reg, w.against, kind::WAR_RETRACTED_BY_CONCORD, &d);
        }
        ended::WAR_HQ_SYSTEM_SECURITY_DROP => {
            if let Some(item) = w.war_hq {
                let name = war::hq_by_item(reg.deltas(), item)
                    .map(|h| war::hq_name(reg, &h))
                    .unwrap_or_default();
                fields.push(("warHQ", Value::Text(name)));
                fields.push((
                    "warHQ_IdType",
                    Value::List(vec![int(item), int(TYPE_WAR_HQ)]),
                ));
            }
            let d: Data = fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
            tell_concord(reg, w.declared_by, kind::WAR_ENDED_HQ_SECURITY_DROP, &d);
            tell_concord(reg, w.against, kind::WAR_ENDED_HQ_SECURITY_DROP, &d);
        }
        ended::CORP_DELETED
        | ended::ALLIANCE_DELETED
        | ended::HQ_OWNER_LEFT
        | ended::UNLICENSED_AGGRESSOR
        | ended::UNLICENSED_DEFENDER
        | ended::CONCORD
        | ended::WARHQ_GONE => {
            let d: Data = fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
            tell_concord(reg, w.declared_by, kind::WAR_INVALID, &d);
            tell_concord(reg, w.against, kind::WAR_INVALID, &d);
        }
        _ => {}
    }
}

/// A defender took an ally offer; 102 to the defender and 126 to the offerer
/// **[V]**.
pub fn ally_offered(reg: &mut Registry, w: &War, n: &Negotiation, actor: i64) {
    tell(
        reg,
        n.owner2,
        kind::MERC_OFFERED,
        &data([
            ("mercID", int(n.owner1)),
            ("defenderID", int(w.against)),
            ("aggressorID", int(w.declared_by)),
            ("iskValue", Value::Real(n.isk)),
        ]),
    );
    tell(
        reg,
        n.owner1,
        kind::OFFERED_TO_ALLY,
        &data([
            ("defenderID", int(w.against)),
            ("enemyID", int(w.declared_by)),
            ("charID", int(actor)),
            ("iskValue", Value::Real(n.isk)),
        ]),
    );
}

pub fn ally_offer_retracted(reg: &mut Registry, w: &War, n: &Negotiation, actor: i64) {
    tell(
        reg,
        n.owner2,
        kind::MERC_OFFER_RETRACTED,
        &data([
            ("mercID", int(n.owner1)),
            ("defenderID", int(w.against)),
            ("aggressorID", int(w.declared_by)),
        ]),
    );
    tell(
        reg,
        n.owner1,
        kind::OFFER_TO_ALLY_RETRACTED,
        &data([
            ("allyID", int(n.owner1)),
            ("defenderID", int(w.against)),
            ("enemyID", int(w.declared_by)),
            ("charID", int(actor)),
        ]),
    );
}

pub fn ally_offer_declined(reg: &mut Registry, w: &War, n: &Negotiation, actor: i64) {
    tell(
        reg,
        n.owner1,
        kind::ALLY_OFFER_DECLINED,
        &data([
            ("defenderID", int(w.against)),
            ("aggressorID", int(w.declared_by)),
            ("allyID", int(n.owner1)),
            ("charID", int(actor)),
        ]),
    );
}

/// An ally joined: 99 to the defender, 100 to the aggressor, 101 to the ally,
/// and 127 to the defender's acceptor **[V]**.
pub fn ally_joined(
    reg: &mut Registry,
    w: &War,
    n: &Negotiation,
    actor: i64,
    start: BlueTime,
    isk: f64,
) {
    let base = |extra: &[(&str, Value)]| -> Data {
        let mut v = vec![
            ("allyID".to_owned(), int(n.owner1)),
            ("startTime".to_owned(), int(start)),
        ];
        v.extend(extra.iter().map(|(k, x)| ((*k).to_owned(), x.clone())));
        v
    };
    tell(
        reg,
        w.against,
        kind::ALLY_JOINED_DEFENDER,
        &base(&[("aggressorID", int(w.declared_by))]),
    );
    tell(
        reg,
        w.declared_by,
        kind::ALLY_JOINED_AGGRESSOR,
        &base(&[("defenderID", int(w.against))]),
    );
    tell(
        reg,
        n.owner1,
        kind::ALLY_JOINED_ALLY,
        &base(&[
            ("defenderID", int(w.against)),
            ("aggressorID", int(w.declared_by)),
        ]),
    );
    tell(
        reg,
        w.against,
        kind::ACCEPTED_ALLY,
        &data([
            ("allyID", int(n.owner1)),
            ("enemyID", int(w.declared_by)),
            ("charID", int(actor)),
            ("iskValue", Value::Real(isk)),
            ("time", int(start)),
        ]),
    );
}

/// The data of notification 103. The body label
/// `Notifications/bodyWarSurrenderOffer` links `warNegotiation:{warNegotiationID}`
/// and the formatter (`notificationUtil.py`, `notificationTypeWarSurrenderOfferMsg`)
/// passes the data dict through, so the key must be in it or the client's
/// localization raises "Failed to find the value for 'warNegotiationID'" **[V]**.
fn surrender_offer_data(n: &Negotiation) -> Data {
    data([
        ("ownerID1", int(n.owner1)),
        ("ownerID2", int(n.owner2)),
        ("iskValue", Value::Real(n.isk)),
        ("warNegotiationID", int(n.id)),
    ])
}

/// A surrender was offered: 103 to the receiver, 122 to the offerer **[V]**.
pub fn surrender_offered(reg: &mut Registry, _w: &War, n: &Negotiation, actor: i64) {
    tell(
        reg,
        n.owner2,
        kind::SURRENDER_OFFER,
        &surrender_offer_data(n),
    );
    tell(
        reg,
        n.owner1,
        kind::OFFERED_SURRENDER,
        &data([
            ("entityID", int(n.owner1)),
            ("offeredID", int(n.owner2)),
            ("charID", int(actor)),
            ("iskValue", Value::Real(n.isk)),
        ]),
    );
}

/// A surrender was accepted: 123 to the acceptor, and the surrender message
/// (29 corp, 6 alliance) to both principals.
pub fn surrender_accepted(reg: &mut Registry, w: &War, n: &Negotiation, actor: i64) {
    tell(
        reg,
        n.owner2,
        kind::ACCEPTED_SURRENDER,
        &data([
            ("entityID", int(n.owner2)),
            ("offeringID", int(n.owner1)),
            ("charID", int(actor)),
            ("iskValue", Value::Real(n.isk)),
        ]),
    );
    let d: Data = owners_data(w)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    for owner in [w.declared_by, w.against] {
        let kind = if war::is_alliance_id(owner) {
            kind::ALL_WAR_SURRENDER
        } else {
            kind::CORP_WAR_SURRENDER
        };
        tell(reg, owner, kind, &d);
    }
}

pub fn surrender_declined(reg: &mut Registry, _w: &War, n: &Negotiation, _actor: i64) {
    tell(
        reg,
        n.owner1,
        kind::SURRENDER_DECLINED,
        &data([("ownerID", int(n.owner2)), ("iskValue", Value::Real(n.isk))]),
    );
}

/// A mutual war was retracted: 238 to both, 125 to the retractor **[V]**.
pub fn mutual_retracted(reg: &mut Registry, w: &War, actor: i64) {
    let d: Data = owners_data(w)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    tell_parties(reg, w, kind::WAR_RETRACTED, &d);
    let mine = reg
        .character(actor)
        .map(|c| c.alliance_id.unwrap_or(c.corporation_id));
    if let Some(me) = mine {
        let enemy = if me == w.declared_by {
            w.against
        } else {
            w.declared_by
        };
        tell(
            reg,
            me,
            kind::RETRACTS_WAR,
            &data([("enemyID", int(enemy)), ("charID", int(actor))]),
        );
    }
}

/// A mutual-war invite went out: 229 to both **[V `WarMutualWarInviteSent`]**.
pub fn invite_sent(reg: &mut Registry, from: i64, to: i64, expires: BlueTime) {
    let d = data([
        ("declaredByID", int(from)),
        ("againstID", int(to)),
        ("expireTimeStamp", int(expires)),
    ]);
    tell(reg, from, kind::MUTUAL_INVITE_SENT, &d);
    tell(reg, to, kind::MUTUAL_INVITE_SENT, &d);
}

/// An invite lapsed: 234 to both **[V `MutualWarExpired`]**.
pub fn invite_expired(reg: &mut Registry, from: i64, to: i64) {
    let d = data([
        ("declaredByID", int(from)),
        ("againstID", int(to)),
        ("numDays", int(war::MUTUAL_INVITE_EXPIRY / crate::seed::DAY)),
    ]);
    tell(reg, from, kind::MUTUAL_EXPIRED, &d);
    tell(reg, to, kind::MUTUAL_EXPIRED, &d);
}

/// An invite was accepted: 231 to both with the spool-up's start, and 124 to
/// the accepter **[V]**.
pub fn mutual_accepted(reg: &mut Registry, w: &War) {
    let d = data([
        ("declaredByID", int(w.declared_by)),
        ("againstID", int(w.against)),
        ("time", int(w.time_started)),
    ]);
    tell_parties(reg, w, kind::MUTUAL_INVITE_ACCEPTED, &d);
}

/// An invite was refused: 230 to both **[V]**.
pub fn mutual_rejected(reg: &mut Registry, from: i64, to: i64) {
    let d = data([("declaredByID", int(from)), ("againstID", int(to))]);
    tell(reg, from, kind::MUTUAL_INVITE_REJECTED, &d);
    tell(reg, to, kind::MUTUAL_INVITE_REJECTED, &d);
}

/// An alliance adopted a corporation's war: 233 to both principals **[V
/// `WarAdopted`]**.
pub fn adopted(reg: &mut Registry, w: &War, alliance: i64) {
    let mut fields = owners_data(w);
    fields.push(("allianceID", int(alliance)));
    let d: Data = fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    tell_parties(reg, w, kind::WAR_ADOPTED, &d);
}

/// A corporation left an alliance with its wars: 235 to the alliance, the
/// leaver and the opponent **[V `WarInherited`]**.
pub fn inherited(reg: &mut Registry, w: &War, alliance: i64, quitter: i64, opponent: i64) {
    let mut fields = owners_data(w);
    fields.extend([
        ("allianceID", int(alliance)),
        ("quitterID", int(quitter)),
        ("opponentID", int(opponent)),
    ]);
    let d: Data = fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    for owner in [alliance, quitter, opponent] {
        tell(reg, owner, kind::WAR_INHERITED, &d);
    }
}

/// A war bill was issued: 10 to the debtor's directors, with the keys
/// `FormatBillNotification` and `bodyBillWar` read **[V]**.
pub fn bill_issued(reg: &mut Registry, b: &Bill, now: BlueTime) {
    tell(
        reg,
        b.debtor,
        kind::CORP_BILL,
        &data([
            ("billTypeID", int(BILL_TYPE_WAR)),
            ("creditorID", int(b.creditor)),
            ("debtorID", int(b.debtor)),
            ("externalID", int(b.against)),
            ("amount", Value::Real(b.amount)),
            ("dueDate", int(b.due)),
            ("currentDate", int(now)),
        ]),
    );
}

/// A bill could not be paid: 11 to the debtor's directors **[V]**.
pub fn bill_unpaid(reg: &mut Registry, b: &Bill) {
    tell(
        reg,
        b.debtor,
        kind::BILL_OUT_OF_MONEY,
        &data([("billTypeID", int(BILL_TYPE_WAR))]),
    );
}

#[cfg(test)]
mod surrender_data_tests {
    use super::*;

    #[test]
    fn surrender_offer_carries_the_negotiation_id_the_body_links() {
        let n = Negotiation {
            id: 7001,
            war_id: 1,
            kind: 2,
            owner1: 11,
            owner2: 12,
            declared_by: 12,
            against: 11,
            isk: 5.0,
            description: String::new(),
            state: 0,
            created: BlueTime::default(),
            accepted: None,
            declined: None,
            retracted: None,
        };
        let d = surrender_offer_data(&n);
        let get = |k: &str| d.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert!(matches!(get("warNegotiationID"), Some(Value::Int(7001))));
        assert!(matches!(get("ownerID1"), Some(Value::Int(11))));
        assert!(matches!(get("iskValue"), Some(Value::Real(_))));
    }
}
