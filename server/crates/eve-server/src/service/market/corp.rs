//! CORP-BUY: a corporation's market orders (`PlaceBuyOrder`'s `useCorp`).
//! Lane note `docs/lanes/corp-buy.md`.
//!
//! The client shows "use corporation wallet" on the buy window
//! (`buyThisTypeWindow.MakeCorpCheckboxMaybe`, :319) and on the multibuy and
//! multisell windows (`buySellMultiBase.py:142`) only to a pilot whose
//! `session.corprole` holds the accountant or the trader bit, and sends the
//! box as `useCorp` (`marketsvc.BuyStuff`, :244-254; `SellMulti`, :263) **[V]**.
//! It names no division: the division is the pilot's active one,
//! `session.corpAccountKey` (`corp_wallet::login_key`) **[A]**, falsified by a
//! capture whose corp order names another.
//!
//! An [`Order`] carries `corp_id` and `key_id` (0 and 0 for a pilot's own).
//! Every ISK move of an order goes through [`pay`], which books the pilot's
//! wallet or the corporation's division and its journal; every arrival of
//! goods for a corp bid goes to the corporation's deliveries hangar
//! ([`goods_slot`], CORP-O's flag 62). `charID` stays the issuer, which is
//! what the client reads to offer Cancel and Modify (`orders.py:265`) **[V]**.

use super::*;
use crate::corp_wallet as cw;
use crate::service::corp_wallet as csw;

/// Whose ISK an order spends and earns: a pilot's own, or one division of a
/// corporation's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Wallet {
    pub corp: i64,
    pub key: i64,
}

impl Wallet {
    /// The pilot's own wallet.
    pub const PILOT: Wallet = Wallet { corp: 0, key: 0 };

    /// The wallet behind an existing order.
    pub fn of(o: &Order) -> Wallet {
        Wallet {
            corp: o.corp_id,
            key: o.key_id,
        }
    }

    pub fn is_corp(self) -> bool {
        self.corp != 0
    }

    /// The owner id a journal row names for this wallet's side of a trade.
    pub fn owner(self, character: i64) -> i64 {
        if self.is_corp() { self.corp } else { character }
    }
}

/// The roles the client offers the corp checkbox to: accountant or trader
/// (`buyThisTypeWindow.py:319`, `buySellMultiBase.py:142`) **[V]**.
pub(super) const TRADE_ROLES: i64 = cw::ROLE_ACCOUNTANT | cw::ROLE_TRADER;

fn denied(reason: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CrpAccessDenied",
        crate::pyutil::pypairs(vec![("reason", PyValue::Unicode(reason.to_owned()))]),
    ))
}

/// The player corporation `character` belongs to and the roles they hold
/// there (`rolesAtAll | rolesAtOther`, the client's `session.corprole`).
fn roles_of(reg: &Registry, character: i64) -> Option<(i64, i64)> {
    let m = reg.corporations.membership(character)?;
    reg.corporations
        .get(m.corporation_id)
        .filter(|c| !c.deleted)?;
    Some((m.corporation_id, m.roles | m.roles_at_other))
}

/// The wallet a call pays from: the pilot's, or, when `use_corp` is true, the
/// corporation's active division. A pilot without the accountant or the
/// trader role, or in no player corporation, is refused with
/// `CrpAccessDenied` **[A]** (the client never offers the box, so the key is
/// a guess; the role check is the client's own).
pub(super) fn wallet_for(
    reg: &Registry,
    actor: i64,
    use_corp: Option<&PyValue>,
) -> Result<Wallet, ServiceError> {
    let on = match use_corp {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        _ => false,
    };
    if !on {
        return Ok(Wallet::PILOT);
    }
    let Some((corp, roles)) = roles_of(reg, actor) else {
        return Err(denied(
            "Only a member of a player corporation can use its wallet.",
        ));
    };
    if roles & TRADE_ROLES == 0 {
        return Err(denied(
            "You need the accountant or the trader role to use the corporation wallet.",
        ));
    }
    Ok(Wallet {
        corp,
        key: csw::login_key(reg, actor),
    })
}

