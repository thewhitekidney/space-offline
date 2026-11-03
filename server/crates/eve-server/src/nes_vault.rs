//! **The PLEX vault** (L1 NES-BACKEND, `handback/r-nes-econ/report.md` §2.1):
//! one balance per **user** (account), a transaction log, a ledger of every
//! movement, and the two invbroker calls that move PLEX between an item stack
//! and the vault.
//!
//! The client reads the balance from the **gateway** (`plex.vault.api
//! .BalanceRequest`, cached 600 s, refreshed by `BalanceChangedNotice`) and,
//! for bots and the machoNet path, `vaultManager.get_account_balance` **[V]**
//! (`plex/client/repository.py`, `vgs/client/vgsClient.py`). Both read the one
//! number kept here.
//!
//! ## Storage: delta kind [`KIND`] (`nes`)
//!
//! - `bal user -> cents`: the balance. A user with no row holds nothing.
//! - `log user seq -> ts delta_cents resulting_cents key`: the transaction
//!   log, the newest [`LOG_KEEP`] per user (`seq` zero-padded so a key range is
//!   in order); `logn user -> n` is the next `seq`.
//! - `led key -> in out`: the **ledger**, whole PLEX credited and debited per
//!   source key since the world began. Keys: `nes:<offer key>` (a purchase, a
//!   sink), `deposit` and `withdraw` (moves between an item and the vault),
//!   `gm` (a dev grant, a faucet), `redeem_trash` (units of a trashed token,
//!   not PLEX). Later lanes add their own (`crate:...`, `site:...`) and the
//!   PLEX-ECON lane folds the table into `econ_ledger`.
//! - `ref -> n`: the next invbroker reference.
//!
//! Every function takes the [`Deltas`], not the registry, so the gateway
//! (which has only the rows) and the services (which have the world) share it.
//!
//! ## What is [A]
//!
//! - The vault holds **cents** (the gateway says `total_in_cents`) but the
//!   game only moves whole PLEX; `vaultManager.get_account_balance` answers
//!   whole units. Falsifier: a vault window showing 100 times the balance.
//! - A deposit's or withdrawal's event is pushed **0.4 s after** the answer
//!   ([`EVENT_DELAY`]): the client builds its listener after the call returns,
//!   and a push in the same flush as the answer could be dispatched first.
//!   Falsifier: `PlexDepositTimeout` with the PLEX in the vault.
//! - The vault is not an item, so ship loss cannot touch it (owner decision,
//!   issue #35).

use crate::delta::Deltas;
use crate::gateway::{Ground, Reply, Target, get_bytes, put_bytes, put_varint};
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;
use std::time::Duration;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "nes";

/// `inventorycommon/const.py::typePlex` **[V]**; also `service.rs`'s constant.
pub const TYPE_PLEX: i64 = 44_992;
/// Cents in one PLEX **[V]** (`total_in_cents`).
pub const CENTS: i64 = 100;
/// `flagCargo` **[V]**: the only ship flag a withdrawal may name.
const FLAG_CARGO: i64 = 5;
/// Log rows kept per user **[A]**; falsifier: a player asking for older ones.
pub const LOG_KEEP: usize = 200;
/// How long after the answer a deposit or withdrawal event is pushed.
pub const EVENT_DELAY: Duration = Duration::from_millis(400);

const BAL: &str = "bal";
const LOG: &str = "log";
const LOGN: &str = "logn";
const LED: &str = "led";
const REF: &str = "ref";

/// Why a vault movement was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultError {
    /// A debit larger than the balance.
    Short { have: i64, want: i64 },
    /// A zero or negative amount.
    BadAmount,
}

/// One log row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub seq: i64,
    pub ts: u64,
    pub delta_cents: i64,
    pub resulting_cents: i64,
}

fn num(v: &[String], i: usize) -> Option<i64> {
    v.get(i)?.parse().ok()
}

/// The vault key of a pilot: the account that owns the character; a pilot
/// nobody owns (the one-user sandbox) belongs to the sandbox's account, which
/// is also the session's user there.
pub fn key_of(reg: &Registry, character: i64) -> i64 {
    reg.character(character)
        .and_then(|c| c.owner_user_id)
        .unwrap_or(reg.config().user_id)
}

/// The vault key as the gateway sees it: [`key_of`] on a server ground; a
/// bare test ground has no characters, so the negated pilot id.
pub fn key_of_ground(g: &Ground, character: i64) -> i64 {
    match g {
        Ground::Server(w) => key_of(
            &crate::service::world_read_timed(w, "nes_vault_key"),
            character,
        ),
        Ground::Local(_) => -character,
    }
}

/// Every character of `user`'s account.
pub fn pilots_of(reg: &Registry, user: i64) -> Vec<i64> {
    reg.config()
        .characters
        .iter()
        .filter(|c| c.owner_user_id.unwrap_or(reg.config().user_id) == user)
        .map(|c| c.character_id)
        .collect()
}

// ---- balance and log ---------------------------------------------------------

/// The balance in cents.
pub fn balance_cents(d: &Deltas, user: i64) -> i64 {
    d.of(KIND)
        .get(&vec![BAL.to_owned(), user.to_string()])
        .and_then(|v| num(v, 0))
        .unwrap_or(0)
}

/// The balance in whole PLEX.
pub fn balance(d: &Deltas, user: i64) -> i64 {
    balance_cents(d, user) / CENTS
}

