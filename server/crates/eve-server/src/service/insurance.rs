//! Ship insurance: the premium sink and the payout faucet.
//!
//! W16 packet CP44 (`docs/content/plan.md` §6.2), spec C5 §5.5 and packet
//! E6. Moved out of `service/inventory.rs`, which answered the five methods
//! with empties so the Insurance window could draw.
//!
//! **What is the client's** **[V]** (`eve/common/script/util/
//! insuranceConst.py`, decompiled): the six packages ([`PACKAGES`]), and the
//! premium `full × (fraction − 0.4) × 0.5` ([`premium`]). The client builds
//! its own quote from `GetInsurancePrice(typeID)` alone
//! (`base_insurance.GetQuoteForShip`), so the server serves only `full`
//! and must charge exactly what the client computed from it.
//!
//! **What is ours** **[A]**: `full` is [`FULL_SHARE`] of `P_ref`
//! (`econ_prices::p_ref`, C5 §3.4); a contract runs [`CONTRACT_WEEKS`]; and
//! the anti-fraud voids in [`voided`].
//!
//! **The wire** (`base_insurance.py`, decompiled; `findings/client-calls.tsv`):
//!
//! - `GetContracts()` is iterated and each row read for `.shipID`,
//!   `.ownerID`, `.startDate`, `.endDate`, `.fraction`; `GetContracts(1)`
//!   is the corporation's, asked only with an accountant role;
//! - `GetContractForShip(itemID)` is truth-tested;
//! - `GetInsurancePrice(typeID)` is stored, and a ship whose price is `<= 0`
//!   is left out of the window ("nothing to insure");
//! - `GetInsurancePrices([typeID])` feeds `dict.update`, so a dict;
//! - `InsureShip(itemID, amount, isCorpItem, voidOld=…)`: `amount` is the
//!   premium of the quote line the player ticked. A `UserError
//!   ('InsureShipFailedSingleContract', {'ownerName': ownerID})` makes the
//!   client ask and retry with `voidOld=True`;
//! - `UnInsureShip(itemID)`.
//!
//! **Persistence**: one `insurance` delta row per live contract, keyed by
//! the ship's item id (plan §4.2): `ship_id -> owner type_id fraction
//! start end`. Expired rows are pruned by the clock's `insurance` stage
//! ([`tick`]) and, as a floor, on every `insuranceSvc` call.
//!
//! **The payout** is the [`on_kill`] hook, a `kill_hooks::PLAYER` row
//! (CA15): `fraction × full` of the hull, to the contract's owner, journal
//! ref 19, unless [`voided`]. The contract ends with the ship either way.

use super::*;
use crate::kill_hooks::KillEvent;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["insuranceSvc"];

/// The delta kind this module owns: `ship_id -> owner type_id fraction
/// start end` (plan §4.2).
pub const KIND: &str = "insurance";

/// `BASE_INSURANCE_FRACTION` **[V]** (`insuranceConst.py`).
pub const BASE_FRACTION: f64 = 0.4;

/// `INSURANCE_FRACTION_INCREMENT` **[V]** (`insuranceConst.py`).
pub const FRACTION_INCREMENT: f64 = 0.5;

/// `INSURANCE_PACKAGES`' payout ratios, Basic to Platinum **[V]**
/// (`insuranceConst.py`).
pub const PACKAGES: [f64; 6] = [0.5, 0.6, 0.7, 0.8, 0.9, 1.0];

/// `full = FULL_SHARE × P_ref(hull)`, C5 §5.5 **[A]**: below material
/// cost, so build, insure and self-destruct loses ISK (C5 §6). Falsified by
/// a retail `GetInsurancePrice` far from this share of a hull's price.
pub const FULL_SHARE: f64 = 0.8;

/// What a lost ship pays its owner with no insurance at all, as a share of
/// `full` (#134): **0.4** **[A]**, the client's `BASE_INSURANCE_FRACTION`
/// ([`BASE_FRACTION`]), which there is the premium's offset. The owner
/// decided every loss pays a small amount; the figure is read from that
/// arithmetic. Falsified by a retail uninsured loss, or the owner calling it
/// too high. An insured, unstolen loss pays the policy alone (the base is
/// inside it); a stolen hull splits the policy ([`settle`]).
pub const BASE_PAYOUT_FRACTION: f64 = 0.4;

/// The lowest share of `P_ref` the insurance cap reads the trading index at
/// **[A]** (the doc's old claim, now enforced): a market the index says is
/// worth less than half the reference does not cut a payout further.
/// Falsified by a hull whose build cost sits under this share of `P_ref`.
pub const INDEX_FLOOR: f64 = 0.5;

/// A contract's life, C5 §5.5 **[A]**: falsified by a retail contract
/// whose `endDate - startDate` is another span.
pub const CONTRACT_WEEKS: i64 = 12;

/// [`CONTRACT_WEEKS`] in blue FILETIME units (100 ns).
pub const CONTRACT_LIFETIME: i64 = CONTRACT_WEEKS * 7 * 24 * 3_600 * 10_000_000;

/// `appConst.refInsurance` = 19 **[V]** (`docs/reference/wallet-ledger.md` §5.4).
/// **[I]** the premium books under it too, as `walletEntryFormat`'s
/// refInsurance branch has a wording for a payment "for ship" (a negative
/// reference: the ship's id) beside the payout's "covering loss of" (a
/// positive one: the hull's type).
pub const REF_INSURANCE: i64 = 19;

/// How close an `InsureShip` amount must be to a quote line, relative: the
/// client sends back the float it computed, so only rounding separates them.
const AMOUNT_TOLERANCE: f64 = 1e-6;

/// One live contract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contract {
    pub ship_id: i64,
    pub owner_id: i64,
    pub type_id: i64,
    pub fraction: f64,
    pub start: i64,
    pub end: i64,
}

impl Contract {
    fn from_row(key: &[String], value: &[String]) -> Option<Contract> {
        let [ship] = key else { return None };
        let [owner, type_id, fraction, start, end] = value else {
            return None;
        };
        let fraction: f64 = fraction.parse().ok()?;
        fraction.is_finite().then_some(())?;
        Some(Contract {
            ship_id: ship.parse().ok()?,
            owner_id: owner.parse().ok()?,
            type_id: type_id.parse().ok()?,
            fraction,
            start: start.parse().ok()?,
            end: end.parse().ok()?,
        })
    }

    fn value(&self) -> Vec<String> {
        vec![
            self.owner_id.to_string(),
            self.type_id.to_string(),
            self.fraction.to_string(),
            self.start.to_string(),
            self.end.to_string(),
        ]
    }

    /// The row the window reads by attribute.
    fn to_py(self) -> PyValue {
        crate::pyutil::keyval(vec![
            ("shipID", PyValue::Int(self.ship_id)),
            ("ownerID", PyValue::Int(self.owner_id)),
            ("typeID", PyValue::Int(self.type_id)),
            ("fraction", PyValue::Real(self.fraction)),
            ("startDate", PyValue::Int(self.start)),
            ("endDate", PyValue::Int(self.end)),
        ])
    }
}

/// `GetInsurancePrice(typeID)`: `full`, or `0.0` for an unpriced type --
/// which the window reads as "not insurable" and leaves out.
///
/// **FE-GUARD-A (R-FLUIDECON §5 G3)**: `FULL_SHARE x min(P_ref, index)`, the
/// trading index ([`crate::econ_prices::average`]) and not the static
/// `P_ref` alone, so a payout never exceeds the market-implied value of the
/// hull: build, insure and lose can not profit when the market falls under
/// the reference price. The index itself sits in `econ_prices::CLAMP`
/// (`[0.30, 4.0] x P_ref`); the old text claimed `[0.5, 2]` and a floor of
/// `0.4 x P_ref`. REVIEW-FIX chose to **keep the doc's floor**: the cap reads
/// the index no lower than [`INDEX_FLOOR`] `x P_ref`, so the payout is never
/// below `FULL_SHARE x INDEX_FLOOR x P_ref` (`0.4 x P_ref`) however far a
/// pilot pulls the index down. **[A]** falsified by a payout below the hull's
/// build cost in a normal market (players stop insuring). The premium and
/// the payout read the same figure, so the quote window stays truthful.
pub fn full_price(reg: &Registry, type_id: i64) -> f64 {
    let Some(p) = crate::econ_prices::p_ref(reg, type_id) else {
        return 0.0;
    };
    let index = crate::econ_prices::average(reg, type_id).unwrap_or(p);
    FULL_SHARE * p.min(index.max(INDEX_FLOOR * p))
}

