//! What passes between the sides of a war: peace treaties, ally offers,
//! surrender offers, mutual-war invites, the war bills, the membership rules
//! a war puts on an alliance, and the GM's hands (C8-b/c/d; R-WAR §2.2-2.4,
//! §3, §7, §9; lane note `docs/lanes/c8-a.md`).
//!
//! The war itself, its predicates and its clock are `war.rs`; the wire is
//! `service/war.rs`; the notices are `war_notice.rs`. Every operation here is
//! `fn(reg, actor, ..) -> Result<_, Refusal>` with the client's own message
//! key on the refusal, and does its own money, rows, pushes and notices.
//!
//! ```text
//! warneg   <negID>        -> warID type owner1 owner2 declaredBy against isk
//!                            description state created accepted declined retracted
//! warinv   <from to>      -> sent                       (a mutual-war invite)
//! warinv   <owner>        -> 1                          (that owner blocks invites)
//! peace    <owner other>  -> expiry reason warID        (a peace treaty, one direction)
//! warbill  <billID>       -> debtor creditor amount due paid paidAt warID against
//! warauto  <owner>        -> 0|1                        (war bills auto-pay)
//! warleft  <character>    -> corp left                  (a pilot who left a corp at war)
//! ```

use crate::alliance::Alliances;
use crate::damage::BlueTime;
use crate::delta::Deltas;
use crate::seed::DAY;
use crate::service::Registry;
use crate::war::{
    self, ALLY_COMMITMENT_PERIOD, ALLY_SPOOLUP, Ally, BILLING_PERIOD, CONCORD_OWNER, COST_OF_WAR,
    FinishWith, MUTUAL_INVITE_EXPIRY, NEGOTIATION_EXPIRY, REF_WAR_ALLY_CONTRACT, REF_WAR_FEE,
    REF_WAR_SURRENDER_FEE, Refusal, Via, WAR_SPOOLUP, War, Wars, ended, id_field, num, opt, peace,
    span,
};
use eve_marshal::PyValue;

pub const KIND_NEG: &str = "warneg";
pub const KIND_INVITE: &str = "warinv";
pub const KIND_PEACE: &str = "peace";
pub const KIND_BILL: &str = "warbill";
pub const KIND_AUTO: &str = "warauto";
pub const KIND_LEFT: &str = "warleft";
pub const KINDS: [&str; 6] = [
    KIND_NEG,
    KIND_INVITE,
    KIND_PEACE,
    KIND_BILL,
    KIND_AUTO,
    KIND_LEFT,
];

/// `WAR_NEGOTIATION_TYPE_*` and `_STATE_*` **[V]**.
pub const NEG_ALLY_OFFER: i64 = 0;
pub const NEG_SURRENDER_OFFER: i64 = 2;
pub const STATE_NEW: i64 = 0;
pub const STATE_ACCEPTED: i64 = 1;
pub const STATE_DECLINED: i64 = 2;
pub const STATE_RETRACTED: i64 = 3;

/// **[A]** the most mutual-war invites an owner may have out at once.
/// `CannotSendMutualWarInviteTooManyOutgoing` exists but its limit is
/// server-side. Falsified by a friend wanting more.
pub const MAX_OUTGOING_INVITES: usize = 3;

/// `appConst.billTypeWarBill = 4` **[V]** (`warLandingPage` counts unpaid
/// bills of this type).
pub const BILL_TYPE_WAR: i64 = 4;

/// **[A]** a war bill is paid out of the owner's wallet when it falls due,
/// unless the owner switched auto-pay off (`warauto`). A forgetful friend
/// does not lose a war to a missed bill; a bot never has to find the bill.
/// Falsified by the owner wanting a war lost to a missed payment (set the
/// default false).
pub const AUTOPAY_DEFAULT: bool = true;

/// **[A]** the base of the CONCORD ally fee: 10,000,000 ISK before the
/// Diplomatic Relations discount **[V-data]** (`corpRegistry.
/// CharGetAllyBaseCost`; the value is a type attribute, read here as a
/// constant). Falsified by a client price that differs.
pub const ALLY_BASE_COST: f64 = 10_000_000.0;
/// Diplomatic Relations, the skill that cheapens an ally **[V-data]**.
pub const SKILL_DIPLOMATIC_RELATIONS: i64 = 3368;
/// **[A]** the discount per level of Diplomatic Relations: 5 % (`attr 1821`
/// is -5 **[V-data]**).
pub const ALLY_DISCOUNT_PER_LEVEL: f64 = 0.05;

/// The Wars tab cuts a pilot who left a corp at war out of it for a week
/// (`CharacterDodgedWar`) **[V-data]**.
pub const DODGE_PERIOD: BlueTime = 7 * DAY;

// ---------------------------------------------------------------------------
// Who is acting
// ---------------------------------------------------------------------------

/// The authority a call needs: the owner a pilot speaks for, as the war
/// windows gate it (`warEntry.IsInCharge`): a corporation's CEO, an
/// alliance's executor director. `director_ok` also takes any director of a
/// corporation (the declare, ally and invite calls ask for a Director).
fn owner_for(
    reg: &Registry,
    actor: i64,
    director_ok: bool,
    key: &'static str,
) -> Result<i64, Refusal> {
    let member = reg
        .corporations()
        .membership(actor)
        .ok_or(Refusal::Key(key))?;
    let corp = member.corporation_id;
    let c = reg.corporations().get(corp).ok_or(Refusal::Key(key))?;
    let ceo = c.ceo_id == actor;
    let director = ceo || member.roles & crate::corp_roles::CORP_ROLE_DIRECTOR != 0;
    match crate::alliance::of_corp(reg.deltas(), corp) {
        Some(alliance) => {
            let st = Alliances::load(reg.deltas());
            let executor = st.get(alliance).and_then(|a| a.executor_corp_id) == Some(corp);
            if executor && director {
                Ok(alliance)
            } else {
                Err(Refusal::Key(if key == "MustBeActiveCEOToEndWar" {
                    "MustBeAllianceCEOToEndWar"
                } else {
                    key
                }))
            }
        }
        None if ceo || (director_ok && director) => Ok(corp),
        None => Err(Refusal::Key(key)),
    }
}

/// The owner a director speaks for, if `actor` is one: the corp, or its
/// alliance when the corp is the executor.
pub fn speaks_for(reg: &Registry, actor: i64) -> Option<i64> {
    director_owner(reg, actor).ok()
}

/// The owner `actor` may end wars for: CEO of a corp, executor director of an
/// alliance.
fn in_charge(reg: &Registry, actor: i64) -> Result<i64, Refusal> {
    owner_for(reg, actor, true, "MustBeActiveCEOToEndWar")
}

/// The owner `actor` may make deals for: any director (executor's, for an
/// alliance). The refusal is `CrpAccessDenied`.
fn director_owner(reg: &Registry, actor: i64) -> Result<i64, Refusal> {
    owner_for(reg, actor, true, "CrpAccessDenied")
        .map_err(|_| Refusal::Access("Only a director can do that for a corporation."))
}

/// The owners a pilot belongs to: their alliance and their corp.
fn my_owners(reg: &Registry, actor: i64) -> Vec<i64> {
    let Some(c) = reg.character(actor) else {
        return Vec::new();
    };
    let mut v = Vec::new();
    v.extend(c.alliance_id);
    v.push(c.corporation_id);
    v
}

/// The session owner: the alliance, else the corp.
pub fn session_owner(reg: &Registry, actor: i64) -> Option<i64> {
    let c = reg.character(actor)?;
    Some(c.alliance_id.unwrap_or(c.corporation_id))
}

// ---------------------------------------------------------------------------
// Peace treaties
// ---------------------------------------------------------------------------

/// One peace treaty, from one owner's side **[V `warHistoryCont`]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Treaty {
    pub owner: i64,
    pub other: i64,
    pub expiry: BlueTime,
    pub reason: i64,
    pub war_id: i64,
}

/// Forbid `owner` to declare on `other` until `until`.
pub fn make_peace(
    d: &mut Deltas,
    owner: i64,
    other: i64,
    until: BlueTime,
    reason: i64,
    war_id: i64,
) {
    let key = vec![owner.to_string(), other.to_string()];
    // A longer treaty is never shortened.
    let keep = d
        .of(KIND_PEACE)
        .get(&key)
        .and_then(|v| num(v, 0).ok())
        .unwrap_or(0);
    if keep >= until {
        return;
    }
    d.put(
        KIND_PEACE,
        key,
        vec![until.to_string(), reason.to_string(), war_id.to_string()],
    );
}

