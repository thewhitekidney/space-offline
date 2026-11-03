//! **DOC-7: the parts the leaders' doctrines want, and who can make them**
//! (R-DOCTRINE 8.4, U4; R-FLUIDECON FE-BUILD).
//!
//! - [`Sources`] answers "where does this part come from?" for **nothing from
//!   nowhere**: an NPC station sells it, a builder makes it from a recipe
//!   whose original is on sale (or is invented) and whose inputs have a
//!   source ([`crate::builder::Sourcing`]), or it is dropped. A part with
//!   none has no source ([`Sources::source_of`] gives [`Source::None`]).
//!   [`makeable`] is the doctrine catalog's `Makeable` set over it, and
//!   [`unsourced`] names the parts a wanted doctrine needs that nothing
//!   can supply, which the builder logs ("parts with no source").
//! - [`wanted_parts`] is what one doctrine's fleet needs, by part type, and
//!   [`PartsBoard`] is the shared board the leaders publish it to (one row
//!   per leader) and the builders read: the **demand map by part**. It also
//!   keeps the bid-pressure streaks that pull a rung ahead of the ladder
//!   ([`crate::build_choice::pulls`]) and the parts already logged.
//! - [`doctrine_rungs`] turns the wanted parts into ladder rungs (one per
//!   part, the blueprints whose product it is), most wanted first, leaving
//!   out a part no source can make and the blueprints the other rungs hold.
//!
//! Registered in the content tables as `parts_board`, beside
//! `rare_board` (`eve-bots`). The leaders' side is [`publish_from_hall`].
//!
//! Every number is **[A]**; falsifiers on the consts.

use crate::builder::{Sourcing, invented_types};
use crate::doctrine::{Doctrine, Makeable};
use eve_server::industry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// Units of a round one ship of a doctrine is counted as needing: its
/// fitted load **[A]** (a fleet's wanted rounds are ships x rounds fitted).
/// Falsified by a round market that never clears while doctrines want
/// thousands (the load is an upper bound on a day's use).
pub const WANTED_ROUNDS_MAX: i64 = 5_000;

/// Where a part comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// An NPC station sells the part itself.
    Sold,
    /// A builder makes it from this blueprint, whose original is on sale.
    Made(i64),
    /// A builder makes it from this blueprint, which an invention yields.
    Invented(i64),
    /// A builder makes it from this blueprint, whose copies the world gives
    /// up in data sites (a hacked can's blueprint copy): found, not sold.
    Copy(i64),
    /// Dropped by the world (loot): taken as given.
    Dropped,
    /// Nothing supplies it.
    None,
}

/// What the world offers, read once from the content tables.
#[derive(Debug, Clone, Copy)]
pub struct Sources<'a> {
    pub recipes: &'a industry::Table,
    /// Types some NPC station sells (`econ_exchange::Table::asked_types`);
    /// `None`: the exchange is unknown, every original counts as on sale.
    pub sold: Option<&'a BTreeSet<i64>>,
    /// Types the world drops.
    pub dropped: Option<&'a BTreeSet<i64>>,
    /// Blueprints the world gives up as copies (data-site loot,
    /// [`copies_of`]); `None`: unknown, none counts.
    pub copies: Option<&'a BTreeSet<i64>>,
    /// The inputs' sources; `None`: not asked.
    pub sourcing: Option<&'a Sourcing<'a>>,
}

