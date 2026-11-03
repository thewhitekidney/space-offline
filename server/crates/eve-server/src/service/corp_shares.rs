//! **The shares and dividends wire** (lane `s7`, #178): the `corpRegistry`
//! calls the Shares and Shareholders panels and the CEO's Dividends button
//! make, answered from `crate::corp_shares`. `service/corp.rs`'s
//! `corp_registry` routes [`CORP_REGISTRY_METHODS`] here.
//!
//! | call | answer |
//! | --- | --- |
//! | `GetSharesByShareholder(corpShares)` | a `CRowset` of `shareholderID corporationID shares`, the caller's (0) or the caller's corp's (1) holdings |
//! | `GetShareholders(corpID)` | a dict `shareholderID -> KeyVal(shareholderID, shareholderCorporationID, corporationID, shares)` |
//! | `MoveCompanyShares(corpID, to, n)` | `None`; a Director gives the corp's own holding away |
//! | `MovePrivateShares(corpID, to, n)` | `None`; a pilot gives their own holding away |
//! | `PayoutDividend(payShareholders, amount)` | `None`; the CEO pays out of the Master Wallet |
//!
//! Pushes: `OnShareChange(shareholderID, corporationID, change)` to each
//! holder a move touched, `change` a dict of `column -> (old, new)` where an
//! all-`None` old side is an add and an all-`None` new side a remove
//! (`bco_base.GetAddRemoveFromChange`). A dividend credits each pilot's
//! wallet (journal ref 38) and sends a type-23 notification.
//!
//! **Who is paid.** `payShareholders` is `1` for shareholders and `0` for
//! members (`corp_ui_home.PayoutDividendForm`: `[1, 0][retval['OdividendType']]`
//! with the shareholder radio first). A corp holding shares is paid into its
//! Master Wallet. A holder that is no pilot or live corp on this server is
//! skipped and its cut stays in the wallet, as does the corp's own and any
//! remainder under a cent. The whole payout is refused, nothing moved, when
//! the Master Wallet holds less than the amount.
//!
//! **[A]** The notification's `payout` is the total paid out, in ISK (the
//! label says "a total payout of {payout}"). Falsifier: the notification
//! text reading blank or as a different figure in the mail window.
//! **[A]** `shareholderCorporationID` is the holder's current corp (0 for a
//! corp holder), because the panel shows it as the Corporation column and
//! falls back to `corporationID` when it is falsy. Falsifier: the panel
//! showing a wrong corporation beside a pilot.

use super::*;
use crate::corp_shares::{self as cs, Change, Refusal};
use crate::corp_wallet as cw;
use crate::notifications::{self as notif, Value};
use eve_marshal::{Column, DbRowDescriptor, DbType, DbValue};

pub(crate) const CORP_REGISTRY_METHODS: &[&str] = &[
    "GetSharesByShareholder",
    "GetShareholders",
    "MoveCompanyShares",
    "MovePrivateShares",
    "PayoutDividend",
];

/// `refCorporationDividendPayment` **[V]**.
pub(crate) const REF_CORPORATION_DIVIDEND_PAYMENT: i64 = 38;

/// A share push: the event, its arguments, and who hears it.
#[derive(Debug, Clone, PartialEq)]
pub struct SharePush {
    pub event: &'static str,
    pub args: Vec<PyValue>,
    pub to: Vec<i64>,
}

impl crate::notify::Push for SharePush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

