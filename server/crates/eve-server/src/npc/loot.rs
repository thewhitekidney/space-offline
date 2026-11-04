//! What an NPC leaves in its wreck.
//!
//! Moved out of `npc.rs` by W10.5 seam S4 (`docs/content/plan.md` §6.1);
//! `crate::npc::loot` is a re-export, and `combat::wreck` its caller.
//!
//! W13 packet CP22 adds **C6 R7** ([`loot_from`], [`loot_in`]): loot by
//! faction, size, tier and bounty, drawn from `npc/loot.tsv`'s pools (CP11b,
//! loaded by `npc_data`). The pools are rows; the probabilities and caps
//! here are policy, each `[A]`. [`loot`], the hand kit, stays the answer for
//! a type the loaded rows do not list (no bundle).

use super::{NpcType, pick, roll};
use crate::npc_data::{Loot, NpcTypeRow, Table, Tier};
use crate::seed::{Kind, Stream, stream};

/// `[A]` R7's meta cap on equipment by tier: Base 0-1, Elite 0-3,
/// Commander and above 0-4. Falsified by a base rat dropping meta-4 kit.
pub fn tier_cap(tier: Tier) -> i64 {
    match tier {
        Tier::Base => 1,
        Tier::Elite => 3,
        Tier::Commander | Tier::Officer | Tier::Deadspace => 4,
    }
}

/// R7's size from a `npc/types.tsv` class: frigate and destroyer 1, cruiser
/// and battlecruiser 2, battleship and above 3. `[A]` a hauler is 2, a
/// drone swarm 1, and an officer with no hull word 3.
pub fn size_of(class: &str) -> i64 {
    let c = class.to_ascii_lowercase();
    if c.contains("frigate") || c.contains("destroyer") || c.contains("swarm") {
        1
    } else if c.contains("cruiser") || c.contains("hauler") {
        2
    } else {
        3
    }
}

/// `[A]` The chance a wreck holds one stack of the faction's ammunition at
/// its size, and the per-size quantity window (R7: 20-100 per stack × size).
pub const AMMO_CHANCE: f64 = 0.5;
pub const AMMO_PER_SIZE: (i64, i64) = (20, 100);
/// `[A]` R7: a commander's wreck holds one faction module with this chance.
pub const FACTION_CHANCE: f64 = 0.25;
/// `[A]` R7: an officer drops this many officer modules, inclusive.
pub const OFFICER_DROPS: (i64, i64) = (1, 2);
/// `[A]` R7: a deadspace battleship drops one deadspace module with this
/// chance.
pub const DEADSPACE_CHANCE: f64 = 0.10;
/// `[A]` R7: a base or elite rat below high-sec drops a tag with this chance.
pub const TAG_CHANCE: f64 = 0.5;
/// `[A]` SEC-1c: a Clone Soldier rat in low-sec leaves its own tag
/// ([`CLONE_TAG_KIND`] row) with this chance, one per kill. The spec says one
/// per kill; CCP never published a rate. Falsified by a retail week of
/// ratting in which tags drop less than every kill.
pub const CLONE_TAG_CHANCE: f64 = 1.0;
/// The `npc/loot.tsv` kind of a Clone Soldier's tag row. Unlike every other
/// kind its `faction_id` column is the NPC **type** (a rat's tag is its own
/// rank's), so no faction pool ever picks it up.
pub const CLONE_TAG_KIND: &str = "clone-tag";
/// `[A]` R7: tag tiers, `clamp(bounty decile / 2, 0, TAG_TIERS - 1)`.
pub const TAG_TIERS: usize = 5;

/// The equipment pools (R7's T1/meta kinds).
pub const EQUIPMENT_KINDS: &[&str] = &["weapon", "tank", "prop", "ewar"];