/// Whether `owner` may not declare on `other` at `now`.
pub fn treaty_between(d: &Deltas, owner: i64, other: i64, now: BlueTime) -> bool {
    d.of(KIND_PEACE)
        .get(&vec![owner.to_string(), other.to_string()])
        .and_then(|v| num(v, 0).ok())
        .is_some_and(|expiry| now < expiry)
}

/// `owner`'s treaties at `now`: `(outgoing, incoming)`. Outgoing are the
/// treaties that bind `owner`; incoming are the ones that bind somebody from
/// declaring on `owner`, with `other` the party they bind.
pub fn treaties_of(d: &Deltas, owner: i64, now: BlueTime) -> (Vec<Treaty>, Vec<Treaty>) {
    let (mut out, mut inc) = (Vec::new(), Vec::new());
    for (k, v) in d.of(KIND_PEACE) {
        let (Ok(a), Ok(b), Ok(expiry)) = (num(k, 0), num(k, 1), num(v, 0)) else {
            continue;
        };
        if expiry <= now {
            continue;
        }
        let t = Treaty {
            owner: a,
            other: b,
            expiry,
            reason: num(v, 1).unwrap_or(peace::UNDEFINED),
            war_id: num(v, 2).unwrap_or(0),
        };
        if a == owner {
            out.push(t);
        } else if b == owner {
            inc.push(Treaty {
                owner: b,
                other: a,
                ..t
            });
        }
    }
    (out, inc)
}

/// A treaty on the wire: `otherOwnerID`, `expiryDate`, `peaceReason`,
/// `warID` **[V `warHistoryCont`, `warDeclare`]**.
pub fn treaty_value(t: &Treaty) -> PyValue {
    crate::pyutil::keyval(vec![
        ("otherOwnerID", PyValue::Int(t.other)),
        ("expiryDate", PyValue::Int(t.expiry)),
        ("peaceReason", PyValue::Int(t.reason)),
        ("warID", PyValue::Int(t.war_id)),
    ])
}

// ---------------------------------------------------------------------------
// Negotiations
// ---------------------------------------------------------------------------

/// One negotiation: an ally offer (`ownerID1` the would-be ally, `ownerID2`
/// the defender) or a surrender offer (`ownerID1` the side that gives in)
/// **[V `corp_ui_wars`, `warEntry`, `warWindows`]**.
#[derive(Debug, Clone, PartialEq)]
pub struct Negotiation {
    pub id: i64,
    pub war_id: i64,
    pub kind: i64,
    pub owner1: i64,
    pub owner2: i64,
    pub declared_by: i64,
    pub against: i64,
    pub isk: f64,
    pub description: String,
    pub state: i64,
    pub created: BlueTime,
    pub accepted: Option<BlueTime>,
    pub declined: Option<BlueTime>,
    pub retracted: Option<BlueTime>,
}

impl Negotiation {
    fn to_row(&self) -> Vec<String> {
        vec![
            self.war_id.to_string(),
            self.kind.to_string(),
            self.owner1.to_string(),
            self.owner2.to_string(),
            self.declared_by.to_string(),
            self.against.to_string(),
            format!("{}", self.isk),
            self.description.clone(),
            self.state.to_string(),
            self.created.to_string(),
            id_field(self.accepted),
            id_field(self.declined),
            id_field(self.retracted),
        ]
    }

    fn from_row(id: i64, v: &[String]) -> Result<Negotiation, String> {
        Ok(Negotiation {
            id,
            war_id: num(v, 0)?,
            kind: num(v, 1)?,
            owner1: num(v, 2)?,
            owner2: num(v, 3)?,
            declared_by: num(v, 4)?,
            against: num(v, 5)?,
            isk: v
                .get(6)
                .ok_or("isk is missing")?
                .parse::<f64>()
                .map_err(|e| e.to_string())?,
            description: v.get(7).cloned().unwrap_or_default(),
            state: num(v, 8)?,
            created: num(v, 9)?,
            accepted: opt(v, 10)?,
            declined: opt(v, 11)?,
            retracted: opt(v, 12)?,
        })
    }

    /// The negotiation as the client reads it.
    pub fn value(&self) -> PyValue {
        let o = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
        crate::pyutil::keyval(vec![
            ("warNegotiationID", PyValue::Int(self.id)),
            ("warID", PyValue::Int(self.war_id)),
            ("warNegotiationTypeID", PyValue::Int(self.kind)),
            ("ownerID1", PyValue::Int(self.owner1)),
            ("ownerID2", PyValue::Int(self.owner2)),
            ("declaredByID", PyValue::Int(self.declared_by)),
            ("againstID", PyValue::Int(self.against)),
            ("iskValue", PyValue::Real(self.isk)),
            ("description", PyValue::Unicode(self.description.clone())),
            ("negotiationState", PyValue::Int(self.state)),
            ("createdDateTime", PyValue::Int(self.created)),
            ("timeAccepted", o(self.accepted)),
            ("timeDeclined", o(self.declined)),
            ("timeRetracted", o(self.retracted)),
        ])
    }

    /// Whether `owner` is a party to it: either end, or either principal of
    /// its war.
    pub fn involves(&self, owner: i64) -> bool {
        [self.owner1, self.owner2, self.declared_by, self.against].contains(&owner)
    }
}

/// One negotiation, from the deltas.
pub fn negotiation(d: &Deltas, id: i64) -> Option<Negotiation> {
    Negotiation::from_row(id, d.of(KIND_NEG).get(&vec![id.to_string()])?).ok()
}

fn put_negotiation(d: &mut Deltas, n: &Negotiation) {
    d.put(KIND_NEG, vec![n.id.to_string()], n.to_row());
}

/// Every negotiation, in id order.
pub fn negotiations(d: &Deltas) -> Vec<Negotiation> {
    d.of(KIND_NEG)
        .iter()
        .filter_map(|(k, v)| Negotiation::from_row(num(k, 0).ok()?, v).ok())
        .collect()
}

/// The new negotiations an owner is in (`GetNegotiations`).
pub fn negotiations_of(d: &Deltas, owner: i64) -> Vec<Negotiation> {
    negotiations(d)
        .into_iter()
        .filter(|n| n.state == STATE_NEW && n.involves(owner))
        .collect()
}

fn load_war_or(reg: &Registry, war_id: i64) -> Result<War, Refusal> {
    war::load_war(reg.deltas(), war_id).ok_or(Refusal::Key("CanNotChangeEndedWar"))
}

/// `GetAllyCostToConcord` **[V]**: no fee for the first ally; then `base ×
/// 2^(n-1)` for `n` allies not yet finished, the exponent capped at 20.
pub fn ally_cost_to_concord(w: &War, base: f64, now: BlueTime) -> f64 {
    let n = w.allies.values().filter(|a| now < a.time_finished).count();
    if n == 0 {
        0.0
    } else {
        base * f64::from(1u32 << (n - 1).min(20))
    }
}

/// `CharGetAllyBaseCost`: [`ALLY_BASE_COST`] less 5 % a level of Diplomatic
/// Relations **[I formula]**.
pub fn ally_base_cost(reg: &Registry, actor: i64) -> f64 {
    let level = reg
        .skills_for(Some(actor))
        .and_then(|s| s.get(SKILL_DIPLOMATIC_RELATIONS))
        .map_or(0, |e| e.level.clamp(0, 5));
    ALLY_BASE_COST * (1.0 - ALLY_DISCOUNT_PER_LEVEL * level as f64)
}

fn new_negotiation(
    reg: &mut Registry,
    w: &War,
    kind: i64,
    (owner1, owner2): (i64, i64),
    isk: f64,
    message: &str,
    now: BlueTime,
) -> Negotiation {
    let n = Negotiation {
        id: war::take_id(reg.deltas_mut(), "neg"),
        war_id: w.war_id,
        kind,
        owner1,
        owner2,
        declared_by: w.declared_by,
        against: w.against,
        isk,
        description: message.chars().take(1000).collect(),
        state: STATE_NEW,
        created: now,
        accepted: None,
        declined: None,
        retracted: None,
    };
    put_negotiation(reg.deltas_mut(), &n);
    n
}

