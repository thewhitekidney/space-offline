//! The price oracle: `P_ref` per type from `econ/prices.tsv`, and the
//! trading index `avg` from the `mktidx` delta.
//!
//! W14 packet CP31 (`docs/content/plan.md` §6.2), spec C5 §3.1, §3.4 and
//! packet E2; the file's header is frozen in `docs/content/schema.md`
//! (`econ/prices.tsv`, written by CP24). Class B data (client facts), with
//! one deliberate difference from the other client-data tables: **with no
//! bundle the loaded table is empty, not the fixture.** The fixture's rows
//! are invented, and serving them to a client as prices would be a lie;
//! empty keeps the pre-CP31 answer to `config.GetAverageMarketPrices`, `{}`.
//! [`Table::fixture`] is still what every test's `Registry` sees.
//!
//! What this module answers:
//! - [`p_ref`]: `P_ref`, the price every sink uses (EIV, reprocessing tax,
//!   insurance; C5 §3.4: "never `avg`"), for CP32b, CP33b and CP44;
//! - [`average`]: the trading index, `P_ref` until the type has traded;
//! - [`fold_day`]: one day's volume-weighted mean trade price folded into
//!   the index, for the market and CP35;
//! - [`average_market_prices`]: the `config.GetAverageMarketPrices` answer.

use crate::content::{Boxed, Bundle, Sources, TableSpec};
use crate::pyutil::keyval;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "econ_prices",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

/// The bundle's rows, or an empty table with no bundle (see the module doc).
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Table::from_bundle(b)?,
        None => Table::default(),
    }))
}

/// The bundle file and its header line, copied from `schema.md`.
pub const PRICES: (&str, &str) = (
    "econ/prices.tsv",
    "#type_id\tref_price\tsource\tfloor_price\tcategory_id\tgroup_id\tmeta_group_id\tportion_size",
);

/// The delta kind this module owns: `type_id -> avg day` (plan §4.2).
pub const KIND: &str = "mktidx";

/// The EWMA weight of one day's mean, C5 §3.4. **[A]**: falsified by a
/// retail index that moves visibly faster or slower than a fifth of the gap
/// per traded day. FE-VALUE moved it from 0.2 to 0.25 (R-FLUIDECON §3.2),
/// with the bots' value model in step.
pub const ALPHA: f64 = 0.25;

/// The index stays within `[CLAMP.0, CLAMP.1] × P_ref`, C5 §3.4. **[A]**:
/// falsified by a retail `averagePrice` outside this band of
/// `adjustedPrice` for a liquid type. FE-VALUE widened it from `(0.5, 2.0)` to
/// match the bots' band (`eve_bot::pricing::FE_BAND_MIN/MAX`): keep the two together.
pub const CLAMP: (f64, f64) = (0.30, 4.0);

/// A fill priced above `WASH_BAND x` the prior index, or below its
/// reciprocal, is left out of the index (never out of `mkthist`). FE-GUARD-A,
/// R-FLUIDECON §5 G2. **[A]**: no real wash-trade data for a 50-bot world;
/// falsified by a wash undetected in the soak's `/econ audit`, or a
/// legitimate trade excluded (a fill rejected while the index sat in band).
pub const WASH_BAND: f64 = 3.0;

/// Whether `a` and `b` are two characters of one player corporation or one
/// alliance, or one and the same character. NPC corporations (ids
/// 1,000,000..=1,999,999) are strangers, as for loot rights and insurance,
/// and an id that is no character (an NPC side) is never in a pair.
pub fn same_group(reg: &Registry, a: i64, b: i64) -> bool {
    let (Some(x), Some(y)) = (reg.character(a), reg.character(b)) else {
        return false;
    };
    if a == b {
        return true;
    }
    let player_corp = |id: i64| !(1_000_000..=1_999_999).contains(&id);
    (player_corp(x.corporation_id) && x.corporation_id == y.corporation_id)
        || x.alliance_id.is_some_and(|al| Some(al) == y.alliance_id)
}