/// **Whether `loot_from` can ever draw this pool row.** The one place the
/// row-side rules live; [`loot_from`] filters every pool by it and
/// [`droppable`] is the set of rows that pass.
///
/// - equipment kinds: the row's meta level at most the cap of the highest
///   tier that may draw the row (a base row is drawn by every tier, so its cap
///   is the top one; an elite row's is [`tier_cap`] of elite, and so on);
/// - `ammo`: any tier (a base row, or the NPC's own tier);
/// - `faction` only at commander tier, `officer` at officer, `deadspace` at
///   deadspace (the draws name those tiers);
/// - any other kind (`tag`, `clone-tag`, an unknown one) is not drawn from
///   the pools: the tag draws read the faction's `tags` and the `clone-tag`
///   rows by their own paths ([`tag`], [`clone_tag`]).
pub fn row_is_drawn(l: &Loot) -> bool {
    let k = l.kind.as_str();
    if EQUIPMENT_KINDS.contains(&k) {
        let cap = match l.tier {
            Tier::Base => tier_cap(Tier::Commander),
            t => tier_cap(t),
        };
        l.meta_level.unwrap_or(0) <= cap
    } else {
        match k {
            "ammo" => true,
            "faction" => l.tier == Tier::Commander,
            "officer" => l.tier == Tier::Officer,
            "deadspace" => l.tier == Tier::Deadspace,
            _ => false,
        }
    }
}

/// **Every type `loot_from` can put in a wreck** over `data`, reachable
/// (`grantable`): the pool rows [`row_is_drawn`] passes, every faction's
/// tags ([`tag`]) and every clone-tag row ([`clone_tag`]). The bots count
/// exactly this as "dropped" (nothing from nowhere), so a part they count
/// can be looted in game. `[A]` an NPC of the right faction, tier, size and
/// context exists for each row; falsified by a row no spawn can draw (a
/// size no class of its faction has).
pub fn droppable(data: &Table, grantable: impl Fn(i64) -> bool) -> std::collections::BTreeSet<i64> {
    let rows = data
        .loot
        .iter()
        .filter(|l| l.kind == CLONE_TAG_KIND || row_is_drawn(l))
        .map(|l| l.type_id);
    let tags = data.factions.values().flat_map(|f| f.tags.iter().copied());
    rows.chain(tags).filter(|t| grantable(*t)).collect()
}

/// One NPC's R7 inputs.
#[derive(Debug, Clone, Copy)]
pub struct Drop<'a> {
    /// Its `npc/types.tsv` row.
    pub row: &'a NpcTypeRow,
    /// Attributes 456/457 (`NpcType::equipment_min/max`).
    pub equipment: (u32, u32),
    /// The system's security: tags drop below high-sec only.
    pub sec: f64,
}

fn draw<T: Copy>(s: &mut Stream, from: &[T]) -> Option<T> {
    (!from.is_empty()).then(|| from[s.range(0, from.len() as u64) as usize])
}

