//! The NPC generators: which ships a belt or a dungeon spawn holds (C6 R4,
//! R5). The loot half (C6 R7) is `npc/loot.rs`.
//!
//! W13 packet CP22 (`docs/content/plan.md` §6.2), spec C6 R4, R5, R7 and
//! packet N3; lane note `docs/lanes/cp22.md`. Every row comes from
//! `npc_data` (CP20); this module holds only the **policy**: the budget
//! anchors, the class windows, the special chances and the squad sizes, each
//! `[A]` with its falsifier (C6 §4). Every function is pure over its inputs
//! and draws through [`crate::seed::stream`] with [`Kind::Spawn`] (plan §3.1),
//! so the same seed gives the same squad.
//!
//! **A squad is a budget, not a list** (R4): a lead, the dearest ship the
//! band allows inside half the budget, then cheapest-weighted escorts while
//! they fit, so a squad's bounty sum never exceeds the budget it was given.
//!
//! **With no bundle** the loaded rows are `npc_data`'s invented fixture, no
//! real system has a loaded pirate, and [`belt_squad`] fills from the hand
//! faction (`npc::faction_for`): the invented fixture rat (CP61), so the
//! sandbox's belts keep their rats.

use crate::celestial::{SecurityClass, pseudo_security};
use crate::npc_data::{Needs, Table, Tier};
use crate::seed::{Kind, Stream, stream};

/// **What the server can fly today** (C6 §0.5): a spawner draws only types
/// whose `needs` are inside this. Each NPC packet that adds a capability
/// (missiles, scram, ewar) widens it, with no data edit.
///
/// `[A]` `SELF_REP` is in from the start: a self-repairing rat the brain
/// does not yet repair is a weaker rat, not a broken one, and without it the
/// extract's belt pools are nearly empty (the lane note has the census).
/// CP23 adds the repair itself. Falsified by a rat that is visibly wrong
/// because it never repairs. CP45 (W17) adds `MISSILE`: `npc_missile`
/// launches and lands the type's missiles. CP49 (W18) adds `SCRAM` and
/// `WEB`: `npc_ewar` holds the scram against warp and slows a webbed ship.
///
/// K-RATAI adds `REMOTE_REP`: `npc::brain` repairs allies for effects 3852,
/// 3855, 6741 and 6742. **[A]** The two other remote-repair effects in the
/// set (6165, 6687) do nothing yet, so those types fly as weaker rats, as a
/// self-repairer did before CP23. Falsified by a pool that draws a type
/// whose missing remote capacitor transfer shows.
///
/// NEUT-NOS adds `NEUT`: `npc_ewar` drains the target's capacitor for effects
/// 6691 and 6756. **[A]** The other neut tags (6882, `npcBehaviorEnergyNosferatu`,
/// and the player 6187) do nothing on a rat, so those types fly as weaker
/// rats. Falsified by a nosferatu rat whose missing drain shows.
pub const SUPPORTED: Needs = Needs(
    Needs::TURRET.0
        | Needs::SELF_REP.0
        | Needs::MISSILE.0
        | Needs::SCRAM.0
        | Needs::WEB.0
        | Needs::NEUT.0
        | Needs::REMOTE_REP.0,
);

/// `[A]` **R4's bounty budget anchors**, `(security, ISK of bounty)`,
/// ascending in security. Between anchors the budget is log-linear; outside
/// them it is the end anchor's. Falsified by belts that feel too empty or
/// too full for their band.
pub const BUDGET_ANCHORS: [(f64, f64); 4] = [
    (-1.0, 3_000_000.0),
    (0.0, 600_000.0),
    (0.5, 60_000.0),
    (0.9, 12_000.0),
];

/// `[A]` A belt squad's size window, inclusive (R4: "2..6 ships"). The fill
/// stops early when nothing else fits the budget.
pub const BELT_SQUAD_SIZE: (u64, u64) = (2, 6);

/// `[A]` A dungeon spawn's squad size window, inclusive (R5: "1..5 ships").
pub const DUNGEON_SQUAD_SIZE: (u64, u64) = (1, 5);

/// `[A]` High-sec belts add cruisers below this pseudo-security (R4).
pub const HISEC_CRUISER_BELOW: f64 = 0.7;

