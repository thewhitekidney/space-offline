//! **Geodes**: a rare crate that a mining cut may leave in the hold, and what
//! is inside when it is opened (lane l6a-geodes, issue #32; spec
//! `handback/r-nes-econ/report.md` §2.6 A).
//!
//! Two rows of the module, each a seam the core gave for it:
//!
//! - [`on_mined`] (`Module::mined`, `eve_server::mining_hooks`): after a cut
//!   that took ore, [`tunables::GEODE_CHANCE`] (times [`tunables::GEODE_CRIT_MULT`]
//!   on a crit) makes one geode **item** in the miner's ship's cargo hold, if
//!   it has room. It is an item: hauled, and lost with the ship by the loss
//!   rules; nothing is credited anywhere else.
//! - [`open`] (`Module::crates`, `eve_server::crate_hooks`): `crateService`
//!   is the core's (`service/crate_svc.rs`: opens only docked, one crate per
//!   open); this makes the prizes in the pilot's station hangar. A geode is
//!   opened only docked **because the core says so** (`GetCrateLocation`).
//!
//! **Which crate types are geodes** is not in this repository. The client's
//! `crates.fsdbinary` has them; `tools/content_geodes.py` picks, by the
//! owner's name patterns in `content/econ/geode-picks.tsv`, which become
//! `mining/geodes.tsv` in the bundle (`#type_id\ttier`). A bundle with none
//! has no geodes and mining is unchanged. A **tier** (1 to 3) makes a
//! geode hold more prizes and bigger ones ([`tunables::PRIZES_BY_TIER`]).
//!
//! **What is inside** is roles, not ids (`content/econ/geode-loot.tsv`), and
//! each role is resolved at run time by a rule over the bundle ([`prize`]):
//! the materials of an ore, of a dear ore, PLEX, an injector, a mining-hull
//! SKIN licence, a mining crystal.
//!
//! **Nothing from nowhere.** Every unit a geode makes is counted in the
//! module's own ledger rows (`geode:crate:<tier>` for a geode made and
//! opened, `geode:prize:<role>` for what it paid, in units), and also where
//! the core already counts it: PLEX in the `nes` ledger under `geode`,
//! injectors in `sptrade` under `geode` and against the day's point budget,
//! items in the goods ledger (`Registry::apply_items`).
//!
//! **Deterministic**: every draw is a function of the world seed and the
//! event (the cut, or the opened crate and the world's count of opens).
//!
//! **Not here**: motherlodes and rich sites multiply the chance
//! (`r-nes-econ` §2.6 C-E, `motherlode`); [`on_mined`] takes the multiplier
//! from [`multiplier`].

use crate::KIND;
use crate::tunables::{self, Role};
use eve_server::content::{Boxed, Origin, Sources, TableSpec};
use eve_server::crate_hooks::{Loot, OpenEvent};
use eve_server::item::{Change, Edit, Slot};
use eve_server::module::{Deltas, MinedEvent};
use eve_server::nes_vault;
use eve_server::seed::{self, Kind, Stream};
use eve_server::service::Registry;
use eve_server::skill_trading::{self, BUCKET, DIVISOR, TYPE_LARGE, TYPE_SMALL};

/// One day of blue time (100 ns ticks).
const DAY_TICKS: i64 = 864_000_000_000;

/// Mixed into the cut's stream, and into an open's, so these draws are this
/// module's own.
const SALT_DROP: u64 = 0x4745_4f44;
const SALT_OPEN: u64 = 0x4745_4f4f;

/// `categoryAsteroid`, ore and its compressed forms [V].
const CATEGORY_ASTEROID: i64 = 25;
/// `categoryCharge` [V].
const CATEGORY_CHARGE: i64 = 8;
/// `chargeGroup1` [V `modules.rs`]: a module that carries it takes charges.
const ATTR_CHARGE_GROUP1: i64 = 604;
/// The mining laser's effect [V `mining::EFFECT_MINING`].
const EFFECT_MINING: i64 = eve_server::mining::EFFECT_MINING;

