//! The hangar and the market as an agent reaches them: stacks in, stacks
//! out, the refinery, the bids. Every function goes through the live path
//! (`Registry::apply_items`, `reprocess::plan`, `Registry::settle_sell`), so
//! the push, the dirty flag, the goods classes of the closed-loop ledger and
//! the journal rows are the live ones.

use crate::item::{Edit, Item, Slot};
use crate::model::FLAG_HANGAR;
use crate::service::Registry;

/// The character's hangar slot at a station.
pub(super) fn slot(character: i64, station: i64) -> Slot {
    Slot::new(character, station, FLAG_HANGAR)
}

/// The packaged stacks of the hangar, ascending by item id.
pub(super) fn stacks(reg: &Registry, character: i64, station: i64) -> Vec<Item> {
    let mut v: Vec<Item> = reg
        .store()
        .at(slot(character, station))
        .filter(|i| !i.singleton)
        .cloned()
        .collect();
    v.sort_by_key(|i| i.item_id);
    v
}

/// Put `quantity` of `type_id` into the hangar: grow the stack that is there,
/// else create one (a mining cycle's own rule). `Err` is the store's reason.
pub(super) fn add(
    reg: &mut Registry,
    character: i64,
    station: i64,
    type_id: i64,
    quantity: i64,
) -> Result<(), String> {
    if quantity < 1 {
        return Ok(());
    }
    let to = slot(character, station);
    let stack = reg
        .store()
        .at(to)
        .filter(|i| i.type_id == type_id && !i.singleton)
        .map(|i| i.item_id)
        .min();
    let edit = match stack {
        Some(item_id) => Edit::Grow { item_id, quantity },
        None => Edit::Create {
            type_id,
            quantity,
            to,
        },
    };
    reg.apply_items(&[edit])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Take `quantity` off a stack (the whole stack is destroyed). `Err` is the
/// store's reason; nothing changed then.
pub(super) fn take(reg: &mut Registry, item_id: i64, quantity: i64) -> Result<(), String> {
    let Some(have) = reg.store().get(item_id).map(|i| i.quantity) else {
        return Err(format!("no item {item_id}"));
    };
    let edit = if quantity >= have {
        Edit::Destroy { item_id }
    } else {
        Edit::Consume { item_id, quantity }
    };
    reg.apply_items(&[edit])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// What a sale did.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct Sold {
    pub units: i64,
    pub isk: f64,
}

/// **Sell every hangar stack `pred` takes into the bids at the station**, at
/// [`crate::econ_agents::rules::sell_floor`] of its `P_ref` or better: the
/// miner's and the ratter's own sale (`builder::sell_goods`,
/// `escort::sell_loot`'s `Fate::Bid`). A type with no `P_ref` has no price to
/// sell at and stays (a starter type, E5). The market fills the best bid
/// first, an NPC bid by destroying the goods, a player's by delivering them,
/// and charges the sales tax. Nothing rests.
pub(super) fn sell_where(
    reg: &mut Registry,
    character: i64,
    station: i64,
    pred: impl Fn(&Item) -> bool,
    notes: &mut Vec<String>,
) -> Sold {
    let mut total = Sold::default();
    let now = crate::pyutil::filetime_now();
    let mut refused = 0;
    for stack in stacks(reg, character, station)
        .into_iter()
        .filter(|i| pred(i))
    {
        let Some(floor) =
            crate::econ_prices::p_ref(reg, stack.type_id).and_then(super::rules::sell_floor)
        else {
            continue;
        };
        let before_isk = reg.character(character).map_or(0.0, |c| c.balance);
        match reg.settle_sell(character, stack.item_id, stack.quantity, floor, now) {
            Ok(true) => {
                let left = reg.store().get(stack.item_id).map_or(0, |i| i.quantity);
                total.units += stack.quantity - left;
                total.isk += reg.character(character).map_or(0.0, |c| c.balance) - before_isk;
            }
            Ok(false) | Err(_) => refused += 1,
        }
    }
    if refused > 0 {
        notes.push(format!("{refused} stacks had no bid at the floor"));
    }
    total
}

/// **Refine the hangar stacks `pred` takes** at the station's refinery: the
/// same plan the client's `Reprocess` runs (`reprocess::plan` with the
/// character's skills and the station's own efficiency and take), applied as
/// one atomic edit list, then the tax charged as the ref-127 row. `false`
/// with a note when the plan is refused (no data, no money for the tax).
pub(super) fn refine(
    reg: &mut Registry,
    character: i64,
    station: i64,
    pred: impl Fn(&Item) -> bool,
    notes: &mut Vec<String>,
) -> bool {
    let table = reg.content().get::<crate::reprocess::Table>();
    let inputs: Vec<crate::reprocess::Input> = stacks(reg, character, station)
        .iter()
        .filter(|i| pred(i) && table.is_reprocessable(i.type_id))
        .filter(|i| i.quantity >= table.portion_size(i.type_id))
        .map(crate::reprocess::Input::of)
        .collect();
    if inputs.is_empty() {
        return false;
    }
    let Some(c) = reg.character(character) else {
        return false;
    };
    let (balance, ship, owner) = (c.balance, c.ship_id, c.station_owner_id);
    let facility = crate::reprocess::Facility::npc_station_at(
        table,
        station,
        reg.npc_standing(character, owner),
    );
    let planned = {
        let pilot = crate::reprocess::pilot(table, reg.skills_for(Some(character)), &[]);
        crate::reprocess::plan(
            table,
            Some(character),
            Some(station),
            &inputs,
            &facility,
            &pilot,
            Some(ship),
            None,
            balance,
        )
    };
    let plan = match planned {
        Ok(p) => p,
        Err(e) => {
            notes.push(format!("refine refused: {e:?}"));
            return false;
        }
    };
    if reg.apply_items(&plan.edits()).is_err() {
        notes.push("refine refused: the stacks moved".to_owned());
        return false;
    }
    if plan.total_isk_cost > 0.0 {
        reg.charge(
            character,
            plan.total_isk_cost,
            crate::reprocess::ENTRY_TYPE_REPROCESSING_TAX,
            owner,
            station,
        );
    }
    true
}

/// **List what is left of the stacks `pred` takes as asks** at the station,
/// the way the live builder and PI farmer do after their bids are served: one
/// ask per (type, station), at the price index
/// ([`crate::econ_prices::average`], `P_ref` until the type has traded) but
/// never under `floor(stack)` (the builder's materials plus its margin), for
/// [`super::rules::LIST_DAYS`], paying the broker's fee and within the order
/// cap, as a live listing does. A type with no price, or one already listed
/// here, is left. The first refusal (the cap, the fee) ends the listing.
pub(super) fn list_where(
    reg: &mut Registry,
    character: i64,
    station: i64,
    pred: impl Fn(&Item) -> bool,
    floor: impl Fn(&Item) -> f64,
    notes: &mut Vec<String>,
) -> usize {
    let now = crate::pyutil::filetime_now();
    let mut listed = 0;
    for stack in stacks(reg, character, station)
        .into_iter()
        .filter(|i| pred(i))
    {
        if reg.settle_has_ask(character, stack.type_id, station) {
            continue;
        }
        let Some(p_ref) = crate::econ_prices::p_ref(reg, stack.type_id) else {
            continue;
        };
        let index = crate::econ_prices::average(reg, stack.type_id).unwrap_or(p_ref);
        let price = (index.max(floor(&stack)) * 100.0).floor() / 100.0;
        if !super::rules::positive(price) {
            continue;
        }
        match reg.settle_list(
            character,
            stack.item_id,
            stack.quantity,
            price,
            super::rules::LIST_DAYS,
            now,
        ) {
            Ok(_) => {
                listed += 1;
                notes.push(format!(
                    "listed {} of {} at {price:.2}",
                    stack.quantity, stack.type_id
                ));
            }
            Err(_) => {
                notes.push(format!("{} not listed (order cap or funds)", stack.type_id));
                break;
            }
        }
    }
    listed
}
