//! Moon belts: the stand-in for moon mining without Upwell structures
//! (R-MOON packet MN-C; the composition is `crate::moon`, MN-B).
//!
//! Spec: `docs/lanes/mn-b.md`.
//!
//! ## Why a belt and not a chunk
//!
//! Retail makes moon ore with a structure: a corporation anchors an Athanor,
//! schedules an extraction, and a chunk fractures. That flow is piloted from
//! inside the structure as the ship (R-MOON §0, `moonExtractions`), so it
//! cannot be faked without the Upwell stack, which the owner deferred. A moon
//! ore rock itself needs none of it: it is an ordinary asteroid ball of a
//! type in the five moon ore groups, mined by any laser with Mining 1
//! (`requiredSkill1` is 3386 on every one of them **[V-data]**), and the
//! client draws it from `groupgraphics` like any rock (`Asteroid.
//! _GetGraphicID`, `spaceObject/asteroid.py:13-26`, which picks by
//! `self.id % len(graphics)`, so any id draws) **[V]**.
//!
//! So the server spawns **time-limited rock fields round a moon**: a
//! *moon belt*. Nothing is made from nowhere: a belt is finite, it is mined
//! out by lasers, it holds only the ores its moon's composition names, and the
//! moon waits [`BELT_GAP_DAYS`] after one before it carries another.
//!
//! ## Which moons are live, and when
//!
//! A pure function of `(world seed, system, game day)` ([`live_in`]), so a
//! restart reconstructs the set and no table is stored. A system's moons are
//! put in a fixed order by a hash of the seed; a *window* is
//! [`BELT_LIFE_DAYS`] game days (staggered per system, so every system does
//! not turn over at once); window `w` lights the moons at positions
//! `[s*K, s*K+K)` of that order, `s = w mod CYCLE`, `K = max(1, moons /
//! MOONS_PER_LIVE)`, `CYCLE = max(ceil(moons / K), MIN_CYCLE)`. A moon is lit
//! once per cycle, so **it waits at least `(MIN_CYCLE - 1) x BELT_LIFE_DAYS`
//! days between belts**, which is at least [`BELT_GAP_DAYS`] ([`gap_days`],
//! a test).
//!
//! ## What a belt is
//!
//! [`plan_belt`]: for each ore of the moon's composition, `percent / 100 x
//! total_units(rarity)` units, cut into rocks of [`ROCK_UNITS`] units each,
//! every rock rolling its quality (base, +15 %, +100 %: `moon::quality`) and
//! sized by the ore's own radius law (`asteroid::Law::radius`, the client's
//! `AsteroidQuantityToRadius` **[V]**). Rocks sit in a flattened ring at
//! [`RING`] round the moon's ball.
//!
//! ## What the world holds
//!
//! - **Ids**: [`MOON_BASE`]`+ seq x `[`IDS_PER_BELT`]` + n`, a band no other
//!   module uses (`site` 8.7e9, `scan` 9.0e12, `missile` 9.5e12,
//!   `command_nodes` 9.6e12, `econ_exchange` 1.0e13). `asteroid::Field::mine`
//!   writes no `rock` delta for it ([`is_moon_id`]): the `rock` delta is keyed
//!   by a real belt and its regrowth would refill a moon belt.
//! - **Persistence**: [`KIND`] `moonrock`, key `(moon, window, n)`, value
//!   `(type_id, remaining)`, written when a rock changes and applied when the
//!   belt is laid out again (a restart, or a pilot returning to a system that
//!   went cold). A window that has passed loses its rows (`tick`, and
//!   [`on_rollover`] for a system nobody visits).
//! - **Lifecycle**: laid out while a system is warm or hot, dropped when it
//!   goes cold (its rows stay), replaced when its window turns over: the old
//!   rocks leave in one `RemoveBalls`, the new ones come in one `AddBalls2`.

use crate::asteroid::{GAP, HALF_HEIGHT, Law, Rock};
use crate::celestial::{Band, Kind, Universe};
use crate::content::Content;
use crate::damage::BlueTime;
use crate::moon::{self, OreTable, Share};
use crate::seed::{self, Stream};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

/// The delta kind this module owns (`delta_kinds::KINDS`): `moon window n ->
/// type_id remaining`.
pub const KIND: &str = "moonrock";

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
pub const SLOT: SlotSpec = SlotSpec {
    name: "moon_belt",
    make,
};

fn make(c: &Content) -> Boxed {
    let ores = c
        .try_get::<crate::typedata::Installed>()
        .map(|i| OreTable::from_types(i.table().types()))
        .unwrap_or_default();
    Box::new(MoonBelts {
        ores: Arc::new(ores),
        ..MoonBelts::default()
    })
}

/// One game day for a belt's life: a belt lives two. **[A]** R-MOON §4.2 says
/// 48 game hours; falsifier: bot mining income against goo price in the soak.
pub const BELT_LIFE_DAYS: u64 = 2;

/// The days a moon rests between two belts, at least. **[A]** R-MOON §4.2.
pub const BELT_GAP_DAYS: u64 = 5;

/// The fewest windows in a moon's cycle: one lit, the rest dark. The gap is
/// `(MIN_CYCLE - 1) x BELT_LIFE_DAYS` = 6 days, which clears
/// [`BELT_GAP_DAYS`].
pub const MIN_CYCLE: u64 = 4;

/// One lit moon per this many in a system. **[A]** R-MOON §4.2.
pub const MOONS_PER_LIVE: u64 = 6;

