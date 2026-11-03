//! What a squadron's non-damage abilities do (FT-F, `docs/lanes/ft-e.md`;
//! spec R-DEEP §2.3, §2.9).
//!
//! **None of the thirteen ability effects has a modifier** [V-data], so each
//! number is a type attribute read here, named in [`attr`].
//!
//! # Mobility, on the squadron itself
//!
//! Afterburner (6440), microwarpdrive (6441), evasive manoeuvres (6439) and
//! the micro jump drive (6442). Each is one cycle of its duration attribute
//! per activation (a cooldown, [`super::ability::Info::one_shot`]). A running
//! one multiplies the squadron's `maxVelocity` and `signatureRadius` by
//! `1 + bonus / 100` (the client's names are `...SpeedBonus` and
//! `...SignatureRadiusBonus`, `abilityAttributes.infoAttributesByDogmaEffect`
//! [V]; the percent reading is **[A]**, the default of the afterburner's
//! `SpeedBonus` being 100 [V-data]). [`mobility`] is what
//! [`super::hull_of`] multiplies in, so the server's own integration, the
//! missile and turret application and the range sweep all see it; the
//! clients are told with `SetMaxSpeed` when it starts and stops. A micro jump
//! drive puts the squadron `fighterAbilityMicroJumpDriveDistance` metres on
//! at the end of its cycle: toward its point or its target, else along its
//! heading **[A]** (the three target modes' meaning for it is not read).
//!
//! # Electronic warfare, on a target
//!
//! Stasis web (6435), warp disruption (6436), energy neutralizer (6434), ECM
//! (6437) and the tackle (6464, a web and a disruption in one). Each cycle of
//! an active ability is **one act**: aggression through crimewatch, then the
//! effect. A web or a disruption is a **hold** of that cycle's length, and a
//! target's web factor and warp-scramble points are the sums of the holds on
//! it, as `crate::npc_ewar`'s are (R-TACKLE §7.5). The numbers are the
//! type's: the web's `...SpeedPenalty`, the points of the disruption and the
//! tackle, the neutralizer's `...Amount`, the ECM's four strengths; the
//! range and falloff attributes are the effect row's. The shapes follow the
//! K-EWAR and NPC paths where the client does not say:
//!
//! - **falloff** `0.5^(((d - range) / falloff)^2)`, none past
//!   [`crate::ewar::FALLOFF_REACH`] falloffs ([`crate::ewar::falloff`]);
//!   a web or a neutralizer or an ECM is weighted by it, a disruption and a
//!   tackle's points are not (they stop at their range) **[A]**;
//! - **resistance**: the type's `...ResistanceID` names an attribute of the
//!   target that multiplies the strength (default 1), as `crate::ewar`
//!   reads `remoteResistanceID` [V];
//! - **webs** are `postPercent` modifiers, stacking-penalised
//!   (`crate::dogma::apply`; `speedFactor` is not stackable [V]);
//! - **ECM** is one roll per cycle: the strength of the type's four that
//!   matches the target's strongest sensor over that sensor, times the
//!   weight (`crate::ewar::jam_chance`'s rule **[A]** for a fighter);
//!   a jam breaks the target's locks and forbids new ones until the cycle's
//!   end;
//! - **the neutralizer** takes `amount x weight` out of a pilot's capacitor
//!   per cycle ([`crate::modules::drain_capacitor`]); an NPC has no
//!   capacitor model and loses nothing.
//!
//! The target's client is told as the other two paths tell it [V shapes]:
//! `OnEwarStart` / `OnEwarEnd(source, module, target, jammingType)` with the
//! client's own strings (`dogma/effects/__init__.py:281-285` [V]: `webify`,
//! `warpScrambler`, `ewEnergyNeut`, `fighterTackle`; an ECM has its
//! `OnJamStart`), `OnModuleAttributeChange` for the speed (37) and the
//! scramble status (104), and `SetMaxSpeed`.
//!
//! **Not here**: `FighterAbilityEWarFailsDueToType`, whose rule is not in the
//! client (no raise site); the target painter, damage boost and the three
//! abilities no shipped type has.

use super::ability::{Active, Cycle, Info};
use super::*;
use crate::damage::Damage;
use crate::dogma::{Incoming, apply, op};
use crate::fighters_data::effect;
use std::collections::BTreeSet;

/// The type attributes of the non-damage abilities [V-data, R-DEEP §2.3].
pub mod attr {
    pub const AB_SPEED: i64 = 2151;
    pub const MWD_SPEED: i64 = 2152;
    pub const MWD_SIGNATURE: i64 = 2153;
    pub const MJD_DISTANCE: i64 = 2154;
    pub const EVASION_SPEED: i64 = 2224;
    pub const EVASION_SIGNATURE: i64 = 2225;
    /// The evasive manoeuvres' resonances: EM, thermal, kinetic, explosive
    /// (`abilityAttributes.infoAttributesByDogmaEffect` order [V]; the ids
    /// [V-data]).
    pub const EVASION_EM: i64 = 2118;
    pub const EVASION_THERMAL: i64 = 2119;
    pub const EVASION_KINETIC: i64 = 2120;
    pub const EVASION_EXPLOSIVE: i64 = 2121;
    pub const WEB_PENALTY: i64 = 2184;
    pub const WEB_RESISTANCE: i64 = 2188;
    pub const DISRUPT_POINTS: i64 = 2205;
    pub const NEUT_RESISTANCE: i64 = 2207;
    pub const NEUT_AMOUNT: i64 = 2211;
    pub const TACKLE_WEB_PENALTY: i64 = 2242;
    pub const TACKLE_POINTS: i64 = 2425;
    pub const ECM_GRAVIMETRIC: i64 = 2246;
    pub const ECM_LADAR: i64 = 2247;
    pub const ECM_MAGNETOMETRIC: i64 = 2248;
    pub const ECM_RADAR: i64 = 2249;
    pub const ECM_RESISTANCE: i64 = 2252;
    /// The target's own: its speed, sensor strengths and the scramble status.
    pub const MAX_VELOCITY: i64 = 37;
    pub const WARP_SCRAMBLE_STATUS: i64 = 104;
}

/// A cycle still counts this long past its end, so a target does not blink
/// between one cycle and the next tick's renewal **[A]**, as
/// [`crate::npc_ewar::SLACK`]. Falsifier: a pilot who warps off a
/// continuously cycling disruption.
pub const SLACK: BlueTime = crate::npc_ewar::SLACK;

/// The roll's salt, so a fighter's ECM stream differs from the others'.
pub const SALT: u64 = 0xF16_7E5;

/// Whether this build runs the effect.
pub fn runs(effect_id: i64) -> bool {
    is_mobility(effect_id) || is_target_ewar(effect_id)
}

pub fn is_mobility(effect_id: i64) -> bool {
    matches!(
        effect_id,
        effect::AFTERBURNER | effect::MICROWARPDRIVE | effect::EVASION | effect::MICRO_JUMP_DRIVE
    )
}