fn seq_key(seq: i64) -> String {
    format!("{seq:012}")
}

/// Move `plex` whole PLEX (positive in, negative out) on `user`'s vault,
/// logging it and ledgering it under `key`. Nothing changes on `Err`.
pub fn apply(
    d: &mut Deltas,
    user: i64,
    plex: i64,
    key: &str,
    now: u64,
) -> Result<Entry, VaultError> {
    if plex == 0 {
        return Err(VaultError::BadAmount);
    }
    let have = balance_cents(d, user);
    let delta = plex.saturating_mul(CENTS);
    let after = have.checked_add(delta).ok_or(VaultError::BadAmount)?;
    if after < 0 {
        return Err(VaultError::Short { have, want: -delta });
    }
    let u = user.to_string();
    d.put(
        KIND,
        vec![BAL.to_owned(), u.clone()],
        vec![after.to_string()],
    );
    let nkey = vec![LOGN.to_owned(), u.clone()];
    let seq = d
        .of(KIND)
        .get(&nkey)
        .and_then(|v| num(v, 0))
        .unwrap_or(1)
        .max(1);
    d.put(KIND, nkey, vec![(seq + 1).to_string()]);
    d.put(
        KIND,
        vec![LOG.to_owned(), u.clone(), seq_key(seq)],
        vec![
            now.to_string(),
            delta.to_string(),
            after.to_string(),
            key.to_owned(),
        ],
    );
    if seq > LOG_KEEP as i64 {
        d.del(KIND, &[LOG.to_owned(), u, seq_key(seq - LOG_KEEP as i64)]);
    }
    let (i, o) = ledger_of(d, key);
    let (i, o) = if plex > 0 {
        (i + plex, o)
    } else {
        (i, o - plex)
    };
    d.put(
        KIND,
        vec![LED.to_owned(), key.to_owned()],
        vec![i.to_string(), o.to_string()],
    );
    Ok(Entry {
        seq,
        ts: now,
        delta_cents: delta,
        resulting_cents: after,
    })
}

/// Put `plex` (> 0) into the vault.
pub fn credit(
    d: &mut Deltas,
    user: i64,
    plex: i64,
    key: &str,
    now: u64,
) -> Result<Entry, VaultError> {
    if plex <= 0 {
        return Err(VaultError::BadAmount);
    }
    apply(d, user, plex, key, now)
}

/// Take `plex` (> 0) out of the vault.
pub fn debit(
    d: &mut Deltas,
    user: i64,
    plex: i64,
    key: &str,
    now: u64,
) -> Result<Entry, VaultError> {
    if plex <= 0 {
        return Err(VaultError::BadAmount);
    }
    apply(d, user, -plex, key, now)
}

/// The log of `user`, oldest first.
pub fn log(d: &Deltas, user: i64) -> Vec<Entry> {
    let u = user.to_string();
    let from = vec![LOG.to_owned(), u.clone(), String::new()];
    d.of(KIND)
        .range(from..)
        .take_while(|(k, _)| k.len() == 3 && k[0] == LOG && k[1] == u)
        .filter_map(|(k, v)| {
            Some(Entry {
                seq: k[2].parse().ok()?,
                ts: v.first()?.parse().ok()?,
                delta_cents: num(v, 1)?,
                resulting_cents: num(v, 2)?,
            })
        })
        .collect()
}

/// `(in, out)` whole PLEX under one ledger key.
pub fn ledger_of(d: &Deltas, key: &str) -> (i64, i64) {
    d.of(KIND)
        .get(&vec![LED.to_owned(), key.to_owned()])
        .map_or((0, 0), |v| (num(v, 0).unwrap_or(0), num(v, 1).unwrap_or(0)))
}

/// Every ledger row, `(key, in, out)`, by key.
pub fn ledger(d: &Deltas) -> Vec<(String, i64, i64)> {
    d.of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[0] == LED)
        .map(|(k, v)| (k[1].clone(), num(v, 0).unwrap_or(0), num(v, 1).unwrap_or(0)))
        .collect()
}

/// Count `units` whole PLEX made by a source against a ledger key (a drop, the
/// seed): the `in` column, the faucet side. The PLEX item is created by the
/// caller; this is only its record, so every unit has a named origin.
pub fn ledger_in(d: &mut Deltas, key: &str, units: i64) {
    let (i, o) = ledger_of(d, key);
    d.put(
        KIND,
        vec![LED.to_owned(), key.to_owned()],
        vec![(i + units).to_string(), o.to_string()],
    );
}

/// Count `units` of something that is not PLEX against a ledger key (a
/// trashed redeem token). Whole units in the `out` column.
pub fn ledger_out(d: &mut Deltas, key: &str, units: i64) {
    let (i, o) = ledger_of(d, key);
    d.put(
        KIND,
        vec![LED.to_owned(), key.to_owned()],
        vec![i.to_string(), (o + units).to_string()],
    );
}