/// How far from the moon's centre a rock's centre sits, in metres. **[A]**
/// R-MOON §4.2 says 20-45 km; falsifier: H-MN-2, a belt the pilot cannot find
/// from the moon, or one that overlaps the moon.
pub const RING: (f64, f64) = (20_000.0, 45_000.0);

/// Units in one rock, `(least, most)`. **[A]** a belt of the totals R-MOON
/// §4.2 gives is a handful of rocks, each a few minutes of a few lasers.
/// Falsifier: bots that cannot share a belt, or one rock that is a whole
/// belt.
pub const ROCK_UNITS: (i64, i64) = (2_000, 8_000);

/// Ids a belt may use; its rocks past this are not laid out.
pub const IDS_PER_BELT: i64 = 200;

/// Belts live at once, the slot count of the id band.
pub const SEQS: u64 = 100_000;

/// The first id of the band: past `missile` (9.5e12) and `command_nodes`
/// (9.6e12), below `econ_exchange` (1.0e13).
pub const MOON_BASE: i64 = 9_700_000_000_000;

/// One past the last.
pub const MOON_END: i64 = MOON_BASE + SEQS as i64 * IDS_PER_BELT;

/// Whether an id is a moon belt rock's.
pub fn is_moon_id(id: i64) -> bool {
    (MOON_BASE..MOON_END).contains(&id)
}

/// Rock `n` of belt `seq`; `None` past [`IDS_PER_BELT`].
pub fn rock_id(seq: u64, n: usize) -> Option<i64> {
    (n < IDS_PER_BELT as usize).then(|| MOON_BASE + (seq % SEQS) as i64 * IDS_PER_BELT + n as i64)
}

const SALT_SHUFFLE: u64 = 0x4d42_454c_0000_0001;
const SALT_STAGGER: u64 = 0x4d42_454c_0000_0002;
const SALT_BELT: u64 = 0x4d42_454c_0000_0003;

/// The window a system is in on game day `day`. Systems are staggered by a
/// hash of the seed, so they do not all turn over at the downtime.
pub fn window_of(seed: u64, system: i64, day: u32) -> u64 {
    let stagger = seed::mix(seed, &[SALT_STAGGER, system as u64]) % BELT_LIFE_DAYS;
    (u64::from(day) + stagger) / BELT_LIFE_DAYS
}

/// The days between the end of a belt and the next on the same moon, at the
/// least: the dark windows of its cycle.
pub fn gap_days() -> u64 {
    (MIN_CYCLE - 1) * BELT_LIFE_DAYS
}

/// **The lit moons of a system in a window**, out of `moons` (any order).
/// See the module header for the arrangement.
pub fn live_in(seed: u64, system: i64, window: u64, moons: &[i64]) -> Vec<i64> {
    let m = moons.len() as u64;
    if m == 0 {
        return Vec::new();
    }
    let mut order: Vec<i64> = moons.to_vec();
    order.sort_unstable();
    order.dedup();
    let m = order.len() as u64;
    order.sort_by_key(|moon| seed::mix(seed, &[SALT_SHUFFLE, system as u64, *moon as u64]));
    let k = (m / MOONS_PER_LIVE).max(1);
    let cycle = m.div_ceil(k).max(MIN_CYCLE);
    let slot = window % cycle;
    order
        .into_iter()
        .skip((slot * k) as usize)
        .take(k as usize)
        .collect()
}

/// One rock of a planned belt, before it has an id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub type_id: i64,
    pub basic: i64,
    pub quantity: i64,
    pub radius: f64,
    pub position: [f64; 3],
    pub law: Law,
}

/// **A belt's rocks**, deterministic in `(seed, moon, window)` and the moon's
/// composition: per ore of `shares`, `percent / 100 x total_units(rarity)`
/// units cut into [`ROCK_UNITS`] rocks, each of a rolled quality, sized by its
/// ore's law and placed in the ring round `centre`, clear of every other rock
/// by at least `asteroid::GAP`. At most [`IDS_PER_BELT`] rocks.
pub fn plan_belt(
    seed: u64,
    moon_id: i64,
    window: u64,
    centre: [f64; 3],
    shares: &[Share],
    ores: &OreTable,
) -> Vec<Spec> {
    let mut rng = Stream::new(seed::mix(seed, &[SALT_BELT, moon_id as u64, window]));
    let mut out: Vec<Spec> = Vec::new();
    let (least, most) = ROCK_UNITS;
    for share in shares {
        let mut left = share.rarity.total_units() * i64::from(share.percent) / 100;
        while left > 0 && out.len() < IDS_PER_BELT as usize {
            let mut q = rng.range(least as u64, most as u64 + 1) as i64;
            // What would leave a crumb is taken whole.
            if left - q < least {
                q = left;
            }
            let q = q.min(left);
            left -= q;
            let meta = moon::quality(rng.unit());
            let Some(ore) = ores
                .ore(share.basic, meta)
                .or_else(|| ores.ore(share.basic, moon::META_BASE))
            else {
                continue;
            };
            let radius = ore.law.radius(q);
            let mut outer = RING.1;
            let position = loop {
                let mut found = None;
                for _ in 0..TRIES {
                    let rho = RING.0 + rng.unit() * (outer - RING.0);
                    let theta = rng.unit() * std::f64::consts::TAU;
                    let y = (rng.unit() * 2.0 - 1.0) * HALF_HEIGHT;
                    let p = [
                        centre[0] + rho * theta.cos(),
                        centre[1] + y,
                        centre[2] + rho * theta.sin(),
                    ];
                    let clear = out.iter().all(|o| {
                        crate::space_state::distance(p, o.position) - radius - o.radius >= GAP
                    });
                    if clear {
                        found = Some(p);
                        break;
                    }
                }
                match found {
                    Some(p) => break p,
                    None => outer *= 1.1,
                }
            };
            out.push(Spec {
                type_id: ore.type_id,
                basic: share.basic,
                quantity: q,
                radius,
                position,
                law: ore.law,
            });
        }
    }
    out
}

