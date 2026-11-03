//! **Warp bubbles: the Interdiction Sphere Launcher's probe and the Heavy
//! Interdictor's Warp Disruption Field Generator** (lane BUBBLES-SB, issue
//! #24; spec `handback/r-pvp-spec/report.md` §4).
//!
//! A bubble is a sphere in a solar system. A ship whose centre is inside one
//! may not warp, and a ship that warps *through* one lands at its edge (the
//! drag). Three things make them:
//!
//! - **A mobile disruptor** (issue #172, NW-3, [`Source::Mobile`]): an
//!   anchored Mobile Warp Disruptor of `crate::anchor`, a sphere of its own
//!   type's `warpScrambleRange` for as long as it stands anchored. It has no
//!   state here: [`zones`] reads `anchor::anchored`. Free, anchoring and
//!   unanchoring units hold nobody; a unit shot dead lets go at once.
//!
//! - **The probe.** An Interdiction Sphere Launcher (group 589) loaded with a
//!   Warp Disrupt Probe (group 548) launches it on activation. It is a
//!   ball that stays where it was launched, with radius `warpScrambleRange`
//!   (103) and a life of `explosionDelay` (281) ms. It is a **local** ball:
//!   [`balls_in`] is one line in `destiny::scene` and `grid::local_balls`.
//!   Its slim carries `warpDisruptionStartTime`, which `ScannerProbe.Assemble`
//!   reads to scale the model and play the deploy curve **[V]**
//!   (`spaceObject/scannerProbe.py`; spec §4.1).
//! - **The field.** A Warp Disruption Field Generator (effect 3380
//!   `warpDisruptSphere`) running on a Heavy Interdictor is a sphere of
//!   radius 103 centred on that ship, for as long as the module runs. It has
//!   no ball (the client draws it from the effect's guid, and
//!   `WarpDisruptFieldGenerating` takes its radius from the module type when
//!   `graphicInfo` is `None` **[V]**), so it is derived from the running
//!   modules each time and needs no state.
//!
//! # Rules
//!
//! - **Inside** ([`inside`], a `WARP_GUARDS` row): `WarpDisrupted{object,
//!   range}` for a probe, `WarpScrambledByWarpDisruptionFieldGenerator{pilot,
//!   ship, range}` for a field **[V keys, spec §4.1]**. A ship with
//!   `warpBubbleImmune` (1538) non-zero is never held. A field never holds its
//!   own ship.
//! - **Drag** ([`drag`], called by `service::space::warp_guarded`): see there.
//! - **Launch**: refused where the type's `disallowInEmpireSpace` (1074) is 1
//!   and the system is high or low security.
//! - Stargate jumps and docking from inside a bubble are allowed.
//!
//! # Assumed, with a falsifier each
//!
//! - **[A]** the probe appears at the launching ship's position and stays.
//!   Falsifier: TQ throws it ahead of the ship.
//! - **[A]** distance is centre to centre, not to the hull's surface.
//!   Falsifier: TQ frees a battleship whose hull edge is outside the sphere.
//! - **[A]** the drag rule: the warp line from the start to the destination
//!   enters a bubble not containing the ship, the entry point is within
//!   [`DRAG_REACH`] of the destination, and the destination is not itself
//!   inside that bubble. The ship lands at the entry point. Falsifier: a TQ
//!   pilot dragged by a bubble more than 500 km from where he was going, or
//!   not dragged by one 300 km off the gate on his line, or dragged when he
//!   warps *into* a bubble.
//! - **[A]** the empire refusal's key (`CustomNotify`): no clear UserError
//!   name was found. Falsifier: a `findings/user-errors.tsv` key for it.
//! - **[A]** the launch needs the launcher's charge skills, as a survey probe
//!   does. Falsifier: TQ launches a probe without them.
//! - **[A]** the HIC's field is an aggression: each cycle starts the weapons
//!   and PvP timers against each pilot it holds ([`field_aggression`]).
//!   Falsifier: TQ gives a HIC no weapons timer for a bubble on a quiet
//!   gate.
//! - **A focused script** (group 908, `crate::tackle::focus`) turns the
//!   module into a targeted point: [`zones`] skips it, `tackle` counts it.

use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION, Slim};
use crate::modules::{Running, type_arg, type_arg_for, user_error};
use crate::movement_guard::Refusal;
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::time::Duration;