fn clean_isk(isk: f64) -> Result<f64, Refusal> {
    if isk.is_finite() && isk >= 0.0 {
        Ok(isk)
    } else {
        Err(Refusal::Key("NotEnoughMoney"))
    }
}

/// `CreateWarAllyOffer(warID, iskValue, defenderID, message)`: the caller's
/// owner offers to fight for the defender.
pub fn create_ally_offer(
    reg: &mut Registry,
    actor: i64,
    war_id: i64,
    isk: f64,
    defender: i64,
    message: &str,
) -> Result<i64, Refusal> {
    let me = director_owner(reg, actor)?;
    let now = crate::pyutil::filetime_now();
    let w = load_war_or(reg, war_id)?;
    let isk = clean_isk(isk)?;
    if w.mutual {
        return Err(Refusal::Key("CantHaveAlliesInMutualWars"));
    }
    if !w.open_for_allies {
        return Err(Refusal::Key("CannotAllyWarIsNotOpen"));
    }
    if !w.active(now) || defender != w.against {
        return Err(Refusal::Key("CanNotChangeEndedWar"));
    }
    if me == w.declared_by || me == w.against {
        return Err(Refusal::With(
            "AlreadyAtWar",
            vec![
                ("ally", PyValue::Int(me)),
                ("aggressor", PyValue::Int(w.declared_by)),
            ],
        ));
    }
    if w.allies.get(&me).is_some_and(|a| now < a.time_finished) {
        return Err(Refusal::Key("AlreadyAnAlly"));
    }
    if Wars::load(reg.deltas())
        .between(me, w.declared_by, now)
        .is_some()
    {
        return Err(Refusal::With(
            "AlreadyAtWar",
            vec![
                ("ally", PyValue::Int(me)),
                ("aggressor", PyValue::Int(w.declared_by)),
            ],
        ));
    }
    if negotiations(reg.deltas())
        .iter()
        .any(|n| n.war_id == war_id && n.kind == NEG_ALLY_OFFER && n.state == STATE_NEW)
    {
        return Err(Refusal::Key("WarAllyNegotiationAlreadyActive"));
    }
    if !war::permit(reg, me) {
        return Err(Refusal::Key("AllyHasNoWarPermit"));
    }
    let n = new_negotiation(reg, &w, NEG_ALLY_OFFER, (me, w.against), isk, message, now);
    crate::war_notice::ally_offered(reg, &w, &n, actor);
    Ok(n.id)
}

/// Load a negotiation that is still open and of `kind`, or the refusal for
/// why it is not.
fn open_negotiation(
    reg: &Registry,
    id: i64,
    kind: i64,
    accepted_key: &'static str,
    declined_key: &'static str,
) -> Result<Negotiation, Refusal> {
    let n = negotiation(reg.deltas(), id).filter(|n| n.kind == kind);
    let n = n.ok_or(Refusal::Key("CanNotChangeEndedWar"))?;
    match n.state {
        STATE_NEW => Ok(n),
        STATE_ACCEPTED => Err(Refusal::Key(accepted_key)),
        _ => Err(Refusal::Key(declined_key)),
    }
}

/// `RetractWarAllyOffer(negID)`: only the offerer.
pub fn retract_ally_offer(reg: &mut Registry, actor: i64, id: i64) -> Result<(), Refusal> {
    let me = director_owner(reg, actor)?;
    let mut n = open_negotiation(
        reg,
        id,
        NEG_ALLY_OFFER,
        "AllyOfferAlreadyAccepted",
        "AllyOfferAlreadyDeclined",
    )?;
    if n.owner1 != me {
        return Err(Refusal::Key("CanNotChangeEndedWar"));
    }
    n.state = STATE_RETRACTED;
    n.retracted = Some(crate::pyutil::filetime_now());
    put_negotiation(reg.deltas_mut(), &n);
    if let Some(w) = war::load_war(reg.deltas(), n.war_id) {
        crate::war_notice::ally_offer_retracted(reg, &w, &n, actor);
    }
    Ok(())
}

/// `DeclineAllyOffer(negID)`: only the receiver.
pub fn decline_ally_offer(reg: &mut Registry, actor: i64, id: i64) -> Result<(), Refusal> {
    let me = director_owner(reg, actor)?;
    let mut n = open_negotiation(
        reg,
        id,
        NEG_ALLY_OFFER,
        "AllyOfferAlreadyAccepted",
        "AllyOfferAlreadyDeclined",
    )?;
    if n.owner2 != me {
        return Err(Refusal::Key("CanNotChangeEndedWar"));
    }
    n.state = STATE_DECLINED;
    n.declined = Some(crate::pyutil::filetime_now());
    put_negotiation(reg.deltas_mut(), &n);
    if let Some(w) = war::load_war(reg.deltas(), n.war_id) {
        crate::war_notice::ally_offer_declined(reg, &w, &n, actor);
    }
    Ok(())
}

/// `AcceptAllyNegotiation(negID)`: the defender pays the ally its price
/// (ref 114) and CONCORD its fee ([`ally_cost_to_concord`], ref 49), and the
/// ally fights from [`ALLY_SPOOLUP`] on, for [`ALLY_COMMITMENT_PERIOD`] or to
/// the end of the war.
pub fn accept_ally_offer(reg: &mut Registry, actor: i64, id: i64) -> Result<(), Refusal> {
    let me = director_owner(reg, actor)?;
    let now = crate::pyutil::filetime_now();
    let mut n = open_negotiation(
        reg,
        id,
        NEG_ALLY_OFFER,
        "AllyOfferAlreadyAccepted",
        "AllyOfferAlreadyDeclined",
    )?;
    if n.owner2 != me {
        return Err(Refusal::Key("CanNotChangeEndedWar"));
    }
    let mut w = load_war_or(reg, n.war_id)?;
    if !w.active(now) {
        return Err(Refusal::Key("CanNotChangeEndedWar"));
    }
    let fee = ally_cost_to_concord(&w, ally_base_cost(reg, actor), now);
    let total = n.isk + fee;
    let balance = war::balance_of(reg, me);
    if balance < total {
        return Err(war::corp_money(
            reg,
            me,
            Refusal::Money {
                amount: total,
                balance,
            },
        ));
    }
    let before = w.clone();
    if n.isk > 0.0 {
        war::pay(reg, me, n.isk, REF_WAR_ALLY_CONTRACT, n.owner1, w.war_id);
        war::receive(reg, n.owner1, n.isk, REF_WAR_ALLY_CONTRACT, me, w.war_id);
    }
    if fee > 0.0 {
        war::pay(reg, me, fee, REF_WAR_FEE, CONCORD_OWNER, w.war_id);
    }
    let started = now.saturating_add(span(reg, ALLY_SPOOLUP));
    let commit = now.saturating_add(span(reg, ALLY_COMMITMENT_PERIOD));
    let finish = w.time_finished.map_or(commit, |f| f.min(commit));
    w.allies.insert(
        n.owner1,
        Ally {
            ally_id: n.owner1,
            time_started: started,
            time_finished: finish.max(started),
            told: false,
        },
    );
    n.state = STATE_ACCEPTED;
    n.accepted = Some(now);
    put_negotiation(reg.deltas_mut(), &n);
    war::put(reg.deltas_mut(), &w);
    war::tell_change(reg, Some(&before), &w, Via::Call);
    crate::war_notice::ally_joined(reg, &w, &n, actor, started, total);
    Ok(())
}

/// `CreateSurrenderNegotiation(warID, iskValue, message)`.
pub fn create_surrender(
    reg: &mut Registry,
    actor: i64,
    war_id: i64,
    isk: f64,
    message: &str,
) -> Result<i64, Refusal> {
    let me = in_charge(reg, actor)?;
    let now = crate::pyutil::filetime_now();
    let w = load_war_or(reg, war_id)?;
    let isk = clean_isk(isk)?;
    if w.mutual {
        return Err(Refusal::Key("CanNotChangeMutualWarWrongParty"));
    }
    if me != w.declared_by && me != w.against {
        return Err(Refusal::Key("CanNotSurenderWarYouAreNotIn"));
    }
    if w.retracted.is_some() {
        return Err(Refusal::Key("CantOfferSurrenderWarRetracted"));
    }
    if !w.active(now) || w.time_finished.is_some() {
        return Err(Refusal::Key("CanNotSurrenderAsWarIsOver"));
    }
    if negotiations(reg.deltas())
        .iter()
        .any(|n| n.war_id == war_id && n.kind == NEG_SURRENDER_OFFER && n.state == STATE_NEW)
    {
        return Err(Refusal::Key("WarSurrenderNegotiationAlreadyActive"));
    }
    let other = if me == w.declared_by {
        w.against
    } else {
        w.declared_by
    };
    let n = new_negotiation(reg, &w, NEG_SURRENDER_OFFER, (me, other), isk, message, now);
    crate::war_notice::surrender_offered(reg, &w, &n, actor);
    Ok(n.id)
}