/// Does a fill count toward the trading index of `type_id`? Not a wash
/// ([`same_group`]) and not an outlier ([`WASH_BAND`] of the index as it
/// stood before the day). The index only: `mkthist`, the client's price
/// history, and every other reader of the fills are unchanged.
pub fn counts_for_index(reg: &Registry, type_id: i64, buyer: i64, seller: i64, price: f64) -> bool {
    if same_group(reg, buyer, seller) {
        return false;
    }
    let Some(prior) = average(reg, type_id) else {
        return true;
    };
    price <= prior * WASH_BAND && price >= prior / WASH_BAND
}

/// The rule that priced a row (`schema.md`'s `source` enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Recipe,
    Reaction,
    Pi,
    Base,
    Parent,
    Override,
}

impl Source {
    fn parse(s: &str) -> Option<Source> {
        Some(match s {
            "recipe" => Source::Recipe,
            "reaction" => Source::Reaction,
            "pi" => Source::Pi,
            "base" => Source::Base,
            "parent" => Source::Parent,
            "override" => Source::Override,
            _ => return None,
        })
    }
}

/// One `econ/prices.tsv` row (C5 §3.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Price {
    /// `P_ref`, C5 §3.2.
    pub ref_price: f64,
    pub source: Source,
    /// The step-6 floor.
    pub floor_price: f64,
    pub category_id: i64,
    pub group_id: i64,
    pub meta_group_id: Option<i64>,
    pub portion_size: i64,
}

/// Every priced type, by `type_id`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub prices: BTreeMap<i64, Price>,
}

impl Table {
    /// The bundle's `econ/prices.tsv`; an absent file is an empty table.
    pub fn from_bundle(b: &Bundle) -> Result<Table, String> {
        let (rel, header) = PRICES;
        Table::parse(&b.data_lines(rel, header)?).map_err(|e| format!("{rel}: {e}"))
    }

    /// The data lines of `econ/prices.tsv`. A bad row is an error naming
    /// its line (counted among the data lines).
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Table, String> {
        let mut prices = BTreeMap::new();
        for (i, line) in lines.iter().enumerate() {
            let line = line.as_ref();
            let (id, row) =
                parse_row(line).ok_or_else(|| format!("data line {}: {line:?}", i + 1))?;
            prices.insert(id, row);
        }
        Ok(Table { prices })
    }

    /// Invented rows (ids in the 99xxxx test band), never a client value.
    pub fn fixture() -> Table {
        Table::parse(&[
            "990001\t256000.0\trecipe\t120000.0\t6\t99001\t1\t1",
            "990002\t12.5\tbase\t10.0\t4\t99002\t\t100",
            "990003\t4000.0\toverride\t1000.0\t25\t99003\t\t1",
        ])
        .unwrap_or_else(|e| unreachable!("the fixture parses: {e}"))
    }
}

fn parse_row(line: &str) -> Option<(i64, Price)> {
    let f: Vec<&str> = line.split('\t').collect();
    let [id, ref_price, source, floor, cat, group, meta, portion] = f[..] else {
        return None;
    };
    let ref_price: f64 = ref_price.parse().ok()?;
    let floor_price: f64 = floor.parse().ok()?;
    if !(ref_price.is_finite() && ref_price > 0.0 && floor_price.is_finite()) {
        return None;
    }
    let meta_group_id = match meta {
        "" => None,
        m => Some(m.parse().ok()?),
    };
    Some((
        id.parse().ok()?,
        Price {
            ref_price,
            source: Source::parse(source)?,
            floor_price,
            category_id: cat.parse().ok()?,
            group_id: group.parse().ok()?,
            meta_group_id,
            portion_size: portion.parse().ok()?,
        },
    ))
}

/// `P_ref` of `type_id`, or `None` for an unpriced type.
pub fn p_ref(reg: &Registry, type_id: i64) -> Option<f64> {
    reg.content()
        .get::<Table>()
        .prices
        .get(&type_id)
        .map(|p| p.ref_price)
}

/// A stored index row, `(avg, day)`; `None` when unreadable.
fn read_row(value: &[String]) -> Option<(f64, u32)> {
    let [avg, day] = value else { return None };
    let avg: f64 = avg.parse().ok()?;
    avg.is_finite().then_some(())?;
    Some((avg, day.parse().ok()?))
}