/// `groupInterdictionSphereLauncher` and `groupWarpDisruptProbe` **[V]**
/// (`inventorycommon/const`).
pub const GROUP_LAUNCHER: i64 = 589;
pub const GROUP_PROBE: i64 = 548;

/// `warpDisruptSphere` **[V-data]**: the field generator's effect.
pub const EFFECT_FIELD: i64 = 3_380;

/// `warpScrambleRange` **[V-data]**: the sphere's radius, metres.
pub const ATTR_RANGE: i64 = 103;
/// `explosionDelay` **[V-data]**: the probe's life, ms.
pub const ATTR_LIFE: i64 = 281;
/// `disallowInEmpireSpace` **[V-data]**.
pub const ATTR_NO_EMPIRE: i64 = 1_074;
/// `warpBubbleImmune` **[V]** (`dogma/const`): 1 on an interdiction nullifier.
pub const ATTR_IMMUNE: i64 = 1_538;

/// How far from the destination a bubble on the warp line may be and still
/// drag the ship (the "on the destination grid" approximation, **[A]**).
pub const DRAG_REACH: f64 = 500_000.0;

/// The refusal keys **[V]** (`findings/user-errors.tsv`; `UI/Messages`).
pub const WARP_DISRUPTED: &str = "WarpDisrupted";
pub const WARP_SCRAMBLED_BY_FIELD: &str = "WarpScrambledByWarpDisruptionFieldGenerator";

/// The radius of a probe whose type the table lacks, metres (the ball's, not
/// the bubble's).
const BALL_RADIUS: f64 = 100.0;

/// Probe ball ids: above the cynosural field and survey probe range and
/// below the NPC orders' base (`econ_exchange::NPC_ORDER_BASE`) **[A]**; a
/// test lists the other ranges.
pub const ID_BASE: i64 = 9_900_000_000_000;
pub const ID_END: i64 = 10_000_000_000_000;

/// `MSEC` in blue ticks **[V]** (`appConst`).
const TICKS_PER_MS: i64 = 10_000;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// One probe in space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    pub ball_id: i64,
    /// The probe's type (the charge).
    pub type_id: i64,
    pub owner: i64,
    pub corporation_id: i64,
    pub system: i64,
    pub centre: [f64; 3],
    pub radius: f64,
    /// Blue time of the launch: the slim's `warpDisruptionStartTime`.
    pub launched: i64,
    pub expiry: i64,
}

/// Every probe out. Nothing persists: a restart empties space.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bubbles {
    probes: Vec<Probe>,
    next: i64,
    /// Field generator module -> the cycles already counted as aggression.
    field_cycles: std::collections::BTreeMap<i64, i64>,
}

impl Bubbles {
    fn mint(&mut self) -> i64 {
        let span = ID_END - ID_BASE;
        loop {
            let id = ID_BASE + self.next.rem_euclid(span);
            self.next = (self.next + 1).rem_euclid(span);
            if !self.probes.iter().any(|p| p.ball_id == id) {
                return id;
            }
        }
    }
}

/// The world slot (coordinator: `world_ext::SLOTS`).
pub const SLOT: SlotSpec = SlotSpec {
    name: "bubble",
    make: |_| Box::new(Bubbles::default()) as Boxed,
};

/// Every probe out.
pub fn probes(w: &Registry) -> &[Probe] {
    &w.ext::<Bubbles>().probes
}

// ---------------------------------------------------------------------------
// The spheres
// ---------------------------------------------------------------------------

/// What makes a sphere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    /// A probe: its type.
    Probe { type_id: i64, ball_id: i64 },
    /// An anchored mobile disruptor (`crate::anchor`): its type and ball.
    Mobile { type_id: i64, ball_id: i64 },
    /// A field generator on `ship`, flown by `character`, of ship type
    /// `ship_type`.
    Field {
        ship: i64,
        character: i64,
        ship_type: i64,
    },
}

/// One sphere now in a system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    pub centre: [f64; 3],
    pub radius: f64,
    pub source: Source,
}

impl Zone {
    fn holds(&self, at: [f64; 3]) -> bool {
        crate::space_state::distance(self.centre, at) <= self.radius
    }
}

