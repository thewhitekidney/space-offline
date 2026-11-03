//! **The miner's moon preference** (R-MOON packet MN-G, the moon miner).
//!
//! There is no moon-miner career and no new miner: a bot that mines is a
//! miner, and one decision point of the one miner changes. [`brain::pick_rock`]
//! chooses the rock a miner claims among the free rocks nearest it; with this
//! module's [`prefer_moon`] in front of that choice, a free **moon belt rock**
//! (MN-C, `eve_server::moon_belt`) beats every ordinary rock in the park.
//!
//! **Why a miner would.** A moon ore is an ordinary rock to the laser (every
//! moon ore asks the Mining skill, attribute 182 = 3386, **[V-data]**, R-MOON
//! §1.4), and refined it gives the moon materials the reactions eat plus the
//! minerals a builder wants, which no ordinary ore of the same volume does
//! (`types/materials.tsv`). **[A]** that a player with a Miner I would warp to
//! a lit moon when one is in the overview: the number is not the point, the
//! direction is. Falsified by the soak's `[econ]` showing moon rocks mined by
//! nobody (the preference never reaches the park: the belt is not laid out in
//! the bots' systems) or by every miner on one lit moon while ordinary belts
//! sit full (then the preference should leash on distance).
//!
//! **"Lit nearby"** is "in the park": the server lays a moon belt out only
//! while its window is open (`moon_belt::live_moons`, a function of the seed,
//! the system and the game day) and puts its rocks in the pilots' ball park
//! of that system, so a rock of the moon id band in [`crate::park`] *is* a
//! lit belt.
//!
//! **Where the scan comes in (MN-D).** A bot has no survey launcher, and
//! nothing is handed to it from nowhere; but a moon rock's slim names its ore,
//! and the ores in a park are exactly what a scan of the moon beside it would
//! list. So [`scanned`] reads them the way a pilot reads the Moon Analysis
//! window: among the free moon rocks it keeps the ones of the **rarest** ore
//! on offer, and the miner takes those. A rare ore is the one reactions want,
//! and the one nobody else can make **[A]** (R-MOON §1.4); falsified by the
//! soak's `[econ]` showing the rare goo in glut while the common sits unmined,
//! or every miner on one belt while it is mined out (the belts are finite, so
//! the next rarity follows).
//!
//! [`brain::pick_rock`]: crate::brain::pick_rock

use crate::Tracked;
use eve_server::content::Content;
use eve_server::moon::OreTable;
use eve_server::moon_belt::is_moon_id;
use std::collections::BTreeMap;

/// **The rank of an ore** (pure): `0` for a type the table does not know, else
/// one more than its rarity's index, so a rarer ore ranks higher.
pub fn rank_of(table: &OreTable, type_id: i64) -> usize {
    table
        .ores_by_type(type_id)
        .and_then(|o| table.rarity_of(o.basic))
        .map_or(0, |r| r.index() + 1)
}

/// **Which free moon rocks to leave alone** (pure): from `(rock id, rank)` for
/// every free moon rock in view, the ids whose rank is below the best.
pub fn below_the_best(free: &[(i64, usize)]) -> Vec<i64> {
    let best = free.iter().map(|(_, r)| *r).max().unwrap_or(0);
    free.iter()
        .filter(|(_, r)| *r < best)
        .map(|(id, _)| *id)
        .collect()
}

/// **The rocks a miner is shown**: `rocks` with the free moon rocks of a lesser
/// ore taken out, when a rarer one is free in the park. Rocks under another
/// laser (`taken`) are left in, as the brain's own test drops them; a world
/// with no moon ore in its type data, or with no moon rock in view, comes back
/// unchanged. The ore table is built only when a moon rock is in view.
pub fn scanned<'a>(
    rocks: Vec<&'a Tracked>,
    taken: &BTreeMap<i64, i64>,
    content: Option<&Content>,
) -> Vec<&'a Tracked> {
    if !rocks
        .iter()
        .any(|r| is_moon_id(r.ball_id) && !taken.contains_key(&r.ball_id))
    {
        return rocks;
    }
    let Some(installed) = content.and_then(|c| c.try_get::<eve_server::typedata::Installed>())
    else {
        return rocks;
    };
    let table = OreTable::from_types(installed.table().types());
    if table.is_empty() {
        return rocks;
    }
    let free: Vec<(i64, usize)> = rocks
        .iter()
        .filter(|r| is_moon_id(r.ball_id) && !taken.contains_key(&r.ball_id))
        .map(|r| (r.ball_id, r.type_id().map_or(0, |t| rank_of(&table, t))))
        .collect();
    let drop = below_the_best(&free);
    rocks
        .into_iter()
        .filter(|r| !drop.contains(&r.ball_id))
        .collect()
}

