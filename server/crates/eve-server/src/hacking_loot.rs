//! **What a hacked can holds** (HK3, R-HACK §3, §4 row HK3).
//!
//! The client ships no loot table for data and relic cans [V: R-HACK §0,
//! `resfileindex.txt` has none], so the table is ours **[A]**:
//! `explore/loot.tsv`, built by class from the bundle by
//! `tools/content_explore.py` (`docs/content/schema.md`). This module:
//!
//! - **loads it** ([`TABLE`], the `content_tables` pattern): per can kind
//!   (data, relic, ancient) its classes (datacores, decryptors, salvage, ...)
//!   and their types, plus the kind of every hackable can type;
//! - **rolls** ([`roll`], pure, on a [`Stream`]): `U(entityEquipmentMin,
//!   entityEquipmentMax)` entries [I: the can attributes' names], floored at
//!   one, scaled by the can's tier (`tierDifficulty` 1919) and its system's
//!   security band; each entry a class by its weight, then a type in it
//!   weighted toward the dearer as the tier rises; a copy row gives one
//!   blueprint copy whose runs are the row's range;
//! - **materialises** ([`on_won`], from `hacking::ended`): on a win the can
//!   becomes a loot container -- its stacks are store items **located at the
//!   can's ball id** and owned by the hacker, minted quietly, the way
//!   `combat::npc_wreck` puts an NPC's loot in its wreck; a copy's runs go
//!   to the industry blueprint table, so it lists as a copy (`quantity -2`);
//! - **is a loot source** ([`loot_source`]): the can as a `combat::Wreck`
//!   record (owner the hacker), so `invbroker` lists it to anyone in the
//!   system (`invCtrl.ItemFloatingCargo` is a `BaseCelestialContainer`, like
//!   `ItemWreck` [V]), and `combat::plan_loot` takes from it unchanged; a
//!   take without loot rights is theft ([`looted`];
//!   `containerGroupsWithLootRights` holds group 1207 [V]);
//! - **cleans up** ([`tick`], from `hacking::tick`): a can out of space
//!   takes its loot with it, and the first tick after a start destroys loot
//!   left at can ids from before the restart (sites are not persisted).
//!
//! **Site loot** (CAP-LOOT, `docs/lanes/cap-loot.md`): `explore/site-loot.tsv`
//! adds to a can's roll by the *dungeon* the can stands in and the can's own
//! type ([`SiteRow`]): one independent draw per row, in parts per million,
//! after the class roll so a table with no rows rolls as it did. It is how the
//! eleven capital commodities have sources. A row's `can_type_id` 0 means
//! any can of the site, and its `bands` limit it to high (`h`), low (`l`),
//! null (`n`) or wormhole (`w`) space.
//!
//! The can is never a store item itself: its id is the site's (`site.rs`'s
//! instance band), which the store cannot mint, so `GetSelfInvItem` gets a
//! row built here ([`self_item`]).

use crate::celestial::Band;
use crate::combat::Wreck;
use crate::content::{Boxed, Bundle, Sources, TableSpec};
use crate::industry::BlueprintState;
use crate::item::{Edit, Item, Slot};
use crate::seed::{self, Stream};
use crate::service::Registry;
use std::collections::{BTreeMap, BTreeSet};

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "hacking_loot",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

/// The bundle's rows; with no bundle an **empty** table (a hacked can opens
/// empty), as `econ_prices` does: the fixture's invented types are no
/// client's.
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Table::from_bundle(b)?,
        None => Table::default(),
    }))
}

/// The bundle file and its header line, copied from `schema.md`.
pub const LOOT: (&str, &str) = (
    "explore/loot.tsv",
    "#kind\tclass\ttype_id\tweight\tqty_min\tqty_max\tcopy\tvalue",
);

/// The site-loot file and its header line, copied from `schema.md`.
pub const SITE_LOOT: (&str, &str) = (
    "explore/site-loot.tsv",
    "#dungeon_id\tcan_type_id\ttype_id\tchance_ppm\tqty_min\tqty_max\tbands",
);

/// One draw's denominator: `chance_ppm` is parts per million.
pub const PPM: u64 = 1_000_000;

/// The class of the rows that name a can type's kind.
pub const CLASS_CAN: &str = "can";

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "hacking_loot",
    make: |_| Box::new(Loot::default()),
};

/// `requiredSkill1` [V] and Archaeology [V]: a can with no `can` row rolls
/// relic loot when it needs Archaeology, data loot otherwise.
const ATTR_REQUIRED_SKILL_1: i64 = 182;
/// `entityEquipmentMin` / `entityEquipmentMax` [V], on the can.
pub const ATTR_EQUIPMENT_MIN: i64 = 456;
pub const ATTR_EQUIPMENT_MAX: i64 = 457;

/// The flag an item lying in a hacked can carries. `[A]` `flagNone`, as
/// `combat::FLAG_IN_WRECK`: `ItemFloatingCargo` is a
/// `BaseCelestialContainer` and lists by location alone [V]. Falsified by a
/// can window that lists nothing.
pub const FLAG_IN_CAN: i64 = 0;

/// The tiers a can rolls at, as `hacking::begin` clamps `tierDifficulty`
/// (some types carry 5 [V]; the game has four).
pub const TIERS: std::ops::RangeInclusive<u8> = 1..=4;

/// `[A]` R-HACK §3: entries × `(1 + (tier − 1) × TIER_ENTRY_STEP)`.
/// Falsified by tier-4 cans that are no richer than tier-1 ones on TQ.
pub const TIER_ENTRY_STEP: f64 = 0.5;

/// `[A]` the security band's scale on the entry count: high, low, null and
/// wormhole space. Falsified by a week of hacking in which high-sec cans pay
/// as well as null-sec ones (the band should matter more), or low-sec ones
/// empty the market (less).
pub const BAND_SCALE_HIGH: f64 = 1.0;
pub const BAND_SCALE_LOW: f64 = 1.5;
pub const BAND_SCALE_NULL: f64 = 2.0;

/// `[A]` R-HACK §3: the classes whose quantity is multiplied by the tier.
/// Falsified by tier-4 salvage stacks that flood the market.
pub const TIER_QUANTITY_CLASSES: &[&str] = &["datacore", "salvage"];

/// `[A]` R-HACK §3: a type's weight in its class is
/// `1 + tier × log10(value + 1) / VALUE_DIVISOR`. Falsified by tier-4 cans
/// that never hold their class's dear types.
pub const VALUE_DIVISOR: f64 = 10.0;

