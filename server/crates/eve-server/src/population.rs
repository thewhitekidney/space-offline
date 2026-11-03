//! The site population: which anomalies and signatures each system holds,
//! and how they cycle.
//!
//! W13 packet CP21 (`docs/content/plan.md` §3.3, §3.4, §4.2; spec
//! `docs/content/C3-population.md` §4 and §6; ruling 4). The authored table
//! is `content/population.tsv` ([`AUTHORED`]); the generator is a pure
//! function of `(world seed, system, row key, k, generation)` plus the loaded
//! tables (plan §3.1). Nothing generated is stored: a slot's only persisted
//! state is its `site` delta, `system_id archetype key k -> generation state
//! at dungeon_id`, written when a site completes or expires.
//!
//! Read [`Prepared::slots`], [`Prepared::draw`] and [`Prepared::sites`]
//! first; [`tick`], [`complete`] and [`on_rollover`] are the Registry side.
//!
//! **What a cold system costs:** nothing. A system with no delta is never
//! visited by the rollover; its sites, all generation 0 born at the world's
//! birth, expire at the first rollover after it goes warm [A] (falsified by
//! a pilot who notices the first day's sites of a fresh system are older).

use crate::celestial::{Band, Catalogue, Kind as CelestialKind, Universe};
use crate::content::{Content, Sources, TableSpec};
use crate::damage::BlueTime;
use crate::delta::Rows;
use crate::dungeon_data::{self, Difficulties, Entry, FACTION_ARCHETYPES, OreBand};
use crate::seed::{self, Kind, Stream};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// The authored file, relative to `--authored`.
pub const FILE: &str = "population.tsv";

/// Its header, frozen in `docs/content/schema.md` §4.
pub const HEADER: &str = "key\tband\tarchetype\tentry\tcount\tdifficulties\trespawn_s\tttl_s\twhy";

/// The shipped copy, so the binary always has the defaults (plan §1.4).
const SHIPPED: &str = include_str!("../../../../content/population.tsv");

/// The registration row in `content_tables::TABLES` (coordinator-only).
pub const AUTHORED: TableSpec = TableSpec {
    name: "population",
    load: boxed_load,
    fixture: boxed_fixture,
};

/// The registration row in `world_ext::SLOTS` (coordinator-only). Its
/// `make` reads the content tables only, never deltas (CA11).
pub const SLOT: SlotSpec = SlotSpec {
    name: "population",
    make,
};

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "site";

/// The wormhole kind, dropped with [`KIND`] when [`GEN_VERSION`] moves
/// (plan §4.2: one place drops every generated delta).
pub const WH_KIND: &str = "wh";

/// The generators' version (plan §4.2). **Bump it in the commit that moves
/// the digest pin** (`tests::the_fixture_world_digest_is_pinned`): every
/// `site` and `wh` delta written under another version is then dropped.
pub const GEN_VERSION: u32 = 2;

/// `dunArchetypeCombatSites` [V] (`appConst`, C3 §1): the archetype the C7
/// floor applies to.
pub const COMBAT_ARCHETYPE: i64 = 24;

/// The client's archetype to scan-strength attribute table [V]
/// (`probescanning/explorationSites.pyc:16-29`, C3 §4.4). Ice (28) has no
/// row there, and gets 0.
pub const SCAN_ATTRIBUTES: [(i64, i64); 5] =
    [(24, 1136), (27, 211), (30, 209), (44, 210), (45, 208)];

/// The site id band (plan §3.4) [V band location]: `SITE_BASE +
/// ((sys_rank * 64 + slot) * 1024 + generation % 1024)`.
pub const SITE_BASE: i64 = 8_100_000_000;
/// One past the band.
pub const SITE_END: i64 = 8_700_000_000;
/// Slots per system, the id formula's 64.
pub const SLOTS_PER_SYSTEM: usize = 64;
/// Generations before an id repeats [A] (plan §3.4: falsified by a client
/// that caches a site object longer than 1,024 respawns).
pub const GENERATIONS: u32 = 1024;
/// Ranks the band holds: `MAX_RANK * 64 * 1024 <= SITE_END - SITE_BASE`.
pub const MAX_RANK: usize = 9_155;

/// [A] C7 §4.4: a pilot within this of a site's anchor is "inside" it, and
/// the rollover keeps it. Falsified by a site expiring under a pilot.
pub const PILOT_RANGE: f64 = 100_000.0;
/// [A] C3 §4.3: an anchor is a planet or moon plus U(lo, hi) AU. Falsified
/// by a site on a gate or station grid.
pub const ANCHOR_AU: (f64, f64) = (0.05, 0.25);
/// [A] C3 §4.3: an anchor keeps this far from every body's surface, metres.
/// Falsified by a warp that lands inside a body.
pub const CLEARANCE: f64 = 1_000_000.0;
/// [A] C3 §4.3: anchor retries before the last try is kept.
pub const ANCHOR_TRIES: u32 = 16;
/// [A] C3 §9: a live system's slots are re-read at this cadence. Falsified
/// by a refill that shows up late enough for a pilot to notice.
pub const SYNC_EVERY: BlueTime = 60 * BLUE_PER_SECOND;
/// [A] a signature's radius and scan strength (C3 unknown 7). Falsified by a
/// probe result that resolves far faster or slower than TQ's.
pub const SIGNATURE_RADIUS: f64 = 100.0;
/// See [`SIGNATURE_RADIUS`].
pub const SIGNATURE_STRENGTH: f64 = 40.0;

const BLUE_PER_SECOND: BlueTime = 10_000_000;

/// Tags the presence draw's stream.
const TAG_PRESENCE: u64 = 1;

fn boxed_load(src: &Sources) -> Result<crate::content::Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    let (rows, _) = file.parse_rows_logged(parse_row);
    Ok(Box::new(
        Table::new(rows).map_err(|e| format!("{FILE}: {e}"))?,
    ))
}

fn boxed_fixture() -> crate::content::Boxed {
    Box::new(Table::fixture())
}

/// A row's `difficulties`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diffs {
    In(Vec<i64>),
    /// `*`: the dungeon's ore band is the system's band (C3 §7.2 R4).
    Star,
}

/// One `population.tsv` row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub key: String,
    pub band: Band,
    pub archetypes: Vec<i64>,
    pub entry: Entry,
    pub count: f64,
    pub difficulties: Diffs,
    pub respawn_s: i64,
    pub ttl_s: i64,
}

impl Row {
    /// The `archetype` field of the delta key: the row's list as written
    /// (`schema.md` list encoding), so `44,45` is one slot key.
    pub fn archetype_field(&self) -> String {
        let v: Vec<String> = self.archetypes.iter().map(i64::to_string).collect();
        v.join(",")
    }

    fn has_factions(&self) -> bool {
        self.archetypes
            .iter()
            .any(|a| FACTION_ARCHETYPES.contains(a))
    }

    /// W17 CP53b: every archetype is a factional-warfare one, so the row
    /// lives only in warzone systems ([`Prepared::with_warzones`]).
    fn is_fw(&self) -> bool {
        self.archetypes
            .iter()
            .all(|a| crate::facwar::FW_ARCHETYPES.contains(a))
    }

    fn is_combat_anomaly(&self) -> bool {
        self.entry == Entry::Anomaly && self.archetypes.contains(&COMBAT_ARCHETYPE)
    }

    /// How many `k` the row has in `system`: the whole part of `count`, plus
    /// one with the fraction's probability, seeded per system (C3 §4.1).
    pub fn live_count(&self, seed: u64, system: i64) -> u32 {
        let whole = self.count.floor();
        let frac = self.count - whole;
        let mut r = seed::stream(
            seed,
            Kind::Site,
            &[system as u64, key_hash(&self.key), TAG_PRESENCE],
        );
        whole as u32 + u32::from(frac > 0.0 && r.chance(frac))
    }
}

fn parse_row(r: &crate::authored::Row) -> Result<Row, String> {
    let key = r.field(0).to_owned();
    let ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_';
    if key.is_empty() || !key.bytes().all(ok) {
        return Err(format!("key {key:?} is not [a-z0-9_]+"));
    }
    let band = match r.field(1) {
        "high" => Band::High,
        "low" => Band::Low,
        "null" => Band::Null,
        "wormhole" => Band::Wormhole,
        v => return Err(format!("band {v:?} is not high, low, null or wormhole")),
    };
    let archetypes = r.list_i64(2)?;
    if archetypes.is_empty() {
        return Err("no archetype".into());
    }
    let entry = match r.field(3) {
        "anomaly" => Entry::Anomaly,
        "signature" => Entry::Signature,
        v => return Err(format!("entry {v:?} is not anomaly or signature")),
    };
    let count = r.f64(4)?;
    if !(0.0..SLOTS_PER_SYSTEM as f64).contains(&count) {
        return Err(format!("count {count} is outside [0, {SLOTS_PER_SYSTEM})"));
    }
    let difficulties = match r.field(5) {
        "*" => Diffs::Star,
        _ => Diffs::In(r.list_i64(5)?),
    };
    let (respawn_s, ttl_s) = (r.i64(6)?, r.i64(7)?);
    if respawn_s <= 0 || ttl_s <= 0 {
        return Err("respawn_s and ttl_s must be positive".into());
    }
    Ok(Row {
        key,
        band,
        archetypes,
        entry,
        count,
        difficulties,
        respawn_s,
        ttl_s,
    })
}