impl Sources<'_> {
    fn original_on_sale(&self, bp: i64) -> bool {
        self.sold.is_none_or(|s| s.contains(&bp))
    }

    fn fed(&self, bp: &industry::Blueprint) -> bool {
        self.sourcing.is_none_or(|s| s.blocked(bp).is_empty())
    }

    /// The first source of `part`, in the order NPC sale, build, invention,
    /// found copy, drop.
    pub fn source_of(&self, part: i64) -> Source {
        if self.sold.is_some_and(|s| s.contains(&part)) {
            return Source::Sold;
        }
        let invented = invented_types(self.recipes);
        for bp in self
            .recipes
            .blueprints
            .iter()
            .filter(|b| b.product_type_id == part)
        {
            if !self.fed(bp) {
                continue;
            }
            if self.original_on_sale(bp.blueprint_type_id) && self.sold.is_some() {
                return Source::Made(bp.blueprint_type_id);
            }
            if invented.contains(&bp.blueprint_type_id) {
                return Source::Invented(bp.blueprint_type_id);
            }
            if self.sold.is_none() {
                return Source::Made(bp.blueprint_type_id);
            }
            if self
                .copies
                .is_some_and(|c| c.contains(&bp.blueprint_type_id))
            {
                return Source::Copy(bp.blueprint_type_id);
            }
        }
        if self.dropped.is_some_and(|d| d.contains(&part)) {
            return Source::Dropped;
        }
        Source::None
    }

    pub fn has_source(&self, part: i64) -> bool {
        self.source_of(part) != Source::None
    }
}

/// **`Makeable`** (DOC-1): of every part some recipe makes or some NPC
/// sells or the world drops, those with a source.
pub fn makeable(src: &Sources<'_>) -> Makeable {
    let mut universe: BTreeSet<i64> = src
        .recipes
        .blueprints
        .iter()
        .map(|b| b.product_type_id)
        .collect();
    universe.extend(src.sold.into_iter().flatten().copied());
    universe.extend(src.dropped.into_iter().flatten().copied());
    Makeable::of(universe.into_iter().filter(|&p| src.has_source(p)))
}

/// The wanted parts no source can supply, ascending: what a doctrine needs
/// and the world cannot make ("parts with no source").
pub fn unsourced(wanted: &BTreeMap<i64, i64>, src: &Sources<'_>) -> Vec<i64> {
    wanted
        .keys()
        .copied()
        .filter(|&p| !src.has_source(p))
        .collect()
}

/// **What a fleet of the doctrine needs**, by part type: each role's share
/// of the target fleet, times the hull (1), the modules fitted and the
/// rounds carried (at most [`WANTED_ROUNDS_MAX`] a ship).
pub fn wanted_parts(d: &Doctrine) -> BTreeMap<i64, i64> {
    let mut out: BTreeMap<i64, i64> = BTreeMap::new();
    for (_, share, fit) in &d.roles {
        let ships = (share * d.fleet.target as f64).ceil().max(1.0) as i64;
        *out.entry(fit.hull).or_insert(0) += ships;
        for &(m, n) in &fit.modules {
            *out.entry(m).or_insert(0) += ships * i64::from(n);
        }
        for &(c, rounds) in &fit.charges {
            *out.entry(c).or_insert(0) += ships * i64::from(rounds).min(WANTED_ROUNDS_MAX);
        }
    }
    out
}

/// **The ladder rungs the wanted parts make** (like
/// [`crate::hulls::warship_rungs`]): for each wanted part with a source,
/// the blueprints whose product it is, one rung per part, the most wanted
/// first (then by type id). Blueprints in `held_by` (the other rungs) are
/// left out, and so is a rung none of whose blueprints has a recipe.
pub fn doctrine_rungs(
    recipes: &industry::Table,
    wanted: &BTreeMap<i64, i64>,
    src: &Sources<'_>,
    held_by: &BTreeSet<i64>,
) -> Vec<Vec<i64>> {
    let mut parts: Vec<(i64, i64)> = wanted
        .iter()
        .filter(|&(_, &n)| n > 0)
        .map(|(&p, &n)| (p, n))
        .collect();
    parts.sort_by_key(|&(p, n)| (std::cmp::Reverse(n), p));
    parts
        .into_iter()
        // A part only found as a copy, or only invented, has no original to buy
        // (an `Invented` source is never one on sale): its rung would lack
        // forever. The copy is built when it is held (`t2_candidates`).
        .filter(|&(p, _)| {
            src.has_source(p) && !matches!(src.source_of(p), Source::Copy(_) | Source::Invented(_))
        })
        .map(|(p, _)| {
            let mut r: Vec<i64> = recipes
                .blueprints
                .iter()
                .filter(|b| b.product_type_id == p && !held_by.contains(&b.blueprint_type_id))
                .map(|b| b.blueprint_type_id)
                .collect();
            r.sort_unstable();
            r.dedup();
            r
        })
        .filter(|r| !r.is_empty())
        .collect()
}

