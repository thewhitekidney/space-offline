//! **Bots insure the combat hulls they fly** (X-INSURE, `docs/lanes/x-insure.md`).
//!
//! Insurance is the server's (`service/insurance.rs`, CP44): a contract on a
//! ship pays `fraction x full` to its owner when the ship is lost, journal
//! ref 19, and ends with the ship or after its weeks. This module is the
//! bot's half: before a bot undocks in a combat hull it asks
//! `insuranceSvc.GetContractForShip` and, when the hull is uncovered, buys
//! the package its role's risk calls for ([`level_for`]) with
//! `InsureShip(itemID, premium, False)` -- the calls the client's Insurance
//! window makes (`base_insurance.py`) **[V]**.
//!
//! - **The numbers are the client's** **[V]**: the six packages
//!   ([`PACKAGES`]) and the premium `full x (fraction - 0.4) x 0.5`
//!   ([`premium`]) are `insuranceConst.py`; `full` is what
//!   `GetInsurancePrice(typeID)` answers.
//! - **Renewal** is the same check at the next undock: an ended contract is
//!   no contract, so an expired hull is covered again **[A]** (a hull that
//!   stays in space past its expiry is bought for at its next dock).
//! - **Payouts are counted without a line of code here**: the income ledger
//!   (`driver::Bot::ledger`) is wallet deltas, and a replacement hull is
//!   bought by [`crate::acquire`] from the live `GetCashBalance`, so a
//!   premium paid and a payout received are both in the wallet both read.
//!
//! The decision ([`plan`]) is pure; [`cover`] is the wire.

use crate::hull_choice;
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;

/// `INSURANCE_PACKAGES`' payout ratios, Basic to Platinum **[V]**
/// (`insuranceConst.py`).
pub const PACKAGES: [f64; 6] = [0.5, 0.6, 0.7, 0.8, 0.9, 1.0];

/// `BASE_INSURANCE_FRACTION` **[V]** (`insuranceConst.py`).
pub const BASE_FRACTION: f64 = 0.4;

/// `INSURANCE_FRACTION_INCREMENT` **[V]** (`insuranceConst.py`).
pub const FRACTION_INCREMENT: f64 = 0.5;

/// The share of the wallet a premium may take **[A]**: a bot that is
/// nearly broke buys a lower package or none, and keeps what a replacement
/// hull needs. Falsified by a closed soak's bots stranded without a hull
/// while the journal shows premiums paid.
pub const PREMIUM_SHARE: f64 = 0.25;

/// The package by career risk **[A]**: the hunter picks fights, so it takes
/// Platinum; a site runner meets the heaviest rats, Gold; a ratter and a
/// mission runner fight rats it chose, Silver; any other role that flies a
/// combat hull is Basic. Falsified by a bot population whose payouts never
/// cover premiums (too high) or whose losses leave it unable to replace the
/// hull (too low).
pub fn level_for(role: &str) -> f64 {
    match role {
        "hunter" | "hostile" => PACKAGES[5],
        "sites" => PACKAGES[3],
        "escort" | "missions" => PACKAGES[1],
        _ => PACKAGES[0],
    }
}

/// `_get_insurance_price_by_fraction` **[V]**: the premium of `fraction`.
pub fn premium(full: f64, fraction: f64) -> f64 {
    full * ((fraction - BASE_FRACTION) * FRACTION_INCREMENT)
}

/// Why a hull was left as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Not a combat hull of [`hull_choice::is_combat_hull`].
    NotCombat,
    /// A running contract exists.
    Covered,
    /// The insurer quotes no price.
    Unpriced,
    /// Even Basic costs more than [`PREMIUM_SHARE`] of the wallet.
    TooPoor,
}

/// What to do about the hull flown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Plan {
    Skip(Why),
    /// Buy the package `fraction`, for `premium`.
    Buy {
        fraction: f64,
        premium: f64,
    },
}

/// **The decision**: the highest package at or below `level` whose premium
/// is within [`PREMIUM_SHARE`] of `wallet`, for an uncovered combat hull.
pub fn plan(combat: bool, covered: bool, full: f64, wallet: f64, level: f64) -> Plan {
    if !combat {
        return Plan::Skip(Why::NotCombat);
    }
    if covered {
        return Plan::Skip(Why::Covered);
    }
    if full.is_nan() || full <= 0.0 {
        return Plan::Skip(Why::Unpriced);
    }
    PACKAGES
        .iter()
        .rev()
        .filter(|&&f| f <= level)
        .map(|&f| (f, premium(full, f)))
        .find(|&(_, p)| p <= wallet * PREMIUM_SHARE)
        .map_or(Plan::Skip(Why::TooPoor), |(fraction, premium)| Plan::Buy {
            fraction,
            premium,
        })
}