fn raise(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

fn custom(text: &str) -> ServiceError {
    raise("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

fn denied(reason: &str) -> ServiceError {
    raise(
        "CrpAccessDenied",
        vec![("reason", PyValue::Unicode(reason.to_owned()))],
    )
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn real(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(x) => Some(*x),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args
        .get(i)
        .or_else(|| crate::pyutil::pairs_get(req.kwargs, name.as_bytes()))
}

/// The caller's player corp, if they are in one.
fn my_corp(reg: &Registry, me: i64) -> Option<i64> {
    let m = reg.corporations().membership(me)?;
    reg.corporations()
        .get(m.corporation_id)
        .filter(|c| !c.deleted)
        .map(|c| c.corporation_id)
}

/// Issue `corp`'s shares if it has none yet (a corp founded before shares).
fn ensure_founded(reg: &mut Registry, corp: i64) {
    if cs::is_founded(reg.deltas(), corp) {
        return;
    }
    if let Some(ceo) = reg
        .corporations()
        .get(corp)
        .filter(|c| !c.deleted)
        .map(|c| c.ceo_id)
    {
        cs::found(reg.deltas_mut(), corp, ceo);
    }
}

/// A pilot with a record here that is not a tombstone (lane S3 keeps a
/// deleted character in the registry), or a live player corp.
fn holder_exists(reg: &Registry, id: i64) -> bool {
    is_live_pilot(reg, id) || reg.corporations().get(id).is_some_and(|c| !c.deleted)
}

fn is_live_pilot(reg: &Registry, id: i64) -> bool {
    reg.character(id).is_some() && !crate::start::is_deleted(reg.deltas(), id)
}

fn is_corp(reg: &Registry, id: i64) -> bool {
    reg.corporations().get(id).is_some_and(|c| !c.deleted)
}

/// Who hears about a holder's change: the pilot, or a corp's members.
fn listeners(reg: &Registry, holder: i64) -> Vec<i64> {
    if is_corp(reg, holder) {
        reg.corporations().members_of(holder)
    } else {
        vec![holder]
    }
}

fn describe(columns: &[(&str, DbType)]) -> DbRowDescriptor {
    DbRowDescriptor::new(
        columns
            .iter()
            .map(|(name, db_type)| Column {
                name: name.as_bytes().to_vec(),
                db_type: *db_type,
            })
            .collect(),
    )
    .expect("the share columns are a valid layout")
}

/// `GetSharesByShareholder`: **a `CRowset`**, because the client calls
/// `.Index('corporationID')` on it and `sharesPanel` then `.itervalues()`
/// on that (`bco_shares.GetSharesByShareholder`); the columns are declared
/// even with no rows. The argument arrives as a `Bool`.
fn shares_rowset(reg: &Registry, holder: i64) -> PyValue {
    let mut rs = eve_rowset::CRowset::new(describe(&[
        ("shareholderID", DbType::I4),
        ("corporationID", DbType::I4),
        ("shares", DbType::I8),
    ]));
    for (corp, n) in cs::held_by(reg.deltas(), holder) {
        rs.push_values(vec![
            DbValue::Int(holder),
            DbValue::Int(corp),
            DbValue::Int(n),
        ])
        .expect("the share row matches its descriptor");
    }
    rs.to_pyvalue()
}

fn get_shares(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        return Ok(shares_rowset(reg, 0));
    };
    if let Some(corp) = my_corp(reg, me) {
        ensure_founded(reg, corp);
    }
    let corp_wide = int(arg(req, 0, "corpShares")).is_some_and(|n| n != 0);
    let holder = if corp_wide {
        my_corp(reg, me).unwrap_or(0)
    } else {
        me
    };
    Ok(shares_rowset(reg, holder))
}

fn get_shareholders(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        return Ok(PyValue::Dict(vec![]));
    };
    let corp = int(arg(req, 0, "corporationID")).unwrap_or(0);
    if my_corp(reg, me) != Some(corp) {
        return Ok(PyValue::Dict(vec![]));
    }
    ensure_founded(reg, corp);
    let rows = cs::holders(reg.deltas(), corp)
        .into_iter()
        .map(|(holder, n)| {
            let home = if is_corp(reg, holder) {
                0
            } else {
                reg.corporations()
                    .membership(holder)
                    .map_or(0, |m| m.corporation_id)
            };
            // A `PyValue::Dict` pair is (value, key): the wire order.
            (
                crate::pyutil::keyval(vec![
                    ("shareholderID", PyValue::Int(holder)),
                    ("shareholderCorporationID", PyValue::Int(home)),
                    ("corporationID", PyValue::Int(corp)),
                    ("shares", PyValue::Int(n)),
                ]),
                PyValue::Int(holder),
            )
        })
        .collect();
    Ok(PyValue::Dict(rows))
}

fn refusal(r: Refusal) -> ServiceError {
    match r {
        Refusal::NothingToMove => custom("Enter a number of shares to give."),
        Refusal::MoreThanHeld { held } => custom(&format!("You only hold {held} of those shares.")),
        Refusal::ToSelf => custom("Those shares already belong to that owner."),
    }
}

/// `(old, new)` as the wire pair, `None` for no row.
fn pair(old: i64, new: i64) -> PyValue {
    let side = |n: i64| {
        if n > 0 {
            PyValue::Int(n)
        } else {
            PyValue::None
        }
    };
    PyValue::Tuple(vec![side(old), side(new)])
}

/// `OnShareChange` to whoever holds the changed shares.
fn tell_change(reg: &mut Registry, corp: i64, c: Change) {
    let to = listeners(reg, c.holder);
    if to.is_empty() {
        return;
    }
    // Dict pairs are (value, key): the wire order.
    let change = PyValue::Dict(vec![
        (
            PyValue::Tuple(vec![
                if c.old > 0 {
                    PyValue::Int(c.holder)
                } else {
                    PyValue::None
                },
                if c.new > 0 {
                    PyValue::Int(c.holder)
                } else {
                    PyValue::None
                },
            ]),
            crate::pyutil::bstr("shareholderID"),
        ),
        (
            PyValue::Tuple(vec![
                if c.old > 0 {
                    PyValue::Int(corp)
                } else {
                    PyValue::None
                },
                if c.new > 0 {
                    PyValue::Int(corp)
                } else {
                    PyValue::None
                },
            ]),
            crate::pyutil::bstr("corporationID"),
        ),
        (pair(c.old, c.new), crate::pyutil::bstr("shares")),
    ]);
    reg.schedule(
        std::time::Duration::ZERO,
        SharePush {
            event: "OnShareChange",
            args: vec![PyValue::Int(c.holder), PyValue::Int(corp), change],
            to,
        },
    );
}

fn do_move(
    reg: &mut Registry,
    req: &Request<'_>,
    me: i64,
    from_corp_holding: bool,
) -> Result<PyValue, ServiceError> {
    let corp = int(arg(req, 0, "corporationID")).unwrap_or(0);
    let to = int(arg(req, 1, "toShareholderID")).unwrap_or(0);
    let n = int(arg(req, 2, "numberOfShares")).unwrap_or(0);
    let giver = if from_corp_holding {
        const NEED: &str = "You need the Director role to give away the corporation's shares.";
        let m = reg
            .corporations()
            .membership(me)
            .ok_or_else(|| denied(NEED))?;
        let is_ceo = reg
            .corporations()
            .get(m.corporation_id)
            .is_some_and(|c| c.ceo_id == me);
        if !is_ceo && m.roles & super::corp::CORP_ROLE_DIRECTOR == 0 {
            return Err(denied(NEED));
        }
        my_corp(reg, me).ok_or_else(|| denied(NEED))?
    } else {
        me
    };
    if !is_corp(reg, corp) {
        return Err(custom("That corporation does not exist."));
    }
    ensure_founded(reg, corp);
    if !holder_exists(reg, to) {
        return Err(custom("That owner does not exist."));
    }
    let (a, b) = cs::move_shares(reg.deltas_mut(), corp, giver, to, n).map_err(refusal)?;
    tell_change(reg, corp, a);
    tell_change(reg, corp, b);
    Ok(PyValue::None)
}

/// `PayoutDividend(payShareholders, amount)`: see the module docs.
fn payout(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let corp = my_corp(reg, me).filter(|c| {
        reg.corporations()
            .get(*c)
            .is_some_and(|corp| corp.ceo_id == me)
    });
    let Some(corp) = corp else {
        return Err(denied("Only the CEO can pay out dividends."));
    };
    ensure_founded(reg, corp);
    let to_shareholders = int(arg(req, 0, "payShareholders")).is_some_and(|n| n != 0);
    let cents = real(arg(req, 1, "payoutAmount")).map_or(0, cw::to_cents);
    if cents < 1 {
        return Err(custom("Enter an amount to pay out."));
    }
    let have = cw::balance(reg.deltas(), corp, cw::MASTER_KEY);
    if cents > have {
        return Err(raise(
            "NotEnoughMoney",
            vec![
                ("amount", PyValue::Real(cw::to_isk(cents))),
                ("balance", PyValue::Real(cw::to_isk(have))),
            ],
        ));
    }
    let paid = if to_shareholders {
        let holders: Vec<(i64, i64)> = cs::holders(reg.deltas(), corp)
            .into_iter()
            .filter(|(who, _)| *who != corp && holder_exists(reg, *who))
            .collect();
        cs::split_by_weight(cents, &holders, cs::issued(reg.deltas(), corp))
    } else {
        let members: Vec<i64> = reg
            .corporations()
            .members_of(corp)
            .into_iter()
            .filter(|m| is_live_pilot(reg, *m))
            .collect();
        cs::split_evenly(cents, &members)
    };
    if paid.is_empty() {
        return Err(custom("That amount is too small to pay anyone."));
    }
    let total_cents: i64 = paid.iter().map(|p| p.1).sum();
    let total_isk = cw::to_isk(total_cents);
    let mut told = Vec::new();
    for (who, c) in paid {
        let isk = cw::to_isk(c);
        let took = super::corp_wallet::transfer(
            reg,
            corp,
            cw::MASTER_KEY,
            -isk,
            REF_CORPORATION_DIVIDEND_PAYMENT,
            (corp, who),
            corp,
            false,
        );
        if took.is_none() {
            continue;
        }
        if is_corp(reg, who) {
            super::corp_wallet::transfer(
                reg,
                who,
                cw::MASTER_KEY,
                isk,
                REF_CORPORATION_DIVIDEND_PAYMENT,
                (corp, who),
                corp,
                false,
            );
        } else {
            // The corp pays, the pilot is paid: the row is the pilot's.
            let paid = super::corp_wallet::char_post(
                reg,
                super::corp_wallet::Via::Call,
                who,
                isk,
                REF_CORPORATION_DIVIDEND_PAYMENT,
                corp,
                who,
                corp,
                "",
            );
            if paid.is_some() {
                told.push(who);
            }
        }
    }
    for who in told {
        notif::notify_from(
            reg,
            who,
            notif::kind::CORP_DIVIDEND,
            corp,
            notif::data([
                ("corpID", Value::Int(corp)),
                ("isMembers", Value::Int(i64::from(!to_shareholders))),
                ("payout", Value::Real(total_isk)),
            ]),
        );
    }
    Ok(PyValue::None)
}

/// The `corpRegistry` arms in [`CORP_REGISTRY_METHODS`].
pub(crate) fn corp_registry(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetSharesByShareholder" => get_shares(reg, req),
        "GetShareholders" => get_shareholders(reg, req),
        _ => {
            let Some(me) = req.actor else {
                return Err(reg.no_method(req));
            };
            match req.method {
                "MoveCompanyShares" => do_move(reg, req, me, true),
                "MovePrivateShares" => do_move(reg, req, me, false),
                "PayoutDividend" => payout(reg, req, me),
                _ => Err(reg.no_method(req)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids: nothing here is a client row.
    const ALICE: i64 = 90_000_781;
    const BOB: i64 = 90_000_782;
    const CAROL: i64 = 90_000_783;
    const DAVE: i64 = 90_000_784;

    fn pilot(id: i64, name: &str) -> crate::model::Character {
        crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 100_000_000,
            ..crate::model::Character::default()
        }
    }

    /// ALICE the CEO of a corp, BOB and CAROL plain members, DAVE outside it.
    fn world() -> (Registry, i64) {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(ALICE, "Alice"),
                pilot(BOB, "Bob"),
                pilot(CAROL, "Carol"),
                pilot(DAVE, "Dave"),
            ];
        });
        super::super::corp::dev_join(&mut reg, ALICE, "TST");
        super::super::corp::dev_join(&mut reg, BOB, "TST");
        super::super::corp::dev_join(&mut reg, CAROL, "TST");
        let corp = reg.corporations().membership(ALICE).unwrap().corporation_id;
        (reg, corp)
    }

    fn fund(reg: &mut Registry, corp: i64, isk: f64) {
        cw::adjust(reg.deltas_mut(), corp, cw::MASTER_KEY, cw::to_cents(isk)).expect("funded");
    }

    fn call(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service: "corpRegistry",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
    }

    fn i(n: i64) -> PyValue {
        PyValue::Int(n)
    }

    fn key_of(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("not a raised exception: {e:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("header")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("args")
        };
        match &args[0] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("the key is a str: {other:?}"),
        }
    }

    fn cash(reg: &Registry, who: i64) -> i64 {
        cw::to_cents(reg.character(who).expect("a pilot").balance)
    }

    fn wallet(reg: &Registry, corp: i64) -> i64 {
        cw::balance(reg.deltas(), corp, cw::MASTER_KEY)
    }

    fn pushes(reg: &mut Registry) -> Vec<(&'static str, Vec<i64>)> {
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1))
            .iter()
            .map(|p| (p.to_push().0, p.audience().unwrap_or_default()))
            .collect()
    }

    fn issued(reg: &Registry, corp: i64) -> i64 {
        cs::issued(reg.deltas(), corp)
    }

    /// The cut `held` of `issued` shares gets of `cents`, rounded down.
    fn cut(cents: i64, held: i64, issued: i64) -> i64 {
        (i128::from(cents) * i128::from(held) / i128::from(issued)) as i64
    }

    /// Mark `who` a deleted character, as lane S3's `DeleteCharacter` does
    /// (they stay in the registry).
    fn tombstone(reg: &mut Registry, who: i64) {
        reg.deltas_mut().put(
            crate::start::DELETE_KIND,
            vec![who.to_string()],
            vec!["1".to_owned(), "1".to_owned()],
        );
    }

    #[test]
    fn a_new_corp_gives_its_ceo_the_founding_shares() {
        let (reg, corp) = world();
        assert_eq!(
            cs::held(reg.deltas(), corp, ALICE),
            cs::FOUNDING_SHARES,
            "the CEO holds the founding grant"
        );
        assert_eq!(cs::held(reg.deltas(), corp, BOB), 0);
    }

    #[test]
    fn the_share_calls_answer_what_was_issued() {
        let (mut reg, corp) = world();
        let rows = call(&mut reg, ALICE, "GetShareholders", &[i(corp)]).unwrap();
        let PyValue::Dict(rows) = rows else {
            panic!("itervalues wants a dict")
        };
        assert_eq!(rows.len(), 1);
        // (value, key): the client's itervalues() must see the KeyVal.
        assert_eq!(rows[0].1, i(ALICE));
        let PyValue::Instance { state, .. } = &rows[0].0 else {
            panic!("a KeyVal row")
        };
        for (name, want) in [
            ("shareholderID", ALICE),
            ("corporationID", corp),
            ("shares", cs::FOUNDING_SHARES),
        ] {
            assert_eq!(
                crate::pyutil::dict_get(state, name.as_bytes()),
                Some(&i(want)),
                "{name}"
            );
        }
        // The row carries the holder's corp column; its value is an [A]
        // (module docs), so only its presence is checked.
        assert!(
            crate::pyutil::dict_get(state, b"shareholderCorporationID").is_some(),
            "the Shareholders panel reads shareholderCorporationID"
        );
        // An outsider sees no list.
        assert_eq!(
            call(&mut reg, DAVE, "GetShareholders", &[i(corp)]).unwrap(),
            PyValue::Dict(vec![])
        );
        // A CRowset comes back for the caller's own holdings (the columns
        // are what `.Index('corporationID')` reads).
        let mine = call(
            &mut reg,
            ALICE,
            "GetSharesByShareholder",
            &[PyValue::Bool(false)],
        );
        assert!(matches!(mine, Ok(PyValue::ObjectEx { .. })));
    }

    #[test]
    fn moving_more_than_held_is_refused() {
        let (mut reg, corp) = world();
        let err = call(
            &mut reg,
            ALICE,
            "MovePrivateShares",
            &[i(corp), i(BOB), i(cs::FOUNDING_SHARES + 1)],
        )
        .expect_err("more than she holds");
        assert_eq!(key_of(&err), "CustomNotify");
        assert_eq!(cs::held(reg.deltas(), corp, ALICE), cs::FOUNDING_SHARES);
        assert_eq!(cs::held(reg.deltas(), corp, BOB), 0);
        assert!(pushes(&mut reg).iter().all(|p| p.0 != "OnShareChange"));
        // Bob holds none to give.
        assert!(
            call(
                &mut reg,
                BOB,
                "MovePrivateShares",
                &[i(corp), i(CAROL), i(1)]
            )
            .is_err()
        );
        // Nobody can be given shares that does not exist.
        assert!(call(&mut reg, ALICE, "MovePrivateShares", &[i(corp), i(1), i(5)]).is_err());
    }

    #[test]
    fn a_move_tells_both_holders() {
        let (mut reg, corp) = world();
        let total = issued(&reg, corp);
        let quarter = total / 4;
        pushes(&mut reg);
        call(
            &mut reg,
            ALICE,
            "MovePrivateShares",
            &[i(corp), i(DAVE), i(quarter)],
        )
        .unwrap();
        assert_eq!(cs::held(reg.deltas(), corp, ALICE), total - quarter);
        assert_eq!(cs::held(reg.deltas(), corp, DAVE), quarter);
        let got: Vec<_> = pushes(&mut reg)
            .into_iter()
            .filter(|p| p.0 == "OnShareChange")
            .collect();
        assert_eq!(
            got,
            vec![
                ("OnShareChange", vec![ALICE]),
                ("OnShareChange", vec![DAVE])
            ]
        );
    }

    #[test]
    fn the_change_dict_is_wire_ordered_value_then_key() {
        let (mut reg, corp) = world();
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1));
        call(
            &mut reg,
            ALICE,
            "MovePrivateShares",
            &[i(corp), i(DAVE), i(1)],
        )
        .unwrap();
        let due = reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1));
        let (_, args) = due[0].to_push();
        let PyValue::Dict(rows) = &args[2] else {
            panic!("change is a dict")
        };
        // Each pair is (value, key); the client indexes it by the name.
        let keys: Vec<_> = rows.iter().map(|(_, k)| k.clone()).collect();
        for name in ["shareholderID", "corporationID", "shares"] {
            assert!(keys.contains(&crate::pyutil::bstr(name)), "{name} is a key");
        }
    }

    #[test]
    fn only_a_director_gives_the_corps_own_shares() {
        let (mut reg, corp) = world();
        cs::move_shares(reg.deltas_mut(), corp, ALICE, corp, 100).unwrap();
        let err = call(
            &mut reg,
            BOB,
            "MoveCompanyShares",
            &[i(corp), i(CAROL), i(10)],
        )
        .expect_err("a plain member");
        assert_eq!(key_of(&err), "CrpAccessDenied");
        assert_eq!(cs::held(reg.deltas(), corp, corp), 100);
        // The CEO may; and not more than the corp holds.
        assert!(
            call(
                &mut reg,
                ALICE,
                "MoveCompanyShares",
                &[i(corp), i(CAROL), i(101)]
            )
            .is_err()
        );
        call(
            &mut reg,
            ALICE,
            "MoveCompanyShares",
            &[i(corp), i(CAROL), i(40)],
        )
        .unwrap();
        assert_eq!(cs::held(reg.deltas(), corp, corp), 60);
        assert_eq!(cs::held(reg.deltas(), corp, CAROL), 40);
    }

    #[test]
    fn members_split_in_whole_cents() {
        let (mut reg, corp) = world();
        fund(&mut reg, corp, 10.0);
        let before: Vec<i64> = [ALICE, BOB, CAROL].iter().map(|c| cash(&reg, *c)).collect();
        call(
            &mut reg,
            ALICE,
            "PayoutDividend",
            &[i(0), PyValue::Real(10.0)],
        )
        .unwrap();
        for (c, was) in [ALICE, BOB, CAROL].iter().zip(before) {
            assert_eq!(
                cash(&reg, *c) - was,
                333,
                "a third of ten ISK, rounded down"
            );
        }
        assert_eq!(cash(&reg, DAVE), 0, "a stranger gets nothing");
        assert_eq!(wallet(&reg, corp), 1, "the odd cent stays in the wallet");
        let n = notif::of(reg.deltas(), BOB);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].type_id, 23);
        assert_eq!(n[0].sender, corp);
        let get = |k: &str| {
            n[0].data
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("corpID"), Some(Value::Int(corp)));
        assert_eq!(get("isMembers"), Some(Value::Int(1)));
        // The payout figure's value is an [A] (module docs): only that
        // the notification carries one.
        assert!(matches!(get("payout"), Some(Value::Real(_))));
        assert!(notif::of(reg.deltas(), DAVE).is_empty());
    }

    #[test]
    fn only_the_ceo_pays() {
        let (mut reg, corp) = world();
        fund(&mut reg, corp, 10.0);
        for who in [BOB, DAVE] {
            let err = call(&mut reg, who, "PayoutDividend", &[i(0), PyValue::Real(5.0)])
                .expect_err("not the CEO");
            assert_eq!(key_of(&err), "CrpAccessDenied");
        }
        assert_eq!(wallet(&reg, corp), cw::to_cents(10.0));
        assert_eq!(cash(&reg, BOB), 0);
    }

    #[test]
    fn a_short_wallet_changes_nothing() {
        let (mut reg, corp) = world();
        fund(&mut reg, corp, 5.0);
        let err = call(
            &mut reg,
            ALICE,
            "PayoutDividend",
            &[i(0), PyValue::Real(5.01)],
        )
        .expect_err("more than the wallet holds");
        assert_eq!(key_of(&err), "NotEnoughMoney");
        assert_eq!(wallet(&reg, corp), cw::to_cents(5.0));
        for c in [ALICE, BOB, CAROL] {
            assert_eq!(cash(&reg, c), 0);
        }
        // Nor does a zero, a negative or a not-a-number amount.
        for bad in [0.0, -3.0, f64::NAN] {
            assert!(
                call(
                    &mut reg,
                    ALICE,
                    "PayoutDividend",
                    &[i(0), PyValue::Real(bad)]
                )
                .is_err()
            );
        }
        assert_eq!(wallet(&reg, corp), cw::to_cents(5.0));
    }

    #[test]
    fn shareholders_are_paid_by_weight() {
        let (mut reg, corp) = world();
        // Bob a fifth, Dave a twentieth, the corp a twentieth; Alice keeps
        // the rest.
        let total = issued(&reg, corp);
        let (bob, dave, own) = (total / 5, total / 20, total / 20);
        for (to, n) in [(BOB, bob), (DAVE, dave), (corp, own)] {
            call(
                &mut reg,
                ALICE,
                "MovePrivateShares",
                &[i(corp), i(to), i(n)],
            )
            .unwrap();
        }
        let alice = total - bob - dave - own;
        fund(&mut reg, corp, 100.0);
        call(
            &mut reg,
            ALICE,
            "PayoutDividend",
            &[i(1), PyValue::Real(100.0)],
        )
        .unwrap();
        let cents = 10_000;
        assert_eq!(cash(&reg, ALICE), cut(cents, alice, total));
        assert_eq!(cash(&reg, BOB), cut(cents, bob, total));
        assert_eq!(cash(&reg, DAVE), cut(cents, dave, total));
        assert_eq!(cash(&reg, CAROL), 0, "a member without shares is not paid");
        let paid = cash(&reg, ALICE) + cash(&reg, BOB) + cash(&reg, DAVE);
        assert_eq!(
            wallet(&reg, corp),
            cents - paid,
            "the corp's own shares and the rounding stay"
        );
        let n = notif::of(reg.deltas(), DAVE);
        assert_eq!(n.len(), 1, "a shareholder outside the corp is told too");
        assert!(n[0].data.contains(&("isMembers".to_owned(), Value::Int(0))));
        // The corp never pays itself: no corp row of ref 38 is to the corp.
        let rows = cw::all_rows(reg.deltas(), corp);
        let dividends: Vec<_> = rows
            .iter()
            .filter(|r| r.ref_type == REF_CORPORATION_DIVIDEND_PAYMENT)
            .collect();
        assert_eq!(dividends.len(), 3, "one row per pilot paid");
        assert!(
            dividends
                .iter()
                .all(|r| r.owner1 == corp && r.owner2 != corp && r.amount < 0)
        );
    }

    #[test]
    fn a_dividend_row_is_in_the_pilots_journal() {
        let (mut reg, corp) = world();
        fund(&mut reg, corp, 10.0);
        call(
            &mut reg,
            ALICE,
            "PayoutDividend",
            &[i(0), PyValue::Real(10.0)],
        )
        .unwrap();
        for who in [ALICE, BOB, CAROL] {
            let rows: Vec<_> = reg
                .journal()
                .entries()
                .iter()
                .filter(|e| e.entry_type_id == REF_CORPORATION_DIVIDEND_PAYMENT)
                .filter(|e| e.wallet() == who)
                .collect();
            assert_eq!(rows.len(), 1, "one dividend row in {who}'s wallet");
            assert!(rows[0].amount > 0.0, "money in");
        }
        assert!(
            reg.journal()
                .entries()
                .iter()
                .filter(|e| e.entry_type_id == REF_CORPORATION_DIVIDEND_PAYMENT)
                .all(|e| e.wallet() != corp),
            "no dividend row lands in the corp's wallet"
        );
    }

    #[test]
    fn a_deleted_pilot_is_not_paid_and_cannot_be_given_shares() {
        let (mut reg, corp) = world();
        let total = issued(&reg, corp);
        call(
            &mut reg,
            ALICE,
            "MovePrivateShares",
            &[i(corp), i(DAVE), i(total / 4)],
        )
        .unwrap();
        tombstone(&mut reg, DAVE);
        // A tombstoned holder's cut stays in the corp wallet.
        fund(&mut reg, corp, 100.0);
        call(
            &mut reg,
            ALICE,
            "PayoutDividend",
            &[i(1), PyValue::Real(100.0)],
        )
        .unwrap();
        assert_eq!(cash(&reg, DAVE), 0);
        assert!(notif::of(reg.deltas(), DAVE).is_empty());
        assert!(wallet(&reg, corp) >= cut(10_000, total / 4, total));
        // And no move to it.
        let err = call(
            &mut reg,
            ALICE,
            "MovePrivateShares",
            &[i(corp), i(DAVE), i(1)],
        )
        .expect_err("a deleted owner");
        assert_eq!(key_of(&err), "CustomNotify");
        // A deleted member is not paid a member split either.
        tombstone(&mut reg, CAROL);
        let before = cash(&reg, CAROL);
        call(
            &mut reg,
            ALICE,
            "PayoutDividend",
            &[i(0), PyValue::Real(10.0)],
        )
        .unwrap();
        assert_eq!(cash(&reg, CAROL), before);
    }
}
