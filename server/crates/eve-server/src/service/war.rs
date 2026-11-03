//! Wars over the wire (C8): `warRegistry`, `warsInfoMgr`,
//! `mutualWarInviteMgr`, `peaceTreatyMgr` and `billMgr`, plus the war arms of
//! services that live elsewhere (`corpRegistry` and `allianceRegistry`
//! `DeclareWarAgainst` and the bills, `structureDirectory` war HQs,
//! `lookupSvc` warable owners, `warStatisticMgr`'s report calls).
//!
//! The rules are `crate::war` (the registry, declaring, the clock) and
//! `crate::war_deal` (offers, surrender, mutual wars, bills); this file reads
//! the client and answers it. The client reading behind each arm is in the
//! lane note `docs/lanes/c8-a.md` §2.
//!
//! **The owner is the session's.** `warRegistry` is bound on `(ownerID, 1)`
//! where `ownerID` is the session's alliance or corp
//! (`eveMoniker.GetWar`) **[V]**, but this server re-derives the owner from
//! the acting character on every call and ignores the bind, so a client
//! cannot act for an owner it is not in.

use super::*;
use crate::war::{self, Refusal, Wars};
use crate::war_deal as deal;

/// The services dispatched here.
pub(crate) const SERVICES: &[&str] = &[
    "warRegistry",
    "warsInfoMgr",
    "mutualWarInviteMgr",
    "peaceTreatyMgr",
    "billMgr",
];

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn refuse_with(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

/// A war refusal as the client's own `UserError`.
fn raise(r: Refusal) -> ServiceError {
    match r {
        Refusal::Key(k) => refuse_with(k, vec![]),
        Refusal::Access(reason) => refuse_with(
            "CrpAccessDenied",
            vec![("reason", PyValue::Unicode(reason.to_owned()))],
        ),
        Refusal::Money { amount, balance } => refuse_with(
            "NotEnoughMoney",
            vec![
                ("amount", PyValue::Real(amount)),
                ("balance", PyValue::Real(balance)),
            ],
        ),
        Refusal::With(k, args) => refuse_with(k, args),
    }
}

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

fn int_arg(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

fn real_arg(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(x) => Some(*x),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

fn text_arg(v: Option<&PyValue>) -> Option<String> {
    match v? {
        PyValue::Unicode(s) => Some(s.clone()),
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

fn bool_arg(v: Option<&PyValue>) -> bool {
    match v {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        _ => false,
    }
}

/// Positional `i`, or the keyword `name`.
fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args.get(i).or_else(|| {
        req.kwargs
            .iter()
            .find(|(k, _)| crate::pyutil::as_bytes(k) == Some(name.as_bytes()))
            .map(|(_, v)| v)
    })
}

fn need_int(req: &Request<'_>, i: usize, name: &str) -> Result<i64, ServiceError> {
    int_arg(arg(req, i, name)).ok_or_else(|| bad_args(req, &format!("{name} is not an id")))
}

fn need_actor(req: &Request<'_>) -> Result<i64, ServiceError> {
    req.actor.ok_or_else(|| bad_args(req, "no character"))
}

fn none_or(r: Result<(), Refusal>) -> Result<PyValue, ServiceError> {
    r.map(|()| PyValue::None).map_err(raise)
}

/// A dict keyed by integers (`PyValue::Dict` pairs are `(value, key)`).
fn int_dict(entries: Vec<(i64, PyValue)>) -> PyValue {
    PyValue::Dict(
        entries
            .into_iter()
            .map(|(k, v)| (v, PyValue::Int(k)))
            .collect(),
    )
}

fn is_gm(reg: &Registry) -> bool {
    match reg.caller() {
        Caller::Internal => true,
        Caller::Session { role, .. } => role & crate::roles::ROLE_GML != 0,
    }
}

fn require_gm(reg: &Registry) -> Result<(), ServiceError> {
    if is_gm(reg) {
        Ok(())
    } else {
        Err(raise(Refusal::Access("Only a GM can do that.")))
    }
}

fn descriptor(columns: &[(&str, DbType)]) -> DbRowDescriptor {
    DbRowDescriptor::from_pairs(columns).expect("a fixed, valid layout")
}

fn db_opt(v: Option<i64>) -> DbValue {
    v.map_or(DbValue::Null, DbValue::Int)
}

// ---------------------------------------------------------------------------
// warRegistry
// ---------------------------------------------------------------------------

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        "warRegistry" => war_registry(reg, req),
        "warsInfoMgr" => wars_info_mgr(reg, req),
        "mutualWarInviteMgr" => mutual_war_invite_mgr(reg, req),
        "peaceTreatyMgr" => peace_treaty_mgr(reg, req),
        "billMgr" => bill_mgr(reg, req),
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

fn war_registry(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let now = crate::pyutil::filetime_now();
    match req.method {
        // `{warID: war}` for every war the owner is in, finished ones
        // included: `PopulateCacheAndCleanupWars` calls `.itervalues()` and
        // reads `.warID` **[V]**. Any owner may be asked.
        "GetWars" => {
            let owner = int_arg(arg(req, 0, "ownerID"))
                .or_else(|| req.actor.and_then(|a| deal::session_owner(reg, a)));
            let wars = Wars::load(reg.deltas());
            Ok(int_dict(
                owner
                    .map(|o| {
                        wars.of_owner(o)
                            .map(|w| (w.war_id, war::war_value(w, now)))
                            .collect()
                    })
                    .unwrap_or_default(),
            ))
        }
        // The new negotiations the owner is in; the client filters by type.
        "GetNegotiations" => {
            let owner = req.actor.and_then(|a| deal::session_owner(reg, a));
            Ok(PyValue::List(
                owner
                    .map(|o| {
                        deal::negotiations_of(reg.deltas(), o)
                            .iter()
                            .map(deal::Negotiation::value)
                            .collect()
                    })
                    .unwrap_or_default(),
            ))
        }
        // `caller:if_not_x`: `None` for one the owner is not a party to.
        "GetWarNegotiation" => {
            let id = need_int(req, 0, "warNegotiationID")?;
            let owners: Vec<i64> = req.actor.map_or_else(Vec::new, |a| {
                reg.character(a)
                    .map(|c| {
                        let mut v = vec![c.corporation_id];
                        v.extend(c.alliance_id);
                        v
                    })
                    .unwrap_or_default()
            });
            Ok(deal::negotiation(reg.deltas(), id)
                .filter(|n| owners.iter().any(|o| n.involves(*o)))
                .map_or(PyValue::None, |n| n.value()))
        }
        "CreateWarAllyOffer" => {
            let actor = need_actor(req)?;
            let war_id = need_int(req, 0, "warID")?;
            let isk = real_arg(arg(req, 1, "iskValue"))
                .ok_or_else(|| bad_args(req, "iskValue is not a number"))?;
            let defender = need_int(req, 2, "defenderID")?;
            let message = text_arg(arg(req, 3, "message")).unwrap_or_default();
            deal::create_ally_offer(reg, actor, war_id, isk, defender, &message)
                .map(|_| PyValue::None)
                .map_err(raise)
        }
        "RetractWarAllyOffer" => {
            let id = need_int(req, 0, "warNegotiationID")?;
            none_or(deal::retract_ally_offer(reg, need_actor(req)?, id))
        }
        "AcceptAllyNegotiation" => {
            let id = need_int(req, 0, "warNegotiationID")?;
            none_or(deal::accept_ally_offer(reg, need_actor(req)?, id))
        }
        "DeclineAllyOffer" => {
            let id = need_int(req, 0, "warNegotiationID")?;
            none_or(deal::decline_ally_offer(reg, need_actor(req)?, id))
        }
        "CreateSurrenderNegotiation" => {
            let actor = need_actor(req)?;
            let war_id = need_int(req, 0, "warID")?;
            let isk = real_arg(arg(req, 1, "iskValue"))
                .ok_or_else(|| bad_args(req, "iskValue is not a number"))?;
            let message = text_arg(arg(req, 2, "message")).unwrap_or_default();
            deal::create_surrender(reg, actor, war_id, isk, &message)
                .map(|_| PyValue::None)
                .map_err(raise)
        }
        "AcceptSurrender" => {
            let id = need_int(req, 0, "warNegotiationID")?;
            none_or(deal::accept_surrender(reg, need_actor(req)?, id))
        }
        "DeclineSurrender" => {
            let id = need_int(req, 0, "warNegotiationID")?;
            none_or(deal::decline_surrender(reg, need_actor(req)?, id))
        }
        "RetractMutualWar" => {
            let war_id = need_int(req, 0, "warID")?;
            none_or(deal::retract_mutual_war(reg, need_actor(req)?, war_id))
        }
        "SetOpenForAllies" => {
            let war_id = need_int(req, 0, "warID")?;
            let state = bool_arg(arg(req, 1, "state"));
            none_or(deal::set_open_for_allies(
                reg,
                need_actor(req)?,
                war_id,
                state,
            ))
        }
        // The GM's six **[V `war_cso`]**.
        "GMJoinDefender" => {
            require_gm(reg)?;
            let war_id = need_int(req, 0, "warID")?;
            let ally = need_int(req, 2, "warEntityID")?;
            deal::gm_join_defender(reg, war_id, ally);
            Ok(PyValue::None)
        }
        "GMActivateDefender" => {
            require_gm(reg)?;
            deal::gm_activate_defender(
                reg,
                need_int(req, 0, "warID")?,
                need_int(req, 1, "allyID")?,
            );
            Ok(PyValue::None)
        }
        "GMDeactivateDefender" => {
            require_gm(reg)?;
            deal::gm_deactivate_defender(
                reg,
                need_int(req, 0, "warID")?,
                need_int(req, 1, "allyID")?,
            );
            Ok(PyValue::None)
        }
        "GMExtendAllyContract" => {
            require_gm(reg)?;
            deal::gm_extend_ally(
                reg,
                need_int(req, 0, "warID")?,
                need_int(req, 1, "allyID")?,
                need_int(req, 2, "time")?,
            );
            Ok(PyValue::None)
        }
        "GMSetDeclareTime" => {
            require_gm(reg)?;
            deal::gm_set_declare_time(reg, need_int(req, 0, "warID")?, need_int(req, 1, "time")?);
            Ok(PyValue::None)
        }
        "GMSetWarFinishTime" => {
            require_gm(reg)?;
            deal::gm_set_finish_time(reg, need_int(req, 0, "warID")?, need_int(req, 1, "time")?);
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}

// ---------------------------------------------------------------------------
// warsInfoMgr
// ---------------------------------------------------------------------------

/// The columns of `GetWarsByOwnerID`: the war's fields but `allies`, plus
/// `noOfAllies` **[V `War.noOfAllies`; V2 order]**. The client calls
/// `.Index('warID')` on the answer, so it is a plain rowset.
const WAR_COLUMNS: &[(&str, DbType)] = &[
    ("warID", DbType::I4),
    ("declaredByID", DbType::I4),
    ("againstID", DbType::I4),
    ("timeDeclared", DbType::FileTime),
    ("timeFinished", DbType::FileTime),
    ("retracted", DbType::FileTime),
    ("timeStarted", DbType::FileTime),
    ("retractedBy", DbType::I4),
    ("billID", DbType::I4),
    ("mutual", DbType::Bool),
    ("createdFromWarID", DbType::I4),
    ("openForAllies", DbType::Bool),
    ("canBeRetracted", DbType::Bool),
    ("reasonEnded", DbType::I4),
    ("warHQ", DbType::I8),
    ("reasonStarted", DbType::I4),
    ("noOfAllies", DbType::I4),
];

fn war_row(w: &war::War, now: i64) -> Vec<DbValue> {
    vec![
        DbValue::Int(w.war_id),
        DbValue::Int(w.declared_by),
        DbValue::Int(w.against),
        DbValue::Int(w.time_declared),
        db_opt(w.time_finished),
        db_opt(w.retracted),
        DbValue::Int(w.time_started),
        db_opt(w.retracted_by),
        db_opt(w.bill_id),
        DbValue::Bool(w.mutual),
        db_opt(w.created_from),
        DbValue::Bool(w.open_for_allies),
        DbValue::Bool(w.can_be_retracted(now)),
        db_opt(w.reason_ended),
        db_opt(w.war_hq),
        DbValue::Int(w.reason_started),
        DbValue::Int(
            w.allies
                .values()
                .filter(|a| war::ally_active(a, now))
                .count() as i64,
        ),
    ]
}

fn wars_rowset<'a>(wars: impl Iterator<Item = &'a war::War>, now: i64) -> PyValue {
    let mut rs = eve_rowset::CRowset::new(descriptor(WAR_COLUMNS));
    for w in wars {
        // A row not matching its own fixed layout is a bug above; leave it
        // out rather than fail the whole answer.
        let _ = rs.push_values(war_row(w, now));
    }
    rs.to_pyvalue()
}

fn owner_ids_arg(v: Option<&PyValue>) -> Vec<i64> {
    /// A list longer than a page is not a window's.
    const CAP: usize = 100;
    match v {
        Some(PyValue::List(l) | PyValue::Tuple(l)) => l
            .iter()
            .filter_map(|x| int_arg(Some(x)))
            .take(CAP)
            .collect(),
        Some(PyValue::Int(n)) => vec![*n],
        _ => Vec::new(),
    }
}

/// `WARS_PER_PAGE` of the all-wars list **[V `appConst`, I: 50, `GetTop50`]**.
const PAGE: usize = 50;

fn wars_info_mgr(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let now = crate::pyutil::filetime_now();
    let wars = Wars::load(reg.deltas());
    match req.method {
        // A rowset with `.Index('warID')` **[V `corp_ui_wars`]**.
        "GetWarsByOwnerID" => {
            let owner = need_int(req, 0, "ownerID")?;
            Ok(wars_rowset(wars.of_owner(owner), now))
        }
        // `{ownerID: {warID: war}}` **[V `PrimeWarsForOwners`]**.
        "GetWarsByOwners" => {
            let owners = owner_ids_arg(arg(req, 0, "ownerIDs"));
            Ok(int_dict(
                owners
                    .into_iter()
                    .map(|o| {
                        (
                            o,
                            int_dict(
                                wars.of_owner(o)
                                    .map(|w| (w.war_id, war::war_value(w, now)))
                                    .collect(),
                            ),
                        )
                    })
                    .collect(),
            ))
        }
        // The wars asking for allies: open, not mutual, running.
        "GetWarsRequiringAssistance" => Ok(PyValue::List(
            wars.all()
                .filter(|w| w.open_for_allies && !w.mutual && w.active(now))
                .take(PAGE)
                .map(|w| war::war_value(w, now))
                .collect(),
        )),
        // Up to 50 wars below `maxWarID`, newest first (`len`, `[-1].warID`)
        // **[V]**.
        "GetTop50" => {
            let below = int_arg(arg(req, 0, "maxWarID")).unwrap_or(i64::MAX);
            let mut v: Vec<&war::War> = wars.all().filter(|w| w.war_id < below).collect();
            v.sort_by(|a, b| b.war_id.cmp(&a.war_id));
            Ok(PyValue::List(
                v.into_iter()
                    .take(PAGE)
                    .map(|w| war::war_value(w, now))
                    .collect(),
            ))
        }
        "GetPublicWarInfo" => {
            let ids = owner_ids_arg(arg(req, 0, "warIDs"));
            Ok(PyValue::List(
                ids.into_iter()
                    .filter_map(|id| wars.get(id))
                    .map(|w| war::war_value(w, now))
                    .collect(),
            ))
        }
        "GetWarsForStructure" => {
            let hq = need_int(req, 0, "structureID")?;
            Ok(PyValue::List(
                wars.all()
                    .filter(|w| w.war_hq == Some(hq))
                    .map(|w| war::war_value(w, now))
                    .collect(),
            ))
        }
        _ => Err(reg.no_method(req)),
    }
}

// ---------------------------------------------------------------------------
// mutualWarInviteMgr, peaceTreatyMgr, billMgr
// ---------------------------------------------------------------------------

fn mutual_war_invite_mgr(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetPendingInvitesForSession" => Ok(PyValue::List(
            req.actor
                .map(|a| {
                    deal::invites_for(reg, a)
                        .iter()
                        .map(deal::Invite::value)
                        .collect()
                })
                .unwrap_or_default(),
        )),
        "SendInviteByPlayer" => {
            let to = need_int(req, 0, "toOwnerID")?;
            none_or(deal::send_invite(reg, need_actor(req)?, to))
        }
        "WithdrawInviteByPlayer" => {
            let to = need_int(req, 0, "toOwnerID")?;
            none_or(deal::withdraw_invite(reg, need_actor(req)?, to))
        }
        "RespondToInviteByPlayer" => {
            let from = need_int(req, 0, "fromOwnerID")?;
            let accepts = bool_arg(arg(req, 1, "accepts"));
            none_or(deal::respond_to_invite(
                reg,
                need_actor(req)?,
                from,
                accepts,
            ))
        }
        "SetInvitesBlockedByPlayer" => {
            let blocked = bool_arg(arg(req, 0, "blocked"));
            none_or(deal::set_invites_blocked(reg, need_actor(req)?, blocked))
        }
        "IsCorpInvitesBlockedPlayer" => Ok(PyValue::Bool(
            req.actor
                .and_then(|a| deal::session_owner(reg, a))
                .is_some_and(|o| deal::invites_blocked(reg.deltas(), o)),
        )),
        _ => Err(reg.no_method(req)),
    }
}

fn peace_treaty_mgr(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        // `(outgoing, incoming)`: both lists, added together by the client
        // **[V `unpack_2`, `outgoing + incoming`]**.
        "GetPeaceTreatiesForSession" => {
            let now = crate::pyutil::filetime_now();
            let (out, inc) = req
                .actor
                .and_then(|a| deal::session_owner(reg, a))
                .map(|o| deal::treaties_of(reg.deltas(), o, now))
                .unwrap_or_default();
            let list =
                |v: Vec<deal::Treaty>| PyValue::List(v.iter().map(deal::treaty_value).collect());
            Ok(PyValue::Tuple(vec![list(out), list(inc)]))
        }
        _ => Err(reg.no_method(req)),
    }
}

fn bill_mgr(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let owners = |reg: &Registry| -> Vec<i64> {
        req.actor
            .and_then(|a| reg.character(a))
            .map(|c| vec![c.corporation_id])
            .unwrap_or_default()
    };
    match req.method {
        "GetCorporationBills" => Ok(PyValue::List(
            deal::bills_of(reg.deltas(), &owners(reg))
                .iter()
                .map(deal::Bill::value)
                .collect(),
        )),
        "GetCorporationBillsReceivable" => Ok(PyValue::List(vec![])),
        "PayCorporationBill" => {
            let id = need_int(req, 0, "billID")?;
            none_or(deal::pay_bill_as(reg, need_actor(req)?, id))
        }
        // A pilot has no war bills of their own.
        "CharPayBill" => Ok(PyValue::None),
        // `{ownerID: {billTypeID: bool, 'divisionID': key}}` **[V
        // `corpBillsPayablePanel`, `corpBillsAutoPaySettingsPanel`]**.
        "GetAutomaticPaySettings" => {
            let mut out = Vec::new();
            for o in req
                .actor
                .and_then(|a| reg.character(a))
                .map(|c| {
                    let mut v = vec![c.corporation_id];
                    v.extend(c.alliance_id);
                    v
                })
                .unwrap_or_default()
            {
                let inner = PyValue::Dict(vec![
                    (
                        PyValue::Bool(deal::autopay(reg.deltas(), o)),
                        PyValue::Int(deal::BILL_TYPE_WAR),
                    ),
                    (PyValue::Int(1000), crate::pyutil::bstr("divisionID")),
                ]);
                out.push((o, inner));
            }
            Ok(int_dict(out))
        }
        "SendAutomaticPaySettings" => {
            let Some(actor) = req.actor else {
                return Ok(PyValue::None);
            };
            let settings = arg(req, 0, "settings").and_then(PyValue::dict_as_kv);
            let mine = deal::session_owner(reg, actor);
            let corp = reg.character(actor).map(|c| c.corporation_id);
            for (k, v) in settings.unwrap_or_default() {
                let (Some(owner), Some(inner)) = (int_arg(Some(k)), v.dict_as_kv()) else {
                    continue;
                };
                if Some(owner) != mine && Some(owner) != corp {
                    continue;
                }
                if deal::speaks_for(reg, actor).is_none() {
                    continue;
                }
                for (bk, bv) in inner {
                    if int_arg(Some(bk)) == Some(deal::BILL_TYPE_WAR) {
                        reg.deltas_mut().put(
                            deal::KIND_AUTO,
                            vec![owner.to_string()],
                            vec![u8::from(bool_arg(Some(bv))).to_string()],
                        );
                    }
                }
            }
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}

// ---------------------------------------------------------------------------
// The war arms of other services
// ---------------------------------------------------------------------------

/// `corpRegistry`'s war methods: `DeclareWarAgainst(againstID, warHQ)` (the
/// answer is discarded), `CharGetAllyBaseCost()` and `PayBill`. `None` for
/// any other method.
pub(crate) fn corp_registry(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    Some(match req.method {
        "DeclareWarAgainst" => declare(reg, req, false),
        "CharGetAllyBaseCost" => {
            Ok(PyValue::Real(req.actor.map_or(deal::ALLY_BASE_COST, |a| {
                deal::ally_base_cost(reg, a)
            })))
        }
        "PayBill" => pay_bill(reg, req),
        _ => return None,
    })
}

/// `allianceRegistry`'s war methods: `DeclareWarAgainst`, and the bills
/// (`PayBill`, `GetBills`, `GetBillBalance`, `GetBillsReceivable`).
pub(crate) fn alliance_registry(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    Some(match req.method {
        "DeclareWarAgainst" => declare(reg, req, true),
        "PayBill" => pay_bill(reg, req),
        "GetBills" => {
            let owners: Vec<i64> = req
                .actor
                .and_then(|a| reg.character(a))
                .and_then(|c| c.alliance_id)
                .into_iter()
                .collect();
            Ok(PyValue::List(
                deal::bills_of(reg.deltas(), &owners)
                    .iter()
                    .map(deal::Bill::value)
                    .collect(),
            ))
        }
        "GetBillBalance" => {
            let id = int_arg(arg(req, 0, "billID"));
            Ok(PyValue::Real(
                id.and_then(|id| deal::bill(reg.deltas(), id))
                    .filter(|b| !b.paid)
                    .map_or(0.0, |b| b.amount),
            ))
        }
        "GetBillsReceivable" => Ok(PyValue::List(vec![])),
        _ => return None,
    })
}

fn declare(
    reg: &mut Registry,
    req: &Request<'_>,
    as_alliance: bool,
) -> Result<PyValue, ServiceError> {
    let actor = need_actor(req)?;
    let against = need_int(req, 0, "againstID")?;
    let hq = int_arg(arg(req, 1, "warHQ"));
    war::declare(reg, actor, as_alliance, against, hq)
        .map(|_| PyValue::None)
        .map_err(raise)
}

fn pay_bill(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let id = need_int(req, 0, "billID")?;
    none_or(deal::pay_bill_as(reg, need_actor(req)?, id))
}

/// `structureDirectory`'s war methods: `GetValidWarHQs(ownerID)` and
/// `GetStructureInfo(structureID)` for a war HQ. `None` for any other call.
pub(crate) fn structure_directory(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    match req.method {
        "GetValidWarHQs" => {
            let list = (|| {
                let actor = req.actor?;
                let owner = deal::session_owner(reg, actor)?;
                let may_anchor = deal::speaks_for(reg, actor) == Some(owner);
                let hqs = if may_anchor {
                    war::valid_hqs(reg, owner, actor)
                } else {
                    war::hq_of(reg.deltas(), owner)
                        .filter(|h| war::high_sec(reg, h.system_id))
                        .into_iter()
                        .collect()
                };
                Some(hqs.iter().map(|h| war::hq_value(reg, h)).collect())
            })();
            Some(Ok(PyValue::List(list.unwrap_or_default())))
        }
        "GetStructureInfo" => {
            let id = int_arg(req.args.first())?;
            let hq = war::hq_by_item(reg.deltas(), id)?;
            Some(Ok(war::hq_value(reg, &hq)))
        }
        _ => None,
    }
}

/// `lookupSvc.LookupWarableCorporationsOrAlliances(searchStr, exact)`: the
/// player corps and alliances whose names match, as `KeyVal(ownerID,
/// ownerName, typeID, warPermit)` **[V reads `ownerID`, `warPermit`; V2 the
/// rest]**. A corporation inside an alliance is shown with no permit: its
/// alliance is the one to declare on.
pub(crate) fn lookup_svc(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    if req.method != "LookupWarableCorporationsOrAlliances" {
        return None;
    }
    let search = text_arg(req.args.first())
        .unwrap_or_default()
        .to_lowercase();
    let exact = bool_arg(req.args.get(1));
    let mine = req
        .actor
        .and_then(|a| reg.character(a))
        .map(|c| c.corporation_id);
    let matches = |name: &str, ticker: &str| -> bool {
        let (name, ticker) = (name.to_lowercase(), ticker.to_lowercase());
        if search.is_empty() {
            return false;
        }
        if exact {
            return name == search || ticker == search;
        }
        search
            .split_whitespace()
            .all(|t| name.split_whitespace().any(|w| w.starts_with(t)) || ticker.starts_with(t))
    };
    let mut rows = Vec::new();
    for c in reg.corporations.all() {
        if Some(c.corporation_id) == mine || !matches(&c.name, &c.ticker) {
            continue;
        }
        let in_alliance = crate::alliance::of_corp(reg.deltas(), c.corporation_id).is_some();
        rows.push(crate::pyutil::keyval(vec![
            ("ownerID", PyValue::Int(c.corporation_id)),
            ("ownerName", PyValue::Unicode(c.name.clone())),
            ("typeID", PyValue::Int(TYPE_CORPORATION)),
            (
                "warPermit",
                PyValue::Int(i64::from(
                    !in_alliance && war::corp_permit(reg, c.corporation_id),
                )),
            ),
        ]));
    }
    let st = crate::alliance::Alliances::load(reg.deltas());
    for a in st.all().filter(|a| !a.deleted) {
        if !matches(&a.name, &a.short_name) {
            continue;
        }
        rows.push(crate::pyutil::keyval(vec![
            ("ownerID", PyValue::Int(a.alliance_id)),
            ("ownerName", PyValue::Unicode(a.name.clone())),
            ("typeID", PyValue::Int(crate::alliance::TYPE_ALLIANCE)),
            (
                "warPermit",
                PyValue::Int(i64::from(war::permit(reg, a.alliance_id))),
            ),
        ]));
    }
    rows.truncate(PAGE);
    Some(Ok(PyValue::List(rows)))
}

/// `typeCorporation`, the owner type of a corporation **[V]**.
const TYPE_CORPORATION: i64 = 2;

/// `warStatisticMgr`'s report calls, bound on a warID **[V
/// `eveMoniker.GetWarStatistic`]**: `GetBaseInfo()`, `GetKills(entityID,
/// shipGroup)` and `GetKillsByGroup()`. `None` for any other call.
pub(crate) fn war_statistic(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    if !matches!(req.method, "GetBaseInfo" | "GetKills" | "GetKillsByGroup") {
        return None;
    }
    let war_id = match req.bind_params {
        Some(PyValue::Int(n)) => *n,
        Some(PyValue::Tuple(t)) => int_arg(t.first())?,
        _ => return Some(Err(bad_args(req, "not bound on a warID"))),
    };
    let now = crate::pyutil::filetime_now();
    let Some(w) = war::load_war(reg.deltas(), war_id) else {
        return Some(Err(raise(Refusal::Key("CanNotChangeEndedWar"))));
    };
    let reports: Vec<crate::killmail::Report> = war::kills_of_war(reg.deltas(), war_id)
        .into_iter()
        .filter_map(|id| crate::killmail::get(reg, id))
        .collect();
    let group_of = |ship: i64| -> Option<(i64, i64)> {
        reg.content()
            .try_get::<crate::typedata::Installed>()?
            .table()
            .get(ship)
            .map(|t| (t.group_id, t.category_id))
    };
    Some(Ok(match req.method {
        "GetBaseInfo" => {
            let (ships, isk) = crate::war_stats::base_info(&w, &reports);
            PyValue::Tuple(vec![
                war::war_value(&w, now),
                int_dict(
                    ships
                        .into_iter()
                        .map(|(k, v)| (k, PyValue::Int(v)))
                        .collect(),
                ),
                int_dict(
                    isk.into_iter()
                        .map(|(k, v)| (k, PyValue::Real(v)))
                        .collect(),
                ),
                war::allies_value(&w),
            ])
        }
        "GetKills" => {
            let entity = int_arg(req.args.first());
            let class = int_arg(req.args.get(1));
            PyValue::List(
                crate::war_stats::kills(&w, &reports, entity, class, group_of)
                    .into_iter()
                    .map(|r| super::killmail::report_value_in(reg, r))
                    .collect(),
            )
        }
        _ => {
            let by = crate::war_stats::kills_by_group(&w, &reports, group_of);
            int_dict(
                by.into_iter()
                    .map(|(group, t)| {
                        (
                            group,
                            crate::pyutil::pydict(vec![
                                ("attackerShipLoss", PyValue::Int(t.attacker_ship_loss)),
                                ("attackerIskLoss", PyValue::Real(t.attacker_isk_loss)),
                                ("defenderShipLoss", PyValue::Int(t.defender_ship_loss)),
                                ("defenderIskLoss", PyValue::Real(t.defender_isk_loss)),
                            ]),
                        )
                    })
                    .collect(),
            )
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Character;
    use crate::notifications as store;
    use crate::war::{COST_OF_WAR, WAR_SPOOLUP};

    /// Invented pilots, ships and corps: nothing here is a client row.
    const ALICE: i64 = 90_000_401;
    const BOB: i64 = 90_000_402;
    const CAROL: i64 = 90_000_403;
    const DAVE: i64 = 90_000_404;
    const RICH: f64 = 5_000_000_000.0;

    fn registry() -> Registry {
        crate::testkit::registry_with(|c| {
            c.characters = [
                (ALICE, "Alice"),
                (BOB, "Bob"),
                (CAROL, "Carol"),
                (DAVE, "Dave"),
            ]
            .into_iter()
            .map(|(id, name)| Character {
                character_id: id,
                name: name.to_owned(),
                ship_id: id + 10_000_000,
                balance: RICH,
                ..Character::default()
            })
            .collect();
        })
    }

    fn call(
        reg: &mut Registry,
        service: &str,
        method: &str,
        actor: i64,
        args: Vec<PyValue>,
    ) -> Result<PyValue, ServiceError> {
        let req = Request {
            service,
            method,
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(actor),
        };
        reg.dispatch(&req)
    }

    fn uni(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    fn int(i: i64) -> PyValue {
        PyValue::Int(i)
    }

    /// The message key of a raised `UserError`.
    fn key_of(e: &ServiceError) -> String {
        if let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e
            && let PyValue::Tuple(parts) = header.as_ref()
            && let Some(PyValue::Tuple(args)) = parts.get(1)
            && let Some(PyValue::Bytes(b)) = args.first()
        {
            return String::from_utf8_lossy(b).into_owned();
        }
        format!("{e}")
    }

    fn found_corp(reg: &mut Registry, who: i64, name: &str, ticker: &str) -> i64 {
        call(
            reg,
            "corpRegistry",
            "AddCorporation",
            who,
            vec![uni(name), uni(ticker), uni(""), uni("")],
        )
        .expect("founds a corp");
        let corp = reg
            .corporations()
            .membership(who)
            .expect("in the new corp")
            .corporation_id;
        set_funds(reg, who, RICH);
        corp
    }

    fn attr<'a>(v: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("not a KeyVal: {v:?}")
        };
        state
            .dict_as_kv()
            .and_then(|kv| {
                kv.into_iter().find_map(|(k, val)| {
                    (crate::pyutil::as_bytes(k) == Some(name.as_bytes())).then_some(val)
                })
            })
            .unwrap_or_else(|| panic!("no {name} on {v:?}"))
    }

    fn get(dict: &PyValue, key: i64) -> Option<&PyValue> {
        dict.dict_as_kv()?
            .into_iter()
            .find_map(|(k, v)| (*k == PyValue::Int(key)).then_some(v))
    }

    /// The Master Wallet of `who`'s corp: where a war's money lives (CORP-W).
    fn balance(reg: &Registry, who: i64) -> f64 {
        let corp = reg.character(who).map_or(0, |c| c.corporation_id);
        crate::corp_wallet::to_isk(crate::corp_wallet::balance(
            reg.deltas(),
            corp,
            crate::corp_wallet::MASTER_KEY,
        ))
    }

    /// Set the Master Wallet of `who`'s corp to `isk`.
    fn set_funds(reg: &mut Registry, who: i64, isk: f64) {
        let corp = reg.character(who).map_or(0, |c| c.corporation_id);
        let now = crate::corp_wallet::balance(reg.deltas(), corp, crate::corp_wallet::MASTER_KEY);
        crate::corp_wallet::adjust(
            reg.deltas_mut(),
            corp,
            crate::corp_wallet::MASTER_KEY,
            crate::corp_wallet::to_cents(isk) - now,
        )
        .expect("sets the funds");
    }

    /// The war HQ the wizard lists for `who`'s owner.
    fn wizard_hq(reg: &mut Registry, who: i64) -> i64 {
        let list = call(
            reg,
            "structureDirectory",
            "GetValidWarHQs",
            who,
            vec![int(0)],
        )
        .expect("answers");
        let PyValue::List(rows) = list else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 1, "one war HQ: {rows:?}");
        assert_eq!(attr(&rows[0], "typeID"), &int(war::TYPE_WAR_HQ));
        match attr(&rows[0], "structureID") {
            PyValue::Int(i) => *i,
            other => panic!("{other:?}"),
        }
    }

    fn declare_on(reg: &mut Registry, who: i64, against: i64) -> Result<PyValue, ServiceError> {
        let hq = wizard_hq(reg, who);
        call(
            reg,
            "corpRegistry",
            "DeclareWarAgainst",
            who,
            vec![int(against), int(hq)],
        )
    }

    fn wars_of(reg: &mut Registry, who: i64, owner: i64) -> PyValue {
        call(reg, "warRegistry", "GetWars", who, vec![int(owner)]).expect("answers")
    }

    /// Two corps with their CEOs: Alice's and Bob's.
    fn two_corps() -> (Registry, i64, i64) {
        let mut reg = registry();
        let a = found_corp(&mut reg, ALICE, "Alpha Invented", "ALPH");
        let b = found_corp(&mut reg, BOB, "Bravo Invented", "BRAV");
        let _ = reg.take_pending();
        (reg, a, b)
    }

    /// A war that is already hostile, declared by Alice's corp on Bob's.
    fn hostile_war(reg: &mut Registry, a: i64, b: i64) -> i64 {
        declare_on(reg, ALICE, b).expect("declares");
        let war_id = war::Wars::load(reg.deltas())
            .of_owner(a)
            .next()
            .map(|w| w.war_id)
            .expect("the war");
        let past = crate::pyutil::filetime_now() - 2 * WAR_SPOOLUP;
        call(
            reg,
            "warRegistry",
            "GMSetDeclareTime",
            ALICE,
            vec![int(war_id), int(past)],
        )
        .expect("the GM sets the declare time");
        let _ = b;
        war_id
    }

    fn event_names(reg: &mut Registry) -> Vec<(&'static str, Option<Vec<i64>>)> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| (p.to_push().0, p.audience()))
            .collect()
    }

    // ---- declaring ---------------------------------------------------------

    /// **The declaration, end to end**: the fee leaves Alice's wallet, the war
    /// exists with the client's 17 fields and a 24-hour spool-up, both sides
    /// hear `OnWarChanged` and get the notices.
    #[test]
    fn declaring_charges_the_fee_creates_the_war_and_tells_both_sides() {
        let (mut reg, a, b) = two_corps();
        let before = balance(&reg, ALICE);
        declare_on(&mut reg, ALICE, b).expect("declares");
        assert_eq!(before - balance(&reg, ALICE), COST_OF_WAR);

        let answer = wars_of(&mut reg, BOB, b);
        let w = get(&answer, 1).expect("Bob's corp is in war 1");
        for field in [
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
        ] {
            let _ = attr(w, field);
        }
        assert_eq!(attr(w, "declaredByID"), &int(a));
        assert_eq!(attr(w, "againstID"), &int(b));
        assert_eq!(attr(w, "timeFinished"), &PyValue::None);
        assert_eq!(attr(w, "mutual"), &PyValue::Bool(false));
        assert_eq!(
            attr(w, "reasonStarted"),
            &int(war::started::PLAYER_DECLARED)
        );
        let (PyValue::Int(declared), PyValue::Int(started)) =
            (attr(w, "timeDeclared"), attr(w, "timeStarted"))
        else {
            panic!("times")
        };
        assert_eq!(started - declared, WAR_SPOOLUP, "24 h at scale 1");
        assert!(attr(w, "allies").dict_as_kv().is_some_and(|d| d.is_empty()));
        // The declarer's corp sees it too; a stranger's corp does not.
        assert!(get(&wars_of(&mut reg, ALICE, a), 1).is_some());
        let c = found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        assert_eq!(wars_of(&mut reg, CAROL, c), PyValue::Dict(vec![]));

        // The push, to both corps' pilots, with the owners and the change.
        let pushes = reg.take_pending();
        let (_, p) = pushes
            .iter()
            .find(|(_, p)| p.to_push().0 == "OnWarChanged")
            .expect("OnWarChanged");
        let mut to = p.audience().expect("addressed");
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB]);
        let (_, args) = p.to_push();
        assert_eq!(
            args[1],
            crate::fleet_finder::py_set(&[a, b].into_iter().collect())
        );
        let change = &args[2];
        assert_eq!(
            get(change, 0),
            None,
            "keyed by field name, not by number: {change:?}"
        );
        let pairs = change.dict_as_kv().expect("a dict");
        let warid = pairs
            .iter()
            .find(|(k, _)| crate::pyutil::as_bytes(k) == Some(b"warID".as_slice()))
            .map(|(_, v)| (*v).clone())
            .expect("warID in the change");
        assert_eq!(warid, PyValue::Tuple(vec![PyValue::None, int(1)]));

        // The notices: both corps' directors hear the declaration (232); the
        // aggressor also gets 121.
        let types = |who: i64| -> Vec<i64> {
            store::of(reg.deltas(), who)
                .iter()
                .map(|n| n.type_id)
                .collect()
        };
        assert!(types(BOB).contains(&crate::war_notice::kind::WAR_DECLARED));
        assert!(types(ALICE).contains(&crate::war_notice::kind::WAR_DECLARED));
        assert!(types(ALICE).contains(&crate::war_notice::kind::DECLARE_WAR));
        assert!(!types(BOB).contains(&crate::war_notice::kind::DECLARE_WAR));
        assert_keys(
            &reg,
            BOB,
            crate::war_notice::kind::WAR_DECLARED,
            &[
                "declaredByID",
                "againstID",
                "cost",
                "hostileState",
                "timeDeclared",
                "timeStarted",
                "delayHours",
                "warHQ",
                "warHQ_IdType",
            ],
        );
        assert_keys(
            &reg,
            ALICE,
            crate::war_notice::kind::DECLARE_WAR,
            &["defenderID", "entityID", "charID"],
        );
    }

    /// The war runs on the game clock: at scale 8 the spool-up is a third of
    /// an hour, and `at_war` flips exactly there.
    #[test]
    fn the_spool_up_follows_the_game_clock_and_at_war_flips_after_it() {
        let (mut reg, a, b) = two_corps();
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(8).unwrap());
        declare_on(&mut reg, ALICE, b).expect("declares");
        let w = war::load_war(reg.deltas(), 1).expect("the war");
        let spool = WAR_SPOOLUP / 8;
        assert_eq!(w.time_started - w.time_declared, spool);
        let t = w.time_declared;
        for (at, expect) in [(t, false), (t + spool - 1, false), (t + spool + 1, true)] {
            assert_eq!(war::at_war_at(&reg, a, b, at).is_some(), expect, "{at}");
            assert_eq!(war::at_war_at(&reg, b, a, at).is_some(), expect, "{at}");
        }
        // Not with a third corp.
        let c = found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        assert!(war::at_war_at(&reg, a, c, t + spool + 1).is_none());
        // The first weekly bill is due a (scaled) week after the declaration.
        let bill = crate::war_deal::bills(reg.deltas()).pop().expect("a bill");
        assert_eq!(bill.due - t, war::WEEK / 8);
        assert_eq!(bill.amount, COST_OF_WAR);
        // At scale 1 it is the nominal 24 h.
        let (mut reg1, _, b1) = two_corps();
        declare_on(&mut reg1, ALICE, b1).expect("declares");
        let w1 = war::load_war(reg1.deltas(), 1).expect("the war");
        assert_eq!(w1.time_started - w1.time_declared, WAR_SPOOLUP);
    }

    /// Every refusal is the client's own key, in R-WAR's order.
    #[test]
    fn declaring_is_refused_with_the_clients_keys() {
        let (mut reg, a, b) = two_corps();
        let hq = wizard_hq(&mut reg, ALICE);
        let declare = |reg: &mut Registry, who: i64, against: i64, hq: i64| {
            call(
                reg,
                "corpRegistry",
                "DeclareWarAgainst",
                who,
                vec![int(against), int(hq)],
            )
            .expect_err("refused")
        };
        // A member who is not a director.
        call(
            &mut reg,
            "corpRegistry",
            "AddCorporation",
            CAROL,
            vec![uni("Charlie Invented"), uni("CHAR"), uni(""), uni("")],
        )
        .expect("founds");
        assert_eq!(
            key_of(&declare(&mut reg, DAVE, b, hq)),
            "CrpAccessDenied",
            "Dave is in no corp at all"
        );
        assert_eq!(
            key_of(&declare(&mut reg, ALICE, a, hq)),
            "CanNotDeclareWarAgainstSelf"
        );
        assert_eq!(
            key_of(&declare(&mut reg, ALICE, 1_000_044, hq)),
            "CrpCantDeclareWarOnNPCCorp"
        );
        assert_eq!(
            key_of(&declare(&mut reg, ALICE, 98_999_999, hq)),
            "CanNotDeclareWarAgainstThem"
        );
        assert_eq!(
            key_of(&declare(&mut reg, ALICE, b, hq + 1)),
            "CanNotDeclareWarInvalidHQ"
        );
        // A broke declarer.
        set_funds(&mut reg, ALICE, COST_OF_WAR - 1.0);
        let e = declare(&mut reg, ALICE, b, hq);
        assert_eq!(key_of(&e), "NotEnoughMoneyCorp");
        assert_eq!(balance(&reg, ALICE), COST_OF_WAR - 1.0, "nothing taken");
        set_funds(&mut reg, ALICE, RICH);
        // Declared once, the second is `AlreadyAtWarWithThem`.
        declare_on(&mut reg, ALICE, b).expect("declares");
        assert_eq!(
            key_of(&declare(&mut reg, ALICE, b, hq)),
            "AlreadyAtWarWithThem"
        );
        let bob_hq = wizard_hq(&mut reg, BOB);
        assert_eq!(
            key_of(&declare(&mut reg, BOB, a, bob_hq)),
            "AlreadyAtWarWithThem"
        );
    }

    /// No more than [`war::MAX_WARS`] aggressive wars at once.
    #[test]
    fn too_many_wars_are_refused() {
        let mut reg = registry();
        let _a = found_corp(&mut reg, ALICE, "Alpha Invented", "ALPH");
        let hq = wizard_hq(&mut reg, ALICE);
        // The war cap is on the war rows, not on corps, so fill it with rows.
        for i in 0..war::MAX_WARS as i64 {
            let id = war::take_id(reg.deltas_mut(), "war");
            let w = war::War::new(
                id,
                _a,
                98_100_000 + i,
                crate::pyutil::filetime_now(),
                0,
                Some(hq),
            );
            war::put(reg.deltas_mut(), &w);
        }
        let b = found_corp(&mut reg, BOB, "Bravo Invented", "BRAV");
        let e = call(
            &mut reg,
            "corpRegistry",
            "DeclareWarAgainst",
            ALICE,
            vec![int(b), int(hq)],
        )
        .expect_err("refused");
        assert_eq!(key_of(&e), "CrpTooManyWarDeclarations");
    }

    /// `allowWar` is the permit in the corp row and the public info, and a GM
    /// override flips it, pushes `OnAllowWarUpdated` and notifies.
    #[test]
    fn allow_war_is_the_permit_everywhere_and_flips_with_a_push() {
        let (mut reg, a, _b) = two_corps();
        let info =
            call(&mut reg, "corpmgr", "GetPublicInfo", ALICE, vec![int(a)]).expect("answers");
        assert_eq!(attr(&info, "allowWar"), &int(1));
        assert!(war::corp_permit(&reg, a));
        assert!(!war::corp_permit(&reg, 1_000_001), "an NPC corp never");
        war::set_permit(&mut reg, a, Some(false), war::Via::Call);
        let info =
            call(&mut reg, "corpmgr", "GetPublicInfo", ALICE, vec![int(a)]).expect("answers");
        assert_eq!(attr(&info, "allowWar"), &int(0));
        let pushes = reg.take_pending();
        let (_, p) = pushes
            .iter()
            .find(|(_, p)| p.to_push().0 == "OnAllowWarUpdated")
            .expect("pushed");
        assert_eq!(p.to_push().1, vec![int(a), int(0)]);
        assert_eq!(p.audience(), Some(vec![ALICE]));
        assert!(
            store::of(reg.deltas(), ALICE)
                .iter()
                .any(|n| n.type_id == crate::war_notice::kind::CORP_NO_LONGER_WAR_ELIGIBLE)
        );
        // Nobody can declare on a corp with no permit, or from one.
        let e = declare_on(&mut reg, BOB, a).expect_err("refused");
        assert_eq!(key_of(&e), "CanNotDeclareWarAgainstThem");
        war::set_permit(&mut reg, a, None, war::Via::Call);
        assert!(war::corp_permit(&reg, a));
        assert!(
            store::of(reg.deltas(), ALICE)
                .iter()
                .any(|n| n.type_id == crate::war_notice::kind::CORP_BECAME_WAR_ELIGIBLE)
        );
        // The wizard's search shows the permit.
        let found = call(
            &mut reg,
            "lookupSvc",
            "LookupWarableCorporationsOrAlliances",
            BOB,
            vec![uni("alpha"), int(0)],
        )
        .expect("answers");
        let PyValue::List(rows) = found else {
            panic!("list")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(attr(&rows[0], "ownerID"), &int(a));
        assert_eq!(attr(&rows[0], "warPermit"), &int(1));
    }

    /// The HQ is anchored where the director is docked, keeps its item id,
    /// answers `GetStructureInfo`, and follows the director until a war
    /// uses it.
    #[test]
    fn the_war_hq_is_a_virtual_item_anchored_at_the_directors_station() {
        let (mut reg, a, b) = two_corps();
        let hq = wizard_hq(&mut reg, ALICE);
        assert!((war::HQ_ITEM_BASE..crate::item::ESCROW_BASE).contains(&hq));
        assert_eq!(wizard_hq(&mut reg, ALICE), hq, "the same item every time");
        let info = call(
            &mut reg,
            "structureDirectory",
            "GetStructureInfo",
            BOB,
            vec![int(hq)],
        )
        .expect("answers");
        assert_eq!(attr(&info, "typeID"), &int(war::TYPE_WAR_HQ));
        assert_eq!(attr(&info, "structureID"), &int(hq));
        // Not a structure: nothing.
        assert_eq!(
            call(
                &mut reg,
                "structureDirectory",
                "GetStructureInfo",
                BOB,
                vec![int(hq + 1)]
            )
            .expect("answers"),
            PyValue::None
        );
        // Moves with the director while no war uses it...
        reg.character_mut(ALICE).expect("alice").station_id = Some(60_999_999);
        assert_eq!(wizard_hq(&mut reg, ALICE), hq);
        assert_eq!(
            war::hq_of(reg.deltas(), a).map(|h| h.station_id),
            Some(60_999_999)
        );
        // ...and stays put once a war carries it.
        declare_on(&mut reg, ALICE, b).expect("declares");
        reg.character_mut(ALICE).expect("alice").station_id = Some(60_111_111);
        let _ = wizard_hq(&mut reg, ALICE);
        assert_eq!(
            war::hq_of(reg.deltas(), a).map(|h| h.station_id),
            Some(60_999_999)
        );
        // A pilot who is not docked gets the HQ it has, anchors none.
        let c = found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        reg.character_mut(CAROL).expect("carol").station_id = None;
        let list = call(
            &mut reg,
            "structureDirectory",
            "GetValidWarHQs",
            CAROL,
            vec![int(c)],
        )
        .expect("answers");
        assert_eq!(list, PyValue::List(vec![]));
        // A member who is not a director cannot anchor one either.
        assert_eq!(war::hq_of(reg.deltas(), c), None);
    }

    /// The wizard offers the station the director is docked in, and nothing
    /// from space: `Character::station_id` stays set on undock, so the
    /// "docked here" HQ must ask `Registry::docked_station`
    /// (`docs/lanes/stale-system.md`).
    #[test]
    fn the_wizard_offers_no_hq_from_space() {
        let (mut reg, _a, _b) = two_corps();
        let c = found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        {
            let ch = reg.character_mut(CAROL).expect("carol");
            ch.station_id = Some(60_999_001);
            ch.solar_system_id = Some(7_001);
        }
        let ask = |reg: &mut Registry| {
            call(
                reg,
                "structureDirectory",
                "GetValidWarHQs",
                CAROL,
                vec![int(c)],
            )
            .expect("answers")
        };
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                CAROL,
                100_000_003,
                7_002,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        assert_eq!(
            ask(&mut reg),
            PyValue::List(vec![]),
            "in space: no HQ anchored"
        );
        reg.roster_mut().leave(CAROL);
        assert!(
            matches!(ask(&mut reg), PyValue::List(l) if l.len() == 1),
            "docked"
        );
    }

    /// A corp's war persists through the world file, with its allies.
    #[test]
    fn wars_survive_the_world_file() {
        let (mut reg, a, b) = two_corps();
        declare_on(&mut reg, ALICE, b).expect("declares");
        let mut w = war::load_war(reg.deltas(), 1).expect("the war");
        w.allies.insert(
            98_300_000,
            war::Ally {
                ally_id: 98_300_000,
                time_started: 5,
                time_finished: 9,
                told: true,
            },
        );
        w.open_for_allies = true;
        war::put(reg.deltas_mut(), &w);
        let dir = crate::testkit::scratch_dir("war-world");
        let store = crate::store::FileStore::new(dir.path().join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
        let back = crate::store::Store::load(&store)
            .expect("read the world")
            .expect("a world was written");
        let mut fresh = registry();
        fresh.apply_snapshot(back);
        let again = war::load_war(fresh.deltas(), 1).expect("the war came back");
        assert_eq!(again, w);
        assert_eq!(war::hq_of(fresh.deltas(), a), war::hq_of(reg.deltas(), a));
        assert_eq!(
            crate::war_deal::bills(fresh.deltas()),
            crate::war_deal::bills(reg.deltas())
        );
        // And the counter goes on from where it was.
        let mut f = fresh;
        assert_eq!(war::take_id(f.deltas_mut(), "war"), 2);
        assert!(
            event_names(&mut reg)
                .iter()
                .any(|(e, _)| *e == "OnWarChanged")
        );
    }

    // ---- deals -------------------------------------------------------------

    fn load(reg: &Registry, id: i64) -> war::War {
        war::load_war(reg.deltas(), id).expect("the war")
    }

    /// `who` holds a notification of `type_id` whose data has exactly these
    /// keys' worth of the formatter's (every one of `keys` is present).
    fn assert_keys(reg: &Registry, who: i64, type_id: i64, keys: &[&str]) {
        let n = store::of(reg.deltas(), who)
            .into_iter()
            .find(|n| n.type_id == type_id)
            .unwrap_or_else(|| panic!("{who} holds no type {type_id}"));
        for k in keys {
            assert!(
                n.data.iter().any(|(d, _)| d == k),
                "type {type_id} has no {k}: {:?}",
                n.data
            );
        }
    }

    fn types_of(reg: &Registry, who: i64) -> Vec<i64> {
        store::of(reg.deltas(), who)
            .iter()
            .map(|n| n.type_id)
            .collect()
    }

    /// **Surrender**: the weaker side offers ISK, the other's CEO accepts,
    /// the price changes hands (ref 113), the war goes into its cooldown as
    /// retracted, and neither may declare on the other for two weeks.
    #[test]
    fn a_surrender_ends_the_war_into_cooldown_and_forbids_redeclaring() {
        let (mut reg, a, b) = two_corps();
        let id = hostile_war(&mut reg, a, b);
        let now = crate::pyutil::filetime_now();
        assert!(war::at_war_at(&reg, a, b, now).is_some());
        let c = found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        let offer = |reg: &mut Registry, who: i64, isk: f64| {
            call(
                reg,
                "warRegistry",
                "CreateSurrenderNegotiation",
                who,
                vec![int(id), PyValue::Real(isk), uni("gg")],
            )
        };
        assert_eq!(
            key_of(&offer(&mut reg, CAROL, 1.0).expect_err("refused")),
            "CanNotSurenderWarYouAreNotIn"
        );
        let _ = c;
        offer(&mut reg, BOB, 100_000_000.0).expect("Bob offers");
        assert_eq!(
            key_of(&offer(&mut reg, BOB, 1.0).expect_err("refused")),
            "WarSurrenderNegotiationAlreadyActive"
        );
        // The offer is listed for both principals, as a negotiation KeyVal.
        for who in [ALICE, BOB] {
            let PyValue::List(rows) =
                call(&mut reg, "warRegistry", "GetNegotiations", who, vec![]).expect("answers")
            else {
                panic!("list")
            };
            assert_eq!(rows.len(), 1);
            assert_eq!(attr(&rows[0], "warNegotiationTypeID"), &int(2));
            assert_eq!(attr(&rows[0], "ownerID1"), &int(b));
            assert_eq!(attr(&rows[0], "ownerID2"), &int(a));
            assert_eq!(attr(&rows[0], "negotiationState"), &int(0));
        }
        // Bob cannot accept his own offer.
        assert_eq!(
            key_of(
                &call(
                    &mut reg,
                    "warRegistry",
                    "AcceptSurrender",
                    BOB,
                    vec![int(1)]
                )
                .expect_err("refused")
            ),
            "CanNotChangeEndedWar"
        );
        let (alice0, bob0) = (balance(&reg, ALICE), balance(&reg, BOB));
        call(
            &mut reg,
            "warRegistry",
            "AcceptSurrender",
            ALICE,
            vec![int(1)],
        )
        .expect("accepts");
        assert_eq!(balance(&reg, ALICE) - alice0, 100_000_000.0);
        assert_eq!(bob0 - balance(&reg, BOB), 100_000_000.0);

        let w = load(&reg, id);
        assert_eq!(w.reason_ended, Some(war::ended::SURRENDER));
        assert_eq!(w.retracted_by, Some(a));
        let finished = w.time_finished.expect("finished");
        assert!(w.retracted.is_some());
        assert!(finished > now && finished <= now + war::WAR_COOLDOWN + 10_000_000);
        // Cooldown is still war; after it, peace.
        assert!(war::at_war_at(&reg, a, b, now + 1).is_some());
        assert!(war::at_war_at(&reg, a, b, finished + 1).is_none());
        // Two treaties, so neither declares.
        for (x, y) in [(a, b), (b, a)] {
            assert!(crate::war_deal::treaty_between(reg.deltas(), x, y, now));
            assert!(!crate::war_deal::treaty_between(
                reg.deltas(),
                x,
                y,
                now + war::FORCED_PEACE_PERIOD + 10_000_000
            ));
        }
        // The cooldown is still war, so there is nothing to declare yet...
        assert_eq!(
            key_of(&declare_on(&mut reg, ALICE, b).expect_err("refused")),
            "AlreadyAtWarWithThem"
        );
        // ...and once it is over the treaty forbids it, for either side.
        let mut over = load(&reg, id);
        over.time_finished = Some(now - 1);
        war::put(reg.deltas_mut(), &over);
        for who in [ALICE, BOB] {
            let against = if who == ALICE { b } else { a };
            assert_eq!(
                key_of(&declare_on(&mut reg, who, against).expect_err("refused")),
                "CanNotDeclareWarAgainstThem"
            );
        }
        // The treaties list for both, both ways.
        let PyValue::Tuple(t) = call(
            &mut reg,
            "peaceTreatyMgr",
            "GetPeaceTreatiesForSession",
            BOB,
            vec![],
        )
        .expect("answers") else {
            panic!("tuple")
        };
        for side in &t {
            let PyValue::List(l) = side else {
                panic!("list")
            };
            assert_eq!(l.len(), 1);
            assert_eq!(attr(&l[0], "otherOwnerID"), &int(a));
            assert_eq!(attr(&l[0], "peaceReason"), &int(war::peace::WAR_SURRENDER));
        }
        // A retracted war takes no more offers; the notices went out.
        assert_eq!(
            key_of(&offer(&mut reg, BOB, 1.0).expect_err("refused")),
            "CantOfferSurrenderWarRetracted"
        );
        assert!(types_of(&reg, ALICE).contains(&crate::war_notice::kind::ACCEPTED_SURRENDER));
        assert!(types_of(&reg, BOB).contains(&crate::war_notice::kind::CORP_WAR_SURRENDER));
        assert!(types_of(&reg, BOB).contains(&crate::war_notice::kind::OFFERED_SURRENDER));
        assert!(types_of(&reg, ALICE).contains(&crate::war_notice::kind::SURRENDER_OFFER));
        use crate::war_notice::kind as nk;
        assert_keys(
            &reg,
            ALICE,
            nk::SURRENDER_OFFER,
            &["ownerID1", "ownerID2", "iskValue"],
        );
        assert_keys(
            &reg,
            BOB,
            nk::OFFERED_SURRENDER,
            &["entityID", "offeredID", "charID", "iskValue"],
        );
        assert_keys(
            &reg,
            ALICE,
            nk::ACCEPTED_SURRENDER,
            &["entityID", "offeringID", "charID", "iskValue"],
        );
        assert_keys(
            &reg,
            BOB,
            nk::CORP_WAR_SURRENDER,
            &["declaredByID", "againstID"],
        );
    }

    #[test]
    fn a_surrender_the_offerer_cannot_pay_is_refused_with_the_clients_key() {
        let (mut reg, a, b) = two_corps();
        let id = hostile_war(&mut reg, a, b);
        set_funds(&mut reg, BOB, 5.0);
        call(
            &mut reg,
            "warRegistry",
            "CreateSurrenderNegotiation",
            BOB,
            vec![int(id), PyValue::Real(100.0), uni("")],
        )
        .expect("offers");
        let e = call(
            &mut reg,
            "warRegistry",
            "AcceptSurrender",
            ALICE,
            vec![int(1)],
        )
        .expect_err("refused");
        assert_eq!(key_of(&e), "CantAcceptSurrenderNotEnoughMoney");
        assert_eq!(load(&reg, id).time_finished, None, "the war goes on");
        // Declining tells the offerer, and the offer is spent.
        call(
            &mut reg,
            "warRegistry",
            "DeclineSurrender",
            ALICE,
            vec![int(1)],
        )
        .expect("declines");
        assert!(types_of(&reg, BOB).contains(&crate::war_notice::kind::SURRENDER_DECLINED));
        assert_keys(
            &reg,
            BOB,
            crate::war_notice::kind::SURRENDER_DECLINED,
            &["ownerID", "iskValue"],
        );
        let e = call(
            &mut reg,
            "warRegistry",
            "DeclineSurrender",
            ALICE,
            vec![int(1)],
        )
        .expect_err("refused");
        assert_eq!(key_of(&e), "SurrenderOfferAlreadyDeclined");
    }

    /// **Allies**: the defender opens the war, a third corp offers, the
    /// defender accepts and pays; the ally fights after its 4 h spool-up. The
    /// first ally costs CONCORD nothing, the second the base, the third twice.
    #[test]
    fn allies_join_pay_and_fight_after_their_spool_up() {
        let (mut reg, a, b) = two_corps();
        let c = found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        let d = found_corp(&mut reg, DAVE, "Delta Invented", "DELT");
        let id = hostile_war(&mut reg, a, b);
        let offer = |reg: &mut Registry, who: i64, isk: f64| {
            call(
                reg,
                "warRegistry",
                "CreateWarAllyOffer",
                who,
                vec![int(id), PyValue::Real(isk), int(b), uni("hire us")],
            )
        };
        assert_eq!(
            key_of(&offer(&mut reg, CAROL, 1.0).expect_err("refused")),
            "CannotAllyWarIsNotOpen"
        );
        call(
            &mut reg,
            "warRegistry",
            "SetOpenForAllies",
            ALICE,
            vec![int(id), PyValue::Bool(true)],
        )
        .expect_err("only the defender opens it");
        call(
            &mut reg,
            "warRegistry",
            "SetOpenForAllies",
            BOB,
            vec![int(id), PyValue::Bool(true)],
        )
        .expect("the defender opens it");
        assert!(load(&reg, id).open_for_allies);
        let PyValue::List(open) = call(
            &mut reg,
            "warsInfoMgr",
            "GetWarsRequiringAssistance",
            CAROL,
            vec![],
        )
        .expect("answers") else {
            panic!("list")
        };
        assert_eq!(open.len(), 1);
        // The aggressor cannot ally the defender.
        assert_eq!(
            key_of(&offer(&mut reg, ALICE, 1.0).expect_err("refused")),
            "AlreadyAtWar"
        );
        offer(&mut reg, CAROL, 1_000_000.0).expect("Carol offers");
        assert_eq!(
            key_of(&offer(&mut reg, DAVE, 5.0).expect_err("refused")),
            "WarAllyNegotiationAlreadyActive"
        );
        let (bob0, carol0) = (balance(&reg, BOB), balance(&reg, CAROL));
        let now = crate::pyutil::filetime_now();
        call(
            &mut reg,
            "warRegistry",
            "AcceptAllyNegotiation",
            BOB,
            vec![int(1)],
        )
        .expect("accepts");
        assert_eq!(
            bob0 - balance(&reg, BOB),
            1_000_000.0,
            "the first ally is free of the fee"
        );
        assert_eq!(balance(&reg, CAROL) - carol0, 1_000_000.0);
        let w = load(&reg, id);
        let ally = w.allies.get(&c).expect("Carol's corp is an ally");
        assert!(ally.time_started > now + war::ALLY_SPOOLUP - 10_000_000);
        assert!(ally.time_started <= now + war::ALLY_SPOOLUP + 10_000_000);
        assert!(ally.time_finished > now + war::ALLY_COMMITMENT_PERIOD - 10_000_000);
        // At war with the aggressor only once the spool-up is over; never
        // with the defender; allies do not fight each other.
        assert!(war::at_war_at(&reg, c, a, ally.time_started - 1).is_none());
        assert!(war::at_war_at(&reg, c, a, ally.time_started + 1).is_some());
        assert!(war::at_war_at(&reg, a, c, ally.time_started + 1).is_some());
        assert!(war::at_war_at(&reg, c, b, ally.time_started + 1).is_none());
        assert!(war::at_war_at(&reg, c, a, ally.time_finished + 1).is_none());
        // The war KeyVal carries the ally; the notices went out.
        let wars = wars_of(&mut reg, CAROL, c);
        let allies = attr(get(&wars, id).expect("in it"), "allies");
        assert!(get(allies, c).is_some());
        assert!(types_of(&reg, ALICE).contains(&crate::war_notice::kind::ALLY_JOINED_AGGRESSOR));
        assert!(types_of(&reg, BOB).contains(&crate::war_notice::kind::ALLY_JOINED_DEFENDER));
        assert!(types_of(&reg, CAROL).contains(&crate::war_notice::kind::ALLY_JOINED_ALLY));
        use crate::war_notice::kind as nk;
        assert_keys(
            &reg,
            BOB,
            nk::ALLY_JOINED_DEFENDER,
            &["allyID", "aggressorID", "startTime"],
        );
        assert_keys(
            &reg,
            ALICE,
            nk::ALLY_JOINED_AGGRESSOR,
            &["allyID", "defenderID", "startTime"],
        );
        assert_keys(
            &reg,
            CAROL,
            nk::ALLY_JOINED_ALLY,
            &["allyID", "defenderID", "aggressorID", "startTime"],
        );
        assert_keys(
            &reg,
            BOB,
            nk::MERC_OFFERED,
            &["mercID", "defenderID", "aggressorID", "iskValue"],
        );
        assert_keys(
            &reg,
            CAROL,
            nk::OFFERED_TO_ALLY,
            &["defenderID", "enemyID", "charID", "iskValue"],
        );
        assert_keys(
            &reg,
            BOB,
            nk::ACCEPTED_ALLY,
            &["allyID", "enemyID", "charID", "iskValue", "time"],
        );
        // A second ally pays CONCORD the base.
        offer(&mut reg, DAVE, 0.0).expect("Dave offers");
        let bob1 = balance(&reg, BOB);
        call(
            &mut reg,
            "warRegistry",
            "AcceptAllyNegotiation",
            BOB,
            vec![int(2)],
        )
        .expect("accepts");
        assert_eq!(bob1 - balance(&reg, BOB), crate::war_deal::ALLY_BASE_COST);
        assert!(load(&reg, id).allies.contains_key(&d));
        assert_eq!(
            key_of(&offer(&mut reg, CAROL, 0.0).expect_err("refused")),
            "AlreadyAnAlly"
        );
    }

    #[test]
    fn an_ally_offer_can_be_declined_or_retracted_and_a_mutual_war_has_no_allies() {
        let (mut reg, a, b) = two_corps();
        let _c = found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        let id = hostile_war(&mut reg, a, b);
        call(
            &mut reg,
            "warRegistry",
            "SetOpenForAllies",
            BOB,
            vec![int(id), PyValue::Bool(true)],
        )
        .expect("opens");
        let make = |reg: &mut Registry| {
            call(
                reg,
                "warRegistry",
                "CreateWarAllyOffer",
                CAROL,
                vec![int(id), PyValue::Real(7.0), int(b), uni("")],
            )
            .expect("offers")
        };
        make(&mut reg);
        // Only the offerer retracts; only the receiver declines.
        assert!(
            call(
                &mut reg,
                "warRegistry",
                "RetractWarAllyOffer",
                BOB,
                vec![int(1)]
            )
            .is_err()
        );
        assert!(
            call(
                &mut reg,
                "warRegistry",
                "DeclineAllyOffer",
                CAROL,
                vec![int(1)]
            )
            .is_err()
        );
        call(
            &mut reg,
            "warRegistry",
            "DeclineAllyOffer",
            BOB,
            vec![int(1)],
        )
        .expect("declines");
        assert!(types_of(&reg, CAROL).contains(&crate::war_notice::kind::ALLY_OFFER_DECLINED));
        assert_keys(
            &reg,
            CAROL,
            crate::war_notice::kind::ALLY_OFFER_DECLINED,
            &["defenderID", "aggressorID", "allyID", "charID"],
        );
        let e = call(
            &mut reg,
            "warRegistry",
            "AcceptAllyNegotiation",
            BOB,
            vec![int(1)],
        )
        .expect_err("refused");
        assert_eq!(key_of(&e), "AllyOfferAlreadyDeclined");
        make(&mut reg);
        call(
            &mut reg,
            "warRegistry",
            "RetractWarAllyOffer",
            CAROL,
            vec![int(2)],
        )
        .expect("retracts");
        assert!(types_of(&reg, BOB).contains(&crate::war_notice::kind::MERC_OFFER_RETRACTED));
        assert_keys(
            &reg,
            BOB,
            crate::war_notice::kind::MERC_OFFER_RETRACTED,
            &["mercID", "defenderID", "aggressorID"],
        );
        assert_keys(
            &reg,
            CAROL,
            crate::war_notice::kind::OFFER_TO_ALLY_RETRACTED,
            &["allyID", "defenderID", "enemyID", "charID"],
        );
        // Nothing is left to list.
        let PyValue::List(rows) =
            call(&mut reg, "warRegistry", "GetNegotiations", BOB, vec![]).expect("answers")
        else {
            panic!("list")
        };
        assert!(rows.is_empty());
        // A war that ended takes no more allies: its open offers lapse.
        make(&mut reg);
        war::finish(
            &mut reg,
            id,
            war::FinishWith {
                reason: war::ended::GM_ACTION,
                at: Some(1),
                ..war::FinishWith::default()
            },
            crate::pyutil::filetime_now(),
            war::Via::Call,
        );
        let n = crate::war_deal::negotiation(reg.deltas(), 3).expect("kept");
        assert_eq!(n.state, crate::war_deal::STATE_RETRACTED);
    }

    /// **Mutual wars**: an invite, a block, a limit, an acceptance that spools
    /// up from the acceptance, no fee and no bill, and a retract.
    #[test]
    fn a_mutual_war_spools_up_from_the_acceptance_and_is_never_billed() {
        let (mut reg, a, b) = two_corps();
        let c = found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        let invite = |reg: &mut Registry, who: i64, to: i64| {
            call(
                reg,
                "mutualWarInviteMgr",
                "SendInviteByPlayer",
                who,
                vec![int(to)],
            )
        };
        // A block is refused with its key.
        call(
            &mut reg,
            "mutualWarInviteMgr",
            "SetInvitesBlockedByPlayer",
            BOB,
            vec![PyValue::Bool(true)],
        )
        .expect("blocks");
        assert_eq!(
            call(
                &mut reg,
                "mutualWarInviteMgr",
                "IsCorpInvitesBlockedPlayer",
                BOB,
                vec![]
            )
            .expect("answers"),
            PyValue::Bool(true)
        );
        assert_eq!(
            key_of(&invite(&mut reg, ALICE, b).expect_err("refused")),
            "CannotSendMutualWarInviteBlocked"
        );
        call(
            &mut reg,
            "mutualWarInviteMgr",
            "SetInvitesBlockedByPlayer",
            BOB,
            vec![PyValue::Bool(false)],
        )
        .expect("unblocks");
        let _ = reg.take_pending();
        invite(&mut reg, ALICE, b).expect("invites");
        assert_eq!(
            key_of(&invite(&mut reg, BOB, a).expect_err("refused")),
            "CannotSendMutualWarInviteAlreadyExists"
        );
        let names = event_names(&mut reg);
        let upd = names
            .iter()
            .find(|(e, _)| *e == "OnMutualWarsUpdated_Remote")
            .expect("both sides are told");
        let mut to = upd.1.clone().expect("addressed");
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB]);
        assert!(types_of(&reg, BOB).contains(&crate::war_notice::kind::MUTUAL_INVITE_SENT));
        assert_keys(
            &reg,
            BOB,
            crate::war_notice::kind::MUTUAL_INVITE_SENT,
            &["declaredByID", "againstID", "expireTimeStamp"],
        );
        for who in [ALICE, BOB] {
            let PyValue::List(rows) = call(
                &mut reg,
                "mutualWarInviteMgr",
                "GetPendingInvitesForSession",
                who,
                vec![],
            )
            .expect("answers") else {
                panic!("list")
            };
            assert_eq!(rows.len(), 1);
            assert_eq!(attr(&rows[0], "fromOwnerID"), &int(a));
            assert_eq!(attr(&rows[0], "toOwnerID"), &int(b));
        }
        // The limit.
        for i in 0..crate::war_deal::MAX_OUTGOING_INVITES as i64 {
            reg.deltas_mut().put(
                crate::war_deal::KIND_INVITE,
                vec![a.to_string(), (98_200_000 + i).to_string()],
                vec!["1".to_owned()],
            );
        }
        assert_eq!(
            key_of(&invite(&mut reg, ALICE, c).expect_err("refused")),
            "CannotSendMutualWarInviteTooManyOutgoing"
        );
        // Withdraw and send again, then Bob accepts.
        call(
            &mut reg,
            "mutualWarInviteMgr",
            "WithdrawInviteByPlayer",
            ALICE,
            vec![int(b)],
        )
        .expect("withdraws");
        for i in 0..crate::war_deal::MAX_OUTGOING_INVITES as i64 {
            reg.deltas_mut().del(
                crate::war_deal::KIND_INVITE,
                &[a.to_string(), (98_200_000 + i).to_string()],
            );
        }
        invite(&mut reg, ALICE, b).expect("invites again");
        let (alice0, bob0) = (balance(&reg, ALICE), balance(&reg, BOB));
        let now = crate::pyutil::filetime_now();
        call(
            &mut reg,
            "mutualWarInviteMgr",
            "RespondToInviteByPlayer",
            BOB,
            vec![int(a), PyValue::Bool(true)],
        )
        .expect("accepts");
        assert_eq!(
            (balance(&reg, ALICE), balance(&reg, BOB)),
            (alice0, bob0),
            "no fee"
        );
        assert!(crate::war_deal::bills(reg.deltas()).is_empty(), "no bill");
        let w = war::Wars::load(reg.deltas())
            .all()
            .next()
            .cloned()
            .expect("the war");
        assert!(w.mutual);
        assert_eq!((w.declared_by, w.against), (a, b));
        assert!(
            w.time_started >= now + WAR_SPOOLUP - 10_000_000,
            "24 h from the acceptance"
        );
        assert!(w.can_be_retracted(now));
        assert!(war::at_war_at(&reg, a, b, w.time_started + 1).is_some());
        assert!(types_of(&reg, ALICE).contains(&crate::war_notice::kind::MUTUAL_INVITE_ACCEPTED));
        assert_keys(
            &reg,
            ALICE,
            crate::war_notice::kind::MUTUAL_INVITE_ACCEPTED,
            &["declaredByID", "againstID", "time"],
        );
        // It cannot take allies, nor a surrender.
        let e = call(
            &mut reg,
            "warRegistry",
            "CreateSurrenderNegotiation",
            BOB,
            vec![int(w.war_id), PyValue::Real(1.0), uni("")],
        )
        .expect_err("refused");
        assert_eq!(key_of(&e), "CanNotChangeMutualWarWrongParty");
        // A stranger cannot retract it; a party can, into the cooldown.
        let e = call(
            &mut reg,
            "warRegistry",
            "RetractMutualWar",
            CAROL,
            vec![int(w.war_id)],
        )
        .expect_err("refused");
        assert_eq!(key_of(&e), "CanNotChangeMutualWarWrongParty");
        // A pilot who is no CEO at all is refused outright.
        let e = call(
            &mut reg,
            "warRegistry",
            "RetractMutualWar",
            DAVE,
            vec![int(w.war_id)],
        )
        .expect_err("refused");
        assert_eq!(key_of(&e), "MustBeActiveCEOToEndWar");
        call(
            &mut reg,
            "warRegistry",
            "RetractMutualWar",
            BOB,
            vec![int(w.war_id)],
        )
        .expect("retracts");
        let w = load(&reg, w.war_id);
        assert_eq!(w.reason_ended, Some(war::ended::RETRACTED));
        assert!(w.time_finished.is_some());
        let e = call(
            &mut reg,
            "warRegistry",
            "RetractMutualWar",
            BOB,
            vec![int(w.war_id)],
        )
        .expect_err("refused");
        assert_eq!(key_of(&e), "CanNotChangeEndedWar");
        assert!(types_of(&reg, ALICE).contains(&crate::war_notice::kind::WAR_RETRACTED));
        // Only the invite's receiver answers; a refusal tells both.
        invite(&mut reg, BOB, a).expect_err("a war ran, but it is over only after the cooldown");
    }

    #[test]
    fn an_invite_that_is_refused_or_ignored_is_gone() {
        let (mut reg, a, b) = two_corps();
        call(
            &mut reg,
            "mutualWarInviteMgr",
            "SendInviteByPlayer",
            ALICE,
            vec![int(b)],
        )
        .expect("invites");
        // Not the sender's to answer.
        let e = call(
            &mut reg,
            "mutualWarInviteMgr",
            "RespondToInviteByPlayer",
            ALICE,
            vec![int(a), PyValue::Bool(true)],
        )
        .expect_err("refused");
        assert_eq!(key_of(&e), "CanNotChangeEndedWar");
        call(
            &mut reg,
            "mutualWarInviteMgr",
            "RespondToInviteByPlayer",
            BOB,
            vec![int(a), PyValue::Bool(false)],
        )
        .expect("refuses");
        assert!(crate::war_deal::invites(reg.deltas()).is_empty());
        assert!(war::Wars::load(reg.deltas()).is_empty());
        assert!(types_of(&reg, ALICE).contains(&crate::war_notice::kind::MUTUAL_INVITE_REJECTED));
        // An ignored one expires after a (scaled) week and tells both.
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(8).unwrap());
        call(
            &mut reg,
            "mutualWarInviteMgr",
            "SendInviteByPlayer",
            BOB,
            vec![int(a)],
        )
        .expect("invites");
        let sent = crate::war_deal::invites(reg.deltas())[0].sent;
        crate::war_deal::tick(&mut reg, sent + war::MUTUAL_INVITE_EXPIRY / 8 - 1);
        assert_eq!(crate::war_deal::invites(reg.deltas()).len(), 1);
        crate::war_deal::tick(&mut reg, sent + war::MUTUAL_INVITE_EXPIRY / 8 + 1);
        assert!(crate::war_deal::invites(reg.deltas()).is_empty());
        for who in [ALICE, BOB] {
            assert!(types_of(&reg, who).contains(&crate::war_notice::kind::MUTUAL_EXPIRED));
            assert_keys(
                &reg,
                who,
                crate::war_notice::kind::MUTUAL_EXPIRED,
                &["declaredByID", "againstID", "numDays"],
            );
        }
    }

    // ---- bills -------------------------------------------------------------

    /// **The weekly bill**: week two's falls due a (scaled) week after the
    /// declaration and is paid from the wallet, which issues the next; an owner
    /// that cannot (or will not) pay loses the war, `ENDED_UNPAID_BILL`, with a
    /// treaty against the aggressor alone.
    #[test]
    fn the_weekly_bill_is_paid_when_due_or_the_war_ends() {
        let (mut reg, a, b) = two_corps();
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(4).unwrap());
        let id = {
            declare_on(&mut reg, ALICE, b).expect("declares");
            1
        };
        let week = war::WEEK / 4;
        let w = load(&reg, id);
        let bill = crate::war_deal::bill(reg.deltas(), w.bill_id.expect("a bill")).expect("row");
        assert_eq!((bill.debtor, bill.creditor), (a, war::CONCORD_OWNER));
        assert_eq!(bill.due - w.time_declared, week);
        // The wallet's Bills tab lists it as an unpaid war bill (type 4).
        let PyValue::List(rows) =
            call(&mut reg, "billMgr", "GetCorporationBills", ALICE, vec![]).expect("answers")
        else {
            panic!("list")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(attr(&rows[0], "billTypeID"), &int(4));
        assert_eq!(attr(&rows[0], "paid"), &PyValue::Bool(false));
        assert_eq!(attr(&rows[0], "externalID"), &int(b));
        // Not due yet: nothing happens.
        let before = balance(&reg, ALICE);
        crate::war_deal::tick(&mut reg, bill.due - 1);
        assert_eq!(balance(&reg, ALICE), before);
        // Due: paid, and the next is issued a week on.
        crate::war_deal::tick(&mut reg, bill.due + 1);
        assert_eq!(before - balance(&reg, ALICE), COST_OF_WAR);
        let bills = crate::war_deal::bills(reg.deltas());
        assert_eq!(bills.len(), 2);
        assert!(bills[0].paid && !bills[1].paid);
        assert_eq!(bills[1].due, bill.due + week);
        assert_eq!(load(&reg, id).bill_id, Some(2));
        assert!(load(&reg, id).time_finished.is_none());
        // Alice cannot pay the next: the war ends, and she alone is barred.
        set_funds(&mut reg, ALICE, 1.0);
        crate::war_deal::tick(&mut reg, bills[1].due + 1);
        let w = load(&reg, id);
        assert_eq!(w.reason_ended, Some(war::ended::UNPAID_BILL));
        assert!(w.time_finished.is_some());
        assert!(crate::war_deal::treaty_between(
            reg.deltas(),
            a,
            b,
            bills[1].due
        ));
        assert!(!crate::war_deal::treaty_between(
            reg.deltas(),
            b,
            a,
            bills[1].due
        ));
        assert!(types_of(&reg, ALICE).contains(&crate::war_notice::kind::BILL_OUT_OF_MONEY));
        assert_keys(
            &reg,
            ALICE,
            crate::war_notice::kind::BILL_OUT_OF_MONEY,
            &["billTypeID"],
        );
        assert_keys(
            &reg,
            ALICE,
            crate::war_notice::kind::CORP_BILL,
            &[
                "billTypeID",
                "creditorID",
                "debtorID",
                "externalID",
                "amount",
                "dueDate",
            ],
        );
        for who in [ALICE, BOB] {
            assert!(
                types_of(&reg, who).contains(&crate::war_notice::kind::WAR_RETRACTED_BY_CONCORD)
            );
            assert_keys(
                &reg,
                who,
                crate::war_notice::kind::WAR_RETRACTED_BY_CONCORD,
                &["declaredByID", "againstID", "endDate"],
            );
        }
        // The cancelled bill is gone.
        assert!(crate::war_deal::bills(reg.deltas()).iter().all(|b| b.paid));
    }

    /// A bill is paid early by an accountant (the wallet's menu), or left to
    /// the war's end if auto-pay is off.
    #[test]
    fn a_bill_is_paid_by_hand_and_auto_pay_can_be_turned_off() {
        let (mut reg, a, b) = two_corps();
        declare_on(&mut reg, ALICE, b).expect("declares");
        // Bob is not the debtor.
        let e = call(&mut reg, "billMgr", "PayCorporationBill", BOB, vec![int(1)])
            .expect_err("refused");
        assert_eq!(key_of(&e), "CrpAccessDenied");
        let before = balance(&reg, ALICE);
        call(
            &mut reg,
            "billMgr",
            "PayCorporationBill",
            ALICE,
            vec![int(1)],
        )
        .expect("pays");
        assert_eq!(before - balance(&reg, ALICE), COST_OF_WAR);
        assert_eq!(
            crate::war_deal::bills(reg.deltas()).len(),
            2,
            "the next is issued"
        );
        // The settings panel's view and its edit.
        let settings = call(
            &mut reg,
            "billMgr",
            "GetAutomaticPaySettings",
            ALICE,
            vec![],
        )
        .expect("answers");
        let mine = get(&settings, a).expect("for Alice's corp");
        assert_eq!(get(mine, 4), Some(&PyValue::Bool(true)));
        let off = PyValue::Dict(vec![(
            PyValue::Dict(vec![(PyValue::Bool(false), int(4))]),
            int(a),
        )]);
        call(
            &mut reg,
            "billMgr",
            "SendAutomaticPaySettings",
            ALICE,
            vec![off],
        )
        .expect("saves");
        assert!(!crate::war_deal::autopay(reg.deltas(), a));
        // With auto-pay off, the unpaid bill ends the war even with the money.
        let due = crate::war_deal::bills(reg.deltas())[1].due;
        crate::war_deal::tick(&mut reg, due + 1);
        assert_eq!(load(&reg, 1).reason_ended, Some(war::ended::UNPAID_BILL));
    }

    // ---- the clock ----------------------------------------------------------

    /// The tick tells the defender when the war can be fought, once, and
    /// pushes `OnWarChanged` to both sides so the overview recolours.
    #[test]
    fn the_clock_announces_a_war_when_it_can_be_fought_once() {
        let (mut reg, a, b) = two_corps();
        declare_on(&mut reg, ALICE, b).expect("declares");
        let w = load(&reg, 1);
        let _ = reg.take_pending();
        let _ = reg.take_due(std::time::Instant::now());
        war::tick(&mut reg, std::time::Instant::now(), w.time_started - 1);
        assert!(!types_of(&reg, BOB).contains(&crate::war_notice::kind::CORP_WAR_FIGHTING_LEGAL));
        assert!(reg.take_due(std::time::Instant::now()).is_empty());
        war::tick(&mut reg, std::time::Instant::now(), w.time_started + 1);
        assert!(types_of(&reg, BOB).contains(&crate::war_notice::kind::CORP_WAR_FIGHTING_LEGAL));
        assert_keys(
            &reg,
            BOB,
            crate::war_notice::kind::CORP_WAR_FIGHTING_LEGAL,
            &["declaredByID", "againstID", "cost"],
        );
        let due = reg.take_due(std::time::Instant::now());
        let told: Vec<_> = due
            .iter()
            .filter(|p| p.to_push().0 == "OnWarChanged")
            .collect();
        assert_eq!(told.len(), 1);
        let mut to = told[0].audience().expect("addressed");
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB]);
        // Once.
        let n = types_of(&reg, BOB).len();
        war::tick(&mut reg, std::time::Instant::now(), w.time_started + 2);
        assert_eq!(types_of(&reg, BOB).len(), n);
        assert!(reg.take_due(std::time::Instant::now()).is_empty());
        let _ = (a, b);
        // A finished war past its keep is dropped.
        let mut w = load(&reg, 1);
        w.time_finished = Some(100);
        war::put(reg.deltas_mut(), &w);
        war::tick(
            &mut reg,
            std::time::Instant::now(),
            100 + war::KEEP_FINISHED + 1,
        );
        assert!(war::load_war(reg.deltas(), 1).is_none());
    }

    /// The stage runs four times a second: with nothing due it must not
    /// touch the world, or the world file would be written every few seconds
    /// for ever.
    #[test]
    fn an_idle_tick_leaves_the_world_clean() {
        let (mut reg, a, b) = two_corps();
        declare_on(&mut reg, ALICE, b).expect("declares");
        let w = load(&reg, 1);
        // Past the start: told once, then idle.
        war::tick(&mut reg, std::time::Instant::now(), w.time_started + 1);
        let _ = reg.take_dirty();
        war::tick(&mut reg, std::time::Instant::now(), w.time_started + 2);
        assert!(!reg.take_dirty(), "nothing was due, nothing was written");
        // A mutual war, a peace treaty and a negotiation sit untouched too.
        call(
            &mut reg,
            "mutualWarInviteMgr",
            "SendInviteByPlayer",
            BOB,
            vec![int(a)],
        )
        .expect_err("at war");
        let (mut reg, _a, b) = two_corps();
        call(
            &mut reg,
            "mutualWarInviteMgr",
            "SendInviteByPlayer",
            ALICE,
            vec![int(b)],
        )
        .expect("invites");
        let _ = reg.take_dirty();
        war::tick(
            &mut reg,
            std::time::Instant::now(),
            crate::pyutil::filetime_now(),
        );
        assert!(!reg.take_dirty());
    }

    // ---- GM ----------------------------------------------------------------

    #[test]
    fn the_gm_calls_move_a_wars_times_and_allies() {
        let (mut reg, a, b) = two_corps();
        let c = found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        declare_on(&mut reg, ALICE, b).expect("declares");
        let now = crate::pyutil::filetime_now();
        assert!(war::at_war_at(&reg, a, b, now).is_none());
        call(
            &mut reg,
            "warRegistry",
            "GMSetDeclareTime",
            ALICE,
            vec![int(1), int(now - 2 * WAR_SPOOLUP)],
        )
        .expect("sets");
        assert!(war::at_war_at(&reg, a, b, now).is_some());
        // The defender gains a GM ally, activates and finishes it.
        call(
            &mut reg,
            "warRegistry",
            "GMJoinDefender",
            CAROL,
            vec![int(1), int(a), int(c)],
        )
        .expect("joins");
        assert!(load(&reg, 1).allies.contains_key(&c));
        assert!(war::at_war_at(&reg, c, a, now).is_none());
        call(
            &mut reg,
            "warRegistry",
            "GMActivateDefender",
            CAROL,
            vec![int(1), int(c)],
        )
        .expect("activates");
        let later = crate::pyutil::filetime_now() + 10_000_000;
        assert!(war::at_war_at(&reg, c, a, later).is_some());
        call(
            &mut reg,
            "warRegistry",
            "GMDeactivateDefender",
            CAROL,
            vec![int(1), int(c)],
        )
        .expect("deactivates");
        let later = crate::pyutil::filetime_now() + 10_000_000;
        assert!(war::at_war_at(&reg, c, a, later).is_none());
        call(
            &mut reg,
            "warRegistry",
            "GMExtendAllyContract",
            CAROL,
            vec![int(1), int(c), int(war::WEEK)],
        )
        .expect("extends");
        // Force finish: the war is over at once.
        call(
            &mut reg,
            "warRegistry",
            "GMSetWarFinishTime",
            ALICE,
            vec![int(1), int(now - crate::seed::DAY)],
        )
        .expect("finishes");
        assert!(war::at_war_at(&reg, a, b, now).is_none());
        assert_eq!(load(&reg, 1).reason_ended, Some(war::ended::GM_ACTION));
    }

    #[test]
    fn a_player_without_the_gm_role_is_refused_the_gm_calls() {
        let (mut reg, a, b) = two_corps();
        declare_on(&mut reg, ALICE, b).expect("declares");
        let req = Request {
            service: "warRegistry",
            method: "GMSetWarFinishTime",
            args: &[int(1), int(5)],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        let e = reg
            .dispatch_as(
                Caller::Session {
                    user_id: 7,
                    role: crate::roles::ROLE_PLAYER,
                },
                &req,
            )
            .expect_err("refused");
        assert_eq!(key_of(&e), "CrpAccessDenied");
        assert_eq!(load(&reg, 1).time_finished, None);
        let _ = (a, b);
    }

    // ---- alliances and corporations around a war ----------------------------

    fn create_alliance(reg: &mut Registry, who: i64, name: &str, short: &str) -> i64 {
        call(
            reg,
            "corpRegistry",
            "CreateAlliance",
            who,
            vec![uni(name), uni(short), uni(""), uni("")],
        )
        .expect("creates an alliance");
        reg.character(who)
            .and_then(|c| c.alliance_id)
            .expect("in it")
    }

    fn join_alliance(reg: &mut Registry, ceo: i64, alliance: i64, executor: i64, corp: i64) {
        call(
            reg,
            "corpRegistry",
            "ApplyToJoinAlliance",
            ceo,
            vec![int(alliance), uni("let us in")],
        )
        .expect("applies");
        let bind = PyValue::Tuple(vec![int(alliance), int(1)]);
        let req = Request {
            service: "allianceRegistry",
            method: "UpdateApplication",
            args: &[int(corp), uni("welcome"), int(2)],
            kwargs: &[],
            bind_params: Some(&bind),
            actor: Some(executor),
        };
        reg.dispatch(&req).expect("accepts");
    }

    fn quit_alliance(reg: &mut Registry, ceo: i64, corp: i64, alliance: i64) {
        let bind = PyValue::Tuple(vec![int(alliance), int(1)]);
        let req = Request {
            service: "allianceRegistry",
            method: "DeleteMember",
            args: &[int(corp)],
            kwargs: &[],
            bind_params: Some(&bind),
            actor: Some(ceo),
        };
        reg.dispatch(&req).expect("quits");
    }

    /// Four corps: Alice's, Bob's, Carol's and Dave's.
    fn four_corps() -> (Registry, [i64; 4]) {
        let mut reg = registry();
        let a = found_corp(&mut reg, ALICE, "Alpha Invented", "ALPH");
        let b = found_corp(&mut reg, BOB, "Bravo Invented", "BRAV");
        let c = found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        let d = found_corp(&mut reg, DAVE, "Delta Invented", "DELT");
        let _ = reg.take_pending();
        (reg, [a, b, c, d])
    }

    /// **The war bill survives an alliance join and leave**: the attacker's
    /// unpaid bill follows the adopted war, and a leaver that inherits an
    /// aggressor war owes its own bill, whose default ends the war.
    #[test]
    fn the_war_bill_survives_alliance_join_and_leave() {
        let (mut reg, [_a, b, c, d]) = four_corps();
        declare_on(&mut reg, BOB, c).expect("declares");
        let bill_a = load(&reg, 1).bill_id.expect("a bill");
        let x = create_alliance(&mut reg, ALICE, "Invented Pact", "INVP");
        join_alliance(&mut reg, CAROL, x, ALICE, c);
        let adopted = load(&reg, 2);
        assert_eq!(adopted.bill_id, Some(bill_a));
        let moved = crate::war_deal::bill(reg.deltas(), bill_a).expect("kept");
        assert_eq!((moved.war_id, moved.against, moved.paid), (2, x, false));
        // Due: Bob pays, and the next bill belongs to the adopted war.
        let before = balance(&reg, BOB);
        crate::war_deal::tick(&mut reg, moved.due + 1);
        assert_eq!(before - balance(&reg, BOB), COST_OF_WAR);
        let next = load(&reg, 2).bill_id.expect("next bill");
        assert_ne!(next, bill_a);
        assert_eq!(
            crate::war_deal::bill(reg.deltas(), next).map(|n| (n.war_id, n.debtor)),
            Some((2, b))
        );
        // The alliance declares on Dave's corp; Carol quits and owes a bill.
        let hq = wizard_hq(&mut reg, ALICE);
        let bind = PyValue::Tuple(vec![int(x), int(1)]);
        reg.dispatch(&Request {
            service: "allianceRegistry",
            method: "DeclareWarAgainst",
            args: &[int(d), int(hq)],
            kwargs: &[],
            bind_params: Some(&bind),
            actor: Some(ALICE),
        })
        .expect("the alliance declares");
        quit_alliance(&mut reg, CAROL, c, x);
        let inherited = war::Wars::load(reg.deltas())
            .all()
            .find(|w| w.reason_started == war::started::INHERITED && w.declared_by == c)
            .cloned()
            .expect("inherited");
        let owed =
            crate::war_deal::bill(reg.deltas(), inherited.bill_id.expect("billed")).expect("row");
        assert_eq!((owed.debtor, owed.war_id), (c, inherited.war_id));
        // Auto-pay off for Carol's corp: the bill falling due ends the war.
        reg.deltas_mut().put(
            crate::war_deal::KIND_AUTO,
            vec![c.to_string()],
            vec!["0".into()],
        );
        crate::war_deal::tick(&mut reg, owed.due + 1);
        assert_eq!(
            load(&reg, inherited.war_id).reason_ended,
            Some(war::ended::UNPAID_BILL)
        );
    }

    /// **An alliance adopts, inherits and closes wars**: a corp that joins is
    /// defended by its alliance in the war already on it; one that leaves
    /// takes the alliance's wars with it; the last one out ends them.
    #[test]
    fn an_alliance_adopts_inherits_and_closes_its_wars() {
        let (mut reg, [a, b, c, d]) = four_corps();
        // Bob's corp is at war with Carol's, hostile.
        declare_on(&mut reg, BOB, c).expect("declares");
        call(
            &mut reg,
            "warRegistry",
            "GMSetDeclareTime",
            BOB,
            vec![int(1), int(crate::pyutil::filetime_now() - 2 * WAR_SPOOLUP)],
        )
        .expect("sets");
        let x = create_alliance(&mut reg, ALICE, "Invented Pact", "INVP");
        join_alliance(&mut reg, CAROL, x, ALICE, c);
        // Adopted: a new war on the alliance, the corp's copy ended at once.
        let adopted = load(&reg, 2);
        assert_eq!((adopted.declared_by, adopted.against), (b, x));
        assert_eq!(adopted.reason_started, war::started::ADOPTION);
        assert_eq!(adopted.created_from, Some(1));
        assert_eq!(
            adopted.time_started,
            load(&reg, 1).time_started,
            "same dates"
        );
        assert_eq!(load(&reg, 1).reason_ended, Some(war::ended::WAR_ADOPTED));
        // The alliance's pilots and the corp's are at war with Bob's.
        let now = crate::pyutil::filetime_now();
        assert!(
            war::at_war_at(&reg, a, b, now).is_some(),
            "Alice's corp, by its alliance"
        );
        assert!(war::at_war_at(&reg, c, b, now).is_some());
        assert!(war::at_war_at(&reg, d, b, now).is_none());
        assert!(types_of(&reg, BOB).contains(&crate::war_notice::kind::WAR_ADOPTED));
        assert_keys(
            &reg,
            BOB,
            crate::war_notice::kind::WAR_ADOPTED,
            &["declaredByID", "againstID", "allianceID"],
        );
        // The alliance declares on Dave's corp, with a war HQ of its own.
        let hq = wizard_hq(&mut reg, ALICE);
        let bind = PyValue::Tuple(vec![int(x), int(1)]);
        reg.dispatch(&Request {
            service: "allianceRegistry",
            method: "DeclareWarAgainst",
            args: &[int(d), int(hq)],
            kwargs: &[],
            bind_params: Some(&bind),
            actor: Some(ALICE),
        })
        .expect("the alliance declares");
        let by_alliance = load(&reg, 3);
        assert_eq!((by_alliance.declared_by, by_alliance.against), (x, d));
        assert_eq!(by_alliance.war_hq, Some(hq));
        // Carol is not the executor: she cannot declare for the alliance.
        let e = reg
            .dispatch(&Request {
                service: "allianceRegistry",
                method: "DeclareWarAgainst",
                args: &[int(d), int(hq)],
                kwargs: &[],
                bind_params: Some(&bind),
                actor: Some(CAROL),
            })
            .expect_err("refused");
        assert_eq!(key_of(&e), "CrpAccessDenied");
        // A corp inside an alliance cannot declare for itself, nor be declared on.
        let e = declare_on(&mut reg, CAROL, d).expect_err("refused");
        assert_eq!(key_of(&e), "CrpCanNotSanctionWarOwner");
        let e = declare_on(&mut reg, DAVE, c).expect_err("refused");
        assert_eq!(key_of(&e), "CrpCanNotSanctionWarAgainstOwner");
        // Carol quits: she inherits the wars the alliance is in, both ways.
        quit_alliance(&mut reg, CAROL, c, x);
        let inherited: Vec<war::War> = war::Wars::load(reg.deltas())
            .all()
            .filter(|w| w.reason_started == war::started::INHERITED)
            .cloned()
            .collect();
        assert_eq!(inherited.len(), 2, "{inherited:?}");
        assert!(
            inherited
                .iter()
                .any(|w| w.declared_by == c && w.against == d && w.created_from == Some(3))
        );
        assert!(
            inherited
                .iter()
                .any(|w| w.declared_by == b && w.against == c && w.created_from == Some(2))
        );
        assert!(types_of(&reg, CAROL).contains(&crate::war_notice::kind::WAR_INHERITED));
        assert_keys(
            &reg,
            CAROL,
            crate::war_notice::kind::WAR_INHERITED,
            &[
                "declaredByID",
                "againstID",
                "allianceID",
                "quitterID",
                "opponentID",
            ],
        );
        // The last corp out closes the alliance, and its wars end.
        quit_alliance(&mut reg, ALICE, a, x);
        for id in [2, 3] {
            assert_eq!(
                load(&reg, id).reason_ended,
                Some(war::ended::ALLIANCE_DELETED),
                "{id}"
            );
        }
    }

    #[test]
    fn a_corp_with_wars_of_its_own_cannot_join_an_alliance() {
        let (mut reg, [a, b, c, d]) = four_corps();
        let x = create_alliance(&mut reg, ALICE, "Invented Pact", "INVP");
        let apply = |reg: &mut Registry, ceo: i64| {
            call(
                reg,
                "corpRegistry",
                "ApplyToJoinAlliance",
                ceo,
                vec![int(x), uni("")],
            )
        };
        // Bob declared a war: no.
        declare_on(&mut reg, BOB, d).expect("declares");
        assert_eq!(
            key_of(&apply(&mut reg, BOB).expect_err("refused")),
            "CantApplyToAllianceHaveDeclaredWars"
        );
        // The alliance is at war with Carol's corp: no.
        let hq = wizard_hq(&mut reg, ALICE);
        let bind = PyValue::Tuple(vec![int(x), int(1)]);
        reg.dispatch(&Request {
            service: "allianceRegistry",
            method: "DeclareWarAgainst",
            args: &[int(c), int(hq)],
            kwargs: &[],
            bind_params: Some(&bind),
            actor: Some(ALICE),
        })
        .expect("declares");
        assert_eq!(
            key_of(&apply(&mut reg, CAROL).expect_err("refused")),
            "CantApplyToAllianceYouAreAtWarWith"
        );
        // Dave's corp has a war on it but declared none: it may apply.
        apply(&mut reg, DAVE).expect("applies");
        // A mutual war blocks, with a different key at acceptance.
        let mutual = war::War {
            mutual: true,
            ..war::War::new(99, b, d, crate::pyutil::filetime_now(), 0, None)
        };
        war::put(reg.deltas_mut(), &mutual);
        assert_eq!(
            crate::war_deal::may_join_alliance(&reg, d, x, false),
            Err("CantApplyToAllianceHaveMutualWars")
        );
        assert_eq!(
            crate::war_deal::may_join_alliance(&reg, d, x, true),
            Err("CanNotJoinAllianceWithActiveMututalWars")
        );
        let _ = a;
    }

    /// A corp that closes ends its wars.
    #[test]
    fn a_closed_corp_ends_its_wars() {
        let (mut reg, a, b) = two_corps();
        declare_on(&mut reg, ALICE, b).expect("declares");
        call(
            &mut reg,
            "corpRegistry",
            "ResignFromCEO",
            BOB,
            vec![PyValue::None],
        )
        .expect("closes");
        let w = load(&reg, 1);
        assert_eq!(w.reason_ended, Some(war::ended::CORP_DELETED));
        assert!(
            w.time_finished
                .is_some_and(|f| f <= crate::pyutil::filetime_now())
        );
        assert!(types_of(&reg, ALICE).contains(&crate::war_notice::kind::WAR_INVALID));
        let _ = a;
    }

    /// A pilot who leaves a corp at war cannot join another for a week,
    /// unless the wars are over.
    #[test]
    fn a_pilot_cannot_dodge_a_war_by_changing_corps() {
        let (mut reg, a, b) = two_corps();
        declare_on(&mut reg, ALICE, b).expect("declares");
        let now = crate::pyutil::filetime_now();
        crate::war_deal::on_member_left(&mut reg, DAVE, a);
        assert_eq!(
            crate::war_deal::may_rejoin(&reg, DAVE, b),
            Err("CharacterDodgedWar")
        );
        assert_eq!(
            crate::war_deal::may_rejoin(&reg, DAVE, a),
            Ok(()),
            "back where they were"
        );
        // After the week, or when the war is over, the way is open.
        let until = reg.deltas().of(crate::war_deal::KIND_LEFT)[&vec![DAVE.to_string()]][1]
            .parse::<i64>()
            .expect("a time");
        assert!(until >= now + crate::war_deal::DODGE_PERIOD - 10_000_000);
        let mut w = load(&reg, 1);
        w.time_finished = Some(now - 1);
        war::put(reg.deltas_mut(), &w);
        assert_eq!(crate::war_deal::may_rejoin(&reg, DAVE, b), Ok(()));
        // A pilot who left a corp at peace is never remembered.
        crate::war_deal::on_member_left(&mut reg, CAROL, b);
        assert!(
            reg.deltas()
                .of(crate::war_deal::KIND_LEFT)
                .get(&vec![CAROL.to_string()])
                .is_none()
        );
        // The real path: a corp's last pilot leaving (a close) while at war.
        let (mut reg, _a, b) = two_corps();
        declare_on(&mut reg, ALICE, b).expect("declares");
        call(
            &mut reg,
            "corpRegistry",
            "ResignFromCEO",
            BOB,
            vec![PyValue::None],
        )
        .expect("closes");
        assert!(
            reg.deltas()
                .of(crate::war_deal::KIND_LEFT)
                .contains_key(&vec![BOB.to_string()])
        );
    }

    // ---- the war HQ's system ---------------------------------------------------

    const HIGH_SYSTEM: i64 = 30_000_501;
    const LOW_SYSTEM: i64 = 30_000_502;

    /// A registry whose universe table has an invented high-sec and an
    /// invented low-sec system.
    fn with_security() -> Registry {
        let row = |id: i64, sec: f64| {
            format!("{id}\t19000001\t29000001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx")
        };
        let u = crate::celestial::Universe::parse(
            &[row(HIGH_SYSTEM, 0.75), row(LOW_SYSTEM, 0.3)],
            &[],
            &[],
        )
        .expect("the invented rows parse");
        let mut content = crate::content::Content::fixture();
        content.insert("universe", u);
        let mut reg = registry().with_content(std::sync::Arc::new(content));
        for who in [ALICE, BOB, CAROL, DAVE] {
            reg.character_mut(who).expect("a pilot").solar_system_id = Some(HIGH_SYSTEM);
        }
        reg
    }

    /// The war HQ must be in high-sec; when it leaves, its wars end with
    /// `ENDED_WAR_HQ_SYSTEM_SECURITY_DROP`, the parties are told (246), the
    /// aggressor is barred, and the corp's permit goes with it.
    #[test]
    fn an_hq_that_leaves_high_sec_ends_its_wars_and_the_permit() {
        let mut reg = with_security();
        let a = found_corp(&mut reg, ALICE, "Alpha Invented", "ALPH");
        let b = found_corp(&mut reg, BOB, "Bravo Invented", "BRAV");
        // A director docked in low-sec is offered no war HQ.
        reg.character_mut(CAROL).expect("carol").solar_system_id = Some(LOW_SYSTEM);
        found_corp(&mut reg, CAROL, "Charlie Invented", "CHAR");
        let list = call(
            &mut reg,
            "structureDirectory",
            "GetValidWarHQs",
            CAROL,
            vec![int(0)],
        )
        .expect("answers");
        assert_eq!(list, PyValue::List(vec![]));
        declare_on(&mut reg, ALICE, b).expect("declares");
        assert!(war::corp_permit(&reg, a));
        let _ = reg.take_pending();
        let hq = war::hq_of(reg.deltas(), a).expect("an HQ");
        assert_eq!(hq.system_id, HIGH_SYSTEM);
        war::move_hq(&mut reg, a, 60_000_777, LOW_SYSTEM, war::Via::Call);
        let w = load(&reg, 1);
        assert_eq!(
            w.reason_ended,
            Some(war::ended::WAR_HQ_SYSTEM_SECURITY_DROP)
        );
        assert!(crate::war_deal::treaty_between(
            reg.deltas(),
            a,
            b,
            crate::pyutil::filetime_now()
        ));
        assert!(!crate::war_deal::treaty_between(
            reg.deltas(),
            b,
            a,
            crate::pyutil::filetime_now()
        ));
        for who in [ALICE, BOB] {
            assert!(
                types_of(&reg, who).contains(&crate::war_notice::kind::WAR_ENDED_HQ_SECURITY_DROP),
                "{who}"
            );
        }
        // The notice carries the HQ the way its formatter unpacks it.
        let n = store::of(reg.deltas(), BOB)
            .into_iter()
            .find(|n| n.type_id == crate::war_notice::kind::WAR_ENDED_HQ_SECURITY_DROP)
            .expect("told");
        let hq_type = n
            .data
            .iter()
            .find(|(k, _)| k == "warHQ_IdType")
            .map(|(_, v)| v.clone());
        assert_eq!(
            hq_type,
            Some(store::Value::List(vec![
                store::Value::Int(hq.item_id),
                store::Value::Int(war::TYPE_WAR_HQ)
            ]))
        );
        // The permit flipped: the corp's row says so and the push went out.
        assert!(!war::corp_permit(&reg, a));
        assert!(
            event_names(&mut reg)
                .iter()
                .any(|(e, _)| *e == "OnAllowWarUpdated")
        );
        // And the wizard offers it nothing while the HQ is there.
        let list = call(
            &mut reg,
            "structureDirectory",
            "GetValidWarHQs",
            ALICE,
            vec![int(0)],
        )
        .expect("answers");
        assert_eq!(list, PyValue::List(vec![]));
        // A new HQ in high-sec turns the permit back on.
        war::move_hq(&mut reg, a, 60_000_888, HIGH_SYSTEM, war::Via::Call);
        assert!(war::corp_permit(&reg, a));
    }

    // ---- war kills and the report -------------------------------------------------

    const HULL: i64 = 990_660_001;
    const GUN: i64 = 990_660_002;

    /// Alice (corp A) and Bob (corp B) in space in one system, at war, past
    /// the spool-up.
    fn war_in_space() -> (Registry, i64, i64) {
        let (mut reg, a, b) = two_corps();
        hostile_war(&mut reg, a, b);
        let mut items = reg.store().items().to_vec();
        for i in items
            .iter_mut()
            .filter(|i| i.item_id == ALICE + 10_000_000 || i.item_id == BOB + 10_000_000)
        {
            i.type_id = HULL;
        }
        let mut snap = reg.to_snapshot();
        snap.items = items;
        snap.next_item_id = 100_000_900;
        reg.apply_snapshot(snap);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        (reg, a, b)
    }

    fn volley(reg: &mut Registry, shooter: i64, victim: i64, em: f64) {
        use crate::damage::{Damage, Shot};
        let t0 = crate::pyutil::filetime_now();
        let v = reg.roster().get(victim).copied().expect("in space");
        let from = reg.roster().get(shooter).map_or(0, |p| p.ship_id);
        let shot = Shot {
            at: t0,
            attacker_ship_id: from,
            weapon_item_id: 1,
            weapon_type_id: GUN,
            target_ship_id: v.ship_id,
            damage: Damage {
                em,
                explosive: 0.0,
                kinetic: 0.0,
                thermal: 0.0,
            },
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(reg, shot, shooter, v, t0);
    }

    fn bound(
        reg: &mut Registry,
        service: &str,
        method: &str,
        who: i64,
        war: i64,
        args: Vec<PyValue>,
    ) -> PyValue {
        let bind = int(war);
        reg.dispatch(&Request {
            service,
            method,
            args: &args,
            kwargs: &[],
            bind_params: Some(&bind),
            actor: Some(who),
        })
        .expect("answers")
    }

    /// **A kill in a war**, through the real combat path: the mail is filed
    /// under the war, its `warID` says so, and the report's three calls add
    /// it up.
    #[test]
    fn a_kill_in_a_war_is_filed_under_it_and_reported() {
        let (mut reg, a, b) = war_in_space();
        let now = crate::pyutil::filetime_now();
        assert!(
            war::legal(&reg, ALICE, BOB, now).is_some(),
            "legal for crimewatch"
        );
        assert_eq!(war::legal(&reg, ALICE, BOB, now), Some(1));
        volley(&mut reg, ALICE, BOB, 1.0e9);
        let report = crate::killmail::involving(&reg, BOB, None, 1)
            .pop()
            .expect("a mail");
        assert_eq!(war::war_of_kill(&reg, report.kill_id), Some(1));
        assert_eq!(war::kills_of_war(reg.deltas(), 1), vec![report.kill_id]);
        // The kill report window gets the war.
        let mail = call(
            &mut reg,
            "warStatisticMgr",
            "GetKillMail",
            BOB,
            vec![int(report.kill_id), uni(&store::kill_hash(&report))],
        )
        .expect("opens");
        assert_eq!(attr(&mail, "warID"), &int(1));
        // The war report: Alice's corp killed one ship.
        let info = bound(&mut reg, "warStatisticMgr", "GetBaseInfo", ALICE, 1, vec![]);
        let PyValue::Tuple(t) = &info else {
            panic!("tuple")
        };
        assert_eq!(t.len(), 4);
        assert_eq!(attr(&t[0], "warID"), &int(1));
        assert_eq!(get(&t[1], a), Some(&int(1)));
        assert_eq!(get(&t[1], b), Some(&int(0)));
        let kills = bound(
            &mut reg,
            "warStatisticMgr",
            "GetKills",
            ALICE,
            1,
            vec![PyValue::None, PyValue::None],
        );
        let PyValue::List(rows) = &kills else {
            panic!("list")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(attr(&rows[0], "killID"), &int(report.kill_id));
        assert_eq!(attr(&rows[0], "warID"), &int(1));
        // The defender's own kills: none.
        let theirs = bound(
            &mut reg,
            "warStatisticMgr",
            "GetKills",
            ALICE,
            1,
            vec![int(b), PyValue::None],
        );
        assert_eq!(theirs, PyValue::List(vec![]));
        // By group: keyed by inventory group, from the type table (this
        // registry's table has no such hull, so the mail adds nothing).
        let by = bound(
            &mut reg,
            "warStatisticMgr",
            "GetKillsByGroup",
            ALICE,
            1,
            vec![],
        );
        assert_eq!(by, PyValue::Dict(vec![]));
        // Wars with no kill bring back no rows.
        let none = bound(
            &mut reg,
            "warStatisticMgr",
            "GetKills",
            ALICE,
            1,
            vec![int(a), int(3)],
        );
        assert_eq!(none, PyValue::List(vec![]));
    }

    /// A kill between two who are not at war is nobody's war kill.
    #[test]
    fn a_kill_outside_any_war_is_not_filed() {
        let (mut reg, a, b) = two_corps();
        // A spool-up war is not a war yet.
        declare_on(&mut reg, ALICE, b).expect("declares");
        let mut items = reg.store().items().to_vec();
        for i in items
            .iter_mut()
            .filter(|i| i.item_id == ALICE + 10_000_000 || i.item_id == BOB + 10_000_000)
        {
            i.type_id = HULL;
        }
        let mut snap = reg.to_snapshot();
        snap.items = items;
        snap.next_item_id = 100_000_900;
        reg.apply_snapshot(snap);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let now = crate::pyutil::filetime_now();
        assert_eq!(war::legal(&reg, ALICE, BOB, now), None);
        volley(&mut reg, ALICE, BOB, 1.0e9);
        let report = crate::killmail::involving(&reg, BOB, None, 1)
            .pop()
            .expect("a mail");
        assert_eq!(war::war_of_kill(&reg, report.kill_id), None);
        let _ = (a, b);
    }

    /// `GetWarsByOwnerID` is a rowset the client can `.Index('warID')`, and
    /// the lists the all-wars window pages through carry the 17 fields.
    #[test]
    fn the_wars_info_calls_answer_in_the_shapes_the_windows_read() {
        let (mut reg, a, b) = two_corps();
        declare_on(&mut reg, ALICE, b).expect("declares");
        // A plain `CRowset` (the client calls `.Index('warID')` on it): its
        // header is the war columns, its one row is the war.
        let rowset = |v: &PyValue| -> (PyValue, usize) {
            let PyValue::ObjectEx { header, .. } = v else {
                panic!("a rowset object: {v:?}")
            };
            let PyValue::Tuple(parts) = header.as_ref() else {
                panic!("header")
            };
            let PyValue::Tuple(args) = &parts[1] else {
                panic!("args")
            };
            let PyValue::List(rows) = &args[1] else {
                panic!("rows")
            };
            (args[0].clone(), rows.len())
        };
        let rows = call(
            &mut reg,
            "warsInfoMgr",
            "GetWarsByOwnerID",
            BOB,
            vec![int(b)],
        )
        .expect("answers");
        let (header, n) = rowset(&rows);
        assert_eq!(header, descriptor(WAR_COLUMNS).to_pyvalue());
        assert_eq!(n, 1);
        let empty = call(
            &mut reg,
            "warsInfoMgr",
            "GetWarsByOwnerID",
            BOB,
            vec![int(a + 5)],
        )
        .expect("answers");
        let (header, n) = rowset(&empty);
        assert_eq!(
            header,
            descriptor(WAR_COLUMNS).to_pyvalue(),
            "an empty rowset keeps its header"
        );
        assert_eq!(n, 0);
        let by = call(
            &mut reg,
            "warsInfoMgr",
            "GetWarsByOwners",
            BOB,
            vec![PyValue::List(vec![int(a), int(b), int(77)])],
        )
        .expect("answers");
        assert_eq!(get(&by, 77), Some(&PyValue::Dict(vec![])));
        assert!(get(get(&by, a).expect("a"), 1).is_some());
        let top = call(
            &mut reg,
            "warsInfoMgr",
            "GetTop50",
            BOB,
            vec![int(i64::MAX)],
        )
        .expect("answers");
        let PyValue::List(l) = &top else {
            panic!("list")
        };
        assert_eq!(l.len(), 1);
        assert_eq!(attr(&l[0], "warID"), &int(1));
        let below = call(&mut reg, "warsInfoMgr", "GetTop50", BOB, vec![int(1)]).expect("answers");
        assert_eq!(below, PyValue::List(vec![]));
        let hq = war::hq_of(reg.deltas(), a).expect("hq").item_id;
        let at = call(
            &mut reg,
            "warsInfoMgr",
            "GetWarsForStructure",
            BOB,
            vec![int(hq)],
        )
        .expect("answers");
        assert!(matches!(&at, PyValue::List(l) if l.len() == 1));
        let public = call(
            &mut reg,
            "warsInfoMgr",
            "GetPublicWarInfo",
            BOB,
            vec![PyValue::List(vec![int(1), int(9)])],
        )
        .expect("answers");
        assert!(matches!(&public, PyValue::List(l) if l.len() == 1));
        // Not bound on a war, the report calls are refused.
        let e = reg.dispatch(&Request {
            service: "warStatisticMgr",
            method: "GetBaseInfo",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        });
        assert!(e.is_err());
    }
}