/// **C6 R7: what an NPC's wreck holds**, as `(typeID, quantity)` stacks,
/// from `data`'s pools, drawn from `stream(seed, Loot, [npc_id])` so one NPC
/// always drops the same thing. Every type passes `grantable` (the store's
/// reach; R7 "every pool type must be grantable"), and pools are filtered by
/// it **before** a draw, so an unreachable row never eats a roll.
///
/// - equipment: between the type's min and max distinct pieces from the
///   faction's weapon, tank, prop and ewar pools at its size, metaLevel at
///   most [`tier_cap`];
/// - ammo: one stack, [`AMMO_CHANCE`], [`AMMO_PER_SIZE`] × size;
/// - commander: one faction module, [`FACTION_CHANCE`];
/// - officer: [`OFFICER_DROPS`] officer modules;
/// - a deadspace size-3 hull: one deadspace module, [`DEADSPACE_CHANCE`];
/// - base or elite below high-sec: one tag of the bounty's decile tier,
///   [`TAG_CHANCE`];
/// - a Clone Soldier in low-sec: its own rank's tag, [`CLONE_TAG_CHANCE`].
///
/// `[A]` officer modules are the faction's officer pool, not the officer's
/// own name (the pool does not key by officer); the loot-value guard
/// (248/249) is not applied.
pub fn loot_from(
    data: &Table,
    d: Drop,
    seed: u64,
    npc_id: i64,
    grantable: impl Fn(i64) -> bool,
) -> Vec<(i64, i64)> {
    let mut s = stream(seed, Kind::Loot, &[npc_id as u64]);
    let row = d.row;
    let size = size_of(&row.class);
    let pool = |kinds: &[&str], tiers: &[Tier], sized: bool, cap: Option<i64>| -> Vec<i64> {
        let mut out: Vec<i64> = data
            .loot
            .iter()
            .filter(|l| {
                l.faction_id == row.faction_id
                    && kinds.contains(&l.kind.as_str())
                    && tiers.contains(&l.tier)
                    && (!sized || l.size == size)
                    && cap.is_none_or(|c| l.meta_level.unwrap_or(0) <= c)
                    && row_is_drawn(l)
                    && grantable(l.type_id)
            })
            .map(|l| l.type_id)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    };
    // Size-matched where the pool has rows at the size, else any size.
    let sized = |kinds: &[&str], tiers: &[Tier]| {
        let p = pool(kinds, tiers, true, None);
        if p.is_empty() {
            pool(kinds, tiers, false, None)
        } else {
            p
        }
    };
    let mut out = Vec::new();

    let mut kit = pool(
        EQUIPMENT_KINDS,
        &[Tier::Base, row.tier],
        true,
        Some(tier_cap(row.tier)),
    );
    let (lo, hi) = d.equipment;
    let n = s.range(u64::from(lo), u64::from(hi.max(lo)) + 1);
    for _ in 0..n {
        if kit.is_empty() {
            break;
        }
        let i = s.range(0, kit.len() as u64) as usize;
        out.push((kit.swap_remove(i), 1));
    }

    let (a, b) = AMMO_PER_SIZE;
    let hit = s.chance(AMMO_CHANCE);
    let qty = s.range(a as u64, b as u64 + 1) as i64 * size;
    let ammo = draw(
        &mut s,
        &pool(&["ammo"], &[Tier::Base, row.tier], true, None),
    );
    if let Some(t) = ammo
        && hit
    {
        out.push((t, qty));
    }

    if row.tier == Tier::Commander
        && s.chance(FACTION_CHANCE)
        && let Some(t) = draw(&mut s, &sized(&["faction"], &[Tier::Commander]))
    {
        out.push((t, 1));
    }

    if row.tier == Tier::Officer {
        let mut mods = pool(&["officer"], &[Tier::Officer], false, None);
        let (a, b) = OFFICER_DROPS;
        for _ in 0..s.range(a as u64, b as u64 + 1) {
            if mods.is_empty() {
                break;
            }
            let i = s.range(0, mods.len() as u64) as usize;
            out.push((mods.swap_remove(i), 1));
        }
    }

    if row.context == crate::npc_gen::DEADSPACE
        && size == 3
        && s.chance(DEADSPACE_CHANCE)
        && let Some(t) = draw(&mut s, &sized(&["deadspace"], &[Tier::Deadspace]))
    {
        out.push((t, 1));
    }

    if matches!(row.tier, Tier::Base | Tier::Elite)
        && crate::npc_gen::Band::of(d.sec) != crate::npc_gen::Band::High
        && s.chance(TAG_CHANCE)
        && let Some(t) = tag(data, row, &grantable)
    {
        out.push((t, 1));
    }

    // SEC-1c: a Clone Soldier's own tag, low-sec only [A], drawn last so
    // every earlier draw of the same NPC is what it was.
    if crate::npc_gen::Band::of(d.sec) == crate::npc_gen::Band::Low
        && let Some(t) = clone_tag(data, row.type_id, &grantable)
        && s.chance(CLONE_TAG_CHANCE)
    {
        out.push((t, 1));
    }
    out
}

