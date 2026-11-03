//! **Wormhole space with content** (Y-WH, series T3-WH-SPACE): which of the
//! client's dungeons are J-space ones, which Sleeper ships guard them, and
//! what a Sleeper wreck gives a salvager.
//!
//! Lane note: `docs/lanes/y-wh.md`; the gap analysis is
//! `docs/lanes/t3-supply.md` lane 3.
//!
//! # Three authored tables, one module
//!
//! 1. **`wormholes/sites.tsv`** ([`Sites`]): the client's dungeon files carry
//!    no wormhole class (every J-space dungeon is faction 500021, a faction
//!    that also holds mission and null-sec dungeons). The 67 J-space
//!    dungeons, and the classes C1-C6 each belongs to, are EvEJS facts joined
//!    to the client's ids. `population::Prepared::with_wormhole` draws a
//!    J-space system's sites only from the rows this table lists for its
//!    class, so the old exclusions (`dungeon_data::EXCLUDED_FACTIONS`,
//!    `EXCLUDED_GAS_TYPES`) stay in force everywhere else and are lifted
//!    **for wormhole space only**.
//! 2. **`wormholes/sleepers.tsv`** ([`SleeperRow`]): the Sleeper NPC types.
//!    The extractor's faction grammar never parsed the Deadspace Sleeper
//!    groups, so `npc/types.tsv` has no row for them
//!    (`tools/wh_sleepers.py` derives them from the same effects).
//!    [`with_sleepers`] merges them into [`npc_data::Table`] as faction
//!    [`FACTION`], context Deadspace, tier deadspace, with one class string
//!    per `(rank, hull)`.
//! 3. **The salvage overlay** ([`salvage_overlay`]): a Sleeper wreck's
//!    yield is Ancient Salvage, the 19 types of the bundle's own
//!    `explore/loot.tsv` class `ancient_salvage`.
//!
//! # Sleeper squads ([`squads`])
//!
//! `npc_gen::dungeon_squads` fills a squad to a **bounty budget**, and a
//! Sleeper pays none (`npc/bounty.tsv` has no row for any Sleeper type
//! [V-data]: they drop salvage and components, not ISK). So a Sleeper site
//! gets its own fill: per spawn row of the dungeon, `1..=`[`SQUAD`] ships,
//! the rank by the site's difficulty ([`RANK_WEIGHTS`]) and the hull by the
//! same ([`HULL_WEIGHTS`]), both [A].
//!
//! The ships fly with K-RATAI's brain: every Sleeper type has attribute 1648
//! (target switching) [V-data], and the logistics types carry the remote
//! repair effects 3852 and 6741/6742 the brain already repairs allies with.
//! `NEUT` and `EWAR` are not implemented ([`SLEEPER_SUPPORTED`]): those
//! types fly as weaker ships, as a self-repairer did before CP23.

use crate::content::{Boxed, Sources, TableSpec};
use crate::npc_data::{self, Faction, Needs, NpcTypeRow, Role, Tier};
use crate::npc_gen::{SlotSquad, Squad};
use crate::salvage;
use crate::seed::{Kind, Stream, stream};
use std::collections::{BTreeMap, BTreeSet};

/// The sites file, relative to the bundle (`out/content`). It joins EvEJS
/// facts to the client's dungeon ids, so it is client data (class B,
/// `content/README.md`) and never lives in the repository.
pub const SITES_FILE: &str = "wormholes/sites.tsv";
/// Its header, frozen in `docs/content/schema.md` §4.
pub const SITES_HEADER: &str = "dungeon_id\tclass_lo\tclass_hi\tfamily\twhy";

/// The Sleeper types file, relative to the bundle: written by
/// `tools/wh_sleepers.py` from the client's type data (class B).
pub const SLEEPERS_FILE: &str = "wormholes/sleepers.tsv";
/// Its header, frozen in `docs/content/schema.md` §4.
pub const SLEEPERS_HEADER: &str = "type_id\tgroup_id\tclass\trank\tneeds\trole\twreck_type_id\twhy";

/// The rows of the bundle's file `rel` (class B data, never compiled in): none
/// when the bundle lacks the file, so a world without it has no J-space content.
fn bundle_rows(
    src: &Sources,
    rel: &str,
    header: &str,
) -> Result<Vec<crate::authored::Row>, String> {
    let Some(path) = src.bundle.and_then(|b| b.path(rel)) else {
        crate::errln!("content: {rel} absent from the bundle; no wormhole content");
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (rows, _) = crate::authored::parse(&text, header).map_err(|e| format!("{rel}: {e}"))?;
    Ok(rows)
}

/// Parse every row, logging (and skipping) the ones `f` refuses.
fn parse_logged<T>(
    rel: &str,
    rows: &[crate::authored::Row],
    f: impl Fn(&crate::authored::Row) -> Result<T, String>,
) -> Vec<T> {
    rows.iter()
        .filter_map(|r| match f(r) {
            Ok(t) => Some(t),
            Err(e) => {
                crate::errln!("content: {rel}: a row skipped: {e}");
                None
            }
        })
        .collect()
}

/// The registration row in `content_tables::TABLES` (coordinator-only).
pub const TABLE: TableSpec = TableSpec {
    name: "wormhole_sites",
    load: boxed_load,
    fixture: boxed_fixture,
};

/// The faction every J-space dungeon and every Sleeper ship carries
/// (`dungeons.tsv`, `types.factionID`) **[V-data]**; the client names it
/// "Unknown". The name "Sleeper" is [I] (the dungeons that hold Sleeper
/// cans), `t3-supply.md`'s third [A].
pub const FACTION: i64 = 500_021;
/// **[A]** The owner id of a Sleeper ship: the faction itself, because the
/// faction has no executor corporation in the client (`npc/factions.tsv`
/// drops it). An owner id may be a faction. Falsified by a client that
/// shows a Sleeper's owner as an error.
pub const OWNER: i64 = FACTION;
/// How long after a site opens its delayed Sleepers arrive, seconds
/// (`site_spawn::DELAY_SPAWN` [A]): what a bot that hit-and-runs a
/// reservoir budgets against.
pub const ARRIVAL_DELAY_S: i64 = crate::site_spawn::DELAY_SPAWN;
/// The J-space classes with content: C1 to C6.
pub const CLASSES: std::ops::RangeInclusive<i64> = 1..=6;
/// The race of a Sleeper hull in `types/market.tsv` **[V-data]**; the
/// salvage table keys yields by `(size, race)`.
pub const RACE: i64 = 64;

fn boxed_load(src: &Sources) -> Result<Boxed, String> {
    if src.bundle.is_none() {
        return Ok(Box::new(Sites::default()));
    }
    Ok(Box::new(Sites::load(src)?))
}

fn boxed_fixture() -> Boxed {
    Box::new(Sites::fixture())
}

/// A site's family (informational: the draw is by the population row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Combat,
    Relic,
    Data,
    Gas,
    Ore,
}

