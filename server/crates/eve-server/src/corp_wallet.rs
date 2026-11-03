//! Corporation wallets: the state and the rules, the pure half (CORP-W;
//! lane note `docs/lanes/corp-w.md`).
//!
//! A player corporation has **seven wallet divisions**, accounting keys
//! 1000..=1006 (`appConst.accountingKeyCash` and `Cash2..7`) **[V]**, with
//! division 1 the Master Wallet. Each holds ISK, and each has its own
//! journal, which is what the Wallet window's Corporation tab reads
//! (`account.GetTransactions(accountKey, year, month, True)`).
//!
//! **The deltas are the only copy**, four kinds (`delta_kinds::KINDS`), each
//! changed one row at a time so a busy corp does not rewrite its whole
//! history on every payout:
//!
//! ```text
//! corpwal   <corp key>      -> cents                   a division's balance
//! corpwjrn  <corp seq>      -> key date ref o1 o2 reference cents balance d:description
//! corpwdiv  <corp>          -> 14 x (n:name | -)        hangar 1..7, wallet 1..7
//! corpwkey  <character>     -> key                      the active division
//! ```
//!
//! **Balances are whole cents**, never an `f64`, so a thousand small tax
//! payments and a withdrawal of the lot leave exactly nothing behind. The
//! wire speaks ISK as a float; [`to_cents`] and [`to_isk`] are the only
//! crossings. `seq` is zero-padded so a corp's rows sort in time order, and
//! the journal is pruned ([`prune`]) to [`JOURNAL_KEEP_DAYS`] and
//! [`JOURNAL_MAX_ROWS`].
//!
//! Who may do what is [`Standing`], from the client's own checks
//! (`walletUtil.py`); the wire, the pushes and the transfers are
//! `service/corp_wallet.rs`. Everything here is tested with invented ids.

use crate::delta::Deltas;
use eve_marshal::PyValue;

/// The delta kinds this module owns (`delta_kinds::KINDS`).
pub const KIND_BALANCE: &str = "corpwal";
pub const KIND_JOURNAL: &str = "corpwjrn";
pub const KIND_NAMES: &str = "corpwdiv";
pub const KIND_KEY: &str = "corpwkey";
pub const KINDS: [&str; 4] = [KIND_BALANCE, KIND_JOURNAL, KIND_NAMES, KIND_KEY];

/// `appConst.accountingKeyCash = 1000`, the Master Wallet; divisions 2..7
/// are 1001..=1006 **[V]** (`inventorycommon/const.pyc`).
pub const MASTER_KEY: i64 = 1000;
/// Wallet divisions per corporation.
pub const DIVISIONS: usize = 7;

/// Whether `key` is one of the seven accounting keys.
pub fn is_key(key: i64) -> bool {
    (MASTER_KEY..MASTER_KEY + DIVISIONS as i64).contains(&key)
}

/// The key's name in `OnAccountChange(accountKey, ownerID, balance)`:
/// `cash`, `cash2` .. `cash7`. The client maps it back with
/// `get_accounting_key_for_name(accountKey, 1000)` and tests membership of
/// exactly this tuple (`walletSvc.OnAccountChange`) **[V]**.
pub fn key_name(key: i64) -> String {
    match key - MASTER_KEY {
        0 => "cash".to_owned(),
        n => format!("cash{}", n + 1),
    }
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

/// `appConst.corpRoleDirector` **[V]**.
pub const ROLE_DIRECTOR: i64 = 1;
/// `appConst.corpRoleAccountant` **[V]**.
pub const ROLE_ACCOUNTANT: i64 = 256;
/// `appConst.corpRoleJuniorAccountant` **[V]**.
pub const ROLE_JUNIOR_ACCOUNTANT: i64 = 4_503_599_627_370_496;
/// `appConst.corpRoleTrader` **[V]**.
pub const ROLE_TRADER: i64 = 18_014_398_509_481_984;
/// `appConst.corpRoleAccountCanTake1..7`, indexed by division **[V]**. There
/// is no "query" role in this build's `appConst`: the client's read checks
/// (`HaveReadAccessToCorpWalletDivision`) are the take roles, and the
/// accountants see every division (`transactionsPanel._GetAccountKeyList`).
pub const ROLE_ACCOUNT_CAN_TAKE: [i64; DIVISIONS] = [
    134_217_728,
    268_435_456,
    536_870_912,
    1_073_741_824,
    2_147_483_648,
    4_294_967_296,
    8_589_934_592,
];
/// All seven take roles.
pub const ALL_TAKE_ROLES: i64 = ROLE_ACCOUNT_CAN_TAKE[0]
    | ROLE_ACCOUNT_CAN_TAKE[1]
    | ROLE_ACCOUNT_CAN_TAKE[2]
    | ROLE_ACCOUNT_CAN_TAKE[3]
    | ROLE_ACCOUNT_CAN_TAKE[4]
    | ROLE_ACCOUNT_CAN_TAKE[5]
    | ROLE_ACCOUNT_CAN_TAKE[6];
/// What a founding CEO gets besides director and personnel manager, so the
/// client builds the Corporation tab at all: `HaveAccessToCorpWallet` is
/// accountant, junior accountant or any take role, and director alone does
/// not open it **[V]** (`walletUtil.py`). **[A]** that Tranquility gives a
/// CEO every role.
pub const CEO_WALLET_ROLES: i64 = ROLE_ACCOUNTANT | ALL_TAKE_ROLES;

/// What the acting member is to the corporation, for the wallet.
///
/// `roles` is the client's `session.corprole`: `rolesAtAll | rolesAtOther`
/// (no HQ or base match) **[V]** (`basesession`). The CEO and a director are
/// **full**: every division, tax, and (CEO only) division names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    pub ceo: bool,
    pub roles: i64,
}

