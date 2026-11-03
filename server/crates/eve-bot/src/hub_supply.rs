//! **Hub supply** (D-HUB, `docs/lanes/d-hub.md`): how a Sovereignty Hub
//! reaches a claimer without anything coming from nowhere.
//!
//! - **Making one.** The builder's ladder has a hub rung
//!   ([`crate::hulls::capital_rungs`], last): the blueprint (the client's own
//!   NPC trade lists put it on sale at a few corporations' stations, so the
//!   exchange already sells the original; no seller is added here), the
//!   inputs capital components and P4 goods ([`crate::builder::Sourcing`]).
//!   A finished hub stays in the builder's hangar ([`hubs_in`]): it is not a
//!   ceiling-listed product ([`crate::builder::is_product`]).
//! - **Can anyone afford one?** [`Bill`] and [`hours_to_afford`] put numbers
//!   on it; the lane note records them. In a closed world the answer is no,
//!   so the rung exists and stays unreached; nothing here raises an income.
//! - **The seam for the sov bots** ([`HubBoard`]): builders [`HubBoard::post`]
//!   a hub they hold, a hauler or a claimer asks [`HubBoard::nearest`]
//!   "is a hub available and where", and [`plan_carry`] says whether a hold
//!   takes it. The board is plain data behind one lock, shared as an `Arc`
//!   (no global); wiring it into `driver::Shared` is the coordinator's diff.
//!
//! Every number is **[A]** unless a line says otherwise.

use crate::verbs::ItemRow;
use eve_server::content::Content;
use eve_server::econ_prices;
use eve_server::industry;
use std::sync::Mutex;

/// A packaged Sovereignty Hub's volume, m3 **[A]**: the owner's figure (60,000).
/// The shipped type data has no row for the hub, so the number could not be
/// read from the client; [`hub_volume`] reads the item store first and falls
/// back to this. Falsified by a hub row (after the extractor fix) whose
/// volume differs.
pub const HUB_VOLUME_FALLBACK_M3: f64 = 60_000.0;

/// `item::find_in`'s volume for `type_id`, else [`HUB_VOLUME_FALLBACK_M3`]
/// (a zero volume is no volume).
pub fn hub_volume(content: &Content, type_id: i64) -> f64 {
    eve_server::item::find_in(content, type_id)
        .map(|t| t.volume)
        .filter(|v| *v > 0.0)
        .unwrap_or(HUB_VOLUME_FALLBACK_M3)
}

/// The hubs in a hangar listing, by item id.
pub fn hubs_in(hangar: &[ItemRow]) -> Vec<&ItemRow> {
    let mut v: Vec<&ItemRow> = hangar
        .iter()
        .filter(|r| r.group_id == crate::hulls::GROUP_SOV_HUB && r.quantity > 0)
        .collect();
    v.sort_by_key(|r| r.item_id);
    v
}

/// A hub a builder holds, where a hauler can fetch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    pub item_id: i64,
    pub type_id: i64,
    /// The character that holds it.
    pub holder: i64,
    /// The station whose hangar holds it.
    pub station: i64,
    /// That station's solar system.
    pub system: i64,
}

/// **The board** the sov bots ask. Plain data; clone the `Arc` around it.
#[derive(Debug, Default)]
pub struct HubBoard {
    offers: Mutex<Vec<Offer>>,
}

impl HubBoard {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Offer>> {
        self.offers.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Post (or move) the offer for `offer.item_id`.
    pub fn post(&self, offer: Offer) {
        let mut v = self.lock();
        v.retain(|o| o.item_id != offer.item_id);
        v.push(offer);
    }

    /// The hub is gone (launched, sold, lost): drop its offer.
    pub fn withdraw(&self, item_id: i64) {
        self.lock().retain(|o| o.item_id != item_id);
    }

    /// Every offer, by item id.
    pub fn all(&self) -> Vec<Offer> {
        let mut v = self.lock().clone();
        v.sort_by_key(|o| o.item_id);
        v
    }