/// `_get_insurance_price_by_fraction` **[V]**: the premium of `fraction`.
pub fn premium(full: f64, fraction: f64) -> f64 {
    full * ((fraction - BASE_FRACTION) * FRACTION_INCREMENT)
}

/// `get_insurance_fractions_and_prices` **[V]**: every package's
/// `(fraction, premium)`, the rows of the quote window.
pub fn quotes(full: f64) -> Vec<(f64, f64)> {
    PACKAGES.iter().map(|&f| (f, premium(full, f))).collect()
}

/// The package `amount` pays for, if it is one of [`quotes`]'s lines.
pub fn package_for(full: f64, amount: f64) -> Option<f64> {
    if !(full > 0.0 && amount.is_finite()) {
        return None;
    }
    quotes(full)
        .into_iter()
        .find(|&(_, p)| (p - amount).abs() <= AMOUNT_TOLERANCE * p.abs().max(1.0))
        .map(|(f, _)| f)
}

/// `ship_id`'s contract while it runs at `now`; `None` once it has ended.
pub fn contract(reg: &Registry, ship_id: i64, now: i64) -> Option<Contract> {
    let rows = reg.deltas().of(KIND);
    let value = rows.get(&vec![ship_id.to_string()])?;
    Contract::from_row(&[ship_id.to_string()], value).filter(|c| c.end > now)
}

/// Every running contract `owner_id` holds at `now`.
pub fn contracts_of(reg: &Registry, owner_id: i64, now: i64) -> Vec<Contract> {
    reg.deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| Contract::from_row(k, v))
        .filter(|c| c.owner_id == owner_id && c.end > now)
        .collect()
}

/// Drop every row that has ended by `now`, or that does not parse. An
/// ended contract's owner is told once (notification 54, X-INSURE): the row
/// goes in the same step, so no contract is announced twice. The
/// world clock sweeps ([`tick`]), and every `insuranceSvc` call does too
/// ([`dispatch`]).
pub fn prune(reg: &mut Registry, now: i64) {
    let dead: Vec<(Vec<String>, Option<Contract>)> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let c = Contract::from_row(k, v);
            c.is_none_or(|c| c.end <= now).then(|| (k.clone(), c))
        })
        .collect();
    if dead.is_empty() {
        return;
    }
    let d = reg.deltas_mut();
    for (k, _) in &dead {
        d.del(KIND, k);
    }
    for c in dead.into_iter().filter_map(|(_, c)| c) {
        crate::notifications::insurance_expired(reg, &c);
    }
}

/// The clock's sweep runs when the blue-time second is a multiple of this:
/// a contract that ended is announced within this many seconds, tuning
/// rather than anything the client fixes. The scan is a walk of the
/// `insurance` rows, so it is not run on all four ticks of every second.
pub const SWEEP_EVERY_SECS: i64 = 15;

/// One second in blue FILETIME units (100 ns).
const SECOND: i64 = 10_000_000;

/// **The world clock's insurance stage** (`tick::STAGES`, "insurance"):
/// [`prune`] every [`SWEEP_EVERY_SECS`], so a contract's end is announced
/// (notification 54) when it happens rather than at the next `insuranceSvc`
/// call by anybody. [`dispatch`] still sweeps first, so a read never shows a
/// contract past its end.
pub fn tick(reg: &mut Registry, _now: Instant, ft: crate::damage::BlueTime) {
    if (ft / SECOND) % SWEEP_EVERY_SECS == 0 {
        prune(reg, ft);
    }
}

/// Why an `InsureShip` was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    NoActor,
    NotDocked,
    /// No such item, or not the actor's.
    NotFound,
    NotAShip,
    Unassembled,
    CorpItem,
    /// A structure that does not offer insurance to this pilot.
    NoService(&'static str),
    /// The hull has no price, so no insurer quotes it.
    Unpriced,
    /// `amount` is no package's premium.
    BadAmount,
    /// A running contract exists and `voidOld` was not set.
    AlreadyInsured {
        owner_id: i64,
    },
    Funds {
        required: f64,
        balance: f64,
    },
}

impl Refusal {
    /// The `UserError` the window has a dialog for.
    fn to_error(&self) -> ServiceError {
        let notify = |text: &str| {
            eve_proto::user_error(
                "CustomNotify",
                crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
            )
        };
        ServiceError::Raise(match self {
            Refusal::NoActor => notify("No character is selected."),
            Refusal::NotDocked => notify("Insurance is sold in stations."),
            Refusal::NoService(why) => notify(why),
            Refusal::NotFound | Refusal::NotAShip => {
                eve_proto::user_error("InsCouldNotFindItem", vec![])
            }
            Refusal::Unassembled => eve_proto::user_error("InsCanNotInsureUnassembledShip", vec![]),
            Refusal::CorpItem => notify("This server does not insure corporation ships."),
            Refusal::Unpriced => notify("This hull cannot be insured here."),
            // **[A]** the closest dialog the client has for an amount that
            // matches no quote; falsified by a retail refusal of another key.
            Refusal::BadAmount => eve_proto::user_error("InsPaymentTooSmall", vec![]),
            // `e.args[1]['ownerName']` is passed to `cfg.eveowners.Get`, so
            // an owner id, not a name **[V]** (`base_insurance.Accept`).
            Refusal::AlreadyInsured { owner_id } => eve_proto::user_error(
                "InsureShipFailedSingleContract",
                crate::pyutil::pypairs(vec![("ownerName", PyValue::Int(*owner_id))]),
            ),
            Refusal::Funds { required, balance } => eve_proto::user_error(
                "NotEnoughMoney",
                crate::pyutil::pypairs(vec![
                    ("amount", PyValue::Real(*required)),
                    ("balance", PyValue::Real(*balance)),
                ]),
            ),
        })
    }
}

/// **Sell `actor` a contract on `ship_id`** for `amount`, the premium of one
/// quote line. The premium is debited (journal ref 19, the station's owner
/// as counterparty, the ship as a negative reference) and the contract
/// written; a running one is replaced only with `void_old`, and its premium
/// is not refunded **[A]**.
pub fn insure(
    reg: &mut Registry,
    actor: i64,
    ship_id: i64,
    amount: f64,
    void_old: bool,
    now: i64,
) -> Result<Contract, Refusal> {
    let c = reg.character(actor).ok_or(Refusal::NoActor)?;
    let (balance, station_owner) = (c.balance, c.station_owner_id);
    reg.docked_station(actor).ok_or(Refusal::NotDocked)?;
    if let Some(why) = crate::structure_dock::service_refusal(
        reg,
        actor,
        crate::structure_profile::SERVICE_INSURANCE,
    ) {
        return Err(Refusal::NoService(why));
    }
    let item = reg
        .item(ship_id)
        .filter(|i| i.owner_id == actor)
        .ok_or(Refusal::NotFound)?;
    if item.category_id != crate::model::CATEGORY_SHIP {
        return Err(Refusal::NotAShip);
    }
    if !item.singleton {
        return Err(Refusal::Unassembled);
    }
    let type_id = item.type_id;
    let full = full_price(reg, type_id);
    if full <= 0.0 {
        return Err(Refusal::Unpriced);
    }
    let fraction = package_for(full, amount).ok_or(Refusal::BadAmount)?;
    if let Some(old) = contract(reg, ship_id, now)
        && !void_old
    {
        return Err(Refusal::AlreadyInsured {
            owner_id: old.owner_id,
        });
    }
    let required = premium(full, fraction);
    if balance < required {
        return Err(Refusal::Funds { required, balance });
    }
    reg.charge(actor, required, REF_INSURANCE, station_owner, -ship_id);
    let new = Contract {
        ship_id,
        owner_id: actor,
        type_id,
        fraction,
        start: now,
        // Lane clock: the policy's weeks at the game clock's pace; the end
        // is stored, so a restart at another scale keeps it.
        end: now + crate::game_clock::span(reg, CONTRACT_LIFETIME),
    };
    reg.deltas_mut()
        .put(KIND, vec![ship_id.to_string()], new.value());
    crate::notifications::insurance_issued(reg, &new);
    Ok(new)
}