impl Standing {
    pub fn director(&self) -> bool {
        self.ceo || self.roles & ROLE_DIRECTOR != 0
    }

    fn take_role(&self, key: i64) -> bool {
        is_key(key) && self.roles & ROLE_ACCOUNT_CAN_TAKE[(key - MASTER_KEY) as usize] != 0
    }

    /// May read division `key`: its balance, its journal, its pushes.
    /// Directors, accountants and junior accountants see every division;
    /// anybody else only the divisions they may take from.
    pub fn can_query(&self, key: i64) -> bool {
        is_key(key)
            && (self.director()
                || self.roles & (ROLE_ACCOUNTANT | ROLE_JUNIOR_ACCOUNTANT) != 0
                || self.take_role(key))
    }

    /// May take ISK out of division `key`. An accountant role alone does not
    /// (it reads and pays bills); the per-division take role does, and so
    /// does a director.
    pub fn can_take(&self, key: i64) -> bool {
        is_key(key) && (self.director() || self.take_role(key))
    }

    /// Whether the client builds the Corporation wallet tab for this member
    /// (`walletUtil.HaveAccessToCorpWallet`) **[V]**.
    pub fn sees_wallet(&self) -> bool {
        self.roles & (ROLE_ACCOUNTANT | ROLE_JUNIOR_ACCOUNTANT | ALL_TAKE_ROLES) != 0
    }

    /// The corp's tax rate: the client offers the edit to directors
    /// (`corp_ui_home`: `SessionChecker.IsCorpDirector`) **[V]**.
    pub fn can_set_tax(&self) -> bool {
        self.director()
    }

    /// The division names: the CEO only (`DivisionsForm` raises
    /// `CorpAccessOnlyCEOEditDivisionNames` otherwise) **[V]**.
    pub fn can_name_divisions(&self) -> bool {
        self.ceo
    }
}

// ---------------------------------------------------------------------------
// Money
// ---------------------------------------------------------------------------

/// ISK to whole cents.
pub fn to_cents(isk: f64) -> i64 {
    (isk * 100.0).round() as i64
}

/// Whole cents to ISK.
pub fn to_isk(cents: i64) -> f64 {
    cents as f64 / 100.0
}

/// Why a balance did not move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Short {
    /// Not an accounting key.
    BadKey,
    /// The division holds less than the debit.
    Insufficient { have: i64 },
}

fn row_key(corp: i64, key: i64) -> Vec<String> {
    vec![corp.to_string(), key.to_string()]
}

/// Division `key`'s balance in cents.
pub fn balance(d: &Deltas, corp: i64, key: i64) -> i64 {
    d.of(KIND_BALANCE)
        .get(&row_key(corp, key))
        .and_then(|v| v.first()?.parse().ok())
        .unwrap_or(0)
}

/// All seven balances, in key order.
pub fn balances(d: &Deltas, corp: i64) -> [i64; DIVISIONS] {
    let mut out = [0; DIVISIONS];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = balance(d, corp, MASTER_KEY + i as i64);
    }
    out
}

/// The corp's whole wallet, in cents.
pub fn total(d: &Deltas, corp: i64) -> i64 {
    balances(d, corp).iter().sum()
}

/// Every corp with a balance row, and its total in cents.
pub fn totals(d: &Deltas) -> std::collections::BTreeMap<i64, i64> {
    let mut out = std::collections::BTreeMap::new();
    for (k, v) in d.of(KIND_BALANCE) {
        let corp = k.first().and_then(|c| c.parse::<i64>().ok());
        let cents = v.first().and_then(|c| c.parse::<i64>().ok());
        if let (Some(corp), Some(cents)) = (corp, cents) {
            *out.entry(corp).or_insert(0) += cents;
        }
    }
    out
}