/// The next invbroker reference (rising, never reused).
pub fn next_reference(d: &mut Deltas) -> i64 {
    let k = vec![REF.to_owned()];
    let n = d
        .of(KIND)
        .get(&k)
        .and_then(|v| num(v, 0))
        .unwrap_or(1)
        .max(1);
    d.put(KIND, k, vec![(n + 1).to_string()]);
    n
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---- the gateway -------------------------------------------------------------

const NOTICE: &str = "eve_public.plex.vault.api.BalanceChangedNotice";

fn put_field_varint(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

/// `plex.Currency { sint64 total_in_cents = 1 }` (type 18 is zigzag **[V]**,
/// `out/gw-protos.txt`).
fn currency(cents: i64) -> Vec<u8> {
    let mut out = Vec::new();
    put_field_varint(&mut out, 1, ((cents << 1) ^ (cents >> 63)) as u64);
    out
}

/// `transaction.Attributes { Timestamp timestamp = 1; Currency
/// amount_transferred = 2; Currency resulting_balance = 3 }`.
fn attributes(e: &Entry) -> Vec<u8> {
    let mut ts = Vec::new();
    put_field_varint(&mut ts, 1, e.ts);
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &ts);
    put_bytes(&mut out, 2, &currency(e.delta_cents));
    put_bytes(&mut out, 3, &currency(e.resulting_cents));
    out
}

fn identifier(seq: i64) -> Vec<u8> {
    let mut out = Vec::new();
    put_field_varint(&mut out, 1, seq as u64);
    out
}

/// The `BalanceChangedNotice` payload for a movement.
pub fn notice_payload(e: &Entry) -> Vec<u8> {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &identifier(e.seq));
    put_bytes(&mut out, 2, &attributes(e));
    out
}

/// Queue the balance-changed notice to every character of `user`'s account
/// (a notice to a pilot with no open stream is dropped, and the client
/// re-reads the balance when it opens a window).
pub fn announce(reg: &mut Registry, user: i64, e: &Entry) {
    let payload = notice_payload(e);
    for c in pilots_of(reg, user) {
        crate::gateway_out::queue(reg, NOTICE, payload.clone(), Target::Character(c));
    }
}

fn reply(status: u32, payload: Option<(String, Vec<u8>)>) -> Reply {
    Reply { status, payload }
}

/// The gateway arm (`gateway_ext::CHAIN`): the balance, and the transaction
/// log of the pilot's account. `None` for everything else.
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    const PKG: &str = "eve_public.plex.vault.";
    let rest = ty.strip_prefix(PKG)?;
    let resp = |value: Vec<u8>| {
        let name = format!("{}Response", ty.strip_suffix("Request").unwrap_or(ty));
        reply(200, Some((name, value)))
    };
    match rest {
        "api.BalanceRequest" => {
            let user = key_of_ground(g, character);
            let cents = g.read(|d| balance_cents(d, user));
            let mut out = Vec::new();
            put_bytes(&mut out, 1, &currency(cents));
            Some(resp(out))
        }
        "transaction.api.GetAllLoggedForUserRequest" => {
            let user = key_of_ground(g, character);
            let mut out = Vec::new();
            for e in g.read(|d| log(d, user)).iter().rev() {
                put_bytes(&mut out, 1, &identifier(e.seq));
            }
            Some(resp(out))
        }
        "transaction.api.GetLogRequest" => {
            let user = key_of_ground(g, character);
            let seq = get_bytes(body, 1).and_then(|i| crate::gateway::get_uint(&i, 1));
            let found = seq.and_then(|s| {
                g.read(|d| log(d, user))
                    .into_iter()
                    .find(|e| e.seq as u64 == s)
            });
            let mut out = Vec::new();
            match found {
                Some(e) => put_bytes(&mut out, 1, &attributes(&e)),
                // `unavailable` (field 2): an entry that aged out of the log.
                None => put_field_varint(&mut out, 2, 1),
            }
            Some(resp(out))
        }
        "transaction.api.GetStatisticsRequest" => {
            let user = key_of_ground(g, character);
            let rows = g.read(|d| log(d, user));
            let (inc, exp) = rows.iter().fold((0i64, 0i64), |(i, x), e| {
                if e.delta_cents > 0 {
                    (i + e.delta_cents, x)
                } else {
                    (i, x - e.delta_cents)
                }
            });
            // One entry, unclassified (`no_classification`), over the kept log.
            let mut entry = Vec::new();
            put_field_varint(&mut entry, 2, 1);
            put_bytes(&mut entry, 3, &currency(inc));
            put_bytes(&mut entry, 4, &currency(exp));
            put_field_varint(&mut entry, 5, rows.len() as u64);
            let mut out = Vec::new();
            if !rows.is_empty() {
                put_bytes(&mut out, 1, &entry);
                let mut ts = Vec::new();
                put_field_varint(&mut ts, 1, rows[0].ts);
                put_bytes(&mut out, 2, &ts);
            }
            Some(resp(out))
        }
        _ => None,
    }
}

// ---- machoNet events ---------------------------------------------------------

/// `OnPlexDeposited(reference)` and the other three events of F5.
#[derive(Debug, Clone)]
pub struct Event {
    to: i64,
    name: &'static str,
    args: Vec<PyValue>,
}

impl crate::notify::Push for Event {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.name, self.args.clone())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

fn event(reg: &mut Registry, to: i64, name: &'static str, args: Vec<PyValue>) {
    reg.push_later(EVENT_DELAY, Event { to, name, args });
}

fn int_at(req: &Request<'_>, i: usize) -> Option<i64> {
    match req.args.get(i)? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

fn bad_args(what: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![(
            "notify",
            crate::pyutil::bstr(&format!("bad arguments, expected {what}")),
        )]),
    ))
}