/// A domain tag in the loot stream's parts, so a can's draw never meets an
/// NPC's (`npc::loot_from` draws `Kind::Loot` on the NPC id alone).
const STREAM_TAG: u64 = 0x0048_4b33;

/// The loot kind of a can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Data,
    Relic,
    /// A Sleeper or Talocan relic can (the extractor decides, [A]).
    Ancient,
}

impl Kind {
    fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "data" => Kind::Data,
            "relic" => Kind::Relic,
            "ancient" => Kind::Ancient,
            _ => return None,
        })
    }
}

/// One loot row: a type of a class of a kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub kind: Kind,
    pub class: String,
    pub type_id: i64,
    /// The class's weight (the same on every row of the class).
    pub weight: u32,
    /// The quantity range; a copy's runs.
    pub qty: (i64, i64),
    /// A blueprint copy: one item whose runs are `qty`.
    pub copy: bool,
    /// `P_ref`, else `basePrice`, else 0.
    pub value: f64,
}

/// One `explore/site-loot.tsv` row: an item a can of one dungeon may hold.
#[derive(Debug, Clone, PartialEq)]
pub struct SiteRow {
    pub dungeon_id: i64,
    /// The can's type; 0 = any can of the dungeon.
    pub can_type_id: i64,
    pub type_id: i64,
    /// The chance in parts per million, 1 to [`PPM`] (`PPM` = always).
    pub chance_ppm: u64,
    pub qty: (i64, i64),
    /// The bands it applies in (`h`, `l`, `n`, `w`); empty = every band.
    pub bands: String,
}

impl SiteRow {
    /// Whether the row applies to a can of `can_type` in `dungeon` in `band`
    /// (void or unknown space counts as high, as [`band_scale`]).
    pub fn applies(&self, dungeon: i64, can_type: i64, band: Option<Band>) -> bool {
        let letter = match band {
            Some(Band::Low) => 'l',
            Some(Band::Null) => 'n',
            Some(Band::Wormhole) => 'w',
            _ => 'h',
        };
        self.dungeon_id == dungeon
            && (self.can_type_id == 0 || self.can_type_id == can_type)
            && (self.bands.is_empty() || self.bands.contains(letter))
    }
}

/// `explore/loot.tsv` and `explore/site-loot.tsv`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub entries: Vec<Entry>,
    /// The site rows (CAP-LOOT), by dungeon.
    pub sites: Vec<SiteRow>,
    /// The kind each hackable can type rolls.
    pub cans: BTreeMap<i64, Kind>,
}

impl Table {
    /// The bundle's file; an absent one is an empty table.
    pub fn from_bundle(b: &Bundle) -> Result<Table, String> {
        let (rel, header) = LOOT;
        let mut t = Table::parse(&b.data_lines(rel, header)?).map_err(|e| format!("{rel}: {e}"))?;
        let (rel, header) = SITE_LOOT;
        t.sites = parse_sites(&b.data_lines(rel, header)?).map_err(|e| format!("{rel}: {e}"))?;
        Ok(t)
    }

    /// The data lines. A bad row is an error naming its line.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Table, String> {
        let mut t = Table::default();
        for (i, line) in lines.iter().enumerate() {
            let line = line.as_ref();
            let bad = || format!("data line {}: {line:?}", i + 1);
            let f: Vec<&str> = line.split('\t').collect();
            let [kind, class, type_id, weight, lo, hi, copy, value] = f[..] else {
                return Err(bad());
            };
            let kind = Kind::parse(kind).ok_or_else(bad)?;
            let type_id: i64 = type_id.parse().map_err(|_| bad())?;
            if class == CLASS_CAN {
                t.cans.insert(type_id, kind);
                continue;
            }
            let qty: (i64, i64) = (
                lo.parse().map_err(|_| bad())?,
                hi.parse().map_err(|_| bad())?,
            );
            if qty.0 < 1 || qty.1 < qty.0 {
                return Err(bad());
            }
            t.entries.push(Entry {
                kind,
                class: class.to_owned(),
                type_id,
                weight: weight.parse().map_err(|_| bad())?,
                qty,
                copy: match copy {
                    "0" => false,
                    "1" => true,
                    _ => return Err(bad()),
                },
                value: value.parse().map_err(|_| bad())?,
            });
        }
        Ok(t)
    }

    /// Invented rows (ids in the 99xxxx test band), never a client value.
    pub fn fixture() -> Table {
        Table::parse(&[
            "data\tdatacore\t990601\t4\t1\t3\t0\t0.0",
            "data\tbpc\t990602\t2\t1\t10\t1\t5000.0",
            "relic\tsalvage\t990603\t6\t1\t5\t0\t100.0",
            "data\tcan\t990604\t0\t0\t0\t0\t0.0",
        ])
        .unwrap_or_else(|e| unreachable!("the fixture parses: {e}"))
    }

    /// The kind a can type rolls, if the table names it.
    pub fn kind_of(&self, can_type_id: i64) -> Option<Kind> {
        self.cans.get(&can_type_id).copied()
    }
}

/// The data lines of `explore/site-loot.tsv`. A bad row is an error naming
/// its line.
pub fn parse_sites<S: AsRef<str>>(lines: &[S]) -> Result<Vec<SiteRow>, String> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let line = line.as_ref();
        let bad = || format!("data line {}: {line:?}", i + 1);
        let f: Vec<&str> = line.split('\t').collect();
        let [dungeon, can, ty, ppm, lo, hi, bands] = f[..] else {
            return Err(bad());
        };
        let row = SiteRow {
            dungeon_id: dungeon.parse().map_err(|_| bad())?,
            can_type_id: can.parse().map_err(|_| bad())?,
            type_id: ty.parse().map_err(|_| bad())?,
            chance_ppm: ppm.parse().map_err(|_| bad())?,
            qty: (
                lo.parse().map_err(|_| bad())?,
                hi.parse().map_err(|_| bad())?,
            ),
            bands: bands.to_owned(),
        };
        if row.chance_ppm == 0
            || row.chance_ppm > PPM
            || row.qty.0 < 1
            || row.qty.1 < row.qty.0
            || row.can_type_id < 0
            || !row.bands.chars().all(|c| "hlnw".contains(c))
        {
            return Err(bad());
        }
        out.push(row);
    }
    Ok(out)
}

