//! **What becomes of the old hull** (PROG-PVE2): a bot that has climbed from a
//! frigate to a cruiser keeps one spare, repackages the rest and lists them
//! on the market, so a society of climbing fighters does not pile up hulls
//! and the ISK they cost comes back.
//!
//! # The policy **[A]**
//!
//! A bot keeps at most [`SPARES_KEPT`] assembled hulls of [`HULLS`] in its
//! hangar besides the one it flies: the biggest class (the cheapest to
//! replace the flown ship with), the lowest item id among equals. A spare is
//! what lets a bot that lost its ship fly again at once; a second one of the
//! same size, or a frigate behind a spare cruiser, is capital sitting idle.
//! Falsified by a bot that loses its hull, finds no spare and cannot afford
//! another (then keep more, or the better one), or by hangars that still
//! fill with hulls (then the sale is not reaching the market).
//!
//! # The sale
//!
//! [`surplus`] (pure) picks the hulls past the spares. [`sell_spares`] (over
//! the wire) does what a player does: the market refuses an assembled ship
//! (`RepackageBeforeSelling`), so the hull is **repackaged** first
//! (`repackagingSvc.RepackageItems`, the one positional dict
//! `{stationID: [(itemID, locationID)]}` the client sends; the server moves
//! the fitted modules into the hangar), then **listed** as an ask by the same
//! rule that sells loot ([`crate::escort::plan`]: the market's price, never
//! under the floor, one order per type and station, room for an order). A hull
//! the rule keeps (no reference price, an order already listed) stays in the
//! hangar repackaged: nothing is destroyed, so there is no ISK leak, only a
//! hull not yet sold. The fitted guns come out with it and are the loot
//! sale's to keep or sell ([`crate::escort::lots`]).
//!
//! Only closed-economy fighters sell, at most once per [`SELL_EVERY`].

use crate::acquire::CATEGORY_SHIP;
use crate::escort::{Lot, plan};
use crate::hull_choice::{Class, HULLS, class_of};
use crate::pricing::{Prices, Quote};
use crate::verbs::Book;
use crate::{Conn, Error, ItemRow, Result};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

/// How many assembled spare hulls a bot keeps **[A]**.
pub const SPARES_KEPT: usize = 1;
/// How often a docked bot looks at its spare hulls **[A]**: hulls change
/// hands at upgrades, a few times a game week.
pub const SELL_EVERY: Duration = Duration::from_secs(60 * 60);

/// **The hulls past the spares** (pure): assembled hulls of [`HULLS`] in the
/// hangar other than the `active` ship, of a class no bigger than the flown
/// one, less the [`SPARES_KEPT`] best. A bigger class in the hangar is not a
/// spare: `hull_choice::board_choice` boards it.
pub fn surplus(hangar: &[ItemRow], active: Option<i64>, flown: Class) -> Vec<ItemRow> {
    let mut spares: Vec<(Class, ItemRow)> = hangar
        .iter()
        .filter(|r| r.category_id == CATEGORY_SHIP && r.singleton && Some(r.item_id) != active)
        .filter_map(|r| class_of(HULLS, r.type_id).map(|c| (c, *r)))
        .filter(|(c, _)| *c <= flown)
        .collect();
    spares.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.item_id.cmp(&b.1.item_id)));
    spares
        .into_iter()
        .skip(SPARES_KEPT)
        .map(|(_, r)| r)
        .collect()
}

/// What one [`sell_spares`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sold {
    /// Hulls repackaged.
    pub repackaged: Vec<i64>,
    /// The asks placed, by hull type.
    pub listed: Vec<i64>,
    /// Hulls the sale rule kept (repackaged, still in the hangar).
    pub kept: Vec<i64>,
    /// Each step the server refused, and why.
    pub refused: Vec<String>,
}

impl Sold {
    /// The act log's note.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "spare hulls: repackaged {}, listed {} ({:?}), kept {}",
            self.repackaged.len(),
            self.listed.len(),
            self.listed,
            self.kept.len()
        );
        for why in &self.refused {
            s.push_str("; ");
            s.push_str(why);
        }
        s
    }
}

/// `repackagingSvc.RepackageItems` for `items` in `station`'s hangar: the
/// refusal key, if any.
pub fn repackage(conn: &mut Conn, station: i64, items: &[i64]) -> Result<Option<String>> {
    let pairs: Vec<PyValue> = items
        .iter()
        .map(|&i| PyValue::Tuple(vec![PyValue::Int(i), PyValue::Int(station)]))
        .collect();
    // A marshal dict entry is `(value, key)`.
    let batch = PyValue::Dict(vec![(PyValue::List(pairs), PyValue::Int(station))]);
    let out = conn.call("repackagingSvc", "RepackageItems", vec![batch])?;
    Ok(out.refusal().map(|k| k.to_string()))
}