// ---- the table ---------------------------------------------------------------

/// The bundle's geode file and its header.
pub const TYPES_FILE: &str = "mining/geodes.tsv";
pub const TYPES_HEADER: &str = "#type_id\ttier";
/// The authored loot weights and their header (`schema.md` §4).
pub const LOOT_FILE: &str = "econ/geode-loot.tsv";
pub const LOOT_HEADER: &str = "role\tweight\twhy";
const SHIPPED_LOOT: &str = include_str!("../../../../content/econ/geode-loot.tsv");

/// Which crate types are geodes, and what they hold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Geodes {
    /// `(type_id, tier)`, ascending by tier then id.
    types: Vec<(i64, usize)>,
    /// `(role, weight)`, in file order.
    loot: Vec<(Role, f64)>,
}

impl Geodes {
    /// Build from the bundle's lines and the loot rows. A tier outside 1 to 3
    /// is an error naming the line.
    pub fn parse<S: AsRef<str>>(
        type_lines: &[S],
        loot: Vec<(Role, f64)>,
    ) -> Result<Geodes, String> {
        let mut types = Vec::new();
        for (i, l) in type_lines.iter().enumerate() {
            let f: Vec<&str> = l.as_ref().split('\t').collect();
            let (Some(id), Some(tier)) = (
                f.first().and_then(|s| s.parse::<i64>().ok()),
                f.get(1).and_then(|s| s.parse::<usize>().ok()),
            ) else {
                return Err(format!(
                    "{TYPES_FILE} data line {}: {:?}",
                    i + 1,
                    l.as_ref()
                ));
            };
            if !(1..=tunables::PRIZES_BY_TIER.len()).contains(&tier) {
                return Err(format!(
                    "{TYPES_FILE} data line {}: tier {tier} is not 1 to {}",
                    i + 1,
                    tunables::PRIZES_BY_TIER.len()
                ));
            }
            types.push((id, tier));
        }
        types.sort_by_key(|(id, tier)| (*tier, *id));
        types.dedup_by_key(|(id, _)| *id);
        Ok(Geodes { types, loot })
    }

    /// The tier of crate type `type_id`, if it is a geode.
    pub fn tier_of(&self, type_id: i64) -> Option<usize> {
        self.types.iter().find(|t| t.0 == type_id).map(|t| t.1)
    }

    /// The crate type to make for tier `want`: the first of that tier, else
    /// of the nearest tier below, else of the lowest there is.
    pub fn type_for(&self, want: usize) -> Option<(i64, usize)> {
        let below = self
            .types
            .iter()
            .rev()
            .find(|(_, t)| *t <= want)
            .map(|(id, t)| (*id, *t));
        let first_of = |tier: usize| self.types.iter().find(|(_, t)| *t == tier).copied();
        below
            .and_then(|(_, t)| first_of(t))
            .or(self.types.first().copied())
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }
}

/// The registration row: one line in the module's `tables`.
pub const TABLE: TableSpec = TableSpec {
    name: "geodes",
    load,
    fixture: || Box::new(Geodes::default()),
};

fn load(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(LOOT_FILE, SHIPPED_LOOT, LOOT_HEADER)?;
    let loot = file.parse_rows(|r| {
        let role = Role::parse(r.field(0)).ok_or_else(|| format!("no role {:?}", r.field(0)))?;
        let w = r.f64(1)?;
        if w.is_nan() || w < 0.0 {
            return Err(format!("weight {w} is negative"));
        }
        Ok((role, w))
    });
    let lines = match src.bundle {
        Some(b) => {
            let lines = b.data_lines(TYPES_FILE, TYPES_HEADER)?;
            if !lines.is_empty() {
                src.note(Origin::Bundle);
            }
            lines
        }
        None => Vec::new(),
    };
    Ok(Box::new(Geodes::parse(&lines, loot)?))
}

pub(crate) fn geodes(reg: &Registry) -> Option<&Geodes> {
    reg.content().try_get::<Geodes>()
}

