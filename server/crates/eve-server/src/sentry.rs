//! **Sentry guns in low sec** (NW-8, `docs/lanes/nw-8.md`).
//!
//! The spec is lane NW-8 and section 7 of the null-war report. The client
//! has the type group (group 99, [`crate::npc_data::SENTRY_GROUP`]) and
//! `sentryTargetSwitchDelay` (40,000 ms, `appConst.py:1522`) and **no rule
//! for placement or targeting** [V]; the rules below are therefore `[A]`s,
//! each with its falsifier.
//!
//! # The rules
//!
//! - **Where**: a low-sec system (the displayed security above 0.0 and under
//!   0.5, `celestial::Band::Low`) with a pilot in it holds
//!   [`PER_ANCHOR`] sentries by every stargate and every station. High sec
//!   has CONCORD's checkpoints, null has nothing. A system with no pilot for
//!   [`VACANT`] loses them. **[A]** Falsified by the owner, or by a TQ
//!   capture of a low-sec gate.
//! - **Whom**: the sentries fire on a pilot the crime watch flags, a criminal
//!   or a suspect (`crimewatch::flag_of`), on their grid, that a law brain
//!   will engage: **not a capsule** (the brain never shoots a pod), not
//!   cloaked and not protected (`invuln::ship_protected`). The innocent are
//!   left alone: a sentry with nobody to shoot stands in [`IDLE`], and a law
//!   ship with no offender sees nobody. **[A]** Falsified by a TQ capture of
//!   a gate gun firing on an unflagged pilot, sparing a suspect, or shooting
//!   a pod.
//! - **The switch waits**: a sentry that has taken a target keeps it for
//!   [`SWITCH_DELAY`] (`sentryTargetSwitchDelay`) before it may take another,
//!   a nearer criminal included. It drops the target at once when the pilot
//!   leaves the grid, docks, dies or stops being flagged **[A]**, and then
//!   takes the next at once. Falsified by a TQ sentry that idles for the
//!   delay after its target left.
//! - **Losses**: a sentry can be destroyed, and the post is refilled
//!   [`RESPAWN`] later **[A]**; nothing persists across a restart.
//! - **The look**: the group-99 type whose `race_id` (`types/market.tsv`) is
//!   one of the system owner faction's `member_races` (`npc/factions.tsv`),
//!   the lowest type id among those of that race ([`type_for`]); the owner
//!   is that faction's corporation. A faction with no such type, or whose
//!   corporation is CONCORD's, gets **no** sentries (logged once per system):
//!   a death to a sentry must not count as a CONCORD kill
//!   (`concord::is_concord_kill`). **[A]** Falsified by a client capture of a
//!   sentry's graphic in a system, or a TQ low-sec system with an unowned
//!   gate gun. Hull, damage and ranges are the typedata's (about 176 dps by
//!   the spec's reading of `damageMultiplier`, a cycle of 2.5 s, 500 km); a
//!   world with no bundle flies [`FIXTURE_SENTRY`], owned by an invented NPC
//!   corporation.
//! - **Cost**: posts are kept in a map ordered by system, so a system's posts
//!   are a range, not a filter; refill and vacancy run every [`SWEEP`]; a
//!   system with no flagged pilot and no held aim is not advanced.

use crate::concord::{CONCORD_CORP, Concord};
use crate::damage::BlueTime;
use crate::npc::{Law, NpcType};
use crate::npc_data::Table;
use crate::service::Registry;
use std::collections::BTreeMap;

/// One second, in blue ticks.
const SEC: BlueTime = 10_000_000;

/// `sentryTargetSwitchDelay`, 40,000 ms [V] (`appConst.py:1522`).
pub const SWITCH_DELAY: BlueTime = 40 * SEC;
/// **[A]** Sentries standing by each gate and each station.
pub const PER_ANCHOR: u32 = 3;
/// **[A]** How far off the surface of its gate or station a sentry stands,
/// and the spread of the three.
pub const DISTANCE: f64 = 3_000.0;
pub const SPACING: f64 = 1_500.0;
/// **[A]** A lost sentry is replaced after this long.
pub const RESPAWN: BlueTime = 10 * 60 * SEC;
/// A system with no pilot for this long loses its sentries.
pub const VACANT: BlueTime = crate::belts::VACANT_DESPAWN;

/// How often posts are refilled and vacant systems noticed: about a second,
/// as `concord::tick_police` sweeps.
pub const SWEEP: BlueTime = SEC;

/// **[A]** The sentry type of an owner faction: of the group-99 types
/// (`Table::sentry_types`, `(race, type)` ascending), the lowest type id whose
/// race is the first of the faction's `member_races` that has one. `None` for
/// no faction, an unknown one, or no type of its races. Falsified by a client
/// capture of a sentry's graphic in a system.
pub fn type_for(t: &Table, faction: Option<i64>) -> Option<i64> {
    let races = &t.factions.get(&faction?)?.member_races;
    races.iter().find_map(|r| {
        t.sentry_types
            .iter()
            .find(|(race, _)| race == r)
            .map(|(_, ty)| *ty)
    })
}