fn real(v: Option<PyValue>) -> f64 {
    match v {
        Some(PyValue::Real(r)) => r,
        Some(PyValue::Int(n)) => n as f64,
        _ => 0.0,
    }
}

/// **Cover the hull flown**, docked: read the contract, the price and the
/// wallet, and buy what [`plan`] says. Returns a note for the act's log
/// line. A refusal is a note, not an error: no undock waits on insurance.
pub fn cover(conn: &mut Conn, role: &str) -> Result<String> {
    cover_with(conn, level_for(role), hull_choice::is_combat_hull)
}

/// [`cover`] at an explicit package `level`, `combat` naming the hulls that
/// are worth covering (a socket test's invented hull is not in the table).
pub fn cover_with(conn: &mut Conn, level: f64, combat: impl Fn(i64) -> bool) -> Result<String> {
    let Some(ship) = conn.session().ship_id() else {
        return Ok("insurance: no ship".to_owned());
    };
    let Some(type_id) = conn
        .hangar_items()?
        .iter()
        .find(|r| r.item_id == ship)
        .map(|r| r.type_id)
    else {
        return Ok("insurance: hull not in the hangar".to_owned());
    };
    if !combat(type_id) {
        return Ok(String::new());
    }
    let covered = !matches!(
        conn.call(
            "insuranceSvc",
            "GetContractForShip",
            vec![PyValue::Int(ship)]
        )?
        .value(),
        Some(PyValue::None) | None
    );
    if covered {
        return Ok(String::new());
    }
    let full = real(
        conn.call(
            "insuranceSvc",
            "GetInsurancePrice",
            vec![PyValue::Int(type_id)],
        )?
        .value(),
    );
    let wallet = conn.cash_balance()?;
    match plan(true, false, full, wallet, level) {
        Plan::Skip(why) => Ok(format!("insurance: none ({why:?})")),
        Plan::Buy { fraction, premium } => {
            let out = conn.call_kw(
                "insuranceSvc",
                "InsureShip",
                vec![
                    PyValue::Int(ship),
                    PyValue::Real(premium),
                    PyValue::Bool(false),
                ],
                vec![],
            );
            match out {
                Ok(o) => match o.refusal() {
                    None => Ok(format!(
                        "insured {ship} at {:.0}% for {premium:.0}",
                        fraction * 100.0
                    )),
                    Some(key) => Ok(format!("insurance refused: {key}")),
                },
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
                Err(e) => Ok(format!("insurance failed: {e}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An invented hull price: a test asserts code against itself.
    const FULL: f64 = 100_000.0;
    const RICH: f64 = 1.0e9;

    #[test]
    fn the_premium_is_the_clients_formula_at_every_package() {
        for f in PACKAGES {
            assert_eq!(
                premium(FULL, f),
                FULL * ((f - BASE_FRACTION) * FRACTION_INCREMENT)
            );
        }
        assert!(premium(FULL, PACKAGES[0]) < premium(FULL, PACKAGES[5]));
    }

    #[test]
    fn an_uncovered_combat_hull_buys_its_roles_package() {
        for role in ["hunter", "sites", "escort", "mine"] {
            let level = level_for(role);
            let Plan::Buy {
                fraction,
                premium: p,
            } = plan(true, false, FULL, RICH, level)
            else {
                panic!("{role} buys")
            };
            assert_eq!(fraction, level);
            assert_eq!(p, premium(FULL, level));
        }
        assert!(level_for("hunter") > level_for("sites"));
        assert!(level_for("sites") > level_for("escort"));
        assert!(level_for("escort") > level_for("mine"));
    }

    #[test]
    fn covered_unpriced_and_civilian_hulls_buy_nothing() {
        let level = level_for("hunter");
        assert_eq!(
            plan(true, true, FULL, RICH, level),
            Plan::Skip(Why::Covered)
        );
        assert_eq!(
            plan(false, false, FULL, RICH, level),
            Plan::Skip(Why::NotCombat)
        );
        assert_eq!(
            plan(true, false, 0.0, RICH, level),
            Plan::Skip(Why::Unpriced)
        );
    }

    #[test]
    fn a_poor_bot_steps_down_a_package_and_a_broke_one_buys_none() {
        let level = level_for("hunter");
        // Wallet that fits Gold's premium but not Platinum's.
        let wallet = premium(FULL, PACKAGES[3]) / PREMIUM_SHARE;
        match plan(true, false, FULL, wallet, level) {
            Plan::Buy { fraction, .. } => assert_eq!(fraction, PACKAGES[3]),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            plan(true, false, FULL, 1.0, level),
            Plan::Skip(Why::TooPoor)
        );
    }

    #[test]
    fn a_package_never_exceeds_the_role_level() {
        let level = level_for("mine");
        let Plan::Buy { fraction, .. } = plan(true, false, FULL, RICH, level) else {
            panic!("buys")
        };
        assert!(fraction <= level);
    }
}