    /// **"Is a hub available and where"** (the sov bots' question): the offer
    /// fewest `jumps` from `from`, ties to the lowest item id; `jumps` is
    /// `None` for an unreachable pair (such an offer is skipped).
    pub fn nearest(
        &self,
        from: i64,
        jumps: impl Fn(i64, i64) -> Option<u32>,
    ) -> Option<(Offer, u32)> {
        self.all()
            .into_iter()
            .filter_map(|o| jumps(from, o.system).map(|j| (o, j)))
            .min_by_key(|(o, j)| (*j, o.item_id))
    }
}

/// Why a hub cannot be carried.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoCarry {
    /// The hold is smaller than the hub: `(volume, capacity)`.
    TooBig(f64, f64),
    /// No gate route to the claimer inside the floor.
    NoRoute,
}

/// **Carry a hub** from `offer` to `to_system`: `Ok(jumps)` when the hold of
/// `capacity` m3 takes `volume` and a route exists.
pub fn plan_carry(
    offer: &Offer,
    volume: f64,
    capacity: f64,
    to_system: i64,
    jumps: impl Fn(i64, i64) -> Option<u32>,
) -> Result<u32, NoCarry> {
    if capacity < volume {
        return Err(NoCarry::TooBig(volume, capacity));
    }
    jumps(offer.system, to_system).ok_or(NoCarry::NoRoute)
}

// ---- the honest bill ---------------------------------------------------------

/// What one hub costs a builder at the price list's reference prices, ISK.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bill {
    /// The blueprint original.
    pub blueprint: f64,
    /// One run's materials at their reference prices, as listed (a part a
    /// builder makes is priced as the list prices it).
    pub materials: f64,
}

impl Bill {
    pub fn total(&self) -> f64 {
        self.blueprint + self.materials
    }
}

/// The bill for `blueprint`, `None` when the recipe or a price is missing.
pub fn bill(
    prices: &econ_prices::Table,
    recipes: &industry::Table,
    blueprint: i64,
) -> Option<Bill> {
    let bp = recipes.find(blueprint)?;
    let price = |t: i64| prices.prices.get(&t).map(|p| p.ref_price);
    let mut materials = 0.0;
    for &(t, qty) in &bp.materials {
        materials += price(t)? * qty as f64;
    }
    Some(Bill {
        blueprint: price(blueprint)?,
        materials,
    })
}

