//! **Which small round a ratter fires** (lane BOT-TUNE).
//!
//! The ratter used to buy one round, EMP S ([`crate::escort::AMMO`]). GB-FIX
//! gave a school-system builder the starter rung instead (Iron, Tungsten,
//! Carbonized Lead and Nuclear S, recipes over Tritanium and Pyerite only),
//! so nothing a builder there makes was a round the ratter would buy, and
//! the ammo loop stayed open. A player does not care which charge it is: any
//! round of the gun's charge group and size will do, and the market has what
//! the market has.
//!
//! **What fits** is read from the bundle, not listed: a charge type is a
//! **small round** when its group is [`GROUP_ROUNDS`] and its `chargeSize`
//! is [`SIZE_SMALL`]; a gun **takes** it when the gun's `chargeGroup1..5`
//! (604-608) name that group and the gun's own `chargeSize` matches
//! ([`takes`]). That is the server's own test (`combat.rs`, `modules.rs`).
//! **Preference** is two rules, in order ([`choose`]): the rounds **local
//! builders make** (the products of the starter rung's blueprints, then the
//! hand-fitted default) before the rest, and within each, the cheapest ask.
//! Without a bundle the choice is the old one, [`crate::escort::AMMO`].
//!
//! **[A]** that group 83 size 1 is the only small projectile round a ratter's
//! gun takes in the bundle's build (the brief's reading); [`takes`] is the
//! real test, so a gun with another group list is served correctly anyway.
//! Falsified by `LoadAmmo` refusing a round [`rounds_for`] returned.

use eve_server::content::Content;

/// `projectile ammo`, the group of every projectile round, EMP S among them
/// **[V-data]**.
pub const GROUP_ROUNDS: i64 = 83;
/// `chargeSize` of a small round, and of the small turret that takes it
/// **[V-data]**.
pub const SIZE_SMALL: f64 = 1.0;
/// `chargeSize`.
pub const ATTR_CHARGE_SIZE: i64 = 128;
/// `chargeGroup1` ... `chargeGroup5`.
pub const ATTR_CHARGE_GROUPS: std::ops::RangeInclusive<i64> = 604..=608;

/// One charge type, as the choice sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Round {
    pub type_id: i64,
    pub group_id: i64,
    pub size: f64,
}

/// Is this a **small round** (group [`GROUP_ROUNDS`], size [`SIZE_SMALL`])?
pub fn is_small_round(r: &Round) -> bool {
    r.group_id == GROUP_ROUNDS && (r.size - SIZE_SMALL).abs() < 1e-9
}

/// Does a gun take `r`? `groups` are the gun's `chargeGroupN` values,
/// `size` its `chargeSize` (`None`: the gun states none, size is not
/// checked).
pub fn takes(groups: &[i64], size: Option<f64>, r: &Round) -> bool {
    groups.contains(&r.group_id) && size.is_none_or(|s| (s - r.size).abs() < 1e-9)
}

/// **The choice**: of `candidates`, the ones `ask` gives a price for (the
/// market has them), those in `local` first (in `local`'s order), then by
/// price, then by id. `None` when the market has none.
pub fn choose(candidates: &[i64], local: &[i64], ask: &dyn Fn(i64) -> Option<f64>) -> Option<i64> {
    let rank = |t: i64| local.iter().position(|&l| l == t).unwrap_or(local.len());
    candidates
        .iter()
        .filter_map(|&t| ask(t).filter(|p| *p > 0.0).map(|p| (t, p)))
        .min_by(|a, b| {
            rank(a.0)
                .cmp(&rank(b.0))
                .then(a.1.total_cmp(&b.1))
                .then(a.0.cmp(&b.0))
        })
        .map(|(t, _)| t)
}

/// **Every round `gun` takes** (small for a small gun, PROG-PVE: its own
/// size for a medium or large one; PROG-PVE2: whatever charge group the gun
/// names, so a hybrid, laser or missile gun gets its own), from the bundle's
/// typedata, by type id; the gun's default round
/// ([`crate::fit_plan::round_of_gun`]) first when it is among them. A
/// charge with no `chargeSize` (a missile) counts as the gun's own size, so a
/// launcher, which states none, takes every missile of its group. Empty
/// without a typedata table (the caller falls back to the kit's own round).
pub fn rounds_for(content: &Content, gun: i64) -> Vec<i64> {
    let Some(td) = content.try_get::<eve_server::typedata::Installed>() else {
        return Vec::new();
    };
    rounds_in(td.table(), gun)
}