/// **Invented** NPC corporation owning [`FIXTURE_SENTRY`]; never CONCORD's.
pub const FIXTURE_CORP: i64 = 990_330_032;

/// **Invented** stand-in for a world with no bundle (and every test): a
/// stationary gun that cannot be out-tanked by a frigate.
pub const FIXTURE_SENTRY: NpcType = {
    let base = crate::concord::fixture(FIXTURE_CORP, 990_330_031, "Fixture Sentry", 10.0, 1.0);
    NpcType {
        hull: crate::destiny::hull::Hull {
            max_velocity: 0.0,
            ..base.hull
        },
        ..base
    }
};

/// A sentry with nobody to shoot: a law ship whose offender is no pilot sees
/// nobody. Not `concord::STANDBY` (offender 0), so a checkpoint's own count
/// and `concord::claim` never mistake a sentry for CONCORD.
pub const IDLE: Law = Law { offender: -1 };

/// What a sentry has taken: whom, and since when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aim {
    pub offender: i64,
    pub since: BlueTime,
}

/// A post: one sentry by one gate or station.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Post {
    pub id: Option<i64>,
    pub respawn_at: Option<BlueTime>,
    pub builds: u32,
}

/// The state, a field of [`Concord`]. Never persisted.
#[derive(Debug, Clone, Default)]
pub struct Sentries {
    /// By `(system, anchor, slot)`.
    posts: BTreeMap<(i64, i64, u32), Post>,
    vacant: BTreeMap<i64, BlueTime>,
    aims: BTreeMap<i64, Aim>,
    kinds: BTreeMap<(i64, i64), &'static NpcType>,
    /// Systems refused (no type, CONCORD owner, stale bundle), already logged.
    refused: std::collections::BTreeSet<i64>,
    /// Systems that hold posts.
    served: std::collections::BTreeSet<i64>,
    next_sweep: BlueTime,
}

impl Sentries {
    /// The sentries standing in `system`.
    pub fn standing(&self, system: i64) -> Vec<i64> {
        self.posts
            .range((system, i64::MIN, 0)..=(system, i64::MAX, u32::MAX))
            .filter_map(|(_, p)| p.id)
            .collect()
    }

    /// What sentry `id` has taken.
    pub fn aim(&self, id: i64) -> Option<Aim> {
        self.aims.get(&id).copied()
    }
}

/// **The sentry pass**, every tick (from `concord::tick`). Aiming runs every
/// tick over the systems that hold posts; the refill and vacancy pass over
/// every hot system runs every [`SWEEP`].
pub fn tick(w: &mut Registry, now: BlueTime) {
    if !w.ext::<Concord>().enabled {
        return;
    }
    let sweep = now >= w.ext::<Concord>().sentries.next_sweep;
    let mut systems: Vec<i64> = w.ext::<Concord>().sentries.served.iter().copied().collect();
    if sweep {
        let every = crate::game_clock::span(w, SWEEP);
        w.ext_mut::<Concord>().sentries.next_sweep = now + every;
        systems.extend(w.ext::<crate::liveness::Liveness>().hot());
        systems.sort_unstable();
        systems.dedup();
    }
    for system in systems {
        if sweep && !sweep_system(w, system, now) {
            continue;
        }
        aim_system(w, system, now);
    }
}

/// The sweep of one system: refill its posts, or let them go when it has been
/// vacant. `false` when the system holds no sentries (not low sec, or empty).
fn sweep_system(w: &mut Registry, system: i64, now: BlueTime) -> bool {
    let low = w
        .content()
        .get::<crate::celestial::Universe>()
        .system(system)
        .is_some_and(|s| s.band() == crate::celestial::Band::Low);
    if !low {
        return false;
    }
    let pilots = w
        .roster()
        .in_system(system)
        .into_iter()
        .any(|p| w.character(p.character_id).is_some());
    if !pilots {
        let since = *w
            .ext_mut::<Concord>()
            .sentries
            .vacant
            .entry(system)
            .or_insert(now);
        if now.saturating_sub(since) >= crate::game_clock::span(w, VACANT) {
            clear_system(w, system);
        }
        return false;
    }
    w.ext_mut::<Concord>().sentries.vacant.remove(&system);
    let anchors: Vec<(i64, [f64; 3], f64)> = w
        .celestials()
        .served_in(system)
        .filter(|c| {
            matches!(
                c.kind,
                crate::celestial::Kind::Gate | crate::celestial::Kind::Station
            )
        })
        .map(|c| (c.id, c.position, c.radius))
        .collect();
    for (anchor, position, radius) in anchors {
        for slot in 0..PER_ANCHOR {
            step_post(w, (system, anchor, slot), position, radius, now);
        }
    }
    true
}

/// Refuse `system`'s sentries, logging once.
fn refuse(w: &mut Registry, system: i64, why: &str) -> Option<&'static NpcType> {
    if w.ext_mut::<Concord>().sentries.refused.insert(system) {
        crate::outln!("[sentry] ERROR: {why}; no sentries in {system}");
    }
    None
}