pub fn is_target_ewar(effect_id: i64) -> bool {
    matches!(
        effect_id,
        effect::WEB | effect::WARP_DISRUPT | effect::NEUT | effect::ECM | effect::TACKLE
    )
}

// ---------------------------------------------------------------------------
// Mobility
// ---------------------------------------------------------------------------

/// What a squadron's running mobility abilities multiply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mobility {
    pub speed: f64,
    pub signature: f64,
}

impl Mobility {
    pub const NONE: Mobility = Mobility {
        speed: 1.0,
        signature: 1.0,
    };
}

fn percent(p: f64) -> f64 {
    1.0 + p / 100.0
}

/// The running mobility abilities of squadron `id`, multiplied.
pub fn mobility(w: &Registry, id: i64) -> Mobility {
    let Some(s) = state(w).squadron(id) else {
        return Mobility::NONE;
    };
    let Some(r) = state(w).rt.get(&id) else {
        return Mobility::NONE;
    };
    let a = |attr: i64| super::skills::attr_of(w, id, s.type_id, attr);
    let mut m = Mobility::NONE;
    for active in r.slots.iter().filter_map(|sl| sl.active.as_ref()) {
        match active.effect {
            effect::AFTERBURNER => m.speed *= percent(a(attr::AB_SPEED)),
            effect::MICROWARPDRIVE => {
                m.speed *= percent(a(attr::MWD_SPEED));
                m.signature *= percent(a(attr::MWD_SIGNATURE));
            }
            effect::EVASION => {
                m.speed *= percent(a(attr::EVASION_SPEED));
                m.signature *= percent(a(attr::EVASION_SIGNATURE));
            }
            _ => {}
        }
    }
    m
}

/// **The evasive manoeuvres' resonances** while they run: what the squadron's
/// resonances are multiplied by, `None` when they are not running.
///
/// `[A]` the four attributes multiply **every layer's** resonance of the
/// type: the effect has no modifier in the client's data and its attributes
/// are named as resonances (`fighterAbilityEvasiveManeuvers*Resonance`).
/// Falsifier: a squadron's resistances in the ship info window, or the
/// damage a fighter takes, against the type's resonances times these.
pub fn evasion_resonance(w: &Registry, id: i64) -> Option<Damage> {
    let s = state(w).squadron(id)?;
    let r = state(w).rt.get(&id)?;
    if !r
        .slots
        .iter()
        .filter_map(|sl| sl.active.as_ref())
        .any(|a| a.effect == effect::EVASION)
    {
        return None;
    }
    // A resonance the type does not state (0, the attribute's own default is
    // 1.0 in the client's data) is no change: never immunity.
    let a = |attribute: i64| {
        let v = super::skills::attr_of(w, id, s.type_id, attribute);
        if v > 0.0 { v } else { 1.0 }
    };
    Some(Damage {
        em: a(attr::EVASION_EM),
        thermal: a(attr::EVASION_THERMAL),
        kinetic: a(attr::EVASION_KINETIC),
        explosive: a(attr::EVASION_EXPLOSIVE),
    })
}

/// A squadron's health record takes the profile it has now: its resonances
/// changed (the evasive manoeuvres began or ended) and the damage already
/// taken stays ([`crate::damage::ShipDamage::set_profile`]). A squadron not
/// yet hit has no record and builds its profile at its first hit.
pub(super) fn refresh_profile(w: &mut Registry, id: i64) {
    let Some(profile) = super::ability::profile_of(w, id) else {
        return;
    };
    if let Some(record) = w.combat_mut().ship_mut(id) {
        record.set_profile(profile, crate::pyutil::filetime_now());
    }
}

/// A micro jump drive's landing point: `distance` metres from `from`, toward
/// `toward` when there is one, else along `heading`; never past `toward`.
pub fn jump_point(
    from: [f64; 3],
    toward: Option<[f64; 3]>,
    heading: [f64; 3],
    distance: f64,
) -> Option<[f64; 3]> {
    let dir = match toward {
        Some(t) => [t[0] - from[0], t[1] - from[1], t[2] - from[2]],
        None => heading,
    };
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len <= 0.0 || distance <= 0.0 {
        return None;
    }
    let reach = match toward {
        Some(_) => distance.min(len),
        None => distance,
    };
    Some([
        from[0] + dir[0] / len * reach,
        from[1] + dir[1] / len * reach,
        from[2] + dir[2] / len * reach,
    ])
}

/// A micro jump drive's cycle ended: the squadron is `MJD_DISTANCE` on. The
/// jump is a `SetBallPosition` to every park, which resets nothing and
/// scatters nothing [V `destiny::Action::SetBallPosition`].
pub(super) fn jump(w: &mut Registry, s: &Squadron, a: &Active) {
    let distance = super::skills::attr_of(w, s.id, s.type_id, attr::MJD_DISTANCE);
    w.advance_system(s.solar_system_id);
    let Some(me) = w.roster().get(s.id).copied() else {
        return;
    };
    let toward = a
        .point
        .or_else(|| a.target.and_then(|t| w.ball_position(t)));
    let heading = me.motion.velocity;
    let Some(to) = jump_point(me.motion.position, toward, heading, distance) else {
        return;
    };
    if let Some(p) = w.roster_mut().get_mut(s.id) {
        p.motion.position = to;
    }
    send_actions(
        w,
        Via::Clock,
        s.solar_system_id,
        vec![Action::SetBallPosition {
            ball: s.id,
            point: to,
        }],
    );
}

/// Change something that changes a squadron's top speed (a mobility ability
/// starting or ending): the system is integrated to now at the old speed
/// first, then `f` runs, then everyone is told the new top speed if it moved.
pub(super) fn around_speed(
    w: &mut Registry,
    via: Via,
    s: &Squadron,
    f: impl FnOnce(&mut Registry),
) {
    w.advance_system(s.solar_system_id);
    let old = hull_of(w, s.id).map_or(0.0, |h| f64::from(h.max_velocity));
    f(w);
    // The evasive manoeuvres' resonances (FT-G).
    refresh_profile(w, s.id);
    let new = hull_of(w, s.id).map_or(0.0, |h| f64::from(h.max_velocity));
    if (new - old).abs() > 1e-6 && new > 0.0 {
        send_actions(
            w,
            via,
            s.solar_system_id,
            vec![Action::SetMaxSpeed {
                ball: s.id,
                max: new,
            }],
        );
    }
}

// ---------------------------------------------------------------------------
// The holds
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Web,
    Scram,
    Neut,
    Ecm,
}

/// One cycle's effect on one target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    pub target: i64,
    pub system: i64,
    pub source_type: i64,
    pub jamming: &'static str,
    /// Web: `postPercent` already weighted (negative). Scram: points.
    pub strength: f64,
    pub until: BlueTime,
    pub period: BlueTime,
}