/// The tag a Clone Soldier of `npc_type` leaves: its [`CLONE_TAG_KIND`] row,
/// if grantable.
pub fn clone_tag(data: &Table, npc_type: i64, grantable: impl Fn(i64) -> bool) -> Option<i64> {
    data.loot
        .iter()
        .filter(|l| l.kind == CLONE_TAG_KIND && l.faction_id == npc_type)
        .map(|l| l.type_id)
        .filter(|t| grantable(*t))
        .min()
}

/// R7's tag for a type: the faction's `tags[tier]`, the tier from the
/// type's bounty decile among its faction's bountied types. `None` when the
/// faction has no tag there or it is not grantable.
pub fn tag(data: &Table, row: &NpcTypeRow, grantable: impl Fn(i64) -> bool) -> Option<i64> {
    let bounty = |id: i64| crate::npc_gen::bounty_of(data, id);
    let mine = bounty(row.type_id);
    let peers: Vec<f64> = data
        .types
        .values()
        .filter(|r| r.faction_id == row.faction_id)
        .map(|r| bounty(r.type_id))
        .filter(|b| *b > 0.0)
        .collect();
    let below = peers.iter().filter(|b| **b < mine).count();
    let decile = (below * 10).checked_div(peers.len()).unwrap_or(0);
    let tier = (decile / 2).min(TAG_TIERS - 1);
    let f = data.factions.get(&row.faction_id)?;
    let t = *f.tags.get(tier)?;
    grantable(t).then_some(t)
}

/// **The loot of a live NPC**: [`loot_from`] when the loaded rows list the
/// type and its faction has a pool, with the system's security and the
/// world seed; otherwise the hand kit ([`loot`]). The store's reach is
/// `item::find_type_in`, the grantable types.
pub fn loot_in(
    w: &crate::service::Registry,
    kind: &NpcType,
    npc_id: i64,
    solar_system_id: i64,
) -> Vec<(i64, i64)> {
    let data = w.content().get::<Table>();
    let Some(row) = data.types.get(&kind.type_id) else {
        return loot(kind, npc_id);
    };
    if !data.loot.iter().any(|l| l.faction_id == row.faction_id) {
        return loot(kind, npc_id);
    }
    let sec = w
        .content()
        .get::<crate::celestial::Universe>()
        .system(solar_system_id)
        .map_or(1.0, |s| s.security);
    let d = Drop {
        row,
        equipment: (kind.equipment_min, kind.equipment_max),
        sec,
    };
    loot_from(data, d, w.world().seed, npc_id, |t| {
        crate::item::find_type_in(w.content(), &t.to_string()).is_some()
    })
}

