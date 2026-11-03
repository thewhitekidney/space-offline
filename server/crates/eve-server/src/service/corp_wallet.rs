//! Corporation wallets over the wire (CORP-W): the Corporation tab of the
//! Wallet window, give-to-corp and take-from-corp, the corp tax on payouts,
//! the division names and the corp-details edit. The state and the rules are
//! `crate::corp_wallet`; the client reading behind every arm is in
//! `docs/lanes/corp-w.md` §2.
//!
//! **Two doors in.** `account` (the Wallet window) is dispatched by
//! `service/wallet.rs`, which offers each call to [`account`] first; it
//! answers `None` for what is not a corporation's, and the personal arms run
//! as ever. `corpRegistry` is dispatched by `service/corp.rs`, which hands
//! [`CORP_REGISTRY_METHODS`] to [`corp_registry`]. The rest is `pub(crate)`
//! for the payout hooks (`npc::bounty`, `mission_rewards`) and the alliance
//! fee.
//!
//! **Money moves through [`post`] (a corp division) and [`char_post`] (a
//! character) and nowhere else**: each books the balance, the journal row
//! and the push together, because any one alone is a bug (the same three
//! halves `Registry::charge` documents). A corporation's System-flow rows
//! (tax in, a fee out) are also written to the character journal under the
//! corp's id, where no pilot's window can see them, so the economy ledger
//! (`econ_ledger`) counts the faucet and the sink.

use super::*;
use crate::corp_wallet::{
    self as cw, CorpAccountChange, MASTER_KEY, REF_CORPORATION_ACCOUNT_WITHDRAWAL,
    REF_CORPORATION_PAYMENT, Short, Standing,
};
use crate::ledger::Entry;

/// The `corpRegistry` methods this file answers. `corp::corp_registry`
/// hands these to [`corp_registry`].
pub(crate) const CORP_REGISTRY_METHODS: &[&str] =
    &["SetAccountKey", "UpdateDivisionNames", "UpdateCorporation"];

/// `appConst.refPlayerDonation` **[V]**: ISK one pilot gives another, or a
/// corporation.
const REF_PLAYER_DONATION: i64 = 10;

/// **[A]** The counterparty of a fee a corporation pays, when the pilot
/// paying is not docked: CONCORD, who Tranquility names as the payee of the
/// registration fees. The client's line reads only the corporation.
const FEE_PAYEE: i64 = 1_000_125;

/// `OnCorporationChanged` columns for the division names, hangar 1..7 then
/// wallet 1..7 (`bco_corporations.GetDivisionNames`) **[V]**.
const NAME_COLUMNS: [&str; cw::NAME_COUNT] = [
    "division1",
    "division2",
    "division3",
    "division4",
    "division5",
    "division6",
    "division7",
    "walletDivision1",
    "walletDivision2",
    "walletDivision3",
    "walletDivision4",
    "walletDivision5",
    "walletDivision6",
    "walletDivision7",
];

/// How a push leaves: with the answer of the call that made it, or on the
/// clock for a payout the world pays by itself (`npc::bounty::tick`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Via {
    Call,
    Clock,
}

fn tell(reg: &mut Registry, via: Via, n: impl crate::notify::Push + 'static) {
    match via {
        Via::Call => reg.push(n),
        Via::Clock => reg.schedule(Duration::ZERO, n),
    }
}

// ---------------------------------------------------------------------------
// Arguments and refusals
// ---------------------------------------------------------------------------

fn refuse(key: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, vec![]))
}