/// The module's range: the dogma sheet's value (a Heavy Interdictor's skills
/// and the hull bonus), else the type's own.
fn field_range(w: &Registry, r: &Running) -> Option<f64> {
    use crate::tackle::Data;
    crate::tackle::Live
        .module_attr(w, r, ATTR_RANGE)
        .filter(|x| *x > 0.0)
}

/// **Every sphere in `system`**: the probes out, and one per running field
/// generator whose ship is in space there. Positions are as the roster last
/// integrated them.
pub fn zones(w: &Registry, system: i64) -> Vec<Zone> {
    let mut out: Vec<Zone> = probes(w)
        .iter()
        .filter(|p| p.system == system)
        .map(|p| Zone {
            centre: p.centre,
            radius: p.radius,
            source: Source::Probe {
                type_id: p.type_id,
                ball_id: p.ball_id,
            },
        })
        .collect();
    // NW-3: a mobile disruptor holds once it is anchored, with the radius
    // of its own type.
    out.extend(
        crate::anchor::anchored(w, system)
            .into_iter()
            .filter_map(|u| {
                // Only the warp-disruptor group makes a bubble (the cyno
                // inhibitor is anchored here too and must not).
                if crate::scan::group_of(w.content(), u.type_id)
                    != Some(crate::anchor::GROUP_MOBILE_DISRUPTOR)
                {
                    return None;
                }
                let radius = crate::typedata::attribute_in(w.content(), u.type_id, ATTR_RANGE)
                    .filter(|r| *r > 0.0)?;
                Some(Zone {
                    centre: u.centre,
                    radius,
                    source: Source::Mobile {
                        type_id: u.type_id,
                        ball_id: u.ball_id,
                    },
                })
            }),
    );
    for r in w.modules().running_all() {
        // A focused script (HIC-SCRIPT) turns the module into a targeted
        // point: no sphere.
        if r.effect.effect_id != EFFECT_FIELD || crate::tackle::is_focused(&r.effect) {
            continue;
        }
        let Some(p) = w.roster().by_ship(r.ship_id) else {
            continue;
        };
        if p.solar_system_id != system {
            continue;
        }
        let Some(radius) = field_range(w, &r) else {
            continue;
        };
        out.push(Zone {
            centre: p.motion.position,
            radius,
            source: Source::Field {
                ship: r.ship_id,
                character: r.character_id,
                ship_type: w.store().get(r.ship_id).map_or(0, |i| i.type_id),
            },
        });
    }
    out
}

/// Whether `ship_id` is immune to bubbles (`warpBubbleImmune` non-zero).
pub fn immune(w: &Registry, ship_id: i64) -> bool {
    crate::dogma::sheet(w, ship_id).is_some_and(|s| s.ship(ATTR_IMMUNE) != 0.0)
}

/// Whether a zone binds `ship_id`: not its own field.
fn binds(z: &Zone, ship_id: i64) -> bool {
    !matches!(z.source, Source::Field { ship, .. } if ship == ship_id)
}

// ---------------------------------------------------------------------------
// The refusal
// ---------------------------------------------------------------------------

/// The refusal a ship inside `zone` is given.
pub fn refusal_of(zone: &Zone) -> Refusal {
    match zone.source {
        Source::Probe { type_id, .. } | Source::Mobile { type_id, .. } => Refusal {
            user_error: WARP_DISRUPTED,
            args: crate::pyutil::pypairs(vec![
                ("object", type_arg(type_id)),
                ("range", PyValue::Real(zone.radius)),
            ]),
        },
        Source::Field {
            character,
            ship_type,
            ..
        } => Refusal {
            user_error: WARP_SCRAMBLED_BY_FIELD,
            args: crate::pyutil::pypairs(vec![
                (
                    "pilot",
                    PyValue::Tuple(vec![
                        PyValue::Int(crate::damage::UE_OWNERID),
                        PyValue::Int(character),
                    ]),
                ),
                (
                    "ship",
                    type_arg_for(
                        "WarpScrambledByWarpDisruptionFieldGenerator",
                        "ship",
                        ship_type,
                    ),
                ),
                ("range", PyValue::Real(zone.radius)),
            ]),
        },
    }
}

/// **The `WARP_GUARDS` row**: the pilot's ship centre is inside a sphere
/// that binds it, and it is not immune.
pub fn inside(w: &Registry, character_id: i64) -> Option<Refusal> {
    let p = w.roster().get(character_id)?;
    if immune(w, p.ship_id) {
        return None;
    }
    zones(w, p.solar_system_id)
        .iter()
        .find(|z| binds(z, p.ship_id) && z.holds(p.motion.position))
        .map(refusal_of)
}

