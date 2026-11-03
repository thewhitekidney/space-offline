//! **BOT-CORP: what a bot corporation does with its money, its roles and its
//! hangar** (docs/lanes/bot-corp.md). The decision points are in
//! [`crate::corpmind`] (`Plan::Ops` for the CEO, `Plan::Stock` for a builder,
//! trader or courier member); this module is the rules (pure) and the calls
//! (the client's own, on the bot's own connection).
//!
//! - **Tax.** The CEO sets the corporation's ISK tax once, from its traits
//!   ([`tax_rate`], **[A]**), with `corpRegistry.UpdateCorporation`.
//! - **Pay.** A share of the Master Wallet's growth since the last payout goes
//!   back to the active members by career ([`payouts`]), with real
//!   `account.GiveCashFromCorpAccount` transfers, never from below the rent
//!   reserve ([`pay_pool`]).
//! - **Roles.** The CEO grants the builders', traders' and couriers'
//!   division (take and query) and appoints directors from the trusted
//!   members ([`role_changes`]), with `corpRegistry.UpdateMember`. Roles are
//!   only ever added.
//! - **The builders' hangar.** A builder with product in its hangar puts a
//!   share of it in the office's division ([`BUILDER_DIVISION`]); a trader or
//!   courier takes stacks out, sells them into the bids at the builder's own
//!   limit and gives [`SELL_REMIT`] of the proceeds to the Master Wallet.
//!   Nothing is created: it is all the members' own goods.

use crate::corpmind::{Hall, OFFICE_RESERVE_PERIODS};
use crate::mind::{Trait, Traits};
use crate::verbs::{ItemRow, SellLine};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::corp_office as co;
use eve_server::corp_wallet as cw;

/// The CEO looks at the corporation's operations this often, game-days
/// **[A]**: a role or a pay needs no faster. Falsified by a new member that
/// waits a day for its division roles.
pub const OPS_EVERY_DAYS: f64 = 0.5;
/// A member looks at the hangar this often, game-days **[A]**.
pub const STOCK_EVERY_DAYS: f64 = 0.25;
/// Tax rate bounds and weights **[A]**: the client sets no rule (the form
/// takes any 0..100 %), so a bot corp's rate is a base that greed raises and
/// loyalty and sociability lower (a loyal corp can ask less of its members
/// and keep them). Falsified by members leaving a corp for its tax
/// (`[soc]` leave lines naming a rate) or a corp wallet that never pays for
/// its rent.
pub const TAX_MIN: f64 = 0.02;
pub const TAX_MAX: f64 = 0.15;
pub const TAX_BASE: f64 = 0.04;
pub const TAX_GREED: f64 = 0.10;
pub const TAX_LOYALTY: f64 = 0.04;
pub const TAX_DILIGENCE: f64 = 0.02;
/// The share of the Master Wallet's growth paid back to members **[A]**.
/// Falsified by the wallet never reaching the alliance fee (too much) or
/// members quitting for pay (too little).
pub const PAY_SHARE: f64 = 0.5;
/// How often a payout runs, game-days **[A]**.
pub const PAY_EVERY_DAYS: f64 = 1.0;
/// A payout under this is not made (dust), ISK **[A]**.
pub const PAY_MIN: f64 = 10_000.0;
/// A member is active when its last social turn was within this many
/// game-days **[A]**.
pub const ACTIVE_DAYS: f64 = 2.0;
/// The Master Wallet is never paid below this: the rent the CEO's office
/// rule keeps ([`OFFICE_RESERVE_PERIODS`] periods of
/// [`co::OFFICE_RENT_ISK`]).
pub fn pay_reserve() -> f64 {
    co::OFFICE_RENT_ISK * OFFICE_RESERVE_PERIODS
}
/// Directors: one per this many members, at most [`DIRECTORS_MAX`] **[A]**.
pub const DIRECTOR_PER: usize = 6;
pub const DIRECTORS_MAX: usize = 3;
/// A member this loyal (trait) and active may be appointed **[A]**: the
/// client has no trust model; this is the bots' own.
pub const DIRECTOR_LOYALTY: f64 = 0.7;
/// The hangar division builders deliver to: index 1, "division 2".
pub const BUILDER_DIVISION: usize = 1;
/// A builder delivers this share of each product stack to the division
/// **[A]**.
pub const DELIVER_SHARE: f64 = 0.5;
/// The most units a trader takes from the division per turn **[A]**.
pub const TAKE_UNITS: i64 = 500;
/// The share of what a seller of the corp's stock earns that goes to the
/// Master Wallet **[A]**: the seller keeps the rest for its trouble.
pub const SELL_REMIT: f64 = 0.9;
/// Careers that use the builders' division.
pub const HANGAR_CAREERS: [&str; 3] = ["builder", "trader", "courier"];