/// `UnInsureShip`: end `actor`'s own contract on `ship_id`, with no refund
/// **[A]** (falsified by a retail refund on cancel). Someone else's
/// contract is left alone.
pub fn uninsure(reg: &mut Registry, actor: i64, ship_id: i64) {
    if reg
        .deltas()
        .of(KIND)
        .get(&vec![ship_id.to_string()])
        .and_then(|v| Contract::from_row(&[ship_id.to_string()], v))
        .is_some_and(|c| c.owner_id == actor)
    {
        reg.deltas_mut().del(KIND, &[ship_id.to_string()]);
    }
}

/// **Whether a loss pays nothing**, C5 §5.5 **[A]** (retail's anti-fraud
/// rule): the killer is the owner -- by its own ship, or by a drone, whose
/// pilot's owner is its character -- or shares a fleet or a **player**
/// corporation with the owner. Two pilots in one NPC corporation are
/// strangers, as for loot rights (`combat::has_loot_right`). An unknown
/// killer voids nothing. Falsified by a retail payout on any of these.
pub fn voided(reg: &Registry, owner_id: i64, killer: Option<i64>) -> bool {
    let Some(k) = killer.and_then(|k| reg.pilot(k)) else {
        return false;
    };
    if k.owner_id == owner_id {
        return true;
    }
    // Only a character (its own ship or a drone) can share a fleet or a
    // player corporation; an NPC's owner is its corporation.
    if reg.character(k.owner_id).is_none() {
        return false;
    }
    let fleets = reg.fleets();
    if let (Some(a), Some(b)) = (fleets.fleet_of(k.owner_id), fleets.fleet_of(owner_id))
        && a.fleet_id == b.fleet_id
    {
        return true;
    }
    let player_corp = |id: i64| !(1_000_000..=1_999_999).contains(&id);
    reg.character(owner_id)
        .is_some_and(|o| player_corp(o.corporation_id) && o.corporation_id == k.corporation_id)
}

/// **The `kill_hooks::PLAYER` row** (CA15): a ship under contract is lost.
///
/// Fired before the death plan, so the ship is still there. The contract
/// ends with the ship whatever happens; unless [`voided`], its owner is
/// paid `fraction × full` of the hull -- the window's own "estimated payout"
/// (`quote.fraction * insurancePrice`, `base_insurance
/// .ConstructRadioButtons`) **[V]** -- on journal ref 19 with the hull's
/// type as the reference ("covering loss of a …"). A hull another pilot
/// boarded splits the payout ([`settle`], #134).
///
/// The `OnAccountChange` leaves on the clock to the owner, as
/// `npc::pay_bounty`'s does: the kill is inside the world tick, nobody's
/// turn, and whatever was on `pending` before is put back untouched.
pub fn on_kill(w: &mut Registry, ev: &KillEvent) {
    let Some(c) = contract(w, ev.victim_ship, ev.when) else {
        // An ended row is dropped here too, so a dead ship leaves none.
        w.deltas_mut().del(KIND, &[ev.victim_ship.to_string()]);
        pay_uninsured(w, ev);
        return;
    };
    w.deltas_mut().del(KIND, &[ev.victim_ship.to_string()]);
    // CONCORD's kill pays nothing (`eventCrimesInsuranceNoPayoutGCC` [V]).
    if crate::concord::is_concord_kill(w, ev.killer) {
        crate::outln!("[insurance] {} lost to CONCORD: no payout", ev.victim_ship);
        return;
    }
    // A hull another pilot boarded is voided per recipient in `settle`.
    if stolen(w, ev, &c) {
        settle(w, ev, &c, base_payout(w, c.type_id));
        return;
    }
    // A self-destruct is the owner's own kill; whether it pays is one switch
    // (`self_destruct::SELF_DESTRUCT_PAYS_INSURANCE`, an open owner question).
    let void = voided(w, c.owner_id, ev.killer) && !crate::self_destruct::pays_insurance(w, ev);
    if void {
        return;
    }
    settle(w, ev, &c, base_payout(w, c.type_id));
}

/// Whether the owner at death is a character other than the policy holder.
fn stolen(w: &Registry, ev: &KillEvent, c: &Contract) -> bool {
    ev.victim_character != c.owner_id && w.character(ev.victim_character).is_some()
}

/// **An uninsured loss pays the base** (#134): the owner at death, a
/// character, is paid [`base_payout`] on ref 19 and told with type 35,
/// `payout` true ("default payout for an uninsured ship"). The void, CONCORD
/// and self-destruct conditions pay nothing, base included. An NPC, a drone
/// and a hull with no base (a capsule, a rookie ship, an unpriced type) pay
/// nothing.
fn pay_uninsured(w: &mut Registry, ev: &KillEvent) {
    if w.character(ev.victim_character).is_none() || crate::concord::is_concord_kill(w, ev.killer) {
        return;
    }
    if voided(w, ev.victim_character, ev.killer) {
        return;
    }
    let base = base_payout(w, ev.ship_type);
    if base > 0.0 {
        pay_out(w, ev, ev.ship_type, &[(ev.victim_character, base, true)]);
    }
}

/// What an **uninsured** loss of a `type_id` hull pays its owner (#134):
/// `BASE_PAYOUT_FRACTION x full`. Zero for a capsule, a rookie ship (the
/// corvette group) and a type with no `full` value.
pub fn base_payout(w: &Registry, type_id: i64) -> f64 {
    let group = crate::item::find_in(w.content(), type_id).map(|t| t.group_id);
    if matches!(
        group,
        Some(crate::fitting::GROUP_CAPSULE | crate::starter_ship::GROUP_CORVETTE)
    ) {
        return 0.0;
    }
    #[cfg(test)]
    if let Some(f) = tests_base::get() {
        return f * full_price(w, type_id);
    }
    BASE_PAYOUT_FRACTION * full_price(w, type_id)
}

/// A test's stand-in for the base share, per test thread, so the split can be
/// driven at a figure of its own, never the [`BASE_PAYOUT_FRACTION`] [A] value.
#[cfg(test)]
pub(crate) mod tests_base {
    use std::cell::Cell;
    thread_local!(static BASE: Cell<Option<f64>> = const { Cell::new(None) });
    pub fn get() -> Option<f64> {
        BASE.with(Cell::get)
    }
    pub fn set(f: Option<f64>) {
        BASE.with(|c| c.set(f));
    }
}