/// `AcceptSurrender(negID)`: the receiver's CEO accepts; the offerer pays
/// (ref 113); the war is retracted into its cooldown with an `ENDED_SURRENDER`
/// and both sides are forbidden to declare on each other for two weeks.
pub fn accept_surrender(reg: &mut Registry, actor: i64, id: i64) -> Result<(), Refusal> {
    let me = in_charge(reg, actor)?;
    let now = crate::pyutil::filetime_now();
    let mut n = open_negotiation(
        reg,
        id,
        NEG_SURRENDER_OFFER,
        "CanNotChangeEndedWar",
        "SurrenderOfferAlreadyDeclined",
    )?;
    if n.owner2 != me {
        return Err(Refusal::Key("CanNotChangeEndedWar"));
    }
    let w = load_war_or(reg, n.war_id)?;
    if !w.active(now) || w.time_finished.is_some() {
        return Err(Refusal::Key("CanNotSurrenderAsWarIsOver"));
    }
    if war::balance_of(reg, n.owner1) < n.isk {
        let executor = war::payer_of(reg, n.owner1).unwrap_or(n.owner1);
        return Err(Refusal::With(
            "CantAcceptSurrenderNotEnoughMoney",
            vec![
                ("executor", PyValue::Int(executor)),
                ("iskValue", PyValue::Real(n.isk)),
            ],
        ));
    }
    if n.isk > 0.0 {
        war::pay(reg, n.owner1, n.isk, REF_WAR_SURRENDER_FEE, me, w.war_id);
        war::receive(reg, me, n.isk, REF_WAR_SURRENDER_FEE, n.owner1, w.war_id);
    }
    n.state = STATE_ACCEPTED;
    n.accepted = Some(now);
    put_negotiation(reg.deltas_mut(), &n);
    war::finish(
        reg,
        w.war_id,
        FinishWith {
            reason: ended::SURRENDER,
            by: Some(me),
            peace: Some(peace::WAR_SURRENDER),
            peace_both: true,
            retract: true,
            ..FinishWith::default()
        },
        now,
        Via::Call,
    );
    crate::war_notice::surrender_accepted(reg, &w, &n, actor);
    Ok(())
}

/// `DeclineSurrender(negID)`.
pub fn decline_surrender(reg: &mut Registry, actor: i64, id: i64) -> Result<(), Refusal> {
    let me = in_charge(reg, actor)?;
    let mut n = open_negotiation(
        reg,
        id,
        NEG_SURRENDER_OFFER,
        "CanNotChangeEndedWar",
        "SurrenderOfferAlreadyDeclined",
    )?;
    if n.owner2 != me {
        return Err(Refusal::Key("CanNotChangeEndedWar"));
    }
    n.state = STATE_DECLINED;
    n.declined = Some(crate::pyutil::filetime_now());
    put_negotiation(reg.deltas_mut(), &n);
    if let Some(w) = war::load_war(reg.deltas(), n.war_id) {
        crate::war_notice::surrender_declined(reg, &w, &n, actor);
    }
    Ok(())
}

/// `SetOpenForAllies(warID, state)`: only the defender's CEO.
pub fn set_open_for_allies(
    reg: &mut Registry,
    actor: i64,
    war_id: i64,
    state: bool,
) -> Result<(), Refusal> {
    let me = in_charge(reg, actor)?;
    let now = crate::pyutil::filetime_now();
    let mut w = load_war_or(reg, war_id)?;
    if w.against != me {
        return Err(Refusal::Key("CanNotChangeMutualWarWrongParty"));
    }
    if w.mutual {
        return Err(Refusal::Key("CantHaveAlliesInMutualWars"));
    }
    if !w.active(now) {
        return Err(Refusal::Key("CanNotChangeEndedWar"));
    }
    if w.open_for_allies == state {
        return Ok(());
    }
    let before = w.clone();
    w.open_for_allies = state;
    war::put(reg.deltas_mut(), &w);
    war::tell_change(reg, Some(&before), &w, Via::Call);
    Ok(())
}

/// `RetractMutualWar(warID)`: either principal's CEO ends a mutual war into
/// its cooldown.
pub fn retract_mutual_war(reg: &mut Registry, actor: i64, war_id: i64) -> Result<(), Refusal> {
    let me = in_charge(reg, actor)?;
    let now = crate::pyutil::filetime_now();
    let w = load_war_or(reg, war_id)?;
    if !w.mutual || (me != w.declared_by && me != w.against) {
        return Err(Refusal::Key("CanNotChangeMutualWarWrongParty"));
    }
    if !w.active(now) || w.time_finished.is_some() {
        return Err(Refusal::Key("CanNotChangeEndedWar"));
    }
    war::finish(
        reg,
        war_id,
        FinishWith {
            reason: ended::RETRACTED,
            by: Some(me),
            retract: true,
            ..FinishWith::default()
        },
        now,
        Via::Call,
    );
    crate::war_notice::mutual_retracted(reg, &w, actor);
    Ok(())
}

// ---------------------------------------------------------------------------
// Mutual-war invites
// ---------------------------------------------------------------------------

/// One invite **[V `warMutalWarInvite`]**: `KeyVal(fromOwnerID, toOwnerID,
/// sentDate)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invite {
    pub from: i64,
    pub to: i64,
    pub sent: BlueTime,
}

impl Invite {
    pub fn value(&self) -> PyValue {
        crate::pyutil::keyval(vec![
            ("fromOwnerID", PyValue::Int(self.from)),
            ("toOwnerID", PyValue::Int(self.to)),
            ("sentDate", PyValue::Int(self.sent)),
        ])
    }
}

/// Every invite.
pub fn invites(d: &Deltas) -> Vec<Invite> {
    d.of(KIND_INVITE)
        .iter()
        .filter_map(|(k, v)| {
            (k.len() == 2).then_some(())?;
            Some(Invite {
                from: num(k, 0).ok()?,
                to: num(k, 1).ok()?,
                sent: num(v, 0).ok()?,
            })
        })
        .collect()
}

/// The invites that touch a pilot's corp or alliance.
pub fn invites_for(reg: &Registry, actor: i64) -> Vec<Invite> {
    let mine = my_owners(reg, actor);
    invites(reg.deltas())
        .into_iter()
        .filter(|i| mine.contains(&i.from) || mine.contains(&i.to))
        .collect()
}

/// Whether `owner` refuses mutual-war invites.
pub fn invites_blocked(d: &Deltas, owner: i64) -> bool {
    d.of(KIND_INVITE).contains_key(&vec![owner.to_string()])
}

/// `SetInvitesBlockedByPlayer(bool)`.
pub fn set_invites_blocked(reg: &mut Registry, actor: i64, blocked: bool) -> Result<(), Refusal> {
    let me = director_owner(reg, actor)?;
    if blocked {
        reg.deltas_mut()
            .put(KIND_INVITE, vec![me.to_string()], vec!["1".to_owned()]);
    } else {
        reg.deltas_mut().del(KIND_INVITE, &[me.to_string()]);
    }
    Ok(())
}

fn tell_invites(reg: &mut Registry, owners: &[i64], via: Via) {
    let mut to = Vec::new();
    for o in owners {
        to.extend(war::owner_characters(reg, *o));
    }
    to.sort_unstable();
    to.dedup();
    if !to.is_empty() {
        war::emit(
            reg,
            via,
            war::Notice {
                to,
                event: "OnMutualWarsUpdated_Remote",
                args: vec![],
            },
        );
    }
}

