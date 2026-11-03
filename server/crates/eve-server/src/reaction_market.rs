//! **Where a reaction formula and a moon material come from** (R-MOON packet
//! MN-F): the tests that pin the answer, and the two group ids the bots read.
//! No new rule: the answer is the exchange that is already live.
//!
//! # Formulas: the NPC sells them, in the client's own table [V-data]
//!
//! `corporationTrades` (`npc/trades.tsv`) lists reaction formulas, as
//! blueprint originals, with a **positive** value for several NPC
//! corporations (the sign is the side, R-NPCTRADE R1, verified on 547,778 TQ
//! orders). A positive entry in category 9 is an ask at **every station the
//! corporation owns** at `basePrice` (R3, exactly 1.0 on TQ) while
//! [`crate::econ_exchange::NPC_BPOS`] (owner decision O5-a: NPCs sell
//! blueprint originals as a knowledge sink and never buy them back). A
//! formula is a category-9 type, so the generic Tier A path already posts it:
//! there is no formula-specific rule to add, and none was added. Measured in
//! the build-3619879 bundle, with the counts in `docs/lanes/mn-fg.md` only:
//! most of the 120 formulas are asked, at the stations of those
//! corporations in most regions; the biochemical (booster) formulas are listed by nobody,
//! which is also what the data says on TQ for those groups **[I]**.
//!
//! So **no GM-given formula, no loot formula, no `/createitem`**: a player or
//! a bot buys the original at an NPC station, at 10-50 M ISK.
//! [`formula_asks_follow_the_corporation_trades`] pins that for any table and
//! [`every_bundle_formula_an_npc_lists_is_asked_at_base_price`] for the real
//! bundle (set `EVE_TEST_BUNDLE`).
//!
//! # Moon materials: priced, never posted by an NPC [V-data, O5-d]
//!
//! Every moon material (group 427) has a `P_ref` row (`econ/prices.tsv`,
//! source `base`, the type's `basePrice`) and a market group, so the client's
//! market lists it and the bots' [`crate::econ_prices`] oracle prices it. No
//! corporation lists one in `corporationTrades`, and the Tier B floor bids on
//! "moon" that C5 §4.3 proposed are **off under O5-d** (`FLOOR_GROUPS` is read
//! by no NPC order). So goo is traded only between pilots: the miner that
//! refines moon ore sells it into a reactor's standing bid, the reactor sells
//! its composite to a builder. Nothing is minted and no NPC buys it, which is
//! "nothing from nowhere": the goo in the economy is the goo mined from a lit
//! moon belt (MN-B, MN-C) and refined (MN-E).
//! [`goo_has_no_npc_order`] pins it for any table and
//! [`every_bundle_goo_is_priced_and_no_npc_trades_it`] for the bundle.