/// `[A]` The chance a belt squad is led by a commander, by band (R4: never
/// in high-sec, 0.02 low, 0.05 null). A special leads only when it fits the
/// budget, so the squad's sum stays inside it.
pub const COMMANDER_CHANCE: [(Band, f64); 3] =
    [(Band::High, 0.0), (Band::Low, 0.02), (Band::Null, 0.05)];

/// `[A]` The chance of an officer lead in null-sec (R4: 0.002 at sec ≤ 0.0).
/// Inside the budget only, as every special: with [`BUDGET_ANCHORS`] an
/// officer's bounty does not fit, so none spawns until a packet gives
/// officers their own rule (lane note, Not done).
pub const OFFICER_CHANCE: f64 = 0.002;

/// The officer classes of `npc/types.tsv` (C6 §2.1).
pub const OFFICER_CLASSES: &[&str] = &["Officer", "Officer Frigate", "Officer Cruiser"];

/// `[A]` The chance of a hauler lead below high-sec (R4: 0.10).
pub const HAULER_CHANCE: f64 = 0.10;

/// `[A]` **R5's dungeon budget by difficulty 1-5**, split evenly over the
/// dungeon's spawn entities. A difficulty outside 1-5 is clamped, and a
/// dungeon with none is difficulty 1. Falsified by a site whose rats are far
/// off its level.
pub const DIFFICULTY_BUDGET: [f64; 5] = [40_000.0, 120_000.0, 400_000.0, 1_200_000.0, 3_500_000.0];

/// `[A]` The archetypes whose spawns draw from the `Deadspace` context (R5:
/// combat sites, escalations, complexes and the named ones); every other
/// archetype draws `Asteroid`. Falsified by a site whose client-side rats
/// are of the other context.
pub const DEADSPACE_ARCHETYPES: &[i64] = &[3, 4, 6, 8, 14, 16, 21, 24, 31, 33, 34, 35, 36];

/// The belt context (C6 §2.1).
pub const ASTEROID: &str = "Asteroid";
/// The deadspace context (C6 §2.1).
pub const DEADSPACE: &str = "Deadspace";

/// A system's band, from the client's own class of its security.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    High,
    Low,
    Null,
}

impl Band {
    /// `SecurityClassFromLevel` over the client's pseudo-security, so a
    /// band agrees with the client's colour for the system.
    pub fn of(sec: f64) -> Band {
        match SecurityClass::from_level(pseudo_security(sec)) {
            SecurityClass::High | SecurityClass::Safe => Band::High,
            SecurityClass::Low => Band::Low,
            SecurityClass::Zero => Band::Null,
        }
    }
}

/// **R4's `B(sec)`**: the ISK of bounty a belt squad may carry.
pub fn budget(sec: f64) -> f64 {
    let a = BUDGET_ANCHORS;
    if sec <= a[0].0 {
        return a[0].1;
    }
    for w in a.windows(2) {
        let ((s0, b0), (s1, b1)) = (w[0], w[1]);
        if sec == s0 {
            return b0;
        }
        if sec < s1 {
            let t = (sec - s0) / (s1 - s0);
            return (b0.ln() + t * (b1.ln() - b0.ln())).exp();
        }
    }
    a[a.len() - 1].1
}

/// **R5's `D(difficulty)`** for the whole dungeon.
pub fn dungeon_budget(difficulty: Option<i64>) -> f64 {
    let i = difficulty
        .unwrap_or(1)
        .clamp(1, DIFFICULTY_BUDGET.len() as i64)
        - 1;
    DIFFICULTY_BUDGET[i as usize]
}

/// R5's context for an archetype.
pub fn dungeon_context(archetype: i64) -> &'static str {
    if DEADSPACE_ARCHETYPES.contains(&archetype) {
        DEADSPACE
    } else {
        ASTEROID
    }
}

/// One ship a generator may draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub type_id: i64,
    pub bounty: f64,
    /// `npc/types.tsv`'s class, verbatim (`Frigate`, `Commander Cruiser`).
    pub class: String,
}

/// A generated squad.
#[derive(Debug, Clone, PartialEq)]
pub struct Squad {
    /// The loaded faction every member belongs to; `None` for a squad from
    /// the hand faction (no bundle, or no loaded pool for the faction).
    pub faction_id: Option<i64>,
    /// Type ids, lead first.
    pub members: Vec<i64>,
    /// The members' bounties summed in order: at most the budget.
    pub bounty: f64,
}