/// **The preference** (pure): from `free`, `(gap, rock id)` pairs, the moon
/// belt rocks alone when there are any, else all of them unchanged. Order
/// is kept, so the caller's sort and spread work as before.
pub fn prefer_moon(free: Vec<(f64, i64)>) -> Vec<(f64, i64)> {
    if free.iter().any(|&(_, id)| is_moon_id(id)) {
        free.into_iter().filter(|&(_, id)| is_moon_id(id)).collect()
    } else {
        free
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ids inside each band, named from the server's own constants.
    const ORDINARY: i64 = eve_server::asteroid::MIN_ROCK_ID;
    const MOON: i64 = eve_server::moon_belt::MOON_BASE;

    #[test]
    fn a_free_moon_rock_beats_every_ordinary_one() {
        assert!(!is_moon_id(ORDINARY) && is_moon_id(MOON));
        let free = vec![(1.0, ORDINARY), (500.0, MOON), (2.0, ORDINARY + 1)];
        assert_eq!(prefer_moon(free), vec![(500.0, MOON)]);
    }

    #[test]
    fn with_no_moon_rock_the_choice_is_untouched() {
        let free = vec![(1.0, ORDINARY), (2.0, ORDINARY + 1)];
        assert_eq!(prefer_moon(free.clone()), free);
        assert!(prefer_moon(Vec::new()).is_empty());
    }

    #[test]
    fn only_the_rarest_free_ore_is_left_on_offer() {
        assert_eq!(
            below_the_best(&[(1, 2), (2, 5), (3, 5), (4, 0)]),
            vec![1, 4]
        );
        assert!(
            below_the_best(&[(1, 3), (2, 3)]).is_empty(),
            "a tie keeps all"
        );
        assert!(below_the_best(&[]).is_empty());
    }

    /// Invented ores, one per rarity named, in the client's own groups [V].
    const COMMON_ORE: i64 = 990_701;
    const RARE_ORE: i64 = 990_702;
    const PLAIN_ROCK: i64 = 990_703;

    fn installed() -> eve_server::typedata::Installed {
        use eve_server::moon::{GROUP_COMMON, GROUP_EXCEPTIONAL};
        let row = |id: i64, group: i64| {
            format!(
                "type\t{id}\t{group}\t25\t1\t100\t10.0\t0.0\t4000.0\t1.0\t0\t\t\
                 1980=0.75;1981=600.0;2699=1.0;2711={id}.0;2727=5000.0\tFixture ore {id}\n"
            )
        };
        let text = format!(
            "{}\n{}{}",
            eve_server::typedata::FIXTURE,
            row(COMMON_ORE, GROUP_COMMON),
            row(RARE_ORE, GROUP_EXCEPTIONAL)
        );
        eve_server::typedata::Installed::build(
            eve_server::typedata::Table::parse(&text).expect("parses"),
        )
    }

    fn table() -> OreTable {
        OreTable::from_types(installed().table().types())
    }

    #[test]
    fn rank_follows_rarity_and_an_unknown_ore_ranks_last() {
        let t = table();
        assert!(rank_of(&t, COMMON_ORE) > 0);
        assert!(rank_of(&t, RARE_ORE) > rank_of(&t, COMMON_ORE));
        assert_eq!(rank_of(&t, PLAIN_ROCK), 0);
        assert_eq!(rank_of(&t, 1), 0);
    }

    fn tracked(id: i64, type_id: i64) -> Tracked {
        Tracked {
            ball_id: id,
            motion: eve_server::space_state::Motion {
                position: [0.0; 3],
                velocity: [0.0; 3],
                mode: eve_server::destiny::Mode::Stop,
                speed_fraction: 0.0,
            },
            radius: 20.0,
            since: 0.0,
            slim: Some(crate::park::SlimInfo {
                type_id: Some(type_id),
                owner_id: None,
                launcher_id: None,
                loot_rights: None,
                char_id: None,
                corp_id: None,
                alliance_id: None,
                hacking_state: None,
                gate_range: None,
                security_status: None,
                wormhole_class: None,
            }),
            own_hull: None,
        }
    }

    fn ids(rocks: &[&Tracked]) -> Vec<i64> {
        rocks.iter().map(|r| r.ball_id).collect()
    }

    #[test]
    fn a_miner_is_shown_the_rarest_free_moon_ore_only() {
        let mut c = Content::fixture();
        c.insert("typedata", installed());
        let (a, b, ord) = (
            tracked(MOON, COMMON_ORE),
            tracked(MOON + 1, RARE_ORE),
            tracked(ORDINARY, PLAIN_ROCK),
        );
        let none = BTreeMap::new();
        let shown = scanned(vec![&a, &b, &ord], &none, Some(&c));
        assert_eq!(
            ids(&shown),
            vec![MOON + 1, ORDINARY],
            "the common moon rock is out"
        );
        // The rare rock is under another laser: the common one is the best free.
        let busy = BTreeMap::from([(MOON + 1, 7)]);
        let shown = scanned(vec![&a, &b, &ord], &busy, Some(&c));
        assert_eq!(ids(&shown), vec![MOON, MOON + 1, ORDINARY]);
    }

    #[test]
    fn nothing_is_changed_without_a_moon_rock_or_a_table() {
        let mut c = Content::fixture();
        c.insert("typedata", installed());
        let (a, ord) = (tracked(MOON, COMMON_ORE), tracked(ORDINARY, PLAIN_ROCK));
        let none = BTreeMap::new();
        assert_eq!(ids(&scanned(vec![&ord], &none, Some(&c))), vec![ORDINARY]);
        assert_eq!(
            ids(&scanned(vec![&a, &ord], &none, None)),
            vec![MOON, ORDINARY]
        );
        // A world with no moon ore in its type data.
        let mut bare = Content::fixture();
        bare.insert(
            "typedata",
            eve_server::typedata::Installed::build(
                eve_server::typedata::Table::parse(eve_server::typedata::FIXTURE).expect("parses"),
            ),
        );
        assert_eq!(
            ids(&scanned(vec![&a, &ord], &none, Some(&bare))),
            vec![MOON, ORDINARY]
        );
    }

    #[test]
    fn the_order_of_moon_rocks_is_kept() {
        let free = vec![(3.0, MOON + 2), (1.0, ORDINARY), (2.0, MOON + 1)];
        assert_eq!(prefer_moon(free), vec![(3.0, MOON + 2), (2.0, MOON + 1)]);
    }
}