/// Every row, in file order (the slot order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    rows: Vec<Row>,
}

impl Table {
    /// # Errors
    ///
    /// Two rows with one key: the key is the delta key (ruling 4).
    pub fn new(rows: Vec<Row>) -> Result<Table, String> {
        let mut seen = BTreeSet::new();
        for r in &rows {
            if !seen.insert(r.key.as_str()) {
                return Err(format!("key {:?} is listed twice", r.key));
            }
        }
        Ok(Table { rows })
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Invented rows over invented archetypes (96-98, as `dungeon_data`'s
    /// fixture) and the named rule constants; never a shipped value.
    pub fn fixture() -> Table {
        let (c, e1, e2) = (
            COMBAT_ARCHETYPE,
            FACTION_ARCHETYPES[1],
            FACTION_ARCHETYPES[2],
        );
        let text = format!(
            "{HEADER}\n\
             fx_combat_high\thigh\t{c}\tanomaly\t2\t1,2\t600\t172800\tx\n\
             fx_sig_high\thigh\t{c}\tsignature\t1\t2,3\t600\t172800\tx\n\
             fx_ore_high\thigh\t97\tanomaly\t1\t*\t600\t172800\tx\n\
             fx_empty_low\tlow\t98\tanomaly\t1\t5\t600\t172800\tx\n\
             fx_combat_null\tnull\t{c}\tanomaly\t3\t2,3\t600\t172800\tx\n\
             fx_explo_null\tnull\t{e1},{e2}\tsignature\t1.5\t2\t600\t172800\tx\n\
             fx_ore_null\tnull\t97\tanomaly\t1\t*\t600\t172800\tx\n"
        );
        let (raw, _) = crate::authored::parse(&text, HEADER).expect("the fixture header");
        let rows = raw.iter().map(|r| parse_row(r).expect("a fixture row"));
        Table::new(rows.collect()).expect("the fixture is well-formed")
    }
}

/// FNV-1a over the key's bytes: the key's part of every seed.
fn key_hash(key: &str) -> u64 {
    key.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The id band's formula (plan §3.4); `None` above [`MAX_RANK`] or
/// [`SLOTS_PER_SYSTEM`].
pub fn site_id(sys_rank: usize, slot: usize, generation: u32) -> Option<i64> {
    (sys_rank < MAX_RANK && slot < SLOTS_PER_SYSTEM).then(|| {
        let cell = (sys_rank * SLOTS_PER_SYSTEM + slot) as i64;
        SITE_BASE + cell * i64::from(GENERATIONS) + i64::from(generation % GENERATIONS)
    })
}

/// The `minimum combat anomalies` of the C7 §6.2 floor, `1 + ceil(pilots /
/// 3)` [A]. Falsified by bots in a system outnumbering its sites.
pub fn floor_for(pilots: usize) -> u32 {
    1 + pilots.div_ceil(3) as u32
}

fn ore_band(b: Band) -> OreBand {
    match b {
        Band::High => OreBand::High,
        Band::Low => OreBand::Low,
        _ => OreBand::Null,
    }
}

/// One slot of a system: a row, its `k`, and its index in the system's list
/// (the id's `slot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotRef {
    pub row: usize,
    pub k: u32,
    pub index: usize,
}

/// A slot's lifecycle state (C3 §6). Expiry is not a state: the rollover
/// refills an expired slot at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Live,
    /// Completed at `at`; refills at `at + respawn_s`.
    Done,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            State::Live => "live",
            State::Done => "done",
        }
    }
}

/// One site, the record C3 §4.4 (shape S1) describes. Its dungeon's name,
/// difficulty and faction are read from `dungeon_data` by whoever emits it.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub system: i64,
    /// Index into [`Table::rows`].
    pub row: usize,
    pub k: u32,
    pub slot: usize,
    pub generation: u32,
    /// The ball's item id; also the instance id (C3 §4.4).
    pub site_id: i64,
    pub dungeon_id: i64,
    pub archetype: i64,
    pub entry: Entry,
    pub anchor: [f64; 3],
    /// `ABC-123`, unique in the system.
    pub target_id: String,
    pub state: State,
    /// When the state began (a live site's birth).
    pub at: BlueTime,
}

/// What a resolution found: the sites, and the delta keys that match no
/// slot or no draw (to be dropped by the caller).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolved {
    pub sites: Vec<Site>,
    pub stale: Vec<Vec<String>>,
}

/// The world a resolution reads: the seed, the birth, the system's bodies
/// and the `site` deltas.
pub struct Inputs<'a> {
    pub seed: u64,
    pub birth: BlueTime,
    pub celestials: &'a Catalogue,
    pub deltas: &'a Rows,
    /// Lane clock: a completed slot refills `clock.span(respawn_s)` after
    /// its completion. Derived at read time from the stored completion, so
    /// a restart at another scale re-derives every pending refill.
    pub clock: crate::game_clock::GameClock,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Sys {
    rank: usize,
    band: Band,
    pirate: Option<i64>,
    /// Y-WH: a J-space system's class, C1 to C6 (`celestial::Universe::
    /// wormhole_class`), set by [`Prepared::with_wormhole`]; `None` for
    /// k-space and for the classes with no content.
    class: Option<i64>,
}

/// The table joined with the universe and the catalogue once: every
/// system's rank, band and rat faction, and every candidate list a system
/// can ask for.
#[derive(Debug, Clone, Default)]
pub struct Prepared {
    rows: Vec<Row>,
    systems: BTreeMap<i64, Sys>,
    /// `(row, faction)` to the dungeon ids, ascending. The faction is the
    /// system's pirate for a row with faction archetypes, else `None`.
    cands: BTreeMap<(usize, Option<i64>), Vec<i64>>,
    archetype: BTreeMap<i64, i64>,
    /// W17 CP53b: each warzone system's two militias, side A first. An FW
    /// row lives only in these systems and draws only their militias'
    /// complexes, keyed `(row, Some(side A))` in `cands`.
    militias: BTreeMap<i64, [i64; 2]>,
}

impl Prepared {
    /// `pirate` maps a region to its rat faction (`npc_data::region_pirate`,
    /// CP20). A row with faction archetypes in a system with no pirate draws
    /// none of them [A] (C3 §8.1 step 4: no rats there). Every candidate
    /// list that comes out empty is logged once here.
    pub fn new(
        table: &Table,
        universe: &Universe,
        dungeons: &dungeon_data::Table,
        pirate: &dyn Fn(i64) -> Option<i64>,
    ) -> Prepared {
        let mut p = Prepared {
            rows: table.rows.clone(),
            ..Prepared::default()
        };
        for (rank, s) in universe.systems().enumerate() {
            let sys = Sys {
                rank,
                band: s.band(),
                pirate: pirate(s.region_id),
                class: None,
            };
            p.systems.insert(s.id, sys);
            for (i, row) in table
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.band == sys.band && r.band != Band::Wormhole)
            {
                let faction = sys.pirate.filter(|_| row.has_factions());
                p.cands
                    .entry((i, faction))
                    .or_insert_with(|| candidates(row, sys.band, faction, dungeons));
            }
        }
        for ((i, faction), list) in &p.cands {
            if list.is_empty() {
                crate::outln!(
                    "[population] row {} faction {faction:?}: no dungeon fits; its slots stay empty",
                    p.rows[*i].key
                );
            }
            for d in list {
                p.archetype.insert(*d, dungeons.dungeons[d].archetype);
            }
        }
        p
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// W17 CP53b: keep the FW rows (`Row::is_fw`) to the warzone systems
    /// `militias` names (`crate::facwar::militias_by_system`), each drawing
    /// only the complexes its two militias own (R-FW §6 [I]: a complex's
    /// `factionID` is its owner).
    pub fn with_warzones(
        mut self,
        militias: BTreeMap<i64, [i64; 2]>,
        dungeons: &dungeon_data::Table,
    ) -> Prepared {
        for (system, pair) in &militias {
            let Some(sys) = self.systems.get(system).copied() else {
                continue;
            };
            for (i, row) in self.rows.iter().enumerate() {
                if !row.is_fw() || row.band != sys.band {
                    continue;
                }
                let all = self.cands.get(&(i, None)).cloned().unwrap_or_default();
                self.cands.entry((i, Some(pair[0]))).or_insert_with(|| {
                    all.into_iter()
                        .filter(|d| {
                            let owner = dungeons.dungeons.get(d).and_then(|x| x.faction);
                            owner.is_some_and(|f| pair.contains(&f))
                        })
                        .collect()
                });
            }
        }
        self.militias = militias;
        self
    }