/// A type's bounty: `npc/bounty.tsv` (attribute 481), 0 when absent.
pub fn bounty_of(data: &Table, type_id: i64) -> f64 {
    data.bounty.get(&type_id).map_or(0.0, |b| b.bounty)
}

/// Which rows a pool takes.
#[derive(Debug, Clone, Copy)]
pub struct Filter<'a> {
    pub faction_id: i64,
    pub context: &'a str,
    pub classes: &'a [&'a str],
    pub tiers: &'a [Tier],
}

/// **R4's pool**: the faction's types of the context, classes and tiers,
/// collapsed per C6 §2.2 (an alias row is dropped for its canonical one),
/// `needs ⊆` [`SUPPORTED`], with a bounty. Ascending by type id.
pub fn pool(data: &Table, f: Filter) -> Vec<Candidate> {
    data.types
        .values()
        .filter(|r| {
            r.faction_id == f.faction_id
                && r.context == f.context
                && f.classes.contains(&r.class.as_str())
                && f.tiers.contains(&r.tier)
                && r.alias_of.is_none()
                && SUPPORTED.contains(r.needs)
        })
        .map(|r| Candidate {
            type_id: r.type_id,
            bounty: bounty_of(data, r.type_id),
            class: r.class.clone(),
        })
        .filter(|c| c.bounty > 0.0)
        .collect()
}

/// R4's class windows: `(lead classes, escort classes)` for a band.
pub fn classes(sec: f64) -> (Vec<&'static str>, Vec<&'static str>) {
    match Band::of(sec) {
        Band::High => {
            let mut c = vec!["Frigate", "Destroyer"];
            if pseudo_security(sec) < HISEC_CRUISER_BELOW {
                c.push("Cruiser");
            }
            (c.clone(), c)
        }
        Band::Low => {
            let c = vec!["Frigate", "Destroyer", "Cruiser", "BattleCruiser"];
            (c.clone(), c)
        }
        Band::Null => (
            vec!["Cruiser", "BattleCruiser", "Battleship"],
            vec!["Frigate", "Cruiser", "BattleCruiser", "Battleship"],
        ),
    }
}

/// R4's tiers: base, and elite below high-sec.
pub fn tiers(sec: f64) -> &'static [Tier] {
    match Band::of(sec) {
        Band::High => &[Tier::Base],
        Band::Low | Band::Null => &[Tier::Base, Tier::Elite],
    }
}

/// **R4's fill.** The lead is the dearest of `lead` inside half the budget
/// (else inside the whole of it; ties to the lowest id); then escorts from
/// `escorts`, each weighted by `1 / bounty` among those that still fit,
/// until the drawn size is reached or nothing fits. The sum never exceeds
/// `budget`. Empty when not even the lead fits.
pub fn fill(
    lead: &[Candidate],
    escorts: &[Candidate],
    budget: f64,
    size: (u64, u64),
    s: &mut Stream,
) -> Vec<Candidate> {
    let target = s.range(size.0, size.1 + 1) as usize;
    let dearest = |cap: f64| {
        lead.iter().filter(|c| c.bounty <= cap).max_by(|a, b| {
            a.bounty
                .total_cmp(&b.bounty)
                .then(b.type_id.cmp(&a.type_id))
        })
    };
    let Some(first) = dearest(budget / 2.0).or_else(|| dearest(budget)) else {
        return Vec::new();
    };
    let mut out = vec![first.clone()];
    while out.len() < target.max(1) {
        let spent: f64 = out.iter().map(|c| c.bounty).sum();
        let fits: Vec<&Candidate> = escorts
            .iter()
            .filter(|c| spent + c.bounty <= budget)
            .collect();
        if fits.is_empty() {
            break;
        }
        let total: f64 = fits.iter().map(|c| 1.0 / c.bounty).sum();
        let mut r = s.unit() * total;
        let mut pick = fits[fits.len() - 1];
        for c in &fits {
            r -= 1.0 / c.bounty;
            if r < 0.0 {
                pick = c;
                break;
            }
        }
        out.push(pick.clone());
    }
    out
}