fn refuse_with(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

/// `CrpAccessDenied {reason}`, the client's own refusal for a role it lacks
/// **[V]**.
fn access_denied(reason: &str) -> ServiceError {
    refuse_with(
        "CrpAccessDenied",
        vec![("reason", PyValue::Unicode(reason.to_owned()))],
    )
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
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn real_arg(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(r) => Some(*r),
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

/// Positional `i`, or the keyword `name`. The transfer window passes
/// `toAccountKey=` and `reason=` by keyword (`transferMoneyWnd.Confirm`)
/// **[V]**. `Request::kwargs` is in wire order, `(value, key)`, like every
/// marshal dict here; the same-named helpers in `corp.rs` and `alliance.rs`
/// read the pair the other way round, which no positional-only caller has
/// noticed (see the lane note).
fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args
        .get(i)
        .or_else(|| crate::pyutil::pairs_get(req.kwargs, name.as_bytes()))
}

/// A flag the client sends as `True`/`1`.
fn truthy(v: Option<&PyValue>) -> bool {
    matches!(v, Some(PyValue::Bool(true)) | Some(PyValue::Int(1)))
}

// ---------------------------------------------------------------------------
// Who is asking
// ---------------------------------------------------------------------------

/// The player corporation `character` is in and what they are there. `None`
/// in an NPC corp or a closed one. `roles` is the client's `session.corprole`,
/// `rolesAtAll | rolesAtOther` **[V]**.
fn standing(reg: &Registry, character: i64) -> Option<(i64, Standing)> {
    let m = reg.corporations.membership(character)?;
    let corp = reg
        .corporations
        .get(m.corporation_id)
        .filter(|c| !c.deleted)?;
    Some((
        corp.corporation_id,
        Standing {
            ceo: corp.ceo_id == character,
            roles: m.roles | m.roles_at_other,
        },
    ))
}

fn my_standing(reg: &Registry, req: &Request<'_>) -> Result<(i64, i64, Standing), ServiceError> {
    let actor = req
        .actor
        .ok_or_else(|| access_denied("There is no character to act for."))?;
    let (corp, s) = standing(reg, actor)
        .ok_or_else(|| access_denied("Only a member of a player corporation can do that."))?;
    Ok((actor, corp, s))
}

/// Who hears of a change to division `key`: the members who may read it.
fn readers(reg: &Registry, corp: i64, key: i64) -> Vec<i64> {
    reg.corporations
        .members_of(corp)
        .into_iter()
        .filter(|id| standing(reg, *id).is_some_and(|(_, s)| s.can_query(key)))
        .collect()
}

pub(crate) fn corp_name(reg: &Registry, corp: i64) -> String {
    reg.corporations
        .get(corp)
        .map_or_else(String::new, |c| c.name.clone())
}

/// The division's name for the client: the corp's own, or the Master Wallet.
pub(crate) fn division_label(reg: &Registry, corp: i64, key: i64) -> String {
    if key == MASTER_KEY {
        return "Master Wallet".to_owned();
    }
    cw::wallet_name(reg.deltas(), corp, key)
        .unwrap_or_else(|| format!("Wallet Division {}", key - 999))
}

// ---------------------------------------------------------------------------
// Posting money
// ---------------------------------------------------------------------------

/// One movement on a corporation division.
struct Post<'a> {
    key: i64,
    /// Cents, positive in, negative out.
    cents: i64,
    ref_type: i64,
    /// Payer and payee, whichever way the money goes.
    owner1: i64,
    owner2: i64,
    reference: i64,
    description: &'a str,
    /// Also book a hidden row in the character journal under the corp's id,
    /// for the economy ledger: set for a faucet or a sink, not a transfer.
    mirror: bool,
}

/// Move money on a corp division: the balance, the corp journal row, the
/// push to the members who may read it, and (for a System flow) the
/// ledger's mirror row. A debit above the balance changes nothing.
fn post(reg: &mut Registry, via: Via, corp: i64, p: &Post<'_>) -> Result<i64, Short> {
    let now = crate::pyutil::filetime_now();
    let balance = cw::adjust(reg.deltas_mut(), corp, p.key, p.cents)?;
    cw::record(
        reg.deltas_mut(),
        corp,
        p.key,
        now,
        p.ref_type,
        p.owner1,
        p.owner2,
        p.reference,
        p.cents,
        balance,
        p.description,
    );
    if p.mirror {
        let total = cw::to_isk(cw::total(reg.deltas(), corp));
        reg.journal_mut().record(
            now,
            p.ref_type,
            p.owner1,
            p.owner2,
            p.reference,
            cw::to_isk(p.cents),
            total,
            "",
        );
    }
    let to = readers(reg, corp, p.key);
    if !to.is_empty() {
        tell(
            reg,
            via,
            CorpAccountChange {
                to,
                corporation_id: corp,
                key: p.key,
                balance: cw::to_isk(balance),
            },
        );
    }
    Ok(balance)
}

/// Move ISK in or out of a character's wallet: the balance, the journal row
/// (this character's, so `delta` is signed from their side), and
/// `OnAccountChange`. `owner1` is the payer and `owner2` the payee, so the
/// row belongs to the character whichever way it goes
/// (`Journal::select` reads the payee of a credit, the payer of a debit).
#[allow(clippy::too_many_arguments)]
pub(crate) fn char_post(
    reg: &mut Registry,
    via: Via,
    character: i64,
    delta: f64,
    ref_type: i64,
    owner1: i64,
    owner2: i64,
    reference: i64,
    description: &str,
) -> Option<f64> {
    let c = reg.character_mut(character)?;
    c.balance += delta;
    let balance = c.balance;
    reg.dirty = true;
    reg.journal_mut().record(
        crate::pyutil::filetime_now(),
        ref_type,
        owner1,
        owner2,
        reference,
        delta,
        balance,
        description,
    );
    tell(
        reg,
        via,
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

fn char_cents(reg: &Registry, character: i64) -> i64 {
    reg.character(character)
        .map_or(0, |c| cw::to_cents(c.balance))
}

/// `NotEnoughMoney {amount, balance}` for a pilot's own wallet **[V]**.
fn short_of_money(reg: &Registry, character: i64, cents: i64) -> ServiceError {
    refuse_with(
        "NotEnoughMoney",
        vec![
            ("amount", PyValue::Real(cw::to_isk(cents))),
            (
                "balance",
                PyValue::Real(reg.character(character).map_or(0.0, |c| c.balance)),
            ),
        ],
    )
}

/// `NotEnoughMoneyCorp {amount, balance, division, owner}` for a corporation
/// division **[V]** (`findings/user-errors.tsv`).
fn short_of_corp_money(reg: &Registry, corp: i64, key: i64, cents: i64) -> ServiceError {
    refuse_with(
        "NotEnoughMoneyCorp",
        vec![
            ("amount", PyValue::Real(cw::to_isk(cents))),
            (
                "balance",
                PyValue::Real(cw::to_isk(cw::balance(reg.deltas(), corp, key))),
            ),
            ("division", PyValue::Unicode(division_label(reg, corp, key))),
            ("owner", PyValue::Unicode(corp_name(reg, corp))),
        ],
    )
}

// ---------------------------------------------------------------------------
// Fees from the wallet (the alliance fee)
// ---------------------------------------------------------------------------

/// Refuse with `NotEnoughMoneyCorp` unless division `key` holds `isk`.
/// The caller has already checked who may spend it.
pub(crate) fn check_fee(reg: &Registry, corp: i64, key: i64, isk: f64) -> Result<(), ServiceError> {
    let cents = cw::to_cents(isk);
    if cw::balance(reg.deltas(), corp, key) < cents {
        return Err(short_of_corp_money(reg, corp, key, cents));
    }
    Ok(())
}

/// Charge a fee to a corporation division: a sink in the economy ledger,
/// journalled under `ref_type` with the reference (the alliance, say).
/// `payer_character` picks the payee as the station's owner, as a pilot's
/// own fee does. Answers the new balance in ISK, `None` when the division
/// holds too little (nothing moved).
pub(crate) fn charge_fee(
    reg: &mut Registry,
    corp: i64,
    key: i64,
    isk: f64,
    ref_type: i64,
    payer_character: i64,
    reference: i64,
) -> Option<f64> {
    let payee = reg
        .character(payer_character)
        .map_or(0, |c| c.station_owner_id);
    let payee = if payee == 0 { FEE_PAYEE } else { payee };
    post(
        reg,
        Via::Call,
        corp,
        &Post {
            key,
            cents: -cw::to_cents(isk),
            ref_type,
            owner1: corp,
            owner2: payee,
            reference,
            description: "",
            mirror: true,
        },
    )
    .ok()
    .map(cw::to_isk)
}

/// Move `isk` (positive in, negative out) on a corporation division as a
/// transfer between two owners (C8's war fees and contracts): the balance,
/// the corp journal row and the push, no ledger mirror unless `sink` (a fee
/// to CONCORD leaves the economy). Answers the new balance in ISK, `None`
/// when the division holds too little (nothing moved).
#[allow(clippy::too_many_arguments)]
pub(crate) fn transfer(
    reg: &mut Registry,
    corp: i64,
    key: i64,
    isk: f64,
    ref_type: i64,
    (owner1, owner2): (i64, i64),
    reference: i64,
    sink: bool,
) -> Option<f64> {
    post(
        reg,
        Via::Call,
        corp,
        &Post {
            key,
            cents: cw::to_cents(isk),
            ref_type,
            owner1,
            owner2,
            reference,
            description: "",
            mirror: sink,
        },
    )
    .ok()
    .map(cw::to_isk)
}

// ---------------------------------------------------------------------------
// The corp tax on payouts
// ---------------------------------------------------------------------------

/// What the corp took from a payout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Skim {
    pub corp: i64,
    pub tax: f64,
}

/// Take the corporation's cut of a payout of `gross` ISK to `character`
/// (journal ref `ref_type`: the bounty prizes, an agent reward or its time
/// bonus), into the Master Wallet. `None` when no tax applies: the pilot is
/// in an NPC corp, the rate is zero, the payout is under
/// [`cw::MIN_TAXED_PAYOUT`], or the ref is not taxed.
///
/// The caller credits `gross - tax` and stamps the row with [`stamped`], so
/// the client shows the whole payout and the tax as its own line.
pub(crate) fn skim(
    reg: &mut Registry,
    via: Via,
    character: i64,
    gross: f64,
    ref_type: i64,
    reference: i64,
) -> Option<Skim> {
    cw::tax_ref(ref_type)?;
    let (corp, _) = standing(reg, character)?;
    let rate = reg.corporations.get(corp)?.tax_rate;
    let tax = cw::tax_for(rate, gross);
    if tax <= 0.0 {
        return None;
    }
    post(
        reg,
        via,
        corp,
        &Post {
            key: MASTER_KEY,
            cents: cw::to_cents(tax),
            ref_type,
            owner1: character,
            owner2: corp,
            reference,
            description: "",
            mirror: true,
        },
    )
    .ok()?;
    Some(Skim { corp, tax })
}

/// A payout row's description: `base` (its own, a YAML mapping or empty)
/// with the tax stamped in when there was one.
pub(crate) fn stamped(base: &str, ref_type: i64, skim: Option<Skim>) -> String {
    match (skim, cw::tax_ref(ref_type)) {
        (Some(s), Some(tax_ref)) => cw::stamp(base, tax_ref, s.corp, s.tax),
        _ => base.to_owned(),
    }
}

/// Pay `character` an amount of ISK the corporation taxes (an agent's
/// reward): skim, credit the net, journal it stamped. The payer is
/// `counterparty` (the agent). Answers the new balance.
pub(crate) fn credit_taxed(
    reg: &mut Registry,
    character: i64,
    gross: f64,
    ref_type: i64,
    counterparty: i64,
    reference: i64,
) -> Option<f64> {
    reg.character(character)?;
    let skim = skim(reg, Via::Call, character, gross, ref_type, reference);
    let net = gross - skim.map_or(0.0, |s| s.tax);
    let description = stamped("", ref_type, skim);
    char_post(
        reg,
        Via::Call,
        character,
        net,
        ref_type,
        counterparty,
        character,
        reference,
        &description,
    )
}

/// `(M_h, M_bots)` the corporations hold, by whether the CEO is a bot, so
/// the economy ledger's money supply includes the wallets (`econ_ledger::money`).
pub(crate) fn corp_money(reg: &Registry) -> (f64, f64) {
    let mut out = (0.0, 0.0);
    for (corp, cents) in cw::totals(reg.deltas()) {
        let bot = reg
            .corporations
            .get(corp)
            .and_then(|c| reg.character(c.ceo_id))
            .is_some_and(|c| c.is_bot);
        if bot {
            out.1 += cw::to_isk(cents);
        } else {
            out.0 += cw::to_isk(cents);
        }
    }
    out
}

/// A corp's whole wallet, in ISK (the GM view, the bots).
pub(crate) fn corp_balance(reg: &Registry, corp: i64, key: i64) -> f64 {
    cw::to_isk(cw::balance(reg.deltas(), corp, key))
}

/// Append a `GetEntryTypes` row for every ref the economy ledger knows and
/// the list does not yet carry. A journal row whose type is missing is
/// dropped by the client, silently (`wallet-ledger.md` §5.3), and the call
/// latches, so the list must be whole on its first answer. The derived tax
/// rows (92..94) are among them: the client builds them from the payout's
/// description and drops one it cannot name.
pub(crate) fn complete_entry_types(rows: &mut Vec<PyValue>) {
    let have: Vec<i64> = rows
        .iter()
        .filter_map(|r| match r {
            PyValue::Instance { state, .. } => {
                int_arg(crate::pyutil::dict_get(state, b"entryTypeID"))
            }
            _ => None,
        })
        .collect();
    for (id, _, name) in crate::econ_ledger::REFS {
        if !have.contains(id) {
            rows.push(entry_type(*id, name));
        }
    }
}

// ---------------------------------------------------------------------------
// account.*: the Wallet window's corporation half
// ---------------------------------------------------------------------------

/// The `account` calls that are a corporation's, `None` for the rest.
pub(crate) fn account(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    Some(match req.method {
        "GetCashBalance" if truthy(req.args.first()) => get_cash_balance(reg, req),
        "GetTransactions" if truthy(req.args.get(3)) => get_transactions(reg, req),
        "GetWalletDivisionsInfo" => get_divisions_info(reg, req),
        "GiveCash" => give_cash(reg, req),
        "GiveCashFromCorpAccount" => give_cash_from_corp(reg, req),
        _ => return None,
    })
}

/// The key a call names, or the member's active division.
fn key_arg(reg: &Registry, req: &Request<'_>, i: usize, name: &str) -> i64 {
    int_arg(arg(req, i, name)).unwrap_or_else(|| {
        req.actor
            .map_or(MASTER_KEY, |a| cw::active_key(reg.deltas(), a))
    })
}

/// `GetCashBalance(1, accountKey=)`: a float, or a `UserError` the caller
/// catches (`walletSvc.GetCorpWealth` shows 0 for it) **[V]**.
fn get_cash_balance(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (_, corp, s) = my_standing(reg, req)?;
    let key = key_arg(reg, req, 1, "accountKey");
    if !s.can_query(key) {
        return Err(access_denied("You may not read that wallet division."));
    }
    Ok(PyValue::Real(corp_balance(reg, corp, key)))
}

/// `GetTransactions(accountKey, year, month, True)`: the corp journal as the
/// same `CRowset` the personal one is (`transactions_rowset`).
fn get_transactions(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (_, corp, s) = my_standing(reg, req)?;
    let key = int_arg(arg(req, 0, "accountKey")).unwrap_or(MASTER_KEY);
    if !s.can_query(key) {
        return Err(access_denied("You may not read that wallet division."));
    }
    let entries: Vec<Entry> = cw::rows(
        reg.deltas(),
        corp,
        key,
        crate::pyutil::filetime_now(),
        int_arg(arg(req, 1, "year")),
        int_arg(arg(req, 2, "month")),
    )
    .into_iter()
    .map(|r| Entry {
        transaction_id: r.seq,
        date: r.date,
        entry_type_id: r.ref_type,
        owner_id_1: r.owner1,
        owner_id_2: r.owner2,
        reference_id: r.reference,
        amount: cw::to_isk(r.amount),
        balance: cw::to_isk(r.balance),
        description: r.description,
    })
    .collect();
    let refs: Vec<&Entry> = entries.iter().collect();
    Ok(transactions_rowset(&refs))
}

/// `GetWalletDivisionsInfo()`: one `KeyVal(key, balance)` per division the
/// member may read; iterated with `enumerate` and read as `.key`/`.balance`
/// (`corpDivisionsPanel`) **[V]**.
fn get_divisions_info(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some((corp, s)) = req.actor.and_then(|a| standing(reg, a)) else {
        return Ok(PyValue::List(vec![]));
    };
    Ok(PyValue::List(
        (MASTER_KEY..MASTER_KEY + cw::DIVISIONS as i64)
            .filter(|k| s.can_query(*k))
            .map(|k| {
                keyval(vec![
                    ("key", PyValue::Int(k)),
                    ("balance", PyValue::Real(corp_balance(reg, corp, k))),
                ])
            })
            .collect(),
    ))
}

/// The amount in whole cents, refusing a figure that is not a positive
/// number of cents.
fn amount_cents(req: &Request<'_>) -> Result<i64, ServiceError> {
    let isk =
        real_arg(arg(req, 1, "amount")).ok_or_else(|| bad_args(req, "amount is not a number"))?;
    let cents = if isk.is_finite() {
        cw::to_cents(isk)
    } else {
        0
    };
    if cents <= 0 {
        return Err(access_denied("The amount must be at least one cent."));
    }
    Ok(cents)
}

/// `GiveCash(toID, amount, reason=None, toAccountKey=None)`: a pilot gives
/// ISK to another pilot, or to a corporation's division (the Give ISK tab,
/// and `transferMoneyWnd.Confirm` for a deposit) **[V]**. Both rows are
/// `refPlayerDonation` **[V]** (`wallet-ledger.md` §5.4). The Give ISK tab
/// truth-tests the answer.
fn give_cash(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let actor = req
        .actor
        .ok_or_else(|| access_denied("There is no character to act for."))?;
    let to = int_arg(arg(req, 0, "toID")).ok_or_else(|| bad_args(req, "toID is not an id"))?;
    let cents = amount_cents(req)?;
    let reason = text_arg(arg(req, 2, "reason")).unwrap_or_default();
    let description = cw::reason_description(&reason);
    if to == actor {
        return Err(access_denied("You cannot give ISK to yourself."));
    }
    if char_cents(reg, actor) < cents {
        return Err(short_of_money(reg, actor, cents));
    }
    if reg.character(to).is_some() {
        char_post(
            reg,
            Via::Call,
            actor,
            -cw::to_isk(cents),
            REF_PLAYER_DONATION,
            actor,
            to,
            0,
            &description,
        );
        char_post(
            reg,
            Via::Call,
            to,
            cw::to_isk(cents),
            REF_PLAYER_DONATION,
            actor,
            to,
            0,
            &description,
        );
        return Ok(PyValue::Bool(true));
    }
    if super::corp::is_player_corp(reg, to) {
        let key = int_arg(arg(req, 3, "toAccountKey")).unwrap_or(MASTER_KEY);
        if !cw::is_key(key) {
            return Err(access_denied("That is not a wallet division."));
        }
        char_post(
            reg,
            Via::Call,
            actor,
            -cw::to_isk(cents),
            REF_PLAYER_DONATION,
            actor,
            to,
            0,
            &description,
        );
        post(
            reg,
            Via::Call,
            to,
            &Post {
                key,
                cents,
                ref_type: REF_PLAYER_DONATION,
                owner1: actor,
                owner2: to,
                reference: 0,
                description: &description,
                mirror: false,
            },
        )
        .map_err(|_| access_denied("That is not a wallet division."))?;
        return Ok(PyValue::Bool(true));
    }
    Err(access_denied("That owner cannot receive ISK."))
}

/// `GiveCashFromCorpAccount(toID, amount, fromAccountKey, toAccountKey=None,
/// reason=None)`: a member with the division's take role (or a director)
/// moves ISK out of it: to a pilot (`refCorporationAccountWithdrawal`), to
/// another of the corp's divisions (the same ref on both rows), or to
/// another player corporation (`refCorporationPayment`) **[V]**. The
/// withdrawal line's reference is the member who took it
/// (`FmtWalletEntry`'s `corpWithdrawl` names `arg1`) **[V]**.
fn give_cash_from_corp(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (actor, corp, s) = my_standing(reg, req)?;
    let to = int_arg(arg(req, 0, "toID")).ok_or_else(|| bad_args(req, "toID is not an id"))?;
    let cents = amount_cents(req)?;
    let from_key =
        int_arg(arg(req, 2, "fromAccountKey")).ok_or_else(|| bad_args(req, "no fromAccountKey"))?;
    if !s.can_take(from_key) {
        return Err(access_denied(
            "You may not take ISK from that wallet division.",
        ));
    }
    let reason = text_arg(arg(req, 4, "reason")).unwrap_or_default();
    let description = cw::reason_description(&reason);
    let to_key = int_arg(arg(req, 3, "toAccountKey"));
    if cw::balance(reg.deltas(), corp, from_key) < cents {
        return Err(short_of_corp_money(reg, corp, from_key, cents));
    }
    let out = |ref_type: i64, payee: i64| Post {
        key: from_key,
        cents: -cents,
        ref_type,
        owner1: corp,
        owner2: payee,
        reference: actor,
        description: "",
        mirror: false,
    };
    if to == corp {
        let to_key = to_key.unwrap_or(MASTER_KEY);
        if !cw::is_key(to_key) || to_key == from_key {
            return Err(access_denied("Choose a different wallet division."));
        }
        let mut debit = out(REF_CORPORATION_ACCOUNT_WITHDRAWAL, corp);
        debit.description = &description;
        post(reg, Via::Call, corp, &debit)
            .map_err(|_| short_of_corp_money(reg, corp, from_key, cents))?;
        post(
            reg,
            Via::Call,
            corp,
            &Post {
                key: to_key,
                cents,
                ref_type: REF_CORPORATION_ACCOUNT_WITHDRAWAL,
                owner1: corp,
                owner2: corp,
                reference: actor,
                description: &description,
                mirror: false,
            },
        )
        .map_err(|_| access_denied("That is not a wallet division."))?;
        return Ok(PyValue::Bool(true));
    }
    if reg.character(to).is_some() {
        let mut debit = out(REF_CORPORATION_ACCOUNT_WITHDRAWAL, to);
        debit.description = &description;
        post(reg, Via::Call, corp, &debit)
            .map_err(|_| short_of_corp_money(reg, corp, from_key, cents))?;
        char_post(
            reg,
            Via::Call,
            to,
            cw::to_isk(cents),
            REF_CORPORATION_ACCOUNT_WITHDRAWAL,
            corp,
            to,
            actor,
            &description,
        );
        return Ok(PyValue::Bool(true));
    }
    if super::corp::is_player_corp(reg, to) {
        let to_key = to_key.unwrap_or(MASTER_KEY);
        if !cw::is_key(to_key) {
            return Err(access_denied("That is not a wallet division."));
        }
        let mut debit = out(REF_CORPORATION_PAYMENT, to);
        debit.description = &description;
        post(reg, Via::Call, corp, &debit)
            .map_err(|_| short_of_corp_money(reg, corp, from_key, cents))?;
        post(
            reg,
            Via::Call,
            to,
            &Post {
                key: to_key,
                cents,
                ref_type: REF_CORPORATION_PAYMENT,
                owner1: corp,
                owner2: to,
                reference: actor,
                description: &description,
                mirror: false,
            },
        )
        .map_err(|_| access_denied("That is not a wallet division."))?;
        return Ok(PyValue::Bool(true));
    }
    Err(access_denied("That owner cannot receive ISK."))
}

// ---------------------------------------------------------------------------
// corpRegistry: the active division, the division names, the details form
// ---------------------------------------------------------------------------

/// The `corpRegistry` arms in [`CORP_REGISTRY_METHODS`].
pub(crate) fn corp_registry(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    match req.method {
        "SetAccountKey" => set_account_key(reg, req),
        "UpdateDivisionNames" => update_division_names(reg, req),
        "UpdateCorporation" => update_corporation(reg, req),
        _ => Err(reg.no_method(req)),
    }
}

/// `SetAccountKey(accountKey)`, inside
/// `PerformSessionChange('corp.setaccountkey', ...)` **[V]**
/// (`base_corporation.SetAccountKey`): remember the member's active
/// division and change `corpAccountKey` in the session. The change is sent
/// even when the key is the same, because `PerformSessionChange` waits for
/// one **[A]**.
fn set_account_key(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let actor = req
        .actor
        .ok_or_else(|| access_denied("There is no character to act for."))?;
    let key = int_arg(arg(req, 0, "accountKey"))
        .ok_or_else(|| bad_args(req, "accountKey is not a key"))?;
    if !cw::is_key(key) {
        return Err(access_denied("That is not a wallet division."));
    }
    let old = cw::active_key(reg.deltas(), actor);
    cw::set_active_key(reg.deltas_mut(), actor, key);
    reg.queue_session_change(
        actor,
        crate::pyutil::pypairs(vec![(
            "corpAccountKey",
            PyValue::Tuple(vec![PyValue::Int(old), PyValue::Int(key)]),
        )]),
    );
    Ok(PyValue::None)
}

/// `UpdateDivisionNames(division1..7, walletDivision1..7)`: fourteen
/// positionals, the CEO only (`CorpAccessOnlyCEOEditDivisionNames`) **[V]**.
/// `ApplyDivisionNames` shows a `notify` refusal whose key contains
/// `CorpDiv` inline and re-raises any other **[V]**; the `div` argument is
/// the offending name.
fn update_division_names(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (_, corp, s) = my_standing(reg, req)?;
    if !s.can_name_divisions() {
        return Err(refuse("CorpAccessOnlyCEOEditDivisionNames"));
    }
    if req.args.len() < cw::NAME_COUNT {
        return Err(bad_args(req, "fourteen division names"));
    }
    let old = cw::names(reg.deltas(), corp);
    let mut new: [Option<String>; cw::NAME_COUNT] = Default::default();
    for (i, slot) in new.iter_mut().enumerate() {
        let raw = text_arg(req.args.get(i)).unwrap_or_default();
        let wallet = i >= 7;
        *slot = cw::check_name(&raw, wallet)
            .map_err(|key| refuse_with(key, vec![("div", PyValue::Unicode(raw.clone()))]))?;
    }
    // Wallet division 1 is the Master Wallet and has no name to set.
    new[7] = None;
    for (range, key) in [
        (0..7, "CorpDivNameInvalidTaken"),
        (8..14, "WalletCorpDivNameInvalidTaken"),
    ] {
        if cw::duplicate(&new[range]) {
            return Err(refuse_with(
                key,
                vec![("div", PyValue::Unicode(String::new()))],
            ));
        }
    }
    if new == old {
        return Ok(PyValue::None);
    }
    cw::set_names(reg.deltas_mut(), corp, &new);
    let change: Vec<(&'static str, PyValue, PyValue)> = (0..cw::NAME_COUNT)
        .filter(|i| old[*i] != new[*i])
        .map(|i| {
            let v = |n: &Option<String>| n.clone().map_or(PyValue::None, PyValue::Unicode);
            (NAME_COLUMNS[i], v(&old[i]), v(&new[i]))
        })
        .collect();
    let to = reg.corporations.members_of(corp);
    reg.push(super::corp::CorporationChanged {
        to,
        corporation_id: corp,
        change,
    });
    Ok(PyValue::None)
}

/// `UpdateCorporation(description, url, iskTaxRate, isRecruiting,
/// loyaltyPointTaxRate)` from the corp-details form (`EditCorpDetails.Submit`):
/// rates arrive as fractions (`GetValue() / 100.0`) **[V]**. The form is
/// offered to directors (`IsCorpDirector`) **[V]**, and the tax rate is what
/// [`skim`] reads. Every member's cached corporation row is told with
/// `OnCorporationChanged` (`bco_corporations.OnCorporationChanged`).
fn update_corporation(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (_, corp, s) = my_standing(reg, req)?;
    if !s.can_set_tax() {
        return Err(access_denied("Only a director can edit the corporation."));
    }
    let description = text_arg(arg(req, 0, "description"));
    let url = text_arg(arg(req, 1, "url"));
    let tax = real_arg(arg(req, 2, "iskTaxRate"));
    let recruiting = int_arg(arg(req, 3, "isRecruiting"));
    let lp_tax = real_arg(arg(req, 4, "loyaltyPointTaxRate"));
    let rate = |v: f64| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let Some(c) = reg.corporations.get_mut(corp) else {
        return Err(access_denied(
            "Only a member of a player corporation can do that.",
        ));
    };
    let mut change: Vec<(&'static str, PyValue, PyValue)> = Vec::new();
    if let Some(d) = description.filter(|d| *d != c.description) {
        change.push(("description", bstr(&c.description), bstr(&d)));
        c.description = d;
    }
    if let Some(u) = url.filter(|u| *u != c.url) {
        change.push(("url", bstr(&c.url), bstr(&u)));
        c.url = u;
    }
    if let Some(t) = tax.map(rate).filter(|t| *t != c.tax_rate) {
        change.push(("taxRate", PyValue::Real(c.tax_rate), PyValue::Real(t)));
        c.tax_rate = t;
    }
    if let Some(r) = recruiting.map(|r| r != 0).filter(|r| *r != c.recruiting) {
        change.push((
            "isRecruiting",
            PyValue::Int(i64::from(c.recruiting)),
            PyValue::Int(i64::from(r)),
        ));
        c.recruiting = r;
    }
    if let Some(t) = lp_tax.map(rate).filter(|t| *t != c.lp_tax_rate) {
        change.push((
            "loyaltyPointTaxRate",
            PyValue::Real(c.lp_tax_rate),
            PyValue::Real(t),
        ));
        c.lp_tax_rate = t;
    }
    if change.is_empty() {
        return Ok(PyValue::None);
    }
    reg.dirty = true;
    let to = reg.corporations.members_of(corp);
    reg.push(super::corp::CorporationChanged {
        to,
        corporation_id: corp,
        change,
    });
    Ok(PyValue::None)
}

/// The corporation row's fourteen name columns, in [`NAME_COLUMNS`] order:
/// the corp's own name or `None` (the client supplies its default label).
/// Wallet division 1 is always `None`.
pub(crate) fn name_values(reg: &Registry, corp: i64) -> [PyValue; cw::NAME_COUNT] {
    let names = cw::names(reg.deltas(), corp);
    let mut out: [PyValue; cw::NAME_COUNT] = std::array::from_fn(|_| PyValue::None);
    for (i, n) in names.into_iter().enumerate() {
        if i != 7
            && let Some(n) = n
        {
            out[i] = PyValue::Unicode(n);
        }
    }
    out
}

/// A character's session `corpAccountKey` at login.
pub(crate) fn login_key(reg: &Registry, character: i64) -> i64 {
    cw::active_key(reg.deltas(), character)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    const ALICE: i64 = 90_000_901;
    const BOB: i64 = 90_000_902;
    const CAROL: i64 = 90_000_903;

    /// The message key of a raised `UserError`, and its arguments, as text.
    fn key_of(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            return format!("{e:?}");
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("an exception header is (class, args)")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError's args are a tuple")
        };
        let key = match &args[0] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("the key is a str: {other:?}"),
        };
        format!("{key} {:?}", args.get(1))
    }

    fn refused(e: &ServiceError, key: &str) -> bool {
        key_of(e).contains(key)
    }

    /// Alice founds a corp (CEO, through the client's own call), Bob and
    /// Carol are members with no roles.
    fn world() -> (Registry, i64) {
        let characters = [(ALICE, "Alice"), (BOB, "Bob"), (CAROL, "Carol")]
            .into_iter()
            .map(|(id, name)| Character {
                character_id: id,
                name: name.into(),
                ship_id: id + 1,
                balance: 5_000_000.0,
                ..Character::default()
            })
            .collect();
        let mut reg = Registry::new(SandboxConfig {
            characters,
            ..SandboxConfig::default()
        });
        let corp = found(&mut reg, ALICE, "Wallet Test", "WLTT");
        for who in [BOB, CAROL] {
            reg.corporations.enrol_for_test(who, corp, 0);
        }
        let _ = reg.take_session_changes();
        let _ = reg.take_pending();
        (reg, corp)
    }

    /// `AddCorporation`'s fifteen positionals, as the create form sends them.
    fn found(reg: &mut Registry, actor: i64, name: &str, ticker: &str) -> i64 {
        let u = |s: &str| PyValue::Unicode(s.to_owned());
        let args = vec![
            u(name),
            u(ticker),
            u("We mine."),
            u("http://"),
            PyValue::Real(0.0),
            PyValue::Int(419),
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::Bool(true),
            PyValue::Bool(false),
            PyValue::Real(0.0),
        ];
        reg.dispatch(&Request {
            service: "corpRegistry",
            method: "AddCorporation",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(actor),
        })
        .expect("founded");
        reg.character(actor).expect("a pilot").corporation_id
    }

    fn call(
        reg: &mut Registry,
        actor: i64,
        service: &'static str,
        method: &'static str,
        args: Vec<PyValue>,
        kwargs: Vec<(PyValue, PyValue)>,
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service,
            method,
            args: &args,
            kwargs: &kwargs,
            actor: Some(actor),
            bind_params: None,
        })
    }

    /// A keyword argument as `Request::kwargs` holds it: wire order,
    /// `(value, key)`.
    fn kw(name: &str, value: PyValue) -> (PyValue, PyValue) {
        (value, crate::pyutil::bstr(name))
    }

    fn set_roles(reg: &mut Registry, who: i64, roles: i64) {
        reg.corporations.member_mut(who).expect("a member").roles = roles;
    }

    fn give(
        reg: &mut Registry,
        actor: i64,
        to: i64,
        isk: f64,
        key: Option<i64>,
    ) -> Result<PyValue, ServiceError> {
        let kw = key
            .map(|k| vec![kw("toAccountKey", PyValue::Int(k))])
            .unwrap_or_default();
        call(
            reg,
            actor,
            "account",
            "GiveCash",
            vec![
                PyValue::Int(to),
                PyValue::Real(isk),
                PyValue::Unicode("ammo".into()),
            ],
            kw,
        )
    }

    /// SEC-FIX 3: `GiveCash` with a reason of `len` characters.
    fn give_reasoned(reg: &mut Registry, actor: i64, to: i64, len: usize) {
        call(
            reg,
            actor,
            "account",
            "GiveCash",
            vec![
                PyValue::Int(to),
                PyValue::Real(10.0),
                PyValue::Unicode("r".repeat(len)),
            ],
            vec![],
        )
        .expect("gives");
    }

    #[test]
    fn a_long_reason_is_cut_to_the_clients_limit_in_both_journals() {
        let (mut reg, corp) = world();
        give_reasoned(&mut reg, BOB, corp, 5_000_000);
        let want = cw::reason_description(&"r".repeat(cw::REASON_MAX_LEN));
        let mine = reg
            .journal()
            .select(crate::pyutil::filetime_now(), None, None, BOB);
        assert_eq!(mine.last().expect("Bob's row").description, want);
        let rows = cw::all_rows(reg.deltas(), corp);
        assert!(
            rows.iter()
                .all(|r| r.description.chars().count() <= want.chars().count()),
            "the corp journal row is capped too"
        );
    }

    fn take(
        reg: &mut Registry,
        actor: i64,
        to: i64,
        isk: f64,
        from: i64,
    ) -> Result<PyValue, ServiceError> {
        call(
            reg,
            actor,
            "account",
            "GiveCashFromCorpAccount",
            vec![PyValue::Int(to), PyValue::Real(isk), PyValue::Int(from)],
            vec![],
        )
    }

    fn corp_isk(reg: &Registry, corp: i64, key: i64) -> f64 {
        corp_balance(reg, corp, key)
    }

    fn wallet_of(reg: &Registry, id: i64) -> f64 {
        reg.character(id).unwrap().balance
    }

    #[test]
    fn a_member_gives_isk_to_the_corp_and_both_journals_show_it() {
        let (mut reg, corp) = world();
        assert!(matches!(
            give(&mut reg, BOB, corp, 1_000.0, None),
            Ok(PyValue::Bool(true))
        ));
        assert_eq!(wallet_of(&reg, BOB), 5_000_000.0 - 1_000.0);
        assert_eq!(corp_isk(&reg, corp, 1000), 1_000.0);
        let mine: Vec<_> = reg
            .journal()
            .select(crate::pyutil::filetime_now(), None, None, BOB);
        let row = mine.last().expect("Bob's own row");
        assert_eq!(
            (row.entry_type_id, row.amount),
            (REF_PLAYER_DONATION, -1_000.0)
        );
        assert_eq!((row.owner_id_1, row.owner_id_2), (BOB, corp));
        assert_eq!(row.description, "{DESC: \"ammo\"}");
        let rows = cw::all_rows(reg.deltas(), corp);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (
                rows[0].key,
                rows[0].ref_type,
                rows[0].amount,
                rows[0].balance
            ),
            (1000, REF_PLAYER_DONATION, 100_000, 100_000)
        );
        // To a chosen division.
        give(&mut reg, BOB, corp, 250.5, Some(1003)).unwrap();
        assert_eq!(corp_isk(&reg, corp, 1003), 250.5);
        assert_eq!(corp_isk(&reg, corp, 1000), 1_000.0);
    }

    #[test]
    fn giving_more_than_you_hold_or_to_nobody_changes_nothing() {
        let (mut reg, corp) = world();
        let e = give(&mut reg, BOB, corp, 9_000_000.0, None).unwrap_err();
        assert!(refused(&e, "NotEnoughMoney"), "{e:?}");
        assert!(
            give(&mut reg, BOB, BOB, 1.0, None).is_err(),
            "not to yourself"
        );
        assert!(
            give(&mut reg, BOB, 12_345, 1.0, None).is_err(),
            "nobody there"
        );
        assert!(give(&mut reg, BOB, corp, 0.0, None).is_err());
        assert!(give(&mut reg, BOB, corp, -5.0, None).is_err());
        assert!(
            give(&mut reg, BOB, corp, 5.0, Some(1007)).is_err(),
            "not a division"
        );
        assert_eq!(wallet_of(&reg, BOB), 5_000_000.0);
        assert_eq!(corp_isk(&reg, corp, 1000), 0.0);
        assert!(cw::all_rows(reg.deltas(), corp).is_empty());
    }

    #[test]
    fn a_pilot_gives_isk_to_a_pilot_with_a_donation_row_each() {
        let (mut reg, _) = world();
        give(&mut reg, BOB, CAROL, 100.0, None).unwrap();
        assert_eq!(wallet_of(&reg, BOB), 5_000_000.0 - 100.0);
        assert_eq!(wallet_of(&reg, CAROL), 5_000_000.0 + 100.0);
        let now = crate::pyutil::filetime_now();
        let bob = reg.journal().select(now, None, None, BOB);
        let carol = reg.journal().select(now, None, None, CAROL);
        assert_eq!((bob.len(), carol.len()), (1, 1));
        assert_eq!(bob[0].amount, -100.0);
        assert_eq!(carol[0].amount, 100.0);
        assert_eq!(carol[0].balance, 5_000_100.0);
    }

    #[test]
    fn the_ceo_withdraws_to_a_member_and_a_plain_member_cannot() {
        let (mut reg, corp) = world();
        give(&mut reg, BOB, corp, 2_000.0, None).unwrap();
        let e = take(&mut reg, BOB, BOB, 100.0, 1000).unwrap_err();
        assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
        assert_eq!(corp_isk(&reg, corp, 1000), 2_000.0);
        take(&mut reg, ALICE, CAROL, 750.0, 1000).unwrap();
        assert_eq!(corp_isk(&reg, corp, 1000), 1_250.0);
        assert_eq!(wallet_of(&reg, CAROL), 5_000_750.0);
        let rows = cw::all_rows(reg.deltas(), corp);
        let w = rows.last().unwrap();
        assert_eq!(
            (
                w.ref_type,
                w.owner1,
                w.owner2,
                w.reference,
                w.amount,
                w.balance
            ),
            (
                REF_CORPORATION_ACCOUNT_WITHDRAWAL,
                corp,
                CAROL,
                ALICE,
                -75_000,
                125_000
            )
        );
        let carol = reg
            .journal()
            .select(crate::pyutil::filetime_now(), None, None, CAROL);
        assert_eq!(
            carol.last().unwrap().entry_type_id,
            REF_CORPORATION_ACCOUNT_WITHDRAWAL
        );
        assert_eq!(carol.last().unwrap().amount, 750.0);
    }

    #[test]
    fn a_take_role_is_one_division_and_an_overdraw_names_the_division() {
        let (mut reg, corp) = world();
        give(&mut reg, ALICE, corp, 100.0, Some(1002)).unwrap();
        set_roles(&mut reg, BOB, cw::ROLE_ACCOUNT_CAN_TAKE[2]);
        let e = take(&mut reg, BOB, BOB, 10.0, 1000).unwrap_err();
        assert!(refused(&e, "CrpAccessDenied"));
        let e = take(&mut reg, BOB, BOB, 500.0, 1002).unwrap_err();
        assert!(refused(&e, "NotEnoughMoneyCorp"), "{e:?}");
        assert!(key_of(&e).contains("Wallet Division 3"), "{e:?}");
        take(&mut reg, BOB, BOB, 40.0, 1002).unwrap();
        assert_eq!(corp_isk(&reg, corp, 1002), 60.0);
    }

    #[test]
    fn divisions_move_money_inside_the_corp_and_between_corps() {
        let (mut reg, corp) = world();
        give(&mut reg, ALICE, corp, 300.0, None).unwrap();
        take(&mut reg, ALICE, corp, 100.0, 1000).unwrap_err(); // same division
        call(
            &mut reg,
            ALICE,
            "account",
            "GiveCashFromCorpAccount",
            vec![PyValue::Int(corp), PyValue::Real(100.0), PyValue::Int(1000)],
            vec![kw("toAccountKey", PyValue::Int(1004))],
        )
        .unwrap();
        assert_eq!(
            (corp_isk(&reg, corp, 1000), corp_isk(&reg, corp, 1004)),
            (200.0, 100.0)
        );
        assert_eq!(
            cw::total(reg.deltas(), corp),
            30_000,
            "moving money keeps the total"
        );
        let other = found(&mut reg, BOB, "Second Test", "SECT");
        take(&mut reg, ALICE, other, 50.0, 1000).unwrap();
        assert_eq!(corp_isk(&reg, other, 1000), 50.0);
        assert_eq!(corp_isk(&reg, corp, 1000), 150.0);
        let rows = cw::all_rows(reg.deltas(), other);
        assert_eq!(rows[0].ref_type, REF_CORPORATION_PAYMENT);
    }

    #[test]
    fn the_wallet_window_reads_by_role() {
        let (mut reg, corp) = world();
        give(&mut reg, BOB, corp, 123.45, Some(1001)).unwrap();
        let ask = |reg: &mut Registry, who: i64, key: i64| {
            call(
                reg,
                who,
                "account",
                "GetCashBalance",
                vec![PyValue::Int(1)],
                vec![kw("accountKey", PyValue::Int(key))],
            )
        };
        assert_eq!(ask(&mut reg, ALICE, 1001).unwrap(), PyValue::Real(123.45));
        assert!(refused(
            &ask(&mut reg, BOB, 1001).unwrap_err(),
            "CrpAccessDenied"
        ));
        // A junior accountant reads every division and takes from none.
        set_roles(&mut reg, CAROL, cw::ROLE_JUNIOR_ACCOUNTANT);
        assert_eq!(ask(&mut reg, CAROL, 1001).unwrap(), PyValue::Real(123.45));
        assert!(take(&mut reg, CAROL, CAROL, 1.0, 1001).is_err());
        // The divisions list follows the same rule.
        let info = |reg: &mut Registry, who: i64| match call(
            reg,
            who,
            "account",
            "GetWalletDivisionsInfo",
            vec![],
            vec![],
        )
        .unwrap()
        {
            PyValue::List(v) => v.len(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            (
                info(&mut reg, ALICE),
                info(&mut reg, CAROL),
                info(&mut reg, BOB)
            ),
            (7, 7, 0)
        );
        // The journal: a CRowset for those who may read, a refusal for the rest.
        let journal = |reg: &mut Registry, who: i64| {
            call(
                reg,
                who,
                "account",
                "GetTransactions",
                vec![
                    PyValue::Int(1001),
                    PyValue::None,
                    PyValue::None,
                    PyValue::Bool(true),
                ],
                vec![],
            )
        };
        assert!(journal(&mut reg, ALICE).is_ok());
        assert!(journal(&mut reg, CAROL).is_ok());
        assert!(journal(&mut reg, BOB).is_err());
        // The personal arms are not ours.
        let req = Request {
            service: "account",
            method: "GetCashBalance",
            args: &[PyValue::Int(0)],
            kwargs: &[],
            actor: Some(ALICE),
            bind_params: None,
        };
        assert!(account(&mut reg, &req).is_none());
    }

    #[test]
    fn a_pilot_outside_a_player_corp_has_no_corporation_wallet() {
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![Character {
                character_id: ALICE,
                balance: 10.0,
                ..Character::default()
            }],
            ..SandboxConfig::default()
        });
        let e = call(
            &mut reg,
            ALICE,
            "account",
            "GetCashBalance",
            vec![PyValue::Int(1)],
            vec![],
        )
        .unwrap_err();
        assert!(refused(&e, "CrpAccessDenied"));
        let info = call(
            &mut reg,
            ALICE,
            "account",
            "GetWalletDivisionsInfo",
            vec![],
            vec![],
        )
        .unwrap();
        assert_eq!(info, PyValue::List(vec![]));
    }

    #[test]
    fn the_tax_skims_a_payout_into_the_master_wallet_and_stamps_the_row() {
        let (mut reg, corp) = world();
        reg.corporations.get_mut(corp).unwrap().tax_rate = 0.10;
        let before = wallet_of(&reg, BOB);
        let paid = credit_taxed(&mut reg, BOB, 1_000_000.0, 33, 3_999_201, 77).unwrap();
        assert_eq!(paid, before + 900_000.0);
        assert_eq!(corp_isk(&reg, corp, 1000), 100_000.0);
        let row = reg
            .journal()
            .select(crate::pyutil::filetime_now(), None, None, BOB)
            .into_iter()
            .last()
            .unwrap()
            .clone();
        assert_eq!((row.entry_type_id, row.amount), (33, 900_000.0));
        assert_eq!((row.owner_id_1, row.owner_id_2), (3_999_201, BOB));
        assert_eq!(row.description, format!("{{93: [{corp}, 100000.00]}}"));
        let c = cw::all_rows(reg.deltas(), corp);
        assert_eq!(
            (c[0].ref_type, c[0].owner1, c[0].owner2, c[0].amount),
            (33, BOB, corp, 10_000_000)
        );
        // The ledger sees the tax as a faucet under the corp's id.
        let hidden = reg
            .journal()
            .entries()
            .iter()
            .filter(|e| e.wallet() == corp)
            .count();
        assert_eq!(hidden, 1);
    }

    #[test]
    fn a_small_payout_a_zero_rate_or_an_npc_corp_pays_no_tax() {
        let (mut reg, corp) = world();
        assert!(
            skim(&mut reg, Via::Call, BOB, 1_000_000.0, 85, 1).is_none(),
            "rate 0"
        );
        reg.corporations.get_mut(corp).unwrap().tax_rate = 0.5;
        assert!(skim(&mut reg, Via::Call, BOB, cw::MIN_TAXED_PAYOUT - 1.0, 85, 1).is_none());
        assert!(
            skim(&mut reg, Via::Call, BOB, 1_000_000.0, 10, 1).is_none(),
            "not a taxed ref"
        );
        let mut npc = Registry::new(SandboxConfig {
            characters: vec![Character {
                character_id: ALICE,
                ..Character::default()
            }],
            ..SandboxConfig::default()
        });
        assert!(skim(&mut npc, Via::Call, ALICE, 1_000_000.0, 85, 1).is_none());
        let s = skim(&mut reg, Via::Call, BOB, 1_000_000.0, 85, 5).unwrap();
        assert_eq!((s.corp, s.tax), (corp, 500_000.0));
        assert_eq!(
            stamped("{NBL: {1: 2}}", 85, Some(s)),
            format!("{{92: [{corp}, 500000.00], NBL: {{1: 2}}}}")
        );
        assert_eq!(stamped("{NBL: {1: 2}}", 85, None), "{NBL: {1: 2}}");
    }

    #[test]
    fn the_alliance_fee_is_a_sink_from_the_master_wallet() {
        let (mut reg, corp) = world();
        let e = check_fee(&reg, corp, 1000, 1_000.0).unwrap_err();
        assert!(refused(&e, "NotEnoughMoneyCorp"));
        give(&mut reg, ALICE, corp, 5_000.0, None).unwrap();
        check_fee(&reg, corp, 1000, 1_000.0).unwrap();
        let left = charge_fee(&mut reg, corp, 1000, 1_000.0, 48, ALICE, 99_000_001).unwrap();
        assert_eq!(left, 4_000.0);
        assert!(charge_fee(&mut reg, corp, 1000, 9_999.0, 48, ALICE, 1).is_none());
        assert_eq!(corp_isk(&reg, corp, 1000), 4_000.0);
        let sink = crate::econ_ledger::summarize(
            reg.journal()
                .entries()
                .iter()
                .filter(|e| e.entry_type_id == 48),
        );
        assert_eq!(sink.sink, 1_000.0);
    }

    #[test]
    fn wallets_count_in_the_money_supply_by_whose_corp_it_is() {
        let (mut reg, corp) = world();
        give(&mut reg, BOB, corp, 1_000.0, None).unwrap();
        assert_eq!(corp_money(&reg), (1_000.0, 0.0));
        reg.character_mut(ALICE).unwrap().is_bot = true;
        assert_eq!(corp_money(&reg), (0.0, 1_000.0));
    }

    #[test]
    fn only_the_ceo_names_divisions_and_the_row_follows() {
        let (mut reg, corp) = world();
        let mut names: Vec<PyValue> = (0..14).map(|_| PyValue::Unicode(String::new())).collect();
        names[0] = PyValue::Unicode("Ore".into());
        names[9] = PyValue::Unicode("Ammo fund".into());
        let e = call(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateDivisionNames",
            names.clone(),
            vec![],
        )
        .unwrap_err();
        assert!(refused(&e, "CorpAccessOnlyCEOEditDivisionNames"));
        call(
            &mut reg,
            ALICE,
            "corpRegistry",
            "UpdateDivisionNames",
            names.clone(),
            vec![],
        )
        .unwrap();
        let v = name_values(&reg, corp);
        assert_eq!(v[0], PyValue::Unicode("Ore".into()));
        assert_eq!(v[9], PyValue::Unicode("Ammo fund".into()));
        assert_eq!(v[7], PyValue::None, "the Master Wallet has no name");
        assert_eq!(division_label(&reg, corp, 1002), "Ammo fund");
        assert_eq!(division_label(&reg, corp, 1000), "Master Wallet");
        // A bad name is the client's own key, with the name as `div`.
        names[1] = PyValue::Unicode("x".repeat(51));
        let e = call(
            &mut reg,
            ALICE,
            "corpRegistry",
            "UpdateDivisionNames",
            names.clone(),
            vec![],
        )
        .unwrap_err();
        assert!(refused(&e, "CorpDivNameInvalidMaxLength"));
        names[1] = PyValue::Unicode(String::new());
        names[10] = PyValue::Unicode("ammo FUND".into());
        let e = call(
            &mut reg,
            ALICE,
            "corpRegistry",
            "UpdateDivisionNames",
            names,
            vec![],
        )
        .unwrap_err();
        assert!(refused(&e, "WalletCorpDivNameInvalidTaken"));
        assert_eq!(
            division_label(&reg, corp, 1002),
            "Ammo fund",
            "a refusal changes nothing"
        );
    }

    #[test]
    fn the_active_division_is_remembered_and_changes_the_session() {
        let (mut reg, _) = world();
        assert_eq!(login_key(&reg, BOB), 1000);
        call(
            &mut reg,
            BOB,
            "corpRegistry",
            "SetAccountKey",
            vec![PyValue::Int(1004)],
            vec![],
        )
        .unwrap();
        assert_eq!(login_key(&reg, BOB), 1004);
        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, BOB);
        assert!(
            call(
                &mut reg,
                BOB,
                "corpRegistry",
                "SetAccountKey",
                vec![PyValue::Int(5)],
                vec![]
            )
            .is_err()
        );
        assert_eq!(login_key(&reg, BOB), 1004);
    }

    #[test]
    fn a_director_sets_the_tax_and_a_member_cannot() {
        let (mut reg, corp) = world();
        let form = |tax: f64| {
            vec![
                PyValue::Unicode("about us".into()),
                PyValue::Unicode("http://example.invalid".into()),
                PyValue::Real(tax),
                PyValue::Int(1),
                PyValue::Real(0.0),
            ]
        };
        let e = call(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateCorporation",
            form(0.2),
            vec![],
        )
        .unwrap_err();
        assert!(refused(&e, "CrpAccessDenied"));
        assert_eq!(reg.corporations.get(corp).unwrap().tax_rate, 0.0);
        call(
            &mut reg,
            ALICE,
            "corpRegistry",
            "UpdateCorporation",
            form(0.2),
            vec![],
        )
        .unwrap();
        let c = reg.corporations.get(corp).unwrap();
        assert_eq!((c.tax_rate, c.description.as_str()), (0.2, "about us"));
        set_roles(&mut reg, BOB, cw::ROLE_DIRECTOR);
        call(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateCorporation",
            form(7.0),
            vec![],
        )
        .unwrap();
        assert_eq!(
            reg.corporations.get(corp).unwrap().tax_rate,
            1.0,
            "clamped to all of it"
        );
    }

    /// **Persistence is the deltas**: balances, journal rows (a description
    /// with tabs and newlines among them), division names and the active
    /// division all come back from the world file.
    #[test]
    fn wallets_survive_the_world_file() {
        let (mut reg, corp) = world();
        give(&mut reg, BOB, corp, 1_234.56, Some(1002)).unwrap();
        take(&mut reg, ALICE, CAROL, 34.56, 1002).unwrap();
        call(
            &mut reg,
            BOB,
            "corpRegistry",
            "SetAccountKey",
            vec![PyValue::Int(1005)],
            vec![],
        )
        .unwrap();
        let mut names: Vec<PyValue> = (0..14).map(|_| PyValue::Unicode(String::new())).collect();
        names[9] = PyValue::Unicode("Ammo fund".into());
        call(
            &mut reg,
            ALICE,
            "corpRegistry",
            "UpdateDivisionNames",
            names,
            vec![],
        )
        .unwrap();
        let rows_before = cw::all_rows(reg.deltas(), corp);
        let dir = crate::testkit::scratch_dir("corp-wallet-world");
        let store = crate::store::FileStore::new(dir.path().join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
        let back = crate::store::Store::load(&store)
            .expect("read the world")
            .expect("a world was written");
        let mut fresh = Registry::new(SandboxConfig::default());
        fresh.apply_snapshot(back);
        assert_eq!(cw::balance(fresh.deltas(), corp, 1002), 120_000);
        assert_eq!(cw::all_rows(fresh.deltas(), corp), rows_before);
        assert_eq!(rows_before.len(), 2);
        assert_eq!(login_key(&fresh, BOB), 1005);
        assert_eq!(division_label(&fresh, corp, 1002), "Ammo fund");
        for kind in cw::KINDS {
            assert!(crate::delta_kinds::is_registered(kind), "{kind}");
        }
    }

    #[test]
    fn the_entry_type_list_is_completed_from_the_ledger_refs() {
        let mut rows = vec![entry_type(10, "Player donation")];
        complete_entry_types(&mut rows);
        let ids: Vec<i64> = rows
            .iter()
            .filter_map(|r| match r {
                PyValue::Instance { state, .. } => {
                    int_arg(crate::pyutil::dict_get(state, b"entryTypeID"))
                }
                _ => None,
            })
            .collect();
        for needed in [10, 11, 17, 33, 34, 37, 48, 85, 92, 93, 94] {
            assert_eq!(
                ids.iter().filter(|i| **i == needed).count(),
                1,
                "ref {needed}"
            );
        }
    }
}