/// Move `delta` cents into (positive) or out of (negative) a division.
/// A debit larger than the balance changes nothing. Answers the new balance.
pub fn adjust(d: &mut Deltas, corp: i64, key: i64, delta: i64) -> Result<i64, Short> {
    if !is_key(key) {
        return Err(Short::BadKey);
    }
    let have = balance(d, corp, key);
    let now = have + delta;
    if now < 0 {
        return Err(Short::Insufficient { have });
    }
    d.put(KIND_BALANCE, row_key(corp, key), vec![now.to_string()]);
    Ok(now)
}

// ---------------------------------------------------------------------------
// The journal
// ---------------------------------------------------------------------------

/// Days of corp journal kept **[A]**: the client's date combo offers the
/// last six months (`GetLastNYearMonthTuples(6)`); three is what a friends'
/// server reads. Falsified by a CEO wanting the older months.
pub const JOURNAL_KEEP_DAYS: i64 = 90;
/// Rows kept per corp **[A]**, newest. A corp whose 50 bots each book a tax
/// row every twenty minutes writes about 3,600 a day; the cap bounds the
/// world file whatever the age rule says.
pub const JOURNAL_MAX_ROWS: usize = 20_000;
/// Pruning runs on every this-many'th row of a corp, so its scan is
/// amortised.
const PRUNE_EVERY: i64 = 64;

const TICKS_PER_DAY: i64 = 864_000_000_000;

/// One corp journal row. Amounts are cents; `balance` is the division's
/// balance after the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub seq: i64,
    pub key: i64,
    pub date: i64,
    pub ref_type: i64,
    pub owner1: i64,
    pub owner2: i64,
    pub reference: i64,
    pub amount: i64,
    pub balance: i64,
    pub description: String,
}

fn seq_key(corp: i64, seq: i64) -> Vec<String> {
    vec![corp.to_string(), format!("{seq:012}")]
}

fn last_seq(d: &Deltas, corp: i64) -> i64 {
    let c = corp.to_string();
    d.of(KIND_JOURNAL)
        .range(vec![c.clone(), String::new()]..=vec![c, "999999999999".to_owned()])
        .next_back()
        .and_then(|(k, _)| k.get(1)?.parse().ok())
        .unwrap_or(0)
}

impl Row {
    fn to_value(&self) -> Vec<String> {
        vec![
            self.key.to_string(),
            self.date.to_string(),
            self.ref_type.to_string(),
            self.owner1.to_string(),
            self.owner2.to_string(),
            self.reference.to_string(),
            self.amount.to_string(),
            self.balance.to_string(),
            format!("d:{}", self.description),
        ]
    }

    fn from_row(key: &[String], v: &[String]) -> Option<Row> {
        let int = |i: usize| v.get(i)?.parse::<i64>().ok();
        Some(Row {
            seq: key.get(1)?.parse().ok()?,
            key: int(0)?,
            date: int(1)?,
            ref_type: int(2)?,
            owner1: int(3)?,
            owner2: int(4)?,
            reference: int(5)?,
            amount: int(6)?,
            balance: int(7)?,
            description: v.get(8)?.strip_prefix("d:")?.to_owned(),
        })
    }
}

/// Append a row to the corp's journal; answers its `seq`. `date` is a real
/// FILETIME (never zero: a year-1601 date takes the whole Journal tab down,
/// `crate::ledger`'s module docs).
#[allow(clippy::too_many_arguments)]
pub fn record(
    d: &mut Deltas,
    corp: i64,
    key: i64,
    date: i64,
    ref_type: i64,
    owner1: i64,
    owner2: i64,
    reference: i64,
    amount: i64,
    balance: i64,
    description: &str,
) -> i64 {
    let seq = last_seq(d, corp) + 1;
    let row = Row {
        seq,
        key,
        date,
        ref_type,
        owner1,
        owner2,
        reference,
        amount,
        balance,
        description: description.to_owned(),
    };
    d.put(KIND_JOURNAL, seq_key(corp, seq), row.to_value());
    if seq % PRUNE_EVERY == 0 {
        prune(d, corp, date);
    }
    seq
}

/// Every row of one corp, oldest first.
pub fn all_rows(d: &Deltas, corp: i64) -> Vec<Row> {
    let c = corp.to_string();
    d.of(KIND_JOURNAL)
        .range(vec![c.clone(), String::new()]..=vec![c, "999999999999".to_owned()])
        .filter_map(|(k, v)| Row::from_row(k, v))
        .collect()
}

/// The rows the Wallet window asked for: division `key`, and either the
/// last thirty days (`year`, `month` both `None`, the window's own default)
/// or that calendar month (`month` 1..=12) **[V]** (see `ledger::Journal::select`).
pub fn rows(
    d: &Deltas,
    corp: i64,
    key: i64,
    now: i64,
    year: Option<i64>,
    month: Option<i64>,
) -> Vec<Row> {
    let cutoff = now - 30 * TICKS_PER_DAY;
    all_rows(d, corp)
        .into_iter()
        .filter(|r| r.key == key)
        .filter(|r| match (year, month) {
            (Some(y), Some(m)) => {
                let (ey, em, _) = crate::ledger::civil_from_filetime(r.date);
                ey == y && em == m
            }
            _ => r.date >= cutoff,
        })
        .collect()
}

