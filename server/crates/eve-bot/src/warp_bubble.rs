//! **A bot held by a warp bubble burns out of it** (BP-HOLD, L2). A probe
//! (`WarpDisrupted`) or a field generator
//! (`WarpScrambledByWarpDisruptionFieldGenerator`) makes the server refuse
//! every warp from inside its sphere. Gates and docks still work. Before
//! this, the refusal counted as a failure of the act (the third sent the bot
//! to `Reconnect`, which the server answers with an emergency warp, so a
//! bot alone in a bubble escaped by logging off), and the field's key even
//! matched [`crate::scram::REFUSALS`] by substring and sent the bot to the
//! scram episode (align home and wait).
//!
//! - **Sense** ([`Sense`]): a refusal in [`REFUSALS`] holds the ship for
//!   [`HOLD`]; the field's key sets [`View::field`]. It is not a failure.
//! - **Zones** ([`View::zones`]): the probe balls in the park (the server's
//!   `bubble::ID_BASE..ID_END` range) with the type's radius (103). A field
//!   moves with its ship, so it has no zone here.
//! - **Response** ([`decide`]), after the scram step: a gate or station
//!   within reach is taken; else align to the known ball that points most
//!   directly out of the sphere (the role's station first), wait, and let the
//!   role try its warp again once the ship is [`EXIT_MARGIN`] outside every
//!   zone, or after [`RETRY`] when no zone is known.
//! - **Never `Reconnect`** while held ([`crate::scram::reconnect_allowed`]).
//!
//! Every number here is **[A]** policy. Falsifiers: bots stuck over
//! [`BUBBLE_MAX`] with nothing on grid (loosen [`EXIT_MARGIN`] / shorten
//! [`RETRY`]); a bot that retries its warp inside the sphere (tighten them).
//! A field's source is not identified, so a bot does not fight the HIC.

use crate::brain::{Act, Ball, Memory, Observation, Place, SHORT_WAIT};
use std::time::{Duration, Instant};

/// The refusal keys of a ship inside a sphere **[V]** (`bubble.rs`).
pub const REFUSALS: [&str; 2] = [
    "WarpDisrupted",
    "WarpScrambledByWarpDisruptionFieldGenerator",
];
/// The field's key **[V]**.
const FIELD_KEY: &str = "WarpScrambledByWarpDisruptionFieldGenerator";
/// `warpScrambleRange` (103) **[V]**: a probe's sphere radius, metres.
pub const ATTRIBUTE_RADIUS: i64 = 103;

/// A refusal holds the ship this long **[A]**; each refusal renews it.
pub const HOLD: Duration = Duration::from_secs(60);
/// The radius of a probe whose type the bundle lacks, metres **[A]**.
/// Falsified by a bot that thinks it is out while still refused.
pub const DEFAULT_RADIUS: f64 = 20_000.0;
/// How far outside the sphere before the warp is tried, metres **[A]**.
pub const EXIT_MARGIN: f64 = 2_000.0;
/// With no zone known, the wait before the role tries again **[A]**.
pub const RETRY: Duration = Duration::from_secs(20);
/// The hold after which the log says the bot is stuck **[A]**.
pub const BUBBLE_MAX: Duration = Duration::from_secs(300);

/// Whether a server error text is a bubble refusal.
pub fn is_refusal(why: &str) -> bool {
    REFUSALS.iter().any(|k| why.contains(k))
}

/// What the brain sees of the bubble this turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct View {
    pub held: bool,
    /// The refusal was a field generator's (moves with its ship).
    pub field: bool,
    /// The probe spheres in the park: `(centre, radius)`.
    pub zones: Vec<([f64; 3], f64)>,
    /// Stargates in the park (balls the route map names).
    pub gates: Vec<Ball>,
}

/// The driver's side: the last refusal.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sense {
    refused: Option<(Instant, bool)>,
}

impl Sense {
    /// A warp was refused with `why` (a [`REFUSALS`] text).
    pub fn refused(&mut self, why: &str, now: Instant) {
        self.refused = Some((now, why.contains(FIELD_KEY)));
    }

    /// `(held, field)` at `now`.
    pub fn state(&self, now: Instant) -> (bool, bool) {
        match self.refused {
            Some((at, field)) if now.saturating_duration_since(at) < HOLD => (true, field),
            _ => (false, false),
        }
    }
}