fn squad(faction_id: Option<i64>, members: Vec<Candidate>) -> Squad {
    Squad {
        faction_id,
        bounty: members.iter().map(|c| c.bounty).sum(),
        members: members.into_iter().map(|c| c.type_id).collect(),
    }
}

/// **R4: a belt squad** of `faction` at security `sec`, drawn from
/// `stream(seed, Spawn, parts)` (the caller's `parts` are the system, the
/// belt and the generation). A special lead (officer, commander, hauler, in
/// that order, at most one) is rolled first and leads only if it fits the
/// budget. With no faction, or no loaded pool for it, the squad is the hand
/// faction's ([`hand_squad`]).
pub fn belt_squad(data: &Table, faction: Option<i64>, sec: f64, seed: u64, parts: &[u64]) -> Squad {
    let mut s = stream(seed, Kind::Spawn, parts);
    let b = budget(sec);
    let (lead_classes, escort_classes) = classes(sec);
    let Some(f) = faction else {
        return hand_squad(sec, &mut s);
    };
    let asteroid = |classes, tiers| Filter {
        faction_id: f,
        context: ASTEROID,
        classes,
        tiers,
    };
    let leads = pool(data, asteroid(&lead_classes[..], tiers(sec)));
    let escorts = pool(data, asteroid(&escort_classes[..], tiers(sec)));
    if leads.is_empty() && escorts.is_empty() {
        return hand_squad(sec, &mut s);
    }
    let band = Band::of(sec);
    let commander = COMMANDER_CHANCE
        .iter()
        .find(|(k, _)| *k == band)
        .map_or(0.0, |(_, p)| *p);
    let commanders: Vec<String> = lead_classes
        .iter()
        .map(|c| format!("Commander {c}"))
        .collect();
    let commanders: Vec<&str> = commanders.iter().map(String::as_str).collect();
    let officer = if band == Band::Null {
        OFFICER_CHANCE
    } else {
        0.0
    };
    let hauler = if band == Band::High {
        0.0
    } else {
        HAULER_CHANCE
    };
    let specials = [
        (
            officer,
            pool(data, asteroid(OFFICER_CLASSES, &[Tier::Officer][..])),
        ),
        (
            commander,
            pool(data, asteroid(&commanders[..], &[Tier::Commander][..])),
        ),
        (hauler, pool(data, asteroid(&["Hauler"][..], tiers(sec)))),
    ];
    let (mut lead, mut special_leads) = (leads, false);
    for (p, special) in specials {
        // Every roll is drawn, so one pool's emptiness or one hit never
        // shifts the stream under the fill.
        let hit = s.chance(p);
        let fitting: Vec<Candidate> = special.into_iter().filter(|c| c.bounty <= b).collect();
        if hit && !special_leads && !fitting.is_empty() {
            (lead, special_leads) = (fitting, true);
        }
    }
    let members = fill(&lead, &escorts, b, BELT_SQUAD_SIZE, &mut s);
    if members.is_empty() {
        return hand_squad(sec, &mut s);
    }
    squad(Some(f), members)
}

/// **The no-bundle belt squad**: R4's fill over the hand faction's types
/// that `npc::find` resolves (the invented `npc::FIXTURE_RAT`, CP61), in
/// R4's class windows. Never empty: if the band's window holds no hand
/// type, the fixture rat leads alone.
pub fn hand_squad(sec: f64, s: &mut Stream) -> Squad {
    use crate::npc::Size;
    let faction = crate::npc::faction_for(0);
    let named = [
        (Size::Frigate, "Frigate"),
        (Size::Destroyer, "Destroyer"),
        (Size::Cruiser, "Cruiser"),
    ];
    let of = |window: &[&str]| -> Vec<Candidate> {
        named
            .iter()
            .filter(|(_, n)| window.contains(n))
            .flat_map(|(size, n)| {
                faction.of_size(*size).into_iter().map(move |t| Candidate {
                    type_id: t.type_id,
                    bounty: t.bounty,
                    class: (*n).to_owned(),
                })
            })
            .filter(|c| c.bounty > 0.0)
            .collect()
    };
    let (lead_classes, escort_classes) = classes(sec);
    let mut lead = of(&lead_classes);
    let escorts = of(&escort_classes);
    if lead.is_empty() {
        lead = escorts.clone();
    }
    let mut members = fill(&lead, &escorts, budget(sec), BELT_SQUAD_SIZE, s);
    if members.is_empty() {
        let t = &crate::npc::FIXTURE_RAT;
        members.push(Candidate {
            type_id: t.type_id,
            bounty: t.bounty,
            class: "Frigate".to_owned(),
        });
    }
    squad(None, members)
}