/// **What NPC `npc_id` of `kind` leaves in its wreck**, as `(typeID,
/// quantity)` stacks: between [`NpcType::equipment_min`] and
/// [`NpcType::equipment_max`] distinct pieces of equipment, one each, then
/// each [`Cargo`](super::Cargo) row on its own roll. Rolled from the NPC's id, so the
/// same NPC always drops the same thing -- reproducible, like
/// `combat::survives`. `[A]` the whole rule; the counts are the type's.
pub fn loot(kind: &NpcType, npc_id: i64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut salt = 0u64;
    let mut next = || {
        salt += 1;
        roll(npc_id, salt)
    };
    let mut pool: Vec<i64> = kind.equipment.to_vec();
    let n = pick(
        next(),
        i64::from(kind.equipment_min),
        i64::from(kind.equipment_max),
    );
    for _ in 0..n {
        if pool.is_empty() {
            break;
        }
        let i = pick(next(), 0, pool.len() as i64 - 1) as usize;
        out.push((pool.swap_remove(i), 1));
    }
    for c in kind.cargo {
        let (hit, qty) = (next(), next());
        if hit < c.chance {
            out.push((c.type_id, pick(qty, c.min, c.max)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::npc::{MIN_NPC_ID, TYPES};

    /// Whatever an NPC drops is something the store can hold -- or the
    /// wreck's loot plan is refused whole -- and its counts stay inside the
    /// type's own: at most `entityEquipmentMax` pieces of equipment, none
    /// twice, each stack a positive quantity in its row's range. And the
    /// wreck itself is a store type of `groupWreck`.
    #[test]
    fn what_an_npc_drops_is_a_store_type_inside_its_counts() {
        for k in TYPES {
            if let Some(w) = k.wreck_type_id {
                let t = crate::item::find_in(fx(), w).unwrap_or_else(|| panic!("wreck {w}"));
                assert_eq!(t.group_id, crate::combat::GROUP_WRECK);
            }
            for id in MIN_NPC_ID..MIN_NPC_ID + 2_000 {
                let got = loot(k, id);
                assert_eq!(got, loot(k, id), "reproducible");
                let equipment: Vec<i64> = got
                    .iter()
                    .filter(|(t, _)| k.equipment.contains(t))
                    .map(|(t, _)| *t)
                    .collect();
                assert!(equipment.len() as u32 >= k.equipment_min.min(k.equipment.len() as u32));
                assert!(equipment.len() as u32 <= k.equipment_max);
                let mut distinct = equipment.clone();
                distinct.sort_unstable();
                distinct.dedup();
                assert_eq!(distinct.len(), equipment.len());
                for (t, q) in &got {
                    assert!(
                        crate::item::find_in(fx(), *t).is_some(),
                        "{t} is a store type"
                    );
                    match k.cargo.iter().find(|c| c.type_id == *t) {
                        Some(c) => assert!((c.min..=c.max).contains(q)),
                        None => assert_eq!(*q, 1, "one of each piece of equipment"),
                    }
                }
            }
        }
    }
    // ---- CP22: C6 R7 over invented rows -------------------------------

    use crate::npc_data::{Faction, Loot, Needs, Role};

    const PIRATE: i64 = 599_071;

    /// Grantable ids, taken from the store's own table (never typed in).
    fn reachable() -> Vec<i64> {
        crate::item::SPAWNABLE
            .iter()
            .map(|t| t.type_id)
            .filter(|t| grantable(*t))
            .take(12)
            .collect()
    }

    fn grantable(t: i64) -> bool {
        crate::item::find_type_in(fx(), &t.to_string()).is_some()
    }

    thread_local! {
        /// The invented content tables, leaked once per test thread so the
        /// grantable check can run in a loop.
        static FX: &'static crate::content::Content =
            Box::leak(Box::new(crate::content::Content::fixture()));
    }

    fn fx() -> &'static crate::content::Content {
        FX.with(|c| *c)
    }

    fn type_row(type_id: i64, context: &str, class: &str, tier: Tier) -> NpcTypeRow {
        NpcTypeRow {
            type_id,
            group_id: 99_700,
            faction_id: PIRATE,
            context: context.to_owned(),
            class: class.to_owned(),
            tier,
            alias_of: None,
            needs: Needs::TURRET,
            role: Role::Dps,
        }
    }

    /// One row per kind, size and tier, half invented (unreachable) ids and
    /// half store ids; five types of each tier.
    fn fixture() -> (Table, Vec<NpcTypeRow>) {
        let ok = reachable();
        let mut t = Table::default();
        let mut invented = 990_700_000;
        let kinds = [
            "weapon",
            "tank",
            "prop",
            "ewar",
            "ammo",
            "faction",
            "officer",
            "deadspace",
            "tag",
        ];
        assert!(ok.len() >= kinds.len(), "one store id per kind");
        // Each kind has its own store id, so a drop's kind is unambiguous.
        for (k, kind) in kinds.into_iter().enumerate() {
            for size in 1..=3 {
                for (meta, tier) in [
                    (0, Tier::Base),
                    (4, Tier::Commander),
                    (5, Tier::Officer),
                    (6, Tier::Deadspace),
                ] {
                    invented += 1;
                    for type_id in [invented, ok[k]] {
                        t.loot.push(Loot {
                            faction_id: PIRATE,
                            size,
                            kind: kind.to_owned(),
                            tier,
                            type_id,
                            meta_level: Some(meta),
                        });
                    }
                }
            }
        }
        t.factions.insert(
            PIRATE,
            Faction {
                faction_id: PIRATE,
                tags: vec![ok[8], 990_799_001, ok[8], ok[8], ok[8]],
                ..Faction::default()
            },
        );
        let rows = vec![
            type_row(990_710_001, "Asteroid", "Frigate", Tier::Base),
            type_row(990_710_002, "Asteroid", "Cruiser", Tier::Elite),
            type_row(
                990_710_003,
                "Asteroid",
                "Commander Battleship",
                Tier::Commander,
            ),
            type_row(990_710_004, "Asteroid", "Officer", Tier::Officer),
            type_row(990_710_005, "Deadspace", "Battleship", Tier::Deadspace),
        ];
        for (i, r) in rows.iter().enumerate() {
            t.types.insert(r.type_id, r.clone());
            let bounty = 10_000.0 * (i + 1) as f64;
            t.bounty.insert(
                r.type_id,
                crate::npc_data::Bounty {
                    bounty,
                    ..Default::default()
                },
            );
        }
        (t, rows)
    }

    fn drops() -> impl Iterator<Item = (NpcTypeRow, i64, Vec<(i64, i64)>)> {
        let (t, rows) = fixture();
        rows.into_iter().flat_map(move |r| {
            let t = t.clone();
            (0..400i64).map(move |id| {
                let d = Drop {
                    row: &r,
                    equipment: (1, 3),
                    sec: 0.2,
                };
                let got = loot_from(&t, d, 17, MIN_NPC_ID + id, grantable);
                (r.clone(), id, got)
            })
        })
    }

    /// DOC-DROPS (#97): what `loot_from` ever emits is in `droppable`, and
    /// `droppable` holds nothing a row's tier cap forbids.
    #[test]
    fn what_loot_from_emits_is_in_the_droppable_set() {
        let (t, _) = fixture();
        let set = droppable(&t, grantable);
        assert!(!set.is_empty());
        for (r, id, got) in drops() {
            for (ty, _) in &got {
                assert!(set.contains(ty), "{ty} dropped by {} #{id}", r.type_id);
            }
        }
        // An elite equipment row over the elite cap is never drawn, so it
        // is not droppable even though the pool lists it.
        let mut t = Table::default();
        let ok = reachable();
        for (i, (tier, meta)) in [(Tier::Elite, tier_cap(Tier::Elite) + 1), (Tier::Elite, 1)]
            .into_iter()
            .enumerate()
        {
            t.loot.push(Loot {
                faction_id: PIRATE,
                size: 1,
                kind: "prop".to_owned(),
                tier,
                type_id: ok[i],
                meta_level: Some(meta),
            });
        }
        let set = droppable(&t, grantable);
        assert!(!set.contains(&ok[0]) && set.contains(&ok[1]));
    }

    #[test]
    fn every_loot_type_is_grantable() {
        let mut dropped = std::collections::BTreeSet::new();
        for (r, _, got) in drops() {
            for (ty, q) in &got {
                assert!(grantable(*ty), "{ty} is not grantable");
                assert!(*q > 0);
            }
            if !got.is_empty() {
                dropped.insert(r.type_id);
            }
        }
        // Not vacuous: every tier's type dropped something somewhere.
        assert_eq!(dropped.len(), fixture().1.len());
    }

    #[test]
    fn loot_is_stable_per_npc_id() {
        let (t, rows) = fixture();
        let mut differ = false;
        for r in &rows {
            let d = Drop {
                row: r,
                equipment: (0, 2),
                sec: -0.5,
            };
            for id in 0..200 {
                let a = loot_from(&t, d, 3, MIN_NPC_ID + id, grantable);
                assert_eq!(a, loot_from(&t, d, 3, MIN_NPC_ID + id, grantable));
                differ |= a != loot_from(&t, d, 3, MIN_NPC_ID + id + 1, grantable);
            }
        }
        assert!(differ, "the npc id moves the draw");
    }

    #[test]
    fn equipment_is_inside_the_count_size_and_tier_cap() {
        let (t, _) = fixture();
        for (r, id, got) in drops() {
            let kit: Vec<i64> = got
                .iter()
                .filter(|(ty, _)| {
                    t.loot
                        .iter()
                        .any(|l| l.type_id == *ty && EQUIPMENT_KINDS.contains(&l.kind.as_str()))
                })
                .map(|(ty, _)| *ty)
                .collect();
            let mut distinct = kit.clone();
            distinct.sort_unstable();
            distinct.dedup();
            assert_eq!(distinct.len(), kit.len(), "{id}: no piece twice");
            assert!(kit.len() <= 3);
            for ty in kit {
                assert!(t.loot.iter().any(|l| l.type_id == ty
                    && EQUIPMENT_KINDS.contains(&l.kind.as_str())
                    && l.size == size_of(&r.class)
                    && l.meta_level.unwrap_or(0) <= tier_cap(r.tier)));
            }
        }
    }

    /// SEC-1c: an invented clone soldier (type 990_710_001) with an invented
    /// tag row; the rest of the fixture is untouched.
    fn clone_world() -> (Table, NpcTypeRow, i64) {
        let (mut t, rows) = fixture();
        let tag = reachable()[10];
        t.loot.push(Loot {
            faction_id: rows[0].type_id,
            size: 0,
            kind: CLONE_TAG_KIND.to_owned(),
            tier: Tier::Base,
            type_id: tag,
            meta_level: None,
        });
        (t, rows[0].clone(), tag)
    }

    fn tags_in(t: &Table, r: &NpcTypeRow, sec: f64, tag: i64) -> usize {
        (0..400i64)
            .filter(|id| {
                let d = Drop {
                    row: r,
                    equipment: (1, 3),
                    sec,
                };
                let got = loot_from(t, d, 17, MIN_NPC_ID + id, grantable);
                got.iter().any(|(ty, _)| *ty == tag)
            })
            .count()
    }

    #[test]
    fn a_clone_soldier_in_low_sec_drops_its_own_tag_and_only_there() {
        let (t, r, tag) = clone_world();
        let low = tags_in(&t, &r, 0.2, tag);
        assert!(low > 0, "low-sec drops it");
        if CLONE_TAG_CHANCE >= 1.0 {
            assert_eq!(low, 400, "one per kill");
        }
        assert_eq!(tags_in(&t, &r, 0.9, tag), 0, "never in high-sec");
        assert_eq!(tags_in(&t, &r, -0.4, tag), 0, "never in null-sec");
    }

    #[test]
    fn another_rat_or_an_unreachable_tag_drops_no_clone_tag() {
        let (t, r, tag) = clone_world();
        let other = NpcTypeRow {
            type_id: 990_710_002,
            ..r.clone()
        };
        assert_eq!(tags_in(&t, &other, 0.2, tag), 0, "the row is the type's");
        assert_eq!(clone_tag(&t, r.type_id, |_| false), None);
        assert_eq!(clone_tag(&t, r.type_id, grantable), Some(tag));
    }

    #[test]
    fn the_clone_tag_draw_leaves_every_earlier_drop_as_it_was() {
        let (t, r, tag) = clone_world();
        let (plain, _) = fixture();
        for id in 0..100i64 {
            let d = Drop {
                row: &r,
                equipment: (1, 3),
                sec: 0.2,
            };
            let with = loot_from(&t, d, 17, MIN_NPC_ID + id, grantable);
            let without = loot_from(&plain, d, 17, MIN_NPC_ID + id, grantable);
            let rest: Vec<_> = with.iter().filter(|(ty, _)| *ty != tag).collect();
            assert_eq!(rest, without.iter().collect::<Vec<_>>());
        }
    }

    #[test]
    fn with_no_loaded_row_the_hand_kit_drops() {
        let w = crate::service::Registry::new(crate::model::SandboxConfig::default());
        for id in MIN_NPC_ID..MIN_NPC_ID + 50 {
            let k = &crate::npc::ANGEL_HIJACKER;
            assert_eq!(loot_in(&w, k, id, 30_000_001), loot(k, id));
        }
    }
}