#[derive(Debug, Default)]
struct Inner {
    /// The parts each leader wants, by leader group.
    wanted: BTreeMap<i64, BTreeMap<i64, i64>>,
    /// Consecutive cycles a product's bid pressure was at or over the pull.
    streak: BTreeMap<i64, u32>,
    /// Parts already reported as having no source.
    logged: BTreeSet<i64>,
    /// Types some NPC station sells, read from the exchange once.
    sold: Option<Arc<BTreeSet<i64>>>,
    /// Copies data sites give up and types NPCs drop, read once.
    found: Option<Arc<(BTreeSet<i64>, BTreeSet<i64>)>>,
}

/// **The demand map by part**, shared by the leaders (who publish) and the
/// builders (who read).
#[derive(Debug, Default)]
pub struct PartsBoard {
    inner: Mutex<Inner>,
}

impl PartsBoard {
    pub fn new() -> PartsBoard {
        PartsBoard::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A leader's wanted parts (replacing its last); empty clears it.
    pub fn publish(&self, leader: i64, parts: BTreeMap<i64, i64>) {
        let mut g = self.lock();
        if parts.is_empty() {
            g.wanted.remove(&leader);
        } else {
            g.wanted.insert(leader, parts);
        }
    }

    /// Every leader's wanted units, summed by part.
    pub fn wanted(&self) -> BTreeMap<i64, i64> {
        let g = self.lock();
        let mut out: BTreeMap<i64, i64> = BTreeMap::new();
        for parts in g.wanted.values() {
            for (&p, &n) in parts {
                *out.entry(p).or_insert(0) += n;
            }
        }
        out
    }

    /// Units wanted of one part.
    pub fn wanted_of(&self, part: i64) -> i64 {
        self.lock()
            .wanted
            .values()
            .map(|m| m.get(&part).copied().unwrap_or(0))
            .sum()
    }

    /// Note one cycle's reading of a product: `hot` (its bid pressure is at
    /// or over [`crate::build_choice::FE_DEMAND_BUY`]) extends its streak,
    /// else resets it. Returns the streak.
    pub fn observe(&self, product: i64, hot: bool) -> u32 {
        let mut g = self.lock();
        if hot {
            let s = g.streak.entry(product).or_insert(0);
            *s += 1;
            *s
        } else {
            g.streak.remove(&product);
            0
        }
    }

    pub fn streak_of(&self, product: i64) -> u32 {
        self.lock().streak.get(&product).copied().unwrap_or(0)
    }

    /// The types an NPC station sells: `read` runs the first time only (the
    /// exchange's walk is not for every builder's every cycle).
    pub fn sold_with(&self, read: impl FnOnce() -> BTreeSet<i64>) -> Arc<BTreeSet<i64>> {
        let mut g = self.lock();
        Arc::clone(g.sold.get_or_insert_with(|| Arc::new(read())))
    }

    /// The `(copies, drops)` the world gives up ([`copies_of`],
    /// [`dropped_of`]): `read` runs the first time only.
    pub fn found_with(
        &self,
        read: impl FnOnce() -> (BTreeSet<i64>, BTreeSet<i64>),
    ) -> Arc<(BTreeSet<i64>, BTreeSet<i64>)> {
        let mut g = self.lock();
        Arc::clone(g.found.get_or_insert_with(|| Arc::new(read())))
    }

    /// Of `parts`, those not yet reported (and now reported): each part with
    /// no source is logged once.
    pub fn first_report(&self, parts: &[i64]) -> Vec<i64> {
        let mut g = self.lock();
        parts
            .iter()
            .copied()
            .filter(|p| g.logged.insert(*p))
            .collect()
    }
}

/// What a builder reads of the content for the parts layer: the board
/// (absent in a world with no leaders) and the NPC-sold set.
pub struct Ctx<'a> {
    pub board: &'a PartsBoard,
    pub sold: Option<Arc<BTreeSet<i64>>>,
    /// Blueprint copies found in data sites, and types NPCs drop.
    pub found: Arc<(BTreeSet<i64>, BTreeSet<i64>)>,
}

