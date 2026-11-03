//! **NPC tackle: warp scramblers and stasis webs** (W18 CP49, C6 N6).
//!
//! The spec is `docs/content/R-TACKLE.md` (§7.5 is this module's) and C6
//! N6/R8; marks as `docs/content/README.md` §2. The brain
//! (`crate::npc::brain`) decides *when* an entity cycles its scrambler or
//! web ([`Kind`], one `Intent` per cycle); this module holds what each
//! cycle does, answers the warp guard and the ship's top speed from it, and
//! tells the target's client.
//!
//! # The numbers [V]
//!
//! Read off the client's `dogmaeffects`/`dogmaattributes` (build 3619879,
//! `tools/dogma.py`):
//!
//! - **Scram**: effect `warpScrambleForEntity` (563), guid
//!   `effects.WarpScramble`, duration attribute `warpScrambleDuration` (505,
//!   default 8,000 ms), range `warpScrambleRange` (103); one modifier,
//!   target `warpScrambleStatus` (104) `modAdd` the entity's
//!   `warpScrambleStrength` (105). The usage chance is
//!   `entityWarpScrambleChance` (504). It names no `activationBlocked`
//!   (1349), so **it does not block a microwarpdrive** (R-TACKLE §4.1).
//! - **Web**: effect `modifyTargetSpeed2` (575), guid
//!   `effects.ModifyTargetSpeed`, duration `modifyTargetSpeedDuration` (513,
//!   default 5,000 ms), range `modifyTargetSpeedRange` (514, default
//!   20,000 m); one modifier, target `maxVelocity` (37) `postPercent` (6)
//!   the entity's `speedFactor` (20). The chance is
//!   `modifyTargetSpeedChance` (512). `speedFactor` is **not stackable**, so
//!   two webs on one ship are stacking-penalised, which
//!   [`crate::dogma::apply`] does as the client's `nurfed` does.
//! - `jammingType` (`dogma/effects/__init__.py`, `BUFF_SLOTS_BY_EFFECT_ID`):
//!   563 is `'warpScrambler'` (`:183`), 575 is `'webify'` (`:184`;
//!   `buffBarConst.Slot_Webify`, `buffBarConst.py:15`).
//!
//! # The rules
//!
//! - **Warp** (R-TACKLE §2): `status = own 104 + Σ player 105 + Σ NPC 105`,
//!   refused iff **`> 0`**, strictly (`autopilot.py:590` [I]): an entity
//!   whose strength only *equals* the hull's warp core stability lets it
//!   warp. [`scrammed`] is the `WARP_GUARDS` row. It computes the whole
//!   sum, `crate::tackle::status` included, so a player disruptor and an NPC
//!   scram that each fail alone refuse together (§7.5's concern); it answers
//!   only when an NPC holds, and `tackle::scrammed` answers the rest.
//! - **Gates**: not refused (R-TACKLE §5 [I], as CP57). No `JUMP_GUARDS` row.
//! - **Speed**: `maxVelocity × apply(postPercent 20, penalised)` over every
//!   web holding. [`speed_factor`] is what the client was last told, and the
//!   coordinator's hunk in `dogma::hull` multiplies it in, so the server's
//!   own integration (`advance_character`), the served attributes and every
//!   later `SetMaxSpeed` fly the webbed ship.
//! - **A hold** is one cycle of the effect, from its boundary for its
//!   duration, and counts while the entity is in the target's system, its
//!   lock is in the graph and the target is inside the range (as
//!   `tackle::holds`, centre to centre [A]). **[A] [`SLACK`]**: a cycle
//!   counts a little past its end, so the guard does not open a window
//!   between one cycle's end and the brain's next tick. Falsified by a pilot
//!   who warps off a continuously cycling rat scram.
//!
//! # What the target's client is told (the [`tick`] stage, after `npc`)
//!
//! - `OnSpecialFX` start/stop with the effect's guid on the entity's own
//!   ball (the beam and the combat-log lines, `tactical.py:455-458`),
//!   declared by its grid role (`SpecialFx::grid`, the entity's ball);
//! - `OnEwarStart`/`OnEwarEnd(entity, entity, ship, jammingType)` to the
//!   pilot alone (`crate::tackle::Ewar`, CP57's push; R-TACKLE §6.2).
//!   **[A]** `moduleID` is the entity's ball id, as its gun's FX
//!   (`npc::brain::fx`; R-TACKLE §7.5);
//! - `OnModuleAttributeChange` for 104 when the total moves (§6.3);
//! - when the speed factor moves: `DoDestinyUpdate` `SetMaxSpeed(ship,
//!   maxVelocity)` [V the action, `destiny::Action::SetMaxSpeed`] to the
//!   pilot and everyone in the system whose park holds the ship (the
//!   update's grid role names the ship), and `OnModuleAttributeChange` for
//!   37 to the pilot **[I]**: the client's dogma does not model a remote
//!   effect on its ship, so the server tells it.
//!
//! **K-RATAI** widens the profile to `warpScrambleTargetMWDBlockActivationForEntity`
//! (5928, the numbers of 563), `behaviorWarpScramble` (6745: 2506, 2507,
//! 2509) and `npcBehaviorWebifier` (6743: 2499, 2500, 20). **Not here**: the
//! microwarpdrive block of 5928/6745 (`Scram::blocks_mwd` is read and not
//! acted on) and `behaviorWarpDisrupt` (6744, no type carries it).
//! TP, damps and ECM (N6's second half).
//!
//! # Neutralizers and the NPC capacitor (NEUT-NOS)
//!
//! **Neut** (effects 6691 and 6756, the rows `npc_data::Neut` reads): each
//! cycle takes `amount (97) x weight` from the target's capacitor, where the
//! weight is the falloff (6756 only), the target's `energyWarfareResistance`
//! (2045) and `min(1, targetSig / 2451)` when the rat's type has 2451. The
//! drain is the player-side one (`modules::drain_capacitor_by`); the hold
//! keeps the icon and the beam up like a web's, with guid
//! `effects.EnergyDestabilization` and `jammingType` `ewEnergyNeut` [V].
//!
//! **An NPC has a capacitor** now: attributes 482 and 55 of its type, the
//! hull row `npc::hull` carries, kept where a ship's is
//! (`Modules::capacitor`), recharged by the same formula, drained by a
//! player neutralizer or nosferatu. It pays `dischargeAttributeID` of each of
//! its effects per cycle ([`pay`]): a repairer, a remote repairer, a
//! scrambler, a web and a neut that cannot pay does nothing that cycle
//! **[A]** (no attribute says otherwise; most pay nothing). A type with no
//! 482 has no capacitor to run dry. [`tick`] drops a dead NPC's.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::damage::BlueTime;
use crate::movement_guard::Refusal;
use crate::npc_data::{Needs, Profile};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};

