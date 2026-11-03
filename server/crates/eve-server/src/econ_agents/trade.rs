//! **The trader's settlement** (R-ECON §1.5: "a trader's arbitrage margin as
//! ISK"). The live trader (`eve-bot/src/trader.rs`) hauls goods between
//! stations whose books differ; the server has no hauler to fly, so it
//! settles the one loop it can price from its own tables: the **NPC
//! trade-goods loop** (E5b, O5-i), where NPC corporations ask trade goods at
//! [`econ_exchange::TRADE_GOOD_ASK`] x base and bid at
//! [`econ_exchange::TIER_A_BID`] x base.
//!
//! - **The edge** ([`rules::trade_edge`]): `bid x (1 - sales tax) / ask - 1`,
//!   with the pilot's own Accounting-aware tax (`Registry::settle_sales_tax_rate`,
//!   the live formula). At Accounting 0 the edge is negative -- the live
//!   header's "0.92 < 0.94" -- so an unskilled trader's settlement earns
//!   **nothing**: the rule is told no case, it finds the numbers.
//! - **The runs** ([`rules::trade_runs`]): [`rules::TRADER_WALLET_SHARE`] of the
//!   wallet a run, [`rules::TRADER_RUNS_PER_H`] x [`rules::EFFICIENCY`] runs an hour of work,
//!   bounded by the day's [`rules::TRADER_TURNOVER_DAY`] (the NPC side's
//!   `TIER_A_STOCK` and trickle seen from the hauler).
//! - **The books**: the purchase and the sale are ref-2 market rows whose
//!   reference is in the NPC order band, which is how the ledger classifies an
//!   NPC fill (`econ_ledger::classify`): the purchase a sink, the sale a
//!   faucet, the sales tax a ref-54 sink. The goods are minted by the NPC ask
//!   and destroyed by the NPC bid, so none appear. The net is the margin.
//!
//! **[A]** the whole loop stands in for the trips (no stock is taken from a
//! station's order, no jump is flown), falsified by a settled trader's
//! `[settle]` ISK an hour above a live trader's in the `[soc]` ledger.

use super::{Agent, rules};
use crate::damage::BlueTime;
use crate::econ_exchange::{NPC_ORDER_BASE, TIER_A_BID, TRADE_GOOD_ASK};
use crate::service::Registry;

/// The NPC corporations' side of a settled trade: the counterparty of both
/// ref-2 rows. The system owner id the journal uses for the world.
const NPC_COUNTERPARTY: i64 = 1;

/// Settle one trader for `work_h` hours.
pub(super) fn settle(
    reg: &mut Registry,
    character: i64,
    a: &Agent,
    work_h: f64,
    _now: BlueTime,
    notes: &mut Vec<String>,
) {
    let _ = a;
    if !rules::positive(work_h) {
        return;
    }
    let tax = reg.settle_sales_tax_rate(character);
    let wallet = reg.character(character).map_or(0.0, |c| c.balance);
    let turnover = rules::TRADER_TURNOVER_DAY * work_h / rules::DAY_HOURS_CAP;
    let trades = rules::trade_runs(wallet, work_h, turnover, TRADE_GOOD_ASK, TIER_A_BID, tax);
    if trades.spent <= 0.0 {
        notes.push(format!(
            "the NPC trade-goods loop earns {:+.3} a ISK at tax {tax:.3}: nothing traded",
            rules::trade_edge(TRADE_GOOD_ASK, TIER_A_BID, tax)
        ));
        return;
    }
    // Each row is a real wallet move, journalled at its own balance.
    book(reg, character, -trades.spent, 2, NPC_ORDER_BASE);
    book(reg, character, trades.revenue, 2, NPC_ORDER_BASE + 1);
    book(reg, character, -trades.tax, 54, NPC_ORDER_BASE + 1);
    notes.push(format!(
        "traded {:.0} ISK of goods at the NPC loop, tax {:.0}",
        trades.spent, trades.tax
    ));
}

/// One wallet row: the balance, the journal entry and the push.
fn book(reg: &mut Registry, character: i64, amount: f64, entry_type: i64, reference: i64) {
    if amount == 0.0 {
        return;
    }
    let amount = (amount * 100.0).round() / 100.0;
    let Some(c) = reg.character_mut(character) else {
        return;
    };
    c.balance += amount;
    let balance = c.balance;
    reg.mark_dirty();
    // The payer first, as `move_isk` writes it.
    let (payer, payee) = if amount < 0.0 {
        (character, NPC_COUNTERPARTY)
    } else {
        (NPC_COUNTERPARTY, character)
    };
    reg.journal_mut().record(
        crate::pyutil::filetime_now(),
        entry_type,
        payer,
        payee,
        reference,
        amount,
        balance,
        "",
    );
    reg.push(crate::notify::AccountChange {
        owner_id: character,
        balance,
    });
}