/// The sentry kind for `system`: the type and owner of its owner faction from
/// the bundle ([`type_for`]); the invented fixture in a world with no bundle;
/// `None` (logged once) for a bundle that gives no type, or an owner that is
/// CONCORD's, so that nothing is invented on a real client and a sentry kill
/// is never a CONCORD kill.
fn kind_in(w: &mut Registry, system: i64) -> Option<&'static NpcType> {
    if !w.content().is_bundle() {
        return Some(&FIXTURE_SENTRY);
    }
    let faction = w
        .content()
        .get::<crate::celestial::Universe>()
        .system(system)
        .and_then(|s| s.owner_faction);
    let pick = w.content().try_get::<Table>().and_then(|t| {
        let owner = t.factions.get(&faction?)?.corporation_id;
        Some((type_for(t, faction)?, owner))
    });
    let Some((type_id, owner)) = pick else {
        return refuse(
            w,
            system,
            &format!(
                "no group-99 type for the owner faction {faction:?} (npc/factions.tsv, types/market.tsv)"
            ),
        );
    };
    if owner == CONCORD_CORP {
        return refuse(w, system, "the owner faction's corporation is CONCORD's");
    }
    if let Some(k) = w.ext::<Concord>().sentries.kinds.get(&(type_id, owner)) {
        return Some(k);
    }
    match crate::npc::derive_owned(w, type_id, owner) {
        Some(k) => {
            w.ext_mut::<Concord>()
                .sentries
                .kinds
                .insert((type_id, owner), k);
            Some(k)
        }
        None => refuse(
            w,
            system,
            &format!("no sentry type {type_id} in the typedata: the bundle is stale"),
        ),
    }
}

/// Where sentry `slot` of an anchor stands: a random direction, off the
/// surface by [`DISTANCE`] and its spacing.
pub fn post_position(anchor: [f64; 3], radius: f64, seed: u64, slot: u32) -> [f64; 3] {
    let s = seed ^ ((u64::from(slot) + 1) << 40);
    let z = 2.0 * crate::concord::unit(s) - 1.0;
    let phi = std::f64::consts::TAU * crate::concord::unit(s ^ 0xA5A5);
    let r = (1.0 - z * z).sqrt();
    let d = (radius + DISTANCE + SPACING * f64::from(slot)).max(1_000.0);
    [
        anchor[0] + d * r * phi.cos(),
        anchor[1] + d * z,
        anchor[2] + d * r * phi.sin(),
    ]
}

/// One post: forget a lost sentry (and start its wait), fill it when the wait
/// is over.
fn step_post(w: &mut Registry, key: (i64, i64, u32), anchor: [f64; 3], radius: f64, now: BlueTime) {
    let mut p = w
        .ext::<Concord>()
        .sentries
        .posts
        .get(&key)
        .cloned()
        .unwrap_or_default();
    if p.id.is_some_and(|id| w.roster().get(id).is_none()) {
        w.ext_mut::<Concord>()
            .sentries
            .aims
            .remove(&p.id.unwrap_or(0));
        p.id = None;
        p.respawn_at = Some(now + crate::game_clock::span(w, RESPAWN));
    }
    if p.id.is_none()
        && p.respawn_at.is_none_or(|t| now >= t)
        && let Some(kind) = kind_in(w, key.0)
    {
        {
            let seed = crate::concord::mix(key.1 as u64 ^ crate::concord::mix(u64::from(p.builds)));
            let at = post_position(anchor, radius, seed, key.2);
            if let Some(id) = crate::npc::spawn_bound(w, kind, key.0, at, IDLE) {
                crate::npc::announce(w, key.0, &[id], true);
                crate::outln!(
                    "[sentry] post {} of {} in {}: type {} owner {}",
                    key.2,
                    key.1,
                    key.0,
                    kind.type_id,
                    kind.owner_corp_id
                );
                p.id = Some(id);
                p.builds += 1;
                p.respawn_at = None;
            }
        }
    }
    let sentries = &mut w.ext_mut::<Concord>().sentries;
    sentries.posts.insert(key, p);
    sentries.served.insert(key.0);
}

/// A vacant system: its sentries leave.
fn clear_system(w: &mut Registry, system: i64) {
    let keys: Vec<(i64, i64, u32)> = w
        .ext::<Concord>()
        .sentries
        .posts
        .range((system, i64::MIN, 0)..=(system, i64::MAX, u32::MAX))
        .map(|(k, _)| *k)
        .collect();
    let mut ids = Vec::new();
    for k in keys {
        if let Some(p) = w.ext_mut::<Concord>().sentries.posts.remove(&k) {
            ids.extend(p.id);
        }
    }
    for id in &ids {
        w.ext_mut::<Concord>().sentries.aims.remove(id);
    }
    crate::npc::despawn_ids(w, system, &ids);
    let sentries = &mut w.ext_mut::<Concord>().sentries;
    sentries.vacant.remove(&system);
    sentries.served.remove(&system);
}