/// `invbroker.DepositPlexToVault(locationID, itemID, quantity, invoiceType)`
/// **[V]**: destroys `quantity` of the caller's PLEX stack, credits the
/// vault, answers a reference and pushes `OnPlexDeposited(reference)` (or
/// `OnPlexDepositFailed(key, reference)`) a moment later.
pub fn deposit(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (Some(_location), Some(item_id), Some(qty)) =
        (int_at(req, 0), int_at(req, 1), int_at(req, 2))
    else {
        return Err(bad_args("(locationID, itemID, quantity, invoiceType)"));
    };
    let Some(actor) = req.actor else {
        return Err(bad_args("a logged-in character"));
    };
    let reference = next_reference(reg.deltas_mut());
    let user = key_of(reg, actor);
    let fail = |reg: &mut Registry, key: &str| {
        event(
            reg,
            actor,
            "OnPlexDepositFailed",
            vec![crate::pyutil::bstr(key), PyValue::Int(reference)],
        );
        Ok(PyValue::Int(reference))
    };
    let stack = reg
        .store()
        .get(item_id)
        .filter(|i| i.owner_id == actor && i.type_id == TYPE_PLEX)
        .map(|i| (i.quantity, i.flag_id));
    let Some((have, _)) = stack else {
        return fail(reg, "PlexDepositInvalidAmount");
    };
    if qty < 1 || qty > have {
        return fail(reg, "PlexDepositInvalidAmount");
    }
    // Refuse before the stack is touched: an amount the vault cannot hold
    // must not cost the pilot the item.
    let fits = qty
        .checked_mul(CENTS)
        .and_then(|c| balance_cents(reg.deltas(), user).checked_add(c));
    if fits.is_none() {
        return fail(reg, "PlexDepositInvalidAmount");
    }
    let edit = if qty == have {
        crate::item::Edit::Destroy { item_id }
    } else {
        crate::item::Edit::Consume {
            item_id,
            quantity: qty,
        }
    };
    if reg.apply_items(&[edit]).is_err() {
        return fail(reg, "PlexDepositInternalError");
    }
    let entry = credit(reg.deltas_mut(), user, qty, "deposit", now_secs());
    match entry {
        Ok(e) => {
            announce(reg, user, &e);
            event(reg, actor, "OnPlexDeposited", vec![PyValue::Int(reference)]);
        }
        Err(_) => return fail(reg, "PlexDepositInternalError"),
    }
    Ok(PyValue::Int(reference))
}