fn soft<T>(r: Result<T>, what: &str, refused: &mut Vec<String>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(Error::Refused(why) | Error::NotSent(why)) => {
            refused.push(format!("{what}: {why}"));
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// **Repackage and list the hulls past the spares**, docked (see the module
/// header). A refusal of one step is noted and the rest go on.
pub fn sell_spares(conn: &mut Conn) -> Result<Sold> {
    let mut sold = Sold::default();
    let station = conn
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("sell_spares: not docked".to_owned()))?;
    let active = conn.session().ship_id();
    let hangar = conn.hangar_items()?;
    let Some(flown) = hangar
        .iter()
        .find(|r| Some(r.item_id) == active)
        .and_then(|r| class_of(HULLS, r.type_id))
    else {
        return Ok(sold);
    };
    let extra = surplus(&hangar, active, flown);
    if extra.is_empty() {
        return Ok(sold);
    }
    let ids: Vec<i64> = extra.iter().map(|r| r.item_id).collect();
    match soft(
        repackage(conn, station, &ids),
        "repackage",
        &mut sold.refused,
    )? {
        Some(None) => sold.repackaged = ids,
        Some(Some(key)) => sold.refused.push(format!("repackage: {key}")),
        None => {}
    }
    if sold.repackaged.is_empty() {
        return Ok(sold);
    }
    // The hulls are packaged stacks now: list them.
    let lots: Vec<Lot> = conn
        .hangar_items()?
        .iter()
        .filter(|r| {
            r.category_id == CATEGORY_SHIP
                && !r.singleton
                && class_of(HULLS, r.type_id).is_some_and(|c| c <= flown)
                && extra.iter().any(|e| e.type_id == r.type_id)
        })
        .map(|r| Lot {
            item_id: r.item_id,
            type_id: r.type_id,
            quantity: r.quantity.max(1),
            whole: false,
        })
        .collect();
    let mut market = Prices::read(conn)?;
    let mut prices = BTreeMap::new();
    let mut books: BTreeMap<i64, Book> = BTreeMap::new();
    for lot in &lots {
        if let Some(p_ref) = market.p_ref(lot.type_id)
            && !prices.contains_key(&lot.type_id)
        {
            let p_mkt = market.p_mkt(conn, lot.type_id)?.unwrap_or(p_ref);
            prices.insert(
                lot.type_id,
                Quote {
                    p_mkt,
                    ..Quote::at_ref(p_ref)
                },
            );
        }
        if let std::collections::btree_map::Entry::Vacant(e) = books.entry(lot.type_id) {
            e.insert(conn.market_orders(lot.type_id)?);
        }
    }
    let mine = conn.my_orders()?;
    let listing = plan(&lots, &books, &prices, station, &mine);
    // Into a bid at once when one pays, else a resting ask: either is a sale.
    if !listing.bids.is_empty()
        && soft(
            conn.sell(&listing.bids, 0, crate::escort::LIST_BROKER_FEE),
            "sell",
            &mut sold.refused,
        )?
        .is_some()
    {
        sold.listed.extend(listing.bids.iter().map(|l| l.type_id));
    }
    for line in &listing.lists {
        let placed = conn.sell(
            &[*line],
            crate::escort::LIST_DAYS,
            crate::escort::LIST_BROKER_FEE,
        );
        if soft(placed, "list", &mut sold.refused)?.is_some() {
            sold.listed.push(line.type_id);
        }
    }
    sold.kept = listing.kept;
    Ok(sold)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hulls of the real table, ids only; items invented.
    const FRIGATE: i64 = 587;
    const FRIGATE_2: i64 = 585;
    const CRUISER: i64 = 629;
    const BC: i64 = 24_702;

    fn hull(item_id: i64, type_id: i64, singleton: bool) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity: 1,
            group_id: 0,
            category_id: CATEGORY_SHIP,
            singleton,
        }
    }

    fn ids(rows: &[ItemRow]) -> Vec<i64> {
        rows.iter().map(|r| r.item_id).collect()
    }

    #[test]
    fn the_best_spare_is_kept_and_the_rest_are_for_sale() {
        // Flying the cruiser (item 10): a frigate and another frigate behind
        // it. One spare is kept; the lower id of two equals.
        let hangar = [
            hull(10, CRUISER, true),
            hull(11, FRIGATE, true),
            hull(12, FRIGATE_2, true),
        ];
        let sell = surplus(&hangar, Some(10), Class::Cruiser);
        assert_eq!(ids(&sell), vec![12]);
        // A spare cruiser outranks a spare frigate: the frigate goes.
        let hangar = [
            hull(10, BC, true),
            hull(11, FRIGATE, true),
            hull(13, CRUISER, true),
        ];
        let sell = surplus(&hangar, Some(10), Class::Battlecruiser);
        assert_eq!(ids(&sell), vec![11]);
    }

    #[test]
    fn one_old_hull_is_the_spare_and_is_not_sold() {
        let hangar = [hull(10, CRUISER, true), hull(11, FRIGATE, true)];
        assert!(surplus(&hangar, Some(10), Class::Cruiser).is_empty());
        // With no hull flown there is no class: the caller sells nothing.
        assert!(surplus(&[], None, Class::Frigate).is_empty());
    }

    #[test]
    fn the_flown_ship_a_packaged_hull_and_a_bigger_class_are_never_surplus() {
        let hangar = [
            hull(10, FRIGATE, true),
            // Packaged: not assembled, not repackageable.
            hull(11, FRIGATE_2, false),
            hull(12, FRIGATE_2, false),
            // A bigger class is for boarding, not for sale.
            hull(13, CRUISER, true),
            hull(14, BC, true),
            // Not a table hull at all.
            hull(15, 1, true),
        ];
        assert!(surplus(&hangar, Some(10), Class::Frigate).is_empty());
        // The active ship is skipped even when it is the biggest.
        let all = [hull(10, BC, true), hull(11, CRUISER, true)];
        assert!(surplus(&all, Some(10), Class::Battlecruiser).is_empty());
    }

    #[test]
    fn the_policy_keeps_the_named_number() {
        let many: Vec<ItemRow> = (0..10)
            .map(|i| hull(100 + i, FRIGATE, true))
            .chain([hull(1, CRUISER, true)])
            .collect();
        let sell = surplus(&many, Some(1), Class::Cruiser);
        assert_eq!(sell.len(), 10 - SPARES_KEPT);
    }
}