    /// **Y-WH: wormhole space's candidates.** A row of band `wormhole`
    /// draws, in a J-space system of class `c` (C1-C6), only the dungeons
    /// `sites` lists for `c` whose archetype and entry the row asks for
    /// (`Difficulties` is ignored: the class is the scope). The faction and
    /// gas exclusions of `dungeon_data::Exclusions::C3` do not apply here,
    /// which is how the 500021 sites and the Fullerite reservoirs come in
    /// **for wormhole space only**. Each list is keyed `(row, Some(-c))`
    /// (no faction id is negative); a list that comes out empty is logged
    /// once.
    pub fn with_wormhole(
        mut self,
        sites: &crate::wormhole_sites::Sites,
        universe: &Universe,
        dungeons: &dungeon_data::Table,
    ) -> Prepared {
        let classes: Vec<(i64, i64)> = universe
            .systems()
            .filter(|s| s.band() == Band::Wormhole)
            .filter_map(|s| {
                let c = universe.wormhole_class(s.id)?;
                sites.serves(c).then_some((s.id, c))
            })
            .collect();
        for (system, class) in &classes {
            if let Some(sys) = self.systems.get_mut(system) {
                sys.class = Some(*class);
            }
        }
        let wanted: BTreeSet<i64> = classes.iter().map(|(_, c)| *c).collect();
        for (i, row) in self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.band == Band::Wormhole)
        {
            for class in &wanted {
                let list: Vec<i64> = dungeons
                    .dungeons
                    .values()
                    .filter(|d| {
                        row.archetypes.contains(&d.archetype)
                            && d.entry_type == Some(row.entry.type_id())
                            && sites.admits(d.id, *class)
                    })
                    .map(|d| d.id)
                    .collect();
                if list.is_empty() {
                    crate::outln!(
                        "[population] row {} class C{class}: no dungeon fits; its slots stay empty",
                        row.key
                    );
                }
                for d in &list {
                    self.archetype.insert(*d, dungeons.dungeons[d].archetype);
                }
                self.cands.insert((i, Some(-*class)), list);
            }
        }
        self
    }

    /// The system's rank among the universe's ids, ascending (plan §3.4).
    pub fn sys_rank(&self, system: i64) -> Option<usize> {
        self.systems.get(&system).map(|s| s.rank)
    }

    /// The system's slots: each band row's own `k` values in file order,
    /// then the floor's extra `k` on the first combat-anomaly row, last, so
    /// pilots coming and going never renumber another slot. At most
    /// [`SLOTS_PER_SYSTEM`].
    pub fn slots(&self, seed: u64, system: i64, pilots: usize) -> Vec<SlotRef> {
        let Some(sys) = self.systems.get(&system) else {
            return Vec::new();
        };
        let (mut own, mut floor, mut floored) = (Vec::new(), Vec::new(), false);
        for (i, row) in self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.band == sys.band)
            .filter(|(_, r)| !r.is_fw() || self.militias.contains_key(&system))
        {
            let n = row.live_count(seed, system);
            own.extend((0..n).map(|k| (i, k)));
            if !floored && row.is_combat_anomaly() {
                floored = true;
                floor.extend((n..floor_for(pilots)).map(|k| (i, k)));
            }
        }
        own.extend(floor);
        own.truncate(SLOTS_PER_SYSTEM);
        own.into_iter()
            .enumerate()
            .map(|(index, (row, k))| SlotRef { row, k, index })
            .collect()
    }

    /// The slot's stream for one generation: the pick, then the anchor.
    fn stream(&self, seed: u64, system: i64, row: usize, k: u32, generation: u32) -> Stream {
        let parts = [
            system as u64,
            key_hash(&self.rows[row].key),
            u64::from(k),
            u64::from(generation),
        ];
        seed::stream(seed, Kind::Site, &parts)
    }

    /// C3 §4.2: the dungeon for `(system, row, k, generation)`, `None` for
    /// an empty candidate set (or a system the universe lacks).
    pub fn draw(&self, seed: u64, system: i64, row: usize, k: u32, generation: u32) -> Option<i64> {
        let sys = self.systems.get(&system)?;
        let faction = if self.rows[row].is_fw() {
            Some(self.militias.get(&system)?[0])
        } else if self.rows[row].band == Band::Wormhole {
            Some(-sys.class?)
        } else {
            sys.pirate.filter(|_| self.rows[row].has_factions())
        };
        let list = self.cands.get(&(row, faction))?;
        if list.is_empty() {
            return None;
        }
        let pick = self.stream(seed, system, row, k, generation).next() % list.len() as u64;
        Some(list[pick as usize])
    }

    /// The delta key of a slot (ruling 4): `system_id archetype key k`.
    pub fn delta_key(&self, system: i64, row: usize, k: u32) -> Vec<String> {
        let r = &self.rows[row];
        vec![
            system.to_string(),
            r.archetype_field(),
            r.key.clone(),
            k.to_string(),
        ]
    }

    /// Every slot of `system` at `now`, with its delta applied, and the
    /// system's delta keys that no longer fit.
    pub fn sites(&self, w: &Inputs, system: i64, pilots: usize, now: BlueTime) -> Resolved {
        let mut out = Resolved::default();
        let Some(sys) = self.systems.get(&system) else {
            out.stale = system_keys(w.deltas, system).cloned().collect();
            return out;
        };
        let known: BTreeSet<(String, String)> = self
            .rows
            .iter()
            .filter(|r| r.band == sys.band)
            .map(|r| (r.archetype_field(), r.key.clone()))
            .collect();
        for key in system_keys(w.deltas, system) {
            let fits = key.len() == 4
                && known.contains(&(key[1].clone(), key[2].clone()))
                && key[3].parse::<u32>().is_ok();
            if !fits {
                out.stale.push(key.clone());
            }
        }
        let mut used = BTreeSet::new();
        for slot in self.slots(w.seed, system, pilots) {
            let row = &self.rows[slot.row];
            let key = self.delta_key(system, slot.row, slot.k);
            let (mut generation, mut state, mut at) = (0, State::Live, w.birth);
            if let Some(v) = w.deltas.get(&key) {
                match parse_value(v) {
                    Some((g, s, a, d)) if d == self.draw(w.seed, system, slot.row, slot.k, g) => {
                        (generation, state, at) = (g, s, a);
                    }
                    _ => out.stale.push(key),
                }
            }
            let respawn = w.clock.span(row.respawn_s * BLUE_PER_SECOND);
            if state == State::Done && now >= at + respawn {
                (generation, state, at) = (generation + 1, State::Live, at + respawn);
            }
            let Some(dungeon_id) = self.draw(w.seed, system, slot.row, slot.k, generation) else {
                continue;
            };
            let Some(site_id) = site_id(sys.rank, slot.index, generation) else {
                continue;
            };
            let mut r = self.stream(w.seed, system, slot.row, slot.k, generation);
            r.next();
            let anchor = anchor(&mut r, w.celestials, system);
            let parts = [
                system as u64,
                key_hash(&row.key),
                u64::from(slot.k),
                u64::from(generation),
            ];
            let mut t = seed::stream(w.seed, Kind::Sig, &parts);
            let target_id = loop {
                let id = target_id(&mut t);
                if used.insert(id.clone()) {
                    break id;
                }
            };
            out.sites.push(Site {
                system,
                row: slot.row,
                k: slot.k,
                slot: slot.index,
                generation,
                site_id,
                dungeon_id,
                archetype: self.archetype.get(&dungeon_id).copied().unwrap_or(0),
                entry: row.entry,
                anchor,
                target_id,
                state,
                at,
            });
        }
        out
    }
}

/// C3 §4.2 steps 1-2 for one row in one band. With no pirate, a faction
/// archetype's dungeons are left out.
fn candidates(row: &Row, band: Band, faction: Option<i64>, d: &dungeon_data::Table) -> Vec<i64> {
    let difficulties = match &row.difficulties {
        Diffs::In(v) => Difficulties::In(v),
        Diffs::Star => Difficulties::Star(ore_band(band)),
    };
    let slot = dungeon_data::Slot {
        archetypes: &row.archetypes,
        entry: row.entry,
        difficulties,
        faction,
    };
    let mut list = dungeon_data::candidates(d, &slot, &dungeon_data::Exclusions::C3, &|_| None);
    if faction.is_none() {
        list.retain(|id| !FACTION_ARCHETYPES.contains(&d.dungeons[id].archetype));
    }
    list
}

/// The `site` delta keys of one system.
fn system_keys(rows: &Rows, system: i64) -> impl Iterator<Item = &Vec<String>> {
    let s = system.to_string();
    rows.range(vec![s.clone()]..)
        .map(|(k, _)| k)
        .take_while(move |k| k.first() == Some(&s))
}

/// `generation state at dungeon_id`; an empty `dungeon_id` is an empty slot.
fn parse_value(v: &[String]) -> Option<(u32, State, BlueTime, Option<i64>)> {
    let [g, s, a, d] = v else { return None };
    let state = match s.as_str() {
        "live" => State::Live,
        "done" => State::Done,
        _ => return None,
    };
    let dungeon = if d.is_empty() {
        None
    } else {
        Some(d.parse().ok()?)
    };
    Some((g.parse().ok()?, state, a.parse().ok()?, dungeon))
}