/// `SendInviteByPlayer(toOwnerID)`.
pub fn send_invite(reg: &mut Registry, actor: i64, to: i64) -> Result<(), Refusal> {
    let me = director_owner(reg, actor)?;
    let now = crate::pyutil::filetime_now();
    if to == me {
        return Err(Refusal::Key("CanNotDeclareWarAgainstSelf"));
    }
    if war::is_npc_corporation(to) {
        return Err(Refusal::Key("CrpCantDeclareWarOnNPCCorp"));
    }
    if !war::owner_exists(reg, to) {
        return Err(Refusal::Key("CanNotDeclareWarAgainstThem"));
    }
    if !war::is_alliance_id(to) && crate::alliance::of_corp(reg.deltas(), to).is_some() {
        return Err(Refusal::Key("CrpCanNotSanctionWarAgainstOwner"));
    }
    if !war::permit(reg, me) || !war::permit(reg, to) {
        return Err(Refusal::Key("CanNotDeclareWarAgainstThem"));
    }
    if Wars::load(reg.deltas()).between(me, to, now).is_some() {
        return Err(Refusal::Key("AlreadyAtWarWithThem"));
    }
    if invites_blocked(reg.deltas(), to) {
        return Err(Refusal::Key("CannotSendMutualWarInviteBlocked"));
    }
    let all = invites(reg.deltas());
    if all
        .iter()
        .any(|i| (i.from == me && i.to == to) || (i.from == to && i.to == me))
    {
        return Err(Refusal::Key("CannotSendMutualWarInviteAlreadyExists"));
    }
    if all.iter().filter(|i| i.from == me).count() >= MAX_OUTGOING_INVITES {
        return Err(Refusal::Key("CannotSendMutualWarInviteTooManyOutgoing"));
    }
    reg.deltas_mut().put(
        KIND_INVITE,
        vec![me.to_string(), to.to_string()],
        vec![now.to_string()],
    );
    tell_invites(reg, &[me, to], Via::Call);
    let expires = now.saturating_add(span(reg, MUTUAL_INVITE_EXPIRY));
    crate::war_notice::invite_sent(reg, me, to, expires);
    Ok(())
}

/// `WithdrawInviteByPlayer(toOwnerID)`.
pub fn withdraw_invite(reg: &mut Registry, actor: i64, to: i64) -> Result<(), Refusal> {
    let me = director_owner(reg, actor)?;
    if reg
        .deltas_mut()
        .del(KIND_INVITE, &[me.to_string(), to.to_string()])
        .is_some()
    {
        tell_invites(reg, &[me, to], Via::Call);
    }
    Ok(())
}

/// `RespondToInviteByPlayer(fromOwnerID, accepts)`: accepting makes a mutual
/// war that spools up for [`WAR_SPOOLUP`] **from the acceptance** **[V-data]**.
pub fn respond_to_invite(
    reg: &mut Registry,
    actor: i64,
    from: i64,
    accepts: bool,
) -> Result<(), Refusal> {
    let me = director_owner(reg, actor)?;
    let now = crate::pyutil::filetime_now();
    let key = vec![from.to_string(), me.to_string()];
    if !reg.deltas().of(KIND_INVITE).contains_key(&key) {
        return Err(Refusal::Key("CanNotChangeEndedWar"));
    }
    reg.deltas_mut().del(KIND_INVITE, &key);
    if accepts {
        if Wars::load(reg.deltas()).between(from, me, now).is_some() {
            tell_invites(reg, &[from, me], Via::Call);
            return Err(Refusal::Key("AlreadyAtWarWithThem"));
        }
        let id = war::take_id(reg.deltas_mut(), "war");
        let mut w = War::new(id, from, me, now, span(reg, WAR_SPOOLUP), None);
        w.mutual = true;
        war::put(reg.deltas_mut(), &w);
        war::tell_change(reg, None, &w, Via::Call);
        crate::war_notice::mutual_accepted(reg, &w);
    } else {
        crate::war_notice::mutual_rejected(reg, from, me);
    }
    tell_invites(reg, &[from, me], Via::Call);
    Ok(())
}

// ---------------------------------------------------------------------------
// Bills (C8-d)
// ---------------------------------------------------------------------------

/// One war bill **[V2 columns; V reads `billTypeID`, `paid`]**.
#[derive(Debug, Clone, PartialEq)]
pub struct Bill {
    pub id: i64,
    pub debtor: i64,
    pub creditor: i64,
    pub amount: f64,
    pub due: BlueTime,
    pub paid: bool,
    pub paid_at: Option<BlueTime>,
    pub war_id: i64,
    pub against: i64,
}

impl Bill {
    fn to_row(&self) -> Vec<String> {
        vec![
            self.debtor.to_string(),
            self.creditor.to_string(),
            format!("{}", self.amount),
            self.due.to_string(),
            u8::from(self.paid).to_string(),
            id_field(self.paid_at),
            self.war_id.to_string(),
            self.against.to_string(),
        ]
    }

    fn from_row(id: i64, v: &[String]) -> Result<Bill, String> {
        Ok(Bill {
            id,
            debtor: num(v, 0)?,
            creditor: num(v, 1)?,
            amount: v
                .get(2)
                .ok_or("amount is missing")?
                .parse::<f64>()
                .map_err(|e| e.to_string())?,
            due: num(v, 3)?,
            paid: num(v, 4)? != 0,
            paid_at: opt(v, 5)?,
            war_id: num(v, 6)?,
            against: num(v, 7)?,
        })
    }

    /// The bill as the wallet's Bills tab reads it: `billID`, `billTypeID`,
    /// `amount`, `interest`, `debtorID`, `creditorID`, `dueDateTime`, `paid`,
    /// `externalID`, `externalID2` **[V2 names; V reads the first six, with
    /// `externalID` the war's opponent]**.
    pub fn value(&self) -> PyValue {
        crate::pyutil::keyval(vec![
            ("billID", PyValue::Int(self.id)),
            ("billTypeID", PyValue::Int(BILL_TYPE_WAR)),
            ("amount", PyValue::Real(self.amount)),
            ("interest", PyValue::Real(0.0)),
            ("debtorID", PyValue::Int(self.debtor)),
            ("creditorID", PyValue::Int(self.creditor)),
            ("dueDateTime", PyValue::Int(self.due)),
            ("paid", PyValue::Bool(self.paid)),
            ("externalID", PyValue::Int(self.against)),
            ("externalID2", PyValue::Int(self.war_id)),
        ])
    }
}

/// One bill.
pub fn bill(d: &Deltas, id: i64) -> Option<Bill> {
    Bill::from_row(id, d.of(KIND_BILL).get(&vec![id.to_string()])?).ok()
}

/// Every bill, in id order.
pub fn bills(d: &Deltas) -> Vec<Bill> {
    d.of(KIND_BILL)
        .iter()
        .filter_map(|(k, v)| Bill::from_row(num(k, 0).ok()?, v).ok())
        .collect()
}

fn put_bill(d: &mut Deltas, b: &Bill) {
    d.put(KIND_BILL, vec![b.id.to_string()], b.to_row());
}

/// Issue a bill for `w`'s next week, due at `due`; tells the debtor.
fn issue_bill(reg: &mut Registry, w: &War, due: BlueTime) -> i64 {
    let b = Bill {
        id: war::take_id(reg.deltas_mut(), "bill"),
        debtor: w.declared_by,
        creditor: CONCORD_OWNER,
        amount: COST_OF_WAR,
        due,
        paid: false,
        paid_at: None,
        war_id: w.war_id,
        against: w.against,
    };
    put_bill(reg.deltas_mut(), &b);
    crate::war_notice::bill_issued(reg, &b, crate::pyutil::filetime_now());
    let to = war::owner_characters(reg, w.declared_by);
    if !to.is_empty() {
        war::emit(
            reg,
            Via::Call,
            war::Notice {
                to,
                event: "OnBillReceived",
                args: vec![PyValue::Int(b.id)],
            },
        );
    }
    b.id
}

/// **At declaration**: the fee paid week one, and the bill for week two is
/// sent at once **[V-data `WarDeclareCost`]**, due a billing period after the
/// declaration. A mutual war has none.
pub fn issue_first_bill(reg: &mut Registry, w: &War, now: BlueTime) -> Option<i64> {
    if w.mutual {
        return None;
    }
    let due = now.saturating_add(span(reg, BILLING_PERIOD));
    Some(issue_bill(reg, w, due))
}

/// Whether `owner` pays its war bills when they fall due.
pub fn autopay(d: &Deltas, owner: i64) -> bool {
    d.of(KIND_AUTO)
        .get(&vec![owner.to_string()])
        .and_then(|v| v.first())
        .map_or(AUTOPAY_DEFAULT, |s| s == "1")
}