/// **The site's draws**: one per applicable row, on the stream (after the
/// class roll). Only types `known` accepts take part.
pub fn site_roll(
    table: &Table,
    dungeon: i64,
    can_type: i64,
    band: Option<Band>,
    s: &mut Stream,
    known: impl Fn(i64) -> bool,
) -> Vec<Stack> {
    let mut out: Vec<Stack> = Vec::new();
    for r in table
        .sites
        .iter()
        .filter(|r| r.applies(dungeon, can_type, band) && known(r.type_id))
    {
        if s.range(0, PPM) >= r.chance_ppm {
            continue;
        }
        let n = s.range(r.qty.0 as u64, r.qty.1 as u64 + 1) as i64;
        match out.iter_mut().find(|x| x.type_id == r.type_id) {
            Some(x) => x.quantity += n,
            None => out.push(Stack {
                type_id: r.type_id,
                quantity: n,
                runs: None,
            }),
        }
    }
    out
}

/// One rolled stack: `runs` is `Some` for a blueprint copy (quantity 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stack {
    pub type_id: i64,
    pub quantity: i64,
    pub runs: Option<i64>,
}

/// What a roll depends on, besides the table and the stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Can {
    pub kind: Kind,
    /// Clamped into [`TIERS`].
    pub tier: u8,
    /// `(entityEquipmentMin, entityEquipmentMax)`.
    pub equipment: (i64, i64),
    pub band: Option<Band>,
}

/// The band's scale on the entry count; void or unknown space is high's.
pub fn band_scale(band: Option<Band>) -> f64 {
    match band {
        Some(Band::Low) => BAND_SCALE_LOW,
        Some(Band::Null | Band::Wormhole) => BAND_SCALE_NULL,
        _ => BAND_SCALE_HIGH,
    }
}

/// The tier's and the band's scale on the entry count.
pub fn scale(c: &Can) -> f64 {
    let tier = c.tier.clamp(*TIERS.start(), *TIERS.end());
    (1.0 + f64::from(tier - 1) * TIER_ENTRY_STEP) * band_scale(c.band)
}

/// How many entries a can rolls: `U(min, max)`, floored at one, scaled.
pub fn entry_count(c: &Can, s: &mut Stream) -> usize {
    let lo = c.equipment.0.max(1);
    let hi = c.equipment.1.max(lo);
    let base = s.range(lo as u64, hi as u64 + 1) as f64;
    ((base * scale(c)).round() as usize).max(1)
}

/// **The roll**: the can's entries, each a class of its kind by weight and
/// a type of the class, merged per type (copies never merge). Only types
/// `known` accepts take part, and a class left with none is skipped; a kind
/// with no class rolls nothing.
pub fn roll(table: &Table, c: &Can, s: &mut Stream, known: impl Fn(i64) -> bool) -> Vec<Stack> {
    let mut classes: BTreeMap<&str, (u32, Vec<&Entry>)> = BTreeMap::new();
    for e in table
        .entries
        .iter()
        .filter(|e| e.kind == c.kind && known(e.type_id))
    {
        let class = classes
            .entry(e.class.as_str())
            .or_insert((e.weight, Vec::new()));
        class.0 = class.0.max(e.weight);
        class.1.push(e);
    }
    classes.retain(|_, (w, m)| *w > 0 && !m.is_empty());
    let total: u64 = classes.values().map(|(w, _)| u64::from(*w)).sum();
    if total == 0 {
        return Vec::new();
    }
    let tier = i64::from(c.tier.clamp(*TIERS.start(), *TIERS.end()));
    let mut out: Vec<Stack> = Vec::new();
    for _ in 0..entry_count(c, s) {
        let mut at = s.range(0, total);
        let Some((class, (_, members))) = classes.iter().find(|(_, (w, _))| {
            let w = u64::from(*w);
            if at < w {
                true
            } else {
                at -= w;
                false
            }
        }) else {
            continue;
        };
        let e = pick(members, tier, s);
        let n = s.range(e.qty.0 as u64, e.qty.1 as u64 + 1) as i64;
        if e.copy {
            out.push(Stack {
                type_id: e.type_id,
                quantity: 1,
                runs: Some(n),
            });
            continue;
        }
        let n = if TIER_QUANTITY_CLASSES.contains(class) {
            n * tier
        } else {
            n
        };
        match out
            .iter_mut()
            .find(|x| x.type_id == e.type_id && x.runs.is_none())
        {
            Some(x) => x.quantity += n,
            None => out.push(Stack {
                type_id: e.type_id,
                quantity: n,
                runs: None,
            }),
        }
    }
    out
}

/// One member of a class, weighted by [`VALUE_DIVISOR`]'s rule.
fn pick<'e>(members: &[&'e Entry], tier: i64, s: &mut Stream) -> &'e Entry {
    let weight = |e: &Entry| 1.0 + tier as f64 * (e.value.max(0.0) + 1.0).log10() / VALUE_DIVISOR;
    let total: f64 = members.iter().map(|e| weight(e)).sum();
    let mut at = s.unit() * total;
    for e in members {
        let w = weight(e);
        if at < w {
            return e;
        }
        at -= w;
    }
    members[members.len() - 1]
}

// ---------------------------------------------------------------------------
// The world side
// ---------------------------------------------------------------------------

/// The hacked cans that hold loot: can id -> the hacker. **Not persisted**
/// (sites are not): [`tick`]'s first run sweeps loot left from before a
/// restart.
#[derive(Debug, Default)]
pub struct Loot {
    cans: BTreeMap<i64, i64>,
    swept: bool,
}

impl Loot {
    /// The hacker who owns a loot can.
    pub fn owner(&self, can_id: i64) -> Option<i64> {
        self.cans.get(&can_id).copied()
    }
}

fn type_row(w: &Registry, type_id: i64) -> Option<&crate::typedata::TypeRow> {
    w.content()
        .try_get::<crate::typedata::Installed>()?
        .table()
        .get(type_id)
}

/// The can's facts for a roll: kind (the table's, else by its skill), tier
/// and equipment from its type, band from its system.
fn can_facts(w: &Registry, type_id: i64, system: i64) -> Can {
    let row = type_row(w, type_id);
    let attr = |a: i64| row.and_then(|r| r.attribute(a));
    let kind = w
        .content()
        .try_get::<Table>()
        .and_then(|t| t.kind_of(type_id))
        .unwrap_or_else(
            || match attr(ATTR_REQUIRED_SKILL_1).map(|v| v.round() as i64) {
                Some(crate::hacking::SKILL_ARCHAEOLOGY) => Kind::Relic,
                _ => Kind::Data,
            },
        );
    let tier = attr(crate::hacking::ATTR_TIER_DIFFICULTY).map_or(1, |v| {
        v.round()
            .clamp(f64::from(*TIERS.start()), f64::from(*TIERS.end())) as u8
    });
    let int = |a: i64, d: i64| attr(a).map_or(d, |v| v.round() as i64);
    Can {
        kind,
        tier,
        equipment: (int(ATTR_EQUIPMENT_MIN, 1), int(ATTR_EQUIPMENT_MAX, 1)),
        band: w
            .content()
            .try_get::<crate::celestial::Universe>()
            .and_then(|u| u.band(system)),
    }
}