/// The crate row's test: is `type_id` a geode.
pub fn is_crate(reg: &Registry, type_id: i64) -> bool {
    geodes(reg).is_some_and(|g| g.tier_of(type_id).is_some())
}

// ---- the ledger --------------------------------------------------------------

fn ledger_key(name: &str) -> Vec<String> {
    vec!["ledger".to_owned(), name.to_owned()]
}

/// `(made, spent)` of ledger row `name`.
pub fn ledger_of(d: &Deltas, name: &str) -> (i64, i64) {
    let Some(v) = d.of(KIND).get(&ledger_key(name)) else {
        return (0, 0);
    };
    let n = |i: usize| v.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
    (n(0), n(1))
}

pub(crate) fn ledger_add(d: &mut Deltas, name: &str, made: i64, spent: i64) {
    let (m, s) = ledger_of(d, name);
    d.put(
        KIND,
        ledger_key(name),
        vec![(m + made).to_string(), (s + spent).to_string()],
    );
}

fn opens_key() -> Vec<String> {
    vec!["opens".to_owned()]
}

/// Crates opened in this world so far; mixed into each open's draw so two
/// opens of one stack differ and a replay opens the same.
pub fn opens(d: &Deltas) -> u64 {
    d.of(KIND)
        .get(&opens_key())
        .and_then(|v| v.first())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

// ---- the drop ----------------------------------------------------------------

/// The chance multiplier for where the cut happened: a motherlode or a rich
/// site raises it (`motherlode::multiplier`, lane l6b-mining-events).
pub fn multiplier(reg: &Registry, ev: &MinedEvent) -> f64 {
    crate::motherlode::multiplier(reg, ev)
}

/// The chance a cut leaves a geode.
pub fn chance(crit: bool, bot: bool, mult: f64) -> f64 {
    let mut p = tunables::GEODE_CHANCE * mult;
    if crit {
        p *= tunables::GEODE_CRIT_MULT;
    }
    if bot {
        p *= tunables::BOT_GEODE_MULT;
    }
    p.clamp(0.0, 1.0)
}

/// A weighted pick among `weights` (index), by `s`. Always draws once, so
/// the stream's use does not depend on the outcome. `None` when every
/// weight is 0.
pub fn weighted(s: &mut Stream, weights: &[f64]) -> Option<usize> {
    let total: f64 = weights.iter().sum();
    let x = s.unit() * total;
    if total <= 0.0 {
        return None;
    }
    let mut acc = 0.0;
    for (i, w) in weights.iter().enumerate() {
        acc += w;
        if x < acc && *w > 0.0 {
            return Some(i);
        }
    }
    weights.iter().rposition(|w| *w > 0.0)
}

/// Which tier a dropped geode is (1 to 3).
pub fn tier_roll(s: &mut Stream, crit: bool) -> usize {
    let w = if crit {
        &tunables::TIER_WEIGHTS_CRIT
    } else {
        &tunables::TIER_WEIGHTS
    };
    weighted(s, w).map_or(1, |i| i + 1)
}

/// The mining-hook row: after a cut that took ore, maybe leave a geode in the
/// ship's cargo hold.
pub fn on_mined(reg: &mut Registry, ev: &MinedEvent) {
    let Some(g) = geodes(reg) else { return };
    if g.is_empty() {
        return;
    }
    let Some(bot) = reg.character(ev.miner).map(|c| c.is_bot) else {
        return;
    };
    let crit = ev.crit > 0;
    let mut s = seed::stream(
        reg.world().seed,
        Kind::Loot,
        &[
            SALT_DROP,
            ev.rock_id as u64,
            ev.source_id as u64,
            ev.miner as u64,
            ev.at as u64,
        ],
    );
    // Both draws first: the stream's use does not depend on the outcome.
    let hit = s.chance(chance(crit, bot, multiplier(reg, ev)));
    let tier = tier_roll(&mut s, crit);
    if !hit {
        return;
    }
    let Some((type_id, tier)) = g.type_for(tier) else {
        return;
    };
    // It needs room: a full hold loses the geode, which was never made.
    let volume =
        eve_server::item::find_in(reg.content(), type_id).map_or(f64::INFINITY, |t| t.volume);
    if eve_server::mining::cargo_free(reg, ev.ship_id, ev.ship_type_id) < volume {
        return;
    }
    let to = Slot::new(ev.miner, ev.ship_id, eve_server::fitting::FLAG_CARGO);
    if give(reg, to, type_id, 1).is_none() {
        return;
    }
    ledger_add(reg.deltas_mut(), &format!("geode:crate:{tier}"), 1, 0);
    eve_server::outln!(
        "[econ-mod] geode: tier {tier} for {} (crit {crit}, system {})",
        ev.miner,
        ev.system
    );
}

/// Make `qty` of `type_id` at `to`, growing a stack that is already there.
/// The item that now holds them, or `None` if the store refused (nothing was
/// made).
pub(crate) fn give(reg: &mut Registry, to: Slot, type_id: i64, qty: i64) -> Option<i64> {
    if qty < 1 {
        return None;
    }
    let stack = reg
        .store()
        .at(to)
        .filter(|i| i.type_id == type_id && !i.singleton)
        .map(|i| i.item_id)
        .min();
    let edit = match stack {
        Some(item_id) => Edit::Grow {
            item_id,
            quantity: qty,
        },
        None => Edit::Create {
            type_id,
            quantity: qty,
            to,
        },
    };
    match reg.apply_items(&[edit]).ok()?.first()? {
        Change::Created { item_id, .. } | Change::QuantityChanged { item_id, .. } => Some(*item_id),
        _ => None,
    }
}

// ---- the open ----------------------------------------------------------------

fn scaled(base: i64, tier: usize) -> i64 {
    let m = tunables::QTY_BY_TIER[tier.clamp(1, 3) - 1];
    ((base as f64 * m).round() as i64).max(1)
}

/// The asteroid-category ores the reprocessing table knows, ascending by id:
/// the raw ones, never a compressed form.
fn ores(reg: &Registry) -> Vec<i64> {
    let table = reg.content().get::<eve_server::reprocess::Table>();
    table
        .reprocessable_types()
        .filter(|t| {
            eve_server::item::find_in(reg.content(), *t)
                .is_some_and(|i| i.category_id == CATEGORY_ASTEROID)
                && !table.is_compressed_type(*t)
                && !table.materials(*t).is_empty()
        })
        .collect()
}

/// The dear end of `ores`, by price, dearest first.
pub fn dear(reg: &Registry, mut ores: Vec<i64>) -> Vec<i64> {
    let table = reg.content().get::<eve_server::reprocess::Table>();
    ores.sort_by(|a, b| {
        table
            .price(*b)
            .partial_cmp(&table.price(*a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(b))
    });
    let keep = ((ores.len() as f64 * tunables::RARE_SHARE).ceil() as usize).max(1);
    ores.truncate(keep);
    ores
}

/// Make the materials of `cycles` cycles of `ore`; each is a prize.
fn refine(reg: &mut Registry, ev: &OpenEvent, ore: i64, cycles: i64, tier: usize) -> Vec<Loot> {
    let table = reg.content().get::<eve_server::reprocess::Table>();
    let portions = (cycles * tunables::CYCLE_UNITS / table.portion_size(ore).max(1)).max(1);
    let mats: Vec<(i64, i64)> = table.materials(ore).to_vec();
    let mut out = Vec::new();
    for (mat, per) in mats {
        let qty = scaled(
            (per as f64 * portions as f64 * tunables::REFINE_RATE).floor() as i64,
            tier,
        );
        let to = Slot::new(ev.character, ev.station, eve_server::model::FLAG_HANGAR);
        if let Some(item_id) = give(reg, to, mat, qty) {
            out.push(Loot {
                item_id,
                type_id: mat,
                quantity: qty,
            });
        }
    }
    out
}

/// One prize of `role` for the open `ev`, drawn from `s`. Empty when the
/// bundle has nothing for that role (a prize that cannot be made is not
/// paid). Every unit made is ledgered before this returns.
pub fn prize(
    reg: &mut Registry,
    ev: &OpenEvent,
    s: &mut Stream,
    role: Role,
    tier: usize,
) -> Vec<Loot> {
    let hangar = Slot::new(ev.character, ev.station, eve_server::model::FLAG_HANGAR);
    let made: Vec<Loot> = match role {
        Role::Minerals | Role::Rare => {
            let all = ores(reg);
            let pool = if role == Role::Rare {
                dear(reg, all)
            } else {
                all
            };
            let pick = s.range(0, pool.len() as u64) as usize;
            let cycles = s.range(tunables::CYCLES.0, tunables::CYCLES.1 + 1) as i64;
            match pool.get(pick).copied() {
                Some(ore) => refine(reg, ev, ore, cycles, tier),
                None => Vec::new(),
            }
        }
        Role::Plex => {
            let qty = scaled(
                s.range(tunables::GEODE_PLEX.0, tunables::GEODE_PLEX.1 + 1) as i64,
                tier,
            );
            let user = nes_vault::key_of(reg, ev.character);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            match nes_vault::credit(reg.deltas_mut(), user, qty, "geode", now) {
                Ok(entry) => {
                    nes_vault::announce(reg, user, &entry);
                    crate::drops::note_minted(reg, day_of(ev.when), qty);
                    vec![Loot {
                        item_id: 0,
                        type_id: nes_vault::TYPE_PLEX,
                        quantity: qty,
                    }]
                }
                Err(_) => Vec::new(),
            }
        }
        Role::InjectorSmall | Role::InjectorLarge => {
            let ty = if role == Role::InjectorLarge {
                TYPE_LARGE
            } else {
                TYPE_SMALL
            };
            match give(reg, hangar, ty, 1) {
                Some(item_id) => {
                    let points = if ty == TYPE_SMALL {
                        BUCKET / DIVISOR
                    } else {
                        BUCKET
                    };
                    skill_trading::ledger_minted(reg.deltas_mut(), "geode", points);
                    crate::injectors::note_minted(reg, day_of(ev.when), points);
                    vec![Loot {
                        item_id,
                        type_id: ty,
                        quantity: 1,
                    }]
                }
                None => Vec::new(),
            }
        }
        Role::Skin => {
            let pool = skin_licences(reg);
            let pick = s.range(0, pool.len() as u64) as usize;
            pool.get(pick)
                .and_then(|t| give(reg, hangar, *t, 1).map(|id| (id, *t)))
                .map(|(item_id, type_id)| {
                    vec![Loot {
                        item_id,
                        type_id,
                        quantity: 1,
                    }]
                })
                .unwrap_or_default()
        }
        Role::Crystal => {
            let pool = crystals(reg);
            let pick = s.range(0, pool.len() as u64) as usize;
            let qty = scaled(s.range(1, 4) as i64, tier);
            pool.get(pick)
                .and_then(|t| give(reg, hangar, *t, qty).map(|id| (id, *t)))
                .map(|(item_id, type_id)| {
                    vec![Loot {
                        item_id,
                        type_id,
                        quantity: qty,
                    }]
                })
                .unwrap_or_default()
        }
    };
    let units: i64 = made.iter().map(|l| l.quantity).sum();
    if units > 0 {
        ledger_add(
            reg.deltas_mut(),
            &format!("geode:prize:{}", role.name()),
            units,
            0,
        );
    }
    made
}

/// The permanent SKIN licences that fit a mining hull, ascending by id.
fn skin_licences(reg: &Registry) -> Vec<i64> {
    let skins = reg.content().get::<eve_server::skins::Skins>();
    let mut out: Vec<i64> = skins
        .item_types()
        .iter()
        .map(|t| t.type_id)
        .filter(|t| {
            let Some(lic) = skins.licence(*t) else {
                return false;
            };
            lic.duration < 0
                && skins.skin(lic.skin_id).is_some_and(|s| {
                    s.hulls.iter().any(|h| {
                        eve_server::item::find_in(reg.content(), *h)
                            .is_some_and(|i| tunables::SKIN_HULL_GROUPS.contains(&i.group_id))
                    })
                })
        })
        .collect();
    out.sort_unstable();
    out
}

/// The mining crystals: published charges of a group some mining laser takes
/// (`chargeGroup1`), ascending by id.
fn crystals(reg: &Registry) -> Vec<i64> {
    let table = eve_server::typedata::installed_in(reg.content()).table();
    let mut groups: Vec<i64> = table
        .types()
        .filter(|t| t.has_effect(EFFECT_MINING))
        .filter_map(|t| t.attribute(ATTR_CHARGE_GROUP1))
        .map(|g| g as i64)
        .collect();
    groups.sort_unstable();
    groups.dedup();
    let mut out: Vec<i64> = table
        .types()
        .filter(|t| t.published && t.category_id == CATEGORY_CHARGE && groups.contains(&t.group_id))
        .map(|t| t.type_id)
        .collect();
    out.sort_unstable();
    out
}

/// The crate row's open: make the prizes of one geode.
pub fn open(reg: &mut Registry, ev: &OpenEvent) -> Vec<Loot> {
    let Some(g) = geodes(reg) else {
        return Vec::new();
    };
    let Some(tier) = g.tier_of(ev.crate_type) else {
        return Vec::new();
    };
    let mut weights: Vec<(Role, f64)> = g.loot.clone();
    let n = opens(reg.deltas());
    reg.deltas_mut()
        .put(KIND, opens_key(), vec![(n + 1).to_string()]);
    ledger_add(reg.deltas_mut(), &format!("geode:crate:{tier}"), 0, 1);
    // Past a day's PLEX or point budget those prizes become rarer, as for a
    // drop: the weight times the over-budget multiplier.
    let day = day_of(ev.when);
    if crate::drops::minted_on(reg.deltas(), day) >= tunables::DAILY_BUDGET {
        scale_role(&mut weights, Role::Plex);
    }
    if crate::injectors::minted_on(reg.deltas(), day) >= tunables::DAILY_SP_BUDGET {
        scale_role(&mut weights, Role::InjectorSmall);
        scale_role(&mut weights, Role::InjectorLarge);
    }
    let mut s = seed::stream(
        reg.world().seed,
        Kind::Loot,
        &[
            SALT_OPEN,
            ev.character as u64,
            ev.crate_item as u64,
            ev.nth,
            n,
        ],
    );
    let w: Vec<f64> = weights.iter().map(|(_, w)| *w).collect();
    let mut out = Vec::new();
    for _ in 0..tunables::PRIZES_BY_TIER[tier - 1] {
        let Some(i) = weighted(&mut s, &w) else { break };
        out.extend(prize(reg, ev, &mut s, weights[i].0, tier));
    }
    eve_server::outln!(
        "[econ-mod] geode: tier {tier} opened by {}, {} prizes",
        ev.character,
        out.len()
    );
    out
}

fn scale_role(weights: &mut [(Role, f64)], role: Role) {
    for (r, w) in weights.iter_mut() {
        if *r == role {
            *w *= tunables::OVER_BUDGET_MULT;
        }
    }
}

/// The day number of a blue time.
pub fn day_of(when: i64) -> i64 {
    when.div_euclid(DAY_TICKS)
}

// ---- /geode ------------------------------------------------------------------

/// `/geode give [tier]`: a geode of `tier` (default 1) in the caller's cargo
/// hold, ledgered like a drop (a GM tool for the client scenario; the gate is
/// the core's for every slash command).
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<eve_marshal::PyValue, eve_server::service::ServiceError> {
    let say = |t: &str| Ok(eve_marshal::PyValue::Bytes(t.as_bytes().to_vec()));
    let ["give", rest @ ..] = args else {
        return say("usage: /geode give [tier]");
    };
    let Some(actor) = actor else {
        return say("no character");
    };
    let want = rest.first().and_then(|t| t.parse().ok()).unwrap_or(1);
    let Some((type_id, tier)) = geodes(reg).and_then(|g| g.type_for(want)) else {
        return say("this world has no geode crate type (mining/geodes.tsv is empty)");
    };
    let Some(c) = reg.character(actor) else {
        return say("no character");
    };
    let ship = c.ship_id;
    let to = Slot::new(actor, ship, eve_server::fitting::FLAG_CARGO);
    if give(reg, to, type_id, 1).is_none() {
        return say("the store refused the geode");
    }
    ledger_add(reg.deltas_mut(), &format!("geode:crate:{tier}"), 1, 0);
    say(&format!("A tier {tier} geode is in your cargo hold."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_server::authored::Dir;

    // Invented ids, not the client's.
    const T1: i64 = 990_701;
    const T2: i64 = 990_702;
    const T3: i64 = 990_703;

    fn table(lines: &[&str]) -> Geodes {
        Geodes::parse(lines, vec![(Role::Plex, 1.0)]).expect("parses")
    }

    #[test]
    fn the_shipped_loot_table_loads_every_role_once() {
        let dir = Dir::compiled_only();
        let src = Sources::new(None, &dir);
        let b = (TABLE.load)(&src).expect("loads");
        let g = b.downcast_ref::<Geodes>().expect("a Geodes");
        for role in Role::ALL {
            assert_eq!(
                g.loot.iter().filter(|(r, _)| *r == role).count(),
                1,
                "{role:?}"
            );
        }
        assert!(g.loot.iter().all(|(_, w)| *w > 0.0));
        assert!(g.is_empty(), "no bundle, no geode type");
    }

    #[test]
    fn the_types_file_parses_and_a_bad_tier_names_its_line() {
        let g = table(&[
            &format!("{T2}\t2"),
            &format!("{T1}\t1"),
            &format!("{T3}\t3"),
        ]);
        assert_eq!(g.tier_of(T1), Some(1));
        assert_eq!(g.tier_of(T3), Some(3));
        assert_eq!(g.tier_of(5), None);
        let bad = Geodes::parse(&[format!("{T1}\t4")], vec![]).unwrap_err();
        assert!(bad.contains("line 1") && bad.contains("tier 4"), "{bad}");
        assert!(Geodes::parse(&["x\t1"], vec![]).is_err());
    }

    #[test]
    fn a_tier_with_no_type_falls_to_the_nearest_below_else_the_lowest() {
        let g = table(&[&format!("{T1}\t1"), &format!("{T3}\t3")]);
        assert_eq!(g.type_for(1), Some((T1, 1)));
        assert_eq!(g.type_for(2), Some((T1, 1)));
        assert_eq!(g.type_for(3), Some((T3, 3)));
        let only3 = table(&[&format!("{T3}\t3")]);
        assert_eq!(only3.type_for(1), Some((T3, 3)));
        assert_eq!(table(&[]).type_for(1), None);
    }

    #[test]
    fn the_chance_is_rare_ten_times_on_a_crit_and_half_for_a_bot() {
        let base = chance(false, false, 1.0);
        assert!(base > 0.0 && base < 0.05);
        assert!((chance(true, false, 1.0) / base - tunables::GEODE_CRIT_MULT).abs() < 1e-9);
        assert!((chance(false, true, 1.0) / base - tunables::BOT_GEODE_MULT).abs() < 1e-9);
        assert!((chance(false, false, 5.0) / base - 5.0).abs() < 1e-9);
        assert_eq!(chance(true, false, 1e9), 1.0);
    }

    #[test]
    fn a_drop_roll_is_deterministic_and_its_rate_is_the_chance() {
        let hits = |crit| {
            (0..20_000u64)
                .filter(|i| Stream::new(*i).chance(chance(crit, false, 1.0)))
                .count() as f64
        };
        let (plain, crit) = (hits(false), hits(true));
        // 20000 draws at 0.002 and 0.02: wide bands, the stream is fixed.
        assert!((10.0..90.0).contains(&plain), "{plain}");
        assert!((250.0..600.0).contains(&crit), "{crit}");
        for i in 0..50u64 {
            assert_eq!(
                tier_roll(&mut Stream::new(i), false),
                tier_roll(&mut Stream::new(i), false)
            );
        }
    }

    #[test]
    fn a_crit_finds_better_geodes() {
        let mean = |crit| {
            (0..4_000u64)
                .map(|i| tier_roll(&mut Stream::new(i), crit) as f64)
                .sum::<f64>()
                / 4_000.0
        };
        assert!(
            mean(true) > mean(false) + 0.2,
            "{} {}",
            mean(true),
            mean(false)
        );
        assert!((1..=3).contains(&tier_roll(&mut Stream::new(1), false)));
    }

    #[test]
    fn a_weighted_pick_follows_the_weights_and_always_draws_once() {
        let w = [70.0, 0.0, 30.0];
        let mut n = [0usize; 3];
        for i in 0..5_000u64 {
            n[weighted(&mut Stream::new(i), &w).expect("a pick")] += 1;
        }
        assert_eq!(n[1], 0, "a zero weight is never drawn");
        assert!((3_300..3_700).contains(&n[0]), "{n:?}");
        let mut a = Stream::new(3);
        let mut b = Stream::new(3);
        let _ = weighted(&mut a, &w);
        let _ = weighted(&mut b, &[0.0, 0.0]);
        assert_eq!(a.next(), b.next());
        assert_eq!(weighted(&mut Stream::new(1), &[0.0]), None);
    }

    #[test]
    fn an_open_roll_is_the_same_for_the_same_event_and_a_stack_differs() {
        let draw = |nth: u64, opens: u64| {
            let mut s = seed::stream(7, Kind::Loot, &[SALT_OPEN, 1, 2, nth, opens]);
            weighted(&mut s, &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0])
        };
        assert_eq!(draw(0, 0), draw(0, 0));
        let spread: std::collections::BTreeSet<_> = (0..30).map(|n| draw(n, n)).collect();
        assert!(spread.len() > 2, "{spread:?}");
    }

    #[test]
    fn the_budget_scale_cuts_only_the_roles_it_names() {
        let mut w = vec![(Role::Plex, 6.0), (Role::Minerals, 70.0)];
        scale_role(&mut w, Role::Plex);
        assert_eq!(w[0].1, 6.0 * tunables::OVER_BUDGET_MULT);
        assert_eq!(w[1].1, 70.0);
    }

    #[test]
    fn every_tunable_table_agrees_on_three_tiers() {
        assert_eq!(tunables::TIER_WEIGHTS.len(), 3);
        assert_eq!(tunables::TIER_WEIGHTS_CRIT.len(), 3);
        assert_eq!(tunables::PRIZES_BY_TIER.len(), 3);
        assert_eq!(tunables::QTY_BY_TIER.len(), 3);
        assert!(tunables::PRIZES_BY_TIER.iter().all(|n| *n >= 1));
        assert_eq!(scaled(10, 1), 10);
        assert!(scaled(10, 3) > scaled(10, 1));
        assert!(Role::parse("minerals") == Some(Role::Minerals) && Role::parse("x").is_none());
        for r in Role::ALL {
            assert_eq!(Role::parse(r.name()), Some(r));
        }
    }

    #[test]
    fn a_ledger_row_counts_what_was_made_and_spent() {
        // The module's own kind must be in the plan; no other test in this
        // crate reads the plan, so the install is ours.
        eve_server::module::install(&[&crate::MODULE]).expect("the plan is ours");
        let mut d = Deltas::default();
        assert_eq!(ledger_of(&d, "geode:crate:1"), (0, 0));
        ledger_add(&mut d, "geode:crate:1", 3, 0);
        ledger_add(&mut d, "geode:crate:1", 0, 2);
        assert_eq!(ledger_of(&d, "geode:crate:1"), (3, 2));
        assert_eq!(opens(&d), 0);
        assert_eq!(day_of(DAY_TICKS * 5 + 3), 5);
    }
}