/// Pay a bill now: the debtor's wallet to CONCORD (ref 49), then the next
/// week's bill if the war still runs. `false` when the debtor cannot pay.
pub fn pay_bill(reg: &mut Registry, id: i64, now: BlueTime) -> bool {
    let Some(mut b) = bill(reg.deltas(), id).filter(|b| !b.paid) else {
        return false;
    };
    if !war::pay(reg, b.debtor, b.amount, REF_WAR_FEE, b.creditor, b.war_id) {
        return false;
    }
    b.paid = true;
    b.paid_at = Some(now);
    put_bill(reg.deltas_mut(), &b);
    if let Some(mut w) = war::load_war(reg.deltas(), b.war_id)
        && w.time_finished.is_none()
    {
        let next = issue_bill(reg, &w, b.due.saturating_add(span(reg, BILLING_PERIOD)));
        w.bill_id = Some(next);
        war::put(reg.deltas_mut(), &w);
    }
    true
}

/// `PayBill(billID, fromAccountKey)` for a pilot: an Accountant of the debtor
/// owner.
pub fn pay_bill_as(reg: &mut Registry, actor: i64, id: i64) -> Result<(), Refusal> {
    let b = bill(reg.deltas(), id).ok_or(Refusal::Key("CanNotChangeEndedWar"))?;
    if !my_owners(reg, actor).contains(&b.debtor) {
        return Err(Refusal::Access(
            "Only an accountant of the paying corporation can pay a bill.",
        ));
    }
    let member = reg.corporations().membership(actor);
    let accountant = member.is_some_and(|m| {
        m.roles & (crate::war_deal::ROLE_ACCOUNTANT | crate::corp_roles::CORP_ROLE_DIRECTOR) != 0
    }) || reg
        .corporations()
        .get(reg.character(actor).map_or(0, |c| c.corporation_id))
        .is_some_and(|c| c.ceo_id == actor);
    if !accountant {
        return Err(Refusal::Access(
            "Only an accountant of the paying corporation can pay a bill.",
        ));
    }
    if b.paid {
        return Ok(());
    }
    let now = crate::pyutil::filetime_now();
    if pay_bill(reg, id, now) {
        Ok(())
    } else {
        Err(war::corp_money(
            reg,
            b.debtor,
            Refusal::Money {
                amount: b.amount,
                balance: war::balance_of(reg, b.debtor),
            },
        ))
    }
}

/// `appConst.corpRoleAccountant` **[V]**.
pub const ROLE_ACCOUNTANT: i64 = 256;

/// The bills an owner owes (paid ones too), newest first.
pub fn bills_of(d: &Deltas, owners: &[i64]) -> Vec<Bill> {
    let mut v: Vec<Bill> = bills(d)
        .into_iter()
        .filter(|b| owners.contains(&b.debtor))
        .collect();
    v.sort_by(|a, b| b.id.cmp(&a.id));
    v
}

// ---------------------------------------------------------------------------
// When a war ends, the world around it
// ---------------------------------------------------------------------------

/// After a war ended: its open offers lapse, and its unpaid bills are
/// cancelled.
pub fn war_ended(reg: &mut Registry, w: &War) {
    let now = crate::pyutil::filetime_now();
    for mut n in negotiations(reg.deltas()) {
        if n.war_id == w.war_id && n.state == STATE_NEW {
            n.state = STATE_RETRACTED;
            n.retracted = Some(now);
            put_negotiation(reg.deltas_mut(), &n);
        }
    }
    for b in bills(reg.deltas()) {
        if b.war_id == w.war_id && !b.paid {
            reg.deltas_mut().del(KIND_BILL, &[b.id.to_string()]);
        }
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// What the war clock owes the deals: offers older than a day lapse, invites
/// older than a week expire, treaties past their term go, and bills that
/// fell due are paid or end their war.
pub fn tick(reg: &mut Registry, now: BlueTime) {
    let d = reg.deltas();
    if [KIND_NEG, KIND_INVITE, KIND_PEACE, KIND_BILL]
        .iter()
        .all(|k| d.of(k).is_empty())
    {
        return;
    }
    expire_negotiations(reg, now);
    expire_invites(reg, now);
    expire_treaties(reg, now);
    settle_bills(reg, now);
}

fn expire_negotiations(reg: &mut Registry, now: BlueTime) {
    let lapse = span(reg, NEGOTIATION_EXPIRY);
    let due: Vec<Negotiation> = reg
        .deltas()
        .of(KIND_NEG)
        .iter()
        .filter_map(|(k, v)| Negotiation::from_row(num(k, 0).ok()?, v).ok())
        .filter(|n| n.state == STATE_NEW && n.created.saturating_add(lapse) < now)
        .collect();
    for mut n in due {
        n.state = STATE_RETRACTED;
        n.retracted = Some(now);
        put_negotiation(reg.deltas_mut(), &n);
    }
    // A settled negotiation is kept as long as a finished war is, then goes.
    let old: Vec<i64> = reg
        .deltas()
        .of(KIND_NEG)
        .iter()
        .filter_map(|(k, v)| Negotiation::from_row(num(k, 0).ok()?, v).ok())
        .filter(|n| n.state != STATE_NEW && n.created.saturating_add(war::KEEP_FINISHED) < now)
        .map(|n| n.id)
        .collect();
    for id in old {
        reg.deltas_mut().del(KIND_NEG, &[id.to_string()]);
    }
}

fn expire_invites(reg: &mut Registry, now: BlueTime) {
    let lapse = span(reg, MUTUAL_INVITE_EXPIRY);
    let due: Vec<Invite> = invites(reg.deltas())
        .into_iter()
        .filter(|i| i.sent.saturating_add(lapse) < now)
        .collect();
    for i in due {
        reg.deltas_mut()
            .del(KIND_INVITE, &[i.from.to_string(), i.to.to_string()]);
        crate::war_notice::invite_expired(reg, i.from, i.to);
        tell_invites(reg, &[i.from, i.to], Via::Clock);
    }
}

/// Treaties past their term go. The world is only marked dirty when one
/// does: this runs every tick.
fn expire_treaties(reg: &mut Registry, now: BlueTime) {
    let gone: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND_PEACE)
        .iter()
        .filter(|(_, v)| num(v, 0).is_ok_and(|e| e <= now))
        .map(|(k, _)| k.clone())
        .collect();
    for k in gone {
        reg.deltas_mut().del(KIND_PEACE, &k);
    }
}

/// Every unpaid bill past its due time: pay it (if the owner pays by itself
/// and can), or end its war with `ENDED_UNPAID_BILL`.
fn settle_bills(reg: &mut Registry, now: BlueTime) {
    // A paid bill is kept as long as a finished war is, then goes.
    let old: Vec<i64> = reg
        .deltas()
        .of(KIND_BILL)
        .iter()
        .filter_map(|(k, v)| Bill::from_row(num(k, 0).ok()?, v).ok())
        .filter(|b| {
            b.paid
                && b.paid_at
                    .unwrap_or(b.due)
                    .saturating_add(war::KEEP_FINISHED)
                    < now
        })
        .map(|b| b.id)
        .collect();
    for id in old {
        reg.deltas_mut().del(KIND_BILL, &[id.to_string()]);
    }
    let due: Vec<Bill> = reg
        .deltas()
        .of(KIND_BILL)
        .iter()
        .filter_map(|(k, v)| Bill::from_row(num(k, 0).ok()?, v).ok())
        .filter(|b| !b.paid && b.due <= now)
        .collect();
    for b in due {
        let auto = autopay(reg.deltas(), b.debtor);
        if auto && pay_bill(reg, b.id, now) {
            continue;
        }
        crate::war_notice::bill_unpaid(reg, &b);
        war::finish(
            reg,
            b.war_id,
            FinishWith {
                reason: ended::UNPAID_BILL,
                at: None,
                peace: Some(peace::UNPAID_BILL),
                ..FinishWith::default()
            },
            now,
            Via::Clock,
        );
        // The war may already have been over: the bill goes either way.
        reg.deltas_mut().del(KIND_BILL, &[b.id.to_string()]);
    }
}

// ---------------------------------------------------------------------------
// Alliances and corporations around a war (R-WAR §9)
// ---------------------------------------------------------------------------

/// May `corp` apply to (`at_accept` false) or join (`at_accept` true)
/// `alliance`? Not with wars it declared, not at war with it, not with a
/// mutual war running **[V keys]**.
pub fn may_join_alliance(
    reg: &Registry,
    corp: i64,
    alliance: i64,
    at_accept: bool,
) -> Result<(), &'static str> {
    let now = crate::pyutil::filetime_now();
    let wars = Wars::load(reg.deltas());
    let mine = || wars.of_owner(corp).filter(|w| w.active(now));
    if mine().any(|w| w.mutual) {
        return Err(if at_accept {
            "CanNotJoinAllianceWithActiveMututalWars"
        } else {
            "CantApplyToAllianceHaveMutualWars"
        });
    }
    if mine().any(|w| w.declared_by == corp) {
        return Err("CantApplyToAllianceHaveDeclaredWars");
    }
    if wars.between(corp, alliance, now).is_some() {
        return Err("CantApplyToAllianceYouAreAtWarWith");
    }
    Ok(())
}