/// The kind a can of `type_id` in `system` rolls (T3-ANCIENT-RATE).
pub fn can_kind(w: &Registry, type_id: i64, system: i64) -> Kind {
    can_facts(w, type_id, system).kind
}

/// **A can was hacked**: roll its loot (seeded by the world, the can and
/// the hacker) and mint it into the can, owned by the hacker, quietly (no
/// one has the can open yet). Two plans, as `combat::npc_wreck`: the
/// stacks, then each copy alone, so its id is known for its runs.
pub fn on_won(w: &mut Registry, can_id: i64, hacker: i64) {
    let Some((p, system)) = crate::hacking::can(w, can_id) else {
        return;
    };
    w.ext_mut::<Loot>().cans.insert(can_id, hacker);
    let facts = can_facts(w, p.type_id, system);
    let mut s = seed::stream(
        w.world().seed,
        seed::Kind::Loot,
        &[STREAM_TAG, can_id as u64, hacker as u64],
    );
    let dungeon = w
        .ext::<crate::site::Sites>()
        .of_item(can_id)
        .map_or(0, |i| i.dungeon_id);
    let mut stacks = match w.content().try_get::<Table>() {
        Some(t) => {
            let known = |id| crate::item::find_in(w.content(), id).is_some();
            let mut stacks = roll(t, &facts, &mut s, known);
            for x in site_roll(t, dungeon, p.type_id, facts.band, &mut s, known) {
                match stacks
                    .iter_mut()
                    .find(|y| y.type_id == x.type_id && y.runs.is_none())
                {
                    Some(y) => y.quantity += x.quantity,
                    None => stacks.push(x),
                }
            }
            stacks
        }
        None => Vec::new(),
    };
    crate::ancient_rate::won(w, facts.kind, &stacks);
    // FIL-WIRE (#121): one filament draw per can, after the rate counters
    // (a filament is not a site's table loot).
    let archetype = match facts.kind {
        Kind::Relic | Kind::Ancient => "relic",
        Kind::Data => "data",
    };
    if let Some(f) = crate::combat::roll_filament(
        w,
        crate::filament_drops::Source::Site,
        can_id,
        archetype,
        facts.band,
    ) {
        stacks.push(Stack {
            type_id: f,
            quantity: 1,
            runs: None,
        });
    }
    let to = Slot::new(hacker, can_id, FLAG_IN_CAN);
    let plain: Vec<Edit> = stacks
        .iter()
        .filter(|x| x.runs.is_none())
        .map(|x| Edit::Create {
            type_id: x.type_id,
            quantity: x.quantity,
            to,
        })
        .collect();
    if !plain.is_empty()
        && let Err(e) = w.apply_items_quiet(&plain)
    {
        crate::outln!("[hack] the loot of can {can_id} was refused: {e:?}");
    }
    for x in stacks.iter().filter(|x| x.runs.is_some()) {
        let create = [Edit::Create {
            type_id: x.type_id,
            quantity: 1,
            to,
        }];
        match w.apply_items_quiet(&create) {
            Ok(changes) => {
                for ch in changes {
                    if let crate::item::Change::Created { item_id, .. } = ch {
                        w.set_blueprint_state(BlueprintState {
                            item_id,
                            runs_remaining: x.runs.unwrap_or(1),
                            material_efficiency: 0,
                            time_efficiency: 0,
                        });
                    }
                }
            }
            Err(e) => crate::outln!("[hack] a copy in can {can_id} was refused: {e:?}"),
        }
    }
    crate::outln!(
        "[hack] can {can_id} ({:?}, tier {}) holds {} stacks for {hacker}",
        facts.kind,
        facts.tier,
        stacks.len()
    );
}

/// A wreck to loot from at `id`: `combat`'s wreck, or a hacked can as one
/// (owner and loot rights the hacker's, position the can's). The callers
/// (`invbroker`'s listing, `fitting::plan_to_cargo`, `combat::plan_loot`)
/// then treat both alike.
pub fn loot_source(w: &Registry, id: i64) -> Option<Wreck> {
    if let Some(wr) = w.combat().wreck(id) {
        return Some(*wr);
    }
    let owner = w.ext::<Loot>().owner(id)?;
    let (p, system) = crate::hacking::can(w, id)?;
    Some(Wreck {
        wreck_id: id,
        type_id: p.type_id,
        solar_system_id: system,
        position: p.position,
        ship_id: id,
        owner_id: owner,
        corp_id: w.character(owner).map_or(owner, |c| c.corporation_id),
        fleet_id: crate::combat::fleet_of(w, owner),
        abandoned: false,
        expires_at: crate::damage::BlueTime::MAX,
    })
}

/// A loot can's radius, for `combat::plan_loot`'s range (a wreck's is the
/// wreck constant).
pub fn can_radius(w: &Registry, id: i64) -> Option<f64> {
    w.ext::<Loot>().owner(id)?;
    crate::hacking::can(w, id).map(|(p, _)| p.radius)
}

/// The can's own row for `GetSelfInvItem`: an `invCacheContainer` built
/// with the station's type would be a structure to the client (the wreck's
/// reason, P2.4).
pub fn self_item(w: &Registry, id: i64) -> Option<Item> {
    let owner = w.ext::<Loot>().owner(id)?;
    let (p, system) = crate::hacking::can(w, id)?;
    let row = type_row(w, p.type_id);
    Some(Item {
        item_id: id,
        type_id: p.type_id,
        owner_id: owner,
        location_id: system,
        flag_id: crate::combat::FLAG_IN_SPACE,
        quantity: 1,
        group_id: row.map_or(0, |r| r.group_id),
        category_id: row.map_or(0, |r| r.category_id),
        singleton: true,
    })
}

/// After a take from a loot can: without loot rights
/// (`combat::has_loot_right`, the hacker's) it is theft -- allowed, and the
/// taker a suspect where the system has suspects. A no-op for anything but
/// a loot can (a wreck's take is `combat::looted`'s).
pub fn looted(w: &mut Registry, character_id: i64, id: i64) {
    if w.combat().wreck(id).is_some() {
        return;
    }
    let Some(wr) = loot_source(w, id) else {
        return;
    };
    if !crate::combat::has_loot_right(w, character_id, &wr) {
        crate::crimewatch::theft_from(
            w,
            character_id,
            Some(wr.owner_id),
            wr.solar_system_id,
            crate::pyutil::filetime_now(),
        );
    }
}