/// What one target's client was last told.
#[derive(Debug, Clone, PartialEq)]
pub struct Told {
    /// The target's roster key: its character, an NPC's own id, a squadron's.
    key: i64,
    pilot: Option<i64>,
    icons: BTreeSet<(i64, &'static str)>,
    factor: f64,
    status: f64,
    /// Jams told: source squadron -> the end of its cycle.
    jams: std::collections::BTreeMap<i64, BlueTime>,
}

/// The world's fighter electronic warfare.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ewar {
    holds: std::collections::BTreeMap<(i64, i64, Kind), Hold>,
    told: std::collections::BTreeMap<i64, Told>,
}

fn ew(w: &Registry) -> &Ewar {
    &state(w).ew
}

fn ew_mut(w: &mut Registry) -> &mut Ewar {
    &mut state_mut(w).ew
}

/// A squadron left (landed, died): its holds end with the next tick.
pub(super) fn source_gone(w: &mut Registry, source: i64) {
    ew_mut(w).holds.retain(|(s, _, _), _| *s != source);
}

/// The holds counting against `ship` at `now`.
fn counting(w: &Registry, ship: i64, now: BlueTime) -> Vec<(i64, Kind, Hold)> {
    ew(w)
        .holds
        .iter()
        .filter(|(_, h)| h.target == ship && h.until + SLACK > now)
        .map(|((src, _, k), h)| (*src, *k, *h))
        .collect()
}

/// **The web factor on `ship`** as its client was last told: 1.0 unwebbed.
/// `ewar::speed_factor` multiplies it in (coordinator).
pub fn web_factor(w: &Registry, ship: i64) -> f64 {
    ew(w).told.get(&ship).map_or(1.0, |t| t.factor)
}

/// The scramble points fighters hold on `ship` at `now`.
pub fn scram_points(w: &Registry, ship: i64, now: BlueTime) -> f64 {
    counting(w, ship, now)
        .iter()
        .filter(|(_, k, _)| *k == Kind::Scram)
        .map(|(_, _, h)| h.strength)
        .sum()
}

/// Whether a fighter's ECM has `ship` jammed at `now`.
pub fn jammed_at(w: &Registry, ship: i64, now: BlueTime) -> bool {
    ew(w)
        .holds
        .values()
        .any(|h| h.target == ship && h.jamming == "electronic" && h.until > now)
}

/// **The `WARP_GUARDS` row** (coordinator): `WarpScrambled` while a
/// fighter's disruption holds against the pilot's ship and the whole 104 is
/// `> 0`: the ship's own, the players' and the NPCs', and the fighters' points
/// together.
pub fn scrammed(w: &Registry, character_id: i64) -> Option<crate::movement_guard::Refusal> {
    let now = crate::pyutil::filetime_now();
    let ship = w.roster().get(character_id)?.ship_id;
    let points = scram_points(w, ship, now);
    (points > 0.0
        && crate::tackle::status(w, ship, now, &crate::tackle::Live)
            + crate::npc_ewar::incoming(w, ship, now)
            + points
            > 0.0)
        .then(crate::tackle::warp_refusal)
}

// ---------------------------------------------------------------------------
// One cycle
// ---------------------------------------------------------------------------

/// A target attribute: its dogma sheet's, else its hull type's.
pub(super) fn target_attr(w: &Registry, ship: i64, attribute: i64) -> Option<f64> {
    if let Some(s) = crate::dogma::sheet(w, ship) {
        return Some(s.ship(attribute));
    }
    let key = w.roster().by_ship(ship)?.character_id;
    let ty = w.pilot(key)?.ship_type_id;
    typedata::attribute_in(w.content(), ty, attribute)
}

/// The weight of an ability on a target `at` metres away: its falloff, and
/// `0` when it does not reach.
pub fn weight(info: &Info, at: f64) -> f64 {
    if info.range > 0.0 {
        crate::ewar::falloff(at, info.range, info.falloff)
    } else {
        1.0
    }
}

/// One cycle of a target ability, at `a.next`. `Ran` when it acted or could
/// not reach; `TargetGone` when the target left.
pub(super) fn cycle(
    w: &mut Registry,
    s: &Squadron,
    a: &Active,
    slot: usize,
    now: BlueTime,
) -> Cycle {
    let Some(target) = a.target else {
        return Cycle::Ran;
    };
    let Some(victim) = w
        .roster()
        .by_ship(target)
        .copied()
        .filter(|p| p.solar_system_id == s.solar_system_id)
    else {
        return Cycle::TargetGone;
    };
    let Some(me) = w.roster().get(s.id).copied() else {
        return Cycle::Ran;
    };
    let Ok(info) = super::ability::info_of(w, s, slot as i64) else {
        return Cycle::Ran;
    };
    let at = crate::space_state::distance(me.motion.position, victim.motion.position);
    let reach = weight(&info, at);
    if reach <= 0.0 {
        return Cycle::Ran;
    }
    // Every number first, while the world is only read.
    let (web_pct, tackle_pct, tackle_points, disrupt_points, neut_amount, ecm_chance) = {
        let value = |attribute: i64| super::skills::attr_of(w, s.id, s.type_id, attribute);
        let resist = |attribute: i64| {
            let id = value(attribute);
            if id > 0.0 {
                target_attr(w, victim.ship_id, id as i64)
                    .unwrap_or(1.0)
                    .max(0.0)
            } else {
                1.0
            }
        };
        let sensors = [
            (crate::ewar::attr::SCAN_RADAR, attr::ECM_RADAR),
            (crate::ewar::attr::SCAN_LADAR, attr::ECM_LADAR),
            (
                crate::ewar::attr::SCAN_MAGNETOMETRIC,
                attr::ECM_MAGNETOMETRIC,
            ),
            (crate::ewar::attr::SCAN_GRAVIMETRIC, attr::ECM_GRAVIMETRIC),
        ];
        let strongest = sensors
            .iter()
            .map(|(sensor, jam)| (target_attr(w, victim.ship_id, *sensor).unwrap_or(0.0), *jam))
            .max_by(|x, y| x.0.total_cmp(&y.0))
            .filter(|(sensor, _)| *sensor > 0.0);
        (
            -value(attr::WEB_PENALTY).abs() * reach * resist(attr::WEB_RESISTANCE),
            // A tackle's web and its points stop at its range.
            -value(attr::TACKLE_WEB_PENALTY).abs(),
            value(attr::TACKLE_POINTS),
            value(attr::DISRUPT_POINTS),
            value(attr::NEUT_AMOUNT) * reach * resist(attr::NEUT_RESISTANCE),
            strongest.map_or(0.0, |(sensor, jam)| {
                (value(jam) / sensor).clamp(0.0, 1.0) * reach * resist(attr::ECM_RESISTANCE)
            }),
        )
    };
    let period = a.duration;
    let until = a.next + period;
    let hold = |jamming: &'static str, strength: f64| Hold {
        target: victim.ship_id,
        system: s.solar_system_id,
        source_type: s.type_id,
        jamming,
        strength,
        until,
        period,
    };
    // Every act is aggression, hit or miss.
    crate::crimewatch::aggression(w, s.owner_id, victim.character_id, a.next, now);
    crate::npc_threat::on_act(w, s.owner_id, victim.character_id, 0.0, a.next);
    let mut put: Vec<(Kind, Hold)> = Vec::new();
    match a.effect {
        effect::WEB => put.push((Kind::Web, hold("webify", web_pct))),
        effect::TACKLE => {
            put.push((Kind::Web, hold("fighterTackle", tackle_pct)));
            put.push((Kind::Scram, hold("fighterTackle", tackle_points)));
        }
        effect::WARP_DISRUPT => put.push((Kind::Scram, hold("warpScrambler", disrupt_points))),
        effect::NEUT => {
            if let Some(p) = crate::modules::drain_capacitor(w, victim.ship_id, neut_amount, a.next)
            {
                w.schedule(Duration::ZERO, p);
            }
            put.push((Kind::Neut, hold("ewEnergyNeut", 0.0)));
        }
        effect::ECM => {
            if crate::drone_skills::roll(s.id, a.cycles as i64, SALT) < ecm_chance {
                put.push((Kind::Ecm, hold("electronic", 0.0)));
            }
        }
        _ => {}
    }
    for (kind, h) in put {
        ew_mut(w).holds.insert((s.id, slot as i64, kind), h);
        if kind == Kind::Ecm {
            jam(w, s, &h, a.next);
        }
    }
    Cycle::Ran
}