/// Drop rows older than [`JOURNAL_KEEP_DAYS`] and any beyond
/// [`JOURNAL_MAX_ROWS`], oldest first, and never the newest row (so the
/// next `seq` is never reused). Answers how many went.
pub fn prune(d: &mut Deltas, corp: i64, now: i64) -> usize {
    let rows = all_rows(d, corp);
    let cutoff = now - JOURNAL_KEEP_DAYS * TICKS_PER_DAY;
    let excess = rows.len().saturating_sub(JOURNAL_MAX_ROWS);
    let mut gone = 0;
    for (i, r) in rows.iter().enumerate() {
        if i + 1 == rows.len() {
            break;
        }
        if i < excess || r.date < cutoff {
            d.del(KIND_JOURNAL, &seq_key(corp, r.seq));
            gone += 1;
        }
    }
    gone
}

// ---------------------------------------------------------------------------
// Division names
// ---------------------------------------------------------------------------

/// Names stored per corp: hangar divisions 1..7, then wallet divisions
/// 1..7. `walletDivision1` is the Master Wallet, which the client never
/// reads a name for (`bco_corporations.GetDivisionNames` supplies its
/// label) **[V]**, so slot 7 is stored but never sent.
pub const NAME_COUNT: usize = 14;
/// The create form's `maxlength` on every division name **[V]**
/// (`corp_ui_home.DivisionsForm`).
pub const NAME_MAX_LEN: usize = 50;

/// The corp's division names, `None` for the client's default label.
pub fn names(d: &Deltas, corp: i64) -> [Option<String>; NAME_COUNT] {
    let mut out: [Option<String>; NAME_COUNT] = Default::default();
    if let Some(v) = d.of(KIND_NAMES).get(&vec![corp.to_string()]) {
        for (slot, f) in out.iter_mut().zip(v) {
            *slot = f.strip_prefix("n:").map(str::to_owned);
        }
    }
    out
}

/// Store the names.
pub fn set_names(d: &mut Deltas, corp: i64, names: &[Option<String>; NAME_COUNT]) {
    let v = names
        .iter()
        .map(|n| {
            n.as_ref()
                .map_or_else(|| "-".to_owned(), |s| format!("n:{s}"))
        })
        .collect();
    d.put(KIND_NAMES, vec![corp.to_string()], v);
}

/// The corp's name for wallet division `key`, when it has set one. The
/// Master Wallet has none to set.
pub fn wallet_name(d: &Deltas, corp: i64, key: i64) -> Option<String> {
    let n = key - MASTER_KEY;
    if !is_key(key) || n == 0 {
        return None;
    }
    names(d, corp)[7 + n as usize].clone()
}

/// What a `UpdateDivisionNames` value means. A blank name resets the
/// division to the client's default label **[A]**; otherwise the name is
/// trimmed and checked. The `Err` is the client's own message key
/// (`CorpDivName*` for a hangar division, `WalletCorpDivName*` for a wallet
/// one; both are `notify`-type, which `ApplyDivisionNames` shows inline)
/// **[V]**.
/// A character no owner-visible name may hold: a control character, or one
/// of the four that open markup (`<`, `>`, `&`, `"`), since names are drawn
/// in rich-text labels. The one rule for division, alliance and corporation
/// names (SEC-FIX 7).
pub fn bad_name_char(c: char) -> bool {
    c.is_control() || matches!(c, '<' | '>' | '&' | '"')
}

pub fn check_name(raw: &str, wallet: bool) -> Result<Option<String>, &'static str> {
    let name = raw.trim();
    let key = |hangar: &'static str, wal: &'static str| if wallet { wal } else { hangar };
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > NAME_MAX_LEN {
        return Err(key(
            "CorpDivNameInvalidMaxLength",
            "WalletCorpDivNameInvalidMaxLength",
        ));
    }
    if name.chars().any(bad_name_char) {
        return Err(key(
            "CorpDivNameInvalidSomeChar",
            "WalletCorpDivNameInvalidSomeChar",
        ));
    }
    Ok(Some(name.to_owned()))
}

/// Whether two of `names` (the hangar seven, or the wallet seven) are the
/// same name, ignoring case **[A]** `...InvalidTaken`.
pub fn duplicate(names: &[Option<String>]) -> bool {
    let set: Vec<String> = names
        .iter()
        .flatten()
        .map(|n| n.to_ascii_lowercase())
        .collect();
    set.iter()
        .enumerate()
        .any(|(i, n)| set[..i].iter().any(|m| m == n))
}

// ---------------------------------------------------------------------------
// The active division
// ---------------------------------------------------------------------------