/// C3 §4.3 R2: a planet or moon of the system, chosen by the stream, plus a
/// random unit vector times U([`ANCHOR_AU`]); retried while within
/// [`CLEARANCE`] of a body's surface. A system with no planet or moon in
/// the catalogue anchors around its star at the origin. Shared with
/// `crate::wormhole` (W16 CP54a).
pub(crate) fn anchor(r: &mut Stream, cat: &Catalogue, system: i64) -> [f64; 3] {
    let all: Vec<_> = cat.served_in(system).collect();
    let bodies: Vec<_> = all
        .iter()
        .filter(|c| matches!(c.kind, CelestialKind::Planet | CelestialKind::Moon))
        .collect();
    let mut p = [0.0; 3];
    for _ in 0..ANCHOR_TRIES {
        let base = match bodies.len() {
            0 => [0.0; 3],
            n => bodies[r.range(0, n as u64) as usize].position,
        };
        let (z, phi) = (2.0 * r.unit() - 1.0, std::f64::consts::TAU * r.unit());
        let ring = (1.0 - z * z).sqrt();
        let dir = [ring * phi.cos(), ring * phi.sin(), z];
        let len = crate::scan::AU * (ANCHOR_AU.0 + (ANCHOR_AU.1 - ANCHOR_AU.0) * r.unit());
        p = std::array::from_fn(|i| base[i] + dir[i] * len);
        if all
            .iter()
            .all(|c| distance(p, c.position) - c.radius >= CLEARANCE)
        {
            break;
        }
    }
    p
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// Three letters and three digits. Shared with `crate::wormhole`.
pub(crate) fn target_id(r: &mut Stream) -> String {
    let letters: String = (0..3)
        .map(|_| char::from(b'A' + r.range(0, 26) as u8))
        .collect();
    format!("{letters}-{:03}", r.range(0, 1000))
}

/// The scan-strength attribute of an archetype ([`SCAN_ATTRIBUTES`]).
pub fn scan_attribute(archetype: i64) -> i64 {
    SCAN_ATTRIBUTES
        .iter()
        .find(|(a, _)| *a == archetype)
        .map_or(0, |(_, attr)| *attr)
}

// ---- the world slot and the Registry side ----

/// The world slot: the prepared table, and what the tracker was handed.
#[derive(Debug, Default)]
pub struct Population {
    prep: Prepared,
    /// Per live system, the site ids the tracker holds.
    shown: BTreeMap<i64, BTreeSet<i64>>,
    /// Per live system, when its slots are next re-read.
    due: BTreeMap<i64, BlueTime>,
    /// `allowedTypes` per dungeon, computed once.
    allowed: BTreeMap<i64, Vec<i64>>,
}

impl Population {
    pub fn new(prep: Prepared) -> Population {
        Population {
            prep,
            ..Population::default()
        }
    }

    pub fn prepared(&self) -> &Prepared {
        &self.prep
    }
}

fn make(c: &Content) -> Boxed {
    let npc = c.try_get::<crate::npc_data::Table>();
    let pirate = |region: i64| npc.and_then(|n| n.region_pirate(region));
    let prep = match (
        c.try_get::<Table>(),
        c.try_get::<Universe>(),
        c.try_get::<dungeon_data::Table>(),
    ) {
        (Some(t), Some(u), Some(d)) => {
            let p = Prepared::new(t, u, d, &pirate)
                .with_warzones(crate::facwar::militias_by_system(c), d);
            match c.try_get::<crate::wormhole_sites::Sites>() {
                Some(w) => p.with_wormhole(w, u, d),
                None => p,
            }
        }
        _ => Prepared::default(),
    };
    Box::new(Population::new(prep))
}

/// [`Prepared::sites`] on the registry's world, at `now`.
pub fn sites(reg: &Registry, system: i64, now: BlueTime) -> Resolved {
    let w = Inputs {
        seed: reg.world().seed,
        birth: reg.world().birth,
        celestials: reg.celestials(),
        deltas: reg.deltas().of(KIND),
        clock: crate::game_clock::of(reg),
    };
    let pilots = reg.roster().in_system(system).len();
    reg.ext::<Population>().prep.sites(&w, system, pilots, now)
}

/// Drop every `site` and `wh` delta written under another [`GEN_VERSION`]
/// (plan §4.2). A world with none is left untouched (and clean).
pub fn settle(reg: &mut Registry) {
    let d = reg.deltas();
    let n = d.of(KIND).len() + d.of(WH_KIND).len();
    if reg.world().gen_version == GEN_VERSION || n == 0 {
        return;
    }
    for kind in [KIND, WH_KIND] {
        let keys: Vec<Vec<String>> = reg.deltas().of(kind).keys().cloned().collect();
        keys.iter().for_each(|k| _ = reg.deltas_mut().del(kind, k));
    }
    stamp(reg);
    crate::outln!("[population] gen_version is now {GEN_VERSION}: dropped {n} site and wh deltas");
}

/// Record [`GEN_VERSION`] in the world: every writer of a `site` or `wh`
/// delta calls this first (CP54a too).
pub fn stamp(reg: &mut Registry) {
    if reg.world().gen_version != GEN_VERSION {
        let world = crate::delta::World {
            gen_version: GEN_VERSION,
            ..*reg.world()
        };
        reg.deltas_mut().set_world(world);
    }
}

fn put(reg: &mut Registry, key: Vec<String>, g: u32, state: State, at: BlueTime, d: Option<i64>) {
    let d = d.map_or_else(String::new, |d| d.to_string());
    let value = vec![g.to_string(), state.word().to_owned(), at.to_string(), d];
    stamp(reg);
    reg.deltas_mut().put(KIND, key, value);
}

fn drop_stale(reg: &mut Registry, system: i64, stale: &[Vec<String>]) {
    if stale.is_empty() {
        return;
    }
    crate::outln!(
        "[population] system {system}: dropped {} site deltas that match no slot or draw",
        stale.len()
    );
    let d = reg.deltas_mut();
    for k in stale {
        d.del(KIND, k);
    }
}

/// **The completion hook** (CP28): the live site `site_id` in `system` is
/// done at `now`. Its slot refills after the row's `respawn_s`. `false` for
/// a site that is not live there.
pub fn complete(reg: &mut Registry, system: i64, site_id: i64, now: BlueTime) -> bool {
    settle(reg);
    let r = sites(reg, system, now);
    let Some(s) = r
        .sites
        .iter()
        .find(|s| s.site_id == site_id && s.state == State::Live)
    else {
        return false;
    };
    let key = reg.ext::<Population>().prep.delta_key(system, s.row, s.k);
    put(reg, key, s.generation, State::Done, now, Some(s.dungeon_id));
    reg.ext_mut::<Population>()
        .due
        .insert(system, BlueTime::MIN);
    true
}

/// **The rollover step** (CP25 registers it): every live site older than
/// its row's `ttl_s` at the start of `day` expires, unless a pilot is
/// within [`PILOT_RANGE`] of it, and its slot refills at once as
/// `generation + 1`. Visits every warm or hot system and every system with
/// a `site` delta. Returns how many sites expired.
pub fn on_rollover(reg: &mut Registry, day: u32) -> usize {
    settle(reg);
    // Lane clock: the rollover's own time at a `--time-scale` above 1.
    let t = crate::game_clock::day_start(reg, day);
    let mut systems: BTreeSet<i64> = reg
        .ext::<crate::liveness::Liveness>()
        .iter()
        .map(|(s, _)| s)
        .collect();
    systems.extend(
        reg.deltas()
            .of(KIND)
            .keys()
            .filter_map(|k| k.first()?.parse::<i64>().ok()),
    );
    let mut expired = 0;
    for system in systems {
        let r = sites(reg, system, t);
        drop_stale(reg, system, &r.stale);
        let pilots: Vec<[f64; 3]> = reg
            .roster()
            .in_system(system)
            .iter()
            .map(|p| p.motion.position)
            .collect();
        for s in &r.sites {
            let seed = reg.world().seed;
            let prep = &reg.ext::<Population>().prep;
            // Lane clock: a site's lifetime at the game clock's pace.
            let ttl = crate::game_clock::span(reg, prep.rows[s.row].ttl_s * BLUE_PER_SECOND);
            let inside = pilots.iter().any(|p| distance(*p, s.anchor) <= PILOT_RANGE);
            if s.state != State::Live || t - s.at < ttl || inside {
                continue;
            }
            let key = prep.delta_key(system, s.row, s.k);
            let next = prep.draw(seed, system, s.row, s.k, s.generation + 1);
            put(reg, key, s.generation + 1, State::Live, t, next);
            expired += 1;
        }
        reg.ext_mut::<Population>()
            .due
            .insert(system, BlueTime::MIN);
    }
    expired
}

/// **The stage**: every warm or hot system's live sites go to the signal
/// tracker (`signal_tracker::add_site` / `remove_site`), re-read every
/// [`SYNC_EVERY`]; a system that went cold takes its sites with it.
pub fn tick(reg: &mut Registry, _now: Instant, ft: BlueTime) {
    let live: BTreeSet<i64> = reg
        .ext::<crate::liveness::Liveness>()
        .iter()
        .map(|(s, _)| s)
        .collect();
    let pop = reg.ext::<Population>();
    let gone: Vec<i64> = pop
        .shown
        .keys()
        .filter(|s| !live.contains(s))
        .copied()
        .collect();
    let due: Vec<i64> = live
        .iter()
        .filter(|s| pop.due.get(s).is_none_or(|t| ft >= *t))
        .copied()
        .collect();
    if gone.is_empty() && due.is_empty() {
        return;
    }
    settle(reg);
    for system in gone {
        let pop = reg.ext_mut::<Population>();
        pop.due.remove(&system);
        for id in pop.shown.remove(&system).unwrap_or_default() {
            crate::signal_tracker::remove_site(reg, system, id);
        }
    }
    for system in due {
        sync(reg, system, ft);
    }
}

/// Re-read one system's slots and hand the tracker the difference.
fn sync(reg: &mut Registry, system: i64, now: BlueTime) {
    let r = sites(reg, system, now);
    drop_stale(reg, system, &r.stale);
    let want: BTreeMap<i64, Site> = r
        .sites
        .into_iter()
        .filter(|s| s.state == State::Live)
        .map(|s| (s.site_id, s))
        .collect();
    let have = reg
        .ext_mut::<Population>()
        .shown
        .remove(&system)
        .unwrap_or_default();
    for id in have.iter().filter(|id| !want.contains_key(id)) {
        crate::signal_tracker::remove_site(reg, system, *id);
    }
    for (id, s) in &want {
        if !have.contains(id) {
            let site = tracker_site(reg, s);
            crate::signal_tracker::add_site(reg, site);
        }
    }
    let pop = reg.ext_mut::<Population>();
    pop.shown.insert(system, want.into_keys().collect());
    pop.due.insert(system, now + SYNC_EVERY);
}

/// The tracker's record of a site (C2 §2.4).
fn tracker_site(reg: &mut Registry, s: &Site) -> crate::signal_tracker::Site {
    use crate::signal_tracker::{GROUP_ANOMALY, GROUP_SIGNATURE};
    let allowed = match reg.ext::<Population>().allowed.get(&s.dungeon_id) {
        Some(a) => a.clone(),
        None => {
            let a = allowed_types(reg, s.dungeon_id);
            reg.ext_mut::<Population>()
                .allowed
                .insert(s.dungeon_id, a.clone());
            a
        }
    };
    crate::signal_tracker::Site {
        site_id: s.site_id,
        instance_id: s.site_id,
        dungeon_id: s.dungeon_id,
        solar_system_id: s.system,
        position: s.anchor,
        group_id: match s.entry {
            Entry::Anomaly => GROUP_ANOMALY,
            Entry::Signature => GROUP_SIGNATURE,
        },
        is_scannable: true,
        scan_strength_attribute: scan_attribute(s.archetype),
        scan_strength_value: SIGNATURE_STRENGTH,
        signature_radius: SIGNATURE_RADIUS,
        target_id: s.target_id.clone(),
        allowed_types: allowed,
    }
}

/// `dungeon_data::allowed_types` over the world's typedata
/// (`reg.content()`); empty (no restriction) when either is missing.
fn allowed_types(reg: &Registry, dungeon: i64) -> Vec<i64> {
    let (Some(t), Some(td)) = (
        reg.content().try_get::<dungeon_data::Table>(),
        reg.content().try_get::<crate::typedata::Installed>(),
    ) else {
        return Vec::new();
    };
    let types = td
        .table()
        .types()
        .map(|r| (r.type_id, r.group_id, r.category_id));
    dungeon_data::allowed_types(t, dungeon, types).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeon_data::{Dungeon, EXCLUDED_FACTIONS, Resource, ResourceKind};
    use crate::model::SandboxConfig;
    use crate::space_state::{Motion, Presence};
    use eve_marshal::PyValue;
    use std::sync::Arc;

    const PIRATE_A: i64 = 599_011;
    const PIRATE_B: i64 = 599_012;
    const SEED: u64 = 0x5eed_c021;

    /// Invented dungeons covering every fixture row, plus one the exclusions
    /// must keep out.
    fn dungeons() -> dungeon_data::Table {
        let d = |id, archetype, faction, difficulty, entry: Entry| Dungeon {
            id,
            archetype,
            faction,
            difficulty: Some(difficulty),
            entry_type: Some(entry.type_id()),
            ..Dungeon::default()
        };
        let ore = |id, band| Dungeon {
            resources: vec![Resource {
                kind: ResourceKind::Ore,
                type_ids: vec![990_003],
                ore_band: Some(band),
            }],
            ..d(id, 97, None, 1, Entry::Anomaly)
        };
        let c = COMBAT_ARCHETYPE;
        let (e1, e2) = (FACTION_ARCHETYPES[1], FACTION_ARCHETYPES[2]);
        let list = [
            d(999_201, c, Some(PIRATE_A), 1, Entry::Anomaly),
            d(999_202, c, Some(PIRATE_A), 2, Entry::Anomaly),
            d(999_203, c, Some(PIRATE_B), 2, Entry::Anomaly),
            d(999_204, c, Some(PIRATE_B), 3, Entry::Anomaly),
            d(999_205, c, Some(PIRATE_A), 2, Entry::Signature),
            d(999_206, c, Some(PIRATE_A), 3, Entry::Signature),
            ore(999_207, OreBand::High),
            ore(999_208, OreBand::Null),
            d(999_209, e1, Some(PIRATE_B), 2, Entry::Signature),
            d(999_210, e2, Some(PIRATE_B), 2, Entry::Signature),
            d(999_211, c, Some(EXCLUDED_FACTIONS[0]), 2, Entry::Anomaly),
            d(999_212, c, Some(PIRATE_B), 2, Entry::Anomaly),
        ];
        dungeon_data::Table {
            dungeons: list.into_iter().map(|d| (d.id, d)).collect(),
            ..dungeon_data::Table::default()
        }
    }

    /// The universe fixture's regions 19_000_001 and 19_000_002.
    fn pirate(region: i64) -> Option<i64> {
        match region {
            19_000_001 => Some(PIRATE_A),
            19_000_002 => Some(PIRATE_B),
            _ => None,
        }
    }

    fn prep_with(table: &Table) -> Prepared {
        Prepared::new(table, &Universe::fixture(), &dungeons(), &pirate)
    }

    fn prep() -> Prepared {
        prep_with(&Table::fixture())
    }

    fn systems() -> Vec<i64> {
        Universe::fixture().systems().map(|s| s.id).collect()
    }

    fn cat() -> Catalogue {
        let mut text = String::new();
        for (n, s) in systems().into_iter().enumerate() {
            let x = 1.0e11 * (n as f64 + 1.0);
            text += &format!(
                "planet\t{}\t990011\t6000000.0\t{x}\t0.0\t0.0\t{s}\n",
                40_000_000 + n
            );
            text += &format!(
                "moon\t{}\t990012\t1000000.0\t{x}\t4.0e8\t0.0\t{s}\n",
                40_100_000 + n
            );
        }
        Catalogue::parse(&text)
    }

    fn resolve(
        p: &Prepared,
        cat: &Catalogue,
        deltas: &Rows,
        system: i64,
        now: BlueTime,
    ) -> Resolved {
        resolve_at(
            p,
            cat,
            deltas,
            system,
            now,
            crate::game_clock::GameClock::REAL,
        )
    }

    fn resolve_at(
        p: &Prepared,
        cat: &Catalogue,
        deltas: &Rows,
        system: i64,
        now: BlueTime,
        clock: crate::game_clock::GameClock,
    ) -> Resolved {
        let w = Inputs {
            seed: SEED,
            birth: crate::delta::World::FIXTURE.birth,
            celestials: cat,
            deltas,
            clock,
        };
        p.sites(&w, system, 0, now)
    }

    fn birth() -> BlueTime {
        crate::delta::World::FIXTURE.birth
    }

    fn row_index(p: &Prepared, key: &str) -> usize {
        p.rows()
            .iter()
            .position(|r| r.key == key)
            .expect("a fixture key")
    }

    /// The same inputs give the same sites, on a fresh `Prepared` too.
    #[test]
    fn the_generator_is_deterministic() {
        let (a, b, cat) = (prep(), prep(), cat());
        for s in systems() {
            let one = resolve(&a, &cat, &Rows::new(), s, birth());
            assert_eq!(one, resolve(&b, &cat, &Rows::new(), s, birth()));
            assert_eq!(one, resolve(&a, &cat, &Rows::new(), s, birth()));
        }
    }

    /// Per system, each band row has exactly its count's slots (the whole
    /// part, plus at most one for a fraction), and no other band's rows.
    #[test]
    fn the_count_per_row_key_equals_the_tables() {
        let (p, u) = (prep(), Universe::fixture());
        let mut fractions_seen = BTreeSet::new();
        for s in u.systems() {
            let slots = p.slots(SEED, s.id, 0);
            for (i, row) in p.rows().iter().enumerate() {
                let n = slots.iter().filter(|x| x.row == i).count() as f64;
                if row.band != s.band() {
                    assert_eq!(n, 0.0, "{} in {}", row.key, s.id);
                } else if row.count.fract() == 0.0 {
                    assert_eq!(n, row.count, "{} in {}", row.key, s.id);
                } else {
                    assert!(
                        n == row.count.floor() || n == row.count.ceil(),
                        "{}",
                        row.key
                    );
                    fractions_seen.insert(row.key.clone());
                }
            }
            let idx: Vec<usize> = slots.iter().map(|x| x.index).collect();
            assert_eq!(idx, (0..slots.len()).collect::<Vec<_>>());
        }
        assert!(
            !fractions_seen.is_empty(),
            "the fixture exercises a fraction"
        );
    }

    /// The floor adds `k` values after the row's own count, on the first
    /// combat-anomaly row, and after every row's own slots.
    #[test]
    fn the_floor_appends_slots_and_moves_none() {
        let p = prep();
        let s = 39_000_003; // null
        let row = row_index(&p, "fx_combat_null");
        let base = p.slots(SEED, s, 0);
        let pilots = 3 * p.rows()[row].count as usize + 3;
        let more = p.slots(SEED, s, pilots);
        assert_eq!(&more[..base.len()], &base[..]);
        let extra = &more[base.len()..];
        assert!(extra.iter().all(|x| x.row == row));
        let ks: Vec<u32> = extra.iter().map(|x| x.k).collect();
        let own = p.rows()[row].count as u32;
        assert_eq!(ks, (own..floor_for(pilots)).collect::<Vec<_>>());
    }

    /// Every drawn dungeon passes `dungeon_data::admits` for its row and the
    /// system's pirate, over several generations; the excluded faction's
    /// dungeon never appears.
    #[test]
    fn every_draw_satisfies_the_filters() {
        let (p, u, d) = (prep(), Universe::fixture(), dungeons());
        let mut drawn = 0;
        for s in u.systems() {
            for slot in p.slots(SEED, s.id, 6) {
                let row = &p.rows()[slot.row];
                for g in 0..8 {
                    let Some(id) = p.draw(SEED, s.id, slot.row, slot.k, g) else {
                        continue;
                    };
                    drawn += 1;
                    let dun = &d.dungeons[&id];
                    let difficulties = match &row.difficulties {
                        Diffs::In(v) => Difficulties::In(v),
                        Diffs::Star => Difficulties::Star(ore_band(s.band())),
                    };
                    let want = dungeon_data::Slot {
                        archetypes: &row.archetypes,
                        entry: row.entry,
                        difficulties,
                        faction: pirate(s.region_id),
                    };
                    assert!(
                        dungeon_data::admits(dun, &want, &dungeon_data::Exclusions::C3, None),
                        "{id}"
                    );
                    assert!(!EXCLUDED_FACTIONS.contains(&dun.faction.unwrap_or(0)));
                }
            }
        }
        assert!(drawn > 0);
    }

    /// An empty candidate set is an empty slot: the slot exists, draws
    /// `None`, and yields no site; nothing panics.
    #[test]
    fn an_empty_candidate_set_gives_an_empty_slot() {
        let p = prep();
        let row = row_index(&p, "fx_empty_low");
        let s = 39_000_002; // low
        let slots = p.slots(SEED, s, 0);
        assert!(slots.iter().any(|x| x.row == row));
        assert_eq!(p.draw(SEED, s, row, 0, 0), None);
        let r = resolve(&p, &cat(), &Rows::new(), s, birth());
        assert!(r.sites.iter().all(|x| x.row != row));
    }

    /// Ids are unique across every system, slot and generation, inside the
    /// site band, and clear of the NPC and rock ranges.
    #[test]
    fn ids_are_unique_and_inside_the_band() {
        let p = prep();
        let mut seen = BTreeSet::new();
        for s in systems() {
            let rank = p.sys_rank(s).expect("a fixture system");
            for slot in p.slots(SEED, s, 9) {
                for g in 0..4 {
                    let id = site_id(rank, slot.index, g).expect("in range");
                    assert!((SITE_BASE..SITE_END).contains(&id));
                    assert!(!(crate::npc::MIN_NPC_ID..crate::npc::END_NPC_ID).contains(&id));
                    assert!(id > crate::asteroid::MIN_ROCK_ID);
                    assert!(seen.insert(id), "{id} twice");
                }
            }
        }
        let last = site_id(MAX_RANK - 1, SLOTS_PER_SYSTEM - 1, GENERATIONS - 1).unwrap();
        assert!(last < SITE_END);
        assert_eq!(site_id(MAX_RANK, 0, 0), None);
        assert_eq!(site_id(0, SLOTS_PER_SYSTEM, 0), None);
    }

    /// `sys_rank` is the id order: distinct, dense and ascending with the id.
    #[test]
    fn sys_rank_is_injective_over_the_fixture_systems() {
        let p = prep();
        let ranks: Vec<usize> = systems().iter().map(|s| p.sys_rank(*s).unwrap()).collect();
        assert_eq!(ranks, (0..ranks.len()).collect::<Vec<_>>());
        assert_eq!(p.sys_rank(1), None);
    }

    /// Changing one row's count leaves every other row's `(key, k)` drawing
    /// the same dungeon at the same anchor.
    #[test]
    fn editing_one_rows_count_moves_no_other_rows_slots() {
        let mut edited = Table::fixture();
        let changed = edited
            .rows
            .iter()
            .position(|r| r.key == "fx_sig_high")
            .unwrap();
        edited.rows[changed].count += 2.0;
        let (a, b, cat) = (prep(), prep_with(&edited), cat());
        let by_slot = |r: Resolved| -> BTreeMap<(usize, u32), (i64, [u64; 3])> {
            r.sites
                .into_iter()
                .filter(|s| s.row != changed)
                .map(|s| ((s.row, s.k), (s.dungeon_id, s.anchor.map(f64::to_bits))))
                .collect()
        };
        for s in systems() {
            let before = by_slot(resolve(&a, &cat, &Rows::new(), s, birth()));
            assert_eq!(
                before,
                by_slot(resolve(&b, &cat, &Rows::new(), s, birth())),
                "{s}"
            );
        }
    }

    fn done_row(p: &Prepared, system: i64, row: usize, g: u32, at: BlueTime) -> Rows {
        let d = p.draw(SEED, system, row, 0, g).expect("a candidate");
        let v = vec![g.to_string(), "done".into(), at.to_string(), d.to_string()];
        Rows::from([(p.delta_key(system, row, 0), v)])
    }

    /// A completed slot shows done until `respawn_s` has passed, then live
    /// as `generation + 1`, born at the refill time.
    #[test]
    fn a_completed_slot_refills_after_respawn_with_the_next_generation() {
        let (p, cat) = (prep(), cat());
        let (s, row) = (39_000_001, row_index(&p, "fx_combat_high"));
        let t0 = birth() + 5 * BLUE_PER_SECOND;
        let rows = done_row(&p, s, row, 3, t0);
        let respawn = p.rows()[row].respawn_s * BLUE_PER_SECOND;
        let at = |now| {
            let r = resolve(&p, &cat, &rows, s, now);
            assert!(r.stale.is_empty());
            let x = r
                .sites
                .into_iter()
                .find(|x| x.row == row && x.k == 0)
                .unwrap();
            (x.generation, x.state, x.at)
        };
        assert_eq!(at(t0 + respawn - 1), (3, State::Done, t0));
        assert_eq!(at(t0 + respawn), (4, State::Live, t0 + respawn));
    }

    /// Lane clock: at scale N a completed slot refills N times sooner.
    #[test]
    fn at_scale_n_a_completed_slot_refills_n_times_sooner() {
        let (p, cat) = (prep(), cat());
        let (s, row) = (39_000_001, row_index(&p, "fx_combat_high"));
        let t0 = birth() + 5 * BLUE_PER_SECOND;
        let rows = done_row(&p, s, row, 3, t0);
        let clock = crate::game_clock::GameClock::new(6).unwrap();
        let respawn = clock.span(p.rows()[row].respawn_s * BLUE_PER_SECOND);
        assert!(respawn * 6 <= p.rows()[row].respawn_s * BLUE_PER_SECOND);
        let at = |now| {
            let r = resolve_at(&p, &cat, &rows, s, now, clock);
            let x = r
                .sites
                .into_iter()
                .find(|x| x.row == row && x.k == 0)
                .unwrap();
            (x.generation, x.state, x.at)
        };
        assert_eq!(at(t0 + respawn - 1), (3, State::Done, t0));
        assert_eq!(at(t0 + respawn), (4, State::Live, t0 + respawn));
    }

    /// A `site` delta whose stored dungeon is not the re-draw, or whose key
    /// names no row, is reported stale, and the slot is generation 0.
    #[test]
    fn a_site_delta_whose_dungeon_disagrees_is_dropped() {
        let (p, cat) = (prep(), cat());
        let (s, row) = (39_000_001, row_index(&p, "fx_combat_high"));
        let mut rows = done_row(&p, s, row, 2, birth());
        let key = p.delta_key(s, row, 0);
        let wrong = 1 + p.draw(SEED, s, row, 0, 2).unwrap();
        rows.get_mut(&key).unwrap()[3] = wrong.to_string();
        let gone = vec![s.to_string(), "24".into(), "no_such_key".into(), "0".into()];
        rows.insert(
            gone.clone(),
            vec!["1".into(), "live".into(), "0".into(), String::new()],
        );
        let r = resolve(&p, &cat, &rows, s, birth());
        let stale: BTreeSet<_> = r.stale.into_iter().collect();
        assert_eq!(stale, BTreeSet::from([key, gone]));
        let x = r.sites.iter().find(|x| x.row == row && x.k == 0).unwrap();
        assert_eq!((x.generation, x.state), (0, State::Live));
    }

    /// A pin over the fixture world: every site of every fixture system, at
    /// generations 0 to 3. **When this moves, bump [`GEN_VERSION`]** in the
    /// same commit and say why (plan §3.1).
    #[test]
    fn the_fixture_world_digest_is_pinned() {
        let (p, cat) = (prep(), cat());
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        let mut eat = |v: u64| h = (h ^ v).wrapping_mul(0x0100_0000_01b3);
        for s in systems() {
            for g in 0..4u32 {
                let mut rows = Rows::new();
                for slot in p.slots(SEED, s, 0) {
                    let d = p.draw(SEED, s, slot.row, slot.k, g);
                    let v = vec![
                        g.to_string(),
                        "live".into(),
                        birth().to_string(),
                        d.map_or(String::new(), |d| d.to_string()),
                    ];
                    rows.insert(p.delta_key(s, slot.row, slot.k), v);
                }
                for x in resolve(&p, &cat, &rows, s, birth()).sites {
                    eat(x.site_id as u64);
                    eat(x.dungeon_id as u64);
                    x.anchor.iter().for_each(|c| eat(c.to_bits()));
                    x.target_id.bytes().for_each(|b| eat(u64::from(b)));
                }
            }
        }
        assert_eq!(h, 0x60ac_2edb_81f1_9958, "the fixture world moved: {h:#x}");
    }

    /// The shipped file parses whole: no row skipped, keys unique. (No value
    /// in it is asserted; every one is [A].)
    #[test]
    fn the_shipped_file_loads() {
        let dir = crate::authored::Dir::compiled_only();
        let file = dir.load(FILE, SHIPPED, HEADER).expect("the header matches");
        let (rows, skipped) = file.parse_rows_logged(parse_row);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert!(Table::new(rows).is_ok());
    }

    // ---- the Registry side ----

    const PILOT: i64 = 1_000_001;

    fn world() -> Registry {
        let mut c = Content::fixture();
        c.insert("universe", Universe::fixture());
        c.insert("dungeons", dungeons());
        c.insert("population", Table::fixture());
        let mut reg = Registry::new(SandboxConfig {
            celestials: cat(),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        *reg.ext_mut::<Population>() = Population::new(prep());
        reg.deltas_mut().set_world(crate::delta::World {
            seed: SEED,
            gen_version: GEN_VERSION,
            ..crate::delta::World::FIXTURE
        });
        reg
    }

    fn pilot_at(reg: &mut Registry, system: i64, at: [f64; 3]) {
        let p = Presence::arriving(PILOT, 100_000_001, system, Motion::arrived(at, [0.0; 3]));
        reg.roster_mut().arrive(p);
    }

    /// A universe of `n` invented high- and low-sec systems in the
    /// fixture's region, so a bench has the size of a real Agency answer.
    fn big_world(n: i64) -> Registry {
        let rows: Vec<String> = (0..n)
            .map(|i| {
                let sec = if i % 2 == 0 { "0.75" } else { "0.25" };
                format!(
                    "{}\t19000001\t29000001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tBig {i}",
                    38_000_000 + i
                )
            })
            .collect();
        let u = Universe::parse(
            &rows,
            &["19000001\t\t\t\t\tR".to_owned()],
            &["29000001\t19000001\t\t\t\tC".to_owned()],
        )
        .expect("the invented universe parses");
        let prep = Prepared::new(&Table::fixture(), &u, &dungeons(), &pirate);
        let mut c = Content::fixture();
        c.insert("universe", u);
        c.insert("dungeons", dungeons());
        c.insert("population", Table::fixture());
        let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
        *reg.ext_mut::<Population>() = Population::new(prep);
        reg.deltas_mut().set_world(crate::delta::World {
            seed: SEED,
            gen_version: GEN_VERSION,
            ..crate::delta::World::FIXTURE
        });
        reg
    }

    /// The Agency's cold-system counts are kept, and a delta row (a site
    /// done) or a new seed drops them: the cached answer always equals a
    /// fresh walk.
    #[test]
    fn agency_cold_counts_follow_the_deltas() {
        use crate::signal_tracker::{COMBAT, agency_instances, compute_cold};
        let mut reg = big_world(40);
        let fresh = |reg: &Registry| compute_cold(reg, COMBAT);
        let ask = |reg: &Registry| agency_instances(reg, None, COMBAT);
        let first = ask(&reg);
        assert_eq!(ask(&reg), first, "a second call answers the same bytes");
        let before = fresh(&reg);
        assert!(!before.is_empty(), "the fixture makes combat sites");
        // Mark every slot of the first combat system done.
        let (&sys, _) = before.iter().next().unwrap();
        let prep = reg.ext::<Population>().prepared();
        let keys: Vec<_> = prep
            .slots(SEED, sys, 0)
            .iter()
            .map(|sl| prep.delta_key(sys, sl.row, sl.k))
            .collect();
        for k in keys {
            reg.deltas_mut()
                .put(KIND, k, vec!["0".to_owned(), "done".to_owned()]);
        }
        let after = fresh(&reg);
        assert!(!after.contains_key(&sys), "{sys} lost its combat sites");
        assert_ne!(ask(&reg), first, "the cache saw the delta");
        let PyValue::Dict(rows) = ask(&reg) else {
            panic!("a dict")
        };
        let want: Vec<_> = after
            .iter()
            .map(|(&s, &n)| (PyValue::Int(n), PyValue::Int(s)))
            .collect();
        assert_eq!(rows, want);
    }

    /// The cold-count cache is keyed on the population `site` rows alone: a
    /// write of any other delta kind leaves it warm (same `Arc`), a `site`
    /// write drops it, and either way the answer is the uncached walk.
    #[test]
    fn agency_cold_counts_key_on_the_site_kind_only() {
        use crate::signal_tracker::{COMBAT, cold_counts, compute_cold};
        let mut reg = big_world(40);
        let warm = cold_counts(&reg, COMBAT);
        assert_eq!(*warm, compute_cold(&reg, COMBAT));
        for k in crate::delta_kinds::KINDS
            .iter()
            .map(|k| k.0)
            .filter(|k| *k != KIND)
        {
            reg.deltas_mut()
                .put_verbatim(k, vec!["1".to_owned()], vec![]);
            assert!(
                Arc::ptr_eq(&warm, &cold_counts(&reg, COMBAT)),
                "a `{k}` write must keep the cache warm"
            );
        }
        let (&sys, _) = warm.iter().next().unwrap();
        let prep = reg.ext::<Population>().prepared();
        let keys: Vec<_> = prep
            .slots(SEED, sys, 0)
            .iter()
            .map(|sl| prep.delta_key(sys, sl.row, sl.k))
            .collect();
        for k in keys {
            reg.deltas_mut()
                .put(KIND, k, vec!["0".to_owned(), "done".to_owned()]);
        }
        let cold = cold_counts(&reg, COMBAT);
        assert!(!Arc::ptr_eq(&warm, &cold), "a `site` write invalidates");
        assert_eq!(*cold, compute_cold(&reg, COMBAT));
        assert!(!cold.contains_key(&sys));
    }

    /// Benchmark: the Agency combat call on 8000 systems, a fresh walk (what
    /// every call cost before) against the kept answer.
    #[test]
    #[ignore = "benchmark: run by hand with --nocapture"]
    fn bench_agency_instances_8000_systems() {
        use crate::signal_tracker::{COMBAT, agency_instances, compute_cold};
        let reg = big_world(8000);
        let t = Instant::now();
        for _ in 0..5 {
            std::hint::black_box(compute_cold(&reg, COMBAT));
        }
        let walk = t.elapsed() / 5;
        std::hint::black_box(agency_instances(&reg, None, COMBAT));
        let t = Instant::now();
        for _ in 0..50 {
            std::hint::black_box(agency_instances(&reg, None, COMBAT));
        }
        let kept = t.elapsed() / 50;
        crate::outln!("agency combat call: fresh walk {walk:?}, kept {kept:?}");
    }

    /// The first day whose start is at least `ttl_s` after the birth.
    fn first_day_after(reg: &Registry, ttl_s: i64) -> u32 {
        let b = reg.world().birth;
        (1..)
            .find(|d| seed::day_start(b, *d) - b >= ttl_s * BLUE_PER_SECOND)
            .unwrap()
    }

    /// `on_rollover(day)` expires each unvisited live site once its row's
    /// `ttl_s` has passed, refilling it as `generation + 1`, and keeps the
    /// one a pilot is inside. Before `ttl_s`, nothing expires.
    #[test]
    fn on_rollover_expires_unvisited_sites_and_keeps_one_with_a_pilot() {
        let mut reg = world();
        let s = 39_000_001;
        let first = sites(&reg, s, reg.world().birth).sites;
        let kept = first[0].clone();
        pilot_at(&mut reg, s, kept.anchor);
        let birth = reg.world().birth;
        crate::liveness::refresh(&mut reg, Instant::now(), birth);
        let ttl = reg.ext::<Population>().prep.rows()[kept.row].ttl_s;
        let day = first_day_after(&reg, ttl);
        assert_eq!(on_rollover(&mut reg, day - 1), 0);
        let n = on_rollover(&mut reg, day);
        assert_eq!(n, first.len() - 1);
        let t = seed::day_start(reg.world().birth, day);
        let after = sites(&reg, s, t).sites;
        for x in &after {
            let was = first.iter().find(|f| (f.row, f.k) == (x.row, x.k)).unwrap();
            if (x.row, x.k) == (kept.row, kept.k) {
                assert_eq!((x.generation, x.site_id), (kept.generation, kept.site_id));
            } else {
                assert_eq!((x.generation, x.at), (was.generation + 1, t));
            }
        }
    }

    /// A different `gen_version` drops every `site` and `wh` delta and
    /// records the current one.
    #[test]
    fn a_gen_version_change_drops_every_site_delta() {
        let mut reg = world();
        let w = crate::delta::World {
            gen_version: GEN_VERSION + 1,
            ..*reg.world()
        };
        reg.deltas_mut().set_world(w);
        let d = reg.deltas_mut();
        d.put(
            KIND,
            vec!["39000001".into(), "24".into(), "fx".into(), "0".into()],
            vec![],
        );
        d.put(
            WH_KIND,
            vec!["39000001".into(), "fx".into(), "0".into()],
            vec![],
        );
        settle(&mut reg);
        assert!(reg.deltas().of(KIND).is_empty());
        assert!(reg.deltas().of(WH_KIND).is_empty());
        assert_eq!(reg.world().gen_version, GEN_VERSION);
    }

    /// A live system's sites reach the tracker on the stage; a completed one
    /// leaves it on the next stage and is persisted as `done`.
    #[test]
    fn the_stage_hands_live_sites_to_the_tracker_and_complete_removes_one() {
        let mut reg = world();
        let s = 39_000_001;
        pilot_at(&mut reg, s, [0.0; 3]);
        let now = reg.world().birth;
        crate::liveness::refresh(&mut reg, Instant::now(), now);
        tick(&mut reg, Instant::now(), now);
        let shown = reg.ext::<Population>().shown[&s].clone();
        let live = sites(&reg, s, now).sites;
        assert_eq!(shown, live.iter().map(|x| x.site_id).collect());
        let gone = live[0].clone();
        assert!(complete(&mut reg, s, gone.site_id, now));
        assert!(!complete(&mut reg, s, gone.site_id, now), "already done");
        tick(&mut reg, Instant::now(), now);
        assert!(!reg.ext::<Population>().shown[&s].contains(&gone.site_id));
        let key = reg.ext::<Population>().prep.delta_key(s, gone.row, gone.k);
        assert_eq!(reg.deltas().of(KIND)[&key][1], "done");
    }

    // ---- Y-WH: wormhole space -------------------------------------------

    /// Invented J-space systems of classes 1, 4 and 6 (the [V] class
    /// constants are the rule's; the ids and names are invented).
    fn wh_universe() -> Universe {
        let sys = |id: i64, class: i64| {
            format!(
                "{id}\t19000001\t29000001\t\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t{class}\t0\t\t\tWH {id}"
            )
        };
        let lines = |rows: Vec<String>| rows;
        Universe::parse(
            &lines(vec![
                sys(31_990_001, 1),
                sys(31_990_004, 4),
                sys(31_990_006, 6),
                "39000001\t19000001\t29000001\t\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tK".into(),
            ]),
            &["19000001\t\t\t\t\tR".to_owned()],
            &["29000001\t19000001\t\t\t\tC".to_owned()],
        )
        .expect("the invented universe parses")
    }

    const WH_GAS: i64 = 990_501;
    const WH_FRONTIER_GAS: i64 = 990_502;
    const WH_CORE_GAS: i64 = 990_503;
    const WH_COMBAT: i64 = 990_504;

    /// Invented J-space dungeons: all carry the excluded faction, and the
    /// gas ones the excluded Fullerite type.
    fn wh_dungeons() -> dungeon_data::Table {
        let gas = |id| Dungeon {
            id,
            archetype: 30,
            faction: Some(EXCLUDED_FACTIONS[0]),
            difficulty: Some(3),
            entry_type: Some(Entry::Signature.type_id()),
            resources: vec![Resource {
                kind: ResourceKind::Gas,
                type_ids: vec![dungeon_data::EXCLUDED_GAS_TYPES[0]],
                ore_band: None,
            }],
            ..Dungeon::default()
        };
        let combat = Dungeon {
            id: WH_COMBAT,
            archetype: COMBAT_ARCHETYPE,
            faction: Some(EXCLUDED_FACTIONS[0]),
            difficulty: Some(3),
            entry_type: Some(Entry::Anomaly.type_id()),
            ..Dungeon::default()
        };
        let list = [gas(WH_GAS), gas(WH_FRONTIER_GAS), gas(WH_CORE_GAS), combat];
        dungeon_data::Table {
            dungeons: list.into_iter().map(|d| (d.id, d)).collect(),
            ..dungeon_data::Table::default()
        }
    }

    fn wh_table() -> Table {
        let text = format!(
            "{HEADER}\n\
             wh_gas\twormhole\t30\tsignature\t2\t*\t600\t172800\tx\n\
             wh_combat\twormhole\t{COMBAT_ARCHETYPE}\tanomaly\t1\t*\t600\t172800\tx\n\
             k_gas\thigh\t30\tsignature\t1\t2\t600\t172800\tx\n"
        );
        let (raw, _) = crate::authored::parse(&text, HEADER).expect("header");
        Table::new(raw.iter().map(|r| parse_row(r).expect("row")).collect()).expect("table")
    }

    fn wh_prep() -> Prepared {
        let (u, d) = (wh_universe(), wh_dungeons());
        Prepared::new(&wh_table(), &u, &d, &|_| None).with_wormhole(
            &crate::wormhole_sites::Sites::fixture(),
            &u,
            &d,
        )
    }

    /// A J-space system draws, for each row, only the dungeons the sites
    /// table lists for its class, though every one carries the faction and
    /// the Fullerite type that k-space excludes.
    #[test]
    fn a_wormhole_system_draws_the_dungeons_of_its_class() {
        let p = wh_prep();
        let (gas, combat) = (row_index(&p, "wh_gas"), row_index(&p, "wh_combat"));
        let drawn = |system, row| -> BTreeSet<i64> {
            (0..64)
                .filter_map(|g| p.draw(SEED, system, row, 0, g))
                .collect()
        };
        assert_eq!(drawn(31_990_001, gas), BTreeSet::from([WH_GAS]));
        assert_eq!(
            drawn(31_990_004, gas),
            BTreeSet::from([WH_GAS, WH_FRONTIER_GAS])
        );
        assert_eq!(
            drawn(31_990_006, gas),
            BTreeSet::from([WH_GAS, WH_FRONTIER_GAS, WH_CORE_GAS])
        );
        assert_eq!(drawn(31_990_001, combat), BTreeSet::from([WH_COMBAT]));
        assert!(
            drawn(31_990_004, combat).is_empty(),
            "the combat site is C1 only"
        );
    }

    /// The same dungeons stay excluded where the C3 rule applies.
    #[test]
    fn the_exclusions_still_hold_outside_wormhole_space() {
        let d = wh_dungeons();
        let table = wh_table();
        let list = candidates(&table.rows()[2], Band::High, None, &d);
        assert!(list.is_empty(), "{list:?}");
    }

    /// A J-space system's slots, sites and ids come out of the usual
    /// machinery: the row's count, a dungeon of the class, a site id in the
    /// band; and a k-space system has none of the wormhole rows.
    #[test]
    fn a_wormhole_system_has_its_rows_slots_and_sites() {
        let p = wh_prep();
        let slots = p.slots(SEED, 31_990_001, 0);
        let keys: Vec<&str> = slots.iter().map(|s| p.rows()[s.row].key.as_str()).collect();
        assert_eq!(keys, ["wh_gas", "wh_gas", "wh_combat"]);
        let got = p.sites(
            &Inputs {
                seed: SEED,
                birth: birth(),
                celestials: &Catalogue::parse(""),
                deltas: &Rows::new(),
                clock: crate::game_clock::GameClock::REAL,
            },
            31_990_001,
            0,
            birth(),
        );
        assert_eq!(got.sites.len(), 3);
        assert!(got.stale.is_empty());
        for s in &got.sites {
            assert!([WH_GAS, WH_COMBAT].contains(&s.dungeon_id), "{s:?}");
            assert!((SITE_BASE..SITE_END).contains(&s.site_id));
        }
        assert_eq!(
            got.sites.iter().filter(|s| s.archetype == 30).count(),
            2,
            "two gas signatures"
        );
        let k = p.slots(SEED, 39_000_001, 0);
        assert!(
            k.iter().all(|s| p.rows()[s.row].band != Band::Wormhole),
            "{k:?}"
        );
    }

    /// The shipped file's wormhole rows parse, are in band `wormhole`, and
    /// come after every older row (so no older slot moved).
    #[test]
    fn the_shipped_wormhole_rows_are_appended() {
        let (raw, skipped) = crate::authored::parse(SHIPPED, HEADER).unwrap();
        assert!(skipped.is_empty());
        let rows: Vec<Row> = raw.iter().map(|r| parse_row(r).unwrap()).collect();
        let first = rows
            .iter()
            .position(|r| r.band == Band::Wormhole)
            .expect("wormhole rows");
        // The rich rows (L6b) are appended after them, in every band.
        assert!(
            rows[first..]
                .iter()
                .all(|r| r.band == Band::Wormhole || r.key.contains("rich"))
        );
        for want in [
            "wh_combat_anom",
            "wh_relic_sig",
            "wh_data_sig",
            "wh_gas_sig",
            "wh_ore_anom",
        ] {
            assert!(rows.iter().any(|r| r.key == want), "{want}");
        }
        assert!(
            rows.iter()
                .all(|r| !(r.band == Band::Wormhole && r.archetypes.contains(&28))),
            "no ice in J-space"
        );
    }
}