/// Blue ticks per millisecond.
const BLUE_PER_MS: i64 = 10_000;

/// The dogma ids this module reads **[V]** (the module doc names each).
pub mod attr {
    pub const WARP_SCRAMBLE_STATUS: i64 = 104;
    pub const SCRAM_DURATION: i64 = 505;
    pub const MAX_VELOCITY: i64 = 37;
    pub const SPEED_FACTOR: i64 = 20;
    pub const WEB_DURATION: i64 = 513;
    pub const WEB_RANGE: i64 = 514;
}

/// `modifyTargetSpeed2` [V].
pub const EFFECT_WEB: i64 = 575;

/// **[A]** How long past its end a cycle still counts: four world ticks
/// (`push::TICK`), so the guard sees an unbroken scram while the brain
/// renews it on its next tick. Falsified as the module doc says.
pub const SLACK: BlueTime = 1_000 * BLUE_PER_MS;

/// One kind of NPC tackle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Scram,
    Web,
    /// An energy neutralizer (NEUT-NOS).
    Neut,
}

impl Kind {
    /// The effect's guid [V].
    pub fn guid(self) -> &'static str {
        match self {
            Kind::Scram => "effects.WarpScramble",
            Kind::Web => "effects.ModifyTargetSpeed",
            Kind::Neut => "effects.EnergyDestabilization",
        }
    }

    /// `jammingType` [V] (module doc).
    pub fn jamming(self) -> &'static str {
        match self {
            Kind::Scram => "warpScrambler",
            Kind::Web => "webify",
            Kind::Neut => "ewEnergyNeut",
        }
    }

    /// What the brain's per-cycle roll is salted with, so a scram and a web
    /// on the same boundary draw differently.
    pub fn salt(self) -> u64 {
        match self {
            Kind::Scram => 0x5C4A,
            Kind::Web => 0x3EB0,
            Kind::Neut => 0x4E07,
        }
    }
}

/// An entity's scrambler or web, in the server's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effect {
    pub kind: Kind,
    /// Metres (103 or 514), above zero.
    pub range: f64,
    /// A scram's 105 (above zero), or a web's 20 (a percentage, below zero).
    pub strength: f64,
    /// The cycle (505 or 513) in blue ticks, above zero.
    pub period: BlueTime,
    /// The chance a cycle runs (504 or 512), `[0, 1]`. **[I]** C6 R2 names
    /// both "chance"; a cycle that fails its roll does nothing.
    pub chance: f64,
}

fn ticks(ms: f64) -> BlueTime {
    (ms.max(0.0) * BLUE_PER_MS as f64) as BlueTime
}

impl Effect {
    /// The scrambler of a type whose profile has `needs ∋ SCRAM` (effect
    /// 563), its cycle read from `td`. `None` without one, or when its
    /// range, strength or cycle is not above zero.
    pub fn scram(p: &Profile, _td: &crate::typedata::Table) -> Option<Effect> {
        if !p.needs.contains(Needs::SCRAM) {
            return None;
        }
        let s = p.scram?;
        let period = ticks(s.duration_ms);
        (s.range > 0.0 && s.strength > 0.0 && period > 0).then_some(Effect {
            kind: Kind::Scram,
            range: s.range,
            strength: s.strength,
            period,
            chance: s.chance.clamp(0.0, 1.0),
        })
    }

    /// The neutralizer of a type whose profile has one (effect 6691 or
    /// 6756): `strength` is the GJ per cycle, `range` the reach (the optimal
    /// plus [`crate::ewar::FALLOFF_REACH`] falloffs) the brain and the hold
    /// test; the falloff itself is read again when the cycle drains.
    pub fn neut(p: &Profile, _td: &crate::typedata::Table) -> Option<Effect> {
        if !p.needs.contains(Needs::NEUT) {
            return None;
        }
        let n = p.neut?;
        let period = ticks(n.duration_ms);
        (n.range > 0.0 && n.amount > 0.0 && period > 0).then_some(Effect {
            kind: Kind::Neut,
            range: n.range + n.falloff * crate::ewar::FALLOFF_REACH,
            strength: n.amount,
            period,
            chance: n.chance.clamp(0.0, 1.0),
        })
    }

    /// The web of a type whose profile has `needs ∋ WEB` and which carries
    /// effect 575, read from `td`. `None` otherwise, or when it would not
    /// slow anything.
    pub fn web(p: &Profile, td: &crate::typedata::Table) -> Option<Effect> {
        if !p.needs.contains(Needs::WEB) {
            return None;
        }
        if let Some(b) = p.web_behaviour.filter(|_| p.web_chance.is_none()) {
            // Effect 6743, `npcBehaviorWebifier` (K-RATAI): no chance
            // attribute, so every cycle runs ([`crate::npc_data::dogma::BEHAVIOUR_CHANCE`]).
            let period = ticks(b.duration_ms);
            return (b.strength < 0.0 && b.range > 0.0 && period > 0).then_some(Effect {
                kind: Kind::Web,
                range: b.range,
                strength: b.strength,
                period,
                chance: crate::npc_data::dogma::BEHAVIOUR_CHANCE,
            });
        }
        let chance = p.web_chance?;
        if !td.get(p.type_id)?.has_effect(EFFECT_WEB) {
            return None;
        }
        let strength = td.attribute(p.type_id, attr::SPEED_FACTOR);
        let range = td.attribute(p.type_id, attr::WEB_RANGE);
        let period = ticks(td.attribute(p.type_id, attr::WEB_DURATION));
        (strength < 0.0 && range > 0.0 && period > 0).then_some(Effect {
            kind: Kind::Web,
            range,
            strength,
            period,
            chance: chance.clamp(0.0, 1.0),
        })
    }
}

// ---------------------------------------------------------------------------
// The state: what each entity holds, and what each target was told
// ---------------------------------------------------------------------------

/// One cycle of one entity's effect on one ship.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    pub target: i64,
    pub solar_system_id: i64,
    pub type_id: i64,
    pub effect: Effect,
    /// The end of the cycle, on the blue clock.
    pub until: BlueTime,
}