impl Family {
    fn parse(s: &str) -> Result<Family, String> {
        Ok(match s {
            "combat" => Family::Combat,
            "relic" => Family::Relic,
            "data" => Family::Data,
            "gas" => Family::Gas,
            "ore" => Family::Ore,
            _ => {
                return Err(format!(
                    "family {s:?} is not combat, relic, data, gas or ore"
                ));
            }
        })
    }
}

/// One `sites.tsv` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteRow {
    pub dungeon: i64,
    pub class_lo: i64,
    pub class_hi: i64,
    pub family: Family,
}

fn parse_site(r: &crate::authored::Row) -> Result<SiteRow, String> {
    let row = SiteRow {
        dungeon: r.i64(0)?,
        class_lo: r.i64(1)?,
        class_hi: r.i64(2)?,
        family: Family::parse(r.field(3))?,
    };
    if !(CLASSES.contains(&row.class_lo)
        && CLASSES.contains(&row.class_hi)
        && row.class_lo <= row.class_hi)
    {
        return Err(format!(
            "classes {}..{} are not inside C1-C6",
            row.class_lo, row.class_hi
        ));
    }
    Ok(row)
}

/// The J-space dungeons.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sites {
    rows: BTreeMap<i64, SiteRow>,
    /// A k-space system's static chance, overriding
    /// `wormhole::KSPACE_HOLE_CHANCE`. `None` in the game; set by a test
    /// that needs the hole to exist ([`Sites::with_hole_chance`]).
    hole_chance: Option<f64>,
}

impl Sites {
    /// # Errors
    ///
    /// A dungeon listed twice (the file's key).
    pub fn new(rows: Vec<SiteRow>) -> Result<Sites, String> {
        let mut map = BTreeMap::new();
        for r in rows {
            if map.insert(r.dungeon, r).is_some() {
                return Err(format!("dungeon {} is listed twice", r.dungeon));
            }
        }
        Ok(Sites {
            rows: map,
            hole_chance: None,
        })
    }

    /// **Test hook**: every k-space system holds a static with this chance
    /// (`wormhole::make` reads it), so a socket test can have the hole it
    /// needs without knowing the world's seed. The game never sets it.
    pub fn with_hole_chance(mut self, chance: f64) -> Sites {
        self.hole_chance = Some(chance);
        self
    }

    pub fn hole_chance(&self) -> Option<f64> {
        self.hole_chance
    }

    /// Whether any listed dungeon may stand in a system of J-space `class`.
    pub fn serves(&self, class: i64) -> bool {
        self.rows
            .values()
            .any(|r| (r.class_lo..=r.class_hi).contains(&class))
    }

    fn load(src: &Sources) -> Result<Sites, String> {
        let raw = bundle_rows(src, SITES_FILE, SITES_HEADER)?;
        let rows = parse_logged(SITES_FILE, &raw, parse_site);
        Sites::new(rows).map_err(|e| format!("{SITES_FILE}: {e}"))
    }

    /// Invented rows over invented dungeons (990_5xx): never a client id.
    pub fn fixture() -> Sites {
        let text = format!(
            "{SITES_HEADER}\n\
             990501\t1\t6\tgas\tx\n\
             990502\t3\t6\tgas\tx\n\
             990503\t5\t6\tgas\tx\n\
             990504\t1\t1\tcombat\tx\n\
             990505\t6\t6\trelic\tx\n"
        );
        let (raw, _) = crate::authored::parse(&text, SITES_HEADER).expect("the fixture header");
        let rows = raw.iter().map(|r| parse_site(r).expect("a fixture row"));
        Sites::new(rows.collect()).expect("the fixture is well-formed")
    }

    /// Whether the dungeon is a J-space one at all.
    pub fn contains(&self, dungeon: i64) -> bool {
        self.rows.contains_key(&dungeon)
    }

    /// Whether the dungeon may stand in a system of J-space `class`.
    pub fn admits(&self, dungeon: i64, class: i64) -> bool {
        self.rows
            .get(&dungeon)
            .is_some_and(|r| (r.class_lo..=r.class_hi).contains(&class))
    }

    /// Every listed dungeon id, ascending.
    pub fn dungeons(&self) -> BTreeSet<i64> {
        self.rows.keys().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

// ---- the Sleeper types ------------------------------------------------------

/// A Sleeper hull class, from the client's group (patroller family =
/// frigates, defender = cruisers, sentinel = battleships) **[V-data]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hull {
    Frigate,
    Cruiser,
    Battleship,
}

impl Hull {
    const ALL: [Hull; 3] = [Hull::Frigate, Hull::Cruiser, Hull::Battleship];

    fn parse(s: &str) -> Result<Hull, String> {
        Hull::ALL
            .into_iter()
            .find(|h| h.name() == s)
            .ok_or_else(|| format!("class {s:?} is not Frigate, Cruiser or Battleship"))
    }

    pub fn name(self) -> &'static str {
        match self {
            Hull::Frigate => "Frigate",
            Hull::Cruiser => "Cruiser",
            Hull::Battleship => "Battleship",
        }
    }

    /// The salvage table's wreck size.
    pub fn size(self) -> u8 {
        match self {
            Hull::Frigate => 1,
            Hull::Cruiser => 2,
            Hull::Battleship => 3,
        }
    }
}

/// A Sleeper rank, from the client's group name **[V]**: Emergent, Awakened,
/// Sleepless. Their hit points rise in that order **[V-data]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    Emergent,
    Awakened,
    Sleepless,
}

impl Rank {
    const ALL: [Rank; 3] = [Rank::Emergent, Rank::Awakened, Rank::Sleepless];

    fn parse(s: &str) -> Result<Rank, String> {
        Rank::ALL
            .into_iter()
            .find(|r| r.word() == s)
            .ok_or_else(|| format!("rank {s:?} is not emergent, awakened or sleepless"))
    }