// ---------------------------------------------------------------------------
// The drag
// ---------------------------------------------------------------------------

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Where the segment `from` to `to` first enters the sphere `(centre,
/// radius)`: `None` when it never does, or when `from` is already inside.
fn entry(from: [f64; 3], to: [f64; 3], centre: [f64; 3], radius: f64) -> Option<[f64; 3]> {
    let d = sub(to, from);
    let len2 = dot(d, d);
    if len2 <= 0.0 {
        return None;
    }
    let f = sub(from, centre);
    let c = dot(f, f) - radius * radius;
    if c <= 0.0 {
        return None;
    }
    let b = dot(f, d);
    let disc = b * b - len2 * c;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - disc.sqrt()) / len2;
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    Some([from[0] + d[0] * t, from[1] + d[1] * t, from[2] + d[2] * t])
}

/// **The drag**: where a warp from `from` to `destination` really ends, if a
/// bubble takes it. `Some(entry point)` for the nearest bubble the line
/// enters (module header **[A]**), `None` when the warp is not dragged.
pub fn drag_in(
    zones: &[Zone],
    ship_id: i64,
    from: [f64; 3],
    destination: [f64; 3],
) -> Option<[f64; 3]> {
    zones
        .iter()
        .filter(|z| binds(z, ship_id) && !z.holds(destination))
        .filter_map(|z| entry(from, destination, z.centre, z.radius))
        .filter(|e| crate::space_state::distance(*e, destination) <= DRAG_REACH)
        .min_by(|a, b| {
            crate::space_state::distance(from, *a)
                .total_cmp(&crate::space_state::distance(from, *b))
        })
}

/// [`drag_in`] for a pilot's warp: `aim` and `standoff` are the warp's own,
/// and the destination the line really runs to is the arrival point.
/// `None` for a pilot not in space, an immune ship, or a warp no bubble
/// takes.
pub fn drag(w: &Registry, character_id: i64, aim: [f64; 3], standoff: f64) -> Option<[f64; 3]> {
    let p = w.roster().get(character_id)?;
    if immune(w, p.ship_id) {
        return None;
    }
    let all = zones(w, p.solar_system_id);
    if all.is_empty() {
        return None;
    }
    let destination = crate::destiny::warp_arrival(p.motion.position, aim, standoff);
    drag_in(&all, p.ship_id, p.motion.position, destination)
}

// ---------------------------------------------------------------------------
// Launching
// ---------------------------------------------------------------------------