/// Hours of `isk_per_hour` income to reach `cost` from `wallet`; `None` with
/// no income.
pub fn hours_to_afford(cost: f64, wallet: f64, isk_per_hour: f64) -> Option<f64> {
    if isk_per_hour <= 0.0 {
        return None;
    }
    Some(((cost - wallet) / isk_per_hour).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hulls::world::*;

    const HOLDER: i64 = 990_730_001;
    const STATION_A: i64 = 990_730_101;
    const STATION_B: i64 = 990_730_102;
    const SYS_A: i64 = 990_730_201;
    const SYS_B: i64 = 990_730_202;
    const SYS_C: i64 = 990_730_203;
    const FAR: i64 = 990_730_299;

    fn offer(item_id: i64, station: i64, system: i64) -> Offer {
        Offer {
            item_id,
            type_id: HUB,
            holder: HOLDER,
            station,
            system,
        }
    }

    /// A line A - B - C, nothing reaches `FAR`.
    fn line(a: i64, b: i64) -> Option<u32> {
        let at = |s: i64| [SYS_A, SYS_B, SYS_C].iter().position(|&x| x == s);
        Some(at(a)?.abs_diff(at(b)?) as u32)
    }

    fn row(item_id: i64, group_id: i64, quantity: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id: HUB,
            owner_id: HOLDER,
            location_id: STATION_A,
            flag_id: 4,
            quantity,
            group_id,
            category_id: 40,
            singleton: false,
        }
    }

    #[test]
    fn only_hub_rows_with_stock_are_hubs() {
        let h = [
            row(9, crate::hulls::GROUP_SOV_HUB, 1),
            row(3, crate::hulls::GROUP_SOV_HUB, 1),
            row(5, 513, 1),
            row(7, crate::hulls::GROUP_SOV_HUB, 0),
        ];
        let ids: Vec<i64> = hubs_in(&h).iter().map(|r| r.item_id).collect();
        assert_eq!(ids, vec![3, 9]);
    }

    #[test]
    fn the_board_answers_the_nearest_reachable_hub() {
        let b = HubBoard::default();
        assert_eq!(b.nearest(SYS_A, line), None, "an empty board has none");
        b.post(offer(2, STATION_B, SYS_C));
        b.post(offer(1, STATION_A, SYS_B));
        b.post(offer(3, STATION_A, FAR));
        assert_eq!(
            b.nearest(SYS_A, line),
            Some((offer(1, STATION_A, SYS_B), 1))
        );
        assert_eq!(
            b.nearest(SYS_C, line),
            Some((offer(2, STATION_B, SYS_C), 0))
        );
        b.withdraw(1);
        assert_eq!(
            b.nearest(SYS_A, line),
            Some((offer(2, STATION_B, SYS_C), 2))
        );
    }

    #[test]
    fn a_tie_goes_to_the_lowest_item_id_and_a_repost_moves_the_offer() {
        let b = HubBoard::default();
        b.post(offer(8, STATION_A, SYS_B));
        b.post(offer(4, STATION_B, SYS_B));
        assert_eq!(b.nearest(SYS_A, line).map(|(o, _)| o.item_id), Some(4));
        b.post(offer(4, STATION_B, SYS_C));
        assert_eq!(b.nearest(SYS_A, line).map(|(o, _)| o.item_id), Some(8));
        assert_eq!(b.all().len(), 2, "one offer per item");
    }

    #[test]
    fn a_hold_must_take_the_volume_and_a_route_must_exist() {
        let o = offer(1, STATION_A, SYS_A);
        let vol = 10.0;
        assert_eq!(plan_carry(&o, vol, vol, SYS_C, line), Ok(2));
        assert_eq!(
            plan_carry(&o, vol, vol - 1.0, SYS_C, line),
            Err(NoCarry::TooBig(vol, vol - 1.0))
        );
        assert_eq!(plan_carry(&o, vol, vol, FAR, line), Err(NoCarry::NoRoute));
    }

    #[test]
    fn volume_is_the_stores_else_the_named_fallback() {
        let (c, _) = hub_world();
        let stored = eve_server::item::find_in(&c, HUB)
            .expect("the hub row")
            .volume;
        assert!(stored > 0.0 && stored != HUB_VOLUME_FALLBACK_M3);
        assert_eq!(hub_volume(&c, HUB), stored, "the store's volume wins");
        assert_eq!(hub_volume(&c, HUB_UNKNOWN), HUB_VOLUME_FALLBACK_M3);
    }

    #[test]
    fn the_bill_adds_the_blueprint_and_a_runs_materials() {
        let (_, recipes) = hub_world();
        let bp_price = 7_000.0;
        let lines = [
            format!("{}\t{bp_price}\tbase\t0.0\t9\t1045\t\t1", bp(HUB)),
            format!("{COMPONENT_OK}\t100.0\tbase\t0.0\t17\t873\t\t1"),
            format!("{P4}\t100.0\tbase\t0.0\t43\t1041\t\t1"),
        ];
        let prices = econ_prices::Table::parse(&lines).expect("prices parse");
        let b = bill(&prices, &recipes, bp(HUB)).expect("priced");
        // Two materials at 100, ten of each.
        assert_eq!(b.blueprint, bp_price);
        assert_eq!(b.materials, 2.0 * 10.0 * 100.0);
        assert_eq!(b.total(), bp_price + 2_000.0);
        assert!(bill(&prices, &recipes, 990_730_999).is_none());
    }

    #[test]
    fn hours_are_zero_when_rich_and_none_without_income() {
        assert_eq!(hours_to_afford(100.0, 500.0, 10.0), Some(0.0));
        assert_eq!(hours_to_afford(100.0, 0.0, 10.0), Some(10.0));
        assert_eq!(hours_to_afford(100.0, 0.0, 0.0), None);
    }
}