/// What one target's client was last told.
#[derive(Debug, Clone, PartialEq)]
struct Told {
    character_id: i64,
    /// `(entity, kind)` with an icon and a beam up.
    icons: BTreeSet<(i64, Kind)>,
    /// The 104 it was last sent.
    status: f64,
    /// The web factor its `SetMaxSpeed` carries, and that `dogma::hull`
    /// reads ([`speed_factor`]).
    factor: f64,
    /// The `maxVelocity` it was last sent.
    max_velocity: f64,
}

/// **The slot**: every entity's current cycle, and per target ship what its
/// pilot's client was told. Built empty and never persisted: a restart
/// empties every park.
#[derive(Debug, Default)]
pub struct Ewars {
    holds: BTreeMap<(i64, Kind), Hold>,
    told: BTreeMap<i64, Told>,
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "npc_ewar",
    make: |_| Box::new(Ewars::default()) as Boxed,
};

/// **One cycle** of `npc`'s `effect` on `target`, starting on the boundary
/// `at`: from the brain's `Intent::Scram`/`Intent::Web`. A target that is
/// not a pilot in the entity's system is not held.
pub fn cycle(w: &mut Registry, npc: &crate::npc::Npc, effect: &Effect, target: i64, at: BlueTime) {
    let pilot = w
        .roster()
        .by_ship(target)
        .filter(|p| p.solar_system_id == npc.solar_system_id)
        .map(|p| p.character_id);
    if pilot.is_none_or(|c| w.character(c).is_none()) {
        return;
    }
    w.ext_mut::<Ewars>().holds.insert(
        (npc.id, effect.kind),
        Hold {
            target,
            solar_system_id: npc.solar_system_id,
            type_id: npc.kind.type_id,
            effect: *effect,
            until: at.saturating_add(effect.period),
        },
    );
    if effect.kind == Kind::Neut {
        neut_drain(w, npc, target, at);
    }
}

/// The energy a cycle of `npc`'s neutralizer takes from `target`, now.
fn neut_drain(w: &mut Registry, npc: &crate::npc::Npc, target: i64, at: BlueTime) {
    let Some(n) = neut_of(w, npc.kind.type_id) else {
        return;
    };
    let (Some(source), Some(victim)) = (w.roster().get(npc.id), w.roster().by_ship(target)) else {
        return;
    };
    let gap = crate::space_state::distance(source.motion.position, victim.motion.position);
    let reach = crate::ewar::falloff(gap, n.range, n.falloff);
    let live = crate::tackle::Live;
    let resist = crate::ewar::energy_resistance(w, &live, target).max(0.0);
    let resolution = w
        .content()
        .try_get::<crate::typedata::Installed>()
        .map_or(0.0, |td| {
            td.table()
                .attribute(npc.kind.type_id, crate::ewar::attr::NEUT_SIGNATURE)
        });
    let fit = if resolution > 0.0 {
        crate::ewar::signature_fit(crate::ewar::signature_of(w, &live, target), resolution)
    } else {
        1.0
    };
    let amount = n.amount * reach * resist * fit;
    if let Some(push) =
        crate::modules::drain_capacitor_by(w, target, amount, at).and_then(|m| m.push)
    {
        w.schedule(Duration::ZERO, push);
    }
}

/// The type's neutralizer row, from the loaded tables.
fn neut_of(w: &Registry, type_id: i64) -> Option<crate::npc_data::Neut> {
    let table = w.content().try_get::<crate::npc_data::Table>()?;
    let td = w.content().try_get::<crate::typedata::Installed>()?;
    table.profile(td.table(), type_id)?.neut
}

/// **A cycle's capacitor need** (NEUT-NOS): the `dischargeAttributeID` of the
/// first of `effects` the type carries, read off its own attributes; 0 when
/// it has none, or no typedata is loaded.
pub fn cap_need(w: &Registry, type_id: i64, effects: &[i64]) -> f64 {
    let Some(td) = w.content().try_get::<crate::typedata::Installed>() else {
        return 0.0;
    };
    let td = td.table();
    let Some(row) = td.get(type_id) else {
        return 0.0;
    };
    effects
        .iter()
        .filter(|e| row.has_effect(**e))
        .filter_map(|e| td.effect(*e))
        .map(|e| e.discharge_attribute)
        .find(|a| *a > 0)
        .map_or(0.0, |a| td.attribute(type_id, a).max(0.0))
}

/// **An NPC pays for one cycle** of the first of `effects` it carries:
/// `false` (and the cycle does nothing) when its capacitor cannot cover it.
pub fn pay(w: &mut Registry, npc: &crate::npc::Npc, effects: &[i64], at: BlueTime) -> bool {
    let need = cap_need(w, npc.kind.type_id, effects);
    crate::modules::spend_npc_capacitor(w, npc.id, npc.kind.type_id, need, at)
}

/// Whether `h`, held by `npc`, counts at `now`.
fn holds(w: &Registry, npc: i64, h: &Hold, now: BlueTime) -> bool {
    if now >= h.until.saturating_add(SLACK) {
        return false;
    }
    let (Some(source), Some(target)) = (w.roster().get(npc), w.roster().by_ship(h.target)) else {
        return false;
    };
    source.solar_system_id == h.solar_system_id
        && target.solar_system_id == h.solar_system_id
        && w.locks().targets_of(npc).contains(&h.target)
        && crate::space_state::distance(source.motion.position, target.motion.position)
            <= h.effect.range
}

/// Every hold that counts against `ship_id` at `now`, with its entity.
pub fn sources(w: &Registry, ship_id: i64, now: BlueTime) -> Vec<(i64, Hold)> {
    // UNDOCK-INVULN: nothing holds a protected ship.
    if crate::invuln::ship_protected(w, ship_id, now) {
        return Vec::new();
    }
    w.ext::<Ewars>()
        .holds
        .iter()
        .filter(|(_, h)| h.target == ship_id)
        .filter(|((npc, _), h)| holds(w, *npc, h, now))
        .map(|((npc, _), h)| (*npc, *h))
        .collect()
}

/// **The NPC term of `warpScrambleStatus`**: Σ 105 of the scrams holding
/// against `ship_id` at `now`. No penalty, no cap (R-TACKLE §2).
pub fn incoming(w: &Registry, ship_id: i64, now: BlueTime) -> f64 {
    sum(&sources(w, ship_id, now))
}