impl Ctx<'_> {
    /// The sources the content offers, for the builders' reading.
    pub fn sources<'a>(
        &'a self,
        recipes: &'a industry::Table,
        sourcing: Option<&'a Sourcing<'a>>,
    ) -> Sources<'a> {
        Sources {
            recipes,
            sold: self.sold.as_deref(),
            dropped: Some(&self.found.1),
            copies: Some(&self.found.0),
            sourcing,
        }
    }
}

/// The parts layer of `content`: the registered [`PartsBoard`] and, when the
/// exchange table is there, what its stations sell.
pub fn ctx_of(content: &eve_server::content::Content) -> Option<Ctx<'_>> {
    let board = content.try_get::<PartsBoard>()?;
    let sold = content
        .try_get::<eve_server::econ_exchange::Table>()
        .map(|t| board.sold_with(|| t.asked_types()));
    let found = board.found_with(|| (copies_of(content), dropped_of(content)));
    Some(Ctx { board, sold, found })
}

/// **Publish what each leader's doctrine wants**: for every leader of the
/// hall with a current doctrine, its [`wanted_parts`] under its group. The
/// wiring (one call after the hall's daily tick) is the coordinator's.
pub fn publish_from_hall(board: &PartsBoard, hall: &crate::corpmind::HallState) {
    let Some(world) = hall.doctrine.as_ref() else {
        return;
    };
    for leader in hall.doctrine_leaders() {
        let key = hall
            .doctrine_of(leader.corps.first().copied().unwrap_or(0))
            .and_then(|b| b.current.as_deref());
        let parts = key
            .and_then(|k| world.find(k))
            .map(|i| wanted_parts(&world.catalog[i]))
            .unwrap_or_default();
        board.publish(leader.group, parts);
    }
}

/// **The blueprints data sites give up as copies**: the copy rows of the
/// data cans' loot (`explore/loot.tsv`, [`hacking_loot::Entry::copy`]).
pub fn copies_of(content: &eve_server::content::Content) -> BTreeSet<i64> {
    use eve_server::hacking_loot::{Kind, Table};
    content
        .try_get::<Table>()
        .map(|t| {
            t.entries
                .iter()
                .filter(|e| e.copy && e.kind == Kind::Data)
                .map(|e| e.type_id)
                .collect()
        })
        .unwrap_or_default()
}

/// **The types NPCs drop** (`npc/loot.tsv`'s pools).
pub fn dropped_of(content: &eve_server::content::Content) -> BTreeSet<i64> {
    content
        .try_get::<eve_server::npc_data::Table>()
        .map(|t| t.loot.iter().map(|l| l.type_id).collect())
        .unwrap_or_default()
}