/// `avg` clamped into `CLAMP × p_ref`.
fn clamp(avg: f64, p_ref: f64) -> f64 {
    avg.clamp(CLAMP.0 * p_ref, CLAMP.1 * p_ref)
}

/// One EWMA step: `prev` moved `ALPHA` of the way to `mean`, then clamped.
pub fn ewma(prev: f64, mean: f64, p_ref: f64) -> f64 {
    clamp(prev + ALPHA * (mean - prev), p_ref)
}

/// The trading index of `type_id`: the `mktidx` row clamped, or `P_ref`
/// when the type has never traded (C5 §3.4: "absent means `avg = P_ref`").
/// `None` for an unpriced type.
pub fn average(reg: &Registry, type_id: i64) -> Option<f64> {
    let p = p_ref(reg, type_id)?;
    let row = reg.deltas().of(KIND).get(&vec![type_id.to_string()]);
    Some(match row.and_then(|v| read_row(v)) {
        Some((avg, _)) => clamp(avg, p),
        None => p,
    })
}

/// Fold one day's volume-weighted mean trade price of `type_id` into the
/// index, and write the `mktidx` row. Returns the new index.
///
/// One call per type per traded day: a second call for a day the row
/// already holds (or an earlier one) is refused with `None`, so a day
/// weighs `ALPHA` however many times its trades are reported. An unpriced
/// type, or a mean that is not a positive finite number, is `None` and
/// writes nothing.
pub fn fold_day(reg: &mut Registry, type_id: i64, mean: f64, day: u32) -> Option<f64> {
    let p = p_ref(reg, type_id)?;
    if !(mean.is_finite() && mean > 0.0) {
        return None;
    }
    let key = vec![type_id.to_string()];
    let prev = match reg.deltas().of(KIND).get(&key).and_then(|v| read_row(v)) {
        Some((_, last)) if last >= day => return None,
        Some((avg, _)) => clamp(avg, p),
        None => p,
    };
    let avg = ewma(prev, mean, p);
    // Rust's `{}` for f64 is the shortest string that parses back exactly.
    reg.deltas_mut()
        .put(KIND, key, vec![avg.to_string(), day.to_string()]);
    Some(avg)
}

/// The `config.GetAverageMarketPrices` answer (C5 §3.1 [V]):
/// `{typeID: KeyVal(adjustedPrice, averagePrice)}`, read by
/// `typeHelpers.GetAdjustedAveragePrice` and `GetAveragePrice`
/// (`typeHelpers.py:106-118`). `adjustedPrice` is `P_ref` **[A]** (falsified
/// by a capture whose two values differ in kind), `averagePrice` the index.
/// An empty table, which is the server with no bundle, answers `{}`.
pub fn average_market_prices(reg: &Registry) -> PyValue {
    // The ~12k-entry copy happens here, after the slot's mutex is released.
    PyValue::clone(&average_market_prices_shared(reg))
}

/// [`average_market_prices`] without the copy: the kept answer itself. The
/// slot's mutex is held only to compare the revision and bump the `Arc`.
pub fn average_market_prices_shared(reg: &Registry) -> std::sync::Arc<PyValue> {
    // The answer moves only with the price table (fixed per world) and the
    // `mktidx` rows, so it is kept under that kind's revision.
    let rev = reg.deltas().kind_revision(KIND);
    let slot = &reg.ext::<PriceCache>().0;
    let hit = match slot.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some((r, v)) if *r == rev => Some(std::sync::Arc::clone(v)),
        _ => None,
    };
    if let Some(v) = hit {
        return v;
    }
    let v = std::sync::Arc::new(compute_average_market_prices(reg));
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((rev, std::sync::Arc::clone(&v)));
    v
}

/// The kept [`average_market_prices`] answer and the `mktidx` revision it
/// was computed at.
#[derive(Debug, Default)]
pub struct PriceCache(std::sync::Mutex<Option<(u64, std::sync::Arc<PyValue>)>>);