/// Refuse unless the wallet holds `needed + fee`: `NotEnoughMoney` for a
/// pilot (the market's own refusal), `NotEnoughMoneyCorp` for a division.
pub(super) fn ensure(
    reg: &Registry,
    actor: i64,
    wallet: Wallet,
    needed: f64,
    fee: f64,
) -> Result<(), ServiceError> {
    if wallet.is_corp() {
        return csw::check_fee(reg, wallet.corp, wallet.key, cents(needed + fee));
    }
    let balance = balance_of(reg, actor);
    if needed + fee > balance + 1e-9 {
        return Err(not_enough_cash(balance, needed, fee));
    }
    Ok(())
}

/// Move `delta` ISK (signed) on `wallet`. `entry` is the journal ref and the
/// other party's id, `at` the reference id and the time. A pilot's wallet is
/// `move_isk`; a division is `corp_wallet::transfer`, which writes the corp
/// journal row, and `sink` also books the ledger mirror a fee, a tax or a fill
/// against an NPC order needs.
pub(super) fn pay(
    reg: &mut Registry,
    character: i64,
    wallet: Wallet,
    delta: f64,
    (entry, other): (i64, i64),
    (reference, now): (i64, i64),
    sink: bool,
) {
    if !wallet.is_corp() {
        move_isk(reg, character, delta, entry, other, reference, now);
        return;
    }
    let delta = cents(delta);
    if delta == 0.0 {
        return;
    }
    let pair = if delta < 0.0 {
        (wallet.corp, other)
    } else {
        (other, wallet.corp)
    };
    let _ = csw::transfer(
        reg,
        wallet.corp,
        wallet.key,
        delta,
        entry,
        pair,
        reference,
        sink,
    );
    reg.dirty = true;
}

/// Where goods bought for `wallet` arrive: the pilot's hangar, or the
/// corporation's deliveries hangar at the station (CORP-O) **[V]**
/// (`invCtrl.StationCorpDeliveries`, flag 62, owner the corporation).
pub(super) fn goods_slot(wallet: Wallet, character: i64, station: i64) -> crate::item::Slot {
    if wallet.is_corp() {
        crate::item::Slot::new(
            wallet.corp,
            station,
            crate::corp_office::FLAG_CORP_DELIVERIES,
        )
    } else {
        crate::item::Slot::new(character, station, crate::model::FLAG_HANGAR)
    }
}

/// Goods have reached a corporation's deliveries hangar: tell the members who
/// may open it.
pub(super) fn arrived(reg: &mut Registry, wallet: Wallet, station: i64) {
    if wallet.is_corp() {
        crate::service::corp_office::tell_deliveries(reg, wallet.corp, station);
    }
}

/// Does `o`'s issuer still hold what a corporation order needs: current
/// membership of the order's corporation and the accountant or trader role?
/// A pilot's own order (`corp_id` 0) always passes.
pub(super) fn has_access(reg: &Registry, o: &Order) -> bool {
    o.corp_id == 0
        || roles_of(reg, o.char_id).is_some_and(|(c, r)| c == o.corp_id && r & TRADE_ROLES != 0)
}

/// Who may cancel or modify `o`: for a pilot's own order its issuer; for a
/// corporation's order a **current** member holding the accountant or trader
/// role, the issuer included (REVIEW-FIX: a kicked trader keeps no reach into
/// the wallet; the client offers the menu to the issuer only,
/// `orders.py:265`, so this is the hand-built call).
pub(super) fn may_manage(reg: &Registry, actor: i64, o: &Order) -> bool {
    if o.corp_id == 0 {
        return o.char_id == actor;
    }
    roles_of(reg, actor).is_some_and(|(c, r)| c == o.corp_id && r & TRADE_ROLES != 0)
}

/// Who is told of a change to `o`: its issuer, and for a corporation's order
/// every member who sees the Corporation Orders tab (the accountants).
fn audience(reg: &Registry, o: &Order) -> Vec<i64> {
    let mut to = vec![o.char_id];
    if o.corp_id != 0 {
        for m in reg.corporations.members_of(o.corp_id) {
            if m != o.char_id && roles_of(reg, m).is_some_and(|(_, r)| r & cw::ROLE_ACCOUNTANT != 0)
            {
                to.push(m);
            }
        }
    }
    to
}