/// One stay in a bubble.
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub started: Instant,
    pub retry_at: Instant,
    /// The ball the last align was sent to.
    pub aligned: Option<i64>,
    pub stuck_noted: bool,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn norm(v: [f64; 3]) -> Option<[f64; 3]> {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (l > 1.0).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = sub(a, b);
    dot(d, d).sqrt()
}

/// How far outside a known sphere a warp line must pass, metres **[A]**.
/// Falsified by a bot dropped out of warp at a sphere's edge (widen).
pub const AVOID_MARGIN: f64 = 1_000.0;

/// Whether the straight line `a` to `b` passes within `r` of `c` (pure).
pub fn segment_hits(a: [f64; 3], b: [f64; 3], c: [f64; 3], r: f64) -> bool {
    let ab = sub(b, a);
    let len2 = dot(ab, ab);
    let t = if len2 <= 0.0 {
        0.0
    } else {
        (dot(sub(c, a), ab) / len2).clamp(0.0, 1.0)
    };
    let p = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
    dist(p, c) <= r
}

/// Whether a warp from `from` to `to` crosses a known sphere (pure): a sphere
/// that holds `from` already is the held bot's business ([`decide`]), not a
/// warp to avoid.
pub fn crosses(zones: &[([f64; 3], f64)], from: [f64; 3], to: [f64; 3]) -> bool {
    zones.iter().any(|(c, r)| {
        let r = *r + AVOID_MARGIN;
        dist(from, *c) > r && segment_hits(from, to, *c, r)
    })
}

/// **Do not warp into a known bubble** (pure): every `WarpToItem` whose line
/// from the ship to the ball crosses a sphere in [`View::zones`] (the probe
/// spheres and the anchored mobile disruptors, group 361) goes to the ball
/// near the target with a clear line instead (a point off the sphere's line);
/// with none, the ship aligns to the target and waits, to burn out, never
/// into the sphere. Other acts, and warps to balls the park does not show,
/// stay.
pub fn guard(acts: Vec<Act>, o: &Observation) -> Vec<Act> {
    let (Some(park), zones) = (o.park.as_ref(), &o.warp_bubble.zones) else {
        return acts;
    };
    if zones.is_empty() || o.warp_bubble.held {
        return acts;
    }
    let here = park.ego.position;
    let known: Vec<&Ball> = park
        .station
        .iter()
        .chain(park.belts.iter())
        .chain(park.boss.iter())
        .chain(park.rocks.iter())
        .chain(o.warp_bubble.gates.iter())
        .collect();
    acts.into_iter()
        .flat_map(|a| {
            let Act::WarpToItem { id, range } = a else {
                return vec![a];
            };
            let Some(target) = known.iter().find(|b| b.id == id) else {
                return vec![Act::WarpToItem { id, range }];
            };
            if !crosses(zones, here, target.position) {
                return vec![Act::WarpToItem { id, range }];
            }
            let via = known
                .iter()
                .filter(|b| b.id != id && !crosses(zones, here, b.position))
                .filter(|b| {
                    zones
                        .iter()
                        .all(|(c, r)| dist(b.position, *c) > *r + AVOID_MARGIN)
                })
                .min_by(|x, y| {
                    dist(x.position, target.position).total_cmp(&dist(y.position, target.position))
                });
            match via {
                Some(b) => {
                    eprintln!("[warp_bubble] warp to {id} crosses a bubble: via {}", b.id);
                    vec![Act::WarpToItem { id: b.id, range }]
                }
                None => {
                    eprintln!("[warp_bubble] warp to {id} crosses a bubble: burning out");
                    vec![Act::AlignTo(id), Act::Wait(SHORT_WAIT)]
                }
            }
        })
        .collect()
}