/// The log line for parts with no source.
pub fn unsourced_line(parts: &[i64]) -> String {
    format!("doc: parts with no source: {parts:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_server::industry::Blueprint;

    const PLATE: i64 = 991_200_001;
    const EXTENDER: i64 = 991_200_002;
    const GUN: i64 = 991_200_003;
    const GHOST: i64 = 991_200_004;
    const INPUT: i64 = 991_200_101;
    const fn bp(p: i64) -> i64 {
        p + 1_000
    }

    fn recipe(product: i64) -> Blueprint {
        Blueprint {
            blueprint_type_id: bp(product),
            product_type_id: product,
            product_quantity: 1,
            time: 600,
            max_production_limit: 10,
            materials: vec![(INPUT, 5)],
            skills: vec![],
        }
    }

    fn table(products: &[i64]) -> industry::Table {
        let mut blueprints: Vec<Blueprint> = products.iter().map(|&p| recipe(p)).collect();
        blueprints.sort_by_key(|b| b.blueprint_type_id);
        industry::Table {
            blueprints,
            ..industry::Table::default()
        }
    }

    fn src<'a>(
        recipes: &'a industry::Table,
        sold: &'a BTreeSet<i64>,
        dropped: &'a BTreeSet<i64>,
    ) -> Sources<'a> {
        Sources {
            recipes,
            sold: Some(sold),
            dropped: Some(dropped),
            copies: None,
            sourcing: None,
        }
    }

    #[test]
    fn makeable_follows_recipes_and_sellers() {
        let recipes = table(&[PLATE, EXTENDER, GHOST]);
        // The plate's original and a gun are sold; the extender has a recipe
        // but nobody sells its original; the ghost likewise.
        let sold = BTreeSet::from([bp(PLATE), GUN]);
        let dropped = BTreeSet::new();
        let s = src(&recipes, &sold, &dropped);
        assert_eq!(s.source_of(PLATE), Source::Made(bp(PLATE)));
        assert_eq!(s.source_of(GUN), Source::Sold);
        assert_eq!(s.source_of(EXTENDER), Source::None);
        let m = makeable(&s);
        assert!(m.has(PLATE) && m.has(GUN));
        assert!(!m.has(EXTENDER) && !m.has(GHOST));
        // A seller appearing makes the part makeable; a drop does too.
        let sold = BTreeSet::from([bp(PLATE), bp(EXTENDER)]);
        let dropped = BTreeSet::from([GHOST]);
        let m = makeable(&src(&recipes, &sold, &dropped));
        assert!(m.has(EXTENDER) && m.has(GHOST));
        // No exchange known: every recipe is makeable.
        let open = Sources {
            recipes: &recipes,
            sold: None,
            dropped: None,
            copies: None,
            sourcing: None,
        };
        assert!(makeable(&open).has(GHOST));
    }

    #[test]
    fn a_wanted_part_climbs_the_ladder() {
        let recipes = table(&[PLATE, EXTENDER]);
        let sold = BTreeSet::from([bp(PLATE), bp(EXTENDER)]);
        let dropped = BTreeSet::new();
        let s = src(&recipes, &sold, &dropped);
        let wanted = BTreeMap::from([(PLATE, 40), (EXTENDER, 90)]);
        let rungs = doctrine_rungs(&recipes, &wanted, &s, &BTreeSet::new());
        assert_eq!(
            rungs,
            vec![vec![bp(EXTENDER)], vec![bp(PLATE)]],
            "most wanted first"
        );
        // A blueprint another rung holds is not repeated.
        let held = BTreeSet::from([bp(EXTENDER)]);
        assert_eq!(
            doctrine_rungs(&recipes, &wanted, &s, &held),
            vec![vec![bp(PLATE)]]
        );
        // Through the ladder: the rung is lacking until the builder holds it,
        // and it is a rung like any other.
        let lacking = crate::builder::lacking_rungs(
            &rungs.iter().map(Vec::as_slice).collect::<Vec<_>>(),
            &[bp(EXTENDER)],
            &recipes,
        );
        assert_eq!(lacking, vec![vec![bp(PLATE)]]);
    }

    #[test]
    fn no_rung_for_an_unmakeable_part() {
        let recipes = table(&[PLATE, GHOST]);
        let sold = BTreeSet::from([bp(PLATE)]);
        let dropped = BTreeSet::new();
        let s = src(&recipes, &sold, &dropped);
        let wanted = BTreeMap::from([(PLATE, 10), (GHOST, 50), (GUN, 5)]);
        let rungs = doctrine_rungs(&recipes, &wanted, &s, &BTreeSet::new());
        assert_eq!(rungs, vec![vec![bp(PLATE)]]);
        assert_eq!(unsourced(&wanted, &s), vec![GUN, GHOST]);
        assert!(unsourced_line(&[GHOST]).contains("no source"));
    }

    /// A part whose blueprint is only invented has no original to buy: no
    /// rung (a real dictor), unless its original is on sale.
    #[test]
    fn an_invented_part_has_no_rung_unless_its_original_is_sold() {
        let mut recipes = table(&[PLATE, EXTENDER]);
        recipes.science.push(industry::Science {
            blueprint_type_id: 4_001,
            research_material_time: None,
            research_time_time: None,
            copying_time: None,
            invention: Some(industry::Invention {
                time: 600,
                materials: vec![],
                products: vec![industry::InventionProduct {
                    type_id: bp(PLATE),
                    quantity: 1,
                    probability: 0.3,
                }],
                skills: vec![],
            }),
        });
        let sold = BTreeSet::from([bp(EXTENDER)]);
        let dropped = BTreeSet::new();
        let s = src(&recipes, &sold, &dropped);
        assert_eq!(s.source_of(PLATE), Source::Invented(bp(PLATE)));
        let wanted = BTreeMap::from([(PLATE, 40), (EXTENDER, 10)]);
        let rungs = doctrine_rungs(&recipes, &wanted, &s, &BTreeSet::new());
        assert_eq!(rungs, vec![vec![bp(EXTENDER)]]);
        // On sale, the original is bought like any other.
        let sold = BTreeSet::from([bp(PLATE), bp(EXTENDER)]);
        let s = src(&recipes, &sold, &dropped);
        assert_eq!(s.source_of(PLATE), Source::Made(bp(PLATE)));
        let rungs = doctrine_rungs(&recipes, &wanted, &s, &BTreeSet::new());
        assert_eq!(rungs, vec![vec![bp(PLATE)], vec![bp(EXTENDER)]]);
    }

    #[test]
    fn the_board_sums_leaders_and_logs_each_gap_once() {
        let b = PartsBoard::new();
        b.publish(1, BTreeMap::from([(PLATE, 10)]));
        b.publish(2, BTreeMap::from([(PLATE, 5), (GUN, 1)]));
        assert_eq!(b.wanted_of(PLATE), 15);
        b.publish(1, BTreeMap::new());
        assert_eq!(b.wanted(), BTreeMap::from([(PLATE, 5), (GUN, 1)]));
        assert_eq!(b.first_report(&[GHOST, GUN]), vec![GHOST, GUN]);
        assert!(b.first_report(&[GHOST]).is_empty());
        assert_eq!(b.observe(PLATE, true), 1);
        assert_eq!(b.observe(PLATE, true), 2);
        assert_eq!(b.observe(PLATE, false), 0);
        assert_eq!(b.streak_of(PLATE), 0);
    }

    #[test]
    fn a_copy_found_in_a_site_makes_its_product_makeable() {
        let recipes = table(&[PLATE, GHOST]);
        // Nobody sells either original; a data site gives up the plate's copy.
        let sold = BTreeSet::new();
        let dropped = BTreeSet::new();
        let found = BTreeSet::from([bp(PLATE)]);
        let mut s = src(&recipes, &sold, &dropped);
        assert_eq!(s.source_of(PLATE), Source::None);
        s.copies = Some(&found);
        assert_eq!(s.source_of(PLATE), Source::Copy(bp(PLATE)));
        assert_eq!(s.source_of(GHOST), Source::None);
        let m = makeable(&s);
        assert!(m.has(PLATE) && !m.has(GHOST));
        // A sale still comes first.
        let sold = BTreeSet::from([bp(PLATE)]);
        let s = Sources {
            sold: Some(&sold),
            ..s
        };
        assert_eq!(s.source_of(PLATE), Source::Made(bp(PLATE)));
    }
}