/// **A belt squad for a live system**: [`belt_squad`] with the system's
/// loaded pirate (`npc::pirate_of`), its security from the universe table,
/// and the world seed; `parts` follow the system id (belt, generation).
///
/// `[A]` a system the universe table lacks is treated as security 1.0 --
/// with no bundle that is every real system, so the sandbox keeps its
/// high-sec squads. Falsified by a bundle-less belt that should be harder.
pub fn belt_squad_in(w: &crate::service::Registry, system: i64, parts: &[u64]) -> Squad {
    let sec = w
        .content()
        .get::<crate::celestial::Universe>()
        .system(system)
        .map_or(1.0, |s| s.security);
    let mut all = vec![system as u64];
    all.extend_from_slice(parts);
    belt_squad(
        w.content().get::<Table>(),
        crate::npc::pirate_of(w, system),
        sec,
        w.world().seed,
        &all,
    )
}

/// One R5 squad: the spawn row it fills, and its ships.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotSquad {
    pub room: i64,
    pub object: i64,
    /// The index of the row in the dungeon's `spawns`.
    pub spawn: usize,
    pub squad: Squad,
}

/// `[A]` the classes a dungeon spawn draws from: every combat hull class,
/// no hauler, commander or officer (those are R4's specials).
pub const DUNGEON_CLASSES: &[&str] = &[
    "Frigate",
    "Destroyer",
    "Cruiser",
    "BattleCruiser",
    "Battleship",
];

/// `[A]` the tiers a dungeon spawn draws from.
pub const DUNGEON_TIERS: &[Tier] = &[Tier::Base, Tier::Elite, Tier::Deadspace];