/// Is `character` a pilot a sentry takes: flagged by the crime watch and one
/// a law brain will engage, which is not a capsule (`npc::brain::best` and
/// `decide` drop them), not cloaked and not protected (`npc::view`).
fn engageable(w: &Registry, character: i64, ship: i64, now: BlueTime) -> bool {
    w.pilot(character)
        .is_some_and(|p| p.ship_type_id != crate::damage::CAPSULE_TYPE_ID)
        && crate::crimewatch::flag_of(w, character, now).is_some()
        && !crate::invuln::ship_protected(w, ship, now)
        && !crate::cloak::is_cloaked(w, ship)
}

/// **Who a sentry shoots**, for each sentry of `system`: the nearest flagged
/// pilot on its grid, kept for [`SWITCH_DELAY`] and dropped at once when it
/// stops being valid; nobody if no pilot is flagged.
fn aim_system(w: &mut Registry, system: i64, now: BlueTime) {
    let guns = w.ext::<Concord>().sentries.standing(system);
    let any = |w: &Registry| {
        w.roster()
            .in_system(system)
            .into_iter()
            .any(|p| engageable(w, p.character_id, p.ship_id, now))
    };
    // Nobody flagged and nothing held: nothing to move or decide.
    if !guns
        .iter()
        .any(|g| w.ext::<Concord>().sentries.aim(*g).is_some())
        && !any(w)
    {
        return;
    }
    w.advance_system(system);
    let flagged: Vec<(i64, [f64; 3])> = w
        .roster()
        .in_system(system)
        .into_iter()
        .filter(|p| engageable(w, p.character_id, p.ship_id, now))
        .map(|p| (p.character_id, p.motion.position))
        .collect();
    let delay = crate::game_clock::span(w, SWITCH_DELAY);
    for id in guns {
        let Some(at) = w.roster().get(id).map(|p| p.motion.position) else {
            continue;
        };
        let near = |c: &(i64, [f64; 3])| crate::space_state::distance(c.1, at);
        let on_grid = |c: &&(i64, [f64; 3])| near(c) <= crate::grid::GRID_ENTER;
        let held = w.ext::<Concord>().sentries.aim(id);
        let valid = held.and_then(|a| {
            flagged
                .iter()
                .filter(on_grid)
                .find(|c| c.0 == a.offender)
                .map(|_| a)
        });
        if valid.is_some_and(|a| now < a.since + delay) {
            continue;
        }
        let pick = flagged
            .iter()
            .filter(on_grid)
            .min_by(|a, b| near(a).total_cmp(&near(b)).then(a.0.cmp(&b.0)))
            .map(|c| c.0);
        match (pick, valid) {
            (Some(p), Some(a)) if p == a.offender => {}
            (Some(p), _) => {
                crate::npc::set_law(w, id, Law { offender: p });
                crate::outln!("[sentry] {id} takes {p} in {system}");
                w.ext_mut::<Concord>().sentries.aims.insert(
                    id,
                    Aim {
                        offender: p,
                        since: now,
                    },
                );
            }
            (None, _) => {
                if held.is_some() {
                    crate::npc::set_law(w, id, IDLE);
                    crate::outln!("[sentry] {id} stands down in {system}");
                    w.ext_mut::<Concord>().sentries.aims.remove(&id);
                }
            }
        }
    }
}