/// What the CEO keeps between passes (not persisted: a restart reads the
/// wallet again and starts a new payout period, which loses at most one).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpsState {
    /// The tax was set (or found set) by us.
    pub tax_set: bool,
    /// The Master Wallet after the last payout (or the first reading).
    pub mark: Option<f64>,
    pub paid_at: Option<u64>,
}

// ---- the rules (pure) ----------------------------------------------------------------------------

/// **The tax a CEO of these traits asks** (**[A]**): whole percents.
pub fn tax_rate(t: &Traits) -> f64 {
    let raw = TAX_BASE + TAX_GREED * t.get(Trait::Greed)
        - TAX_LOYALTY * t.get(Trait::Loyalty)
        - TAX_LOYALTY * t.get(Trait::Sociability) * 0.5
        + TAX_DILIGENCE * t.get(Trait::Diligence);
    ((raw.clamp(TAX_MIN, TAX_MAX)) * 100.0).round() / 100.0
}

/// How much a career counts in a payout **[A]**: the careers that serve the
/// corporation's stock and supply (builders, traders, couriers) count most;
/// miners and the like next; the combat careers least, since the tax is
/// taken from their bounties already. The client has no pay rule; falsified
/// by a career whose members leave for pay.
pub fn career_weight(career: &str) -> f64 {
    match career {
        "builder" | "trader" | "courier" => 1.5,
        "mine" | "ice" | "pi" | "plex" => 1.0,
        _ => 0.5,
    }
}

/// **What the payout may total**: [`PAY_SHARE`] of the growth, and never
/// more than the wallet holds above the reserve.
pub fn pay_pool(income: f64, balance: f64, reserve: f64) -> f64 {
    (PAY_SHARE * income.max(0.0)).min((balance - reserve).max(0.0))
}

/// **Who gets what**: the pool split by [`career_weight`] over `members`
/// `(id, career)`, sorted by id; a share under [`PAY_MIN`] is left out (its
/// ISK stays in the wallet). Cents are floored.
pub fn payouts(pool: f64, members: &[(i64, String)]) -> Vec<(i64, f64)> {
    let total: f64 = members.iter().map(|(_, c)| career_weight(c)).sum();
    if pool <= 0.0 || total <= 0.0 {
        return Vec::new();
    }
    let mut out: Vec<(i64, f64)> = members
        .iter()
        .map(|(id, c)| {
            let isk = (pool * career_weight(c) / total * 100.0).floor() / 100.0;
            (*id, isk)
        })
        .filter(|(_, isk)| *isk >= PAY_MIN)
        .collect();
    out.sort_by_key(|(id, _)| *id);
    out
}

/// One member as the role rules see it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cand {
    pub id: i64,
    pub career: String,
    pub loyalty: f64,
    pub diligence: f64,
    /// The member's direct `roles` as the server holds them.
    pub roles: i64,
    /// A bot of the hall that has been seen lately: only these are granted.
    pub eligible: bool,
}

/// How many directors a corp of `size` has **[A]**.
pub fn directors_wanted(size: usize) -> usize {
    (size / DIRECTOR_PER).min(DIRECTORS_MAX)
}

/// The take and query roles of the doctrine division (DOC-6): what a
/// fighter needs to draw his kit and the procurement member to stock it.
pub fn kit_roles() -> i64 {
    let d = crate::doctrine_stock::DOCTRINE_DIVISION;
    co::ROLE_HANGAR_TAKE[d] | co::ROLE_HANGAR_QUERY[d]
}