fn sum(found: &[(i64, Hold)]) -> f64 {
    found
        .iter()
        .filter(|(_, h)| h.effect.kind == Kind::Scram)
        .map(|(_, h)| h.effect.strength)
        .sum()
}

/// The web factor of `found`: 1.0 times each web's `postPercent`, all
/// stacking-penalised (`speedFactor` is not stackable [V]).
fn web_factor(found: &[(i64, Hold)]) -> f64 {
    let webs: Vec<crate::dogma::Incoming> = found
        .iter()
        .filter(|(_, h)| h.effect.kind == Kind::Web)
        .map(|(_, h)| crate::dogma::Incoming {
            op: crate::dogma::op::POST_PERCENT,
            value: h.effect.strength,
            penalised: true,
        })
        .collect();
    crate::dogma::apply(1.0, &webs)
}

/// [`scrammed`] at `now`, over `d` (CP57's reads of the ship's own 104 and
/// the player sources).
pub fn scrammed_by(
    w: &Registry,
    character_id: i64,
    now: BlueTime,
    d: &dyn crate::tackle::Data,
) -> Option<Refusal> {
    let ship_id = w.roster().get(character_id)?.ship_id;
    let npc = incoming(w, ship_id, now);
    (npc > 0.0 && crate::tackle::status(w, ship_id, now, d) + npc > 0.0)
        .then(crate::tackle::warp_refusal)
}

/// **The `WARP_GUARDS` row**: `WarpScrambled` while an NPC scram holds
/// against the pilot's ship and the whole 104 is `> 0`. A pilot not in
/// space is never scrammed; the guard reads the wall clock, as
/// `tackle::scrammed` does.
pub fn scrammed(w: &Registry, character_id: i64) -> Option<Refusal> {
    scrammed_by(
        w,
        character_id,
        crate::pyutil::filetime_now(),
        &crate::tackle::Live,
    )
}

/// **The web factor on `ship_id`**, as its client was last told (1.0
/// unwebbed): what the coordinator's hunk in `dogma::hull` multiplies its
/// `maxVelocity` by.
pub fn speed_factor(w: &Registry, ship_id: i64) -> f64 {
    w.ext::<Ewars>()
        .told
        .get(&ship_id)
        .map_or(1.0, |t| t.factor)
}