fn refuse(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// Whether `type_id` is an Interdiction Sphere Launcher.
pub fn is_launcher(w: &Registry, type_id: i64) -> bool {
    crate::scan::group_of(w.content(), type_id) == Some(GROUP_LAUNCHER)
}

/// Whether `type_id` is a Warp Disrupt Probe.
pub fn is_probe(w: &Registry, type_id: i64) -> bool {
    crate::scan::group_of(w.content(), type_id) == Some(GROUP_PROBE)
}

/// How many cycles a module may repeat: one for a sphere launcher (a probe
/// leaves on activation, a second cycle would spend a round for nothing).
pub fn cycles(w: &Registry, module_type: i64, repeat: i64) -> i64 {
    if is_launcher(w, module_type) {
        1
    } else {
        repeat
    }
}

/// Whether the system forbids what `module_type` makes: its 1074 is 1 and
/// the system is high or low security. A system the table lacks forbids
/// nothing.
pub(crate) fn forbidden_here(w: &Registry, module_type: i64, system: i64) -> bool {
    let banned = crate::typedata::attribute_in(w.content(), module_type, ATTR_NO_EMPIRE)
        .is_some_and(|v| v == 1.0);
    banned
        && matches!(
            w.content().get::<crate::celestial::Universe>().band(system),
            Some(crate::celestial::Band::High | crate::celestial::Band::Low)
        )
}

/// **The checks a bubble module's activation owes** (`modules::activate`,
/// for a module found fitted, online and loaded): a launcher takes only
/// probes and the pilot needs their skills; both launcher and field
/// generator are refused in empire space and in a warp. Any other module
/// passes untouched.
pub fn check_activation(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    module_type: i64,
    effect_id: i64,
    flag_id: i64,
) -> Result<(), ServiceError> {
    let launcher = is_launcher(w, module_type);
    if !launcher && effect_id != EFFECT_FIELD {
        return Ok(());
    }
    w.advance_character(character_id);
    let Some(here) = w.roster().get(character_id).copied() else {
        return Ok(());
    };
    if here.motion.warp_active() {
        return Err(user_error("DeniedActivateInWarp", Vec::new()));
    }
    if forbidden_here(w, module_type, here.solar_system_id) {
        return Err(refuse(
            "Warp bubbles cannot be used in high or low security space.",
        ));
    }
    if launcher && let Some(charge) = crate::modules::loaded(w.store(), ship_id, flag_id) {
        if !is_probe(w, charge.type_id) {
            return Err(refuse("That launcher takes warp disrupt probes."));
        }
        if let Some((skill, level)) =
            crate::drone_skills::missing_skill(w, character_id, charge.type_id)
        {
            let name = crate::item::find_in(w.content(), skill).map_or("a skill", |t| t.name);
            return Err(refuse(&format!(
                "Warp disrupt probes need {name} at level {level}."
            )));
        }
    }
    Ok(())
}

/// **A launcher went active**: the round is spent (`crate::modules` took it)
/// and the probe is out. Returns the probe's ball id, or `None` for a module
/// that is no launcher, a charge that is no probe, or a pilot not in space.
pub fn on_activated(
    w: &mut Registry,
    character_id: i64,
    module_type: i64,
    charge_type: Option<i64>,
    now: i64,
) -> Option<i64> {
    if !is_launcher(w, module_type) {
        return None;
    }
    let charge_type = charge_type.filter(|t| is_probe(w, *t))?;
    w.advance_character(character_id);
    let here = w.roster().get(character_id).copied()?;
    let radius =
        crate::typedata::attribute_in(w.content(), charge_type, ATTR_RANGE).filter(|r| *r > 0.0)?;
    let life_ms = crate::typedata::attribute_in(w.content(), charge_type, ATTR_LIFE)
        .filter(|d| *d > 0.0)? as i64;
    let corporation_id = w.character(character_id).map_or(0, |c| c.corporation_id);
    let ball_id = w.ext_mut::<Bubbles>().mint();
    let probe = Probe {
        ball_id,
        type_id: charge_type,
        owner: character_id,
        corporation_id,
        system: here.solar_system_id,
        centre: here.motion.position,
        radius,
        launched: now,
        expiry: now + life_ms * TICKS_PER_MS,
    };
    w.ext_mut::<Bubbles>().probes.push(probe);
    announce_added(w, &probe);
    Some(ball_id)
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **Take down every probe whose time is up** (`modules::tick`): the ball
/// leaves every park of its system, on the clock.
pub fn tick(w: &mut Registry, now: i64) {
    field_aggression(w, now);
    if w.ext::<Bubbles>().probes.is_empty() {
        return;
    }
    let (gone, keep): (Vec<Probe>, Vec<Probe>) = std::mem::take(&mut w.ext_mut::<Bubbles>().probes)
        .into_iter()
        .partition(|p| p.expiry <= now);
    w.ext_mut::<Bubbles>().probes = keep;
    for p in gone {
        let to = audience(w, p.system);
        let stamp = w.next_destiny_stamp();
        if let Some(push) = crate::destiny::BallsRemoved::new(to, stamp, vec![p.ball_id]) {
            w.schedule(Duration::ZERO, push);
        }
    }
}

/// **A field is aggression** (HIC-SCRIPT): every cycle of a running field
/// generator is an act by its pilot against each pilot it holds, the weapons
/// timer that keeps a HIC on the gate (`crate::tackle::reconcile` does the
/// same for a point). Counted once per cycle per module.
///
/// **[A]** a field is offensive (its effect row says `isOffensive`) and a
/// pilot it holds is its target (spec §4.2 "an offensive effect, so start a
/// crimewatch check per area"). Falsifier: TQ gives a HIC no weapons timer
/// for a bubble on a quiet gate.
fn field_aggression(w: &mut Registry, now: i64) {
    let running: Vec<Running> = w
        .modules()
        .running_all()
        .into_iter()
        .filter(|r| r.effect.effect_id == EFFECT_FIELD && !crate::tackle::is_focused(&r.effect))
        .collect();
    if running.is_empty() && w.ext::<Bubbles>().field_cycles.is_empty() {
        return;
    }
    let live: std::collections::BTreeSet<i64> = running.iter().map(|r| r.module_id).collect();
    w.ext_mut::<Bubbles>()
        .field_cycles
        .retain(|m, _| live.contains(m));
    for r in running {
        let seen = w
            .ext::<Bubbles>()
            .field_cycles
            .get(&r.module_id)
            .copied()
            .unwrap_or(0);
        if r.cycles <= seen {
            continue;
        }
        w.ext_mut::<Bubbles>()
            .field_cycles
            .insert(r.module_id, r.cycles);
        let Some(source) = w.roster().by_ship(r.ship_id).copied() else {
            continue;
        };
        let Some(radius) = field_range(w, &r) else {
            continue;
        };
        let held: Vec<i64> = w
            .roster()
            .in_system(source.solar_system_id)
            .into_iter()
            .filter(|p| p.ship_id != r.ship_id && p.character_id != r.character_id)
            .filter(|p| !immune(w, p.ship_id))
            .filter(|p| {
                crate::space_state::distance(source.motion.position, p.motion.position) <= radius
            })
            .map(|p| p.character_id)
            .collect();
        for victim in held {
            crate::crimewatch::aggression(w, r.character_id, victim, now, now);
        }
    }
}

// ---------------------------------------------------------------------------
// The ball
// ---------------------------------------------------------------------------

/// Every character in space in `system`.
fn audience(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

/// The probe's ball: a body at rest, no mass, not free.
pub fn ball(w: &Registry, p: &Probe) -> Ball {
    let radius = crate::typedata::installed_in(w.content())
        .table()
        .get(p.type_id)
        .map(|r| r.radius)
        .filter(|r| *r > 0.0)
        .unwrap_or(BALL_RADIUS);
    Ball {
        ball_id: p.ball_id,
        mode: Mode::Stop,
        radius: radius as f32,
        position: p.centre,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: i32::try_from(p.corporation_id).unwrap_or(-1),
        alliance_id: -1,
        physics: None,
        formation_id: NO_FORMATION,
    }
}

/// The probe's slim: its type, its pilot as owner and corp, no `charID`.
pub fn slim(p: &Probe) -> Slim {
    Slim {
        item_id: p.ball_id,
        type_id: p.type_id,
        owner_id: p.owner,
        char_id: None,
        corp_id: Some(p.corporation_id),
        response_thresholds: None,
    }
}

/// `system`'s probes as balls and slims, for `destiny::scene` and
/// `grid::local_balls`.
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    probes(w)
        .iter()
        .filter(|p| p.system == system)
        .map(|p| (ball(w, p), slim(p)))
        .collect()
}

/// `warpDisruptionStartTime` for the probes among `ids` (`slim_extra::SOURCES`).
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    probes(w)
        .iter()
        .filter(|p| ids.contains(&p.ball_id))
        .map(|p| {
            (
                p.ball_id,
                vec![("warpDisruptionStartTime", PyValue::Int(p.launched))],
            )
        })
        .collect()
}

/// Where the probe `ball_id` is, for `Registry::ball_target`: approach and
/// orbit it. **[A]** no warp-in point, as a cynosural field's.
pub fn target(w: &Registry, ball_id: i64) -> Option<crate::model::Target> {
    probes(w)
        .iter()
        .find(|p| p.ball_id == ball_id)
        .map(|p| crate::model::Target {
            position: p.centre,
            radius: ball(w, p).radius as f64,
            warp_in: None,
        })
}

/// `AddBalls2` of a new probe to the pilots in its system (the grid's
/// filter then keeps it to those on the grid).
fn announce_added(w: &mut Registry, p: &Probe) {
    let to = audience(w, p.system);
    if to.is_empty() {
        return;
    }
    let stamp = w.next_destiny_stamp();
    let added = crate::destiny::BallsAdded::new(
        to,
        stamp,
        crate::destiny::Park {
            current_time: stamp,
            balls: vec![ball(w, p)],
        },
        vec![(slim(p), None)],
    );
    if let Some(a) = added {
        let extras = crate::slim_extra::for_slims(w, &[p.ball_id]);
        w.push(a.with_extras(extras));
    }
}

#[cfg(test)]
mod tests;