/// Destroy what lies at `location` (and copies' runs with it), quietly.
fn clear(w: &mut Registry, locations: &BTreeSet<i64>) {
    let items: Vec<i64> = w
        .store()
        .items()
        .iter()
        .filter(|i| locations.contains(&i.location_id))
        .map(|i| i.item_id)
        .collect();
    if items.is_empty() {
        return;
    }
    let edits: Vec<Edit> = items
        .iter()
        .map(|&item_id| Edit::Destroy { item_id })
        .collect();
    match w.apply_items_quiet(&edits) {
        Ok(_) => {
            for id in items {
                if w.jobs().blueprints().get(id).is_some() {
                    w.set_blueprint_state(BlueprintState::original(id));
                }
            }
        }
        Err(e) => crate::outln!("[hack] loot left in {locations:?} was refused: {e:?}"),
    }
}

/// **The stage's tail** (`hacking::tick`): a loot can no longer in space
/// (its site vacated) takes what is left in it; the first run after a
/// start also destroys anything lying at a site-band id no loot can holds
/// -- loot from before a restart, whose cans are gone.
pub fn tick(w: &mut Registry) {
    let mut gone: BTreeSet<i64> = w
        .ext::<Loot>()
        .cans
        .keys()
        .copied()
        .filter(|id| crate::hacking::can(w, *id).is_none())
        .collect();
    if !w.ext::<Loot>().swept {
        w.ext_mut::<Loot>().swept = true;
        let live: BTreeSet<i64> = w.ext::<Loot>().cans.keys().copied().collect();
        gone.extend(w.store().items().iter().map(|i| i.location_id).filter(|l| {
            (crate::site::INSTANCE_BASE..crate::site::INSTANCE_END).contains(l) && !live.contains(l)
        }));
    }
    if gone.is_empty() {
        return;
    }
    for id in &gone {
        w.ext_mut::<Loot>().cans.remove(id);
    }
    clear(w, &gone);
}