/// `/flag suspect|criminal [characterID]`: flag a pilot by hand, as a crime
/// would, for the client scenario `fci_sentry` and a GM who wants to see the
/// guns work. The flag runs its ordinary timer. A `slash::COMMANDS` row
/// (coordinator).
pub fn slash(
    w: &mut Registry,
    actor: Option<i64>,
    words: &[&str],
) -> Result<eve_marshal::PyValue, crate::service::ServiceError> {
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    let (kind, who) = match words {
        [k] => (*k, actor),
        [k, c] => (*k, c.parse::<i64>().ok()),
        _ => return say("usage: /flag suspect|criminal [characterID]"),
    };
    let Some(who) = who.filter(|id| w.character(*id).is_some()) else {
        return say("no such character");
    };
    let now = crate::pyutil::filetime_now();
    match kind {
        "suspect" => crate::crimewatch::suspect(w, who, now),
        "criminal" => crate::crimewatch::flag_criminal(w, who, now, now),
        _ => return say("usage: /flag suspect|criminal [characterID]"),
    }
    say(&format!("{who} flagged {kind}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const SHIP: [i64; 2] = [100_000_001, 100_000_002];
    /// An invented hull: the typedata fixture's frigate.
    const HULL: i64 = 990_000_001;
    const SYS: i64 = 30_000_003;
    const T0: BlueTime = 133_000_000_000_000_000;
    const GATE: i64 = 50_000_001;
    const STATION: i64 = 60_000_001;

    /// A world of one system of `security` with a gate and a station, and
    /// Alice and Bob in space on the gate's grid.
    fn world(security: f64) -> Registry {
        world_with(security, None)
    }

    /// [`world`], as a bundle's with `npc_data` when it is given.
    fn world_with(security: f64, bundle: Option<Table>) -> Registry {
        let text = format!(
            "gate\t{GATE}\t990001\t5000\t0\t0\t0\t{SYS}\t{}\t30000099\n\
             station\t{STATION}\t990002\t3000\t2000000\t0\t0\t{SYS}\t0\t0\n",
            GATE + 1000
        );
        let pilot = |i: usize, id: i64| crate::model::Character {
            character_id: id,
            name: format!("pilot {id}"),
            ship_id: SHIP[i],
            ship_type_id: HULL,
            corporation_id: 98_000_001 + i as i64,
            balance: 1_000_000.0,
            ..crate::model::Character::default()
        };
        let mut content = crate::content::Content::fixture();
        if let Some(t) = bundle {
            content.mark_bundle();
            content.insert("npc_data", t);
        }
        content.insert(
            "universe",
            crate::celestial::Universe::parse(
                &[format!(
                    "{SYS}\t19000001\t29000001\t\t\t{security}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx"
                )],
                &[],
                &[],
            )
            .expect("the invented row parses"),
        );
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![pilot(0, ALICE), pilot(1, BOB)],
            celestials: crate::celestial::Catalogue::parse(&text),
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
        for (i, id) in [ALICE, BOB].into_iter().enumerate() {
            reg.character_mut(id).expect("a pilot").solar_system_id = Some(SYS);
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    id,
                    SHIP[i],
                    SYS,
                    crate::space_state::Motion::arrived(
                        [10_000.0 + i as f64 * 5_000.0, 0.0, 0.0],
                        [0.0; 3],
                    ),
                ));
        }
        let _ = reg.take_pending();
        reg
    }

    fn live(reg: &mut Registry, now: BlueTime) {
        crate::liveness::refresh(reg, std::time::Instant::now(), now);
        crate::concord::tick(reg, std::time::Instant::now(), now);
    }

    fn step(reg: &mut Registry, now: BlueTime) {
        crate::combat::tick(reg, now);
        crate::npc::tick(reg, now);
        crate::liveness::refresh(reg, std::time::Instant::now(), now);
        crate::concord::tick(reg, std::time::Instant::now(), now);
    }

    fn standing(reg: &Registry) -> Vec<i64> {
        reg.ext::<Concord>().sentries.standing(SYS)
    }

    fn offender_of(reg: &Registry, id: i64) -> Option<i64> {
        reg.npcs().get(id).and_then(|n| n.law).map(|l| l.offender)
    }

    /// **Low sec only**: 0.1 to 0.4 hold [`PER_ANCHOR`] sentries by each of
    /// the gate and the station; 0.5 and up (CONCORD's), null, and a system
    /// the table lacks hold none.
    #[test]
    fn sentries_stand_only_in_low_sec() {
        for sec in [0.1, 0.25, 0.4] {
            let mut reg = world(sec);
            live(&mut reg, T0);
            assert_eq!(standing(&reg).len() as u32, 2 * PER_ANCHOR, "{sec}");
            for id in standing(&reg) {
                let n = reg.npcs().get(id).expect("an npc");
                assert_eq!(n.law, Some(IDLE));
                assert_eq!(n.kind.type_id, FIXTURE_SENTRY.type_id);
            }
        }
        for sec in [0.5, 0.9, 0.0, -0.4] {
            let mut reg = world(sec);
            live(&mut reg, T0);
            assert!(standing(&reg).is_empty(), "{sec}");
            assert!(
                reg.npcs()
                    .iter()
                    .all(|n| n.kind.type_id != FIXTURE_SENTRY.type_id),
                "{sec}"
            );
        }
        // Posts are by the gate and the station, off their surfaces.
        let mut reg = world(0.25);
        live(&mut reg, T0);
        let near = |anchor: [f64; 3], radius: f64| {
            standing(&reg)
                .into_iter()
                .filter(|id| {
                    let d = crate::space_state::distance(
                        reg.roster().get(*id).expect("placed").motion.position,
                        anchor,
                    );
                    d >= radius + DISTANCE - 1.0
                        && d <= radius + DISTANCE + SPACING * f64::from(PER_ANCHOR) + 1.0
                })
                .count()
        };
        assert_eq!(near([0.0; 3], 5_000.0), PER_ANCHOR as usize);
        assert_eq!(near([2_000_000.0, 0.0, 0.0], 3_000.0), PER_ANCHOR as usize);
    }

    /// `/flag` flags the caller, by name, and refuses a word it does not know.
    #[test]
    fn the_flag_command_flags_the_caller() {
        let mut reg = world(0.25);
        let usage = crate::pyutil::bstr("usage: /flag suspect|criminal [characterID]");
        assert_eq!(
            slash(&mut reg, Some(ALICE), &["bogus"]).expect("answers"),
            usage
        );
        let said = |r: Result<eve_marshal::PyValue, crate::service::ServiceError>, w: &str| {
            r.expect("answers") == crate::pyutil::bstr(w)
        };
        assert!(!crate::crimewatch::is_suspect(
            &reg,
            ALICE,
            crate::pyutil::filetime_now()
        ));
        assert!(said(
            slash(&mut reg, Some(ALICE), &["suspect"]),
            &format!("{ALICE} flagged suspect")
        ));
        let now = crate::pyutil::filetime_now();
        assert!(crate::crimewatch::is_suspect(&reg, ALICE, now));
        assert!(crate::crimewatch::flag_of(&reg, BOB, now).is_none());
        assert!(said(
            slash(&mut reg, Some(BOB), &["criminal"]),
            &format!("{BOB} flagged criminal")
        ));
        assert!(crate::crimewatch::is_criminal(
            &reg,
            BOB,
            crate::pyutil::filetime_now()
        ));
    }

    /// An invented faction table: faction 990_100 of races 7 then 9, faction
    /// 990_101 of race 8 (which has no sentry type), types 990_201 and 990_200
    /// of race 9 and 990_202 of race 7.
    fn table() -> Table {
        let mut t = Table::default();
        let faction = |id: i64, corp: i64, races: &[i64]| crate::npc_data::Faction {
            faction_id: id,
            corporation_id: corp,
            member_races: races.to_vec(),
            ..crate::npc_data::Faction::default()
        };
        t.factions
            .insert(990_100, faction(990_100, 990_110, &[7, 9]));
        t.factions.insert(990_101, faction(990_101, 990_111, &[8]));
        t.factions
            .insert(990_102, faction(990_102, CONCORD_CORP, &[9]));
        t.sentry_types = vec![(7, 990_202), (9, 990_200), (9, 990_201)];
        t
    }

    /// **The type follows the owner faction's race**, the lowest id of the
    /// race on a tie, and a faction with none gets none.
    #[test]
    fn the_type_follows_the_owner_faction() {
        let t = table();
        assert_eq!(type_for(&t, Some(990_100)), Some(990_202), "first race");
        assert_eq!(
            type_for(&t, Some(990_102)),
            Some(990_200),
            "lowest of a race"
        );
        assert_eq!(type_for(&t, Some(990_101)), None, "no type of its race");
        assert_eq!(type_for(&t, Some(1)), None, "unknown faction");
        assert_eq!(type_for(&t, None), None);
        assert_eq!(SWITCH_DELAY, 40 * SEC);
    }

    /// **No CONCORD-owned sentries**: the fixture's owner is an invented NPC
    /// corporation, and a bundle world whose owner faction is CONCORD's, or
    /// that has no type for it, spawns nothing.
    #[test]
    fn a_sentry_is_never_concords() {
        assert_ne!(FIXTURE_SENTRY.owner_corp_id, CONCORD_CORP);
        let mut reg = world(0.25);
        live(&mut reg, T0);
        for id in standing(&reg) {
            assert_ne!(
                reg.npcs().get(id).expect("npc").kind.owner_corp_id,
                CONCORD_CORP
            );
        }
        // A bundle world with a table, an unowned system: refused.
        let mut reg = world_with(0.25, Some(table()));
        live(&mut reg, T0);
        assert!(standing(&reg).is_empty(), "no owner faction");
        assert!(reg.ext::<Concord>().sentries.refused.contains(&SYS));
    }

    /// **They hold on the innocent**: a clean pilot beside the guns for a
    /// minute is not targeted and takes no damage; a pilot whose flag lapses
    /// is let go.
    #[test]
    fn they_hold_on_the_innocent() {
        let mut reg = world(0.25);
        live(&mut reg, T0);
        let mut now = T0;
        for _ in 0..240 {
            now += SEC / 4;
            step(&mut reg, now);
        }
        for id in standing(&reg) {
            assert_eq!(offender_of(&reg, id), Some(-1));
            assert!(reg.npcs().get(id).expect("npc").target.is_none());
        }
        assert!(reg.ext::<Concord>().sentries.aims.is_empty());
        assert_eq!(reg.roster().get(ALICE).map(|p| p.ship_id), Some(SHIP[0]));
        // Alice turns criminal: taken. Her flag runs out: let go.
        crate::crimewatch::flag_criminal(&mut reg, ALICE, now, now);
        now += SEC;
        step(&mut reg, now);
        assert!(
            standing(&reg)
                .iter()
                .all(|id| offender_of(&reg, *id) == Some(ALICE))
        );
        let later = now + crate::crimewatch::CRIMINAL_TIMEOUT + 5 * SEC;
        assert!(crate::crimewatch::flag_of(&reg, ALICE, later).is_none());
        step(&mut reg, later);
        for id in standing(&reg) {
            assert_eq!(offender_of(&reg, id), Some(-1), "{id} lets go");
        }
        assert!(reg.ext::<Concord>().sentries.aims.is_empty());
    }

    /// **A pod is not shot**, so a sentry does not pin on one: with a criminal
    /// in a capsule nearer than a criminal in a ship, every sentry takes the
    /// ship.
    #[test]
    fn a_pod_does_not_pin_a_sentry() {
        let mut reg = world(0.25);
        live(&mut reg, T0);
        reg.character_mut(ALICE).expect("a pilot").ship_type_id = crate::damage::CAPSULE_TYPE_ID;
        // Alice (the pod) sits on the gate's guns, Bob farther out.
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .position = [100.0, 0.0, 0.0];
        crate::crimewatch::flag_criminal(&mut reg, ALICE, T0, T0);
        crate::crimewatch::flag_criminal(&mut reg, BOB, T0, T0);
        step(&mut reg, T0 + SEC);
        let guns = standing(&reg);
        assert_eq!(guns.len() as u32, 2 * PER_ANCHOR);
        for id in &guns {
            assert_eq!(offender_of(&reg, *id), Some(BOB), "{id} takes the ship");
        }
        // Bob alone leaves: the pod is not picked up in his place.
        reg.roster_mut()
            .get_mut(BOB)
            .expect("in space")
            .motion
            .position = [0.0, 0.0, 10.0 * crate::grid::GRID_ENTER];
        step(&mut reg, T0 + 2 * SEC);
        for id in &guns {
            assert_eq!(offender_of(&reg, *id), Some(-1), "{id} stands down");
        }
    }

    /// **They fire on a criminal**: a flagged pilot is taken by every sentry
    /// on the grid, and only the flagged one.
    #[test]
    fn they_fire_on_a_criminal() {
        let mut reg = world(0.25);
        live(&mut reg, T0);
        crate::crimewatch::flag_criminal(&mut reg, ALICE, T0, T0);
        assert!(crate::crimewatch::is_criminal(&reg, ALICE, T0));
        let mut now = T0;
        for _ in 0..8 {
            now += SEC / 4;
            step(&mut reg, now);
        }
        let guns = standing(&reg);
        assert_eq!(guns.len() as u32, 2 * PER_ANCHOR);
        for id in &guns {
            assert_eq!(offender_of(&reg, *id), Some(ALICE), "{id}");
            assert_eq!(
                reg.ext::<Concord>().sentries.aim(*id).map(|a| a.offender),
                Some(ALICE)
            );
        }
        // They lock and shoot: a sentry has Alice's ship locked at some
        // point (or has already killed her), and never Bob's.
        let mut locked: Vec<i64> = Vec::new();
        let mut alice_hit = false;
        for _ in 0..120 {
            now += SEC / 4;
            step(&mut reg, now);
            locked.extend(guns.iter().flat_map(|id| reg.locks().targets_of(*id)));
            alice_hit |= reg.roster().get(ALICE).is_none_or(|p| p.ship_id != SHIP[0]);
        }
        assert!(locked.contains(&SHIP[0]) || alice_hit, "Alice is locked");
        assert!(!locked.contains(&SHIP[1]), "Bob never");
        // Bob, clean on the same grid, is never taken.
        assert!(
            reg.npcs()
                .iter()
                .all(|n| n.law.is_none_or(|l| l.offender != BOB))
        );
        // A suspect is shot at too: the server's own rule (`crimewatch::flag_of`
        // counts a suspect), an [A] about TQ; not a claim about the client.
        let mut reg = world(0.25);
        live(&mut reg, T0);
        crate::crimewatch::suspect(&mut reg, BOB, T0);
        step(&mut reg, T0 + SEC);
        assert!(
            standing(&reg)
                .iter()
                .all(|id| offender_of(&reg, *id) == Some(BOB))
        );
    }

    /// A flagged pilot off the grid, or one whose flag ran out, is left alone;
    /// the sentry stands down at once, its target and locks released.
    #[test]
    fn a_pilot_who_leaves_or_clears_is_dropped_at_once() {
        let released = |reg: &Registry, guns: &[i64]| {
            guns.iter().all(|id| {
                offender_of(reg, *id) == Some(-1)
                    && reg.npcs().get(*id).is_some_and(|n| n.target.is_none())
                    && reg.locks().targets_of(*id).is_empty()
            })
        };
        // Out of reach.
        let mut reg = world(0.25);
        live(&mut reg, T0);
        crate::crimewatch::flag_criminal(&mut reg, ALICE, T0, T0);
        let mut now = T0;
        let mut locked = false;
        for _ in 0..80 {
            now += SEC / 4;
            step(&mut reg, now);
            locked = standing(&reg)
                .iter()
                .any(|id| !reg.locks().targets_of(*id).is_empty());
            if locked {
                break;
            }
        }
        assert!(locked, "the guns lock Alice first");
        let guns = standing(&reg);
        assert!(guns.iter().all(|id| offender_of(&reg, *id) == Some(ALICE)));
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .position = [0.0, 0.0, 10.0 * crate::grid::GRID_ENTER];
        for _ in 0..4 {
            now += SEC / 4;
            step(&mut reg, now);
        }
        assert!(released(&reg, &guns), "off the grid");
        // The flag lapses.
        let mut reg = world(0.25);
        live(&mut reg, T0);
        crate::crimewatch::flag_criminal(&mut reg, ALICE, T0, T0);
        let mut now = T0;
        for _ in 0..80 {
            now += SEC / 4;
            step(&mut reg, now);
            if standing(&reg)
                .iter()
                .any(|id| !reg.locks().targets_of(*id).is_empty())
            {
                break;
            }
        }
        let guns = standing(&reg);
        now += crate::crimewatch::CRIMINAL_TIMEOUT + 5 * SEC;
        for _ in 0..4 {
            now += SEC / 4;
            step(&mut reg, now);
        }
        assert!(released(&reg, &guns), "the flag ran out");
    }

    /// **The switch waits**: while a sentry has Alice, a nearer criminal does
    /// not take it before [`SWITCH_DELAY`], and does at the delay; and when
    /// the target goes it takes the next at once.
    #[test]
    fn the_switch_waits() {
        let mut reg = world(0.25);
        live(&mut reg, T0);
        // Alice (10 km out) is the nearer to the gate, Bob farther.
        reg.roster_mut()
            .get_mut(BOB)
            .expect("in space")
            .motion
            .position = [60_000.0, 0.0, 0.0];
        crate::crimewatch::flag_criminal(&mut reg, ALICE, T0, T0);
        step(&mut reg, T0 + SEC);
        let gate_guns: Vec<i64> = standing(&reg)
            .into_iter()
            .filter(|id| {
                crate::space_state::distance(
                    reg.roster().get(*id).expect("placed").motion.position,
                    [0.0; 3],
                ) < 20_000.0
            })
            .collect();
        assert_eq!(gate_guns.len() as u32, PER_ANCHOR);
        let taken = T0 + SEC;
        for id in &gate_guns {
            assert_eq!(offender_of(&reg, *id), Some(ALICE));
        }
        // Bob turns criminal and parks on the guns, nearer than Alice.
        crate::crimewatch::flag_criminal(&mut reg, BOB, taken, taken);
        let at = reg
            .roster()
            .get(gate_guns[0])
            .expect("placed")
            .motion
            .position;
        reg.roster_mut()
            .get_mut(BOB)
            .expect("in space")
            .motion
            .position = [at[0] + 100.0, at[1], at[2]];
        step(&mut reg, taken + SWITCH_DELAY - SEC);
        for id in &gate_guns {
            assert_eq!(offender_of(&reg, *id), Some(ALICE), "held before the delay");
        }
        step(&mut reg, taken + SWITCH_DELAY);
        let nearest = gate_guns
            .iter()
            .filter(|id| offender_of(&reg, **id) == Some(BOB))
            .count();
        assert!(nearest >= 1, "the nearest gun switched at the delay");
    }

    /// A target that goes is dropped at once, not after the delay: the next
    /// criminal is taken in the same pass.
    #[test]
    fn a_lost_target_is_replaced_at_once() {
        let mut reg = world(0.25);
        live(&mut reg, T0);
        crate::crimewatch::flag_criminal(&mut reg, ALICE, T0, T0);
        step(&mut reg, T0 + SEC);
        crate::crimewatch::flag_criminal(&mut reg, BOB, T0 + 2 * SEC, T0 + 2 * SEC);
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .position = [0.0, 0.0, 10.0 * crate::grid::GRID_ENTER];
        step(&mut reg, T0 + 3 * SEC);
        assert!(
            standing(&reg)
                .iter()
                .all(|id| offender_of(&reg, *id) == Some(BOB))
        );
    }

    /// A lost sentry is refilled after the wait, not before; a vacant system
    /// loses them; the pass respects `--concord off`.
    #[test]
    fn posts_refill_and_a_vacant_system_loses_them() {
        let mut reg = world(0.25);
        live(&mut reg, T0);
        let first = standing(&reg);
        let lost = first[0];
        crate::npc::despawn_ids(&mut reg, SYS, &[lost]);
        live(&mut reg, T0 + SEC);
        assert_eq!(standing(&reg).len() as u32, 2 * PER_ANCHOR - 1);
        live(&mut reg, T0 + SEC + RESPAWN - 1);
        assert_eq!(standing(&reg).len() as u32, 2 * PER_ANCHOR - 1);
        // The refill is a sweep: within SWEEP of the wait's end.
        live(&mut reg, T0 + SEC + RESPAWN + SWEEP);
        assert_eq!(standing(&reg).len() as u32, 2 * PER_ANCHOR);
        // Everyone leaves.
        for id in [ALICE, BOB] {
            reg.roster_mut().leave(id);
        }
        let t = T0 + SEC + RESPAWN + SWEEP;
        live(&mut reg, t + SEC);
        live(&mut reg, t + SEC + VACANT + SEC);
        assert!(standing(&reg).is_empty());
        assert!(
            reg.npcs()
                .iter()
                .all(|n| n.kind.type_id != FIXTURE_SENTRY.type_id)
        );
        // Off.
        let mut reg = world(0.25);
        crate::concord::set_enabled(&mut reg, false);
        live(&mut reg, T0);
        assert!(standing(&reg).is_empty());
    }
}