/// A member's active division (`session.corpAccountKey`): the Master Wallet
/// until they pick another.
pub fn active_key(d: &Deltas, character: i64) -> i64 {
    d.of(KIND_KEY)
        .get(&vec![character.to_string()])
        .and_then(|v| v.first()?.parse().ok())
        .filter(|k| is_key(*k))
        .unwrap_or(MASTER_KEY)
}

/// Remember the active division.
pub fn set_active_key(d: &mut Deltas, character: i64, key: i64) {
    d.put(KIND_KEY, vec![character.to_string()], vec![key.to_string()]);
}

// ---------------------------------------------------------------------------
// Tax
// ---------------------------------------------------------------------------

/// `appConst.minCorporationTaxAmount = 100000.0` **[V]**: the figure the
/// corp-details form's tax hint is told (`ISKTaxRateDescription
/// min_isk_amount`) and the derived tax row's label is given
/// (`amountInIsk`). **[A]** that it is the smallest payout the tax is taken
/// from. Falsified by the hint's English text (`tools/loctable.py`, label
/// `UI/Corporations/BaseCorporationUI/ISKTaxRateDescription`) saying
/// something else, or by a Tranquility capture of a smaller taxed payout.
pub const MIN_TAXED_PAYOUT: f64 = 100_000.0;

/// `appConst.refCorporationTaxNpcBounties`, `...AgentRewards` and
/// `...AgentBonusRewards` **[V]**. A tax is never a journal row of its own:
/// the payout's row carries the net amount and a description naming the
/// tax by one of these ids, and the *client* derives the tax row
/// (`transactionsUtil.GetDerivedTransactions`) **[V]**.
pub const REF_CORPORATION_TAX_NPC_BOUNTIES: i64 = 92;
pub const REF_CORPORATION_TAX_AGENT_REWARDS: i64 = 93;
pub const REF_CORPORATION_TAX_AGENT_BONUS_REWARDS: i64 = 94;
/// `appConst.refCorporationPayment` **[V]**: ISK one corporation pays
/// another.
pub const REF_CORPORATION_PAYMENT: i64 = 11;
/// `appConst.refCorporationAccountWithdrawal` **[V]**: ISK a member takes
/// out of a corporation division.
pub const REF_CORPORATION_ACCOUNT_WITHDRAWAL: i64 = 37;

/// The tax id for a payout's ref: the NPC bounty prize (single or
/// aggregated), the agent reward and the agent time bonus, as
/// `appConst.derivedTransactionParentMapping` pairs them **[V]**.
pub fn tax_ref(ref_type: i64) -> Option<i64> {
    match ref_type {
        17 | 85 => Some(REF_CORPORATION_TAX_NPC_BOUNTIES),
        33 => Some(REF_CORPORATION_TAX_AGENT_REWARDS),
        34 => Some(REF_CORPORATION_TAX_AGENT_BONUS_REWARDS),
        _ => None,
    }
}

/// The corp's cut of `gross` ISK at `rate` (a fraction), in whole cents of
/// ISK, `0` below [`MIN_TAXED_PAYOUT`] or at a zero rate.
pub fn tax_for(rate: f64, gross: f64) -> f64 {
    if !(rate.is_finite() && gross.is_finite()) || rate <= 0.0 || gross < MIN_TAXED_PAYOUT {
        return 0.0;
    }
    to_isk(to_cents(gross * rate.min(1.0)))
}

/// A payout row's description with the tax stamped in:
/// `{92: [corpID, taxAmount], <the rest>}`. `base` is the payout's own
/// description, a YAML flow mapping or empty (`transactionsUtil` accepts
/// only those two) **[V]**. The tax amount is written with two decimals so
/// it loads as a float.
pub fn stamp(base: &str, tax_ref: i64, corp: i64, tax: f64) -> String {
    let entry = format!("{tax_ref}: [{corp}, {tax:.2}]");
    let base = base.trim();
    match base.strip_prefix('{').and_then(|b| b.strip_suffix('}')) {
        Some(rest) if !rest.trim().is_empty() => format!("{{{entry}, {}}}", rest.trim()),
        _ => format!("{{{entry}}}"),
    }
}

/// A YAML double-quoted scalar for `text`, safe in a flow mapping.
pub fn yaml_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The transfer window's reason edit is `maxLength=40` **[V]**
/// (`transferMoneyWnd.py`, the `reason` `SingleLineEditText`).
pub const REASON_MAX_LEN: usize = 40;

/// The description of a transfer with a reason: `{DESC: "reason"}`
/// (`appConst.recDescription`, shown as the row's hint) **[V]**; empty for
/// no reason.
pub fn reason_description(reason: &str) -> String {
    // SEC-FIX 3: cut to the client's limit, so no caller can write a
    // megabyte into both journals.
    let reason: String = reason.trim().chars().take(REASON_MAX_LEN).collect();
    let reason = reason.trim();
    if reason.is_empty() {
        String::new()
    } else {
        format!("{{DESC: {}}}", yaml_quote(reason))
    }
}