/// [`rounds_for`] over a bare table (DOC-2): any gun group, any size. A
/// charge that states no `chargeSize` counts as the gun's own; a gun that
/// states none takes every charge of its groups.
pub fn rounds_in(table: &eve_server::typedata::Table, gun: i64) -> Vec<i64> {
    let Some(g) = table.get(gun) else {
        return Vec::new();
    };
    let groups: Vec<i64> = ATTR_CHARGE_GROUPS
        .filter_map(|a| g.attribute(a))
        .map(|v| v as i64)
        .collect();
    let size = g.attribute(ATTR_CHARGE_SIZE);
    let mut out: Vec<i64> = table
        .types()
        .filter(|t| t.published && t.category_id == crate::CATEGORY_CHARGE)
        .filter_map(|t| {
            let r = Round {
                type_id: t.type_id,
                group_id: t.group_id,
                size: t.attribute(ATTR_CHARGE_SIZE).or(size).unwrap_or(0.0),
            };
            takes(&groups, size, &r).then_some(t.type_id)
        })
        .collect();
    let first = crate::fit_plan::round_of_gun(gun);
    out.sort_by_key(|&t| (t != first, t));
    out
}

/// **The round for a gun of any group and size** (DOC-2): [`choose`] over
/// [`rounds_in`], so a hybrid railgun gets the market's cheapest charge of
/// its own group and size, a crystal for a laser, a missile for a launcher.
pub fn choose_for(
    table: &eve_server::typedata::Table,
    gun: i64,
    local: &[i64],
    ask: &dyn Fn(i64) -> Option<f64>,
) -> Option<i64> {
    choose(&rounds_in(table, gun), local, ask)
}

/// The damage attributes of a round, in the order of a resonance row:
/// em, explosive, kinetic, thermal.
pub const DAMAGE_ATTRS: [i64; 4] = [114, 116, 117, 118];