/// `OnOwnOrdersChanged` for `o`'s audience.
pub(super) fn tell(reg: &mut Registry, o: &Order, rows: Vec<PyValue>, reason: &'static str) {
    for to in audience(reg, o) {
        reg.push(OwnOrdersChanged {
            to,
            orders: rows.clone(),
            reason,
        });
    }
}

/// `GetCorporationOrders`: the corporation's open orders, for a member with
/// the accountant role (the tab is built for no other,
/// `marketOrdersWnd.IsCorpOrdersAccessible`) **[V]**; nothing for anyone else.
/// The same rows as `GetCharOrders`, `keyID` the division and `charID` the
/// issuer, which `GetWalletDivision` and `GetIssuedBy` read (`quote.py:431`).
pub(super) fn corporation_orders(reg: &Registry, actor: i64) -> PyValue {
    match roles_of(reg, actor) {
        Some((corp, roles)) if roles & cw::ROLE_ACCOUNTANT != 0 => {
            own_orders_rowset(reg.market.open().filter(|o| o.corp_id == corp))
        }
        _ => PyValue::List(vec![]),
    }
}

/// What `character` is to their player corporation, for reading its wallet:
/// the same `Standing` the wallet service builds (`service/corp_wallet.rs`).
fn standing_of(reg: &Registry, character: i64) -> Option<(i64, cw::Standing)> {
    let m = reg.corporations.membership(character)?;
    let corp = reg
        .corporations
        .get(m.corporation_id)
        .filter(|c| !c.deleted)?;
    Some((
        corp.corporation_id,
        cw::Standing {
            ceo: corp.ceo_id == character,
            roles: m.roles | m.roles_at_other,
        },
    ))
}

/// `CorpGetTransactions(fromDate, accountKey)`: the trades the caller's
/// corporation made through division `key` (`None`: every division the
/// caller may read), a pilot who may not read the division getting nothing.
///
/// **A row is one side of a trade, seen from the corporation.** The client
/// computes `isBuyTransaction = accountID == buyerAccountID`
/// (`marketSvc._GetCorpTransactionKeyVal`) and `CharGetTransactions` sends
/// the cash key 1000 for all three, which is also the corporation's Master
/// Wallet: a row that did the same here would read a corporation's sale to a
/// pilot as a purchase. So `accountID` is the corporation's division, the
/// corporation's own side repeats it and the other side is 0 (a pilot's
/// wallet has no division, and a division is never below 1000), whatever
/// the counterparty paid through. A trade between two divisions of the same
/// corporation is two rows, one per side. `buyerID` and `sellerID` stay the
/// pilots who traded; `fromDate` `None` is "everything" **[A]**.
pub(super) fn transactions(
    reg: &Registry,
    actor: i64,
    key: Option<i64>,
    from: Option<i64>,
) -> PyValue {
    let mut rowset = eve_rowset::CRowset::new(transaction_descriptor(true));
    let Some((corp, standing)) = standing_of(reg, actor) else {
        return rowset.to_pyvalue();
    };
    let readable = |k: i64| standing.can_query(k) && key.is_none_or(|want| want == k);
    for t in reg
        .market
        .trades
        .iter()
        .filter(|t| from.is_none_or(|f| t.date >= f))
    {
        let sides = [
            (
                t.buyer_corp == corp && readable(t.buyer_key),
                t.buyer_key,
                true,
            ),
            (
                t.seller_corp == corp && readable(t.seller_key),
                t.seller_key,
                false,
            ),
        ];
        for (_, k, buy) in sides.into_iter().filter(|s| s.0) {
            let (client, buyer_account, seller_account) = if buy {
                (t.seller_id, k, 0)
            } else {
                (t.buyer_id, 0, k)
            };
            rowset
                .push_values(vec![
                    DbValue::Int(t.transaction_id),
                    DbValue::Int(t.date),
                    DbValue::Int(t.type_id),
                    DbValue::Int(t.quantity),
                    DbValue::Int((t.price * 10_000.0).round() as i64),
                    DbValue::Int(client),
                    DbValue::Int(t.station_id),
                    DbValue::Int(k),
                    DbValue::Int(t.buyer_id),
                    DbValue::Int(t.seller_id),
                    DbValue::Int(buyer_account),
                    DbValue::Int(seller_account),
                    DbValue::Int(k),
                ])
                .expect("a transaction row matches its descriptor");
        }
    }
    rowset.to_pyvalue()
}