/// `invbroker.WithdrawPlexFromVault(quantity, destinationItemID, invoiceType,
/// locationFlag, otherPartyID)` **[V]**: debits the vault and creates a PLEX
/// stack at `(destinationItemID, locationFlag)` for the caller, then pushes
/// `OnPlexItemAddedToInventory(itemID, reference)` (or
/// `OnPlexWithdrawFailed(key, reference)`).
///
/// The destination must be the station the pilot is docked in or an item the
/// pilot owns (their ship or a container); otherwise the call fails with
/// `PlexWithdrawalInternalError`. A ship's hold is checked for room.
pub fn withdraw(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (Some(qty), Some(dest)) = (int_at(req, 0), int_at(req, 1)) else {
        return Err(bad_args(
            "(quantity, destinationItemID, invoiceType, locationFlag, otherPartyID)",
        ));
    };
    let flag = int_at(req, 3)
        .filter(|f| *f > 0)
        .unwrap_or(crate::model::FLAG_HANGAR);
    let Some(actor) = req.actor else {
        return Err(bad_args("a logged-in character"));
    };
    let reference = next_reference(reg.deltas_mut());
    let user = key_of(reg, actor);
    let fail = |reg: &mut Registry, key: &str| {
        event(
            reg,
            actor,
            "OnPlexWithdrawFailed",
            vec![crate::pyutil::bstr(key), PyValue::Int(reference)],
        );
        Ok(PyValue::Int(reference))
    };
    if qty < 1 {
        return fail(reg, "PlexWithdrawalInvalidAmount");
    }
    if balance(reg.deltas(), user) < qty {
        return fail(reg, "PlexWithdrawalInsufficientFunds");
    }
    // Where it may go: the station hangar the pilot is docked in (the roster
    // decides, `Character::station_id` is never cleared on undock), or the
    // cargo of the ship they sit in. Nothing else, and only those flags.
    let docked_here = reg.docked_station(actor) == Some(dest) && flag == crate::model::FLAG_HANGAR;
    let in_ship = reg.character(actor).is_some_and(|c| c.ship_id == dest)
        && flag == FLAG_CARGO
        && reg.store().get(dest).is_some_and(|i| i.owner_id == actor);
    if !(docked_here || in_ship) {
        return fail(reg, "PlexWithdrawalInternalError");
    }
    // A hold has a capacity; a station hangar has none.
    if in_ship {
        let cap = reg
            .store()
            .get(dest)
            .and_then(|i| crate::item::find_in(reg.content(), i.type_id))
            .map_or(0.0, |t| t.capacity);
        let unit = crate::item::find_in(reg.content(), TYPE_PLEX).map_or(0.0, |t| t.volume);
        let used = reg.store().used_volume_in(reg.content(), dest, flag);
        if cap > 0.0 && used + unit * qty as f64 > cap {
            return fail(reg, "PlexWithdrawalInternalError");
        }
    }
    let at = crate::item::Slot::new(actor, dest, flag);
    let made = reg.apply_items(&[crate::item::Edit::Create {
        type_id: TYPE_PLEX,
        quantity: qty,
        to: at,
    }]);
    let Ok(changes) = made else {
        return fail(reg, "PlexWithdrawalInternalError");
    };
    let item_id = changes
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .unwrap_or_default();
    match debit(reg.deltas_mut(), user, qty, "withdraw", now_secs()) {
        Ok(e) => {
            announce(reg, user, &e);
            event(
                reg,
                actor,
                "OnPlexItemAddedToInventory",
                vec![PyValue::Int(item_id), PyValue::Int(reference)],
            );
            Ok(PyValue::Int(reference))
        }
        Err(_) => {
            // Cannot happen after the balance check above; undo the item.
            let _ = reg.apply_items(&[crate::item::Edit::Destroy { item_id }]);
            fail(reg, "PlexWithdrawalInternalError")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const U: i64 = 7_001;

    #[test]
    fn a_faucet_unit_is_named_in_the_ledger() {
        let mut d = Deltas::default();
        ledger_in(&mut d, "seed", 500);
        ledger_in(&mut d, "seed", 5);
        ledger_out(&mut d, "seed", 2);
        assert_eq!(ledger_of(&d, "seed"), (505, 2));
        assert_eq!(ledger_of(&d, "other"), (0, 0));
    }

    #[test]
    fn a_credit_and_a_debit_move_the_balance_and_the_log() {
        let mut d = Deltas::default();
        assert_eq!(balance(&d, U), 0);
        let e = credit(&mut d, U, 30, "gm", 1_000).unwrap();
        assert_eq!((e.seq, e.delta_cents, e.resulting_cents), (1, 3_000, 3_000));
        let e = debit(&mut d, U, 12, "nes:thing", 1_001).unwrap();
        assert_eq!(
            (e.seq, e.delta_cents, e.resulting_cents),
            (2, -1_200, 1_800)
        );
        assert_eq!(balance(&d, U), 18);
        assert_eq!(log(&d, U).len(), 2);
        // Another user's vault is untouched.
        assert_eq!(balance(&d, U + 1), 0);
        assert!(log(&d, U + 1).is_empty());
    }

    #[test]
    fn a_short_debit_changes_nothing() {
        let mut d = Deltas::default();
        credit(&mut d, U, 5, "gm", 1).unwrap();
        let before = d.clone();
        assert_eq!(
            debit(&mut d, U, 6, "nes:x", 2),
            Err(VaultError::Short {
                have: 500,
                want: 600
            })
        );
        assert_eq!(d, before, "a refused debit leaves no row, log or ledger");
        assert_eq!(debit(&mut d, U, 0, "nes:x", 2), Err(VaultError::BadAmount));
        assert_eq!(credit(&mut d, U, -3, "gm", 2), Err(VaultError::BadAmount));
    }

    #[test]
    fn every_movement_is_ledgered_under_its_key() {
        let mut d = Deltas::default();
        credit(&mut d, U, 100, "gm", 1).unwrap();
        debit(&mut d, U, 40, "nes:a", 2).unwrap();
        debit(&mut d, U, 10, "nes:a", 3).unwrap();
        credit(&mut d, U, 5, "deposit", 4).unwrap();
        debit(&mut d, U, 5, "withdraw", 5).unwrap();
        assert_eq!(ledger_of(&d, "nes:a"), (0, 50));
        assert_eq!(ledger_of(&d, "gm"), (100, 0));
        let rows = ledger(&d);
        let (i, o) = rows.iter().fold((0, 0), |(i, o), r| (i + r.1, o + r.2));
        // Everything that went in minus everything that came out is the balance.
        assert_eq!(i - o, balance(&d, U), "the ledger and the vault agree");
        ledger_out(&mut d, "redeem_trash", 3);
        assert_eq!(ledger_of(&d, "redeem_trash"), (0, 3));
    }

    #[test]
    fn the_log_keeps_the_newest_rows_in_order() {
        let mut d = Deltas::default();
        for i in 0..(LOG_KEEP as i64 + 5) {
            credit(&mut d, U, 1, "gm", i as u64).unwrap();
        }
        let l = log(&d, U);
        assert_eq!(l.len(), LOG_KEEP);
        assert_eq!(l.first().unwrap().seq, 6);
        assert_eq!(l.last().unwrap().seq, LOG_KEEP as i64 + 5);
        assert!(l.windows(2).all(|w| w[0].seq < w[1].seq));
    }

    #[test]
    fn references_rise_and_survive_other_rows() {
        let mut d = Deltas::default();
        let a = next_reference(&mut d);
        credit(&mut d, U, 1, "gm", 1).unwrap();
        let b = next_reference(&mut d);
        assert!(b > a);
    }

    #[test]
    fn the_vault_round_trips_through_the_world_file() {
        let mut d = Deltas::default();
        credit(&mut d, U, 9, "gm", 11).unwrap();
        debit(&mut d, U, 4, "nes:k", 12).unwrap();
        let again: Vec<_> = d
            .kinds()
            .map(|(k, rows)| (k.to_owned(), rows.clone()))
            .collect();
        let mut back = Deltas::default();
        for (k, rows) in again {
            for (key, val) in rows {
                back.put(&k, key, val);
            }
        }
        assert_eq!(balance(&back, U), 5);
        assert_eq!(log(&back, U), log(&d, U));
        assert_eq!(ledger(&back), ledger(&d));
    }

    // ---- deposit and withdraw ---------------------------------------------

    const PILOT: i64 = 2_001;
    const STATION: i64 = 600_009;

    fn world() -> Registry {
        crate::testkit::registry_with(|cfg| {
            cfg.characters = vec![crate::model::Character {
                character_id: PILOT,
                owner_user_id: Some(21),
                station_id: Some(STATION),
                ..Default::default()
            }];
        })
    }

    fn req<'a>(method: &'a str, args: &'a [PyValue]) -> Request<'a> {
        Request {
            service: "invbroker",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(PILOT),
        }
    }

    /// A PLEX stack of `n` in the pilot's hangar; its item id.
    fn stack(reg: &mut Registry, n: i64) -> i64 {
        let at = crate::item::Slot::new(PILOT, STATION, crate::model::FLAG_HANGAR);
        let ch = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: TYPE_PLEX,
                quantity: n,
                to: at,
            }])
            .unwrap();
        match ch[0] {
            crate::item::Change::Created { item_id, .. } => item_id,
            _ => panic!("created"),
        }
    }

    /// PLEX the pilot holds as items anywhere.
    fn held(reg: &Registry) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.owner_id == PILOT && i.type_id == TYPE_PLEX)
            .map(|i| i.quantity)
            .sum()
    }

    fn events(reg: &mut Registry) -> Vec<(&'static str, Vec<PyValue>)> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .collect()
    }

    #[test]
    fn a_deposit_destroys_units_credits_the_vault_and_pushes_the_same_reference() {
        let mut reg = world();
        let item = stack(&mut reg, 50);
        let args = [
            PyValue::Int(STATION),
            PyValue::Int(item),
            PyValue::Int(20),
            PyValue::Int(0),
        ];
        let PyValue::Int(reference) = deposit(&mut reg, &req("DepositPlexToVault", &args)).unwrap()
        else {
            panic!()
        };
        assert_eq!(held(&reg), 30);
        assert_eq!(balance(reg.deltas(), 21), 20);
        let ev = events(&mut reg);
        assert!(
            ev.iter()
                .any(|(n, a)| *n == "OnPlexDeposited" && a == &[PyValue::Int(reference)]),
            "{ev:?}"
        );
        // A balance notice is queued for the gateway.
        let out = crate::gateway_out::take(&mut reg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, NOTICE);
        // Depositing the rest takes the stack away whole.
        let args = [
            PyValue::Int(STATION),
            PyValue::Int(item),
            PyValue::Int(30),
            PyValue::Int(0),
        ];
        deposit(&mut reg, &req("DepositPlexToVault", &args)).unwrap();
        assert!(reg.store().get(item).is_none());
        assert_eq!((held(&reg), balance(reg.deltas(), 21)), (0, 50));
    }

    #[test]
    fn a_bad_deposit_changes_nothing_and_pushes_a_failure_with_its_reference() {
        let mut reg = world();
        let item = stack(&mut reg, 5);
        for qty in [0, 6, -1] {
            let args = [
                PyValue::Int(STATION),
                PyValue::Int(item),
                PyValue::Int(qty),
                PyValue::Int(0),
            ];
            let PyValue::Int(reference) =
                deposit(&mut reg, &req("DepositPlexToVault", &args)).unwrap()
            else {
                panic!()
            };
            let ev = events(&mut reg);
            let failed = ev
                .iter()
                .find(|(n, _)| *n == "OnPlexDepositFailed")
                .expect("a failure");
            assert_eq!(failed.1[1], PyValue::Int(reference));
            assert_eq!(failed.1[0], crate::pyutil::bstr("PlexDepositInvalidAmount"));
        }
        // Something that is not PLEX, and an item that is not there.
        let args = [
            PyValue::Int(STATION),
            PyValue::Int(1),
            PyValue::Int(1),
            PyValue::Int(0),
        ];
        deposit(&mut reg, &req("DepositPlexToVault", &args)).unwrap();
        assert_eq!((held(&reg), balance(reg.deltas(), 21)), (5, 0));
    }

    #[test]
    fn deposit_then_withdraw_conserves_units() {
        let mut reg = world();
        let item = stack(&mut reg, 40);
        let args = [
            PyValue::Int(STATION),
            PyValue::Int(item),
            PyValue::Int(40),
            PyValue::Int(0),
        ];
        deposit(&mut reg, &req("DepositPlexToVault", &args)).unwrap();
        let _ = events(&mut reg);
        let total = |reg: &Registry| held(reg) + balance(reg.deltas(), 21);
        assert_eq!(total(&reg), 40);
        let args = [
            PyValue::Int(15),
            PyValue::Int(STATION),
            PyValue::Int(0),
            PyValue::Int(crate::model::FLAG_HANGAR),
            PyValue::None,
        ];
        let PyValue::Int(reference) =
            withdraw(&mut reg, &req("WithdrawPlexFromVault", &args)).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            (held(&reg), balance(reg.deltas(), 21), total(&reg)),
            (15, 25, 40)
        );
        let ev = events(&mut reg);
        let added = ev
            .iter()
            .find(|(n, _)| *n == "OnPlexItemAddedToInventory")
            .expect("the item event");
        assert_eq!(added.1[1], PyValue::Int(reference));
        let PyValue::Int(new_item) = added.1[0] else {
            panic!()
        };
        let it = reg.store().get(new_item).expect("the new stack");
        assert_eq!(
            (it.type_id, it.quantity, it.location_id),
            (TYPE_PLEX, 15, STATION)
        );
        // The ledger saw both movements.
        assert_eq!(ledger_of(reg.deltas(), "deposit"), (40, 0));
        assert_eq!(ledger_of(reg.deltas(), "withdraw"), (0, 15));
    }

    #[test]
    fn a_withdraw_past_the_balance_or_to_nowhere_fails_and_changes_nothing() {
        let mut reg = world();
        credit(reg.deltas_mut(), 21, 10, "gm", 1).unwrap();
        let before = reg.deltas().clone();
        let to = |q: i64, dest: i64| {
            [
                PyValue::Int(q),
                PyValue::Int(dest),
                PyValue::Int(0),
                PyValue::Int(4),
                PyValue::None,
            ]
        };
        for (q, dest, key) in [
            (11, STATION, "PlexWithdrawalInsufficientFunds"),
            (0, STATION, "PlexWithdrawalInvalidAmount"),
            (5, 12_345, "PlexWithdrawalInternalError"),
        ] {
            let args = to(q, dest);
            let PyValue::Int(reference) =
                withdraw(&mut reg, &req("WithdrawPlexFromVault", &args)).unwrap()
            else {
                panic!()
            };
            let ev = events(&mut reg);
            let f = ev
                .iter()
                .find(|(n, _)| *n == "OnPlexWithdrawFailed")
                .expect("a failure");
            assert_eq!(f.1, vec![crate::pyutil::bstr(key), PyValue::Int(reference)]);
        }
        assert_eq!(held(&reg), 0);
        assert_eq!(balance(reg.deltas(), 21), 10);
        // Only the reference counter moved.
        assert_eq!(ledger(reg.deltas()), ledger(&before));
    }

    #[test]
    fn the_pilots_other_accounts_vault_is_not_theirs() {
        let mut reg = crate::testkit::registry_with(|cfg| {
            let a = crate::model::Character {
                character_id: PILOT,
                owner_user_id: Some(21),
                ..Default::default()
            };
            let mut b = a.clone();
            b.character_id = PILOT + 1;
            b.ship_id = a.ship_id + 1;
            // Same account: shares the vault.
            cfg.characters = vec![a, b];
        });
        credit(reg.deltas_mut(), 21, 3, "gm", 1).unwrap();
        assert_eq!(key_of(&reg, PILOT), key_of(&reg, PILOT + 1));
        announce(
            &mut reg,
            21,
            &Entry {
                seq: 1,
                ts: 1,
                delta_cents: 300,
                resulting_cents: 300,
            },
        );
        assert_eq!(
            crate::gateway_out::take(&mut reg).len(),
            2,
            "a notice per pilot of the account"
        );
    }

    // ---- review fixes ---------------------------------------------------

    fn withdraw_to(
        reg: &mut Registry,
        qty: i64,
        dest: i64,
        flag: i64,
    ) -> Vec<(&'static str, Vec<PyValue>)> {
        let args = [
            PyValue::Int(qty),
            PyValue::Int(dest),
            PyValue::Int(0),
            PyValue::Int(flag),
            PyValue::None,
        ];
        withdraw(reg, &req("WithdrawPlexFromVault", &args)).unwrap();
        events(reg)
    }

    fn failed(ev: &[(&'static str, Vec<PyValue>)]) -> bool {
        ev.iter().any(|(n, _)| *n == "OnPlexWithdrawFailed")
    }

    /// H3: undocking leaves `Character::station_id` set; the roster says where
    /// the pilot is, and a pilot in space cannot withdraw into that hangar.
    #[test]
    fn a_pilot_in_space_cannot_withdraw_to_their_last_hangar() {
        let mut reg = world();
        credit(reg.deltas_mut(), 21, 10, "gm", 1).unwrap();
        let ship = reg.character(PILOT).unwrap().ship_id;
        assert!(
            failed(&withdraw_to(&mut reg, 0, STATION, 4)),
            "control: a bad amount fails"
        );
        assert!(
            !failed(&withdraw_to(&mut reg, 2, STATION, 4)),
            "docked it works"
        );
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                PILOT,
                ship,
                777_001,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        assert!(reg.character(PILOT).unwrap().station_id.is_some());
        assert!(failed(&withdraw_to(&mut reg, 2, STATION, 4)));
        assert_eq!(
            balance(reg.deltas(), 21),
            8,
            "the refused call cost nothing"
        );
    }

    /// L2: only the station hangar (flag 4) or the active ship's cargo.
    #[test]
    fn a_withdrawal_may_only_name_the_hangar_or_the_ships_cargo() {
        let mut reg = world();
        credit(reg.deltas_mut(), 21, 10, "gm", 1).unwrap();
        let other_stack = stack(&mut reg, 3);
        // An owned item that is not the active ship, however owned.
        assert!(failed(&withdraw_to(&mut reg, 1, other_stack, 5)));
        // The hangar with a flag that is not the hangar's.
        assert!(failed(&withdraw_to(&mut reg, 1, STATION, 11)));
        assert_eq!(balance(reg.deltas(), 21), 10);
        assert_eq!(held(&reg), 3);
    }

    /// L1: a stack the vault cannot hold is refused and the item survives.
    #[test]
    fn a_deposit_that_would_overflow_the_vault_keeps_the_item() {
        let mut reg = world();
        let item = stack(&mut reg, 5);
        credit(reg.deltas_mut(), 21, i64::MAX / CENTS - 2, "gm", 1).unwrap();
        let args = [
            PyValue::Int(STATION),
            PyValue::Int(item),
            PyValue::Int(5),
            PyValue::Int(0),
        ];
        deposit(&mut reg, &req("DepositPlexToVault", &args)).unwrap();
        let ev = events(&mut reg);
        assert!(
            ev.iter().any(|(n, _)| *n == "OnPlexDepositFailed"),
            "{ev:?}"
        );
        assert_eq!(held(&reg), 5, "nothing was destroyed");
    }

    // ---- the gateway ----------------------------------------------------

    #[test]
    fn the_balance_request_answers_sint64_cents() {
        let g = Ground::local();
        // A Local ground has no owners: the pilot's own negated id is the key.
        g.write(|d| {
            credit(d, -8, 3, "gm", 1).unwrap();
        });
        let r = gateway("eve_public.plex.vault.api.BalanceRequest", b"", 8, &g).unwrap();
        assert_eq!(r.status, 200);
        let (name, body) = r.payload.unwrap();
        assert_eq!(name, "eve_public.plex.vault.api.BalanceResponse");
        let cur = get_bytes(&body, 1).unwrap();
        // zigzag(300) = 600
        assert_eq!(crate::gateway::get_uint(&cur, 1), Some(600));
    }

    #[test]
    fn the_real_chain_asks_the_vault_before_the_gateways_zero_stub() {
        let g = Ground::local();
        g.write(|d| {
            credit(d, -8, 2, "gm", 1).unwrap();
        });
        let r = crate::gateway_ext::handle("eve_public.plex.vault.api.BalanceRequest", b"", 8, &g)
            .expect("the chain answers");
        let cur = get_bytes(&r.payload.unwrap().1, 1).unwrap();
        assert_eq!(
            crate::gateway::get_uint(&cur, 1),
            Some(400),
            "zigzag(200), not the stub's 0"
        );
    }

    #[test]
    fn a_negative_amount_is_zigzagged() {
        assert_eq!(crate::gateway::get_uint(&currency(-200), 1), Some(399));
        assert_eq!(crate::gateway::get_uint(&currency(0), 1), Some(0));
    }

    #[test]
    fn the_log_requests_list_read_and_total() {
        let g = Ground::local();
        g.write(|d| {
            credit(d, -8, 10, "gm", 50).unwrap();
            debit(d, -8, 4, "nes:k", 60).unwrap();
        });
        let all = gateway(
            "eve_public.plex.vault.transaction.api.GetAllLoggedForUserRequest",
            b"",
            8,
            &g,
        )
        .unwrap();
        let (_, body) = all.payload.unwrap();
        let ids = crate::gateway::field(&body, 1);
        assert_eq!(ids.len(), 2);
        // Newest first.
        assert_eq!(crate::gateway::get_uint(&ids[0], 1), Some(2));
        let mut req = Vec::new();
        put_bytes(&mut req, 1, &identifier(2));
        let one = gateway(
            "eve_public.plex.vault.transaction.api.GetLogRequest",
            &req,
            8,
            &g,
        )
        .unwrap();
        let (_, body) = one.payload.unwrap();
        let attrs = get_bytes(&body, 1).expect("the transaction");
        let moved = get_bytes(&attrs, 2).unwrap();
        assert_eq!(crate::gateway::get_uint(&moved, 1), Some(799)); // zigzag(-400)
        // An id that is not in the log is `unavailable`.
        let mut req = Vec::new();
        put_bytes(&mut req, 1, &identifier(99));
        let gone = gateway(
            "eve_public.plex.vault.transaction.api.GetLogRequest",
            &req,
            8,
            &g,
        )
        .unwrap();
        assert_eq!(
            crate::gateway::get_uint(&gone.payload.unwrap().1, 2),
            Some(1)
        );
        let stats = gateway(
            "eve_public.plex.vault.transaction.api.GetStatisticsRequest",
            b"",
            8,
            &g,
        )
        .unwrap();
        let entry = get_bytes(&stats.payload.unwrap().1, 1).unwrap();
        assert_eq!(crate::gateway::get_uint(&entry, 5), Some(2));
    }

    #[test]
    fn other_gateway_types_fall_through() {
        let g = Ground::local();
        assert_eq!(
            gateway("eve_public.cosmetic.ship.api.GetRequest", b"", 8, &g),
            None
        );
        assert_eq!(
            gateway("eve_public.plex.vault.api.PurchaseRequest", b"", 8, &g),
            None
        );
    }

    #[test]
    fn the_notice_names_the_transaction() {
        let e = Entry {
            seq: 4,
            ts: 77,
            delta_cents: -500,
            resulting_cents: 1_500,
        };
        let p = notice_payload(&e);
        assert_eq!(
            crate::gateway::get_uint(&get_bytes(&p, 1).unwrap(), 1),
            Some(4)
        );
        let a = get_bytes(&p, 2).unwrap();
        let after = get_bytes(&a, 3).unwrap();
        assert_eq!(crate::gateway::get_uint(&after, 1), Some(3_000));
    }
}