/// **The round that does most against a tank layer** (DOC-2, R-DOCTRINE
/// 8.2): of `candidates`, the one whose damage, each type scaled by the
/// target layer's `resonance` (`1` takes it all, `0` none; em, explosive,
/// kinetic, thermal), is the greatest; at a tie the lower id. Rounds that
/// state no damage are left out.
pub fn against(
    table: &eve_server::typedata::Table,
    candidates: &[i64],
    resonance: [f64; 4],
) -> Option<i64> {
    candidates
        .iter()
        .filter_map(|&r| {
            let dealt: f64 = DAMAGE_ATTRS
                .iter()
                .zip(resonance)
                .map(|(a, res)| table.attribute(r, *a) * res)
                .sum();
            (dealt > 0.0).then_some((r, dealt))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(r, _)| r)
}

/// **The rounds local builders make**: the products of the starter rung's
/// blueprints ([`crate::builder::STARTER_AMMO`]) the recipes know, then the
/// hand-fitted default.
pub fn local_products(content: &Content) -> Vec<i64> {
    let mut out: Vec<i64> = content
        .try_get::<eve_server::industry::Table>()
        .map(|t| {
            crate::builder::STARTER_AMMO
                .iter()
                .filter_map(|&bp| t.find(bp).map(|b| b.product_type_id))
                .collect()
        })
        .unwrap_or_default();
    out.push(crate::escort::AMMO);
    out
}

/// **Rounds loaded in the gun at `flag`**, whichever small round it holds
/// (a gun holds one type): [`crate::escort::AMMO`] first, then `types` (the
/// gun's rounds, [`rounds_for`]); the first non-empty count, else 0.
pub fn loaded(conn: &mut crate::Conn, ship: i64, flag: i64, types: &[i64]) -> crate::Result<i64> {
    Ok(loaded_round(conn, ship, flag, types)?.map_or(0, |(_, n)| n))
}

/// [`loaded`], and which round it is: `(type, count)` of the first
/// non-empty one. A launcher's reach is its round's (ENGINE-FIX).
pub fn loaded_round(
    conn: &mut crate::Conn,
    ship: i64,
    flag: i64,
    types: &[i64],
) -> crate::Result<Option<(i64, i64)>> {
    for &t in std::iter::once(&crate::escort::AMMO).chain(types) {
        let n = conn.loaded_rounds(ship, flag, t)?;
        if n > 0 {
            return Ok(Some((t, n)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_server::content::Content;

    const SMALL_A: i64 = 9_001;
    const SMALL_B: i64 = 9_002;
    const SMALL_C: i64 = 9_003;
    const GROUP_OTHER: i64 = 9_090;

    fn round(t: i64, g: i64, size: f64) -> Round {
        Round {
            type_id: t,
            group_id: g,
            size,
        }
    }

    #[test]
    fn a_small_round_is_group_83_size_1_and_a_gun_takes_its_groups_and_size() {
        assert!(is_small_round(&round(SMALL_A, GROUP_ROUNDS, SIZE_SMALL)));
        assert!(!is_small_round(&round(SMALL_A, GROUP_ROUNDS, 2.0)));
        assert!(!is_small_round(&round(SMALL_A, GROUP_OTHER, SIZE_SMALL)));
        let r = round(SMALL_A, GROUP_ROUNDS, SIZE_SMALL);
        assert!(takes(&[GROUP_ROUNDS], Some(SIZE_SMALL), &r));
        assert!(takes(&[GROUP_OTHER, GROUP_ROUNDS], None, &r));
        assert!(!takes(&[GROUP_OTHER], Some(SIZE_SMALL), &r));
        assert!(!takes(&[GROUP_ROUNDS], Some(2.0), &r));
    }

    #[test]
    fn the_choice_prefers_what_local_builders_make_then_the_cheapest_ask() {
        let cands = [SMALL_A, SMALL_B, SMALL_C];
        let book = |t: i64| match t {
            SMALL_A => Some(1.0),
            SMALL_B => Some(5.0),
            _ => None,
        };
        // Nothing local: the cheapest the market has.
        assert_eq!(choose(&cands, &[], &book), Some(SMALL_A));
        // A local product beats a cheaper foreign one.
        assert_eq!(choose(&cands, &[SMALL_B], &book), Some(SMALL_B));
        // A local product the market lacks is no choice (C has no ask).
        assert_eq!(choose(&cands, &[SMALL_C, SMALL_B], &book), Some(SMALL_B));
        // Local order breaks a tie between two local rounds.
        assert_eq!(choose(&cands, &[SMALL_B, SMALL_A], &book), Some(SMALL_B));
        // An empty market has none.
        assert_eq!(choose(&cands, &[SMALL_A], &|_| None), None);
        assert_eq!(choose(&[], &[], &book), None);
    }

    #[test]
    fn a_zero_ask_is_no_ask() {
        assert_eq!(choose(&[SMALL_A], &[], &|_| Some(0.0)), None);
    }

    /// PROG-PVE2: a gun takes the rounds of the charge group its type names,
    /// of its own size, and a launcher (no size) takes every charge of its
    /// group. Invented rows throughout.
    #[test]
    fn a_gun_gets_the_rounds_of_its_own_group_and_size() {
        const LASER_S: i64 = 990_920_001;
        const CRYSTAL_S: i64 = 990_920_011;
        const CRYSTAL_S2: i64 = 990_920_012;
        const CRYSTAL_M: i64 = 990_920_013;
        const HYBRID_S: i64 = 990_920_014;
        const LAUNCHER: i64 = 990_920_002;
        const MISSILE_A: i64 = 990_920_021;
        const MISSILE_B: i64 = 990_920_022;
        const GROUP_CRYSTAL: i64 = 990_921;
        const GROUP_HYBRID_X: i64 = 990_922;
        const GROUP_MISSILE: i64 = 990_923;
        let rows = format!(
            "{}\
             type\t{LASER_S}\t53\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t0\t\t128=1.0;604={GROUP_CRYSTAL}.0\tFixture Laser\n\
             type\t{LAUNCHER}\t509\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t0\t\t604={GROUP_MISSILE}.0\tFixture Launcher\n\
             type\t{CRYSTAL_S}\t{GROUP_CRYSTAL}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t128=1.0\tFixture Crystal S\n\
             type\t{CRYSTAL_S2}\t{GROUP_CRYSTAL}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t128=1.0\tFixture Crystal S2\n\
             type\t{CRYSTAL_M}\t{GROUP_CRYSTAL}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t128=2.0\tFixture Crystal M\n\
             type\t{HYBRID_S}\t{GROUP_HYBRID_X}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t128=1.0\tFixture Hybrid S\n\
             type\t{MISSILE_A}\t{GROUP_MISSILE}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t\tFixture Missile A\n\
             type\t{MISSILE_B}\t{GROUP_MISSILE}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t\tFixture Missile B\n",
            eve_server::typedata::FIXTURE,
        );
        let mut c = Content::fixture();
        let table = eve_server::typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        // The laser takes its group's small crystals, and no medium one nor
        // another system's charge.
        assert_eq!(rounds_for(&c, LASER_S), vec![CRYSTAL_S, CRYSTAL_S2]);
        // The launcher states no size: every missile of its group.
        assert_eq!(rounds_for(&c, LAUNCHER), vec![MISSILE_A, MISSILE_B]);
        // A gun the table lacks has none (the caller falls back).
        assert!(rounds_for(&c, 1).is_empty());
    }

    /// DOC-2: the round follows the gun's own charge group and size, for any
    /// system; the cheapest ask wins, a local product first; against a tank
    /// layer the round whose damage that layer resists least.
    #[test]
    fn ammo_follows_the_guns_group_and_size() {
        const GUN_M: i64 = 990_970_001;
        const GUN_S: i64 = 990_970_002;
        const A_M: i64 = 990_970_011;
        const B_M: i64 = 990_970_012;
        const A_S: i64 = 990_970_013;
        const OTHER: i64 = 990_970_014;
        const G: i64 = 990_971;
        let rows = format!(
            "{}\
             type\t{GUN_M}\t74\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t0\t\t128=2.0;604={G}.0\tFixture Medium Rail\n\
             type\t{GUN_S}\t74\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t0\t\t128=1.0;604={G}.0\tFixture Small Rail\n\
             type\t{A_M}\t{G}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t114=8.0;117=2.0;128=2.0\tFixture Round A\n\
             type\t{B_M}\t{G}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t116=5.0;118=5.0;128=2.0\tFixture Round B\n\
             type\t{A_S}\t{G}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=2.0;128=1.0\tFixture Round S\n\
             type\t{OTHER}\t{G2}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=9.0;128=2.0\tFixture Other\n",
            eve_server::typedata::FIXTURE,
            G2 = G + 1,
        );
        let table = eve_server::typedata::Table::parse(&rows).expect("rows parse");
        // A medium gun takes the medium rounds of its group, a small one
        // the small; another group's round is neither's.
        let mut m = rounds_in(&table, GUN_M);
        m.sort_unstable();
        assert_eq!(m, vec![A_M, B_M]);
        assert_eq!(rounds_in(&table, GUN_S), vec![A_S]);
        // The choice: the cheaper ask; a local product before it.
        let ask = |t: i64| match t {
            A_M => Some(9.0),
            B_M => Some(4.0),
            _ => None,
        };
        assert_eq!(choose_for(&table, GUN_M, &[], &ask), Some(B_M));
        assert_eq!(choose_for(&table, GUN_M, &[A_M], &ask), Some(A_M));
        assert_eq!(choose_for(&table, GUN_S, &[], &ask), None);
        // Against a tank that resists em and kinetic hard, the explosive and
        // thermal round; against one that resists the others, the first.
        let cands = [A_M, B_M];
        assert_eq!(against(&table, &cands, [0.2, 1.0, 0.2, 1.0]), Some(B_M));
        assert_eq!(against(&table, &cands, [1.0, 0.2, 1.0, 0.2]), Some(A_M));
        assert_eq!(against(&table, &cands, [0.0; 4]), None);
    }
}