/// A jam: every lock the target holds is dropped now, and its pilot's HUD
/// timer starts (`OnJamStart(source, module, target, 'electronic', start,
/// ms)` [V shape], `crate::ewar::JamPush`).
fn jam(w: &mut Registry, s: &Squadron, h: &Hold, at: BlueTime) {
    use crate::targeting::{Reason, TargetChange, What};
    let Some(key) = w.roster().by_ship(h.target).map(|p| p.character_id) else {
        return;
    };
    for t in w.locks().targets_of(h.target) {
        if !w.locks_mut().remove(h.target, t) {
            continue;
        }
        if w.character(key).is_some() {
            w.schedule(
                Duration::ZERO,
                TargetChange {
                    to: vec![key],
                    what: What::Lost,
                    ship: Some(t),
                    reason: Some(Reason::Unlocked),
                },
            );
        }
        if let Some(other) = w
            .roster()
            .by_ship(t)
            .map(|x| x.character_id)
            .filter(|c| w.character(*c).is_some())
        {
            w.schedule(
                Duration::ZERO,
                TargetChange {
                    to: vec![other],
                    what: What::OtherLost,
                    ship: Some(h.target),
                    reason: Some(Reason::Unlocked),
                },
            );
        }
    }
    if w.character(key).is_some() {
        w.schedule(
            Duration::ZERO,
            crate::ewar::JamPush {
                to: key,
                source_ball: s.id,
                module_id: s.id,
                target_ball: h.target,
                jamming: "electronic",
                start: Some((at, h.period / BLUE_PER_MS)),
            },
        );
        let told = ew_mut(w).told.entry(h.target).or_insert_with(|| Told {
            key,
            pilot: Some(key),
            icons: BTreeSet::new(),
            factor: 1.0,
            status: 0.0,
            jams: Default::default(),
        });
        told.jams.insert(s.id, h.until);
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// Bring every affected ship up to `now`: expired holds forgotten, the
/// icons, the speed, the scramble status and the jam timers told.
pub(super) fn reconcile(w: &mut Registry, now: BlueTime) {
    ew_mut(w).holds.retain(|_, h| h.until + SLACK > now);
    let mut ships: BTreeSet<i64> = ew(w).holds.values().map(|h| h.target).collect();
    ships.extend(ew(w).told.keys().copied());
    for ship in ships {
        reconcile_ship(w, ship, now);
    }
}

fn reconcile_ship(w: &mut Registry, ship: i64, now: BlueTime) {
    let before = ew(w).told.get(&ship).cloned();
    let here = w.roster().by_ship(ship).copied();
    let Some(here) = here else {
        // Left space: the icons end; the session change re-primes the rest.
        if let Some(t) = before {
            for (src, jamming) in &t.icons {
                if let Some(pilot) = t.pilot {
                    ewar_push(w, pilot, *src, ship, jamming, false);
                }
            }
            ew_mut(w).told.remove(&ship);
        }
        return;
    };
    let key = here.character_id;
    let pilot = w.character(key).is_some().then_some(key);
    let found = counting(w, ship, now);
    if found.is_empty() && before.is_none() {
        return;
    }
    let mut told = before.clone().unwrap_or(Told {
        key,
        pilot,
        icons: BTreeSet::new(),
        factor: 1.0,
        status: crate::tackle::status(w, ship, now, &crate::tackle::Live)
            + crate::npc_ewar::incoming(w, ship, now),
        jams: Default::default(),
    });
    // Jams that ended: the HUD timer goes.
    let ended: Vec<i64> = told
        .jams
        .iter()
        .filter(|(_, until)| **until <= now)
        .map(|(src, _)| *src)
        .collect();
    for src in ended {
        told.jams.remove(&src);
        if let Some(to) = pilot {
            w.schedule(
                Duration::ZERO,
                crate::ewar::JamPush {
                    to,
                    source_ball: src,
                    module_id: src,
                    target_ball: ship,
                    jamming: "electronic",
                    start: None,
                },
            );
        }
    }
    // The icons.
    let icons: BTreeSet<(i64, &'static str)> = found
        .iter()
        .filter(|(_, k, _)| *k != Kind::Ecm)
        .map(|(src, _, h)| (*src, h.jamming))
        .collect();
    if let Some(to) = pilot {
        for (src, jamming) in icons.difference(&told.icons) {
            ewar_push(w, to, *src, ship, jamming, true);
        }
        for (src, jamming) in told.icons.difference(&icons) {
            ewar_push(w, to, *src, ship, jamming, false);
        }
    }
    told.icons = icons;
    // The scramble status the pilot's autopilot reads.
    let status = crate::tackle::status(w, ship, now, &crate::tackle::Live)
        + crate::npc_ewar::incoming(w, ship, now)
        + found
            .iter()
            .filter(|(_, k, _)| *k == Kind::Scram)
            .map(|(_, _, h)| h.strength)
            .sum::<f64>();
    if let Some(to) = pilot
        && (status - told.status).abs() > f64::EPSILON
    {
        attribute_push(
            w,
            to,
            ship,
            attr::WARP_SCRAMBLE_STATUS,
            status,
            told.status,
            now,
        );
    }
    told.status = status;
    // The web: every web's `postPercent`, stacking-penalised.
    let mods: Vec<Incoming> = found
        .iter()
        .filter(|(_, k, _)| *k == Kind::Web)
        .map(|(_, _, h)| Incoming {
            op: op::POST_PERCENT,
            value: h.strength,
            penalised: true,
        })
        .collect();
    let factor = apply(1.0, &mods);
    let old_factor = told.factor;
    told.pilot = pilot;
    told.key = key;
    if (factor - old_factor).abs() > f64::EPSILON {
        // Integrated to now at the old speed, before the slot changes what
        // `dogma::hull` answers.
        w.advance_system(here.solar_system_id);
    }
    told.factor = factor;
    let idle = found.is_empty() && told.icons.is_empty() && told.jams.is_empty();
    ew_mut(w).told.insert(ship, told);
    if (factor - old_factor).abs() > f64::EPSILON {
        let new_max = max_velocity(w, ship, here.solar_system_id);
        let old_max = if factor > 0.0 {
            new_max / factor * old_factor
        } else {
            new_max
        };
        send_actions(
            w,
            Via::Clock,
            here.solar_system_id,
            vec![Action::SetMaxSpeed {
                ball: ship,
                max: new_max,
            }],
        );
        if let Some(to) = pilot {
            attribute_push(w, to, ship, attr::MAX_VELOCITY, new_max, old_max, now);
        }
    }
    if idle {
        ew_mut(w).told.remove(&ship);
    }
}

/// The ship's top speed with every factor in: its sheet's or hull's.
fn max_velocity(w: &Registry, ship: i64, _system: i64) -> f64 {
    if let Some(h) = crate::dogma::hull(w, ship) {
        return f64::from(h.max_velocity);
    }
    let base = w
        .roster()
        .by_ship(ship)
        .and_then(|p| w.pilot(p.character_id))
        .and_then(|p| crate::destiny::hull::find_in(w.content(), p.ship_type_id))
        .map_or(0.0, |h| f64::from(h.max_velocity));
    base * crate::ewar::speed_factor(w, ship)
}

fn ewar_push(
    w: &mut Registry,
    to: i64,
    source: i64,
    ship: i64,
    jamming: &'static str,
    start: bool,
) {
    w.schedule(
        Duration::ZERO,
        crate::tackle::Ewar {
            to,
            source_ball: source,
            module_id: source,
            target_ball: ship,
            jamming,
            start,
        },
    );
}

fn attribute_push(
    w: &mut Registry,
    to: i64,
    ship: i64,
    id: i64,
    new: f64,
    old: f64,
    now: BlueTime,
) {
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

#[cfg(test)]
mod tests {
    use super::super::ability::tests::ABER;
    use super::super::ability::tests::{
        CAPACITOR, EWARER, JAM_TARGET, JUMP_M, MOVER, NEUT, POINTS, TACKLE_PCT, TACKLER,
        WEB_FALLOFF, WEB_PCT, WEB_RANGE, activate_call, deactivate_call, fly, fly_with_bob, ok_for,
        rat_beyond, refusal_for, velocity_zero,
    };
    use super::*;
    use crate::fighters::tests as ft;
    use crate::fighters::tests::{ALICE, ALICE_SHIP, BOB, BOB_SHIP};
    use crate::pyutil::bstr;

    /// Every `DoDestinyUpdate` among `sent` that names `action`.
    fn destiny_named(sent: &[ft::Sent], action: &str) -> usize {
        sent.iter()
            .filter(|(_, n, a)| {
                *n == "DoDestinyUpdate" && format!("{a:?}").contains(&format!("{:?}", bstr(action)))
            })
            .count()
    }

    fn speed_of(reg: &Registry, sq: i64) -> f64 {
        f64::from(hull_of(reg, sq).expect("a flying squadron").max_velocity)
    }

    fn signature_of(reg: &Registry, sq: i64) -> f64 {
        hull_of(reg, sq)
            .expect("a flying squadron")
            .signature_radius
    }

    const BASE_SPEED: f64 = 3_000.0;
    const BASE_SIGNATURE: f64 = 30.0;
    /// The fixture mover's bonuses, percent: MWD speed and bloom, evasion.
    const MWD_SPEED: f64 = 300.0;
    const MWD_BLOOM: f64 = 400.0;
    const EVASION_SPEED: f64 = -50.0;
    const EVASION_SIGNATURE: f64 = -30.0;
    const AB_SPEED: f64 = 100.0;

    fn tick_at(reg: &mut Registry, t: BlueTime) {
        tick(reg, t);
    }

    // -- mobility -----------------------------------------------------------

    #[test]
    fn mwd_raises_speed_and_signature_for_its_duration() {
        let (mut reg, ids, t0) = fly(&[(MOVER, 6)]);
        let sq = ids[0];
        assert_eq!(speed_of(&reg, sq), BASE_SPEED);
        assert_eq!(signature_of(&reg, sq), BASE_SIGNATURE);
        assert_eq!(activate_call(&mut reg, &ids, 0, &[]), ok_for(&ids));
        assert_eq!(speed_of(&reg, sq), BASE_SPEED * percent(MWD_SPEED));
        assert_eq!(signature_of(&reg, sq), BASE_SIGNATURE * percent(MWD_BLOOM));
        let sent = ft::pending(&mut reg);
        assert_eq!(destiny_named(&sent, "SetMaxSpeed"), 1, "everyone is told");
        let on = &ft::named(&sent, "OnFighterAbilitySlotActivated")[0].2;
        assert_eq!(on[1], PyValue::Int(0));
        // The cycle ends: back to the hull's own, and told again.
        tick_at(&mut reg, t0 + ms(30_000));
        assert_eq!(speed_of(&reg, sq), BASE_SPEED);
        assert_eq!(signature_of(&reg, sq), BASE_SIGNATURE);
        let sent = ft::due(&mut reg);
        assert_eq!(destiny_named(&sent, "SetMaxSpeed"), 1);
        assert_eq!(ft::named(&sent, "OnFighterAbilitySlotDeactivated").len(), 1);
    }

    #[test]
    fn mwd_moves_the_squadron_at_its_new_speed() {
        let (mut reg, ids, _) = fly(&[(MOVER, 6)]);
        let sq = ids[0];
        // The server's own integration reads the same hull the clients are
        // told of: `Registry::mover` through `dogma::hull`.
        let hull = crate::dogma::hull(&reg, sq).expect("dogma's arm for a squadron");
        assert_eq!(f64::from(hull.max_velocity), BASE_SPEED);
        activate_call(&mut reg, &ids, 0, &[]);
        let hull = crate::dogma::hull(&reg, sq).expect("dogma's arm for a squadron");
        assert_eq!(
            f64::from(hull.max_velocity),
            BASE_SPEED * percent(MWD_SPEED)
        );
    }

    #[test]
    fn an_afterburner_and_evasion_use_their_own_attributes() {
        let (mut reg, ids, _) = fly(&[(ABER, 6)]);
        assert_eq!(activate_call(&mut reg, &ids, 0, &[]), ok_for(&ids));
        assert_eq!(speed_of(&reg, ids[0]), BASE_SPEED * percent(AB_SPEED));
        assert_eq!(signature_of(&reg, ids[0]), BASE_SIGNATURE, "no bloom");
        let (mut reg, ids, _) = fly(&[(MOVER, 6)]);
        assert_eq!(activate_call(&mut reg, &ids, 2, &[]), ok_for(&ids));
        assert_eq!(speed_of(&reg, ids[0]), BASE_SPEED * percent(EVASION_SPEED));
        assert_eq!(
            signature_of(&reg, ids[0]),
            BASE_SIGNATURE * percent(EVASION_SIGNATURE)
        );
    }

    #[test]
    fn a_recall_ends_a_mobility_ability_without_its_jump() {
        let (mut reg, ids, t0) = fly(&[(MOVER, 6)]);
        let sq = ids[0];
        velocity_zero(&mut reg, sq);
        let at = reg.roster().get(sq).expect("flying").motion.position;
        let far = PyValue::Tuple(vec![
            PyValue::Real(at[0] + 50_000.0),
            PyValue::Real(at[1]),
            PyValue::Real(at[2]),
        ]);
        assert_eq!(activate_call(&mut reg, &ids, 1, &[far]), ok_for(&ids));
        ft::call(&mut reg, "RecallFightersToTubes", &[ft::ints(&ids)]).expect("recalled");
        let _ = ft::pending(&mut reg);
        tick_at(&mut reg, t0 + ms(10_000));
        let sent = ft::due(&mut reg);
        assert_eq!(destiny_named(&sent, "SetBallPosition"), 0, "no jump");
        assert!(hull_of(&reg, sq).is_none(), "the squadron has landed");
    }

    #[test]
    fn a_micro_jump_drive_puts_the_squadron_on_by_its_distance_when_the_cycle_ends() {
        let (mut reg, ids, t0) = fly(&[(MOVER, 6)]);
        let sq = ids[0];
        velocity_zero(&mut reg, sq);
        let at = reg.roster().get(sq).expect("flying").motion.position;
        let far = PyValue::Tuple(vec![
            PyValue::Real(at[0] + 50_000.0),
            PyValue::Real(at[1]),
            PyValue::Real(at[2]),
        ]);
        assert_eq!(activate_call(&mut reg, &ids, 1, &[far]), ok_for(&ids));
        let _ = ft::pending(&mut reg);
        // Still spooling: nothing has moved yet.
        tick_at(&mut reg, crate::pyutil::filetime_now() + ms(100));
        assert_eq!(reg.roster().get(sq).expect("flying").motion.position, at);
        tick_at(&mut reg, t0 + ms(10_000));
        let to = reg.roster().get(sq).expect("flying").motion.position;
        assert!(
            (to[0] - (at[0] + JUMP_M)).abs() < 1e-6 && to[1] == at[1] && to[2] == at[2],
            "{to:?} from {at:?}"
        );
        let sent = ft::due(&mut reg);
        assert_eq!(destiny_named(&sent, "SetBallPosition"), 1, "{sent:?}");
    }

    #[test]
    fn the_jump_point_never_passes_its_target_and_needs_a_direction() {
        let from = [0.0, 0.0, 0.0];
        assert_eq!(
            jump_point(from, Some([300.0, 0.0, 400.0]), [0.0; 3], 1_000.0),
            Some([300.0, 0.0, 400.0]),
            "a point nearer than the distance is the landing"
        );
        assert_eq!(
            jump_point(from, Some([0.0, 3_000.0, 0.0]), [1.0, 0.0, 0.0], 1_000.0),
            Some([0.0, 1_000.0, 0.0])
        );
        assert_eq!(
            jump_point(from, None, [0.0, 0.0, -2.0], 500.0),
            Some([0.0, 0.0, -500.0]),
            "along the heading"
        );
        assert_eq!(
            jump_point(from, None, [0.0; 3], 500.0),
            None,
            "at rest, no heading"
        );
        assert_eq!(jump_point(from, Some(from), [1.0, 0.0, 0.0], 500.0), None);
    }

    // -- webs and tackles ---------------------------------------------------

    #[test]
    fn web_applies_the_types_penalty_not_the_ships() {
        let (mut reg, ids, t0) = fly(&[(EWARER, 6)]);
        let rat = rat_beyond(&mut reg, ids[0], 3_000.0);
        assert_eq!(crate::ewar::speed_factor(&reg, rat), 1.0);
        let _ = ft::pending(&mut reg);
        assert_eq!(
            activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]),
            ok_for(&ids)
        );
        tick_at(&mut reg, crate::pyutil::filetime_now() + ms(100));
        let factor = crate::ewar::speed_factor(&reg, rat);
        assert!(
            (factor - percent(-WEB_PCT)).abs() < 1e-9,
            "one web: 1 - {WEB_PCT}% = {factor}"
        );
        assert_eq!(web_factor(&reg, rat), factor);
        // It ends with the ability and the slack.
        deactivate_call(&mut reg, &ids, 0);
        tick_at(&mut reg, t0 + ms(60_000));
        assert_eq!(crate::ewar::speed_factor(&reg, rat), 1.0);
        assert_eq!(web_factor(&reg, rat), 1.0);
        assert!(
            state(&reg).ew.told.is_empty() && state(&reg).ew.holds.is_empty(),
            "nothing is left to remember"
        );
    }

    #[test]
    fn a_web_in_falloff_is_weaker_and_out_of_reach_is_refused() {
        let (mut reg, ids, _) = fly(&[(EWARER, 6)]);
        let sq = ids[0];
        velocity_zero(&mut reg, sq);
        // Out to range + 3 falloffs counts; past it does not.
        let far = rat_beyond(&mut reg, sq, WEB_RANGE + 3.0 * WEB_FALLOFF + 1_000.0);
        let a = activate_call(&mut reg, &ids, 0, &[PyValue::Int(far)]);
        assert_eq!(refusal_for(&a, sq), "FighterAbilityEWarFailsDueToRange");
        assert!(
            state(&reg)
                .rt
                .get(&sq)
                .is_some_and(|r| r.slots[0].active.is_none())
        );
        // One falloff past the range: half the penalty.
        let edge = rat_beyond(&mut reg, sq, WEB_RANGE + WEB_FALLOFF);
        assert_eq!(
            activate_call(&mut reg, &ids, 0, &[PyValue::Int(edge)]),
            ok_for(&ids)
        );
        tick_at(&mut reg, crate::pyutil::filetime_now() + ms(100));
        let factor = crate::ewar::speed_factor(&reg, edge);
        assert!(
            (factor - percent(-WEB_PCT * 0.5)).abs() < 1e-9,
            "half the penalty at one falloff: {factor}"
        );
    }

    #[test]
    fn a_pilot_is_told_of_a_web_with_the_clients_own_words() {
        let (mut reg, ids, t0) = fly_with_bob(&[(EWARER, 6)]);
        let _ = ft::pending(&mut reg);
        assert_eq!(
            activate_call(&mut reg, &ids, 0, &[PyValue::Int(BOB_SHIP)]),
            ok_for(&ids)
        );
        tick_at(&mut reg, crate::pyutil::filetime_now() + ms(100));
        let sent = ft::due(&mut reg);
        let start = ft::named(&sent, "OnEwarStart");
        assert_eq!(start.len(), 1, "{sent:?}");
        assert_eq!(start[0].0, Some(vec![BOB]), "the target alone");
        assert_eq!(
            start[0].2,
            vec![
                PyValue::Int(ids[0]),
                PyValue::Int(ids[0]),
                PyValue::Int(BOB_SHIP),
                bstr("webify")
            ]
        );
        assert!(
            sent.iter()
                .any(|(a, n, args)| *n == "OnModuleAttributeChange"
                    && a == &Some(vec![BOB])
                    && args.contains(&PyValue::Int(BOB_SHIP))
                    && args.contains(&PyValue::Int(attr::MAX_VELOCITY))),
            "his speed attribute: {:?}",
            sent.iter().map(|(_, n, _)| n).collect::<Vec<_>>()
        );
        assert_eq!(
            destiny_named(&sent, "SetMaxSpeed"),
            1,
            "the park is told his top speed"
        );
        // The squadron lands: the hold goes at once and the icon ends.
        ft::call(&mut reg, "RecallFightersToTubes", &[ft::ints(&ids)]).expect("recalled");
        tick_at(&mut reg, t0 + ms(60_000));
        let sent = ft::due(&mut reg);
        assert_eq!(ft::named(&sent, "OnEwarEnd").len(), 1, "{sent:?}");
        assert_eq!(crate::ewar::speed_factor(&reg, BOB_SHIP), 1.0);
    }

    #[test]
    fn a_tackle_is_a_web_and_a_disruption_under_one_icon() {
        let (mut reg, ids, _) = fly_with_bob(&[(TACKLER, 6)]);
        let _ = ft::pending(&mut reg);
        assert_eq!(
            activate_call(&mut reg, &ids, 0, &[PyValue::Int(BOB_SHIP)]),
            ok_for(&ids)
        );
        tick_at(&mut reg, crate::pyutil::filetime_now() + ms(100));
        let factor = crate::ewar::speed_factor(&reg, BOB_SHIP);
        assert!((factor - percent(-TACKLE_PCT)).abs() < 1e-9, "{factor}");
        assert!(scrammed(&reg, BOB).is_some(), "and he cannot warp");
        let sent = ft::due(&mut reg);
        let icons = ft::named(&sent, "OnEwarStart");
        assert_eq!(icons.len(), 1, "one icon for the one ability: {sent:?}");
        assert_eq!(icons[0].2[3], bstr("fighterTackle"));
    }

    // -- warp disruption ----------------------------------------------------

    #[test]
    fn warp_disrupt_strength_counts_as_points() {
        let (mut reg, ids, t0) = fly_with_bob(&[(EWARER, 6)]);
        assert!(scrammed(&reg, BOB).is_none());
        let _ = ft::pending(&mut reg);
        assert_eq!(
            activate_call(&mut reg, &ids, 1, &[PyValue::Int(BOB_SHIP)]),
            ok_for(&ids)
        );
        let now = crate::pyutil::filetime_now() + ms(100);
        tick_at(&mut reg, now);
        assert_eq!(scram_points(&reg, BOB_SHIP, now), POINTS);
        let refusal = scrammed(&reg, BOB).expect("a warp under a disruption is refused");
        assert_eq!(refusal.user_error, "WarpScrambled");
        // The warp guard table has the row: the whole refusal path.
        assert!(
            crate::movement_guard::check(&crate::module::plan().warp_guards, &reg, BOB).is_some()
        );
        let sent = ft::due(&mut reg);
        assert_eq!(
            ft::named(&sent, "OnEwarStart")[0].2[3],
            bstr("warpScrambler")
        );
        assert!(
            sent.iter()
                .any(|(a, n, args)| *n == "OnModuleAttributeChange"
                    && a == &Some(vec![BOB])
                    && args.contains(&PyValue::Int(attr::WARP_SCRAMBLE_STATUS))),
            "the scramble status the autopilot reads"
        );
        // Past the cycle and the slack nothing holds.
        assert_eq!(scram_points(&reg, BOB_SHIP, t0 + ms(600_000)), 0.0);
    }

    // -- neutralizer --------------------------------------------------------

    #[test]
    fn a_neutralizer_takes_its_amount_from_a_pilots_capacitor_each_cycle() {
        let (mut reg, ids, _) = fly_with_bob(&[(EWARER, 6)]);
        let now = crate::pyutil::filetime_now();
        let (before, capacity) = crate::modules::capacitor_at(&reg, BOB_SHIP, JAM_TARGET, now);
        assert_eq!(capacity, CAPACITOR);
        assert_eq!(before, CAPACITOR, "full");
        let _ = ft::pending(&mut reg);
        assert_eq!(
            activate_call(&mut reg, &ids, 2, &[PyValue::Int(BOB_SHIP)]),
            ok_for(&ids)
        );
        tick_at(&mut reg, now + ms(100));
        let (after, _) = crate::modules::capacitor_at(&reg, BOB_SHIP, JAM_TARGET, now + ms(100));
        // 3 km is inside the neutralizer's range: the whole amount, once.
        assert!(
            (before - after - NEUT).abs() < 1.0,
            "{before} -> {after}, took {NEUT}"
        );
        let sent = ft::due(&mut reg);
        assert_eq!(
            ft::named(&sent, "OnEwarStart")[0].2[3],
            bstr("ewEnergyNeut")
        );
        assert!(
            sent.iter()
                .any(|(a, n, _)| *n == "OnModuleAttributeChange" && a == &Some(vec![BOB])),
            "his capacitor is told"
        );
    }

    #[test]
    fn a_neutralizer_on_an_npc_takes_nothing_and_does_not_fail() {
        let (mut reg, ids, _) = fly(&[(EWARER, 6)]);
        let rat = rat_beyond(&mut reg, ids[0], 3_000.0);
        assert_eq!(
            activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]),
            ok_for(&ids)
        );
        tick_at(&mut reg, crate::pyutil::filetime_now() + ms(100));
        assert!(
            state(&reg)
                .rt
                .get(&ids[0])
                .is_some_and(|r| r.slots[2].active.is_some())
        );
    }

    // -- ECM ----------------------------------------------------------------

    #[test]
    fn ecm_uses_the_sensor_strength_attrs() {
        let (mut reg, ids, _) = fly_with_bob(&[(TACKLER, 6)]);
        // Bob's radar is 4 and the fixture's strength of that type 4: a
        // certain jam. He holds a lock.
        let rat = rat_beyond(&mut reg, ids[0], 6_000.0);
        assert!(reg.locks_mut().add(BOB_SHIP, rat));
        let _ = ft::pending(&mut reg);
        assert_eq!(
            activate_call(&mut reg, &ids, 1, &[PyValue::Int(BOB_SHIP)]),
            ok_for(&ids)
        );
        let now = crate::pyutil::filetime_now() + ms(100);
        tick_at(&mut reg, now);
        assert!(jammed_at(&reg, BOB_SHIP, now));
        assert!(
            crate::ewar::jammed_at(&reg, BOB_SHIP, now),
            "the targeting seam reads it through ewar"
        );
        assert!(
            reg.locks().targets_of(BOB_SHIP).is_empty(),
            "his locks broke"
        );
        let sent = ft::due(&mut reg);
        let start = ft::named(&sent, "OnJamStart");
        assert_eq!(start.len(), 1, "{sent:?}");
        assert_eq!(start[0].0, Some(vec![BOB]));
        assert_eq!(
            start[0].2[..4],
            [
                PyValue::Int(ids[0]),
                PyValue::Int(ids[0]),
                PyValue::Int(BOB_SHIP),
                bstr("electronic")
            ]
        );
        // The cycle ends: the timer goes.
        assert!(!jammed_at(&reg, BOB_SHIP, now + ms(10_000)));
        deactivate_call(&mut reg, &ids, 1);
        tick_at(&mut reg, now + ms(10_000));
        let sent = ft::due(&mut reg);
        assert_eq!(ft::named(&sent, "OnJamEnd").len(), 1, "{sent:?}");
    }

    #[test]
    fn an_ecm_cannot_jam_what_has_no_sensor() {
        let (mut reg, ids, _) = fly(&[(TACKLER, 6)]);
        let rat = rat_beyond(&mut reg, ids[0], 3_000.0);
        assert_eq!(
            activate_call(&mut reg, &ids, 1, &[PyValue::Int(rat)]),
            ok_for(&ids)
        );
        let now = crate::pyutil::filetime_now() + ms(100);
        tick_at(&mut reg, now);
        assert!(
            !jammed_at(&reg, rat, now),
            "the rat's type has no strength to read"
        );
    }

    #[test]
    fn an_ability_this_build_does_not_run_is_refused_in_words() {
        let (mut reg, ids, _) = fly(&[(super::super::ability::tests::PLAIN, 6)]);
        // The bomb is run now (FT-G, `strike.rs`); the plain type's bomb slot
        // has no duration to time it with, which is refused in words too.
        let a = activate_call(&mut reg, &ids, 1, &[]);
        assert_eq!(refusal_for(&a, ids[0]), "CustomNotify");
        const NOBODY_RUNS: i64 = 990_999;
        assert!(!super::super::ability::supported(NOBODY_RUNS));
        assert!(super::super::ability::supported(effect::BOMB));
        assert!(runs(effect::TACKLE) && runs(effect::MICRO_JUMP_DRIVE));
    }

    #[test]
    fn the_guards_and_hooks_are_in_the_tables() {
        assert!(
            crate::movement_guard::WARP_GUARDS
                .iter()
                .any(|g| *g as *const () == scrammed as *const ()),
            "the warp table carries the fighters row"
        );
        let _ = ALICE;
        let _ = ALICE_SHIP;
    }

    // -- FT-G: the evasive manoeuvres' resonances ------------------------------

    fn profile_now(reg: &Registry, sq: i64) -> crate::damage::Profile {
        super::super::ability::profile_of(reg, sq).expect("a flying squadron")
    }

    #[test]
    fn evasion_halves_every_resonance_while_it_runs_and_no_longer() {
        use crate::fighters::fixture::{EVADER, EVASION_RESONANCE};
        let (mut reg, ids, t0) = fly(&[(EVADER, 6)]);
        let sq = ids[0];
        let plain = profile_now(&reg, sq);
        assert_eq!(plain.shield_resonance.em, 1.0);
        assert_eq!(activate_call(&mut reg, &ids, 0, &[]), ok_for(&ids));
        for layer in [
            profile_now(&reg, sq).shield_resonance,
            profile_now(&reg, sq).armor_resonance,
            profile_now(&reg, sq).hull_resonance,
        ] {
            for r in [layer.em, layer.thermal, layer.kinetic, layer.explosive] {
                assert_eq!(r, EVASION_RESONANCE);
            }
        }
        assert_eq!(
            profile_now(&reg, sq).shield_capacity,
            plain.shield_capacity,
            "only the resonances move"
        );
        // It runs its cycle out and the resonances are the type's again.
        tick(&mut reg, t0 + ms(60_000));
        assert_eq!(profile_now(&reg, sq), plain);
    }

    /// **The setter**: a squadron already hurt keeps what it has lost, and
    /// takes what comes next at the new resonances.
    #[test]
    fn evasion_changes_the_health_record_without_healing_it() {
        use crate::damage::{Damage, ShipDamage};
        use crate::fighters::fixture::{EVADER, EVASION_RESONANCE};
        let (mut reg, ids, t0) = fly(&[(EVADER, 6)]);
        let sq = ids[0];
        let now = crate::pyutil::filetime_now();
        let before = profile_now(&reg, sq);
        reg.combat_mut().set_ship(sq, ShipDamage::full(before, now));
        let em = |points: f64| Damage {
            em: points,
            ..Damage::ZERO
        };
        reg.combat_mut()
            .ship_mut(sq)
            .expect("a record")
            .apply(em(10.0), now);
        let lost = |reg: &Registry, at: BlueTime| {
            before.shield_capacity - reg.combat().ship(sq).expect("a record").shield_at(at)
        };
        assert!((lost(&reg, now) - 10.0).abs() < 1e-6);
        assert_eq!(activate_call(&mut reg, &ids, 0, &[]), ok_for(&ids));
        {
            let rec = reg.combat().ship(sq).expect("a record");
            assert_eq!(rec.profile().shield_resonance.em, EVASION_RESONANCE);
            assert_eq!(rec.profile().shield_capacity, before.shield_capacity);
        }
        assert!(
            (lost(&reg, now) - 10.0).abs() < 1e-6,
            "nothing healed or hurt by the change"
        );
        // 20 points at one half: 10 more.
        reg.combat_mut()
            .ship_mut(sq)
            .expect("a record")
            .apply(em(20.0), now);
        assert!((lost(&reg, now) - 20.0).abs() < 1e-6, "{}", lost(&reg, now));
        // The cycle ends: the record is the type's again, the loss stays.
        tick(&mut reg, t0 + ms(60_000));
        let rec = reg.combat().ship(sq).expect("a record");
        assert_eq!(rec.profile().shield_resonance.em, 1.0);
    }

    #[test]
    fn a_resonance_the_type_does_not_state_is_not_immunity() {
        // The mover's evasion states speed and signature and no resonances.
        let (mut reg, ids, _) = fly(&[(MOVER, 6)]);
        assert_eq!(activate_call(&mut reg, &ids, 2, &[]), ok_for(&ids));
        assert_eq!(profile_now(&reg, ids[0]).shield_resonance.em, 1.0);
    }
}