/// **After a corporation joined an alliance**: the wars declared on the corp
/// are adopted by the alliance (`STARTED_ADOPTION`; the corp's copy ends
/// `ENDED_WAR_ADOPTED` with no cooldown).
pub fn on_corp_joined_alliance(reg: &mut Registry, corp: i64, alliance: i64) {
    let now = crate::pyutil::filetime_now();
    let adopt: Vec<War> = Wars::load(reg.deltas())
        .all()
        .filter(|w| w.against == corp && !w.mutual && w.active(now) && w.time_finished.is_none())
        .cloned()
        .collect();
    for old in adopt {
        let id = war::take_id(reg.deltas_mut(), "war");
        let mut copy = old.clone();
        copy.war_id = id;
        copy.against = alliance;
        copy.created_from = Some(old.war_id);
        copy.reason_started = war::started::ADOPTION;
        copy.reason_ended = None;
        copy.notified = old.notified;
        // The attacker's bill goes with the war: an unpaid one is re-homed
        // before `finish`, so `war_ended` leaves it; a paid or missing one
        // gets a fresh bill (a mutual war has none) **[A: EvEJS does this]**.
        let old_bill = old.bill_id.and_then(|id| bill(reg.deltas(), id));
        match old_bill {
            Some(mut b) if !b.paid => {
                b.war_id = copy.war_id;
                b.against = alliance;
                put_bill(reg.deltas_mut(), &b);
                copy.bill_id = Some(b.id);
            }
            other => {
                let due = other
                    .map(|b| b.due)
                    .unwrap_or_else(|| now.saturating_add(span(reg, BILLING_PERIOD)));
                copy.bill_id = (!copy.mutual).then(|| issue_bill(reg, &copy, due));
            }
        }
        war::put(reg.deltas_mut(), &copy);
        war::tell_change(reg, None, &copy, Via::Call);
        war::finish(
            reg,
            old.war_id,
            FinishWith {
                reason: ended::WAR_ADOPTED,
                at: Some(now),
                ..FinishWith::default()
            },
            now,
            Via::Call,
        );
        crate::war_notice::adopted(reg, &copy, alliance);
    }
}

/// **After a corporation left an alliance**: the alliance's wars are
/// inherited by the leaver (`STARTED_INHERITED`, same dates), except mutual
/// ones **[A]**.
pub fn on_corp_left_alliance(reg: &mut Registry, corp: i64, alliance: i64) {
    let now = crate::pyutil::filetime_now();
    let inherit: Vec<War> = Wars::load(reg.deltas())
        .of_owner(alliance)
        .filter(|w| !w.mutual && w.active(now) && w.time_finished.is_none())
        .filter(|w| w.declared_by == alliance || w.against == alliance)
        .cloned()
        .collect();
    for old in inherit {
        let id = war::take_id(reg.deltas_mut(), "war");
        let mut copy = old.clone();
        copy.war_id = id;
        if copy.declared_by == alliance {
            copy.declared_by = corp;
        } else {
            copy.against = corp;
        }
        copy.created_from = Some(old.war_id);
        copy.reason_started = war::started::INHERITED;
        copy.reason_ended = None;
        copy.bill_id = None;
        if copy.declared_by == corp {
            // The leaver is now the aggressor and owes the bill, due where
            // the alliance's current cycle ends.
            let due = old
                .bill_id
                .and_then(|id| bill(reg.deltas(), id))
                .filter(|b| !b.paid)
                .map_or_else(|| now.saturating_add(span(reg, BILLING_PERIOD)), |b| b.due);
            copy.bill_id = Some(issue_bill(reg, &copy, due));
            // [A: an inherited aggressor war needs the leaver's own war HQ;
            // falsifier: a TQ capture where it survives without one.]
            if let Some(hq) = war::hq_of(reg.deltas(), corp) {
                copy.war_hq = Some(hq.item_id);
            }
        }
        war::put(reg.deltas_mut(), &copy);
        war::tell_change(reg, None, &copy, Via::Call);
        let opponent = if old.declared_by == alliance {
            old.against
        } else {
            old.declared_by
        };
        crate::war_notice::inherited(reg, &copy, alliance, corp, opponent);
    }
}

/// A pilot left `corp` (or was kicked): if the corp or its alliance is at war,
/// remember it, so they cannot slip back in the same week
/// (`CharacterDodgedWar`).
pub fn on_member_left(reg: &mut Registry, character: i64, corp: i64) {
    let now = crate::pyutil::filetime_now();
    let wars = Wars::load(reg.deltas());
    let owners = [
        corp,
        crate::alliance::of_corp(reg.deltas(), corp).unwrap_or(corp),
    ];
    if wars
        .all()
        .any(|w| w.active(now) && owners.iter().any(|o| w.involves(*o)))
    {
        let until = now.saturating_add(span(reg, DODGE_PERIOD));
        reg.deltas_mut().put(
            KIND_LEFT,
            vec![character.to_string()],
            vec![corp.to_string(), until.to_string()],
        );
    }
}