/// The take and query roles of the builders' division and, with it, the
/// doctrine division (DOC-6): the stock the hangar careers tend.
pub fn hangar_roles() -> i64 {
    co::ROLE_HANGAR_TAKE[BUILDER_DIVISION] | co::ROLE_HANGAR_QUERY[BUILDER_DIVISION] | kit_roles()
}

/// **The role grants** `(member, new full roles)`: the builders', traders'
/// and couriers' division, and a director from the most trusted members
/// while the corp has fewer than [`directors_wanted`] (`size` counts the CEO).
/// Roles are only added; the CEO is not in `cands`.
pub fn role_changes(cands: &[Cand], size: usize) -> Vec<(i64, i64)> {
    let have = cands
        .iter()
        .filter(|c| c.roles & co::ROLE_DIRECTOR != 0)
        .count();
    let mut want_more = directors_wanted(size).saturating_sub(have);
    let mut order: Vec<&Cand> = cands
        .iter()
        .filter(|c| c.eligible && c.roles & co::ROLE_DIRECTOR == 0 && c.loyalty >= DIRECTOR_LOYALTY)
        .collect();
    order.sort_by(|a, b| {
        b.loyalty
            .total_cmp(&a.loyalty)
            .then(b.diligence.total_cmp(&a.diligence))
            .then(a.id.cmp(&b.id))
    });
    let mut promoted = Vec::new();
    for c in order {
        if want_more == 0 {
            break;
        }
        promoted.push(c.id);
        want_more -= 1;
    }
    let mut out = Vec::new();
    for c in cands.iter().filter(|c| c.eligible) {
        let mut roles = c.roles;
        if HANGAR_CAREERS.contains(&c.career.as_str()) {
            roles |= hangar_roles();
        } else if crate::doctrine_hall::is_fighter(&c.career) {
            roles |= kit_roles();
        }
        if promoted.contains(&c.id) {
            roles |= co::ROLE_DIRECTOR;
        }
        if roles != c.roles {
            out.push((c.id, roles));
        }
    }
    out.sort_by_key(|(id, _)| *id);
    out
}

/// What a builder puts in the division: [`DELIVER_SHARE`] of each product
/// stack, whole units, none of a stack too small to split.
pub fn deliveries(hangar: &[ItemRow]) -> Vec<(i64, i64)> {
    hangar
        .iter()
        .filter(|r| crate::builder::is_product(r))
        .filter_map(|r| {
            let n = (r.quantity as f64 * DELIVER_SHARE).floor() as i64;
            (n > 0).then_some((r.item_id, n))
        })
        .collect()
}

/// The remittance to the Master Wallet out of `proceeds`.
pub fn remit(proceeds: f64) -> f64 {
    ((proceeds.max(0.0) * SELL_REMIT) * 100.0).floor() / 100.0
}

// ---- reading answers -------------------------------------------------------------------------------

fn text(v: &PyValue) -> Option<String> {
    match v {
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        PyValue::Unicode(s) => Some(s.clone()),
        _ => None,
    }
}

/// A cell of an `eve_rowset::Row` (`Row(header, line)`), or an attribute of
/// a `KeyVal`.
fn cell(v: &PyValue, column: &str) -> Option<PyValue> {
    if let PyValue::ObjectEx { header, .. } = v
        && let PyValue::Tuple(h) = header.as_ref()
        && let Some(PyValue::Tuple(args)) = h.get(1)
        && let (Some(PyValue::List(names) | PyValue::Tuple(names)), Some(PyValue::List(line))) =
            (args.first(), args.get(1))
    {
        let i = names
            .iter()
            .position(|n| text(n).as_deref() == Some(column))?;
        return line.get(i).cloned();
    }
    match crate::attributes(v) {
        PyValue::Dict(entries) => entries
            .iter()
            .find(|(_, k)| text(k).as_deref() == Some(column))
            .map(|(value, _)| value.clone()),
        _ => None,
    }
}