/// Placement attempts per rock before the ring grows by 10 %.
const TRIES: usize = 64;

/// One laid-out belt.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub moon_id: i64,
    pub system: i64,
    pub window: u64,
    pub seq: u64,
    /// `(rock id, units when last written)`, by `n`.
    pub rocks: Vec<(i64, i64)>,
}

/// The world slot.
#[derive(Debug, Default)]
pub struct MoonBelts {
    ores: Arc<OreTable>,
    live: BTreeMap<i64, Instance>,
    /// A system and the window its belts are laid out for.
    settled: BTreeMap<i64, u64>,
    next_seq: u64,
}

impl MoonBelts {
    /// The belt on a moon, if one is laid out.
    pub fn get(&self, moon_id: i64) -> Option<&Instance> {
        self.live.get(&moon_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Instance> {
        self.live.values()
    }

    /// The ore types this world can make moon belts of.
    pub fn ores(&self) -> &OreTable {
        &self.ores
    }

    fn alloc(&mut self) -> Option<u64> {
        let used: std::collections::BTreeSet<u64> = self.live.values().map(|i| i.seq).collect();
        (0..SEQS)
            .map(|k| (self.next_seq + k) % SEQS)
            .find(|s| !used.contains(s))
            .inspect(|s| self.next_seq = (s + 1) % SEQS)
    }
}

/// A moon's delta key.
fn key(moon_id: i64, window: u64, n: usize) -> Vec<String> {
    vec![moon_id.to_string(), window.to_string(), n.to_string()]
}

/// `(moon, window, n)` out of a key.
fn parse_key(k: &[String]) -> Option<(i64, u64, usize)> {
    match k {
        [m, w, n] => Some((m.parse().ok()?, w.parse().ok()?, n.parse().ok()?)),
        _ => None,
    }
}

/// The band of a system: `None` without a universe or for an unknown system.
fn band_of(w: &Registry, system: i64) -> Option<Band> {
    w.content().try_get::<Universe>()?.band(system)
}

/// The moons of a system the server serves, as `(id, centre)`.
fn moons_of(w: &Registry, system: i64) -> Vec<(i64, [f64; 3])> {
    w.celestials()
        .served_in(system)
        .filter(|c| c.kind == Kind::Moon)
        .map(|c| (c.id, c.position))
        .collect()
}

/// The units a rock has left after the delta rows, or `None` for a row that
/// describes another rock (it is dropped).
fn apply_row(spec: &Spec, value: &[String]) -> Option<i64> {
    let [t, left] = value else { return None };
    let (t, left) = (t.parse::<i64>().ok()?, left.parse::<i64>().ok()?);
    (t == spec.type_id && (0..spec.quantity).contains(&left)).then_some(left)
}

/// **Lay out one moon's belt** for `window`: plan it, apply the saved rows,
/// put the rocks in the field and send them in one `AddBalls2` to the pilots
/// in the system. `false` when the band is full or the moon holds nothing.
fn lay_out(w: &mut Registry, system: i64, moon_id: i64, centre: [f64; 3], window: u64) -> bool {
    let seed = w.world().seed;
    let Some(band) = band_of(w, system) else {
        return false;
    };
    let ores = Arc::clone(&w.ext::<MoonBelts>().ores);
    let shares = moon::composition(seed, moon_id, band, &ores);
    let specs = plan_belt(seed, moon_id, window, centre, &shares, &ores);
    if specs.is_empty() {
        return false;
    }
    let Some(seq) = w.ext_mut::<MoonBelts>().alloc() else {
        crate::outln!("[moon_belt] the id band is full; moon {moon_id} waits");
        return false;
    };
    let mut rocks = Vec::new();
    let mut state = Vec::new();
    let mut stale = Vec::new();
    for (n, spec) in specs.iter().enumerate() {
        let Some(id) = rock_id(seq, n) else { break };
        let k = key(moon_id, window, n);
        let left = match w.deltas().of(KIND).get(&k) {
            None => spec.quantity,
            Some(v) => match apply_row(spec, v) {
                Some(left) => left,
                None => {
                    stale.push(k);
                    spec.quantity
                }
            },
        };
        state.push((id, left));
        if left == 0 {
            continue;
        }
        rocks.push(Rock {
            id,
            type_id: spec.type_id,
            belt_id: moon_id,
            solar_system_id: system,
            quantity: left,
            radius: spec.law.radius(left),
            position: spec.position,
            law: spec.law,
        });
    }
    if !stale.is_empty() {
        let d = w.deltas_mut();
        for k in &stale {
            d.del(KIND, k);
        }
    }
    for r in &rocks {
        w.rocks_mut().add(*r);
    }
    w.ext_mut::<MoonBelts>().live.insert(
        moon_id,
        Instance {
            moon_id,
            system,
            window,
            seq,
            rocks: state,
        },
    );
    announce(w, system, &rocks);
    // MN-D: the lit moon is an ore site on the Agency's list.
    crate::moon_scan::show_site(w, system, seq, centre);
    true
}

/// Rocks appearing in a running park: one `AddBalls2` to every character in
/// space in the system, on the clock. Each viewer gets only their own grid's
/// (`grid::filter`, through `BallsAdded`).
fn announce(w: &mut Registry, system: i64, rocks: &[Rock]) {
    if rocks.is_empty() {
        return;
    }
    let to = crate::dungeon::audience(w, system);
    if to.is_empty() {
        return;
    }
    let stamp = w.next_destiny_stamp();
    let park = crate::destiny::Park {
        current_time: stamp,
        balls: rocks.iter().map(crate::destiny::rock_ball).collect(),
    };
    let extra = rocks
        .iter()
        .map(|r| (crate::destiny::rock_slim(r), None))
        .collect();
    if let Some(added) = crate::destiny::BallsAdded::new(to, stamp, park, extra) {
        w.schedule(std::time::Duration::ZERO, added);
    }
}

/// **Take a belt out of space**: its rocks leave the field and every one still
/// there leaves in one `RemoveBalls`. With `forget`, its saved rows go too
/// (the window is over); without, they stay (the system went cold).
/// `false` for a moon with no belt.
pub fn vacate(w: &mut Registry, moon_id: i64, forget: bool) -> bool {
    let Some(inst) = w.ext_mut::<MoonBelts>().live.remove(&moon_id) else {
        return false;
    };
    let mut gone = Vec::new();
    for (id, _) in &inst.rocks {
        if w.rocks_mut().remove(*id).is_some() {
            gone.push(*id);
        }
    }
    if forget {
        forget_rows(w, |m, win| m == moon_id && win == inst.window);
    }
    crate::moon_scan::hide_site(w, inst.system, inst.seq);
    let to = crate::dungeon::audience(w, inst.system);
    if !to.is_empty() {
        let stamp = w.next_destiny_stamp();
        if let Some(rm) = crate::destiny::BallsRemoved::new(to, stamp, gone) {
            w.schedule(std::time::Duration::ZERO, rm);
        }
    }
    true
}

/// Delete every row whose `(moon, window)` `pick` accepts, and every row that
/// is not a key of this kind at all. Answers how many went.
fn forget_rows(w: &mut Registry, pick: impl Fn(i64, u64) -> bool) -> usize {
    let doomed: Vec<Vec<String>> = w
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| parse_key(k).is_none_or(|(m, win, _)| pick(m, win)))
        .cloned()
        .collect();
    if doomed.is_empty() {
        return 0;
    }
    let d = w.deltas_mut();
    for k in &doomed {
        d.del(KIND, k);
    }
    doomed.len()
}

/// Write every rock that changed since it was last written, as a row.
fn capture(w: &mut Registry) {
    let mut rows: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    let mut lasts: Vec<(i64, usize, i64)> = Vec::new();
    for inst in w.ext::<MoonBelts>().live.values() {
        for (n, (id, last)) in inst.rocks.iter().enumerate() {
            let now = w.rocks().get(*id).map_or(0, |r| r.quantity);
            if now == *last {
                continue;
            }
            let Some(type_id) = w.rocks().get(*id).map(|r| r.type_id).or_else(|| {
                w.deltas()
                    .of(KIND)
                    .get(&key(inst.moon_id, inst.window, n))
                    .and_then(|v| v.first()?.parse().ok())
                    .or_else(|| type_of_planned(w, inst, n))
            }) else {
                continue;
            };
            rows.push((
                key(inst.moon_id, inst.window, n),
                vec![type_id.to_string(), now.to_string()],
            ));
            lasts.push((inst.moon_id, n, now));
        }
    }
    if rows.is_empty() {
        return;
    }
    let d = w.deltas_mut();
    for (k, v) in rows {
        d.put(KIND, k, v);
    }
    let belts = w.ext_mut::<MoonBelts>();
    for (moon, n, now) in lasts {
        if let Some(r) = belts.live.get_mut(&moon).and_then(|i| i.rocks.get_mut(n)) {
            r.1 = now;
        }
    }
}

/// A gone rock's type, from the plan (its ball is no longer in the field).
fn type_of_planned(w: &Registry, inst: &Instance, n: usize) -> Option<i64> {
    let band = band_of(w, inst.system)?;
    let belts = w.ext::<MoonBelts>();
    let centre = moons_of(w, inst.system)
        .into_iter()
        .find(|(id, _)| *id == inst.moon_id)?
        .1;
    let seed = w.world().seed;
    let shares = moon::composition(seed, inst.moon_id, band, &belts.ores);
    plan_belt(
        seed,
        inst.moon_id,
        inst.window,
        centre,
        &shares,
        &belts.ores,
    )
    .get(n)
    .map(|s| s.type_id)
}

/// **The stage** (after `asteroid`, which lays out the belts' rocks): write
/// every bite as a row; drop the belts of every system that went cold; and for
/// every system that is warm or hot, replace belts whose window has passed and
/// lay out the lit moons of the current one.
pub fn tick(w: &mut Registry, _now: Instant, _ft: BlueTime) {
    if w.ext::<MoonBelts>().ores.is_empty() {
        return;
    }
    capture(w);
    use crate::liveness::{Liveness, Tier};
    let (cold, live): (Vec<i64>, Vec<i64>) = {
        let l = w.ext::<Liveness>();
        let b = w.ext::<MoonBelts>();
        let cold: Vec<i64> = b
            .live
            .values()
            .map(|i| i.system)
            .chain(b.settled.keys().copied())
            .filter(|s| l.tier(*s) == Tier::Cold)
            .collect();
        (cold, l.iter().map(|(s, _)| s).collect())
    };
    for system in cold {
        let moons: Vec<i64> = w
            .ext::<MoonBelts>()
            .live
            .values()
            .filter(|i| i.system == system)
            .map(|i| i.moon_id)
            .collect();
        for m in moons {
            vacate(w, m, false);
        }
        w.ext_mut::<MoonBelts>().settled.remove(&system);
    }
    for system in live {
        settle(w, system);
    }
}

/// One live system: nothing to do while its belts are laid out for the
/// current window.
fn settle(w: &mut Registry, system: i64) {
    let seed = w.world().seed;
    let window = window_of(seed, system, w.world().day);
    if w.ext::<MoonBelts>().settled.get(&system) == Some(&window) {
        return;
    }
    let old: Vec<i64> = w
        .ext::<MoonBelts>()
        .live
        .values()
        .filter(|i| i.system == system && i.window != window)
        .map(|i| i.moon_id)
        .collect();
    for m in old {
        vacate(w, m, true);
    }
    let moons = moons_of(w, system);
    let ids: Vec<i64> = moons.iter().map(|m| m.0).collect();
    for lit in live_in(seed, system, window, &ids) {
        if w.ext::<MoonBelts>().live.contains_key(&lit) {
            continue;
        }
        if let Some((_, centre)) = moons.iter().find(|m| m.0 == lit) {
            lay_out(w, system, lit, *centre, window);
        }
    }
    w.ext_mut::<MoonBelts>().settled.insert(system, window);
}

/// **The rollover step**: delete the rows of every belt whose window has
/// passed, in systems nobody visited to do it. Answers the rows dropped.
pub fn on_rollover(w: &mut Registry, day: u32) -> usize {
    if w.deltas().of(KIND).is_empty() {
        return 0;
    }
    let seed = w.world().seed;
    let systems: BTreeMap<i64, i64> = w
        .deltas()
        .of(KIND)
        .keys()
        .filter_map(|k| parse_key(k))
        .filter_map(|(m, _, _)| Some((m, w.celestials().system_of(m)?)))
        .collect();
    forget_rows(w, |m, win| {
        systems
            .get(&m)
            .is_none_or(|s| window_of(seed, *s, day) != win)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::celestial::Catalogue;
    use crate::model::SandboxConfig;
    use crate::moon::tests as moon_fixture;
    use crate::space_state::{Motion, Presence};
    use std::time::Duration;

    const SEED: u64 = 0x5eed_0000_f1c5_7e57;
    const ALICE: i64 = 90_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    /// Invented systems in the four bands (see [`universe`]).
    const NULL: i64 = 39_000_003;
    const LOW: i64 = 39_000_002;
    const HIGH: i64 = 39_000_001;
    const WORMHOLE: i64 = 31_000_001;

    fn ores() -> OreTable {
        moon_fixture::table()
    }

    fn shares(moon: i64, band: Band) -> Vec<Share> {
        moon::composition(SEED, moon, band, &ores())
    }

    fn moons(n: i64) -> Vec<i64> {
        (0..n).map(|i| 40_100_000 + i).collect()
    }

    #[test]
    fn ids_are_in_a_band_nothing_else_uses() {
        assert!(is_moon_id(MOON_BASE) && is_moon_id(MOON_END - 1));
        assert!(!is_moon_id(MOON_BASE - 1) && !is_moon_id(MOON_END));
        assert!(!is_moon_id(crate::asteroid::MIN_ROCK_ID));
        assert!(!crate::site::is_instance_id(MOON_BASE));
        const {
            assert!(MOON_BASE > crate::missile::ID_END);
        }
        const {
            assert!(MOON_BASE >= crate::command_nodes::NODE_ID_END);
        }
        const {
            assert!(MOON_END < crate::econ_exchange::NPC_ORDER_BASE);
        }
        const {
            assert!(MOON_BASE > crate::scan::PROBE_ID_BASE);
        }
        assert_eq!(rock_id(0, 0), Some(MOON_BASE));
        assert_eq!(rock_id(1, 3), Some(MOON_BASE + IDS_PER_BELT + 3));
        assert_eq!(rock_id(0, IDS_PER_BELT as usize), None);
        assert_eq!(rock_id(SEQS, 0), Some(MOON_BASE), "wraps");
    }

    #[test]
    fn live_moons_are_deterministic_and_a_function_of_the_window() {
        let m = moons(30);
        for w in 0..20 {
            let a = live_in(SEED, NULL, w, &m);
            assert_eq!(a, live_in(SEED, NULL, w, &m));
            assert_eq!(a.len(), (30 / MOONS_PER_LIVE) as usize, "window {w}");
            assert!(a.iter().all(|x| m.contains(x)));
        }
        let mut rev = m.clone();
        rev.reverse();
        assert_eq!(live_in(SEED, NULL, 3, &m), live_in(SEED, NULL, 3, &rev));
        assert!(live_in(SEED, NULL, 0, &[]).is_empty());
        assert_ne!(live_in(SEED, NULL, 0, &m), live_in(SEED, NULL, 1, &m));
    }

    #[test]
    fn a_belt_expires_and_its_moon_waits() {
        assert!(gap_days() >= BELT_GAP_DAYS);
        for count in [1i64, 2, 3, 5, 6, 7, 12, 30, 61] {
            let m = moons(count);
            let lit: Vec<Vec<i64>> = (0..200).map(|w| live_in(SEED, NULL, w, &m)).collect();
            for moon in &m {
                let at: Vec<usize> = (0..lit.len()).filter(|w| lit[*w].contains(moon)).collect();
                assert!(
                    !at.is_empty() || count > 61,
                    "{count} moons: {moon} never lit"
                );
                for p in at.windows(2) {
                    let dark_days = (p[1] - p[0] - 1) as u64 * BELT_LIFE_DAYS;
                    assert!(
                        dark_days >= BELT_GAP_DAYS,
                        "{count} moons: moon {moon} lit in windows {} and {}",
                        p[0],
                        p[1]
                    );
                }
            }
        }
    }

    #[test]
    fn windows_turn_over_every_belt_life_and_systems_are_staggered() {
        let days = |s| -> Vec<u64> { (0..40).map(|d| window_of(SEED, s, d)).collect() };
        let a = days(NULL);
        assert!(a.windows(2).all(|p| p[1] == p[0] || p[1] == p[0] + 1));
        // Each window spans BELT_LIFE_DAYS days (the first may be short).
        for w in a[0] + 1..*a.last().expect("nonempty") {
            assert_eq!(a.iter().filter(|x| **x == w).count() as u64, BELT_LIFE_DAYS);
        }
        let offsets: std::collections::BTreeSet<u64> = (0..50)
            .map(|s| 30_000_000 + s)
            .map(|s| window_of(SEED, s, 0))
            .collect();
        let turn: std::collections::BTreeSet<u64> = (0..50)
            .map(|s| 30_000_000 + s)
            .map(|s| window_of(SEED, s, 1))
            .collect();
        assert!(
            offsets.len() + turn.len() > 2,
            "some systems turn on odd days"
        );
    }

    #[test]
    fn a_belt_holds_only_its_moons_ores() {
        let t = ores();
        for moon in 40_000_000..40_000_040i64 {
            let s = shares(moon, Band::Null);
            let belt = plan_belt(SEED, moon, 7, [0.0; 3], &s, &t);
            assert!(!belt.is_empty());
            for r in &belt {
                assert!(s.iter().any(|x| x.basic == r.basic), "{moon}: {r:?}");
                let ore = t
                    .ores_by_type(r.type_id)
                    .expect("a rock is an ore of the table");
                assert_eq!(ore.basic, r.basic);
            }
            for share in &s {
                let units: i64 = belt
                    .iter()
                    .filter(|r| r.basic == share.basic)
                    .map(|r| r.quantity)
                    .sum();
                assert_eq!(
                    units,
                    share.rarity.total_units() * i64::from(share.percent) / 100,
                    "the share's units are all in rocks"
                );
            }
        }
    }

    #[test]
    fn rocks_use_the_radius_law_and_the_ring() {
        let t = ores();
        let centre = [1.0e11, 5.0e9, -2.0e10];
        for moon in 40_000_000..40_000_020i64 {
            let belt = plan_belt(SEED, moon, 3, centre, &shares(moon, Band::Null), &t);
            for r in &belt {
                assert_eq!(r.radius, r.law.radius(r.quantity), "the client's own law");
                let flat = (r.position[0] - centre[0]).hypot(r.position[2] - centre[2]);
                assert!(flat >= RING.0 - 1e-6, "{flat}");
                assert!((r.position[1] - centre[1]).abs() <= HALF_HEIGHT + 1e-6);
                assert!(r.quantity >= 1 && r.quantity < ROCK_UNITS.0 + ROCK_UNITS.1);
            }
            for (i, a) in belt.iter().enumerate() {
                for b in &belt[i + 1..] {
                    let gap =
                        crate::space_state::distance(a.position, b.position) - a.radius - b.radius;
                    assert!(gap >= GAP - 1e-6, "rocks overlap: {gap}");
                }
            }
        }
    }

    #[test]
    fn a_belt_is_deterministic_and_changes_with_the_window() {
        let t = ores();
        let s = shares(40_000_001, Band::Null);
        let a = plan_belt(SEED, 40_000_001, 5, [0.0; 3], &s, &t);
        assert_eq!(a, plan_belt(SEED, 40_000_001, 5, [0.0; 3], &s, &t));
        assert_ne!(a, plan_belt(SEED, 40_000_001, 9, [0.0; 3], &s, &t));
        assert!(plan_belt(SEED, 1, 0, [0.0; 3], &[], &t).is_empty());
    }

    #[test]
    fn rocks_roll_every_quality() {
        let t = ores();
        let mut metas = std::collections::BTreeSet::new();
        for moon in 40_000_000..40_000_200i64 {
            for r in plan_belt(SEED, moon, 1, [0.0; 3], &shares(moon, Band::Null), &t) {
                metas.insert(t.ores_by_type(r.type_id).expect("ore").meta_level);
            }
        }
        assert_eq!(
            metas,
            [moon::META_BASE, moon::META_RICH, moon::META_RICHEST].into()
        );
    }

    // ---- the world ----

    /// One moon per system in a few systems, as invented celestials.
    fn universe() -> Universe {
        let row = |id: i64, sec: f64| {
            format!(
                "{id}\t19000001\t29000001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture {id}"
            )
        };
        Universe::parse(
            &[
                row(NULL, -0.5),
                row(LOW, 0.3),
                row(HIGH, 0.9),
                row(WORMHOLE, -0.99),
            ],
            &["19000001\t\t\t\t\tFixture region".to_owned()],
            &["29000001\t19000001\t\t\t\tFixture constellation".to_owned()],
        )
        .expect("the invented rows parse")
    }

    fn world(moon_count: usize) -> Registry {
        let mut c = Content::fixture();
        c.insert("universe", universe());
        let table = crate::typedata::Table::parse(&format!(
            "{}\n{}",
            crate::typedata::FIXTURE,
            moon_fixture::fixture_text()
        ))
        .expect("parses");
        c.insert("typedata", crate::typedata::Installed::build(table));
        let mut text = String::new();
        for (s, system) in [NULL, LOW, HIGH, WORMHOLE].iter().enumerate() {
            for m in 0..moon_count {
                text += &format!(
                    "moon\t{}\t990021\t1500000.0\t{}\t0.0\t{}\t{system}\n",
                    40_200_000 + (s * 1_000 + m) as i64,
                    1.0e11 * (s + 1) as f64,
                    1.0e9 * (m + 1) as f64,
                );
            }
        }
        Registry::new(SandboxConfig {
            celestials: Catalogue::parse(&text),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c))
    }

    fn arrive(reg: &mut Registry, system: i64) {
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            system,
            Motion::arrived([1.0e11, 0.0, 0.0], [0.0; 3]),
        ));
    }

    fn drain(reg: &mut Registry) -> Vec<(&'static str, usize)> {
        let _ = reg.take_pending();
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (name, args.len())
            })
            .collect()
    }

    fn step(reg: &mut Registry) {
        step_at(reg, 0);
    }

    fn step_at(reg: &mut Registry, ft: BlueTime) {
        let now = Instant::now();
        crate::liveness::refresh(reg, now, ft);
        tick(reg, now, ft);
    }

    fn set_day(reg: &mut Registry, day: u32) {
        let world = crate::delta::World {
            day,
            ..*reg.world()
        };
        reg.deltas_mut().set_world(world);
    }

    /// A day on which the system's window lights moon slot 0 of a cycle.
    fn lit_day(reg: &Registry, system: i64, atleast: u32) -> u32 {
        let ids: Vec<i64> = moons_of(reg, system).iter().map(|m| m.0).collect();
        (atleast..atleast + 60)
            .find(|d| {
                !live_in(
                    reg.world().seed,
                    system,
                    window_of(reg.world().seed, system, *d),
                    &ids,
                )
                .is_empty()
            })
            .expect("a lit window within two cycles")
    }

    #[test]
    fn a_pilot_in_a_null_system_gets_the_lit_moons_belt_and_the_field_holds_it() {
        let mut reg = world(7);
        arrive(&mut reg, NULL);
        let day = lit_day(&reg, NULL, 0);
        set_day(&mut reg, day);
        drain(&mut reg);
        step(&mut reg);
        let belts: Vec<Instance> = reg.ext::<MoonBelts>().iter().cloned().collect();
        assert!(!belts.is_empty(), "a belt is laid out");
        for b in &belts {
            assert_eq!(b.system, NULL);
            assert!(!b.rocks.is_empty());
            for (id, units) in &b.rocks {
                let r = reg.rocks().get(*id).expect("in the field");
                assert_eq!(r.quantity, *units);
                assert!(is_moon_id(*id));
                assert_eq!(r.belt_id, b.moon_id);
                assert_eq!(r.solar_system_id, NULL);
            }
        }
        let pushes = drain(&mut reg);
        assert_eq!(pushes.len(), 1, "one AddBalls2 for the lot: {pushes:?}");
        let before = reg.ext::<MoonBelts>().iter().count();
        step(&mut reg);
        assert_eq!(reg.ext::<MoonBelts>().iter().count(), before, "settled");
        assert!(drain(&mut reg).is_empty(), "nothing is sent twice");
    }

    #[test]
    fn no_belt_in_a_wormhole_system() {
        let mut reg = world(30);
        assert_eq!(band_of(&reg, WORMHOLE), Some(Band::Wormhole));
        assert_eq!(band_of(&reg, NULL), Some(Band::Null));
        assert_eq!(band_of(&reg, LOW), Some(Band::Low));
        arrive(&mut reg, WORMHOLE);
        for day in 0..30 {
            set_day(&mut reg, day);
            step(&mut reg);
            assert!(reg.ext::<MoonBelts>().iter().next().is_none(), "day {day}");
        }
    }

    #[test]
    fn a_high_sec_belt_is_only_ubiquitous_ore() {
        let mut reg = world(30);
        arrive(&mut reg, HIGH);
        let mut seen = false;
        for day in (0..40).step_by(2) {
            set_day(&mut reg, day);
            step(&mut reg);
            for b in reg.ext::<MoonBelts>().iter() {
                seen = true;
                for (id, _) in &b.rocks {
                    let t = reg.rocks().get(*id).map(|r| r.type_id);
                    if let Some(t) = t {
                        let o = reg.ext::<MoonBelts>().ores().ores_by_type(t).expect("ore");
                        assert_eq!(o.rarity, moon::Rarity::Ubiquitous);
                    }
                }
            }
        }
        assert!(seen, "some belt was lit");
    }

    #[test]
    fn a_belt_that_stopped_being_live_is_removed_with_its_rows() {
        let mut reg = world(7);
        arrive(&mut reg, NULL);
        let day = lit_day(&reg, NULL, 0);
        set_day(&mut reg, day);
        step(&mut reg);
        let b = reg
            .ext::<MoonBelts>()
            .iter()
            .next()
            .expect("a belt")
            .clone();
        let (id, _) = b.rocks[0];
        let taken = reg.rocks_mut().mine(id, 100).expect("mined");
        assert_eq!(taken.taken, 100);
        drain(&mut reg);
        step(&mut reg);
        assert_eq!(reg.deltas().of(KIND).len(), 1, "the bite is a row");
        // A window or two on, the belt is replaced and its rows are gone.
        let next = (day + 1..day + 40)
            .find(|d| window_of(reg.world().seed, NULL, *d) != b.window)
            .expect("the window turns");
        set_day(&mut reg, next);
        step(&mut reg);
        assert!(
            reg.ext::<MoonBelts>()
                .get(b.moon_id)
                .is_none_or(|i| i.window != b.window)
        );
        assert!(
            reg.rocks().get(id).is_none(),
            "the old rocks left the field"
        );
        assert!(reg.deltas().of(KIND).is_empty(), "its rows went with it");
        let pushes = drain(&mut reg);
        assert!(
            pushes.iter().any(|p| p.0 == "DoDestinyUpdate"),
            "a RemoveBalls went out: {pushes:?}"
        );
    }

    #[test]
    fn a_mined_belt_comes_back_as_it_was_after_the_system_goes_cold() {
        let mut reg = world(7);
        arrive(&mut reg, NULL);
        let day = lit_day(&reg, NULL, 0);
        set_day(&mut reg, day);
        step(&mut reg);
        let b = reg
            .ext::<MoonBelts>()
            .iter()
            .next()
            .expect("a belt")
            .clone();
        let (id0, units0) = b.rocks[0];
        let (id1, units1) = b.rocks.get(1).copied().unwrap_or(b.rocks[0]);
        reg.rocks_mut().mine(id0, 123).expect("mined");
        if id1 != id0 {
            reg.rocks_mut().mine(id1, units1).expect("mined out");
        }
        step(&mut reg);
        // Everyone leaves: the system goes cold and the belt with it.
        reg.roster_mut().leave(ALICE);
        step_at(&mut reg, crate::liveness::COOL_AFTER + 1);
        assert!(reg.ext::<MoonBelts>().get(b.moon_id).is_none(), "dropped");
        assert!(reg.rocks().get(id0).is_none());
        assert!(!reg.deltas().of(KIND).is_empty(), "the rows stay");
        // Back again: laid out whole, minus what was taken.
        arrive(&mut reg, NULL);
        step_at(&mut reg, crate::liveness::COOL_AFTER + 2);
        let back = reg
            .ext::<MoonBelts>()
            .get(b.moon_id)
            .expect("again")
            .clone();
        let (nid0, _) = back.rocks[0];
        assert_eq!(
            reg.rocks().get(nid0).map(|r| r.quantity),
            Some(units0 - 123)
        );
        if id1 != id0 {
            let (nid1, _) = back.rocks[1];
            assert!(
                reg.rocks().get(nid1).is_none(),
                "a mined-out rock stays out"
            );
        }
    }

    #[test]
    fn a_row_that_describes_another_rock_is_dropped() {
        let spec = Spec {
            type_id: 7,
            basic: 7,
            quantity: 100,
            radius: 1.0,
            position: [0.0; 3],
            law: Law {
                growth: 0.5,
                unit_size: 90.0,
                max_radius: 1.0e4,
            },
        };
        let v = |t: &str, l: &str| vec![t.to_owned(), l.to_owned()];
        assert_eq!(apply_row(&spec, &v("7", "40")), Some(40));
        assert_eq!(apply_row(&spec, &v("7", "0")), Some(0));
        assert_eq!(apply_row(&spec, &v("8", "40")), None, "another type");
        assert_eq!(apply_row(&spec, &v("7", "100")), None, "never had more");
        assert_eq!(apply_row(&spec, &v("7", "x")), None);
        assert_eq!(apply_row(&spec, &["7".to_owned()]), None);
    }

    #[test]
    fn the_rollover_drops_rows_of_a_window_that_passed() {
        let mut reg = world(7);
        let moon = moons_of(&reg, NULL)[0].0;
        let day = 10u32;
        let win = window_of(reg.world().seed, NULL, day);
        let d = reg.deltas_mut();
        d.put(KIND, key(moon, win, 0), vec!["1".into(), "5".into()]);
        d.put(KIND, key(moon, win + 7, 1), vec!["1".into(), "5".into()]);
        d.put(KIND, vec!["junk".into()], vec![]);
        assert_eq!(
            on_rollover(&mut reg, day),
            2,
            "the stale window and the junk"
        );
        assert_eq!(reg.deltas().of(KIND).len(), 1);
        assert_eq!(on_rollover(&mut reg, day), 0);
    }

    #[test]
    fn without_moon_ore_in_the_type_data_nothing_is_laid_out() {
        let mut c = Content::fixture();
        c.insert("universe", universe());
        let mut reg = Registry::new(SandboxConfig {
            celestials: Catalogue::parse(&format!(
                "moon\t40200000\t990021\t1.0\t1.0\t0.0\t0.0\t{NULL}\n"
            )),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        arrive(&mut reg, NULL);
        step(&mut reg);
        assert!(reg.ext::<MoonBelts>().ores().is_empty());
        assert!(reg.ext::<MoonBelts>().iter().next().is_none());
    }
}