/// **A stolen hull's loss splits the policy** (#134). `ev.victim_character`
/// is the hull's owner at death; the contract's `owner_id` is the policy
/// holder. When they are the same the holder is paid `fraction x full` as
/// ever. When another pilot boarded the hull, the owner at death gets `base`
/// (what an uninsured loss pays) and the holder `fraction x full - base`,
/// floored at 0, so the two sum to the policy. A holder who is deleted loses
/// only their share; one who is offline is paid and told like any other.
/// `fw_lp::note_insured` is given the total actually paid.
fn settle(w: &mut Registry, ev: &KillEvent, c: &Contract, base: f64) {
    let policy = c.fraction * full_price(w, c.type_id);
    let split = stolen(w, ev, c);
    let (holder, owner) = if split {
        ((policy - base).max(0.0), base.max(0.0))
    } else {
        (policy, 0.0)
    };
    // Each share is voided against its own recipient. The holder's is also
    // void when the owner at death (or his drone) is the killer -- a
    // self-destruct or a thief shooting his own hull -- unless the
    // self-destruct switch pays. An unstolen hull was voided by `on_kill`.
    let owner_killed = ev
        .killer
        .and_then(|k| w.pilot(k))
        .is_some_and(|k| k.owner_id == ev.victim_character);
    let holder_void = split
        && (voided(w, c.owner_id, ev.killer)
            || (owner_killed && !crate::self_destruct::pays_insurance(w, ev)));
    let owner_void = voided(w, ev.victim_character, ev.killer);
    let mut paid = Vec::new();
    if holder > 0.0 && !holder_void && w.character(c.owner_id).is_some() {
        paid.push((c.owner_id, holder, false));
    }
    if owner > 0.0 && !owner_void {
        paid.push((ev.victim_character, owner, true));
    }
    pay_out(w, ev, c.type_id, &paid);
}

/// Pays each `(to, amount, default)` on ref 19 with its own notification and
/// wallet push, then tells `fw_lp::note_insured` the total, so an LP-earning
/// kill nets out the base as it does a policy.
fn pay_out(w: &mut Registry, ev: &KillEvent, type_id: i64, paid: &[(i64, f64, bool)]) {
    // A hull the store refused to remove is killed again: never paid twice.
    if paid.is_empty() || crate::fw_lp::last_insured_ship(w) == ev.victim_ship {
        return;
    }
    let before = w.take_pending();
    let mut total = 0.0;
    for &(to, amount, default) in paid {
        w.charge(to, -amount, REF_INSURANCE, to, type_id);
        if default {
            crate::notifications::insurance_default_payout(w, to, ev.victim_ship, amount);
        } else {
            crate::notifications::insurance_payout(w, to, ev.victim_ship, amount);
        }
        total += amount;
        // Each recipient's wallet change goes to that recipient alone.
        for (_, p) in w.take_pending() {
            w.schedule(
                std::time::Duration::ZERO,
                crate::notify::Addressed {
                    to: Some(vec![to]),
                    inner: p,
                },
            );
        }
    }
    crate::fw_lp::note_insured(w, ev.victim_ship, total);
    for (after, p) in before {
        w.push_later(after, crate::notify::Addressed { to: None, inner: p });
    }
}