    fn word(self) -> &'static str {
        match self {
            Rank::Emergent => "emergent",
            Rank::Awakened => "awakened",
            Rank::Sleepless => "sleepless",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Rank::Emergent => "Emergent",
            Rank::Awakened => "Awakened",
            Rank::Sleepless => "Sleepless",
        }
    }
}

/// The `class` string of a `(rank, hull)` in `npc_data` (`Sleepless
/// Battleship`): the key of the faction's wreck map and of [`squads`]'s
/// buckets.
pub fn class_name(rank: Rank, hull: Hull) -> String {
    format!("{} {}", rank.title(), hull.name())
}

/// One `sleepers.tsv` row.
#[derive(Debug, Clone, PartialEq)]
pub struct SleeperRow {
    pub type_id: i64,
    pub group_id: i64,
    pub hull: Hull,
    pub rank: Rank,
    pub needs: Needs,
    pub role: Role,
    pub wreck_type_id: Option<i64>,
}

fn parse_sleeper(r: &crate::authored::Row) -> Result<SleeperRow, String> {
    Ok(SleeperRow {
        type_id: r.i64(0)?,
        group_id: r.i64(1)?,
        hull: Hull::parse(r.field(2))?,
        rank: Rank::parse(r.field(3))?,
        needs: Needs::parse(r.field(4))?,
        role: Role::parse(r.field(5))?,
        wreck_type_id: match r.field(6) {
            "" => None,
            _ => Some(r.i64(6)?),
        },
    })
}

/// The Sleeper rows of the authored file; **empty without a bundle**, where
/// the world is the invented fixture and a client type id means nothing.
///
/// # Errors
///
/// A file whose header is wrong.
pub fn sleeper_rows(src: &Sources) -> Result<Vec<SleeperRow>, String> {
    if src.bundle.is_none() {
        return Ok(Vec::new());
    }
    let raw = bundle_rows(src, SLEEPERS_FILE, SLEEPERS_HEADER)?;
    Ok(parse_logged(SLEEPERS_FILE, &raw, parse_sleeper))
}

/// The J-space dungeon ids the loader knows, for `npc_data`'s
/// `sleeper_dungeons` (the squads' switch). Empty without a bundle.
pub fn site_ids(src: &Sources) -> Result<BTreeSet<i64>, String> {
    if src.bundle.is_none() {
        return Ok(BTreeSet::new());
    }
    Ok(Sites::load(src)?.dungeons())
}

/// **Merge the Sleepers into `npc_data`**: the faction row (owner
/// [`OWNER`], wreck by class string), one type row per Sleeper (context
/// `Deadspace`, tier deadspace, class [`class_name`]) and the J-space
/// dungeons that take [`squads`]. A type already listed (a future extractor
/// that parses the groups) is left as the bundle has it.
pub fn with_sleepers(
    mut t: npc_data::Table,
    rows: &[SleeperRow],
    sites: BTreeSet<i64>,
) -> npc_data::Table {
    if rows.is_empty() {
        return t;
    }
    let mut wrecks = BTreeMap::new();
    for r in rows {
        if let Some(w) = r.wreck_type_id {
            wrecks.entry(class_name(r.rank, r.hull)).or_insert(w);
        }
        t.types.entry(r.type_id).or_insert_with(|| NpcTypeRow {
            type_id: r.type_id,
            group_id: r.group_id,
            faction_id: FACTION,
            context: crate::npc_gen::DEADSPACE.to_owned(),
            class: class_name(r.rank, r.hull),
            tier: Tier::Deadspace,
            alias_of: None,
            needs: r.needs,
            role: r.role,
        });
    }
    t.factions.entry(FACTION).or_insert_with(|| Faction {
        faction_id: FACTION,
        corporation_id: OWNER,
        wrecks,
        name: "Sleepers".to_owned(),
        ..Faction::default()
    });
    t.sleeper_dungeons = sites;
    t
}

// ---- squads -------------------------------------------------------------------

/// **[A]** A spawn row's squad size, inclusive: a J-space site has many
/// spawn rows (a relic site 16, a combat anomaly up to 22), so one row is a
/// pair or a trio, not the 1-5 of a pirate dungeon. Falsified by a Sleeper
/// site a human calls empty or an unfightable wall.
pub const SQUAD: (u64, u64) = (1, 3);

/// **[A]** Rank weights `[emergent, awakened, sleepless]` by the site's
/// difficulty (Perimeter 3, Frontier 4, Core 5, which the client's own
/// difficulty field says [V-data]); a lower difficulty reads as 3, a higher
/// as 5. Falsified by a C1 site that holds Sleepless battleships, or a C6
/// one with frigates only.
pub const RANK_WEIGHTS: [(i64, [u32; 3]); 3] =
    [(3, [70, 30, 0]), (4, [20, 60, 20]), (5, [0, 30, 70])];

/// **[A]** Hull weights `[frigate, cruiser, battleship]` by the same
/// difficulty. Falsified as [`RANK_WEIGHTS`].
pub const HULL_WEIGHTS: [(i64, [u32; 3]); 3] =
    [(3, [40, 40, 20]), (4, [20, 40, 40]), (5, [10, 30, 60])];

/// **[A]** What a Sleeper pool may need: everything the pirates may
/// ([`crate::npc_gen::SUPPORTED`]) plus `NEUT` and `EWAR`, which fly as
/// weaker ships (their effect does nothing yet; K-EWAR). Falsified by a
/// Sleeper whose missing neutralizer shows.
pub const SLEEPER_SUPPORTED: Needs =
    Needs(crate::npc_gen::SUPPORTED.0 | Needs::NEUT.0 | Needs::EWAR.0);

/// `SLEEPER_SUPPORTED` is every capability bit there is, so the supported
/// filter in [`squads`] drops nothing today; it stays so that a capability
/// added to `Needs` later is not drawn until the server can fly it.
const _: () = assert!(SLEEPER_SUPPORTED.0 == 0xFF);

fn weights_for(table: &[(i64, [u32; 3])], difficulty: Option<i64>) -> [u32; 3] {
    let d = difficulty.unwrap_or(3).clamp(3, 5);
    table
        .iter()
        .find(|(k, _)| *k == d)
        .map_or([1, 1, 1], |(_, w)| *w)
}

fn weighted(w: &[u32; 3], s: &mut Stream) -> usize {
    let total: u64 = w.iter().map(|x| u64::from(*x)).sum();
    if total == 0 {
        return 0;
    }
    let mut at = s.range(0, total);
    for (i, x) in w.iter().enumerate() {
        if at < u64::from(*x) {
            return i;
        }
        at -= u64::from(*x);
    }
    w.len() - 1
}