/// May `character` join `corp`? Not within a week of leaving a corp at war,
/// unless the wars are over **[V key `CharacterDodgedWar`]**.
pub fn may_rejoin(reg: &Registry, character: i64, corp: i64) -> Result<(), &'static str> {
    let now = crate::pyutil::filetime_now();
    let Some(v) = reg.deltas().of(KIND_LEFT).get(&vec![character.to_string()]) else {
        return Ok(());
    };
    let (Ok(left), Ok(until)) = (num(v, 0), num(v, 1)) else {
        return Ok(());
    };
    if now >= until || left == corp {
        return Ok(());
    }
    let wars = Wars::load(reg.deltas());
    let owners = [
        left,
        crate::alliance::of_corp(reg.deltas(), left).unwrap_or(left),
    ];
    if wars
        .all()
        .any(|w| w.active(now) && owners.iter().any(|o| w.involves(*o)))
    {
        Err("CharacterDodgedWar")
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The GM's hands
// ---------------------------------------------------------------------------

/// `GMSetDeclareTime(warID, time)`: the war was declared at `time`, and
/// spools up from there.
pub fn gm_set_declare_time(reg: &mut Registry, war_id: i64, time: BlueTime) -> bool {
    let Some(mut w) = war::load_war(reg.deltas(), war_id) else {
        return false;
    };
    let before = w.clone();
    w.time_declared = time;
    w.time_started = time.saturating_add(span(reg, WAR_SPOOLUP));
    if w.time_started > crate::pyutil::filetime_now() {
        w.notified &= !war::TOLD_STARTED;
    }
    war::put(reg.deltas_mut(), &w);
    war::tell_change(reg, Some(&before), &w, Via::Call);
    true
}

/// `GMSetWarFinishTime(warID, time)`.
pub fn gm_set_finish_time(reg: &mut Registry, war_id: i64, time: BlueTime) -> bool {
    let Some(mut w) = war::load_war(reg.deltas(), war_id) else {
        return false;
    };
    let before = w.clone();
    w.time_finished = Some(time);
    w.reason_ended.get_or_insert(ended::GM_ACTION);
    w.notified &= !war::TOLD_FINISHED;
    war::put(reg.deltas_mut(), &w);
    war::tell_change(reg, Some(&before), &w, Via::Call);
    true
}

/// `GMJoinDefender(warID, entityID, warEntityID)`: `warEntityID` joins the
/// defender's side, pending.
pub fn gm_join_defender(reg: &mut Registry, war_id: i64, ally: i64) -> bool {
    let Some(mut w) = war::load_war(reg.deltas(), war_id) else {
        return false;
    };
    if ally == w.declared_by || ally == w.against {
        return false;
    }
    let before = w.clone();
    let now = crate::pyutil::filetime_now();
    let start = now.saturating_add(span(reg, ALLY_SPOOLUP));
    let end = start.saturating_add(span(reg, ALLY_COMMITMENT_PERIOD));
    w.allies.insert(
        ally,
        Ally {
            ally_id: ally,
            time_started: start,
            time_finished: w.time_finished.map_or(end, |f| f.min(end)).max(start),
            told: false,
        },
    );
    war::put(reg.deltas_mut(), &w);
    war::tell_change(reg, Some(&before), &w, Via::Call);
    true
}

/// `GMActivateDefender(warID, allyID)`: the ally fights now.
pub fn gm_activate_defender(reg: &mut Registry, war_id: i64, ally: i64) -> bool {
    gm_ally(reg, war_id, ally, |a, now, _| {
        a.time_started = now.saturating_sub(1)
    })
}

/// `GMDeactivateDefender(warID, allyID)`: the ally's contract ends now.
pub fn gm_deactivate_defender(reg: &mut Registry, war_id: i64, ally: i64) -> bool {
    gm_ally(reg, war_id, ally, |a, now, _| {
        a.time_finished = now.saturating_sub(1)
    })
}

/// `GMExtendAllyContract(warID, allyID, time)`: `time` more.
pub fn gm_extend_ally(reg: &mut Registry, war_id: i64, ally: i64, time: BlueTime) -> bool {
    gm_ally(reg, war_id, ally, |a, _, scale| {
        a.time_finished = a.time_finished.saturating_add(scale(time));
    })
}

fn gm_ally(
    reg: &mut Registry,
    war_id: i64,
    ally: i64,
    edit: impl FnOnce(&mut Ally, BlueTime, &dyn Fn(BlueTime) -> BlueTime),
) -> bool {
    let Some(mut w) = war::load_war(reg.deltas(), war_id) else {
        return false;
    };
    let before = w.clone();
    let now = crate::pyutil::filetime_now();
    let clock = crate::game_clock::of(reg);
    let Some(a) = w.allies.get_mut(&ally) else {
        return false;
    };
    edit(a, now, &|t| clock.span(t));
    war::put(reg.deltas_mut(), &w);
    war::tell_change(reg, Some(&before), &w, Via::Call);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGG: i64 = 98_000_001;
    const DEF: i64 = 98_000_002;

    /// `GetAllyCostToConcord`: free for the first ally, then the base doubling
    /// with each ally (the exponent capped at 20), counting only allies not
    /// yet finished.
    #[test]
    fn the_ally_fee_doubles_with_each_ally() {
        const BASE: f64 = 1_000.0;
        let mut w = War::new(1, AGG, DEF, 0, 0, None);
        let fee = |w: &War, now| ally_cost_to_concord(w, BASE, now);
        assert_eq!(fee(&w, 5), 0.0, "the first ally is free");
        let add = |w: &mut War, id: i64, until: i64| {
            w.allies.insert(
                id,
                Ally {
                    ally_id: id,
                    time_started: 0,
                    time_finished: until,
                    told: true,
                },
            );
        };
        add(&mut w, 98_100_001, 100);
        assert_eq!(fee(&w, 5), BASE);
        add(&mut w, 98_100_002, 100);
        assert_eq!(fee(&w, 5), 2.0 * BASE);
        add(&mut w, 98_100_003, 10);
        assert_eq!(fee(&w, 5), 4.0 * BASE);
        assert_eq!(
            fee(&w, 50),
            2.0 * BASE,
            "an ally past its contract does not count"
        );
        assert_eq!(fee(&w, 500), 0.0);
        // The cap: 2^20, not 2^40.
        for i in 0..40 {
            add(&mut w, 98_200_000 + i, 100);
        }
        assert_eq!(fee(&w, 5), BASE * f64::from(1u32 << 20));
    }

    /// Diplomatic Relations is 5 % off the base a level, and an untrained
    /// pilot pays the base.
    #[test]
    fn the_ally_base_cost_follows_the_skill() {
        let reg = crate::testkit::registry();
        assert_eq!(ally_base_cost(&reg, 90_000_001), ALLY_BASE_COST);
        assert_eq!(
            ALLY_BASE_COST * (1.0 - ALLY_DISCOUNT_PER_LEVEL * 5.0),
            ALLY_BASE_COST * 0.75
        );
    }

    /// A treaty binds for its term, one direction per row, and a longer one is
    /// never shortened.
    #[test]
    fn treaties_bind_for_their_term_and_list_both_ways() {
        let mut d = Deltas::default();
        make_peace(&mut d, AGG, DEF, 1_000, peace::WAR_SURRENDER, 4);
        assert!(treaty_between(&d, AGG, DEF, 999));
        assert!(!treaty_between(&d, AGG, DEF, 1_000), "over at the expiry");
        assert!(
            !treaty_between(&d, DEF, AGG, 10),
            "the other way is its own row"
        );
        make_peace(&mut d, AGG, DEF, 500, peace::UNPAID_BILL, 5);
        assert!(treaty_between(&d, AGG, DEF, 999), "not shortened");
        make_peace(&mut d, AGG, DEF, 2_000, peace::HQ_REMOVED, 6);
        assert!(treaty_between(&d, AGG, DEF, 1_999));
        let (out, inc) = treaties_of(&d, AGG, 10);
        assert_eq!((out.len(), inc.len()), (1, 0));
        assert_eq!(
            (out[0].other, out[0].reason, out[0].war_id),
            (DEF, peace::HQ_REMOVED, 6)
        );
        let (out, inc) = treaties_of(&d, DEF, 10);
        assert_eq!((out.len(), inc.len()), (0, 1));
        assert_eq!(inc[0].other, AGG, "the incoming names who is bound");
        assert_eq!(
            treaties_of(&d, DEF, 2_000),
            (vec![], vec![]),
            "expired ones are not listed"
        );
        let mut reg = crate::testkit::registry();
        for (k, v) in d.of(KIND_PEACE) {
            reg.deltas_mut().put(KIND_PEACE, k.clone(), v.clone());
        }
        expire_treaties(&mut reg, 2_000);
        assert!(reg.deltas().of(KIND_PEACE).is_empty());
    }

    #[test]
    fn negotiations_bills_and_invites_round_trip() {
        let mut d = Deltas::default();
        let n = Negotiation {
            id: 4,
            war_id: 9,
            kind: NEG_ALLY_OFFER,
            owner1: AGG,
            owner2: DEF,
            declared_by: 98_000_005,
            against: DEF,
            isk: 1_234_567.89,
            description: "hello\tworld\n\u{e9}".to_owned(),
            state: STATE_NEW,
            created: 77,
            accepted: None,
            declined: Some(5),
            retracted: None,
        };
        put_negotiation(&mut d, &n);
        assert_eq!(negotiation(&d, 4), Some(n.clone()));
        assert_eq!(negotiations_of(&d, DEF), vec![n.clone()]);
        assert_eq!(
            negotiations_of(&d, 98_000_005),
            vec![n.clone()],
            "the aggressor is a party"
        );
        assert!(negotiations_of(&d, 98_000_777).is_empty());
        let b = Bill {
            id: 2,
            debtor: AGG,
            creditor: war::CONCORD_OWNER,
            amount: COST_OF_WAR,
            due: 1_000,
            paid: false,
            paid_at: None,
            war_id: 9,
            against: DEF,
        };
        put_bill(&mut d, &b);
        assert_eq!(bill(&d, 2), Some(b.clone()));
        assert_eq!(bills_of(&d, &[AGG]), vec![b.clone()]);
        assert!(bills_of(&d, &[DEF]).is_empty());
        d.put(
            KIND_INVITE,
            vec![AGG.to_string(), DEF.to_string()],
            vec!["5".into()],
        );
        d.put(KIND_INVITE, vec![DEF.to_string()], vec!["1".into()]);
        assert_eq!(
            invites(&d),
            vec![Invite {
                from: AGG,
                to: DEF,
                sent: 5
            }]
        );
        assert!(invites_blocked(&d, DEF) && !invites_blocked(&d, AGG));
        assert!(
            autopay(&d, AGG) == AUTOPAY_DEFAULT,
            "the default is the named assumption"
        );
        d.put(KIND_AUTO, vec![AGG.to_string()], vec!["0".into()]);
        assert!(!autopay(&d, AGG));
    }
}