/// **The ball to align to** (pure): of the known balls, the one whose
/// direction best matches `outward`; the station when it points out at all.
fn exit_ball(o: &Observation, outward: [f64; 3]) -> Option<i64> {
    let park = o.park.as_ref()?;
    let here = park.ego.position;
    let score = |b: &Ball| norm(sub(b.position, here)).map(|d| dot(d, outward));
    if let Some(s) = park.station.as_ref()
        && score(s).is_some_and(|d| d > 0.0)
    {
        return Some(s.id);
    }
    park.belts
        .iter()
        .chain(park.boss.iter())
        .chain(park.rocks.iter())
        .chain(o.warp_bubble.gates.iter())
        .filter_map(|b| score(b).map(|d| (d, b.id)))
        .filter(|(d, _)| *d > 0.0)
        .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, id)| id)
}

/// **The bubble step** (pure but for the log), asked right after the scram
/// step. `None`: not held, or the role's own act may run this turn.
pub fn decide(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if !matches!(o.place, Place::InSpace { bound: true, .. }) {
        return None;
    }
    let v = &o.warp_bubble;
    if !v.held {
        if let Some(ep) = m.warp_bubble.take() {
            eprintln!(
                "[warp_bubble] {}: free after {} s",
                m.name,
                o.now.saturating_duration_since(ep.started).as_secs()
            );
        }
        return None;
    }
    // A scramble is the scram step's.
    if o.scram.scrambled {
        return None;
    }
    // BP-CAMP: a dictor sits in its own probe's sphere; it stays to camp.
    if m.combat.role == Some(crate::doctrine::Role::Interdict)
        && m.camp.own_probe_alive(o.now)
        && m.homing.is_none()
    {
        return None;
    }
    let park = o.park.as_ref()?;
    let now = o.now;
    let here = park.ego.position;
    let mut ep = m.warp_bubble.take().unwrap_or_else(|| {
        eprintln!("[warp_bubble] {}: held", m.name);
        Episode {
            started: now,
            retry_at: now + RETRY,
            aligned: None,
            stuck_noted: false,
        }
    });
    if !ep.stuck_noted && now.saturating_duration_since(ep.started) >= BUBBLE_MAX {
        ep.stuck_noted = true;
        eprintln!("[warp_bubble] {}: held over the budget", m.name);
    }

    // Gates and docks work inside a bubble.
    let reach = crate::MIN_WARP_DISTANCE;
    let gate = v
        .gates
        .iter()
        .map(|g| (park.ego.surface_to(g), g.id))
        .filter(|(d, _)| *d < reach)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, g)) = gate {
        m.warp_bubble = Some(ep);
        return Some(vec![Act::JumpGate { gate: g }]);
    }
    if let Some(s) = park.station.as_ref()
        && park.ego.surface_to(s) < reach
    {
        m.warp_bubble = Some(ep);
        return Some(vec![Act::Dock(s.id)]);
    }

    // The sphere that holds the ship; out of every one, the warp is the role's.
    let inside = v
        .zones
        .iter()
        .filter(|(c, r)| dist(here, *c) <= *r + EXIT_MARGIN)
        .min_by(|a, b| dist(here, a.0).total_cmp(&dist(here, b.0)));
    if !v.zones.is_empty() && inside.is_none() && !v.field {
        ep.aligned = None;
        m.warp_bubble = Some(ep);
        return None;
    }
    let outward = inside
        .and_then(|(c, _)| norm(sub(here, *c)))
        // A moving field, or no zone: out is wherever the destination is.
        .or_else(|| {
            park.station
                .as_ref()
                .and_then(|s| norm(sub(s.position, here)))
        })
        .unwrap_or([1.0, 0.0, 0.0]);

    // BP-KIT: a micro jump drive carries the ship out in one spool, when the
    // walk is longer than that.
    if let Some(b) = exit_ball(o, outward) {
        let edge = inside.map(|(c, r)| (r + EXIT_MARGIN - dist(here, *c)).max(0.0));
        if let Some(acts) = crate::pvp_kit::bubble_exit(m, o, edge, b) {
            ep.aligned = Some(b);
            m.warp_bubble = Some(ep);
            return Some(acts);
        }
    }

    let mut acts = Vec::new();
    if let Some(b) = exit_ball(o, outward)
        && ep.aligned != Some(b)
    {
        ep.aligned = Some(b);
        acts.push(Act::AlignTo(b));
    }
    // Nothing known to know the sphere: try the role's act after a wait.
    if inside.is_none() && now >= ep.retry_at {
        ep.retry_at = now + RETRY;
        m.warp_bubble = Some(ep);
        return if acts.is_empty() { None } else { Some(acts) };
    }
    acts.push(Act::Wait(SHORT_WAIT));
    m.warp_bubble = Some(ep);
    Some(acts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{ParkView, Policy};

    const SHIP: i64 = 1_000_101;
    const STATION: i64 = 1_000_301;
    const BELT: i64 = 1_000_401;
    const GATE: i64 = 1_000_501;
    const RADIUS: f64 = 10_000.0;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    /// In space at `x`, held by a sphere at the origin.
    fn obs(now: Instant, x: f64) -> Observation {
        let mut o = Observation::empty(now);
        o.place = Place::InSpace {
            system: 1_000_001,
            bound: true,
        };
        o.home = Some(STATION);
        o.park = Some(ParkView {
            ego: ball(SHIP, x),
            ego_warping: false,
            rocks: Vec::new(),
            belts: vec![ball(BELT, -900_000.0)],
            station: Some(ball(STATION, 5_000_000.0)),
            boss: None,
            taken: Default::default(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o.warp_bubble = View {
            held: true,
            field: false,
            zones: vec![([0.0; 3], RADIUS)],
            gates: Vec::new(),
        };
        o
    }

    fn mem() -> Memory {
        Memory::new("Miner1", Policy::fast())
    }

    #[test]
    fn a_disrupted_warp_is_not_a_failure_and_never_reconnects() {
        assert!(is_refusal("x: WarpDisrupted"));
        assert!(!crate::scram::counts("WarpDisrupted"));
        assert!(!crate::scram::is_refusal("WarpDisrupted"));
        let t = Instant::now();
        let m = mem();
        let o = obs(t, 3_000.0);
        assert!(!crate::scram::reconnect_allowed(&m, &o));
        let mut free = obs(t, 3_000.0);
        free.warp_bubble.held = false;
        assert!(crate::scram::reconnect_allowed(&m, &free));
    }

    #[test]
    fn a_field_refusal_is_not_a_scramble() {
        let why = "WarpScrambledByWarpDisruptionFieldGenerator";
        assert!(is_refusal(why));
        assert!(!crate::scram::is_refusal(why));
        let t = Instant::now();
        let mut s = Sense::default();
        s.refused(why, t);
        assert_eq!(s.state(t), (true, true));
        s.refused("WarpDisrupted", t);
        assert_eq!(s.state(t), (true, false));
        assert_eq!(s.state(t + HOLD), (false, false));
    }

    #[test]
    fn a_held_bot_burns_out_then_warps() {
        let t = Instant::now();
        let mut m = mem();
        // Inside: it aligns to the ball pointing out (the station is on the
        // outward side) and waits.
        let acts = decide(&mut m, &obs(t, 3_000.0)).expect("held");
        assert!(acts.contains(&Act::AlignTo(STATION)));
        assert!(matches!(acts.last(), Some(Act::Wait(_))));
        // The align is not repeated.
        let again = decide(&mut m, &obs(t, 4_000.0)).expect("held");
        assert!(!again.iter().any(|a| matches!(a, Act::AlignTo(_))));
        // Outside the sphere and its margin: the role's warp goes.
        let out = obs(t, RADIUS + EXIT_MARGIN + 1.0);
        assert_eq!(decide(&mut m, &out), None);
    }

    #[test]
    fn a_dictor_stays_in_its_own_probe_unless_it_goes_home() {
        let t = Instant::now();
        let mut m = mem();
        m.combat.role = Some(crate::doctrine::Role::Interdict);
        m.camp.launched_at = Some(t);
        let o = obs(t, 3_000.0);
        assert_eq!(decide(&mut m, &o), None, "camping in its own bubble");
        // Losing, it leaves its own bubble like anyone else.
        m.homing = Some(crate::brain::Homing::Repair);
        assert!(decide(&mut m, &o).is_some());
    }

    #[test]
    fn a_held_bot_does_not_align_toward_the_inside() {
        let t = Instant::now();
        let mut m = mem();
        // East of the centre, so out is east; the station and the belt are west.
        let mut o = obs(t, 3_000.0);
        o.park.as_mut().unwrap().station = Some(ball(STATION, -5_000_000.0));
        let acts = decide(&mut m, &o).expect("held");
        // The station is west (inward) and the belt too: nothing points out.
        assert!(!acts.iter().any(|a| matches!(a, Act::AlignTo(_))));
    }

    #[test]
    fn a_held_bot_at_a_gate_jumps() {
        let t = Instant::now();
        let mut m = mem();
        let mut o = obs(t, 3_000.0);
        o.warp_bubble.gates = vec![ball(GATE, 9_000.0)];
        let acts = decide(&mut m, &o).expect("held");
        assert_eq!(acts, vec![Act::JumpGate { gate: GATE }]);
    }

    #[test]
    fn a_field_with_no_zone_waits_then_lets_the_role_retry() {
        let t = Instant::now();
        let mut m = mem();
        let mut o = obs(t, 3_000.0);
        o.warp_bubble.zones.clear();
        o.warp_bubble.field = true;
        let first = decide(&mut m, &o).expect("held");
        assert!(matches!(first.last(), Some(Act::Wait(_))));
        let mut later = o.clone();
        later.now = t + RETRY;
        let acts = decide(&mut m, &later);
        assert!(!acts.is_some_and(|a| a.iter().any(|x| matches!(x, Act::Wait(_)))));
    }

    #[test]
    fn a_scramble_is_the_scram_steps() {
        let t = Instant::now();
        let mut m = mem();
        let mut o = obs(t, 3_000.0);
        o.scram.scrambled = true;
        assert_eq!(decide(&mut m, &o), None);
    }

    #[test]
    fn free_drops_the_episode() {
        let t = Instant::now();
        let mut m = mem();
        decide(&mut m, &obs(t, 3_000.0));
        assert!(m.warp_bubble.is_some());
        let mut o = obs(t, 3_000.0);
        o.warp_bubble.held = false;
        assert_eq!(decide(&mut m, &o), None);
        assert!(m.warp_bubble.is_none());
    }

    #[test]
    fn a_warp_through_a_bubble_goes_by_a_clear_ball() {
        let t = Instant::now();
        // Ship at x=-50 km, a sphere at the origin, the station beyond it.
        let mut o = obs(t, -50_000.0);
        o.warp_bubble.held = false;
        let p = o.park.as_mut().unwrap();
        p.station = Some(ball(STATION, 50_000.0));
        let mut side = ball(BELT + 1, 0.0);
        side.position = [0.0, 80_000.0, 0.0];
        p.belts = vec![side];
        let warp = Act::WarpToItem {
            id: STATION,
            range: 0.0,
        };
        let out = guard(vec![warp.clone(), Act::Wait(SHORT_WAIT)], &o);
        assert_eq!(
            out,
            vec![
                Act::WarpToItem {
                    id: BELT + 1,
                    range: 0.0
                },
                Act::Wait(SHORT_WAIT)
            ]
        );
        // No bubble: untouched.
        o.warp_bubble.zones.clear();
        assert_eq!(guard(vec![warp.clone()], &o), vec![warp]);
    }

    #[test]
    fn with_no_clear_ball_it_burns_out_and_a_line_beside_is_free() {
        let t = Instant::now();
        let mut o = obs(t, -50_000.0);
        o.warp_bubble.held = false;
        let p = o.park.as_mut().unwrap();
        p.station = Some(ball(STATION, 50_000.0));
        p.belts.clear();
        let warp = Act::WarpToItem {
            id: STATION,
            range: 0.0,
        };
        assert_eq!(
            guard(vec![warp.clone()], &o),
            vec![Act::AlignTo(STATION), Act::Wait(SHORT_WAIT)]
        );
        // A station well off the line is not guarded.
        o.park.as_mut().unwrap().station = Some(Ball {
            id: STATION,
            position: [-50_000.0, 90_000.0, 0.0],
            radius: 50.0,
        });
        assert_eq!(guard(vec![warp.clone()], &o), vec![warp]);
        // The segment test itself: stops short of the sphere.
        assert!(!segment_hits(
            [-50_000.0, 0.0, 0.0],
            [-20_000.0, 0.0, 0.0],
            [0.0; 3],
            10_000.0
        ));
        assert!(segment_hits(
            [-50_000.0, 0.0, 0.0],
            [50_000.0, 0.0, 0.0],
            [0.0; 3],
            10_000.0
        ));
    }
}