/// **R5: a squad per spawn entity** of the dungeon, from its faction and
/// context, with [`dungeon_budget`] split evenly over its spawns and
/// [`DUNGEON_SQUAD_SIZE`] ships. Each spawn draws from
/// `stream(seed, Spawn, parts ++ [dungeon, index])`. `[A]` a `Deadspace`
/// dungeon whose faction has no deadspace pool draws `Asteroid`. Empty for a
/// dungeon with no faction. Positions, triggers and respawn are the spawn
/// row's own fields, which the caller (CP28b) applies.
pub fn dungeon_squads(
    d: &crate::dungeon_data::Dungeon,
    data: &Table,
    seed: u64,
    parts: &[u64],
) -> Vec<SlotSquad> {
    let Some(faction) = d.faction else {
        return Vec::new();
    };
    if d.spawns.is_empty() {
        return Vec::new();
    }
    // Y-WH: a J-space dungeon's guards are Sleepers, which pay no bounty
    // and so cannot be drawn by the budgeted fill below.
    if data.sleeper_dungeons.contains(&d.id) {
        return crate::wormhole_sites::squads(d, data, seed, parts);
    }
    let filter = |context| Filter {
        faction_id: faction,
        context,
        classes: DUNGEON_CLASSES,
        tiers: DUNGEON_TIERS,
    };
    let mut cands = pool(data, filter(dungeon_context(d.archetype)));
    if cands.is_empty() {
        cands = pool(data, filter(ASTEROID));
    }
    let each = dungeon_budget(d.difficulty) / d.spawns.len() as f64;
    d.spawns
        .iter()
        .enumerate()
        .map(|(i, sp)| {
            let mut all = parts.to_vec();
            all.extend_from_slice(&[d.id as u64, i as u64]);
            let mut s = stream(seed, Kind::Spawn, &all);
            SlotSquad {
                room: sp.room,
                object: sp.object,
                spawn: i,
                squad: squad(
                    Some(faction),
                    fill(&cands, &cands, each, DUNGEON_SQUAD_SIZE, &mut s),
                ),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::npc_data::{Bounty, NpcTypeRow, Role};

    const PIRATE: i64 = 599_061;
    const OTHER: i64 = 599_062;

    fn row(
        type_id: i64,
        faction_id: i64,
        context: &str,
        class: &str,
        tier: Tier,
        needs: Needs,
    ) -> NpcTypeRow {
        NpcTypeRow {
            type_id,
            group_id: 99_600,
            faction_id,
            context: context.to_owned(),
            class: class.to_owned(),
            tier,
            alias_of: None,
            needs,
            role: Role::Dps,
        }
    }

    /// Invented rows: two pirates with a full class ladder each, some
    /// unsupported (missile) types, an alias, commanders and a hauler.
    fn fixture() -> Table {
        let mut t = Table::default();
        let mut id = 990_600_000;
        let classes = [
            "Frigate",
            "Destroyer",
            "Cruiser",
            "BattleCruiser",
            "Battleship",
        ];
        for f in [PIRATE, OTHER] {
            for (k, class) in classes.iter().enumerate() {
                for (n, tier) in [Tier::Base, Tier::Base, Tier::Elite]
                    .into_iter()
                    .enumerate()
                {
                    for (context, needs) in [
                        (ASTEROID, Needs::TURRET),
                        (ASTEROID, Needs::MISSILE),
                        (DEADSPACE, Needs::TURRET.union(Needs::SELF_REP)),
                    ] {
                        id += 1;
                        t.types.insert(id, row(id, f, context, class, tier, needs));
                        let bounty = 3_000.0 * 4f64.powi(k as i32) * (1.0 + n as f64);
                        t.bounty.insert(
                            id,
                            Bounty {
                                bounty,
                                ..Bounty::default()
                            },
                        );
                    }
                }
                id += 1;
                let cmd = format!("Commander {class}");
                t.types.insert(
                    id,
                    row(id, f, ASTEROID, &cmd, Tier::Commander, Needs::TURRET),
                );
                t.bounty.insert(
                    id,
                    Bounty {
                        bounty: 50_000.0 * (k + 1) as f64,
                        ..Bounty::default()
                    },
                );
            }
            id += 1;
            t.types.insert(
                id,
                row(id, f, ASTEROID, "Hauler", Tier::Base, Needs::default()),
            );
            t.bounty.insert(
                id,
                Bounty {
                    bounty: 9_000.0,
                    ..Bounty::default()
                },
            );
            // An alias of the first frigate: never drawn.
            id += 1;
            let mut alias = row(id, f, ASTEROID, "Frigate", Tier::Base, Needs::TURRET);
            alias.alias_of = Some(id - 1);
            t.types.insert(id, alias);
            t.bounty.insert(
                id,
                Bounty {
                    bounty: 1.0,
                    ..Bounty::default()
                },
            );
        }
        t
    }

    const SECS: [f64; 9] = [1.0, 0.9, 0.7, 0.5, 0.45, 0.3, 0.1, 0.0, -0.8];

    fn squads() -> impl Iterator<Item = (f64, u64, Squad)> {
        let t = fixture();
        SECS.into_iter().flat_map(move |sec| {
            let t = t.clone();
            (0..300u64)
                .map(move |seed| (sec, seed, belt_squad(&t, Some(PIRATE), sec, seed, &[7, 1])))
        })
    }

    #[test]
    fn a_squads_bounty_sum_is_at_most_the_budget() {
        let t = fixture();
        for (sec, _, s) in squads() {
            let sum: f64 = s.members.iter().map(|m| bounty_of(&t, *m)).sum();
            assert_eq!(sum, s.bounty);
            assert!(
                s.bounty <= budget(sec),
                "{sec}: {} > {}",
                s.bounty,
                budget(sec)
            );
            assert!(!s.members.is_empty());
        }
    }

    #[test]
    fn no_type_outside_supported() {
        let t = fixture();
        for (_, _, s) in squads() {
            for m in &s.members {
                assert!(SUPPORTED.contains(t.types[m].needs), "{m}");
                assert!(t.types[m].alias_of.is_none(), "{m} is an alias");
            }
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_squad() {
        let t = fixture();
        for sec in SECS {
            for seed in 0..50 {
                let a = belt_squad(&t, Some(PIRATE), sec, seed, &[3, 4]);
                assert_eq!(a, belt_squad(&t, Some(PIRATE), sec, seed, &[3, 4]));
            }
        }
        let d = dungeon();
        assert_eq!(
            dungeon_squads(&d, &t, 9, &[1]),
            dungeon_squads(&d, &t, 9, &[1])
        );
    }

    #[test]
    fn every_member_is_of_the_requested_faction() {
        let t = fixture();
        for (_, _, s) in squads() {
            assert_eq!(s.faction_id, Some(PIRATE));
            assert!(s.members.iter().all(|m| t.types[m].faction_id == PIRATE));
        }
        for sec in SECS {
            let s = belt_squad(&t, Some(OTHER), sec, 1, &[]);
            assert!(s.members.iter().all(|m| t.types[m].faction_id == OTHER));
        }
    }

    #[test]
    fn members_are_inside_the_bands_class_window_and_tiers() {
        let t = fixture();
        for (sec, _, s) in squads() {
            let (lead, escorts) = classes(sec);
            for (i, m) in s.members.iter().enumerate() {
                let r = &t.types[m];
                let special =
                    matches!(r.tier, Tier::Commander | Tier::Officer) || r.class == "Hauler";
                if special {
                    assert_eq!(i, 0, "a special only leads");
                    assert_ne!(Band::of(sec), Band::High, "no special in high-sec here");
                    continue;
                }
                let window = if i == 0 { &lead } else { &escorts };
                assert!(window.contains(&r.class.as_str()), "{sec} {}", r.class);
                assert!(tiers(sec).contains(&r.tier));
                assert_eq!(r.context, ASTEROID);
            }
        }
    }

    #[test]
    fn the_budget_falls_with_security_and_hits_its_anchors() {
        for (s, b) in BUDGET_ANCHORS {
            assert!((budget(s) - b).abs() < 1e-6 * b);
        }
        let mut last = f64::MAX;
        for i in -15..=12 {
            let b = budget(f64::from(i) / 10.0);
            assert!(b <= last, "{i}: {b} > {last}");
            last = b;
        }
    }

    #[test]
    fn with_no_bundle_a_belt_still_gets_a_hand_squad() {
        let t = Table::fixture();
        for sec in SECS {
            for seed in 0..20 {
                let s = belt_squad(&t, None, sec, seed, &[1]);
                assert_eq!(s.faction_id, None);
                assert!(!s.members.is_empty());
                assert!(s.members.iter().all(|m| crate::npc::find(*m).is_some()));
            }
        }
        // A faction with no loaded pool falls back the same way.
        let s = belt_squad(&t, Some(OTHER), 1.0, 0, &[]);
        assert_eq!(s.faction_id, None);
        // And through a registry with no bundle.
        let w = crate::service::Registry::new(crate::model::SandboxConfig::default());
        assert!(!belt_squad_in(&w, 30_000_001, &[0, 0]).members.is_empty());
    }

    fn dungeon() -> crate::dungeon_data::Dungeon {
        let spawn = |object| crate::dungeon_data::Spawn {
            room: 1,
            object,
            ..Default::default()
        };
        crate::dungeon_data::Dungeon {
            id: 990_610_001,
            archetype: DEADSPACE_ARCHETYPES[0],
            faction: Some(PIRATE),
            difficulty: Some(3),
            spawns: vec![spawn(11), spawn(12), spawn(13)],
            ..Default::default()
        }
    }

    #[test]
    fn dungeon_squads_split_the_budget_and_keep_the_faction() {
        let t = fixture();
        let d = dungeon();
        for seed in 0..100 {
            let got = dungeon_squads(&d, &t, seed, &[5]);
            assert_eq!(got.len(), d.spawns.len());
            let each = dungeon_budget(d.difficulty) / d.spawns.len() as f64;
            for (i, g) in got.iter().enumerate() {
                assert_eq!((g.spawn, g.object), (i, d.spawns[i].object));
                assert!(g.squad.bounty <= each);
                assert!(g.squad.members.len() as u64 <= DUNGEON_SQUAD_SIZE.1);
                for m in &g.squad.members {
                    let r = &t.types[m];
                    assert_eq!(r.faction_id, PIRATE);
                    assert_eq!(r.context, dungeon_context(d.archetype));
                    assert!(SUPPORTED.contains(r.needs));
                }
            }
        }
        let none = crate::dungeon_data::Dungeon { faction: None, ..d };
        assert!(dungeon_squads(&none, &t, 0, &[]).is_empty());
    }
}