/// `groupMoonMaterials` **[V-data]**: the raw moon goo (R4 .. R64), what
/// reprocessing a moon ore yields and the first reaction's input.
pub const GOO_GROUP: i64 = 427;
/// The simple-reaction products (intermediate materials) **[V-data]**.
pub const INTERMEDIATE_GROUP: i64 = 428;
/// The complex-reaction products (composites) **[V-data]**, what a T2
/// component's recipe eats.
pub const COMPOSITE_GROUP: i64 = 429;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::econ_exchange::{self as exchange, Side, Station, Table, TypeRow};
    use crate::econ_prices;
    use crate::model::SandboxConfig;
    use crate::reactions::{REACTION_GROUPS, is_formula_group};
    use crate::service::Registry;
    use std::collections::BTreeMap;

    const NOW: i64 = 134_200_000_000_000_000;
    const REGION: i64 = 19_000_777;
    /// Invented: two corporations, three stations, a formula, a second
    /// formula nobody lists, a goo and a hub.
    const SELLER: i64 = 1_999_771;
    const OTHER: i64 = 1_999_772;
    const SELLER_A: i64 = 69_777_001;
    const SELLER_B: i64 = 69_777_002;
    const OTHER_A: i64 = 69_777_003;
    const FORMULA: i64 = 990_770_001;
    const UNLISTED: i64 = 990_770_002;
    const GOO: i64 = 990_770_101;
    const FORMULA_BASE: f64 = 12_000_000.0;
    const GOO_BASE: f64 = 16.0;

    fn row(group_id: i64, category_id: i64, base_price: f64) -> TypeRow {
        TypeRow {
            group_id,
            category_id,
            base_price,
            meta_group_id: None,
            tech_level: None,
        }
    }

    fn station(station_id: i64, owner_id: i64) -> Station {
        Station {
            station_id,
            system_id: 39_777_001,
            region_id: REGION,
            owner_id,
        }
    }

    fn registry(table: Table, prices: econ_prices::Table) -> Registry {
        let mut c = crate::content::Content::fixture();
        c.insert("econ_exchange", table);
        c.insert("econ_prices", prices);
        Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(c))
    }

    fn types() -> BTreeMap<i64, TypeRow> {
        BTreeMap::from([
            (
                FORMULA,
                row(
                    REACTION_GROUPS[0],
                    exchange::CATEGORY_BLUEPRINT,
                    FORMULA_BASE,
                ),
            ),
            (
                UNLISTED,
                row(
                    REACTION_GROUPS[2],
                    exchange::CATEGORY_BLUEPRINT,
                    FORMULA_BASE,
                ),
            ),
            (GOO, row(GOO_GROUP, 4, GOO_BASE)),
        ])
    }

    /// **MN-F, formulas.** A corporation that lists a formula with a positive
    /// value asks it at every station it owns, at `basePrice`, and bids on
    /// it nowhere; a corporation that lists nothing posts nothing; a formula
    /// no corporation lists is sold by nobody.
    #[test]
    fn formula_asks_follow_the_corporation_trades() {
        assert!(is_formula_group(REACTION_GROUPS[0]));
        let stations = vec![
            station(SELLER_A, SELLER),
            station(SELLER_B, SELLER),
            station(OTHER_A, OTHER),
        ];
        let trades = [(SELLER, FORMULA, 0.25)];
        let t = Table::new(stations, &trades, types(), &[]).expect("builds");
        let reg = registry(t, econ_prices::Table::default());
        let all = exchange::orders(&reg, REGION, None, NOW);
        let at = |ty, side| {
            let mut v: Vec<i64> = all
                .iter()
                .filter(|o| o.type_id == ty && o.side == side)
                .map(|o| o.station_id)
                .collect();
            v.sort_unstable();
            v
        };
        if exchange::NPC_BPOS {
            assert_eq!(at(FORMULA, Side::Ask), [SELLER_A, SELLER_B]);
            let price = all
                .iter()
                .find(|o| o.type_id == FORMULA)
                .map(|o| o.price)
                .expect("an ask");
            assert_eq!(price, FORMULA_BASE * exchange::TIER_A_ASK);
        } else {
            assert!(at(FORMULA, Side::Ask).is_empty());
        }
        assert!(
            at(FORMULA, Side::Bid).is_empty(),
            "no NPC buys a formula back"
        );
        assert!(at(UNLISTED, Side::Ask).is_empty(), "nobody lists it");
        // The index a bot reads to skip a formula no NPC sells.
        let sold = reg.content().get::<Table>().asked_types();
        assert_eq!(sold.contains(&FORMULA), exchange::NPC_BPOS);
        assert!(!sold.contains(&UNLISTED) && !sold.contains(&GOO));
    }

    /// **MN-F, goo.** No NPC posts moon goo, either side, at a corporation
    /// station or at a Tier B hub, whatever its price row says: the economy's
    /// goo is mined and refined, not bought from nowhere.
    #[test]
    fn goo_has_no_npc_order() {
        let stations = vec![station(SELLER_A, SELLER), station(OTHER_A, OTHER)];
        let trades = [(SELLER, FORMULA, 0.25)];
        let hubs = [OTHER_A];
        let t = Table::new(stations, &trades, types(), &hubs).expect("builds");
        let prices = econ_prices::Table::parse(&[format!(
            "{GOO}\t{GOO_BASE}\tbase\t0.0\t4\t{GOO_GROUP}\t\t1"
        )])
        .expect("prices");
        let reg = registry(t, prices);
        let all = exchange::orders(&reg, REGION, None, NOW);
        assert!(all.iter().all(|o| o.type_id != GOO), "{all:?}");
        assert!(
            exchange::FLOOR_GROUPS.contains(&GOO_GROUP),
            "the group is one the miner classifies as raw material"
        );
    }

    /// The real bundle, every formula and goo, with **no id named**: set
    /// `EVE_TEST_BUNDLE` to an extracted `out/content` (otherwise it passes
    /// without reading anything).
    fn bundle() -> Option<crate::content::Loaded> {
        let dir = std::env::var_os("EVE_TEST_BUNDLE")?;
        Some(
            crate::content::load(
                Some(std::path::Path::new(&dir)),
                &crate::authored::Dir::compiled_only(),
            )
            .expect("the bundle loads"),
        )
    }

    /// Every region the bundle's stations are in.
    fn regions(dir: &std::path::Path) -> Vec<i64> {
        let (b, _) = crate::content::Bundle::open(dir).expect("opens");
        let (rel, header) = exchange::STATIONS;
        let mut regions: Vec<i64> = b
            .data_lines(rel, header)
            .expect("stations")
            .iter()
            .filter_map(|l| l.split('\t').nth(2)?.parse().ok())
            .collect();
        regions.sort_unstable();
        regions.dedup();
        regions
    }

    #[test]
    fn every_bundle_formula_an_npc_lists_is_asked_at_base_price() {
        let Some(loaded) = bundle() else {
            return;
        };
        let dir = std::path::PathBuf::from(std::env::var_os("EVE_TEST_BUNDLE").expect("set"));
        let content = std::sync::Arc::new(loaded.content);
        let recipes: &crate::industry::Table = content.get();
        let prices: &econ_prices::Table = content.get();
        let reg = Registry::new(SandboxConfig::default()).with_content(content.clone());
        let mut asked = std::collections::BTreeSet::new();
        for region in regions(&dir) {
            for o in exchange::orders(&reg, region, None, NOW) {
                if !recipes.is_formula(o.type_id) {
                    continue;
                }
                assert_eq!(o.side, Side::Ask, "an NPC never bids on a formula: {o:?}");
                let base = prices.prices.get(&o.type_id).expect("a formula is priced");
                assert!(
                    o.price >= base.ref_price * exchange::TIER_A_ASK,
                    "{o:?} under its base price"
                );
                asked.insert(o.type_id);
            }
        }
        if exchange::NPC_BPOS {
            assert!(!asked.is_empty(), "the bundle's corporations sell formulas");
        }
        crate::errln!(
            "MN-F: {} of {} formulas asked by an NPC",
            asked.len(),
            recipes.reactions.len()
        );
    }

    #[test]
    fn every_bundle_goo_is_priced_and_no_npc_trades_it() {
        let Some(loaded) = bundle() else {
            return;
        };
        let dir = std::path::PathBuf::from(std::env::var_os("EVE_TEST_BUNDLE").expect("set"));
        let content = std::sync::Arc::new(loaded.content);
        let prices: &econ_prices::Table = content.get();
        let goo: Vec<i64> = prices
            .prices
            .iter()
            .filter(|(_, p)| p.group_id == GOO_GROUP)
            .map(|(&t, _)| t)
            .collect();
        assert!(!goo.is_empty(), "the bundle prices moon materials");
        for t in &goo {
            assert!(prices.prices[t].ref_price > 0.0, "goo {t} has no price");
        }
        let reg = Registry::new(SandboxConfig::default()).with_content(content.clone());
        for region in regions(&dir) {
            assert!(
                exchange::orders(&reg, region, None, NOW)
                    .iter()
                    .all(|o| !goo.contains(&o.type_id)),
                "an NPC order for goo in {region}"
            );
        }
    }
}