/// An integer positional argument.
fn int_arg(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// A truthy argument: `True`, or a non-zero integer.
fn truthy(v: Option<&PyValue>) -> bool {
    matches!(v, Some(PyValue::Bool(true))) || matches!(v, Some(PyValue::Int(n)) if *n != 0)
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let now = crate::pyutil::filetime_now();
    if req.service == "insuranceSvc" {
        prune(reg, now);
    }
    match req.service {
        "insuranceSvc" => match req.method {
            // Iterated (`base_insurance.GetContracts`), so a list. `(1)` is
            // the corporation's, and no corporation insures anything here.
            "GetContracts" => {
                let Some(actor) = req.actor else {
                    return Ok(PyValue::List(vec![]));
                };
                if truthy(req.args.first()) {
                    return Ok(PyValue::List(vec![]));
                }
                Ok(PyValue::List(
                    contracts_of(reg, actor, now)
                        .into_iter()
                        .map(Contract::to_py)
                        .collect(),
                ))
            }
            // Truth-tested: `None` means "no contract".
            "GetContractForShip" => Ok(int_arg(req.args.first())
                .and_then(|ship| contract(reg, ship, now))
                .map_or(PyValue::None, Contract::to_py)),
            // Stored as a number and formatted as ISK; `0.0` with no type.
            "GetInsurancePrice" => Ok(PyValue::Real(
                int_arg(req.args.first()).map_or(0.0, |t| full_price(reg, t)),
            )),
            // `self.insurancePrice.update(...)`: `{typeID: full}`. A
            // `PyValue::Dict` entry is `(value, key)`, the wire's order
            // (`pyutil::pairs_get`).
            "GetInsurancePrices" => {
                let types = match req.args.first() {
                    Some(PyValue::List(v) | PyValue::Tuple(v)) => {
                        v.iter().filter_map(|t| int_arg(Some(t))).collect()
                    }
                    _ => Vec::new(),
                };
                Ok(PyValue::Dict(
                    types
                        .into_iter()
                        .map(|t| (PyValue::Real(full_price(reg, t)), PyValue::Int(t)))
                        .collect(),
                ))
            }
            "InsureShip" => {
                let actor = req
                    .actor
                    .ok_or(Refusal::NoActor)
                    .map_err(|r| r.to_error())?;
                let ship = int_arg(req.args.first())
                    .ok_or(Refusal::NotFound)
                    .map_err(|r| r.to_error())?;
                let amount = match req.args.get(1) {
                    Some(PyValue::Real(r)) => *r,
                    Some(PyValue::Int(n)) => *n as f64,
                    _ => f64::NAN,
                };
                if truthy(req.args.get(2)) {
                    return Err(Refusal::CorpItem.to_error());
                }
                let void_old = truthy(crate::space_state::kwarg(req.kwargs, "voidOld"))
                    || truthy(req.args.get(3));
                insure(reg, actor, ship, amount, void_old, now).map_err(|r| r.to_error())?;
                Ok(PyValue::None)
            }
            "UnInsureShip" => {
                if let (Some(actor), Some(ship)) = (req.actor, int_arg(req.args.first())) {
                    uninsure(reg, actor, ship);
                }
                Ok(PyValue::None)
            }
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` gates the call. An error, not a panic.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::{Damage, Shot};

    const ALICE: i64 = 90_000_101;
    const BOB: i64 = 90_000_102;
    const ALICE_SHIP: i64 = 100_000_101;
    const BOB_SHIP: i64 = 100_000_102;
    const RIFTER: i64 = 587;
    /// An invented hull price; a test asserts code against itself, never it.
    const INVENTED_PRICE: f64 = 100_000.0;

    /// Alice and Bob docked, each in a Rifter that is a real item, with an
    /// invented price for the hull and ISK to spend.
    fn docked() -> Registry {
        let pilot = |id: i64, name: &str, ship: i64| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: RIFTER,
            balance: 10.0 * INVENTED_PRICE,
            ..crate::model::Character::default()
        };
        let mut content = crate::content::Content::fixture();
        let prices = crate::econ_prices::Table::parse(&[format!(
            "{RIFTER}\t{INVENTED_PRICE}\trecipe\t{}\t6\t25\t1\t1",
            INVENTED_PRICE / 2.0
        )])
        .expect("parses");
        content.insert("econ_prices", prices);
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(ALICE, "Alice", ALICE_SHIP),
                pilot(BOB, "Bob", BOB_SHIP),
            ];
        })
        .with_content(std::sync::Arc::new(content));
        let rifter = *crate::item::find_in(reg.content(), RIFTER).expect("Rifter is spawnable");
        let mut items = reg.store().items().to_vec();
        for i in items
            .iter_mut()
            .filter(|i| i.item_id == ALICE_SHIP || i.item_id == BOB_SHIP)
        {
            i.type_id = rifter.type_id;
            i.group_id = rifter.group_id;
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: BOB_SHIP + 1,
            ..crate::store::Snapshot::default()
        });
        let _ = reg.take_pending();
        reg
    }

    fn call(
        reg: &mut Registry,
        actor: i64,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "insuranceSvc",
                method,
                args,
                kwargs,
                bind_params: None,
                actor: Some(actor),
            },
        )
    }

    fn raised_key(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("not a raise: {e:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("header")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("args")
        };
        String::from_utf8_lossy(crate::pyutil::as_bytes(&args[0]).expect("key")).into_owned()
    }

    fn balance(reg: &Registry, who: i64) -> f64 {
        reg.character(who).expect("character").balance
    }

    /// **The quote is the client's own formula** [V]: for every package,
    /// `full × (fraction − 0.4) × 0.5` with `full` what `GetInsurancePrice`
    /// answered, transcribed here from `insuranceConst.py`; and insuring at
    /// one of those amounts debits exactly it and records that fraction.
    #[test]
    fn the_quote_equals_the_clients_premium_formula() {
        let mut reg = docked();
        let PyValue::Real(full) = call(
            &mut reg,
            ALICE,
            "GetInsurancePrice",
            &[PyValue::Int(RIFTER)],
            &[],
        )
        .unwrap() else {
            panic!("a float")
        };
        assert!(full > 0.0);
        let client = |fraction: f64| full * ((fraction - 0.4) * 0.5);
        let fractions = [0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
        assert_eq!(
            quotes(full),
            fractions
                .iter()
                .map(|&f| (f, client(f)))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            call(
                &mut reg,
                ALICE,
                "GetInsurancePrices",
                &[PyValue::List(vec![PyValue::Int(RIFTER)])],
                &[]
            )
            .unwrap(),
            PyValue::Dict(vec![(PyValue::Real(full), PyValue::Int(RIFTER))])
        );

        let before = balance(&reg, ALICE);
        let amount = client(0.7);
        let r = call(
            &mut reg,
            ALICE,
            "InsureShip",
            &[
                PyValue::Int(ALICE_SHIP),
                PyValue::Real(amount),
                PyValue::Bool(false),
            ],
            &[],
        );
        assert_eq!(r.unwrap(), PyValue::None);
        assert_eq!(balance(&reg, ALICE), before - amount);
        let c = contract(&reg, ALICE_SHIP, crate::pyutil::filetime_now()).expect("insured");
        assert_eq!((c.owner_id, c.fraction, c.type_id), (ALICE, 0.7, RIFTER));
        let PyValue::List(rows) = call(&mut reg, ALICE, "GetContracts", &[], &[]).unwrap() else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 1);

        // An amount on no quote line is refused, and charges nothing.
        let before = balance(&reg, BOB);
        let e = call(
            &mut reg,
            BOB,
            "InsureShip",
            &[PyValue::Int(BOB_SHIP), PyValue::Real(amount + 1.0)],
            &[],
        )
        .unwrap_err();
        assert_eq!(raised_key(&e), "InsPaymentTooSmall");
        assert_eq!(balance(&reg, BOB), before);

        // A second contract asks first, then replaces with `voidOld`.
        let e = call(
            &mut reg,
            ALICE,
            "InsureShip",
            &[PyValue::Int(ALICE_SHIP), PyValue::Real(client(1.0))],
            &[],
        )
        .unwrap_err();
        assert_eq!(raised_key(&e), "InsureShipFailedSingleContract");
        call(
            &mut reg,
            ALICE,
            "InsureShip",
            &[PyValue::Int(ALICE_SHIP), PyValue::Real(client(1.0))],
            // `(value, key)`, the wire's order (`pyutil::pairs_get`).
            &[(PyValue::Bool(true), crate::pyutil::bstr("voidOld"))],
        )
        .unwrap();
        let now = crate::pyutil::filetime_now();
        assert_eq!(
            contract(&reg, ALICE_SHIP, now).map(|c| c.fraction),
            Some(1.0)
        );
    }

    /// `Character::station_id` stays set on undock, so the docked gate asks
    /// `Registry::docked_station`: no insuring from space
    /// (`docs/lanes/stale-system.md`). **[A]** retail refuses it undocked.
    #[test]
    fn insuring_needs_the_pilot_to_be_docked() {
        let mut reg = docked();
        let PyValue::Real(full) = call(
            &mut reg,
            ALICE,
            "GetInsurancePrice",
            &[PyValue::Int(RIFTER)],
            &[],
        )
        .unwrap() else {
            panic!("a float")
        };
        let amount = full * ((0.7 - 0.4) * 0.5);
        undock_both(&mut reg);
        let before = balance(&reg, ALICE);
        let args = [PyValue::Int(ALICE_SHIP), PyValue::Real(amount)];
        assert!(call(&mut reg, ALICE, "InsureShip", &args, &[]).is_err());
        assert_eq!(balance(&reg, ALICE), before, "nothing charged");
        assert!(contract(&reg, ALICE_SHIP, crate::pyutil::filetime_now()).is_none());
    }

    /// Put both pilots in space, one system, close together.
    fn undock_both(reg: &mut Registry) {
        crate::destiny::on_ballpark_bind(reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(reg, Some(BOB));
        let _ = reg.take_pending();
    }

    /// One overwhelming volley from roster key `shooter` at Alice's ship.
    fn kill_alice_by(reg: &mut Registry, shooter: i64) {
        let t0 = crate::pyutil::filetime_now();
        let v = reg.roster().get(ALICE).copied().expect("Alice in space");
        let from = reg.roster().get(shooter).map_or(0, |p| p.ship_id);
        let shot = Shot {
            at: t0,
            attacker_ship_id: from,
            weapon_item_id: 1,
            weapon_type_id: 3640,
            target_ship_id: v.ship_id,
            damage: Damage {
                em: 1.0e9,
                explosive: 1.0e9,
                kinetic: 1.0e9,
                thermal: 1.0e9,
            },
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(reg, shot, shooter, v, t0);
        assert_ne!(
            reg.roster().get(ALICE).map(|p| p.ship_id),
            Some(ALICE_SHIP),
            "setup: Alice's ship died"
        );
    }

    /// Insure Alice's ship at `fraction` and answer what the payout would be.
    fn insure_alice(reg: &mut Registry, fraction: f64) -> f64 {
        let full = full_price(reg, RIFTER);
        let now = crate::pyutil::filetime_now();
        insure(reg, ALICE, ALICE_SHIP, premium(full, fraction), false, now).expect("insured");
        fraction * full
    }

    /// **An NPC kill pays** `fraction × full` of the hull to the owner, on
    /// ref 19 with the hull as the reference, once, and the contract ends
    /// with the ship. Fired through the real `kill_hooks::PLAYER` table.
    #[test]
    fn an_npc_kill_pays_the_owner() {
        let mut reg = docked();
        let payout = insure_alice(&mut reg, 0.9);
        undock_both(&mut reg);
        let system = reg.roster().get(ALICE).expect("in space").solar_system_id;
        let at = reg.roster().get(ALICE).expect("in space").motion.position;
        let npc = crate::npc::spawn(
            &mut reg,
            &crate::npc::ANGEL_HIJACKER,
            system,
            [at[0] + 3_000.0, at[1], at[2]],
        )
        .expect("an id");
        let before = balance(&reg, ALICE);
        kill_alice_by(&mut reg, npc);
        assert!(payout > 0.0);
        assert_eq!(balance(&reg, ALICE), before + payout);
        let row = reg
            .journal()
            .entries()
            .iter()
            .rev()
            .find(|e| e.entry_type_id == REF_INSURANCE)
            .expect("a ref-19 row");
        assert_eq!((row.amount, row.reference_id), (payout, RIFTER));
        assert_eq!(row.wallet(), ALICE);
        assert!(reg.deltas().of(KIND).is_empty(), "the contract ended");
        assert!(
            crate::kill_hooks::PLAYER
                .iter()
                .any(|r| r.name == "insurance_on_kill"),
            "the payout is a kill_hooks::PLAYER row"
        );
    }

    /// A loss of `ship` (type Rifter) whose owner at death is `victim`.
    fn loss(victim: i64, ship: i64) -> KillEvent {
        KillEvent {
            victim_character: victim,
            victim_ship: ship,
            ship_type: RIFTER,
            killer: None,
            system: 0,
            npc: None,
            when: crate::pyutil::filetime_now(),
            park: vec![],
        }
    }

    /// A contract held by `holder` on `ship` at `fraction`, as `insure` writes it.
    fn policy(holder: i64, ship: i64, fraction: f64) -> Contract {
        let now = crate::pyutil::filetime_now();
        Contract {
            ship_id: ship,
            owner_id: holder,
            type_id: RIFTER,
            fraction,
            start: now,
            end: now + CONTRACT_LIFETIME,
        }
    }

    fn ref19(reg: &Registry, who: i64) -> Vec<f64> {
        reg.journal()
            .entries()
            .iter()
            .filter(|e| e.entry_type_id == REF_INSURANCE && e.wallet() == who)
            .map(|e| e.amount)
            .collect()
    }

    /// **A stolen hull splits the policy** (#134): the owner at death is paid
    /// the base, the holder the rest, and the two sum to the policy.
    #[test]
    fn a_stolen_hull_splits_the_policy_and_the_parts_sum_to_it() {
        let mut reg = docked();
        let full = full_price(&reg, RIFTER);
        let (policy_total, base) = (0.9 * full, 0.3 * full);
        let (a0, b0) = (balance(&reg, ALICE), balance(&reg, BOB));
        settle(
            &mut reg,
            &loss(BOB, ALICE_SHIP),
            &policy(ALICE, ALICE_SHIP, 0.9),
            base,
        );
        assert_eq!(balance(&reg, ALICE) - a0, policy_total - base);
        assert_eq!(balance(&reg, BOB) - b0, base);
        assert_eq!(ref19(&reg, ALICE), vec![policy_total - base]);
        assert_eq!(ref19(&reg, BOB), vec![base]);
        let paid = (balance(&reg, ALICE) - a0) + (balance(&reg, BOB) - b0);
        assert!(
            (paid - policy_total).abs() < 1e-6,
            "the parts sum to the policy"
        );
    }

    /// **An unstolen hull pays as before**: the holder, all of it, and the
    /// base is not paid to anyone.
    #[test]
    fn an_unstolen_hull_pays_the_holder_all_of_it() {
        let mut reg = docked();
        let full = full_price(&reg, RIFTER);
        let (a0, b0) = (balance(&reg, ALICE), balance(&reg, BOB));
        settle(
            &mut reg,
            &loss(ALICE, ALICE_SHIP),
            &policy(ALICE, ALICE_SHIP, 0.9),
            0.3 * full,
        );
        assert_eq!(balance(&reg, ALICE) - a0, 0.9 * full);
        assert_eq!(balance(&reg, BOB), b0);
    }

    /// The holder's share is floored at 0; the owner still gets the base.
    #[test]
    fn a_base_above_the_policy_leaves_the_holder_nothing() {
        let mut reg = docked();
        let full = full_price(&reg, RIFTER);
        let (a0, b0) = (balance(&reg, ALICE), balance(&reg, BOB));
        settle(
            &mut reg,
            &loss(BOB, ALICE_SHIP),
            &policy(ALICE, ALICE_SHIP, 0.5),
            0.7 * full,
        );
        assert_eq!(balance(&reg, ALICE), a0);
        assert_eq!(balance(&reg, BOB) - b0, 0.7 * full);
    }

    /// A holder who no longer exists loses their share; the owner's is paid.
    #[test]
    fn a_deleted_holder_leaves_the_owners_share_paid() {
        let mut reg = docked();
        let full = full_price(&reg, RIFTER);
        let b0 = balance(&reg, BOB);
        let gone = 990_001;
        assert!(reg.character(gone).is_none());
        settle(
            &mut reg,
            &loss(BOB, ALICE_SHIP),
            &policy(gone, ALICE_SHIP, 0.9),
            0.3 * full,
        );
        assert_eq!(balance(&reg, BOB) - b0, 0.3 * full);
        assert!(ref19(&reg, gone).is_empty());
    }

    /// With a zero base a stolen hull's holder is paid everything; the owner
    /// gets nothing.
    #[test]
    fn with_no_base_the_holder_is_paid_everything() {
        let mut reg = docked();
        let full = full_price(&reg, RIFTER);
        let b0 = balance(&reg, BOB);
        let a0 = balance(&reg, ALICE);
        tests_base::set(Some(0.0));
        let base = base_payout(&reg, RIFTER);
        assert_eq!(base, 0.0);
        settle(
            &mut reg,
            &loss(BOB, ALICE_SHIP),
            &policy(ALICE, ALICE_SHIP, 0.9),
            base,
        );
        assert_eq!(balance(&reg, ALICE) - a0, 0.9 * full);
        assert_eq!(balance(&reg, BOB), b0);
    }

    fn told(reg: &Registry, who: i64) -> usize {
        crate::notifications::unread(reg.deltas(), who)
            .iter()
            .filter(|n| n.type_id == crate::notifications::kind::INSURANCE_PAYOUT)
            .count()
    }

    /// **Voids are per recipient** (#134). Bob boarded Alice's insured hull.
    /// Alice kills it: she is the killer, so her share is void, but Bob, a
    /// stranger to her, still gets the base.
    #[test]
    fn the_holder_killing_a_stolen_hull_still_pays_the_thief_the_base() {
        let mut reg = docked();
        tests_base::set(Some(0.25));
        insure_alice(&mut reg, 0.9);
        let full = full_price(&reg, RIFTER);
        let (a0, b0) = (balance(&reg, ALICE), balance(&reg, BOB));
        let ev = KillEvent {
            killer: Some(ALICE),
            ..loss(BOB, ALICE_SHIP)
        };
        on_kill(&mut reg, &ev);
        assert_eq!(balance(&reg, ALICE), a0, "the holder killed it");
        assert_eq!(balance(&reg, BOB) - b0, 0.25 * full, "the thief's base");
    }

    /// A thief who destroys the hull he boarded -- by self-destruct (the
    /// switch is off by default) or by shooting it -- is paid no base and
    /// the holder no share: neither can be farmed.
    #[test]
    fn a_thief_destroying_his_own_stolen_hull_pays_nobody() {
        let mut reg = docked();
        tests_base::set(Some(0.25));
        insure_alice(&mut reg, 0.9);
        let (a0, b0) = (balance(&reg, ALICE), balance(&reg, BOB));
        let ev = KillEvent {
            killer: Some(BOB),
            ..loss(BOB, ALICE_SHIP)
        };
        on_kill(&mut reg, &ev);
        assert_eq!((balance(&reg, ALICE), balance(&reg, BOB)), (a0, b0));
        assert!(reg.deltas().of(KIND).is_empty(), "the contract ended");
    }

    /// A stranger's kill of the stolen hull splits the policy as before.
    #[test]
    fn a_strangers_kill_of_a_stolen_hull_splits_the_policy() {
        let mut reg = docked();
        tests_base::set(Some(0.25));
        let payout = insure_alice(&mut reg, 0.9);
        let full = full_price(&reg, RIFTER);
        let (a0, b0) = (balance(&reg, ALICE), balance(&reg, BOB));
        on_kill(&mut reg, &loss(BOB, ALICE_SHIP));
        assert_eq!(balance(&reg, ALICE) - a0, payout - 0.25 * full);
        assert_eq!(balance(&reg, BOB) - b0, 0.25 * full);
    }

    /// The same hull killed twice (the store refused its removal the first
    /// time) is paid once.
    #[test]
    fn one_hull_is_paid_once() {
        let mut reg = docked();
        tests_base::set(Some(0.25));
        let a0 = balance(&reg, ALICE);
        on_kill(&mut reg, &loss(ALICE, ALICE_SHIP));
        let once = balance(&reg, ALICE);
        assert!(once > a0, "setup: paid");
        on_kill(&mut reg, &loss(ALICE, ALICE_SHIP));
        assert_eq!(balance(&reg, ALICE), once, "no second base");
        assert_eq!(told(&reg, ALICE), 1);
    }

    /// **An uninsured loss pays the base** (#134): ref 19 to the owner, one
    /// type-35 note, the base share of `full`.
    #[test]
    fn an_uninsured_loss_pays_the_base_on_ref_19() {
        let mut reg = docked();
        tests_base::set(Some(0.25));
        let full = full_price(&reg, RIFTER);
        let a0 = balance(&reg, ALICE);
        on_kill(&mut reg, &loss(ALICE, ALICE_SHIP));
        assert_eq!(balance(&reg, ALICE) - a0, 0.25 * full);
        assert_eq!(ref19(&reg, ALICE), vec![0.25 * full]);
        assert_eq!(told(&reg, ALICE), 1, "told once");
    }

    /// An insured, unstolen loss pays the policy and not the base on top.
    #[test]
    fn an_insured_loss_pays_the_policy_without_the_base() {
        let mut reg = docked();
        tests_base::set(Some(0.25));
        let payout = insure_alice(&mut reg, 0.9);
        let a0 = balance(&reg, ALICE);
        on_kill(&mut reg, &loss(ALICE, ALICE_SHIP));
        assert_eq!(balance(&reg, ALICE) - a0, payout);
        // The premium is the other ref-19 row (a charge); one payout, no base.
        let paid: Vec<f64> = ref19(&reg, ALICE)
            .into_iter()
            .filter(|a| *a > 0.0)
            .collect();
        assert_eq!(paid, vec![payout]);
    }

    /// A capsule, a rookie ship, an unpriced type and a zero base pay nothing.
    #[test]
    fn a_hull_with_no_base_pays_nothing() {
        let reg = docked();
        tests_base::set(Some(0.25));
        assert!(base_payout(&reg, RIFTER) > 0.0, "setup: a priced hull");
        for group in [
            crate::fitting::GROUP_CAPSULE,
            crate::starter_ship::GROUP_CORVETTE,
        ] {
            let ty = crate::item::SPAWNABLE
                .iter()
                .find(|t| t.group_id == group)
                .map(|t| t.type_id)
                .expect("a spawnable hull of the group");
            assert_eq!(base_payout(&reg, ty), 0.0, "group {group}");
        }
        assert_eq!(base_payout(&reg, 990_123), 0.0, "unpriced");
        tests_base::set(Some(0.0));
        assert_eq!(base_payout(&reg, RIFTER), 0.0);
    }

    /// CONCORD's kill, a self kill and a fleet-mate's kill pay no base; a
    /// stranger's, an NPC's and an unknown killer's do; an NPC victim never.
    #[test]
    fn the_void_conditions_pay_no_base() {
        tests_base::set(Some(0.25));
        let mut reg = docked();
        let cop = crate::npc::spawn(
            &mut reg,
            &crate::concord::FIXTURE_COMMANDER,
            990_003,
            [0.0; 3],
        )
        .expect("an id");
        let a0 = balance(&reg, ALICE);
        let by = |k: Option<i64>| KillEvent {
            killer: k,
            ..loss(ALICE, ALICE_SHIP)
        };
        on_kill(&mut reg, &by(Some(cop)));
        on_kill(&mut reg, &by(Some(ALICE)));
        assert_eq!(balance(&reg, ALICE), a0, "CONCORD and self pay nothing");
        on_kill(&mut reg, &by(Some(BOB)));
        assert!(balance(&reg, ALICE) > a0, "a stranger's kill pays");
        let a1 = balance(&reg, ALICE);
        // Another hull: one hull is paid once.
        on_kill(
            &mut reg,
            &KillEvent {
                victim_ship: ALICE_SHIP + 10,
                ..by(None)
            },
        );
        assert!(balance(&reg, ALICE) > a1, "an unknown killer pays");
        let b0 = balance(&reg, BOB);
        on_kill(&mut reg, &loss(990_002, ALICE_SHIP));
        assert_eq!(balance(&reg, BOB), b0);
    }

    /// **No payout on a self kill or a fleet kill**, and the contract still
    /// ends with the ship; a stranger's kill pays.
    #[test]
    fn a_self_or_fleet_kill_pays_nothing() {
        let ev = |killer: i64| KillEvent {
            victim_character: ALICE,
            victim_ship: ALICE_SHIP,
            ship_type: RIFTER,
            killer: Some(killer),
            system: 0,
            npc: None,
            when: crate::pyutil::filetime_now(),
            park: Vec::new(),
        };

        // Self.
        let mut reg = docked();
        insure_alice(&mut reg, 1.0);
        let before = balance(&reg, ALICE);
        on_kill(&mut reg, &ev(ALICE));
        assert_eq!(balance(&reg, ALICE), before);
        assert!(reg.deltas().of(KIND).is_empty(), "the contract ended");

        // Fleet: Bob and Alice in one fleet.
        let mut reg = docked();
        insure_alice(&mut reg, 1.0);
        let f = reg.fleets_mut();
        let fleet = f.create(ALICE).expect("a fleet");
        f.init(ALICE, fleet, None, None, None).expect("seated");
        f.invite(ALICE, fleet, BOB, None, None, None)
            .expect("invited");
        f.accept_invite(BOB, fleet, None, None, None)
            .expect("joined");
        assert!(voided(&reg, ALICE, Some(BOB)), "setup: one fleet");
        let before = balance(&reg, ALICE);
        on_kill(&mut reg, &ev(BOB));
        assert_eq!(balance(&reg, ALICE), before);
        assert!(reg.deltas().of(KIND).is_empty());

        // A stranger in the same NPC corporation is not a fleet-mate.
        let mut reg = docked();
        let payout = insure_alice(&mut reg, 1.0);
        let before = balance(&reg, ALICE);
        on_kill(&mut reg, &ev(BOB));
        assert_eq!(balance(&reg, ALICE), before + payout);
    }

    /// Put `avg` as the trading index of the Rifter.
    fn set_index(reg: &mut Registry, avg: f64) {
        reg.deltas_mut().put(
            crate::econ_prices::KIND,
            vec![RIFTER.to_string()],
            vec![avg.to_string(), "1".to_owned()],
        );
    }

    /// **FE-GUARD-A (R-FLUIDECON §5 G3)**: the quote and the payout follow
    /// the market index when it is under `P_ref`, and never exceed `P_ref`
    /// when it is over; a stranger's kill pays `fraction x full` of the
    /// capped figure, so build, insure, lose never profits.
    #[test]
    fn insurance_payout_never_exceeds_index() {
        let ev = KillEvent {
            victim_character: ALICE,
            victim_ship: ALICE_SHIP,
            ship_type: RIFTER,
            killer: Some(BOB),
            system: 0,
            npc: None,
            when: crate::pyutil::filetime_now(),
            park: Vec::new(),
        };
        let p = INVENTED_PRICE;
        let mut reg = docked();
        assert_eq!(full_price(&reg, RIFTER), FULL_SHARE * p, "no trade: P_ref");
        for (index, want) in [
            (0.6 * p, FULL_SHARE * 0.6 * p),
            (0.5 * p, FULL_SHARE * 0.5 * p),
            (1.7 * p, FULL_SHARE * p),
            // Pulled below the floor, the payout stays at the floor.
            (0.35 * p, FULL_SHARE * INDEX_FLOOR * p),
        ] {
            let mut reg = docked();
            set_index(&mut reg, index);
            assert_eq!(full_price(&reg, RIFTER), want, "index {index}");
            let payout = insure_alice(&mut reg, 0.9);
            assert_eq!(payout, 0.9 * want);
            let before = balance(&reg, ALICE);
            on_kill(&mut reg, &ev);
            assert_eq!(balance(&reg, ALICE), before + payout);
            assert!(payout <= 0.9 * FULL_SHARE * p, "never over P_ref");
        }
        // The index falls after the policy: the payout follows it down.
        let payout = insure_alice(&mut reg, 1.0);
        set_index(&mut reg, 0.5 * p);
        let before = balance(&reg, ALICE);
        on_kill(&mut reg, &ev);
        assert_eq!(balance(&reg, ALICE), before + FULL_SHARE * 0.5 * p);
        assert!(balance(&reg, ALICE) - before < payout);
    }

    /// **CONCORD's kill pays nothing** (CONCORD-1a;
    /// `eventCrimesInsuranceNoPayoutGCC` [V]); the same ship killed by an
    /// ordinary NPC pays, and the contract ends either way.
    #[test]
    fn a_concord_kill_pays_nothing() {
        let ev = |killer: i64| KillEvent {
            victim_character: ALICE,
            victim_ship: ALICE_SHIP,
            ship_type: RIFTER,
            killer: Some(killer),
            system: 0,
            npc: None,
            when: crate::pyutil::filetime_now(),
            park: Vec::new(),
        };
        let mut reg = docked();
        let cop = crate::npc::spawn(
            &mut reg,
            &crate::concord::FIXTURE_COMMANDER,
            30_000_001,
            [0.0; 3],
        )
        .expect("an id");
        insure_alice(&mut reg, 1.0);
        let before = balance(&reg, ALICE);
        on_kill(&mut reg, &ev(cop));
        assert_eq!(balance(&reg, ALICE), before, "no payout");
        assert!(reg.deltas().of(KIND).is_empty(), "the contract ended");

        let mut reg = docked();
        let rat = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, 30_000_001, [0.0; 3])
            .expect("an id");
        let payout = insure_alice(&mut reg, 1.0);
        let before = balance(&reg, ALICE);
        on_kill(&mut reg, &ev(rat));
        assert_eq!(balance(&reg, ALICE), before + payout);
    }

    /// Lane clock: at scale N a policy lasts N times shorter.
    #[test]
    fn at_scale_n_a_policy_lasts_n_times_shorter() {
        let mut reg = docked();
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(7).unwrap());
        let now = crate::pyutil::filetime_now();
        let amount = premium(full_price(&reg, RIFTER), 0.5);
        let c = insure(&mut reg, ALICE, ALICE_SHIP, amount, false, now).expect("insured");
        assert_eq!(c.end - c.start, CONTRACT_LIFETIME / 7);
        assert!(contract(&reg, ALICE_SHIP, c.end - 1).is_some());
        assert!(contract(&reg, ALICE_SHIP, c.end).is_none());
    }

    /// **Contracts expire**: a contract read at its end is gone, a loss
    /// after it pays nothing, and the next `GetContracts` prunes the row.
    #[test]
    fn contracts_expire() {
        let mut reg = docked();
        let now = crate::pyutil::filetime_now();
        let amount = premium(full_price(&reg, RIFTER), 0.5);
        let c = insure(&mut reg, ALICE, ALICE_SHIP, amount, false, now).expect("insured");
        assert_eq!(c.end - c.start, CONTRACT_LIFETIME);
        assert!(contract(&reg, ALICE_SHIP, c.end - 1).is_some());
        assert!(contract(&reg, ALICE_SHIP, c.end).is_none());
        assert!(contracts_of(&reg, ALICE, c.end).is_empty());

        let before = balance(&reg, ALICE);
        on_kill(
            &mut reg,
            &KillEvent {
                victim_character: ALICE,
                victim_ship: ALICE_SHIP,
                ship_type: RIFTER,
                killer: Some(BOB),
                system: 0,
                npc: None,
                when: c.end,
                park: Vec::new(),
            },
        );
        assert_eq!(
            balance(&reg, ALICE) - before,
            base_payout(&reg, RIFTER),
            "an expired contract pays the uninsured base, not the policy"
        );

        let mut reg = docked();
        let amount = premium(full_price(&reg, RIFTER), 0.5);
        insure(&mut reg, ALICE, ALICE_SHIP, amount, false, now).expect("insured");
        let told = |reg: &Registry| {
            crate::notifications::of(reg.deltas(), ALICE)
                .iter()
                .filter(|n| n.type_id == crate::notifications::kind::INSURANCE_EXPIRED)
                .count()
        };
        assert_eq!(told(&reg), 0, "nothing has expired yet");
        prune(&mut reg, now + CONTRACT_LIFETIME);
        assert!(reg.deltas().of(KIND).is_empty(), "pruned");
        assert_eq!(told(&reg), 1, "the owner is told it ran out");
        prune(&mut reg, now + CONTRACT_LIFETIME);
        assert_eq!(told(&reg), 1, "and only once");
    }

    /// **The clock announces the end** (X-GAPS): no `insuranceSvc` call is
    /// needed. The stage sweeps on the seconds that are multiples of
    /// [`SWEEP_EVERY_SECS`] and leaves the rest alone.
    #[test]
    fn the_clock_announces_an_ended_contract() {
        let mut reg = docked();
        let now = crate::pyutil::filetime_now();
        let amount = premium(full_price(&reg, RIFTER), 0.5);
        let c = insure(&mut reg, ALICE, ALICE_SHIP, amount, false, now).expect("insured");
        let told = |reg: &Registry| {
            crate::notifications::of(reg.deltas(), ALICE)
                .iter()
                .filter(|n| n.type_id == crate::notifications::kind::INSURANCE_EXPIRED)
                .count()
        };
        // The first sweep second at or after the contract's end.
        let sweep = (c.end / SECOND / SWEEP_EVERY_SECS + 1) * SWEEP_EVERY_SECS * SECOND;
        tick(&mut reg, Instant::now(), sweep - SECOND);
        assert_eq!(told(&reg), 0, "not a sweep second");
        tick(&mut reg, Instant::now(), c.start);
        assert_eq!(told(&reg), 0, "nothing has ended at the contract's start");
        tick(&mut reg, Instant::now(), sweep);
        assert_eq!(told(&reg), 1, "told at the end, with no call");
        assert!(reg.deltas().of(KIND).is_empty());
        tick(&mut reg, Instant::now(), sweep);
        assert_eq!(told(&reg), 1, "once");
    }

    /// The window's other reads: an unpriced type is `0.0` (left out of the
    /// window), `UnInsureShip` ends only the caller's own contract, and an
    /// uninsured ship has no contract (`None`, truth-tested).
    #[test]
    fn the_window_reads_and_uninsure() {
        let mut reg = docked();
        assert_eq!(
            call(
                &mut reg,
                ALICE,
                "GetInsurancePrice",
                &[PyValue::Int(999_999)],
                &[]
            )
            .unwrap(),
            PyValue::Real(0.0)
        );
        assert_eq!(
            call(
                &mut reg,
                ALICE,
                "GetContractForShip",
                &[PyValue::Int(ALICE_SHIP)],
                &[]
            )
            .unwrap(),
            PyValue::None
        );
        insure_alice(&mut reg, 0.5);
        assert_ne!(
            call(
                &mut reg,
                ALICE,
                "GetContractForShip",
                &[PyValue::Int(ALICE_SHIP)],
                &[]
            )
            .unwrap(),
            PyValue::None
        );
        call(
            &mut reg,
            BOB,
            "UnInsureShip",
            &[PyValue::Int(ALICE_SHIP)],
            &[],
        )
        .unwrap();
        assert_eq!(reg.deltas().of(KIND).len(), 1, "not Bob's to cancel");
        call(
            &mut reg,
            ALICE,
            "UnInsureShip",
            &[PyValue::Int(ALICE_SHIP)],
            &[],
        )
        .unwrap();
        assert!(reg.deltas().of(KIND).is_empty());
    }
}