/// Registered in `world_ext::SLOTS`; builds empty.
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "econ_prices_cache",
    make: |_| Box::new(PriceCache::default()) as crate::world_ext::Boxed,
};

fn compute_average_market_prices(reg: &Registry) -> PyValue {
    let table = reg.content().get::<Table>();
    PyValue::Dict(
        table
            .prices
            .iter()
            .map(|(&id, p)| {
                let avg = average(reg, id).unwrap_or(p.ref_price);
                (
                    keyval(vec![
                        ("adjustedPrice", PyValue::Real(p.ref_price)),
                        ("averagePrice", PyValue::Real(avg)),
                    ]),
                    PyValue::Int(id),
                )
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::model::SandboxConfig;

    fn world() -> Registry {
        Registry::new(SandboxConfig::default())
    }

    fn with_table(table: Table) -> Registry {
        let mut c = Content::fixture();
        c.insert("econ_prices", table);
        Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(c))
    }

    /// The fixture's first type, and its `P_ref`.
    fn first() -> (i64, f64) {
        let t = Table::fixture();
        let (&id, p) = t.prices.iter().next().expect("the fixture has rows");
        (id, p.ref_price)
    }

    /// PERF-HOLDS: the kept answer is the computed one before and after a
    /// `mktidx` write, and an unrelated write leaves it standing.
    #[test]
    fn the_kept_answer_follows_the_index_rows() {
        let (id, _) = first();
        let mut reg = with_table(Table::fixture());
        let before = average_market_prices(&reg);
        assert_eq!(before, compute_average_market_prices(&reg));
        let other = crate::delta_kinds::KINDS
            .iter()
            .map(|k| k.0)
            .find(|k| *k != KIND)
            .expect("another kind");
        reg.deltas_mut()
            .put_verbatim(other, vec!["1".to_owned()], vec![]);
        assert_eq!(average_market_prices(&reg), before);
        assert!(std::sync::Arc::ptr_eq(
            &average_market_prices_shared(&reg),
            &average_market_prices_shared(&reg)
        ));
        fold_day(&mut reg, id, first().1 * 2.0, 5).expect("folds");
        let after = average_market_prices(&reg);
        assert_ne!(after, before);
        assert_eq!(after, compute_average_market_prices(&reg));
    }

    /// **CP31**: the answer is a dict keyed by type id whose values are
    /// `KeyVal`s carrying `adjustedPrice` and `averagePrice` [V names,
    /// `typeHelpers.py:106-118`], read by attribute.
    #[test]
    fn a_dict_of_keyval_with_adjusted_price_and_average_price() {
        let reg = world();
        let PyValue::Dict(rows) = average_market_prices(&reg) else {
            panic!("a dict")
        };
        let table = Table::fixture();
        assert_eq!(rows.len(), table.prices.len(), "one entry per priced type");
        for (value, key) in &rows {
            // `(value, key)`: marshal dict entries are inverted.
            let PyValue::Int(id) = key else {
                panic!("keyed by type id: {key:?}")
            };
            let PyValue::Instance { state, .. } = value else {
                panic!("a KeyVal, read by attribute: {value:?}")
            };
            let want = table.prices[id].ref_price;
            for attr in [&b"adjustedPrice"[..], b"averagePrice"] {
                assert_eq!(
                    crate::pyutil::dict_get(state, attr).cloned(),
                    Some(PyValue::Real(want)),
                    "an untraded type's two prices are both P_ref"
                );
            }
        }
    }

    /// **CP31**: an empty table answers `{}`, the pre-CP31 answer, which is
    /// what the server with no bundle serves.
    #[test]
    fn empty_with_an_empty_table() {
        let reg = with_table(Table::default());
        assert_eq!(average_market_prices(&reg), PyValue::Dict(vec![]));
        let src_dir = crate::authored::Dir::compiled_only();
        let src = Sources::new(None, &src_dir);
        let loaded = (TABLE.load)(&src).expect("loads");
        assert_eq!(
            loaded.downcast_ref::<Table>(),
            Some(&Table::default()),
            "no bundle loads no invented prices"
        );
    }

    /// **CP31**: the EWMA stays inside `CLAMP × P_ref` whatever the day
    /// means are, and moves toward the mean.
    #[test]
    fn the_ewma_stays_inside_its_clamp() {
        let (id, p) = first();
        let mut reg = world();
        let means = [p * 1e6, p * 1e-6, p, p * 3.0, p * 0.1, f64::MAX];
        for (day, &m) in means.iter().enumerate() {
            let before = average(&reg, id).expect("priced");
            let after = fold_day(&mut reg, id, m, day as u32 + 1).expect("folded");
            assert!(
                (CLAMP.0 * p..=CLAMP.1 * p).contains(&after),
                "{after} outside the clamp of {p}"
            );
            assert_eq!(average(&reg, id), Some(after));
            assert!(
                (after - before) * (m - before) >= 0.0,
                "moves toward the mean"
            );
        }
        // A stored value outside the band (a hand-edited world) reads clamped.
        reg.deltas_mut().put(
            KIND,
            vec![id.to_string()],
            vec![(p * 100.0).to_string(), "1".into()],
        );
        assert_eq!(average(&reg, id), Some(CLAMP.1 * p));
    }

    /// **CP31**: the `mktidx` row round-trips: another world on the same
    /// rows reads the same index; a repeated day, an unpriced type and a bad
    /// mean write nothing; an unreadable row is no row.
    #[test]
    fn the_mktidx_delta_round_trips() {
        let (id, p) = first();
        let mut reg = world();
        assert_eq!(average(&reg, id), Some(p), "absent means P_ref");
        let avg = fold_day(&mut reg, id, p * 1.5, 7).expect("folded");
        assert_eq!(avg, ewma(p, p * 1.5, p));
        assert_eq!(fold_day(&mut reg, id, p, 7), None, "one fold per day");
        assert_eq!(fold_day(&mut reg, id, p, 6), None, "never backwards");
        assert_eq!(fold_day(&mut reg, -1, p, 8), None, "unpriced");
        assert_eq!(fold_day(&mut reg, id, f64::NAN, 8), None);
        assert_eq!(reg.deltas().of(KIND).len(), 1);

        let mut again = world();
        for (k, v) in reg.deltas().of(KIND).clone() {
            again.deltas_mut().put(KIND, k, v);
        }
        assert_eq!(average(&again, id), Some(avg), "bit-exact");
        assert_eq!(average_market_prices(&again), average_market_prices(&reg));

        again
            .deltas_mut()
            .put(KIND, vec![id.to_string()], vec!["x".into()]);
        assert_eq!(average(&again, id), Some(p), "unreadable is absent");
    }

    /// A bad row names its line; the fixture's rows are the schema's shape.
    #[test]
    fn a_bad_row_is_refused() {
        assert!(Table::parse(&["1\t2.0\trecipe\t1.0\t6\t9\t\t1"]).is_ok());
        for bad in [
            "1\t2.0\tmagic\t1.0\t6\t9\t\t1",
            "1\tx\trecipe\t1.0\t6\t9\t\t1",
            "1\t0\trecipe\t1.0\t6\t9\t\t1",
            "1\t2.0\trecipe\t1.0\t6\t9\t",
        ] {
            let e = Table::parse(&[bad]).expect_err(bad);
            assert!(e.contains("data line 1"), "{e}");
        }
    }

    const BUYER: i64 = 90_000_201;
    const SELLER: i64 = 90_000_202;
    const OTHER: i64 = 90_000_203;
    const NPC_CORP: i64 = 1_000_007;
    const PLAYER_CORP: i64 = 98_000_001;
    const ALLIANCE: i64 = 99_000_001;

    /// Three characters; BUYER and SELLER share `corp` and `alliance`.
    fn with_pilots(corp: i64, alliance: Option<i64>) -> Registry {
        let pilot = |id: i64, corp: i64, alliance: Option<i64>| crate::model::Character {
            character_id: id,
            ship_id: id + 10_000_000,
            corporation_id: corp,
            alliance_id: alliance,
            ..crate::model::Character::default()
        };
        crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(BUYER, corp, alliance),
                pilot(SELLER, corp, alliance),
                pilot(OTHER, PLAYER_CORP + 1, None),
            ];
        })
    }

    /// **FE-GUARD-A**: a fill between two members of one player corporation
    /// or alliance, or from a character to itself, is a wash; two pilots of
    /// one NPC corporation, strangers and NPC sides are not.
    #[test]
    fn same_group_is_a_player_corp_an_alliance_or_oneself() {
        let reg = with_pilots(PLAYER_CORP, None);
        assert!(same_group(&reg, BUYER, SELLER));
        assert!(same_group(&reg, BUYER, BUYER));
        assert!(!same_group(&reg, BUYER, OTHER));
        assert!(!same_group(&reg, BUYER, 1_000_001), "an NPC side");
        let npc = with_pilots(NPC_CORP, None);
        assert!(
            !same_group(&npc, BUYER, SELLER),
            "NPC corp members: strangers"
        );
        let allied = with_pilots(NPC_CORP, Some(ALLIANCE));
        assert!(same_group(&allied, BUYER, SELLER), "one alliance");
    }

    /// **FE-GUARD-A**: a wash fill moves no index and an outlier neither;
    /// an honest fill does; the fills themselves are untouched (the fold of
    /// `mkthist` and the Tier B multipliers still read every fill).
    #[test]
    fn wash_fill_does_not_move_index() {
        let (id, p) = first();
        let mut reg = with_pilots(PLAYER_CORP, None);
        assert!(counts_for_index(&reg, id, BUYER, OTHER, p * 1.4));
        assert!(!counts_for_index(&reg, id, BUYER, SELLER, p * 1.4), "wash");
        assert!(!counts_for_index(
            &reg,
            id,
            BUYER,
            OTHER,
            p * WASH_BAND * 1.01
        ));
        assert!(!counts_for_index(
            &reg,
            id,
            BUYER,
            OTHER,
            p / WASH_BAND / 1.01
        ));
        assert!(counts_for_index(
            &reg,
            id,
            BUYER,
            OTHER,
            p * WASH_BAND * 0.99
        ));
        // The band follows the index, not P_ref: after the index rises, the
        // same price is no outlier.
        fold_day(&mut reg, id, p * 2.0, 1).expect("folded");
        let prior = average(&reg, id).expect("priced");
        assert!(!counts_for_index(&reg, id, BUYER, OTHER, prior * 3.5));
        assert!(counts_for_index(&reg, id, BUYER, OTHER, prior * 2.5));
        // Through the rollover: one honest fill and one huge wash fill.
        let mut reg = with_pilots(PLAYER_CORP, None);
        let station = 60_000_001;
        let fill = |buyer, seller, price| crate::econ_exchange::Fill {
            date: 1,
            station_id: station,
            type_id: id,
            quantity: 10,
            price,
            buyer_id: buyer,
            seller_id: seller,
        };
        reg.seed_market_fills(vec![
            fill(BUYER, OTHER, p * 1.2),
            fill(BUYER, SELLER, p * 100.0),
        ]);
        let day = crate::rollover::Day {
            day: 1,
            days: 1,
            now: 2,
        };
        crate::econ_exchange::on_rollover(&mut reg, &day);
        assert_eq!(
            average(&reg, id),
            Some(ewma(p, p * 1.2, p)),
            "only the honest fill"
        );
        assert_eq!(
            reg.market_fills_since(0).len(),
            2,
            "the fills are all still there"
        );
    }

    /// **FE-GUARD-A**: the clamp is `CLAMP` and the step is `ALPHA`, both
    /// consts, so the band is one edit.
    #[test]
    fn index_clamp_wide_band() {
        let (id, p) = first();
        let mut reg = world();
        let up = fold_day(&mut reg, id, p * 1e9, 1).expect("folded");
        assert_eq!(up, CLAMP.1 * p);
        assert_eq!(ewma(p, 2.0 * p, p), p + ALPHA * p);
        assert_eq!(ewma(p, 0.0, p), (p - ALPHA * p).max(CLAMP.0 * p));
    }
}