/// The ship's unwebbed `maxVelocity`: the dogma sheet's when there is one,
/// the hull table's otherwise.
fn base_max_velocity(w: &Registry, ship_id: i64) -> Option<f64> {
    if let Some(s) = crate::dogma::sheet(w, ship_id) {
        return Some(s.ship(attr::MAX_VELOCITY));
    }
    let p = w.roster().by_ship(ship_id)?;
    let ty = w.pilot(p.character_id)?.ship_type_id;
    crate::destiny::hull::find_in(w.content(), ty).map(|h| f64::from(h.max_velocity))
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The world clock's stage**, after `npc`: [`reconcile`] on the server's
/// own data.
pub fn tick(w: &mut Registry, _: std::time::Instant, now: BlueTime) {
    reconcile(w, now, &crate::tackle::Live);
    // A dead NPC's capacitor goes with it (NEUT-NOS).
    let gone: Vec<i64> = w
        .modules()
        .capacitor_ships()
        .into_iter()
        .filter(|s| (crate::npc::MIN_NPC_ID..crate::npc::END_NPC_ID).contains(s))
        .filter(|s| w.npcs().get(*s).is_none())
        .collect();
    for ship in gone {
        w.modules_mut().forget_capacitor(ship);
    }
}

/// Bring every NPC-tackled pilot's client up to `now`, over `d`; forget the
/// holds that can no longer count.
pub fn reconcile(w: &mut Registry, now: BlueTime, d: &dyn crate::tackle::Data) {
    let live: Vec<(i64, Kind)> = w
        .ext::<Ewars>()
        .holds
        .iter()
        .filter(|((npc, _), h)| {
            now < h.until.saturating_add(SLACK) && w.roster().get(*npc).is_some()
        })
        .map(|(k, _)| *k)
        .collect();
    let slot = w.ext_mut::<Ewars>();
    slot.holds.retain(|k, _| live.contains(k));
    let mut ships: BTreeSet<i64> = slot.holds.values().map(|h| h.target).collect();
    ships.extend(slot.told.keys().copied());
    for ship in ships {
        reconcile_ship(w, ship, now, d);
    }
}

fn reconcile_ship(w: &mut Registry, ship: i64, now: BlueTime, d: &dyn crate::tackle::Data) {
    let pilot = w
        .roster()
        .by_ship(ship)
        .map(|p| p.character_id)
        .filter(|c| w.character(*c).is_some());
    let before = w.ext::<Ewars>().told.get(&ship).cloned();
    let Some(character) = pilot else {
        // Left space (docked, jumped, died, logged off): the icons end; the
        // session change re-primes the ship's attributes and speed.
        if let Some(t) = before {
            for (npc, kind) in &t.icons {
                ewar(w, t.character_id, *npc, ship, *kind, false);
            }
            w.ext_mut::<Ewars>().told.remove(&ship);
        }
        return;
    };
    let found = sources(w, ship, now);
    if found.is_empty() && before.is_none() {
        return;
    }
    let player = crate::tackle::status(w, ship, now, d);
    let base = base_max_velocity(w, ship).unwrap_or(0.0);
    let before = before.unwrap_or(Told {
        character_id: character,
        icons: BTreeSet::new(),
        status: player,
        factor: 1.0,
        max_velocity: base,
    });
    let icons: BTreeSet<(i64, Kind)> = found.iter().map(|(n, h)| (*n, h.effect.kind)).collect();
    let by_icon: BTreeMap<(i64, Kind), Hold> = found
        .iter()
        .map(|(n, h)| ((*n, h.effect.kind), *h))
        .collect();
    for (npc, kind) in icons.difference(&before.icons) {
        if let Some(h) = by_icon.get(&(*npc, *kind)) {
            fx(w, *npc, h, true);
        }
        ewar(w, character, *npc, ship, *kind, true);
    }
    for (npc, kind) in before.icons.difference(&icons) {
        fx_stop(w, *npc, ship, *kind);
        ewar(w, character, *npc, ship, *kind, false);
    }
    let status = player + sum(&found);
    if (status - before.status).abs() > f64::EPSILON {
        attribute(
            w,
            character,
            ship,
            attr::WARP_SCRAMBLE_STATUS,
            status,
            before.status,
            now,
        );
    }
    let factor = web_factor(&found);
    // A player web (K-EWAR) multiplies in too.
    let max_velocity = base * factor * crate::ewar::speed_factor(w, ship);
    if (factor - before.factor).abs() > f64::EPSILON {
        // Integrated to now at the old speed, before the slot changes what
        // `dogma::hull` answers.
        w.advance_character(character);
        speed(w, character, ship, max_velocity);
        attribute(
            w,
            character,
            ship,
            attr::MAX_VELOCITY,
            max_velocity,
            before.max_velocity,
            now,
        );
    }
    let told = &mut w.ext_mut::<Ewars>().told;
    if found.is_empty() {
        told.remove(&ship);
    } else {
        told.insert(
            ship,
            Told {
                character_id: character,
                icons,
                status,
                factor,
                max_velocity,
            },
        );
    }
}

/// Every character in space in `system`: who sees a beam.
fn audience(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

/// The effect's beam and combat-log lines, from the entity's own ball.
fn fx(w: &mut Registry, npc: i64, h: &Hold, start: bool) {
    let to = audience(w, h.solar_system_id);
    if to.is_empty() {
        return;
    }
    w.schedule(
        Duration::ZERO,
        crate::modules::SpecialFx {
            to,
            ship_id: npc,
            // The entity's own ball, as its gun's FX (`npc::brain::fx`).
            module_id: npc,
            type_id: h.type_id,
            target_id: Some(h.target),
            guid: h.effect.kind.guid(),
            is_offensive: true,
            start,
            duration_ms: h.effect.period / BLUE_PER_MS,
            // `[A]` as the gun: one start, repeating, until the stop.
            repeat: 1000,
        },
    );
}

/// The beams of the holds still running at `now` (`to` empty), for a late
/// arrival's replay ([`crate::modules::active_fx`]): the start `fx` sent.
pub fn running_fx(w: &Registry, now: BlueTime) -> Vec<crate::modules::SpecialFx> {
    w.ext::<Ewars>()
        .holds
        .iter()
        .filter(|(_, h)| now < h.until.saturating_add(SLACK))
        .map(|((npc, _), h)| crate::modules::SpecialFx {
            to: Vec::new(),
            ship_id: *npc,
            module_id: *npc,
            type_id: h.type_id,
            target_id: Some(h.target),
            guid: h.effect.kind.guid(),
            is_offensive: true,
            start: true,
            duration_ms: h.effect.period / BLUE_PER_MS,
            repeat: 1000,
        })
        .collect()
}

/// The stop of a beam whose hold is gone: the entity's type from the NPC
/// table, if it is still alive (a dead one's ball took its beam with it).
fn fx_stop(w: &mut Registry, npc: i64, ship: i64, kind: Kind) {
    let Some(n) = w.npcs().get(npc).copied() else {
        return;
    };
    let h = Hold {
        target: ship,
        solar_system_id: n.solar_system_id,
        type_id: n.kind.type_id,
        effect: Effect {
            kind,
            range: 0.0,
            strength: 0.0,
            period: 0,
            chance: 0.0,
        },
        until: 0,
    };
    fx(w, npc, &h, false);
}

/// `OnEwarStart`/`OnEwarEnd` to the target's pilot (CP57's push).
fn ewar(w: &mut Registry, to: i64, npc: i64, ship: i64, kind: Kind, start: bool) {
    w.schedule(
        Duration::ZERO,
        crate::tackle::Ewar {
            to,
            source_ball: npc,
            module_id: npc,
            target_ball: ship,
            jamming: kind.jamming(),
            start,
        },
    );
}

/// `OnModuleAttributeChange` of one ship attribute, to its pilot.
fn attribute(w: &mut Registry, to: i64, ship: i64, id: i64, new: f64, old: f64, now: BlueTime) {
    w.schedule(
        Duration::ZERO,
        crate::modules::ModulePush::Attribute(crate::modules::AttributeChange {
            to: vec![to],
            owner_id: to,
            item_id: ship,
            attribute_id: id,
            time: now,
            new_value: new,
            old_value: old,
        }),
    );
}

/// `SetMaxSpeed(ship, max)` to the pilot and the others in the system; the
/// update's grid role (`ActionUpdate::grid`, the ship's ball) keeps it to
/// the parks that hold the ship.
fn speed(w: &mut Registry, character: i64, ship: i64, max: f64) {
    let Some(here) = w.roster().get(character).copied() else {
        return;
    };
    let mut to = vec![character];
    to.extend(w.roster().others_in_system(here.solar_system_id, character));
    to.retain(|id| w.character(*id).is_some());
    let stamp = w.next_destiny_stamp();
    let action = crate::destiny::Action::SetMaxSpeed { ball: ship, max };
    if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, vec![action]) {
        w.schedule(Duration::ZERO, update);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::Running;
    use crate::npc::{FIXTURE_RAT, Npc};
    use eve_marshal::PyValue;
    use std::time::Instant;

    // Invented ids and round fixture values (lane rule), never a client row.
    const ALICE: i64 = 90_049_001;
    const ALICE_SHIP: i64 = 100_049_001;
    const RIFTER: i64 = 587;
    const SYSTEM: i64 = 30_000_001;
    /// `npc_data::Table::fixture()`'s tackler: `needs` = TURRET,SCRAM.
    const TACKLER: i64 = 990_300_002;
    /// Its turret-only sibling.
    const PLAIN: i64 = 990_300_001;

    /// Invented typedata for the tackler: a scram (563: 103/105/504/505)
    /// and a web (575: 512/20/513/514). **Not the client's numbers.**
    const TYPEDATA: &str = "\
attribute\t114\t0.0\temDamage
type\t990300002\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t10,563,575\t114=2.0;103=10000.0;105=2.0;504=1.0;505=4000.0;512=1.0;20=-40.0;513=4000.0;514=10000.0\tFixture Tackler
type\t990300001\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t10\t114=2.0\tFixture A
";

    fn td() -> crate::typedata::Table {
        crate::typedata::Table::parse(TYPEDATA).expect("the invented typedata parses")
    }

    fn profile(type_id: i64) -> Profile {
        crate::npc_data::Table::fixture()
            .profile(&td(), type_id)
            .expect("a profile")
    }

    fn scram() -> Effect {
        Effect::scram(&profile(TACKLER), &td()).expect("a scrambler")
    }

    fn web() -> Effect {
        Effect::web(&profile(TACKLER), &td()).expect("a web")
    }

    /// The hull's own 104, a parameter of the fixture; no player sources.
    struct Hull(f64);

    impl crate::tackle::Data for Hull {
        fn ship_attr(&self, _: &Registry, ship: i64, a: i64) -> Option<f64> {
            (ship == ALICE_SHIP && a == attr::WARP_SCRAMBLE_STATUS).then_some(self.0)
        }
        fn module_attr(&self, _: &Registry, _: &Running, _: i64) -> Option<f64> {
            None
        }
        fn type_attr(&self, _: &Registry, _: i64, _: i64) -> Option<f64> {
            None
        }
    }

    /// Alice undocked in a Rifter, her park bound; one entity `offset`
    /// metres off, in her park, with her locked.
    fn world(offset: f64) -> (Registry, Npc) {
        world_of(offset, &FIXTURE_RAT, None)
    }

    fn world_of(
        offset: f64,
        kind: &'static crate::npc::NpcType,
        content: Option<crate::content::Content>,
    ) -> (Registry, Npc) {
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ship_type_id: RIFTER,
                ..crate::model::Character::default()
            }],
            ..crate::model::SandboxConfig::default()
        });
        if let Some(c) = content {
            reg = reg.with_content(std::sync::Arc::new(c));
        }
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let at = reg.roster().get(ALICE).expect("in space").motion.position;
        let id = crate::npc::spawn(&mut reg, kind, SYSTEM, [at[0] + offset, at[1], at[2]])
            .expect("an id");
        assert!(reg.locks_mut().add(id, ALICE_SHIP));
        crate::grid::reconcile_in(&mut reg, SYSTEM);
        let _ = reg.take_due(far());
        let npc = *reg.npcs().get(id).expect("spawned");
        (reg, npc)
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(600)
    }

    fn now() -> BlueTime {
        crate::pyutil::filetime_now()
    }

    fn pushes(reg: &mut Registry) -> Vec<(&'static str, Vec<PyValue>)> {
        reg.take_due(far())
            .into_iter()
            .map(|p| p.to_push())
            .collect()
    }

    /// **An NPC with `needs ∋ SCRAM` refuses warp through the guard** when
    /// its strength beats the hull's stability, and **not when it only
    /// equals it**: the rule is strictly `> 0` (R-TACKLE §2,
    /// `autopilot.py:590`), the same strict test CP57 uses, so "strength ≥
    /// stability" in the plan row reads "strength > stability". The refusal
    /// is CP57's `WarpScrambled` with no args [V], and the warp itself is
    /// refused before anything moves.
    #[test]
    fn an_npc_that_needs_scram_refuses_warp_through_the_guard() {
        let p = profile(TACKLER);
        assert!(p.needs.contains(Needs::SCRAM));
        let e = scram();
        assert_eq!((e.kind, e.strength, e.range), (Kind::Scram, 2.0, 10_000.0));

        let (mut reg, npc) = world(e.range / 2.0);
        let t = now();
        cycle(&mut reg, &npc, &e, ALICE_SHIP, t);
        assert_eq!(incoming(&reg, ALICE_SHIP, t), e.strength);

        let stronger = Hull(-e.strength / 2.0);
        let equal = Hull(-e.strength);
        assert_eq!(
            scrammed_by(&reg, ALICE, t, &stronger),
            Some(crate::tackle::warp_refusal())
        );
        assert_eq!(
            crate::tackle::warp_refusal(),
            Refusal {
                user_error: "WarpScrambled",
                args: vec![]
            }
        );
        assert_eq!(scrammed_by(&reg, ALICE, t, &equal), None, "0 is not > 0");

        // Through the guard table and the warp itself.
        fn guard(w: &Registry, c: i64) -> Option<Refusal> {
            scrammed_by(w, c, now(), &Hull(-1.0))
        }
        assert!(crate::movement_guard::check(&[guard], &reg, ALICE).is_some());
        let before = reg.roster().get(ALICE).expect("in space").motion;
        let req = crate::service::Request {
            service: "beyonce",
            method: "CmdWarpToStuff",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        let refused = crate::service::warp_guarded(
            &mut reg,
            &req,
            crate::space_state::WARP_SUBJECT_CHAR.to_vec(),
            Some(&PyValue::Int(npc.id)),
            0.0,
            &[guard],
        );
        assert_eq!(refused, Some(crate::tackle::warp_refusal()));
        assert_eq!(
            reg.roster().get(ALICE).expect("in space").motion.mode,
            before.mode
        );

        // A type without SCRAM in its needs has no scrambler.
        assert_eq!(Effect::scram(&profile(PLAIN), &td()), None);
        let no_scram = Profile {
            needs: Needs::TURRET,
            ..profile(TACKLER)
        };
        assert_eq!(Effect::scram(&no_scram, &td()), None);
    }

    /// A hold counts only while it holds: not once the lock is gone, not
    /// beyond its range, not after its cycle (plus [`SLACK`]).
    #[test]
    fn a_hold_counts_only_while_locked_in_range_and_in_its_cycle() {
        let e = scram();
        let t = now();
        let (mut reg, npc) = world(e.range / 2.0);
        cycle(&mut reg, &npc, &e, ALICE_SHIP, t);
        assert_eq!(incoming(&reg, ALICE_SHIP, t + e.period), e.strength);
        assert_eq!(incoming(&reg, ALICE_SHIP, t + e.period + SLACK), 0.0);
        reg.locks_mut().remove(npc.id, ALICE_SHIP);
        assert_eq!(incoming(&reg, ALICE_SHIP, t), 0.0, "no lock");

        let (mut reg, npc) = world(e.range * 1.5);
        cycle(&mut reg, &npc, &e, ALICE_SHIP, t);
        assert_eq!(incoming(&reg, ALICE_SHIP, t), 0.0, "out of range");
    }

    fn set_max_speed(p: &[(&'static str, Vec<PyValue>)]) -> Vec<f64> {
        p.iter()
            .filter(|(n, _)| *n == "DoDestinyUpdate")
            .flat_map(|(_, args)| match &args[0] {
                PyValue::List(entries) => entries.clone(),
                _ => vec![],
            })
            .filter_map(|e| match e {
                PyValue::Tuple(t) => match &t[1] {
                    PyValue::Tuple(call) if call[0] == crate::pyutil::bstr("SetMaxSpeed") => {
                        match &call[1] {
                            PyValue::Tuple(a) => match (&a[0], &a[1]) {
                                (PyValue::Int(ALICE_SHIP), PyValue::Real(v)) => Some(*v),
                                _ => None,
                            },
                            _ => None,
                        }
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    /// **A web applies the speed factor from the type's attributes** [V]:
    /// `maxVelocity × (1 + speedFactor/100)` (575's `postPercent` of 20),
    /// read off the type. The client is told by `SetMaxSpeed` on Alice's
    /// ball [V the action], and `speed_factor` (what `dogma::hull` reads)
    /// follows; when the web lets go, both go back.
    #[test]
    fn a_web_applies_the_speed_factor_from_the_types_attributes() {
        let e = web();
        let td = td();
        assert_eq!(e.strength, td.attribute(TACKLER, attr::SPEED_FACTOR));
        assert_eq!(e.range, td.attribute(TACKLER, attr::WEB_RANGE));
        let (mut reg, npc) = world(e.range / 2.0);
        let base = f64::from(
            crate::destiny::hull::find_in(reg.content(), RIFTER)
                .expect("a hand hull")
                .max_velocity,
        );
        let t = now();
        cycle(&mut reg, &npc, &e, ALICE_SHIP, t);
        reconcile(&mut reg, t, &Hull(0.0));
        let factor = 1.0 + e.strength / 100.0;
        assert!((speed_factor(&reg, ALICE_SHIP) - factor).abs() < 1e-12);
        let sent = pushes(&mut reg);
        assert_eq!(set_max_speed(&sent), vec![base * factor]);
        assert!(
            sent.iter().any(|(n, a)| *n == "OnModuleAttributeChange"
                && a[2] == PyValue::Int(attr::MAX_VELOCITY))
        );

        reg.locks_mut().remove(npc.id, ALICE_SHIP);
        reconcile(&mut reg, t, &Hull(0.0));
        assert_eq!(speed_factor(&reg, ALICE_SHIP), 1.0);
        assert_eq!(set_max_speed(&pushes(&mut reg)), vec![base]);
    }

    /// Two webs are stacking-penalised (`speedFactor` is not stackable
    /// [V]): the second slows less than its own factor would alone.
    #[test]
    fn two_webs_stack_with_the_penalty() {
        let e = web();
        let (mut reg, a) = world(e.range / 2.0);
        let at = reg.roster().get(a.id).expect("in space").motion.position;
        let b = crate::npc::spawn(&mut reg, &FIXTURE_RAT, SYSTEM, at).expect("an id");
        assert!(reg.locks_mut().add(b, ALICE_SHIP));
        let b = *reg.npcs().get(b).expect("spawned");
        let t = now();
        cycle(&mut reg, &a, &e, ALICE_SHIP, t);
        cycle(&mut reg, &b, &e, ALICE_SHIP, t);
        reconcile(&mut reg, t, &Hull(0.0));
        let one = 1.0 + e.strength / 100.0;
        let both = speed_factor(&reg, ALICE_SHIP);
        assert!(both < one, "two webs slow more than one");
        assert!(both > one * one, "but less than two unpenalised");
    }

    /// **The target is told** (CP57's pushes): the beam and `OnEwarStart`
    /// with `'warpScrambler'` [V] when a scram starts holding, the 104 it
    /// now has; `OnEwarEnd` and the beam's stop when it lets go.
    #[test]
    fn the_target_is_told_when_an_npc_scram_starts_and_ends() {
        let e = scram();
        let (mut reg, npc) = world(e.range / 2.0);
        let t = now();
        cycle(&mut reg, &npc, &e, ALICE_SHIP, t);
        reconcile(&mut reg, t, &Hull(0.0));
        let sent = pushes(&mut reg);
        let start = sent
            .iter()
            .find(|(n, _)| *n == "OnEwarStart")
            .expect("OnEwarStart");
        assert_eq!(
            start.1,
            vec![
                PyValue::Int(npc.id),
                PyValue::Int(npc.id),
                PyValue::Int(ALICE_SHIP),
                crate::pyutil::bstr("warpScrambler"),
            ]
        );
        let fx = sent
            .iter()
            .find(|(n, _)| *n == "OnSpecialFX")
            .expect("the beam");
        assert_eq!(fx.1[5], PyValue::Bytes(b"effects.WarpScramble".to_vec()));
        assert!(sent.iter().any(|(n, a)| *n == "OnModuleAttributeChange"
            && a[2] == PyValue::Int(attr::WARP_SCRAMBLE_STATUS)
            && a[4] == PyValue::Real(e.strength)));
        // A second pass with nothing changed says nothing.
        reconcile(&mut reg, t, &Hull(0.0));
        assert!(pushes(&mut reg).is_empty());

        reg.locks_mut().remove(npc.id, ALICE_SHIP);
        reconcile(&mut reg, t, &Hull(0.0));
        let sent = pushes(&mut reg);
        assert!(
            sent.iter()
                .any(|(n, a)| *n == "OnEwarEnd" && a[3] == crate::pyutil::bstr("warpScrambler"))
        );
        assert!(sent.iter().any(|(n, a)| *n == "OnSpecialFX"
            && a[0] == PyValue::Int(npc.id)
            && a[7] == PyValue::Int(0)));
        assert_eq!(speed_factor(&reg, ALICE_SHIP), 1.0);
    }

    /// A web's icon is `'webify'` [V], and every destiny push this module
    /// sends declares its ball (the grid filter's input).
    #[test]
    fn a_web_is_webify_and_its_destiny_push_names_the_ship() {
        let e = web();
        let (mut reg, npc) = world(e.range / 2.0);
        let t = now();
        cycle(&mut reg, &npc, &e, ALICE_SHIP, t);
        reconcile(&mut reg, t, &Hull(0.0));
        let due = reg.take_due(far());
        let mut destiny = 0;
        for p in &due {
            let (name, args) = p.to_push();
            if name == "OnEwarStart" {
                assert_eq!(args[3], crate::pyutil::bstr("webify"));
            }
            if name == "DoDestinyUpdate" {
                destiny += 1;
                assert_eq!(p.grid(), crate::grid::GridRole::Names(vec![ALICE_SHIP]));
            }
        }
        assert_eq!(destiny, 1);
    }

    // ---- NEUT-NOS ----------------------------------------------------------

    /// Invented rows for a neutralizer rat: a type with effect 6691 (942
    /// cycle, 98 range, 97 amount, 931 chance) beside the tackler's scram,
    /// and a made-up effect (990000001) whose discharge attribute (990001)
    /// is a capacitor need. **Not the client's numbers.**
    const NEUT_ROWS: &str = "\
attribute\t114\t0.0\temDamage
effect\t990000001\t2\t1\t0\t990001\t0\t0\t0\tfixtureNeed\t\t0
type\t990300002\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t10,563,6691,990000001\t114=2.0;103=10000.0;105=2.0;504=1.0;505=4000.0;97=30.0;98=8000.0;931=1.0;942=4000.0;990001=25.0;482=40.0;55=400000.0\tFixture Neuter
";

    fn neut_td() -> crate::typedata::Table {
        crate::typedata::Table::parse(NEUT_ROWS).expect("the invented rows parse")
    }

    fn neuter() -> &'static crate::npc::NpcType {
        static KIND: std::sync::OnceLock<crate::npc::NpcType> = std::sync::OnceLock::new();
        KIND.get_or_init(|| crate::npc::NpcType {
            type_id: TACKLER,
            hull: crate::destiny::hull::Hull {
                type_id: TACKLER,
                ..FIXTURE_RAT.hull
            },
            ..FIXTURE_RAT
        })
    }

    fn neut_content() -> crate::content::Content {
        let mut c = crate::content::Content::fixture();
        c.insert("npc_data", crate::npc_data::Table::fixture());
        c.insert("typedata", crate::typedata::Installed::build(neut_td()));
        c
    }

    fn alice_cap(reg: &Registry, t: BlueTime) -> f64 {
        crate::modules::capacitor_at(reg, ALICE_SHIP, RIFTER, t).0
    }

    /// A type with effect 6691 has `needs ∋ NEUT` and a neutralizer in the
    /// brain's units; one without has none.
    #[test]
    fn effect_6691_is_a_neutralizer() {
        let p = crate::npc_data::Table::fixture()
            .profile(&neut_td(), TACKLER)
            .expect("a profile");
        assert!(p.needs.contains(Needs::NEUT));
        let e = Effect::neut(&p, &neut_td()).expect("a neutralizer");
        assert_eq!(e.kind, Kind::Neut);
        assert_eq!(
            (e.range, e.strength, e.period, e.chance),
            (8_000.0, 30.0, 4_000 * BLUE_PER_MS, 1.0)
        );
        assert_eq!(Kind::Neut.jamming(), "ewEnergyNeut");
        assert_eq!(Kind::Neut.guid(), "effects.EnergyDestabilization");
        assert_eq!(Effect::neut(&profile(PLAIN), &td()), None);
        assert!(crate::npc_gen::SUPPORTED.contains(Needs::NEUT));
    }

    /// One cycle drains the target's capacitor by the type's amount, tells
    /// the pilot (attribute 18, the icon `ewEnergyNeut`, the beam), and the
    /// target's resistance (2045) scales it.
    #[test]
    fn an_npc_neutralizer_drains_the_pilots_capacitor() {
        let p = crate::npc_data::Table::fixture()
            .profile(&neut_td(), TACKLER)
            .expect("a profile");
        let e = Effect::neut(&p, &neut_td()).expect("a neutralizer");
        let (mut reg, npc) = world_of(e.range / 2.0, neuter(), Some(neut_content()));
        let t = now();
        let before = alice_cap(&reg, t);
        reg.modules_mut().set_capacitor(ALICE_SHIP, before, t);
        cycle(&mut reg, &npc, &e, ALICE_SHIP, t);
        reconcile(&mut reg, t, &Hull(0.0));
        let sent = pushes(&mut reg);
        let after = alice_cap(&reg, t);
        assert!(
            (before - after - e.strength).abs() < 1e-9,
            "{before} {after}"
        );
        assert!(sent.iter().any(|(n, a)| *n == "OnModuleAttributeChange"
            && a[2] == PyValue::Int(18)
            && a[4] == PyValue::Real(after)));
        let start = sent
            .iter()
            .find(|(n, _)| *n == "OnEwarStart")
            .expect("OnEwarStart");
        assert_eq!(start.1[3], crate::pyutil::bstr("ewEnergyNeut"));
        let fx = sent
            .iter()
            .find(|(n, _)| *n == "OnSpecialFX")
            .expect("the beam");
        assert_eq!(
            fx.1[5],
            PyValue::Bytes(b"effects.EnergyDestabilization".to_vec())
        );
        // Reconciling again says nothing and drains nothing.
        reconcile(&mut reg, t, &Hull(0.0));
        assert!(pushes(&mut reg).is_empty());
        assert!((alice_cap(&reg, t) - after).abs() < 1e-9);
    }

    /// An NPC pays its effects' discharge attribute from its own
    /// capacitor, and does nothing when it cannot; the capacitor
    /// recharges, and a dead NPC's record is dropped.
    #[test]
    fn an_npc_that_cannot_pay_does_not_cycle() {
        let (mut reg, npc) = world_of(1_000.0, neuter(), Some(neut_content()));
        let t = now();
        let effects = [990_000_001];
        assert_eq!(cap_need(&reg, TACKLER, &effects), 25.0);
        assert_eq!(cap_need(&reg, TACKLER, &[6691]), 0.0, "no discharge");
        // Attributes 482 and 55 of its type, through the hull table.
        let full = crate::modules::capacitor_at(&reg, npc.id, TACKLER, t).1;
        assert_eq!(full, 40.0);
        assert!(pay(&mut reg, &npc, &effects, t));
        assert!(!pay(&mut reg, &npc, &effects, t), "not twice");
        let left = crate::modules::capacitor_at(&reg, npc.id, TACKLER, t).0;
        assert!((left - (full - 25.0)).abs() < 1e-9);
        // A player neut drains it dry, too: the rat cannot pay at all.
        let push = crate::modules::drain_capacitor_by(&mut reg, npc.id, 1_000.0, t);
        assert_eq!(push.map(|m| m.push), Some(None), "no pilot to tell");
        assert!(!pay(&mut reg, &npc, &effects, t));
        // Later it has recharged enough.
        let later = t + 600_000 * BLUE_PER_MS;
        assert!(pay(&mut reg, &npc, &effects, later));
        // A need of zero always pays.
        assert!(pay(&mut reg, &npc, &[6691], t));
        // The record goes with the NPC.
        assert!(reg.modules().charge(npc.id).is_some());
        reg.roster_mut().leave(npc.id);
        reg.npcs_mut().remove(npc.id);
        tick(&mut reg, Instant::now(), later);
        assert!(reg.modules().charge(npc.id).is_none());
    }
}