fn int(v: Option<PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(n),
        PyValue::Bool(b) => Some(i64::from(b)),
        _ => None,
    }
}

fn real(v: Option<PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(r) => Some(r),
        PyValue::Int(n) => Some(n as f64),
        _ => None,
    }
}

/// The member rows of a `GetMembersPaged` answer: `(characterID, roles)`.
pub fn member_roles(v: &PyValue) -> Vec<(i64, i64)> {
    let PyValue::ObjectEx { header, .. } = v else {
        return Vec::new();
    };
    let PyValue::Tuple(h) = header.as_ref() else {
        return Vec::new();
    };
    let Some(PyValue::Dict(state)) = h.get(1) else {
        return Vec::new();
    };
    let Some((PyValue::List(rows), _)) = state
        .iter()
        .find(|(_, k)| text(k).as_deref() == Some("collection"))
    else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|r| Some((int(cell(r, "characterID"))?, int(cell(r, "roles"))?)))
        .collect()
}

fn kw(name: &str, value: PyValue) -> (PyValue, PyValue) {
    (value, PyValue::Bytes(name.as_bytes().to_vec()))
}

fn soft<T>(r: Result<T>) -> Result<std::result::Result<T, String>> {
    match r {
        Ok(v) => Ok(Ok(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(Err(e.to_string())),
    }
}

pub(crate) fn corp_call(
    conn: &mut Conn,
    corp: i64,
    method: &str,
    args: Vec<PyValue>,
) -> Result<crate::Answer> {
    conn.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
}

fn master_wallet(conn: &mut Conn) -> Result<Option<f64>> {
    let a = soft(conn.call_kw(
        "account",
        "GetCashBalance",
        vec![PyValue::Int(1)],
        vec![kw("accountKey", PyValue::Int(cw::MASTER_KEY))],
    ))?;
    Ok(a.ok().and_then(|o| real(o.value())))
}

// ---- the CEO's pass -------------------------------------------------------------------------------

/// **The CEO's pass** ([`crate::corpmind::Plan::Ops`]): tax, roles, pay.
pub fn run_ceo(conn: &mut Conn, hall: &Hall, me: i64, corp: i64, now: u64) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let (traits, day, mut ops) = {
        let st = hall.lock();
        (
            st.members.get(&me).map(|m| m.traits),
            st.day,
            st.ops.get(&corp).cloned().unwrap_or_default(),
        )
    };
    let Some(traits) = traits else {
        return Ok(out);
    };

    // 1. The tax, once: the corporation row, then the form's own call.
    if !ops.tax_set {
        let row = soft(corp_call(conn, corp, "GetCorporation", vec![]))?;
        if let Ok(a) = row
            && let Some(v) = a.value
        {
            let want = tax_rate(&traits);
            let now_rate = real(cell(&v, "taxRate")).unwrap_or(0.0);
            if (now_rate - want).abs() < 0.005 {
                ops.tax_set = true;
            } else {
                let desc = cell(&v, "description")
                    .and_then(|d| text(&d))
                    .unwrap_or_default();
                let url = cell(&v, "url").and_then(|d| text(&d)).unwrap_or_default();
                let recruiting = int(cell(&v, "isRecruiting")).unwrap_or(1);
                let lp = real(cell(&v, "loyaltyPointTaxRate")).unwrap_or(0.0);
                let a = corp_call(
                    conn,
                    corp,
                    "UpdateCorporation",
                    vec![
                        PyValue::Unicode(desc),
                        PyValue::Unicode(url),
                        PyValue::Real(want),
                        PyValue::Int(recruiting),
                        PyValue::Real(lp),
                    ],
                )?;
                match a.refusal() {
                    Some(key) => out.push(format!("tax {want}: refused {key}")),
                    None => {
                        ops.tax_set = true;
                        hall.lock().act(format!(
                            "{me} set corp {corp}'s tax to {:.0}%",
                            want * 100.0
                        ));
                        out.push(format!("tax {:.0}%: ok", want * 100.0));
                    }
                }
            }
        }
    }

    // 2. The members: roles first (a grant), then pay.
    let rows = match soft(corp_call(
        conn,
        corp,
        "GetMembersPaged",
        vec![PyValue::Int(1)],
    ))? {
        Ok(a) => a.value.as_ref().map(member_roles).unwrap_or_default(),
        Err(why) => {
            out.push(format!("members: {why}"));
            Vec::new()
        }
    };
    let (cands, active): (Vec<Cand>, Vec<(i64, String)>) = {
        let st = hall.lock();
        let recent = (ACTIVE_DAYS * st.day as f64) as u64;
        let mut cands = Vec::new();
        let mut active = Vec::new();
        for (id, roles) in rows.iter().filter(|(id, _)| *id != me) {
            let m = st.members.get(id).filter(|m| m.corp == corp);
            let seen =
                m.is_some_and(|m| m.last_turn.is_some_and(|t| now.saturating_sub(t) <= recent));
            cands.push(Cand {
                id: *id,
                career: m.map(|m| m.career.clone()).unwrap_or_default(),
                loyalty: m.map_or(0.0, |m| m.traits.get(Trait::Loyalty)),
                diligence: m.map_or(0.0, |m| m.traits.get(Trait::Diligence)),
                roles: *roles,
                eligible: seen,
            });
            if seen && let Some(m) = m {
                active.push((*id, m.career.clone()));
            }
        }
        (cands, active)
    };
    for (id, roles) in role_changes(&cands, cands.len() + 1) {
        let mut args = vec![
            PyValue::Int(id),
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::Int(roles),
        ];
        args.extend(std::iter::repeat_n(PyValue::None, 10));
        let a = corp_call(conn, corp, "UpdateMember", args)?;
        match a.refusal() {
            Some(key) => out.push(format!("roles of {id}: refused {key}")),
            None => {
                let director = roles & co::ROLE_DIRECTOR != 0;
                hall.lock().act(format!(
                    "{me} granted {id} roles in corp {corp}{}",
                    if director { " (director)" } else { "" }
                ));
                out.push(format!("roles of {id} -> {roles}: ok"));
            }
        }
    }

    // 3. Pay: a share of the wallet's growth, by career.
    let Some(balance) = master_wallet(conn)? else {
        hall.lock().ops.insert(corp, ops);
        return Ok(out);
    };
    let due = ops
        .paid_at
        .is_none_or(|t| now.saturating_sub(t) >= (PAY_EVERY_DAYS * day as f64) as u64);
    match ops.mark {
        None => ops.mark = Some(balance),
        Some(mark) if due => {
            let pool = pay_pool(balance - mark, balance, pay_reserve());
            let mut paid = 0.0;
            for (id, isk) in payouts(pool, &active) {
                let a = conn.call_kw(
                    "account",
                    "GiveCashFromCorpAccount",
                    vec![
                        PyValue::Int(id),
                        PyValue::Real(isk),
                        PyValue::Int(cw::MASTER_KEY),
                    ],
                    vec![kw("reason", PyValue::Unicode("Corp pay".to_owned()))],
                );
                match soft(a)? {
                    Ok(o) if o.refusal().is_none() => {
                        paid += isk;
                        hall.lock()
                            .act(format!("{me} paid {id} {isk:.0} ISK from corp {corp}"));
                        out.push(format!("pay {id} {isk:.0}: ok"));
                    }
                    Ok(o) => out.push(format!("pay {id}: refused {:?}", o.refusal())),
                    Err(why) => out.push(format!("pay {id}: {why}")),
                }
            }
            ops.paid_at = Some(now);
            ops.mark = Some((balance - paid).max(0.0));
        }
        Some(mark) => ops.mark = Some(mark.min(balance)),
    }
    hall.lock().ops.insert(corp, ops);
    Ok(out)
}

// ---- a member's pass ------------------------------------------------------------------------------

/// The office of `corp` at the bot's station, if any.
pub fn office_here(conn: &mut Conn) -> Result<Option<(i64, i64)>> {
    let Some(station) = conn.session().get_i64("stationid") else {
        return Ok(None);
    };
    let mine = match soft(conn.call("officeManager", "GetMyCorporationsOffices", vec![]))? {
        Ok(o) => o,
        Err(_) => return Ok(None),
    };
    let Some(PyValue::List(rows)) = mine.value() else {
        return Ok(None);
    };
    Ok(rows.iter().find_map(|r| {
        (int(cell(r, "stationID")) == Some(station))
            .then(|| int(cell(r, "officeID")))
            .flatten()
            .map(|office| (station, office))
    }))
}

/// **A member's pass** ([`crate::corpmind::Plan::Stock`]): a builder
/// delivers, a trader or courier sells.
pub fn run_member(
    conn: &mut Conn,
    hall: &Hall,
    me: i64,
    corp: i64,
    career: &str,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let Some((station, office)) = office_here(conn)? else {
        return Ok(out);
    };
    let flag = co::FLAG_SAG[BUILDER_DIVISION];
    if career == "builder" {
        let hangar = conn.hangar_items()?;
        let give = deliveries(&hangar);
        if give.is_empty() {
            return Ok(out);
        }
        let inv = conn.open_container(office)?;
        for (item, n) in give {
            let a = conn.call_on(
                &inv,
                "Add",
                vec![PyValue::Int(item), PyValue::Int(station)],
                vec![kw("qty", PyValue::Int(n)), kw("flag", PyValue::Int(flag))],
            )?;
            match a.refusal() {
                Some(key) => {
                    out.push(format!("deliver {item}: refused {key}"));
                    break;
                }
                None => {
                    hall.lock()
                        .act(format!("{me} delivered {n} of {item} to corp {corp}"));
                    out.push(format!("deliver {n} of {item}: ok"));
                }
            }
        }
        return Ok(out);
    }
    // A trader or courier: take what a bid will buy, sell it, remit.
    let inv = conn.open_container(office)?;
    let stock: Vec<ItemRow> = conn
        .list(&inv, Some(flag))?
        .into_iter()
        .filter(crate::builder::is_product)
        .collect();
    if stock.is_empty() {
        return Ok(out);
    }
    let prices: std::collections::BTreeMap<i64, f64> =
        conn.adjusted_prices()?.into_iter().collect();
    let before = conn.cash_balance()?;
    let hangar = conn.open_hangar()?;
    let mut taken: Vec<(i64, i64)> = Vec::new();
    let mut room = TAKE_UNITS;
    for row in &stock {
        if room <= 0 {
            break;
        }
        let book = conn.market_orders(row.type_id)?;
        let Some(limit) = crate::builder::sell_limit(prices.get(&row.type_id).copied(), &book)
        else {
            continue;
        };
        if book.best_bid().is_none_or(|b| b.price < limit) {
            continue;
        }
        let n = row.quantity.min(room);
        let a = conn.call_on(
            &hangar,
            "Add",
            vec![PyValue::Int(row.item_id), PyValue::Int(office)],
            vec![
                kw("qty", PyValue::Int(n)),
                kw("flag", PyValue::Int(eve_server::model::FLAG_HANGAR)),
            ],
        )?;
        if let Some(key) = a.refusal() {
            out.push(format!("take {}: refused {key}", row.type_id));
            break;
        }
        room -= n;
        taken.push((row.type_id, n));
    }
    if taken.is_empty() {
        return Ok(out);
    }
    let mut lines = Vec::new();
    for (type_id, mut n) in taken {
        let mut stacks: Vec<ItemRow> = conn
            .hangar_items()?
            .into_iter()
            .filter(|r| r.type_id == type_id && !r.singleton)
            .collect();
        stacks.sort_by_key(|r| std::cmp::Reverse(r.quantity));
        let book = conn.market_orders(type_id)?;
        let Some(limit) = crate::builder::sell_limit(prices.get(&type_id).copied(), &book) else {
            continue;
        };
        for s in stacks {
            if n <= 0 {
                break;
            }
            let q = s.quantity.min(n);
            n -= q;
            lines.push(SellLine {
                station_id: station,
                type_id,
                item_id: s.item_id,
                price: limit,
                quantity: q,
            });
        }
    }
    if !lines.is_empty() {
        conn.sell(&lines, 0, crate::builder::BROKER_FEE)?;
    }
    let proceeds = conn.cash_balance()? - before;
    let share = remit(proceeds);
    if share > 0.0 {
        let a = conn.call_kw(
            "account",
            "GiveCash",
            vec![
                PyValue::Int(corp),
                PyValue::Real(share),
                PyValue::Unicode("Stock sold".to_owned()),
            ],
            vec![kw("toAccountKey", PyValue::Int(cw::MASTER_KEY))],
        );
        match soft(a)? {
            Ok(o) if o.refusal().is_none() => {
                hall.lock().act(format!(
                    "{me} sold corp {corp}'s stock and remitted {share:.0} ISK"
                ));
                out.push(format!("sold stock, remitted {share:.0}: ok"));
            }
            Ok(o) => out.push(format!("remit: refused {:?}", o.refusal())),
            Err(why) => out.push(format!("remit: {why}")),
        }
    } else {
        out.push("sold stock: nothing earned".to_owned());
    }
    Ok(out)
}

/// Is `career` one that has a pass of its own ([`run_member`])?
pub fn has_stock_pass(career: &str) -> bool {
    HANGAR_CAREERS.contains(&career)
}

/// The cadence test for the hall's two timers (pure): `last` and `now` in
/// seconds, `days` of `day` seconds each.
pub fn due(last: Option<u64>, now: u64, days: f64, day: u64) -> bool {
    last.is_none_or(|t| now.saturating_sub(t) >= (days * day as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mind::TRAIT_COUNT;

    fn traits(set: &[(Trait, f64)]) -> Traits {
        let mut t = Traits([0.5; TRAIT_COUNT]);
        for (k, v) in set {
            t.0[*k as usize] = *v;
        }
        t
    }

    fn cand(id: i64, career: &str, loyalty: f64, roles: i64, eligible: bool) -> Cand {
        Cand {
            id,
            career: career.to_owned(),
            loyalty,
            diligence: 0.5,
            roles,
            eligible,
        }
    }

    #[test]
    fn tax_follows_greed_and_loyalty_inside_its_bounds_in_whole_percents() {
        let greedy = tax_rate(&traits(&[(Trait::Greed, 1.0), (Trait::Loyalty, 0.0)]));
        let loyal = tax_rate(&traits(&[(Trait::Greed, 0.0), (Trait::Loyalty, 1.0)]));
        assert!(greedy > loyal);
        for r in [greedy, loyal] {
            assert!((TAX_MIN..=TAX_MAX).contains(&r));
            assert!(((r * 100.0).round() - r * 100.0).abs() < 1e-9);
        }
        assert_eq!(
            tax_rate(&traits(&[(Trait::Greed, 5.0), (Trait::Loyalty, -5.0)])),
            TAX_MAX
        );
    }

    #[test]
    fn the_pool_is_a_share_of_the_growth_and_never_dips_into_the_reserve() {
        assert_eq!(
            pay_pool(1_000_000.0, 5_000_000.0, 200_000.0),
            PAY_SHARE * 1_000_000.0
        );
        assert_eq!(pay_pool(1_000_000.0, 300_000.0, 200_000.0), 100_000.0);
        assert_eq!(pay_pool(1_000_000.0, 100_000.0, 200_000.0), 0.0);
        assert_eq!(pay_pool(-5.0, 9e9, 0.0), 0.0);
    }

    #[test]
    fn a_payout_splits_by_career_weight_and_skips_dust() {
        let members = vec![
            (3, "hunter".to_owned()),
            (1, "builder".to_owned()),
            (2, "mine".to_owned()),
        ];
        let pay = payouts(300_000.0, &members);
        assert_eq!(pay.iter().map(|p| p.0).collect::<Vec<_>>(), vec![1, 2, 3]);
        let sum: f64 = pay.iter().map(|p| p.1).sum();
        assert!(sum <= 300_000.0 && sum > 299_990.0);
        assert!(pay[0].1 > pay[1].1 && pay[1].1 > pay[2].1);
        // A pool too small for anyone pays nobody.
        assert!(payouts(PAY_MIN, &members).is_empty());
        assert!(payouts(0.0, &members).is_empty());
        assert!(payouts(1e6, &[]).is_empty());
    }

    #[test]
    fn hangar_careers_get_the_division_and_the_most_trusted_become_directors() {
        let cands = vec![
            cand(1, "builder", 0.1, 0, true),
            cand(2, "mine", 0.9, 0, true),
            cand(3, "hunter", 0.95, 0, true),
            cand(4, "trader", 0.99, 0, false),
            cand(5, "courier", 0.2, hangar_roles(), true),
        ];
        // Six members with the CEO: one director.
        let grants = role_changes(&cands, DIRECTOR_PER);
        let get = |id: i64| grants.iter().find(|g| g.0 == id).map(|g| g.1);
        assert_eq!(get(1), Some(hangar_roles()));
        assert_eq!(
            get(3),
            Some(co::ROLE_DIRECTOR),
            "the most loyal, active one"
        );
        assert_eq!(get(2), None);
        assert_eq!(get(4), None, "not seen lately");
        assert_eq!(get(5), None, "already holds them");
        // A small corp has no director; an existing one fills the seat.
        assert!(
            role_changes(&cands, DIRECTOR_PER - 1)
                .iter()
                .all(|g| g.1 & co::ROLE_DIRECTOR == 0)
        );
        let mut with = cands.clone();
        with[1].roles = co::ROLE_DIRECTOR;
        assert!(
            role_changes(&with, DIRECTOR_PER)
                .iter()
                .all(|g| g.1 & co::ROLE_DIRECTOR == 0)
        );
        // Roles are only added.
        let mut kept = cands;
        kept[0].roles = 1 << 9;
        assert_eq!(
            role_changes(&kept, 1)
                .iter()
                .find(|g| g.0 == 1)
                .map(|g| g.1),
            Some((1 << 9) | hangar_roles())
        );
        assert_eq!(directors_wanted(100), DIRECTORS_MAX);
    }

    #[test]
    fn a_builder_delivers_a_share_of_its_product_whole_units_only() {
        let row = |id: i64, qty: i64, category: i64| ItemRow {
            item_id: id,
            type_id: id + 100,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity: qty,
            group_id: 1,
            category_id: category,
            singleton: false,
        };
        let product = eve_server::econ_exchange::CEILING_CATEGORIES[0];
        let rows = vec![row(1, 10, product), row(2, 1, product), row(3, 10, -1)];
        let give = deliveries(&rows);
        assert_eq!(give, vec![(1, (10.0 * DELIVER_SHARE).floor() as i64)]);
    }

    #[test]
    fn remittance_is_a_share_in_whole_cents_and_never_negative() {
        assert_eq!(remit(1_000.0), 1_000.0 * SELL_REMIT);
        assert_eq!(remit(-5.0), 0.0);
        assert!(has_stock_pass("trader") && !has_stock_pass("hunter"));
        assert!(due(None, 0, 1.0, 100));
        assert!(!due(Some(0), 99, 1.0, 100));
        assert!(due(Some(0), 100, 1.0, 100));
    }

    #[test]
    fn paged_member_rows_are_read_by_column() {
        // `Row(header, line)`: `ObjectEx` with `(class, (names, line))`.
        let row = PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::New,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Tuple(vec![]),
                PyValue::Tuple(vec![
                    PyValue::List(vec![
                        PyValue::Bytes(b"characterID".to_vec()),
                        PyValue::Bytes(b"roles".to_vec()),
                    ]),
                    PyValue::List(vec![PyValue::Int(7), PyValue::Int(1 << 49)]),
                ]),
            ])),
            list: Vec::new(),
            dict: Vec::new(),
        };
        let page = eve_server::pyutil::new_style(
            "eve.common.script.util.pagedCollection.PagedResultSet",
            vec![("collection", PyValue::List(vec![row]))],
        );
        assert_eq!(member_roles(&page), vec![(7, 1 << 49)]);
    }
}