/// The Sleeper types of `data`, by `(rank, hull)`, ascending by type id.
fn buckets(data: &npc_data::Table) -> BTreeMap<(Rank, Hull), Vec<i64>> {
    let mut out: BTreeMap<(Rank, Hull), Vec<i64>> = BTreeMap::new();
    for r in data.types.values() {
        if r.faction_id != FACTION
            || r.alias_of.is_some()
            || !SLEEPER_SUPPORTED.contains(r.needs)
            || r.context != crate::npc_gen::DEADSPACE
        {
            continue;
        }
        let key = Rank::ALL.into_iter().find_map(|rank| {
            Hull::ALL
                .into_iter()
                .find(|h| class_name(rank, *h) == r.class)
                .map(|h| (rank, h))
        });
        if let Some(k) = key {
            out.entry(k).or_default().push(r.type_id);
        }
    }
    out
}

/// **A Sleeper site's squads**: one per spawn row of `d`, drawn from
/// `stream(seed, Spawn, parts ++ [dungeon, index])` like
/// `npc_gen::dungeon_squads`, so the same seed gives the same squad. Empty
/// for a dungeon with no spawns or a world with no Sleeper types.
pub fn squads(
    d: &crate::dungeon_data::Dungeon,
    data: &npc_data::Table,
    seed: u64,
    parts: &[u64],
) -> Vec<SlotSquad> {
    let pool = buckets(data);
    if pool.is_empty() {
        return Vec::new();
    }
    let rank_w = weights_for(&RANK_WEIGHTS, d.difficulty);
    let hull_w = weights_for(&HULL_WEIGHTS, d.difficulty);
    d.spawns
        .iter()
        .enumerate()
        .map(|(i, sp)| {
            let mut all = parts.to_vec();
            all.extend_from_slice(&[d.id as u64, i as u64]);
            let mut s = stream(seed, Kind::Spawn, &all);
            let n = s.range(SQUAD.0, SQUAD.1 + 1);
            let mut members = Vec::new();
            for _ in 0..n {
                let rank = Rank::ALL[weighted(&rank_w, &mut s)];
                let hull = Hull::ALL[weighted(&hull_w, &mut s)];
                // The wanted bucket, else the nearest non-empty one.
                let list = pool.get(&(rank, hull)).or_else(|| {
                    pool.iter()
                        .min_by_key(|((r, h), _)| {
                            (
                                (*r as i64 - rank as i64).abs() + (*h as i64 - hull as i64).abs(),
                                *r,
                                *h,
                            )
                        })
                        .map(|(_, v)| v)
                });
                if let Some(list) = list {
                    members.push(list[s.range(0, list.len() as u64) as usize]);
                }
            }
            SlotSquad {
                room: sp.room,
                object: sp.object,
                spawn: i,
                squad: Squad {
                    faction_id: Some(FACTION),
                    members,
                    bounty: 0.0,
                },
            }
        })
        .collect()
}

// ---- the GM jump -----------------------------------------------------------

/// **The system `arg` names**: a class `1..=6`, meaning the lowest-id
/// J-space system of that class, or any system id the universe lists (the
/// way home from J-space is a GM jump too). `None` for anything else.
pub fn pick_system(u: &crate::celestial::Universe, arg: &str) -> Option<i64> {
    let n: i64 = arg.parse().ok()?;
    if CLASSES.contains(&n) {
        return u
            .systems()
            .filter(|s| crate::celestial::WORMHOLE_SYSTEMS.contains(&s.id))
            .find(|s| u.wormhole_class(s.id) == Some(n))
            .map(|s| s.id);
    }
    u.system(n).map(|s| s.id)
}

/// **The lowest-id J-space system `has_beacon` says carries an effect
/// beacon** (`/whjump fx`, SYS-3): a scenario that must stand under a
/// black hole cannot know which system has one. The slash passes a test for
/// a beacon that moves a ship attribute (a Red Giant's effects aim at
/// modules and charges only).
pub fn pick_with_effect(
    u: &crate::celestial::Universe,
    has_beacon: impl Fn(i64) -> bool,
) -> Option<i64> {
    u.systems()
        .map(|s| s.id)
        .filter(|id| crate::celestial::WORMHOLE_SYSTEMS.contains(id) && has_beacon(*id))
        .min()
}

/// **`/whjump <class|systemID|fx>`**: a GM tool, so a human or a client
/// scenario can stand in J-space without finding a hole (and leave it). The pilot (in
/// space) leaves for the system as a jump-drive jump does
/// (`jump_drive::depart`) and lands 5,000 km off its first planet [A].
/// Not a rule of the game: wormholes are the only way in for a player.
pub fn slash(
    reg: &mut crate::service::Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<eve_marshal::PyValue, crate::service::ServiceError> {
    let say = |t: &str| Ok(crate::pyutil::bstr(t));
    let Some(actor) = actor else {
        return say("no character");
    };
    let Some(arg) = args.first() else {
        return say("usage: /whjump <class 1-6 | systemID | fx | fxmod>");
    };
    let content = reg.content();
    let system = content
        .try_get::<crate::celestial::Universe>()
        .and_then(|u| {
            if *arg == "fxmod" {
                pick_with_effect(u, |id| {
                    crate::system_effect::beacon_of(content, id).is_some_and(|r| {
                        crate::system_effect::moves_modules(content, r.beacon_type_id)
                    })
                })
            } else if *arg == "fx" {
                pick_with_effect(u, |id| {
                    crate::system_effect::beacon_of(content, id).is_some_and(|r| {
                        crate::system_effect::slows_ships(content, r.beacon_type_id)
                    })
                })
            } else {
                pick_system(u, arg)
            }
        });
    let Some(system) = system else {
        return say(&format!("{arg}: no such wormhole system or class"));
    };
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return say("you are not in space: undock first");
    };
    let planet = reg
        .celestials()
        .served_in(system)
        .find(|c| matches!(c.kind, crate::celestial::Kind::Planet))
        .map_or([0.0; 3], |c| c.position);
    let spot = [planet[0] + 5.0e9, planet[1], planet[2]];
    crate::jump_drive::depart(reg, actor, here.ship_id, here.solar_system_id, system, spot);
    say(&format!("jumping to {system}"))
}

// ---- the salvage overlay ------------------------------------------------------