// ---------------------------------------------------------------------------
// The push
// ---------------------------------------------------------------------------

/// `OnAccountChange(accountKey, corpID, balance)` for a corporation
/// division, to the members who may read it. The client's
/// `WalletSvc.OnAccountChange` takes it when the key is one of
/// `cash`..`cash7` and the owner is `session.corpid`, then checks the
/// member's own take role before updating anything **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct CorpAccountChange {
    pub to: Vec<i64>,
    pub corporation_id: i64,
    pub key: i64,
    pub balance: f64,
}

impl crate::notify::Push for CorpAccountChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnAccountChange",
            vec![
                PyValue::Bytes(key_name(self.key).into_bytes()),
                PyValue::Int(self.corporation_id),
                PyValue::Real(self.balance),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORP: i64 = 98_000_777;
    const OTHER: i64 = 98_000_778;
    const NOW: i64 = 134_116_992_000_000_000 + 40 * TICKS_PER_DAY;

    fn standing(roles: i64) -> Standing {
        Standing { ceo: false, roles }
    }

    #[test]
    fn the_seven_keys_have_the_clients_names() {
        assert!(is_key(1000) && is_key(1006));
        assert!(!is_key(999) && !is_key(1007));
        let names: Vec<String> = (1000..1007).map(key_name).collect();
        assert_eq!(
            names,
            ["cash", "cash2", "cash3", "cash4", "cash5", "cash6", "cash7"]
        );
    }

    #[test]
    fn the_take_roles_are_the_clients_bits() {
        for (i, role) in ROLE_ACCOUNT_CAN_TAKE.iter().enumerate() {
            assert_eq!(*role, 1_i64 << (27 + i));
        }
        assert_eq!(ROLE_ACCOUNTANT, 1 << 8);
        assert_eq!(ROLE_JUNIOR_ACCOUNTANT, 1 << 52);
    }

    #[test]
    fn the_ceo_and_a_director_are_full() {
        let ceo = Standing {
            ceo: true,
            roles: 0,
        };
        let director = standing(ROLE_DIRECTOR);
        for key in 1000..1007 {
            for s in [ceo, director] {
                assert!(s.can_query(key) && s.can_take(key));
            }
        }
        assert!(ceo.can_set_tax() && director.can_set_tax());
        assert!(ceo.can_name_divisions());
        assert!(!director.can_name_divisions(), "the client says CEO only");
    }

    #[test]
    fn an_accountant_reads_every_division_and_takes_from_none() {
        let a = standing(ROLE_ACCOUNTANT);
        let j = standing(ROLE_JUNIOR_ACCOUNTANT);
        for key in 1000..1007 {
            assert!(a.can_query(key) && j.can_query(key));
            assert!(!a.can_take(key) && !j.can_take(key));
        }
        assert!(a.sees_wallet() && j.sees_wallet());
        assert!(!a.can_set_tax());
    }

    #[test]
    fn a_take_role_is_one_division() {
        let s = standing(ROLE_ACCOUNT_CAN_TAKE[2]);
        assert!(s.can_take(1002) && s.can_query(1002));
        for key in [1000, 1001, 1003, 1006] {
            assert!(!s.can_take(key) && !s.can_query(key), "key {key}");
        }
        assert!(s.sees_wallet(), "the client builds the tab for it");
        assert!(!standing(0).sees_wallet());
        assert!(!standing(ROLE_DIRECTOR).sees_wallet(), "director alone");
        assert!(!s.can_take(2000), "not a key");
    }

    #[test]
    fn the_ceos_founding_roles_open_the_tab() {
        assert!(standing(CEO_WALLET_ROLES).sees_wallet());
        assert!(standing(CEO_WALLET_ROLES).can_take(1006));
    }

    #[test]
    fn money_is_whole_cents_and_a_debit_cannot_overdraw() {
        assert_eq!(to_cents(1_000_000_000.0), 100_000_000_000);
        assert_eq!(to_cents(0.1 + 0.2), 30);
        assert_eq!(to_isk(12_345), 123.45);
        let mut d = Deltas::default();
        assert_eq!(balance(&d, CORP, 1000), 0);
        assert_eq!(adjust(&mut d, CORP, 1000, 500), Ok(500));
        assert_eq!(
            adjust(&mut d, CORP, 1000, -501),
            Err(Short::Insufficient { have: 500 })
        );
        assert_eq!(
            balance(&d, CORP, 1000),
            500,
            "a refused debit changes nothing"
        );
        assert_eq!(adjust(&mut d, CORP, 1000, -500), Ok(0));
        assert_eq!(adjust(&mut d, CORP, 1007, 1), Err(Short::BadKey));
        assert_eq!(adjust(&mut d, OTHER, 1003, 7), Ok(7));
        assert_eq!(balances(&d, CORP), [0; DIVISIONS]);
        assert_eq!(total(&d, OTHER), 7);
        assert_eq!(totals(&d).get(&OTHER), Some(&7));
    }

    #[test]
    fn a_thousand_small_payments_and_the_withdrawal_leave_nothing() {
        let mut d = Deltas::default();
        let mut sum = 0;
        for i in 1..=1000 {
            sum += i;
            adjust(&mut d, CORP, 1000, i).unwrap();
        }
        assert_eq!(adjust(&mut d, CORP, 1000, -sum), Ok(0));
    }

    #[test]
    fn journal_rows_come_back_by_division_and_month() {
        let mut d = Deltas::default();
        let day = |n: i64| NOW - n * TICKS_PER_DAY;
        let a = record(&mut d, CORP, 1000, day(2), 10, 1, CORP, 0, 500, 500, "");
        let b = record(
            &mut d,
            CORP,
            1001,
            day(1),
            10,
            1,
            CORP,
            0,
            700,
            700,
            "{DESC: \"x\"}",
        );
        let c = record(&mut d, CORP, 1000, day(40), 37, CORP, 2, 3, -100, 400, "");
        record(&mut d, OTHER, 1000, day(1), 10, 1, OTHER, 0, 9, 9, "");
        assert_eq!((a, b, c), (1, 2, 3), "seq counts per corp");
        let recent = rows(&d, CORP, 1000, NOW, None, None);
        assert_eq!(recent.len(), 1, "the 40-day-old row is outside thirty days");
        assert_eq!(recent[0].amount, 500);
        assert_eq!(
            rows(&d, CORP, 1001, NOW, None, None)[0].description,
            "{DESC: \"x\"}"
        );
        let (y, m, _) = crate::ledger::civil_from_filetime(day(40));
        let month = rows(&d, CORP, 1000, NOW, Some(y), Some(m));
        assert!(month.iter().any(|r| r.ref_type == 37));
        assert_eq!(all_rows(&d, OTHER).len(), 1);
        assert!(rows(&d, CORP, 1000, NOW, None, None)[0].date != 0);
    }

    #[test]
    fn a_row_survives_the_world_file_escaping() {
        let mut d = Deltas::default();
        record(
            &mut d, CORP, 1002, NOW, 11, CORP, OTHER, 5, -250, 750, "a\tb\nc",
        );
        let r = &all_rows(&d, CORP)[0];
        assert_eq!(r.description, "a\tb\nc", "escaping is the store's job");
        assert_eq!(
            (r.key, r.ref_type, r.amount, r.balance),
            (1002, 11, -250, 750)
        );
    }

    #[test]
    fn prune_drops_the_old_and_the_excess_but_never_the_newest() {
        let mut d = Deltas::default();
        for i in 0..5 {
            record(
                &mut d,
                CORP,
                1000,
                NOW - (200 - i) * TICKS_PER_DAY,
                10,
                1,
                CORP,
                0,
                1,
                1,
                "",
            );
        }
        record(&mut d, CORP, 1000, NOW, 10, 1, CORP, 0, 1, 6, "");
        assert_eq!(prune(&mut d, CORP, NOW), 5, "five are older than the keep");
        assert_eq!(all_rows(&d, CORP).len(), 1);
        // Everything old: the newest still stays, so the next seq is 7.
        let mut e = Deltas::default();
        record(
            &mut e,
            CORP,
            1000,
            NOW - 300 * TICKS_PER_DAY,
            10,
            1,
            CORP,
            0,
            1,
            1,
            "",
        );
        assert_eq!(prune(&mut e, CORP, NOW), 0);
        assert_eq!(record(&mut e, CORP, 1000, NOW, 10, 1, CORP, 0, 1, 2, ""), 2);
    }

    #[test]
    fn the_sixty_fourth_row_prunes_by_itself() {
        let mut d = Deltas::default();
        record(
            &mut d,
            CORP,
            1000,
            NOW - 300 * TICKS_PER_DAY,
            10,
            1,
            CORP,
            0,
            1,
            1,
            "",
        );
        for _ in 0..63 {
            record(&mut d, CORP, 1000, NOW, 10, 1, CORP, 0, 1, 1, "");
        }
        assert_eq!(all_rows(&d, CORP).len(), 63, "row 1 went when row 64 came");
    }

    #[test]
    fn division_names_round_trip_and_default_to_none() {
        let mut d = Deltas::default();
        assert!(names(&d, CORP).iter().all(Option::is_none));
        let mut n: [Option<String>; NAME_COUNT] = Default::default();
        n[0] = Some("Ore".to_owned());
        n[8] = Some("Ammo fund".to_owned());
        set_names(&mut d, CORP, &n);
        assert_eq!(names(&d, CORP), n);
        assert_eq!(wallet_name(&d, CORP, 1001).as_deref(), Some("Ammo fund"));
        assert_eq!(
            wallet_name(&d, CORP, 1000),
            None,
            "the Master Wallet has none"
        );
        assert_eq!(wallet_name(&d, OTHER, 1001), None);
    }

    #[test]
    fn a_division_name_is_checked_with_the_clients_keys() {
        assert_eq!(
            check_name("  Mining  ", false),
            Ok(Some("Mining".to_owned()))
        );
        assert_eq!(check_name("   ", true), Ok(None), "blank resets");
        assert_eq!(
            check_name(&"x".repeat(51), false),
            Err("CorpDivNameInvalidMaxLength")
        );
        assert_eq!(
            check_name(&"x".repeat(51), true),
            Err("WalletCorpDivNameInvalidMaxLength")
        );
        assert_eq!(check_name(&"x".repeat(50), true), Ok(Some("x".repeat(50))));
        assert_eq!(check_name("a<b", false), Err("CorpDivNameInvalidSomeChar"));
        assert_eq!(
            check_name("a\u{7}b", true),
            Err("WalletCorpDivNameInvalidSomeChar")
        );
        let some = |s: &str| Some(s.to_owned());
        assert!(duplicate(&[some("Ore"), None, some("ore")]));
        assert!(!duplicate(&[some("Ore"), None, None, some("Ice")]));
    }

    #[test]
    fn the_active_division_defaults_to_the_master_wallet() {
        let mut d = Deltas::default();
        assert_eq!(active_key(&d, 90_000_001), MASTER_KEY);
        set_active_key(&mut d, 90_000_001, 1003);
        assert_eq!(active_key(&d, 90_000_001), 1003);
        d.put(KIND_KEY, vec!["90000002".into()], vec!["77".into()]);
        assert_eq!(
            active_key(&d, 90_000_002),
            MASTER_KEY,
            "a bad key is ignored"
        );
    }

    #[test]
    fn the_tax_ids_pair_with_the_payouts_as_the_client_does() {
        assert_eq!(tax_ref(85), Some(92));
        assert_eq!(tax_ref(17), Some(92));
        assert_eq!(tax_ref(33), Some(93));
        assert_eq!(tax_ref(34), Some(94));
        assert_eq!(tax_ref(10), None);
    }

    #[test]
    fn tax_is_a_rate_of_a_payout_over_the_minimum() {
        assert_eq!(tax_for(0.10, 1_000_000.0), 100_000.0);
        assert_eq!(tax_for(0.0, 1_000_000.0), 0.0);
        assert_eq!(tax_for(0.10, MIN_TAXED_PAYOUT - 0.01), 0.0);
        assert_eq!(tax_for(0.10, MIN_TAXED_PAYOUT), MIN_TAXED_PAYOUT / 10.0);
        assert_eq!(tax_for(2.0, 200_000.0), 200_000.0, "clamped to the payout");
        assert_eq!(tax_for(f64::NAN, 200_000.0), 0.0);
        assert_eq!(tax_for(0.333, 123_456.78), 41_111.11);
    }

    #[test]
    fn a_stamp_merges_into_the_payouts_own_description() {
        assert_eq!(
            stamp("", 92, CORP, 100_000.0),
            "{92: [98000777, 100000.00]}"
        );
        assert_eq!(
            stamp("{NBL: {599011: 3}}", 92, CORP, 12.5),
            "{92: [98000777, 12.50], NBL: {599011: 3}}"
        );
        assert_eq!(stamp("{}", 93, CORP, 1.0), "{93: [98000777, 1.00]}");
        assert_eq!(stamp("not yaml", 94, CORP, 1.0), "{94: [98000777, 1.00]}");
    }

    #[test]
    fn a_reason_is_cut_to_the_clients_limit() {
        let long = "x".repeat(REASON_MAX_LEN + 500);
        let d = reason_description(&long);
        assert_eq!(d, reason_description(&"x".repeat(REASON_MAX_LEN)));
        assert!(d.len() < REASON_MAX_LEN + 20);
    }

    #[test]
    fn a_reason_is_a_yaml_mapping_or_nothing() {
        assert_eq!(reason_description(""), "");
        assert_eq!(reason_description("  "), "");
        assert_eq!(reason_description("ammo"), "{DESC: \"ammo\"}");
        assert_eq!(
            reason_description("say \"hi\" \\ there\n"),
            "{DESC: \"say \\\"hi\\\" \\\\ there\"}"
        );
    }

    #[test]
    fn the_corp_push_names_the_division_as_the_client_tests_it() {
        use crate::notify::Push;
        let p = CorpAccountChange {
            to: vec![1, 2],
            corporation_id: CORP,
            key: 1002,
            balance: 5.0,
        };
        let (name, args) = p.to_push();
        assert_eq!(name, "OnAccountChange");
        assert_eq!(args[0], PyValue::Bytes(b"cash3".to_vec()));
        assert_eq!(args[1], PyValue::Int(CORP));
        assert_eq!(p.audience(), Some(vec![1, 2]));
    }
}