// ---------------------------------------------------------------------------
// Tests: invented ids and rows; no [A] value appears in one.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeon_data::{Dungeon, Entry as DunEntry, Object, Room};
    use crate::population::{self, Site};
    use eve_marshal::PyValue;

    // --- the roll, on invented tables -----------------------------------

    const CORE: i64 = 990_701;
    const CORE_DEAR: i64 = 990_702;
    const BPC: i64 = 990_703;
    const SALVAGE: i64 = 990_704;
    const UNKNOWN: i64 = 990_705;
    const HULL: i64 = 990_706;
    const CAN_TYPE: i64 = 990_707;
    const FILAMENT: i64 = 990_708;

    fn table() -> Table {
        Table::parse(&[
            format!("data\tdatacore\t{CORE}\t4\t1\t3\t0\t0.0"),
            format!("data\tdatacore\t{CORE_DEAR}\t4\t1\t3\t0\t90000.0"),
            format!("data\tbpc\t{BPC}\t2\t2\t7\t1\t5000.0"),
            format!("data\tghost\t{UNKNOWN}\t50\t1\t1\t0\t1.0"),
            format!("relic\tsalvage\t{SALVAGE}\t6\t1\t5\t0\t10.0"),
            format!("data\tcan\t{CAN_TYPE}\t0\t0\t0\t0\t0.0"),
        ])
        .expect("the test rows parse")
    }

    fn known(id: i64) -> bool {
        id != UNKNOWN
    }

    fn can(kind: Kind, tier: u8, band: Option<Band>) -> Can {
        Can {
            kind,
            tier,
            equipment: (2, 5),
            band,
        }
    }

    #[test]
    fn the_table_parses_its_classes_and_its_cans() {
        let t = table();
        assert_eq!(t.entries.len(), 5);
        assert_eq!(t.kind_of(CAN_TYPE), Some(Kind::Data));
        assert_eq!(t.kind_of(CORE), None);
        assert!(Table::parse(&["data\tdatacore\t1\t4\t3\t1\t0\t0.0"]).is_err());
        assert!(Table::parse(&["oops\tdatacore\t1\t4\t1\t1\t0\t0.0"]).is_err());
        assert!(!Table::fixture().entries.is_empty());
    }

    /// Only copies in the table, so every entry is its own stack: the
    /// count stays within `[min, max]` scaled, at every tier and band.
    #[test]
    fn the_roll_count_is_within_the_equipment_range_scaled() {
        let only_copies = Table::parse(&[format!("data\tbpc\t{BPC}\t2\t1\t1\t1\t0.0")])
            .expect("the test row parses");
        for tier in 1..=4u8 {
            for band in [Some(Band::High), Some(Band::Low), Some(Band::Null), None] {
                let c = can(Kind::Data, tier, band);
                let lo = ((c.equipment.0 as f64 * scale(&c)).round() as usize).max(1);
                let hi = (c.equipment.1 as f64 * scale(&c)).round() as usize;
                for seed in 0..50u64 {
                    let n = roll(&only_copies, &c, &mut Stream::new(seed), known).len();
                    assert!((lo..=hi).contains(&n), "tier {tier} {band:?}: {n}");
                }
            }
        }
        let none = Can {
            equipment: (0, 0),
            ..can(Kind::Data, 1, None)
        };
        assert_eq!(entry_count(&none, &mut Stream::new(1)), 1, "floored at one");
    }

    #[test]
    fn the_classes_respect_the_kind() {
        let t = table();
        for seed in 0..100u64 {
            for s in roll(&t, &can(Kind::Data, 4, None), &mut Stream::new(seed), known) {
                assert_ne!(s.type_id, SALVAGE, "a data can never rolls salvage");
            }
            for s in roll(
                &t,
                &can(Kind::Relic, 4, None),
                &mut Stream::new(seed),
                known,
            ) {
                assert_eq!(s.type_id, SALVAGE);
            }
        }
        assert!(roll(&t, &can(Kind::Ancient, 1, None), &mut Stream::new(1), known).is_empty());
    }

    #[test]
    fn the_roll_is_deterministic_per_seed() {
        let t = table();
        let c = can(Kind::Data, 3, Some(Band::Low));
        let a = roll(&t, &c, &mut Stream::new(77), known);
        assert_eq!(a, roll(&t, &c, &mut Stream::new(77), known));
        let differs = (0..20u64).any(|s| roll(&t, &c, &mut Stream::new(s), known) != a);
        assert!(differs, "the seed matters");
    }

    #[test]
    fn copies_come_out_one_each_with_runs_in_the_rows_range() {
        let t = table();
        let mut seen = 0;
        for seed in 0..200u64 {
            for s in roll(&t, &can(Kind::Data, 2, None), &mut Stream::new(seed), known) {
                if s.type_id == BPC {
                    seen += 1;
                    assert_eq!(s.quantity, 1);
                    assert!(s.runs.is_some_and(|r| (2..=7).contains(&r)), "{s:?}");
                } else {
                    assert_eq!(s.runs, None);
                }
            }
        }
        assert!(seen > 0);
    }

    /// The `ghost` class has the biggest weight but only an unknown type:
    /// skipped, never rolled, no panic; a table of nothing known rolls
    /// nothing.
    #[test]
    fn an_empty_class_is_skipped() {
        let t = table();
        for seed in 0..100u64 {
            let got = roll(&t, &can(Kind::Data, 1, None), &mut Stream::new(seed), known);
            assert!(!got.is_empty());
            assert!(got.iter().all(|s| s.type_id != UNKNOWN));
        }
        let c = can(Kind::Data, 1, None);
        assert!(roll(&t, &c, &mut Stream::new(1), |_| false).is_empty());
        assert!(roll(&Table::default(), &c, &mut Stream::new(1), known).is_empty());
    }

    #[test]
    fn a_higher_tier_leans_to_the_dearer_type() {
        let t = table();
        let share = |tier: u8| {
            let (mut dear, mut all) = (0usize, 0usize);
            for seed in 0..400u64 {
                for s in roll(
                    &t,
                    &can(Kind::Data, tier, None),
                    &mut Stream::new(seed),
                    known,
                ) {
                    if s.type_id == CORE || s.type_id == CORE_DEAR {
                        all += 1;
                        dear += usize::from(s.type_id == CORE_DEAR);
                    }
                }
            }
            dear as f64 / all as f64
        };
        assert!(share(4) > share(1));
    }

    // --- the world: a site with a hacked can ------------------------------

    /// Outside the NPC id band, so neither looks like an NPC owner.
    const ALICE: i64 = 90_000_101;
    const BOB: i64 = 90_000_102;
    const ALICE_SHIP: i64 = 100_000_001;
    const BOB_SHIP: i64 = 100_000_002;
    const DUNGEON: i64 = 999_402;
    const SITE_ID: i64 = population::SITE_BASE + 7_778;
    const CAN_POS: [f64; 3] = [3_000.0, 0.0, 0.0];
    const CAN_RADIUS: f64 = 50.0;
    /// `flagCargo` [V].
    const CARGO: i64 = 5;

    /// `typedata::FIXTURE` plus invented types: two loot types, a
    /// blueprint, a hull with a hold and the can (relic loot by its
    /// `can` row, equipment 2..2, tier 1).
    fn content(loot: Table, filaments: Option<&str>) -> crate::content::Content {
        let rows = format!(
            "type\t{CORE}\t990333\t17\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\tFixture Core\n\
             type\t{CORE_DEAR}\t990333\t17\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\tFixture Dear Core\n\
             type\t{BPC}\t990105\t9\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t\t\tFixture Blueprint\n\
             type\t{SALVAGE}\t990754\t4\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t\t\tFixture Salvage\n\
             type\t{HULL}\t990025\t6\t1\t1\t1000.0\t500.0\t1000000.0\t30.0\t0\t\t\tFixture Hull\n\
             type\t{CAN_TYPE}\t990001\t2\t1\t1\t1.0\t0.0\t0.0\t{CAN_RADIUS}\t0\t\t182={}.0;456=2.0;457=2.0;1919=1.0;1927=1.0\tFixture Data Can\n\
             type\t{FILAMENT}\t{}\t17\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t{}=1.0\tFixture Filament\n",
            crate::hacking::SKILL_ARCHAEOLOGY + 1,
            crate::random_jump::GROUP_RANDOM_JUMP_KEYS,
            crate::random_jump::ATTR_MAX_SHIPS,
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = crate::content::Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        let mut dungeons = crate::dungeon_data::Table::fixture();
        dungeons.dungeons.insert(
            DUNGEON,
            Dungeon {
                id: DUNGEON,
                archetype: 45,
                rooms: vec![Room {
                    id: 7,
                    ..Room::default()
                }],
                objects: vec![Object {
                    room: 7,
                    id: 9002,
                    type_id: CAN_TYPE,
                    pos: CAN_POS,
                    radius: Some(CAN_RADIUS),
                    ..Object::default()
                }],
                ..Dungeon::default()
            },
        );
        c.insert("dungeon_data", dungeons);
        c.insert("hacking_loot", loot);
        if let Some(text) = filaments {
            c.insert(
                crate::filament_drops::TABLE.name,
                crate::filament_drops::Table::parse(text).expect("the test rows parse"),
            );
        }
        c
    }

    struct World {
        reg: Registry,
        can: i64,
        system: i64,
    }

    /// Alice and Bob in the invented hull, in a fixture system where theft
    /// flags, both 1 km off the can's surface; the can hacked by Alice.
    fn world() -> World {
        world_with(table())
    }

    /// [`world`] with another loot table.
    fn world_with(loot: Table) -> World {
        world_full(loot, None)
    }

    /// [`world_with`], and a filament drop table of the given text.
    fn world_full(loot: Table, filaments: Option<&str>) -> World {
        let pilot = |id: i64, ship: i64| crate::model::Character {
            character_id: id,
            name: format!("pilot {id}"),
            ship_id: ship,
            ship_type_id: HULL,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![pilot(ALICE, ALICE_SHIP), pilot(BOB, BOB_SHIP)],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content(loot, filaments)));
        let declared = crate::crimewatch::declared_null_systems(&reg);
        let system = reg
            .content()
            .get::<crate::celestial::Universe>()
            .systems()
            .find(|s| s.band() == Band::High && !declared.contains(&s.id))
            .map(|s| s.id)
            .expect("a fixture high-sec system");
        let mut items = reg.store().items().to_vec();
        for i in items
            .iter_mut()
            .filter(|i| i.item_id == ALICE_SHIP || i.item_id == BOB_SHIP)
        {
            i.type_id = HULL;
            i.group_id = 990_025;
            i.category_id = crate::model::CATEGORY_SHIP;
        }
        let next = reg.store().next_id();
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: next,
            ..crate::store::Snapshot::default()
        });
        let anchor = [5.0e10, 1.0e9, -2.0e10];
        let site = Site {
            system,
            row: 0,
            k: 0,
            slot: 0,
            generation: 0,
            site_id: SITE_ID,
            dungeon_id: DUNGEON,
            archetype: 45,
            entry: DunEntry::Signature,
            anchor,
            target_id: "HAK-002".into(),
            state: population::State::Live,
            at: 0,
        };
        assert!(crate::site::instantiate(&mut reg, &site));
        let can = reg
            .ext::<crate::site::Sites>()
            .get(SITE_ID)
            .and_then(|s| s.objects.iter().find(|p| p.dun_object_id == 9002))
            .map(|p| p.item_id)
            .expect("placed");
        for (id, ship) in [(ALICE, ALICE_SHIP), (BOB, BOB_SHIP)] {
            let at = [
                anchor[0] + CAN_POS[0] - CAN_RADIUS - 1_000.0,
                anchor[1],
                anchor[2],
            ];
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    id,
                    ship,
                    system,
                    crate::space_state::Motion::arrived(at, [0.0; 3]),
                ));
        }
        on_won(&mut reg, can, ALICE);
        let _ = reg.take_pending();
        World { reg, can, system }
    }

    fn inv(
        reg: &mut Registry,
        actor: i64,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
        container: i64,
    ) -> Result<PyValue, crate::service::ServiceError> {
        let bind = PyValue::Tuple(vec![PyValue::Int(container), PyValue::Int(0)]);
        reg.dispatch(&crate::service::Request {
            service: "invbroker::inventory",
            method,
            args,
            kwargs,
            bind_params: Some(&bind),
            actor: Some(actor),
        })
    }

    fn inside(reg: &Registry, can: i64) -> Vec<Item> {
        reg.store().contents_of(can).cloned().collect()
    }

    fn take(reg: &mut Registry, who: i64, ship: i64, item: i64, can: i64) {
        inv(
            reg,
            who,
            "MultiAdd",
            &[PyValue::List(vec![PyValue::Int(item)]), PyValue::Int(can)],
            // `(value, key)`, as the dispatcher hands kwargs over.
            &[(PyValue::Int(CARGO), PyValue::Bytes(b"flag".to_vec()))],
            ship,
        )
        .expect("taken");
    }

    fn suspect(reg: &Registry, who: i64) -> bool {
        reg.combat().crimewatch().phase(
            who,
            crate::crimewatch::Timer::Criminal,
            crate::pyutil::filetime_now(),
        ) != crate::crimewatch::Phase::Idle
    }

    const FIL_ROWS: &str = "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
        site\tdata\tjump\t*\t1000000\tany\tinvented\n";

    /// **A hacked data can can hold a filament** (FIL-WIRE): a `data` row
    /// that always hits puts one in the can, a `relic` row does not match a
    /// data can, and the world switch stops it.
    #[test]
    fn a_hacked_can_can_hold_a_filament() {
        let w = world_full(table(), Some(FIL_ROWS));
        let held: Vec<(i64, i64)> = inside(&w.reg, w.can)
            .iter()
            .map(|i| (i.type_id, i.quantity))
            .collect();
        assert!(held.contains(&(FILAMENT, 1)), "{held:?}");
        let relic = FIL_ROWS.replace("site\tdata", "site\trelic");
        let off = format!("{FIL_ROWS}off\t*\t-\t*\t0\t-\tinvented\n");
        for text in [relic, off] {
            let w = world_full(table(), Some(&text));
            assert!(inside(&w.reg, w.can).iter().all(|i| i.type_id != FILAMENT));
        }
    }

    /// The can's `can` row makes it a data can, tier 1, two entries in
    /// high-sec: its stacks lie in it, the hacker's, and it lists them to
    /// both pilots in the system, with its own row as its self item.
    #[test]
    fn a_hacked_can_holds_its_roll_and_lists_it() {
        let mut w = world();
        let items = inside(&w.reg, w.can);
        assert!(!items.is_empty(), "something was rolled");
        for i in &items {
            assert_eq!((i.owner_id, i.flag_id), (ALICE, FLAG_IN_CAN));
            assert!(
                [CORE, CORE_DEAR, BPC].contains(&i.type_id),
                "data loot only"
            );
        }
        for who in [ALICE, BOB] {
            match inv(&mut w.reg, who, "List", &[], &[], w.can) {
                Ok(PyValue::List(rows)) => assert_eq!(rows.len(), items.len()),
                other => panic!("a list, got {other:?}"),
            }
        }
        let me = self_item(&w.reg, w.can).expect("a self item");
        assert_eq!((me.type_id, me.owner_id), (CAN_TYPE, ALICE));
        assert_eq!(
            loot_source(&w.reg, w.can).map(|wr| wr.solar_system_id),
            Some(w.system)
        );
    }

    /// A copy rolled into the can carries its runs, so it lists as a copy
    /// (`quantity -2`, `industry::BlueprintItem::wire_quantity`).
    #[test]
    fn a_rolled_copy_is_a_copy() {
        let only_copies = Table::parse(&[
            format!("data\tbpc\t{BPC}\t2\t3\t3\t1\t0.0"),
            format!("data\tcan\t{CAN_TYPE}\t0\t0\t0\t0\t0.0"),
        ])
        .expect("the test rows parse");
        let w = world_with(only_copies);
        let copies = inside(&w.reg, w.can);
        assert!(!copies.is_empty());
        for i in copies {
            assert_eq!((i.type_id, i.quantity), (BPC, 1));
            let state = w.reg.jobs().blueprints().state_of(i.item_id);
            assert_eq!(state.runs_remaining, 3);
            let bp = crate::industry::BlueprintItem {
                item_id: i.item_id,
                type_id: i.type_id,
                owner_id: i.owner_id,
                location_id: i.location_id,
                location_type_id: 0,
                flag_id: i.flag_id,
                runs_remaining: -1,
                material_efficiency: 0,
                time_efficiency: 0,
            }
            .with_state(&state);
            assert_eq!(bp.wire_quantity(), -2);
        }
    }

    /// The hacker takes into her hold with no flag; Bob's take is theft
    /// (his safety lowered to yellow first: green refuses a theft, CW-1).
    #[test]
    fn a_take_by_another_pilot_is_theft() {
        for (who, ship, thief) in [(ALICE, ALICE_SHIP, false), (BOB, BOB_SHIP, true)] {
            let mut w = world();
            crate::crimewatch::set_safety(&mut w.reg, who, crate::crimewatch::Safety::Partial);
            let first = inside(&w.reg, w.can)[0].item_id;
            take(&mut w.reg, who, ship, first, w.can);
            let got = w.reg.store().get(first).expect("still exists");
            assert_eq!(
                (got.owner_id, got.location_id, got.flag_id),
                (who, ship, CARGO)
            );
            assert_eq!(suspect(&w.reg, who), thief, "pilot {who}");
        }
    }

    /// A vacated site takes the can's loot (and a copy's runs) with it; the
    /// first tick after a start sweeps loot whose can is gone.
    #[test]
    fn loot_leaves_with_its_can_and_a_restart_sweeps_it() {
        let mut w = world();
        tick(&mut w.reg);
        assert!(
            !inside(&w.reg, w.can).is_empty(),
            "a live can keeps its loot"
        );
        assert!(crate::site::vacate(&mut w.reg, SITE_ID));
        tick(&mut w.reg);
        assert!(inside(&w.reg, w.can).is_empty());
        assert_eq!(w.reg.ext::<Loot>().owner(w.can), None);

        let mut w = world();
        let orphan = crate::site::INSTANCE_BASE + 1;
        assert_ne!(orphan, w.can);
        w.reg
            .apply_items_quiet(&[Edit::Create {
                type_id: CORE,
                quantity: 2,
                to: Slot::new(ALICE, orphan, FLAG_IN_CAN),
            }])
            .expect("minted");
        tick(&mut w.reg);
        assert!(inside(&w.reg, orphan).is_empty(), "swept");
        assert!(!inside(&w.reg, w.can).is_empty(), "the live can is kept");
    }

    // --- CAP-LOOT: the site rows -----------------------------------------

    const OTHER_DUNGEON: i64 = 999_403;
    const OTHER_CAN: i64 = 990_708;
    const COMMODITY: i64 = 990_709;

    fn sites(rows: &[String]) -> Vec<SiteRow> {
        parse_sites(rows).expect("the test rows parse")
    }

    #[test]
    fn the_site_file_parses_and_refuses_a_bad_row() {
        let ok = sites(&[
            format!("{DUNGEON}\t0\t{COMMODITY}\t{PPM}\t1\t2\thl"),
            format!("{DUNGEON}\t{CAN_TYPE}\t{COMMODITY}\t1\t1\t1\t"),
        ]);
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].qty, (1, 2));
        for bad in [
            format!("{DUNGEON}\t0\t{COMMODITY}\t0\t1\t1\t"),
            format!("{DUNGEON}\t0\t{COMMODITY}\t{}\t1\t1\t", PPM + 1),
            format!("{DUNGEON}\t0\t{COMMODITY}\t5\t0\t1\t"),
            format!("{DUNGEON}\t0\t{COMMODITY}\t5\t3\t1\t"),
            format!("{DUNGEON}\t0\t{COMMODITY}\t5\t1\t1\tx"),
            format!("{DUNGEON}\t0\t{COMMODITY}\t5\t1\t1"),
            format!("{DUNGEON}\tx\t{COMMODITY}\t5\t1\t1\t"),
        ] {
            assert!(parse_sites(&[bad.as_str()]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_site_row_applies_by_dungeon_can_type_and_band() {
        let t = Table {
            sites: sites(&[
                format!("{DUNGEON}\t{CAN_TYPE}\t{COMMODITY}\t{PPM}\t2\t2\t"),
                format!("{DUNGEON}\t0\t{CORE}\t{PPM}\t1\t1\thl"),
            ]),
            ..Table::default()
        };
        let roll_in = |d, c, b| {
            let mut got = site_roll(&t, d, c, b, &mut Stream::new(7), |_| true);
            got.sort_by_key(|x| x.type_id);
            got.iter()
                .map(|x| (x.type_id, x.quantity))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            roll_in(DUNGEON, CAN_TYPE, Some(Band::High)),
            vec![(CORE, 1), (COMMODITY, 2)]
        );
        assert_eq!(
            roll_in(DUNGEON, OTHER_CAN, Some(Band::Low)),
            vec![(CORE, 1)]
        );
        assert_eq!(roll_in(DUNGEON, OTHER_CAN, Some(Band::Null)), vec![]);
        assert_eq!(
            roll_in(DUNGEON, CAN_TYPE, Some(Band::Wormhole)),
            vec![(COMMODITY, 2)]
        );
        assert_eq!(roll_in(OTHER_DUNGEON, CAN_TYPE, None), vec![]);
        let unknown = site_roll(&t, DUNGEON, CAN_TYPE, None, &mut Stream::new(7), |id| {
            id != COMMODITY
        });
        assert_eq!(unknown.len(), 1, "a type the server cannot mint is skipped");
    }

    #[test]
    fn a_site_chance_is_a_draw_in_parts_per_million() {
        let one_in_a_million = Table {
            sites: sites(&[format!("{DUNGEON}\t0\t{COMMODITY}\t1\t1\t1\t")]),
            ..Table::default()
        };
        let hits = (0..500u64)
            .filter(|n| {
                !site_roll(
                    &one_in_a_million,
                    DUNGEON,
                    CAN_TYPE,
                    None,
                    &mut Stream::new(*n),
                    |_| true,
                )
                .is_empty()
            })
            .count();
        assert!(hits < 5, "a one-in-a-million row almost never hits: {hits}");
    }

    /// A hacked can of the site's dungeon holds the always-row's item on
    /// top of its class roll; a row of another dungeon or can type adds
    /// nothing.
    #[test]
    fn a_hacked_can_holds_its_sites_commodity() {
        let mut loot = table();
        loot.sites = sites(&[format!("{DUNGEON}\t{CAN_TYPE}\t{CORE}\t{PPM}\t4\t4\t")]);
        let w = world_with(loot);
        let got: i64 = inside(&w.reg, w.can)
            .iter()
            .filter(|i| i.type_id == CORE)
            .map(|i| i.quantity)
            .sum();
        assert!(got >= 4, "the site's 4 cores are in the can: {got}");

        let mut elsewhere = table();
        elsewhere.sites = sites(&[
            format!("{OTHER_DUNGEON}\t0\t{SALVAGE}\t{PPM}\t1\t1\t"),
            format!("{DUNGEON}\t{OTHER_CAN}\t{SALVAGE}\t{PPM}\t1\t1\t"),
            format!("{DUNGEON}\t0\t{SALVAGE}\t{PPM}\t1\t1\tl"),
        ]);
        let w = world_with(elsewhere);
        assert!(
            inside(&w.reg, w.can).iter().all(|i| i.type_id != SALVAGE),
            "no row of this dungeon, can type and high-sec band"
        );
    }
}