/// **[A]** How many Ancient Salvage a draw of a Sleeper wreck gives, and
/// each type's weight: the 19 types are equal. The client has no yield
/// table for Sleeper wrecks (`salvage/*.tsv` is built from ship hulls only,
/// category 6) and the type ids come from the bundle's own
/// `ancient_salvage` loot class. That Sleeper wrecks yield Ancient Salvage
/// at all is game knowledge, not client data. Falsified by the first soak
/// that prints Sleeper wrecks salvaged against Ancient Salvage found.
pub const SALVAGE_QTY: (i64, i64) = (1, 3);

/// The `ancient_salvage` class of the bundle's `explore/loot.tsv`.
fn ancient_salvage_types(b: &crate::content::Bundle) -> Result<Vec<i64>, String> {
    const HEADER: &str = "#kind\tclass\ttype_id\tweight\tqty_min\tqty_max\tcopy\tvalue";
    let mut out = Vec::new();
    for line in b.data_lines("explore/loot.tsv", HEADER)? {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() > 2 && f[0] == "ancient" && f[1] == "ancient_salvage" {
            out.push(
                f[2].parse()
                    .map_err(|_| format!("explore/loot.tsv: {line:?}"))?,
            );
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// The wreck and yield lines the salvage table merges: every Sleeper wreck
/// type as `(size, RACE)`, and for each size every Ancient Salvage type.
/// Empty without a bundle.
///
/// # Errors
///
/// A bad `explore/loot.tsv` or sleepers file.
pub fn salvage_overlay(src: &Sources) -> Result<salvage::Table, String> {
    let Some(b) = src.bundle else {
        return Ok(salvage::Table::default());
    };
    let rows = sleeper_rows(src)?;
    let types = ancient_salvage_types(b)?;
    Ok(overlay_of(&rows, &types))
}

/// [`salvage_overlay`] over given rows and types, for tests.
pub fn overlay_of(rows: &[SleeperRow], ancient: &[i64]) -> salvage::Table {
    let mut wrecks = Vec::new();
    let mut seen = BTreeSet::new();
    for r in rows {
        if let Some(w) = r.wreck_type_id
            && seen.insert(w)
        {
            wrecks.push(format!("{w}\t{}\t{RACE}", r.hull.size()));
        }
    }
    let mut yields = Vec::new();
    if !wrecks.is_empty() {
        for size in 1..=3u8 {
            for t in ancient {
                yields.push(format!(
                    "{size}\t{RACE}\t{t}\t1\t{}\t{}\t0.0",
                    SALVAGE_QTY.0, SALVAGE_QTY.1
                ));
            }
        }
    }
    salvage::Table::parse(&wrecks, &yields).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeon_data::{Dungeon, Spawn};

    fn row(type_id: i64, hull: Hull, rank: Rank, needs: Needs) -> SleeperRow {
        SleeperRow {
            type_id,
            group_id: 990_600,
            hull,
            rank,
            needs,
            role: Role::Dps,
            wreck_type_id: Some(990_700 + hull.size() as i64),
        }
    }

    /// Invented Sleepers: one of every `(rank, hull)` (990_610 + index).
    fn rows() -> Vec<SleeperRow> {
        let mut out = Vec::new();
        for (i, rank) in Rank::ALL.into_iter().enumerate() {
            for (j, hull) in Hull::ALL.into_iter().enumerate() {
                out.push(row(990_610 + (i * 3 + j) as i64, hull, rank, Needs::TURRET));
            }
        }
        out
    }

    fn data() -> npc_data::Table {
        with_sleepers(
            npc_data::Table::fixture(),
            &rows(),
            BTreeSet::from([990_501]),
        )
    }

    fn dungeon(difficulty: i64, spawns: usize) -> Dungeon {
        Dungeon {
            id: 990_501,
            archetype: 24,
            faction: Some(FACTION),
            difficulty: Some(difficulty),
            spawns: (0..spawns)
                .map(|i| Spawn {
                    room: 1,
                    object: 8000 + i as i64,
                    entity: i as i64,
                    ..Spawn::default()
                })
                .collect(),
            ..Dungeon::default()
        }
    }

    /// Against the real bundle only (`EVE_TEST_BUNDLE=<out/content>`): the
    /// files are client data and never part of the repository's tests.
    #[test]
    fn the_bundle_files_parse_and_hold_every_class() {
        let Ok(dir) = std::env::var("EVE_TEST_BUNDLE") else {
            return;
        };
        let read =
            |rel: &str| std::fs::read_to_string(std::path::Path::new(&dir).join(rel)).unwrap();
        let sites_text = read(SITES_FILE);
        let (raw, skipped) = crate::authored::parse(&sites_text, SITES_HEADER).unwrap();
        assert!(skipped.is_empty());
        let sites = Sites::new(raw.iter().map(|r| parse_site(r).unwrap()).collect()).unwrap();
        for class in CLASSES {
            for family in [Family::Combat, Family::Relic, Family::Data, Family::Gas] {
                assert!(
                    sites
                        .rows
                        .values()
                        .any(|r| r.family == family && (r.class_lo..=r.class_hi).contains(&class)),
                    "C{class} has no {family:?} site"
                );
            }
        }
        let sleepers_text = read(SLEEPERS_FILE);
        let (raw, skipped) = crate::authored::parse(&sleepers_text, SLEEPERS_HEADER).unwrap();
        assert!(skipped.is_empty());
        let sleepers: Vec<_> = raw.iter().map(|r| parse_sleeper(r).unwrap()).collect();
        for rank in Rank::ALL {
            for hull in Hull::ALL {
                assert!(
                    sleepers.iter().any(|s| s.rank == rank && s.hull == hull),
                    "no {rank:?} {hull:?}"
                );
            }
        }
    }

    #[test]
    fn a_site_is_admitted_only_in_its_classes() {
        let s = Sites::fixture();
        assert!(s.admits(990_501, 1) && s.admits(990_501, 6));
        assert!(!s.admits(990_502, 2) && s.admits(990_502, 3));
        assert!(!s.admits(990_503, 4) && s.admits(990_503, 5));
        assert!(s.admits(990_504, 1) && !s.admits(990_504, 2));
        assert!(!s.admits(1, 1), "a dungeon the file does not list");
        assert!(s.contains(990_505) && !s.contains(1));
    }

    #[test]
    fn the_gm_jump_names_a_j_space_system_by_id_or_class() {
        let sys = |id: i64, class: i64| {
            format!(
                "{id}\t19000001\t29000001\t\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t{class}\t0\t\t\tWH {id}"
            )
        };
        let u = crate::celestial::Universe::parse(
            &[sys(31_990_005, 3), sys(31_990_002, 3), sys(31_990_001, 1)],
            &["19000001\t\t\t\t\tR".to_owned()],
            &["29000001\t19000001\t\t\t\tC".to_owned()],
        )
        .unwrap();
        assert_eq!(
            pick_system(&u, "3"),
            Some(31_990_002),
            "the lowest of the class"
        );
        assert_eq!(pick_system(&u, "1"), Some(31_990_001));
        assert_eq!(pick_system(&u, "31990005"), Some(31_990_005));
        assert_eq!(pick_system(&u, "31990009"), None, "not in the universe");
        assert_eq!(pick_system(&u, "6"), None, "no such class here");
        assert_eq!(
            pick_system(&u, "7"),
            None,
            "7 is neither a J class nor a system"
        );
        assert_eq!(pick_system(&u, "x"), None);
    }

    #[test]
    fn fx_picks_the_lowest_j_space_system_with_a_beacon() {
        let sys = |id: i64| {
            format!("{id}\t19000001\t29000001\t\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t3\t0\t\t\tWH {id}")
        };
        let u = crate::celestial::Universe::parse(
            &[sys(31_990_005), sys(31_990_002), sys(31_990_001)],
            &["19000001\t\t\t\t\tR".to_owned()],
            &["29000001\t19000001\t\t\t\tC".to_owned()],
        )
        .unwrap();
        let with = |ids: &'static [i64]| move |id: i64| ids.contains(&id);
        assert_eq!(
            pick_with_effect(&u, with(&[31_990_005, 31_990_002])),
            Some(31_990_002)
        );
        assert_eq!(pick_with_effect(&u, with(&[])), None);
        assert_eq!(
            pick_with_effect(&u, with(&[31_990_999])),
            None,
            "not in the universe"
        );
    }

    #[test]
    fn a_site_row_outside_c1_to_c6_or_listed_twice_is_refused() {
        let text = format!(
            "{SITES_HEADER}\n990501\t0\t6\tgas\tx\n990502\t5\t4\tgas\tx\n990503\t1\t2\tmoon\tx\n"
        );
        let (raw, _) = crate::authored::parse(&text, SITES_HEADER).unwrap();
        for r in &raw {
            assert!(parse_site(r).is_err(), "{r:?}");
        }
        let one = SiteRow {
            dungeon: 990_501,
            class_lo: 1,
            class_hi: 1,
            family: Family::Gas,
        };
        assert!(Sites::new(vec![one, one]).is_err());
    }

    #[test]
    fn the_sleepers_become_a_deadspace_faction_with_a_wreck_per_class() {
        let t = data();
        let f = &t.factions[&FACTION];
        assert_eq!(f.owner(), OWNER);
        assert_eq!(
            f.wrecks[&class_name(Rank::Sleepless, Hull::Battleship)],
            990_703
        );
        let r = &t.types[&990_610];
        assert_eq!(
            (r.faction_id, r.context.as_str(), r.tier, r.class.as_str()),
            (FACTION, "Deadspace", Tier::Deadspace, "Emergent Frigate")
        );
        assert!(t.sleeper_dungeons.contains(&990_501));
        assert!(
            !data().bounty.contains_key(&990_610),
            "a Sleeper pays no bounty"
        );
        assert_eq!(
            with_sleepers(npc_data::Table::fixture(), &[], BTreeSet::new()),
            npc_data::Table::fixture(),
            "no rows, no change"
        );
    }

    #[test]
    fn a_squad_per_spawn_row_with_no_bounty() {
        let t = data();
        let d = dungeon(4, 6);
        let got = squads(&d, &t, 7, &[1]);
        assert_eq!(got.len(), 6);
        for (i, g) in got.iter().enumerate() {
            assert_eq!((g.spawn, g.object), (i, 8000 + i as i64));
            assert!((1..=3).contains(&(g.squad.members.len() as u64)));
            assert_eq!(g.squad.faction_id, Some(FACTION));
            assert_eq!(g.squad.bounty, 0.0);
            assert!(
                g.squad
                    .members
                    .iter()
                    .all(|m| (990_610..990_619).contains(m))
            );
        }
        assert_eq!(
            got,
            squads(&d, &t, 7, &[1]),
            "the same seed, the same squads"
        );
        assert_ne!(got, squads(&d, &t, 8, &[1]));
        assert!(squads(&dungeon(4, 0), &t, 7, &[1]).is_empty());
        assert!(squads(&d, &npc_data::Table::fixture(), 7, &[1]).is_empty());
    }

    /// Rank follows the site's difficulty: a Perimeter site holds no
    /// Sleepless, a Core one no Emergent (the weights' zeros).
    #[test]
    fn rank_follows_difficulty() {
        let t = data();
        let ranks = |difficulty: i64| -> BTreeSet<Rank> {
            let mut seen = BTreeSet::new();
            for seed in 0..40 {
                for sq in squads(&dungeon(difficulty, 8), &t, seed, &[0]) {
                    for m in sq.squad.members {
                        seen.insert(t.types[&m].class.clone());
                    }
                }
            }
            Rank::ALL
                .into_iter()
                .filter(|r| seen.iter().any(|c| c.starts_with(r.title())))
                .collect()
        };
        assert!(!ranks(3).contains(&Rank::Sleepless));
        assert!(ranks(3).contains(&Rank::Emergent));
        assert!(!ranks(5).contains(&Rank::Emergent));
        assert!(ranks(5).contains(&Rank::Sleepless));
        assert_eq!(ranks(2), ranks(3), "below 3 reads as 3");
        assert_eq!(ranks(9), ranks(5), "above 5 reads as 5");
    }

    #[test]
    fn a_missing_bucket_falls_back_to_the_nearest() {
        let only = vec![row(990_800, Hull::Cruiser, Rank::Awakened, Needs::TURRET)];
        let t = with_sleepers(npc_data::Table::fixture(), &only, BTreeSet::new());
        let got = squads(&dungeon(5, 4), &t, 3, &[0]);
        assert!(
            got.iter()
                .all(|g| !g.squad.members.is_empty()
                    && g.squad.members.iter().all(|m| *m == 990_800))
        );
    }

    #[test]
    fn the_salvage_overlay_gives_sleeper_wrecks_ancient_salvage_by_size() {
        let ancient = [990_901, 990_902, 990_903];
        let t = overlay_of(&rows(), &ancient);
        for hull in Hull::ALL {
            let wreck = 990_700 + hull.size() as i64;
            assert_eq!(t.class_of(wreck), (hull.size(), RACE));
            let y = t.yields_for(hull.size(), RACE);
            let got: Vec<i64> = y.iter().map(|r| r.type_id).collect();
            assert_eq!(got, ancient, "{hull:?}");
            assert!(y.iter().all(|r| r.qty == SALVAGE_QTY));
        }
        assert!(
            overlay_of(&[], &ancient).yields_for(1, RACE).is_empty(),
            "no Sleeper wreck, no yield"
        );
    }

    /// The real bundle, with **no id named**: every J-space dungeon the
    /// sites file lists exists and is faction 500021; every class has a
    /// candidate for every wormhole row; every spawn row of a listed
    /// dungeon gets Sleeper ships; the Sleeper wrecks yield salvage.
    /// `cargo test -p eve-server --lib wormhole_sites::tests::real -- --ignored
    /// --nocapture` after copying `out/content` into the build copy.
    #[test]
    #[ignore = "needs out/content, which is never committed"]
    fn real_bundle_gives_every_class_its_sites_and_every_rank_its_ships() {
        use crate::celestial::Universe;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/content");
        let loaded = crate::content::load(Some(&dir), &crate::authored::Dir::compiled_only())
            .expect("the bundle loads");
        let c = &loaded.content;
        let (sites, dungeons) = (c.get::<Sites>(), c.get::<crate::dungeon_data::Table>());
        let (npc, universe) = (c.get::<npc_data::Table>(), c.get::<Universe>());
        let pop = c.get::<crate::population::Table>();
        assert!(sites.len() >= 60, "{} sites", sites.len());
        for id in sites.dungeons() {
            let d = dungeons
                .dungeons
                .get(&id)
                .unwrap_or_else(|| panic!("dungeon {id}"));
            assert_eq!(d.faction, Some(FACTION), "dungeon {id}");
        }
        let prep =
            crate::population::Prepared::new(pop, universe, dungeons, &|r| npc.region_pirate(r))
                .with_wormhole(sites, universe, dungeons);
        for class in CLASSES {
            let system = universe
                .systems()
                .find(|s| {
                    s.band() == crate::celestial::Band::Wormhole
                        && universe.wormhole_class(s.id) == Some(class)
                })
                .unwrap_or_else(|| panic!("no C{class} system"))
                .id;
            for (i, row) in prep.rows().iter().enumerate() {
                if row.band != crate::celestial::Band::Wormhole {
                    continue;
                }
                let got = prep.draw(1, system, i, 0, 0);
                assert!(got.is_some(), "C{class} row {} draws nothing", row.key);
            }
        }
        // Squads: every listed dungeon with spawns is guarded by Sleepers.
        let mut guarded = 0;
        let mut ships = 0;
        for id in sites.dungeons() {
            let d = &dungeons.dungeons[&id];
            if d.spawns.is_empty() {
                continue;
            }
            let sq = crate::npc_gen::dungeon_squads(d, npc, 1, &[0]);
            assert_eq!(sq.len(), d.spawns.len(), "dungeon {id}");
            assert!(
                sq.iter()
                    .all(|g| !g.squad.members.is_empty() && g.squad.faction_id == Some(FACTION))
            );
            guarded += 1;
            ships += sq.iter().map(|g| g.squad.members.len()).sum::<usize>();
        }
        crate::outln!(
            "[y-wh] {guarded} guarded J-space dungeons, {ships} Sleeper ships in one draw each"
        );
        for rank in Rank::ALL {
            for hull in Hull::ALL {
                let n = npc
                    .types
                    .values()
                    .filter(|r| r.faction_id == FACTION && r.class == class_name(rank, hull))
                    .count();
                assert!(n > 0, "{rank:?} {hull:?}");
                crate::outln!("[y-wh] {} types: {n}", class_name(rank, hull));
            }
        }
        let t = c.get::<salvage::Table>();
        let wreck = npc.factions[&FACTION]
            .wrecks
            .values()
            .next()
            .copied()
            .expect("a wreck");
        let (size, race) = t.class_of(wreck);
        assert_eq!(race, RACE);
        assert!(
            t.yields_for(size, race).len() >= 10,
            "Ancient Salvage types"
        );
    }

    /// **The model** (Y-WH, lane note "What a day yields"): what the J-space
    /// content gives, from the real bundle and the server's own rules, and
    /// what a few bots could take of it. Prints, never asserts a number.
    /// `cargo test -p eve-server --lib wormhole_sites::tests::model --
    /// --ignored --nocapture` with `out/content` copied in.
    #[test]
    #[ignore = "needs out/content, which is never committed"]
    fn model_what_a_day_of_a_few_bots_yields() {
        use crate::hacking_loot;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/content");
        let loaded = crate::content::load(Some(&dir), &crate::authored::Dir::compiled_only())
            .expect("the bundle loads");
        let c = &loaded.content;
        let (sites, dungeons) = (c.get::<Sites>(), c.get::<crate::dungeon_data::Table>());
        let npc = c.get::<npc_data::Table>();
        let td = crate::typedata::installed_in(c);
        let volume = |t: i64| td.table().get(t).map_or(1.0, |r| r.volume);
        // ---- Fullerite per reservoir, and per class
        let mut per_class: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
        for (id, row) in &sites.rows {
            if row.family != Family::Gas {
                continue;
            }
            let d = &dungeons.dungeons[id];
            let gas = crate::site::gas_types(d);
            let (mut units, mut m3) = (0.0, 0.0);
            for o in d.objects.iter().filter(|o| gas.contains(&o.type_id)) {
                // The authored value is the cloud's quantity (client
                // `dungeonHelper.py`), as `site::layout` reads it.
                let q = o.radius.unwrap_or(0.0).round().max(1.0);
                units += q;
                m3 += q * volume(o.type_id);
            }
            crate::outln!(
                "[model] reservoir {id}: {units:.0} units, {m3:.0} m3 (C{}-C{})",
                row.class_lo,
                row.class_hi
            );
            for class in row.class_lo..=row.class_hi {
                per_class.entry(class).or_default().push(m3);
            }
        }
        for (class, v) in &per_class {
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            crate::outln!(
                "[model] C{class}: {} reservoir kinds, mean {mean:.0} m3 each",
                v.len()
            );
        }
        // ---- the scoop, from its own attributes
        let amount = crate::typedata::attribute_in(c, 25_266, 77).unwrap_or(10.0);
        let cycle_s = crate::typedata::attribute_in(c, 25_266, 73).unwrap_or(30_000.0) / 1000.0;
        let m3_per_h = amount / cycle_s * 3600.0;
        crate::outln!(
            "[model] a Gas Cloud Scoop I cuts {amount} m3 per {cycle_s} s = {m3_per_h:.0} m3/h; a Venture's mining hold takes {} m3",
            crate::mining_hold::capacity(c, 32_880)
        );
        // A dive's day, from the bot's own constants (huffer::MAX_SITES 3,
        // wh_dive::SITE_BUDGET) and ours [A]: 300 s to scan and warp to a
        // site, 420 s each way through a hole, 300 s home and dock.
        let (sites_per_trip, budget_s) = (3.0, (ARRIVAL_DELAY_S - 120) as f64);
        let trip_s = 2.0 * 420.0 + 300.0 + sites_per_trip * (300.0 + budget_s);
        let m3_trip = sites_per_trip * budget_s / 3600.0 * m3_per_h;
        let trips = 86_400.0 / trip_s;
        crate::outln!(
            "[model] a dive: {trip_s:.0} s, {m3_trip:.0} m3 a trip, {trips:.1} trips a day = {:.0} m3 a bot a day; a Tengu kit is 23778 m3 (7994 units)",
            m3_trip * trips
        );
        // ---- ancient cans
        let table = c.get::<hacking_loot::Table>();
        let mut cans: BTreeMap<i64, usize> = BTreeMap::new();
        for id in sites.dungeons() {
            for o in &dungeons.dungeons[&id].objects {
                if table.kind_of(o.type_id) == Some(hacking_loot::Kind::Ancient) {
                    *cans.entry(id).or_default() += 1;
                }
            }
        }
        let salvage_types: BTreeSet<i64> = table
            .entries
            .iter()
            .filter(|e| e.class == "ancient_salvage")
            .map(|e| e.type_id)
            .collect();
        let relics: BTreeSet<i64> = table
            .entries
            .iter()
            .filter(|e| e.class == "t3_relic")
            .map(|e| e.type_id)
            .collect();
        let cores: BTreeSet<i64> = table
            .entries
            .iter()
            .filter(|e| e.class == "t3_datacore")
            .map(|e| e.type_id)
            .collect();
        crate::outln!(
            "[model] {} J-space dungeons hold ancient cans: {} cans",
            cans.len(),
            cans.values().sum::<usize>()
        );
        let (mut s_tot, mut r_tot, mut c_tot, mut n) = (0.0, 0.0, 0.0, 0.0);
        for (dungeon, count) in &cans {
            let d = &dungeons.dungeons[dungeon];
            for o in d
                .objects
                .iter()
                .filter(|o| table.kind_of(o.type_id) == Some(hacking_loot::Kind::Ancient))
            {
                let attr = |a| crate::typedata::attribute_in(c, o.type_id, a);
                let tier = attr(crate::hacking::ATTR_TIER_DIFFICULTY)
                    .map_or(1, |v| v.round().clamp(1.0, 4.0) as u8);
                let eq = (
                    attr(hacking_loot::ATTR_EQUIPMENT_MIN).map_or(0, |v| v.round() as i64),
                    attr(hacking_loot::ATTR_EQUIPMENT_MAX).map_or(0, |v| v.round() as i64),
                );
                let can = hacking_loot::Can {
                    kind: hacking_loot::Kind::Ancient,
                    tier,
                    equipment: eq,
                    band: Some(crate::celestial::Band::Wormhole),
                };
                for seed in 0..400u64 {
                    let mut st = stream(seed, Kind::Spawn, &[*dungeon as u64, o.id as u64]);
                    let got = hacking_loot::roll(table, &can, &mut st, |_| true);
                    for g in got {
                        let q = g.quantity as f64 / 400.0;
                        if salvage_types.contains(&g.type_id) {
                            s_tot += q;
                        }
                        if relics.contains(&g.type_id) {
                            r_tot += q;
                        }
                        if cores.contains(&g.type_id) {
                            c_tot += q;
                        }
                    }
                }
                n += 1.0;
            }
            let _ = count;
        }
        if n > 0.0 {
            crate::outln!(
                "[model] per ancient can in J-space: {:.1} Ancient Salvage, {:.2} relics, {:.2} Subsystem cores ({n} cans sampled)",
                s_tot / n,
                r_tot / n,
                c_tot / n
            );
        }
        // ---- Sleeper wrecks per combat anomaly
        let mut ships_by_size = [0.0f64; 3];
        let mut anomalies = 0.0;
        for (id, row) in &sites.rows {
            if row.family != Family::Combat {
                continue;
            }
            anomalies += 1.0;
            for seed in 0..40u64 {
                for sq in crate::npc_gen::dungeon_squads(&dungeons.dungeons[id], npc, seed, &[0]) {
                    for m in sq.squad.members {
                        let class = &npc.types[&m].class;
                        let size = Hull::ALL
                            .iter()
                            .find(|h| class.ends_with(h.name()))
                            .map_or(2, |h| h.size());
                        ships_by_size[usize::from(size) - 1] += 1.0 / 40.0;
                    }
                }
            }
        }
        let avg_draws = |size: u8| {
            let (lo, hi) = salvage::DRAWS[usize::from(size) - 1];
            (lo + hi) as f64 / 2.0
        };
        let qty = (SALVAGE_QTY.0 + SALVAGE_QTY.1) as f64 / 2.0;
        let p = salvage::chance(5.0, 3, 1.0);
        let salvage_per_day: f64 = (1..=3u8)
            .map(|s| ships_by_size[usize::from(s) - 1] * avg_draws(s) * qty * p)
            .sum::<f64>()
            / anomalies;
        crate::outln!(
            "[model] per Sleeper anomaly: {:.1} / {:.1} / {:.1} frigate/cruiser/battleship wrecks = {:.0} Ancient Salvage at a {p:.2} salvage chance (Salvager I, Salvaging III)",
            ships_by_size[0] / anomalies,
            ships_by_size[1] / anomalies,
            ships_by_size[2] / anomalies,
            salvage_per_day
        );
    }
}
