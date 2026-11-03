//! **A pilot's capacitor sense** (BOT-CAP). The server enforces
//! `NotEnoughEnergy` (MODULE-RULES), stops a module whose capacitor runs
//! dry mid-run, and NPC neutralizers drain a pilot (NEUT-NOS). A bot with no
//! capacitor sense would lose its guns and lasers and re-activate against
//! the refusal every turn; this module is the sense.
//!
//! - **Tracking.** The server pushes `OnModuleAttributeChange(owner, ship,
//!   18, time, new, old, wallclock)` on every change of the ship's charge
//!   (attribute 18) **[V]** (`modules::AttributeChange`). [`Cap`] keeps the
//!   last value and when it arrived, and projects the recharge from it with
//!   the client's own curve, `C (1 + (sqrt(c0/C) - 1) e^(-5 t/T))^2`, from
//!   the hull's capacity (482) and recharge time (55), read once per ship
//!   off its `ItemGetInfo` row. A bot that could not read them keeps the
//!   last pushed value for [`READ_STALE`] and then knows nothing **[A]**
//!   (falsifier: a bot that stays held with a full capacitor).
//! - **Refusals.** `NotEnoughEnergy`, `ModuleReactivationDelayed2` and
//!   `EffectCrowdedOut` on an activation are not failures and not retried at
//!   once: [`Watch::refused`] holds activations for a back-off that doubles
//!   from [`BACKOFF_BASE`] to [`BACKOFF_MAX`] **[A]**, and for an energy
//!   refusal at least until the capacitor is projected back to
//!   [`RESUME_AT`]. The delay's own field is not read **[A]** (the server's
//!   unit for it is itself an assumption, `modules::check_reactivation`).
//! - **Self-drain.** Below [`DRY_AT`] the bot is *drained* and stays so until
//!   [`RESUME_AT`] (a hysteresis); while drained, or held, [`gate`] takes
//!   `Mine` and `Fire` out of the turn's acts.
//! - **A neut on grid.** `OnEwarStart(source ball, module, target ship,
//!   "ewEnergyNeut")` aimed at this ship names the NPC; `OnEwarEnd` or
//!   [`NEUT_STALE`] clears it. A drained ratter with one on
//!   grid ([`answer`]) kills it when it can, else goes home.
//!
//! Every number here is **[A]**: falsified by a human round in which a
//! bot's guns sit idle with a charged capacitor (too eager to hold), or
//! stay dry-fired against a neut for minutes (too slow to leave).

use crate::brain::{Act, Homing, Memory, Observation, ParkView, SHORT_WAIT};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The ship's current capacitor charge, GJ **[V]**.
pub const ATTR_CHARGE: i64 = 18;
/// Capacitor recharge time, milliseconds **[V]**.
pub const ATTR_RECHARGE: i64 = 55;
/// Capacitor capacity, GJ **[V]**.
pub const ATTR_CAPACITY: i64 = 482;
/// `jammingType` of a neutralizer's `OnEwarStart` **[V]**.
pub const NEUT_TYPE: &str = "ewEnergyNeut";

/// Below this fraction of the capacitor the bot is drained **[A]**.
pub const DRY_AT: f64 = 0.10;
/// A drained bot (or one refused for energy) is back at this fraction **[A]**.
pub const RESUME_AT: f64 = 0.35;
/// The first back-off after a refusal, doubled by each one in a row **[A]**.
pub const BACKOFF_BASE: Duration = Duration::from_secs(4);
/// The back-off's ceiling **[A]**.
pub const BACKOFF_MAX: Duration = Duration::from_secs(64);
/// A neutralizer is believed this long after its start **[A]**: the server
/// tells a client once per start (`tackle::Told`), so this only backstops a
/// lost `OnEwarEnd`; the brain also needs the NPC on the grid.
pub const NEUT_STALE: Duration = Duration::from_secs(300);
/// Without the hull's numbers a pushed charge is believed this long **[A]**.
pub const READ_STALE: Duration = Duration::from_secs(30);
/// How long a ratter fights a neut rat with a dry capacitor before it
/// leaves **[A]**.
pub const NEUT_PATIENCE: Duration = Duration::from_secs(40);
/// How long a held bot waits when [`gate`] took every act away **[A]**.
pub const HOLD_WAIT: Duration = Duration::from_secs(2);

/// Why an activation was refused, as far as the hold cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// `NotEnoughEnergy`.
    Energy,
    /// `ModuleReactivationDelayed2`.
    Delayed,
    /// `EffectCrowdedOut`.
    Crowded,
}

impl Refusal {
    /// The refusal a server error text carries, if it is one of these.
    pub fn of(why: &str) -> Option<Refusal> {
        if why.contains("NotEnoughEnergy") {
            Some(Refusal::Energy)
        } else if why.contains("ModuleReactivationDelayed") {
            Some(Refusal::Delayed)
        } else if why.contains("EffectCrowdedOut") {
            Some(Refusal::Crowded)
        } else {
            None
        }
    }
}

/// The back-off after `strikes` refusals in a row (pure): `BACKOFF_BASE`
/// doubled for each one past the first, up to [`BACKOFF_MAX`].
pub fn backoff(strikes: u32) -> Duration {
    let doublings = strikes.saturating_sub(1).min(16);
    BACKOFF_BASE.saturating_mul(1 << doublings).min(BACKOFF_MAX)
}

/// The ship's capacitor as the pushes told it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cap {
    /// The ship the numbers are of.
    pub ship: i64,
    /// The last pushed charge and when it arrived.
    value: Option<(f64, Instant)>,
    /// Capacity (482), GJ, and recharge time (55), when read.
    model: Option<(f64, Duration)>,
    /// The largest charge seen: a lower bound for the capacity when the
    /// hull's numbers were not read.
    peak: f64,
}

impl Cap {
    /// The hull's numbers, read off `ship`'s `ItemGetInfo` row. A new ship
    /// starts a new tracking.
    pub fn set_model(
        &mut self,
        ship: i64,
        capacity: f64,
        recharge_ms: f64,
        charge: Option<f64>,
        now: Instant,
    ) {
        if self.ship != ship {
            *self = Cap {
                ship,
                ..Cap::default()
            };
        }
        if capacity > 0.0 && recharge_ms > 0.0 {
            self.model = Some((capacity, Duration::from_secs_f64(recharge_ms / 1_000.0)));
        }
        if let Some(c) = charge {
            self.on_charge(ship, c, now);
        }
    }

    /// An attribute-18 push for `ship` with the new charge.
    pub fn on_charge(&mut self, ship: i64, charge: f64, now: Instant) {
        if self.ship != ship {
            *self = Cap {
                ship,
                ..Cap::default()
            };
        }
        if !charge.is_finite() || charge < 0.0 {
            return;
        }
        self.peak = self.peak.max(charge);
        self.value = Some((charge, now));
    }

    fn capacity(&self) -> Option<f64> {
        self.model.map(|m| m.0)
    }

    /// The charge now, GJ, when known: the last push carried forward on the
    /// recharge curve (or, without the hull's numbers, as it was while
    /// fresh).
    pub fn level(&self, now: Instant) -> Option<f64> {
        let (v, at) = self.value?;
        let dt = now.saturating_duration_since(at);
        match self.model {
            Some((cap, t)) => {
                let x = (v / cap).clamp(0.0, 1.0).sqrt();
                let k = (-5.0 * dt.as_secs_f64() / t.as_secs_f64()).exp();
                Some(cap * (1.0 + (x - 1.0) * k).powi(2))
            }
            None => (dt <= READ_STALE).then_some(v),
        }
    }

    /// [`Self::level`] as a fraction of the capacity. Without the hull's
    /// numbers the largest charge seen stands for the capacity **[A]**.
    pub fn fraction(&self, now: Instant) -> Option<f64> {
        let cap = self.capacity().or((self.peak > 0.0).then_some(self.peak))?;
        Some((self.level(now)? / cap).clamp(0.0, 1.0))
    }

    /// How long until the capacitor is projected at `frac` of capacity;
    /// zero if it is there already, `None` when it cannot be told.
    pub fn time_to(&self, frac: f64, now: Instant) -> Option<Duration> {
        let (cap, t) = self.model?;
        if self.fraction(now)? >= frac {
            return Some(Duration::ZERO);
        }
        let (v, at) = self.value?;
        let x = (v / cap).clamp(0.0, 1.0).sqrt();
        let target = frac.clamp(0.0, 0.999).sqrt();
        let k = (1.0 - target) / (1.0 - x);
        let secs = -t.as_secs_f64() / 5.0 * k.ln();
        let from_push = Duration::from_secs_f64(secs.max(0.0));
        Some(from_push.saturating_sub(now.saturating_duration_since(at)))
    }
}

/// What the brain reads (in [`Observation::cap`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// The charge as a fraction of the capacity, when known.
    pub fraction: Option<f64>,
    /// Drained: below [`DRY_AT`], and not yet back to [`RESUME_AT`].
    pub drained: bool,
    /// Activations are held: drained, or inside a refusal's back-off.
    pub hold: bool,
    /// The NPCs whose neutralizers are on this ship now, by ball id.
    pub neuts: Vec<i64>,
}

/// The driver's side: the capacitor, the hold and the neuts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Watch {
    pub cap: Cap,
    hold_until: Option<Instant>,
    strikes: u32,
    drained: bool,
    neuts: BTreeMap<i64, Instant>,
}

impl Watch {
    /// An activation was refused: hold the next ones for the back-off, and
    /// for an energy refusal until the capacitor is projected at
    /// [`RESUME_AT`] (capped at [`BACKOFF_MAX`] **[A]**).
    pub fn refused(&mut self, why: Refusal, now: Instant) {
        self.strikes = self.strikes.saturating_add(1);
        let mut wait = backoff(self.strikes);
        if why == Refusal::Energy
            && let Some(t) = self.cap.time_to(RESUME_AT, now)
        {
            wait = wait.max(t.min(BACKOFF_MAX));
        }
        let until = now + wait;
        self.hold_until = Some(self.hold_until.map_or(until, |h| h.max(until)));
    }

    /// An activation went through: the refusals are over.
    pub fn ran(&mut self) {
        self.strikes = 0;
    }

    /// `OnEwarStart` from `source` aimed at this ship, of `jamming`.
    pub fn ewar_start(&mut self, source: i64, jamming: &str, now: Instant) {
        if jamming == NEUT_TYPE {
            self.neuts.insert(source, now);
        }
    }

    /// `OnEwarEnd` from `source`.
    pub fn ewar_end(&mut self, source: i64, jamming: &str) {
        if jamming == NEUT_TYPE {
            self.neuts.remove(&source);
        }
    }

    /// **One push**, folded in: the ship's charge (attribute 18) from
    /// `OnModuleAttributeChange` or, in a batch, `OnModuleAttributeChanges`;
    /// a neutralizer from `OnEwarStart`/`OnEwarEnd`; and a neutralizer
    /// cycle's drain keeps its source fresh. `ship` is this pilot's ship;
    /// a push about another item is not ours.
    pub fn on_push(&mut self, event: &str, args: &[PyValue], ship: Option<i64>, now: Instant) {
        let Some(ship) = ship else { return };
        match event {
            "OnModuleAttributeChange" => {
                if let (Some(PyValue::Int(item)), Some(PyValue::Int(a)), Some(new)) =
                    (args.get(1), args.get(2), crate::real_at(args, 4))
                    && *a == ATTR_CHARGE
                    && *item == ship
                {
                    self.cap.on_charge(ship, new, now);
                }
            }
            "OnModuleAttributeChanges" => {
                let Some(PyValue::List(changes)) = args.first() else {
                    return;
                };
                for c in changes {
                    let PyValue::Tuple(t) = c else { continue };
                    if let (Some(PyValue::Int(item)), Some(PyValue::Int(a)), Some(new)) =
                        (t.get(2), t.get(3), crate::real_at(t, 5))
                        && *a == ATTR_CHARGE
                        && *item == ship
                    {
                        self.cap.on_charge(ship, new, now);
                    }
                }
            }
            "OnEwarStart" | "OnEwarEnd" => {
                if let (Some(PyValue::Int(src)), Some(PyValue::Int(target)), Some(kind)) =
                    (args.first(), args.get(2), args.get(3).and_then(crate::text))
                    && *target == ship
                {
                    if event == "OnEwarStart" {
                        self.ewar_start(*src, &kind, now);
                    } else {
                        self.ewar_end(*src, &kind);
                    }
                }
            }
            _ => {}
        }
    }

    /// The turn's scene at `now`.
    pub fn scene(&mut self, now: Instant) -> Scene {
        self.neuts
            .retain(|_, at| now.saturating_duration_since(*at) < NEUT_STALE);
        let fraction = self.cap.fraction(now);
        match fraction {
            Some(f) if f < DRY_AT => self.drained = true,
            Some(f) if f >= RESUME_AT => self.drained = false,
            None => self.drained = false,
            _ => {}
        }
        if self.hold_until.is_some_and(|h| now >= h) {
            self.hold_until = None;
        }
        Scene {
            fraction,
            drained: self.drained,
            hold: self.drained || self.hold_until.is_some(),
            neuts: self.neuts.keys().copied().collect(),
        }
    }
}

/// Whether `act` spends the capacitor on a module cycle the gate holds: a
/// laser or a gun.
fn spends(act: &Act) -> bool {
    matches!(
        act,
        Act::Mine { .. } | Act::Fire { .. } | Act::Ore(crate::ore::OreAct::Mine(_))
    )
}

/// **The hold** (pure): while [`Scene::hold`], no laser or gun is
/// activated -- they are taken out of `acts`, and a turn left with nothing
/// waits [`HOLD_WAIT`]. Everything else (locks, orbits, warps, docking)
/// goes on.
pub fn gate(o: &Observation, acts: Vec<Act>) -> Vec<Act> {
    if !o.cap.hold || !acts.iter().any(spends) {
        return acts;
    }
    let mut kept: Vec<Act> = acts.into_iter().filter(|a| !spends(a)).collect();
    if kept.is_empty() {
        kept.push(Act::Wait(HOLD_WAIT.max(SHORT_WAIT)));
    }
    kept
}

/// What a ratter does about a neut rat on grid.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// Nothing: not drained, or no neut.
    Ignore,
    /// Make this NPC the primary target.
    Kill(i64, String),
    /// Warp off: the neut cannot be killed.
    Flee(String),
}

/// **The neut-on-grid decision** (pure but for the patience clock in
/// `m.neut_fight`): with the capacitor [`Scene::drained`] and a neut on
/// this ship, the neut's NPC is the primary target when it is on the grid,
/// inside lock range, the guns are loaded, and the fire on the ship is
/// light (half [`crate::escort::RETREAT_AT`] by the profiles; no profiles
/// count as light **[A]**) -- for [`NEUT_PATIENCE`]; else the answer is to
/// leave. Falsifier: ratters that die to a neut rat they could have killed
/// (too timid), or sit dry-firing at one (too bold).
pub fn answer(m: &mut Memory, o: &Observation, park: &ParkView) -> Answer {
    if !o.cap.drained || o.cap.neuts.is_empty() {
        m.neut_fight = None;
        return Answer::Ignore;
    }
    let hostile = park
        .hostiles
        .iter()
        .filter(|h| o.cap.neuts.contains(&h.ball.id))
        .min_by(|a, b| {
            park.ego
                .surface_to(&a.ball)
                .total_cmp(&park.ego.surface_to(&b.ball))
                .then(a.ball.id.cmp(&b.ball.id))
        });
    let light = m.policy.profiles.as_deref().is_none_or(|p| {
        crate::escort::incoming_dps(p, park, &park.ego) <= crate::escort::RETREAT_AT / 2.0
    });
    let armed = !o.guns.is_empty() && !crate::escort::dry(&o.guns);
    let in_reach = hostile.is_some_and(|h| park.ego.surface_to(&h.ball) <= o.lock_range);
    let since = *m.neut_fight.get_or_insert(o.now);
    let patient = o.now.saturating_duration_since(since) < NEUT_PATIENCE;
    match hostile {
        Some(h) if light && armed && in_reach && patient => Answer::Kill(h.ball.id, h.name.clone()),
        _ => Answer::Flee(format!(
            "drained: capacitor {:.0}% with {} neut(s) on me (kill: light {light}, armed {armed}, in reach {in_reach}, patient {patient})",
            o.cap.fraction.unwrap_or(0.0) * 100.0,
            o.cap.neuts.len()
        )),
    }
}

/// The homing a drained flight sets.
pub const FLEE: Homing = Homing::Drained;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Ball, Hostile, Policy};

    const SHIP: i64 = 7_001;
    const OTHER: i64 = 7_002;
    const NPC: i64 = 7_101;

    fn t0() -> Instant {
        Instant::now()
    }

    fn model(now: Instant) -> Cap {
        let mut c = Cap::default();
        c.set_model(SHIP, 1_000.0, 100_000.0, Some(1_000.0), now);
        c
    }

    #[test]
    fn pushes_set_the_level_and_recharge_projects_it() {
        let now = t0();
        let mut c = model(now);
        assert_eq!(c.fraction(now), Some(1.0));
        c.on_charge(SHIP, 100.0, now);
        assert!((c.fraction(now).unwrap() - 0.1).abs() < 1e-9);
        // Later, with the curve: higher, and never past the capacity.
        let later = now + Duration::from_secs(20);
        let f = c.fraction(later).unwrap();
        assert!(f > 0.1 && f < 1.0, "{f}");
        let much_later = now + Duration::from_secs(1_000);
        assert!(c.fraction(much_later).unwrap() > 0.99);
        // Another ship's push starts a new tracking.
        c.on_charge(OTHER, 5.0, now);
        assert_eq!(c.ship, OTHER);
        assert_eq!(c.fraction(now), Some(1.0));
    }

    #[test]
    fn without_the_hull_numbers_a_push_goes_stale() {
        let now = t0();
        let mut c = Cap::default();
        c.on_charge(SHIP, 500.0, now);
        c.on_charge(SHIP, 50.0, now);
        // The peak stands for the capacity.
        assert!((c.fraction(now).unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(c.level(now + READ_STALE + Duration::from_secs(1)), None);
    }

    #[test]
    fn time_to_matches_the_curve() {
        let now = t0();
        let mut c = model(now);
        c.on_charge(SHIP, 0.0, now);
        let wait = c.time_to(RESUME_AT, now).unwrap();
        let there = now + wait;
        assert!((c.fraction(there).unwrap() - RESUME_AT).abs() < 0.01);
        assert_eq!(
            c.time_to(RESUME_AT, there + Duration::from_secs(60)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn refusals_back_off_and_double() {
        assert_eq!(backoff(1), BACKOFF_BASE);
        assert_eq!(backoff(2), BACKOFF_BASE * 2);
        assert_eq!(backoff(3), BACKOFF_BASE * 4);
        assert_eq!(backoff(40), BACKOFF_MAX);
        assert_eq!(Refusal::of("x NotEnoughEnergy y"), Some(Refusal::Energy));
        assert_eq!(
            Refusal::of("ModuleReactivationDelayed2"),
            Some(Refusal::Delayed)
        );
        assert_eq!(Refusal::of("EffectCrowdedOut"), Some(Refusal::Crowded));
        assert_eq!(Refusal::of("TargetTooFar"), None);

        let now = t0();
        let mut w = Watch::default();
        w.refused(Refusal::Delayed, now);
        assert!(w.scene(now).hold);
        assert!(w.scene(now + BACKOFF_BASE - Duration::from_millis(1)).hold);
        assert!(!w.scene(now + BACKOFF_BASE).hold);
        // Two in a row hold longer.
        w.refused(Refusal::Delayed, now);
        w.refused(Refusal::Delayed, now);
        assert!(w.scene(now + BACKOFF_BASE * 3).hold);
        assert!(!w.scene(now + BACKOFF_BASE * 4).hold);
        // A success clears the count.
        w.ran();
        w.refused(Refusal::Crowded, now + BACKOFF_MAX);
        assert!(!w.scene(now + BACKOFF_MAX + BACKOFF_BASE).hold);
    }

    #[test]
    fn an_energy_refusal_holds_until_the_capacitor_is_back() {
        let now = t0();
        let mut w = Watch {
            cap: model(now),
            ..Watch::default()
        };
        w.cap.on_charge(SHIP, 0.0, now);
        w.refused(Refusal::Energy, now);
        let need = w.cap.time_to(RESUME_AT, now).unwrap().min(BACKOFF_MAX);
        assert!(need > BACKOFF_BASE);
        assert!(w.scene(now + need - Duration::from_millis(50)).hold);
        // Drained too (below DRY_AT), until it is back at RESUME_AT.
        let back = now + w.cap.time_to(RESUME_AT, now).unwrap() + Duration::from_millis(50);
        assert!(!w.scene(back).drained);
    }

    #[test]
    fn drained_has_hysteresis_and_gates_lasers_and_guns() {
        let now = t0();
        let mut w = Watch {
            cap: model(now),
            ..Watch::default()
        };
        w.cap.on_charge(SHIP, 50.0, now);
        let s = w.scene(now);
        assert!(s.drained && s.hold);
        let mut o = Observation::empty(now);
        o.cap = s;
        let acts = vec![
            Act::Mine { module: 1, rock: 2 },
            Act::Fire { gun: 3, target: 4 },
            Act::Lock(4),
        ];
        assert_eq!(gate(&o, acts), vec![Act::Lock(4)]);
        assert_eq!(
            gate(&o, vec![Act::Mine { module: 1, rock: 2 }]),
            vec![Act::Wait(HOLD_WAIT)]
        );
        // Recharged past DRY_AT but not RESUME_AT: still drained.
        w.cap.on_charge(SHIP, 200.0, now);
        assert!(w.scene(now).drained);
        w.cap.on_charge(SHIP, 400.0, now);
        let s = w.scene(now);
        assert!(!s.drained && !s.hold);
        o.cap = s;
        let acts = vec![Act::Mine { module: 1, rock: 2 }];
        assert_eq!(gate(&o, acts.clone()), acts);
    }

    #[test]
    fn a_neut_is_on_until_it_ends_or_goes_stale() {
        let now = t0();
        let mut w = Watch::default();
        w.ewar_start(NPC, "ewTargetPaint", now);
        assert!(w.scene(now).neuts.is_empty());
        w.ewar_start(NPC, NEUT_TYPE, now);
        assert_eq!(w.scene(now).neuts, vec![NPC]);
        w.ewar_end(NPC, NEUT_TYPE);
        assert!(w.scene(now).neuts.is_empty());
        w.ewar_start(NPC, NEUT_TYPE, now);
        assert!(w.scene(now + NEUT_STALE).neuts.is_empty());
        w.ewar_start(NPC, NEUT_TYPE, now);
        assert_eq!(w.scene(now + NEUT_STALE / 2).neuts, vec![NPC]);
    }

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 0.0,
        }
    }

    fn scene_with_neut(drained: bool) -> (Memory, Observation) {
        let now = t0();
        let mut o = Observation::empty(now);
        o.lock_range = 50_000.0;
        o.guns = vec![crate::brain::GunInfo {
            item_id: 11,
            type_id: 12,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(10),
            capacity: 10,
            range: 5_000.0,
            reload: Duration::from_secs(5),
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: None,
            taken: BTreeMap::new(),
            hostiles: vec![Hostile {
                ball: ball(NPC, 20_000.0),
                type_id: 13,
                name: "Neut Rat".to_owned(),
                shooting: Some(SHIP),
            }],
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o.cap = Scene {
            fraction: Some(0.02),
            drained,
            hold: drained,
            neuts: vec![NPC],
        };
        (Memory::new("t", Policy::fast()), o)
    }

    #[test]
    fn a_drained_ratter_kills_a_neut_it_can_reach_else_leaves() {
        let (mut m, o) = scene_with_neut(true);
        let park = o.park.clone().unwrap();
        assert_eq!(
            answer(&mut m, &o, &park),
            Answer::Kill(NPC, "Neut Rat".to_owned())
        );
        // Not drained: nothing to do.
        let (mut m2, o2) = scene_with_neut(false);
        assert_eq!(answer(&mut m2, &o2, &park), Answer::Ignore);
        // Out of lock range: leave.
        let (mut m3, mut o3) = scene_with_neut(true);
        o3.lock_range = 1_000.0;
        assert!(matches!(answer(&mut m3, &o3, &park), Answer::Flee(_)));
        // Dry guns: leave.
        let (mut m4, mut o4) = scene_with_neut(true);
        o4.guns[0].rounds = Some(0);
        assert!(matches!(answer(&mut m4, &o4, &park), Answer::Flee(_)));
        // The neut is not in the park: leave.
        let (mut m5, mut o5) = scene_with_neut(true);
        o5.cap.neuts = vec![NPC + 1];
        assert!(matches!(answer(&mut m5, &o5, &park), Answer::Flee(_)));
        // Patience runs out.
        let (mut m6, mut o6) = scene_with_neut(true);
        assert!(matches!(answer(&mut m6, &o6, &park), Answer::Kill(..)));
        o6.now += NEUT_PATIENCE;
        assert!(matches!(answer(&mut m6, &o6, &park), Answer::Flee(_)));
    }

    #[test]
    fn the_servers_pushes_are_tracked() {
        use eve_server::modules::AttributeChange;
        use eve_server::notify::Push;
        let now = t0();
        let mut w = Watch {
            cap: model(now),
            ..Watch::default()
        };
        let (ev, args) = AttributeChange {
            to: vec![1],
            owner_id: 1,
            item_id: SHIP,
            attribute_id: ATTR_CHARGE,
            time: 0,
            new_value: 120.0,
            old_value: 900.0,
        }
        .to_push();
        w.on_push(ev, &args, Some(SHIP), now);
        assert!((w.cap.fraction(now).unwrap() - 0.12).abs() < 1e-9);
        // Another attribute, or another item, or no ship: ignored.
        let other = |item, attr| {
            AttributeChange {
                to: vec![1],
                owner_id: 1,
                item_id: item,
                attribute_id: attr,
                time: 0,
                new_value: 5.0,
                old_value: 6.0,
            }
            .to_push()
        };
        let (ev, args) = other(SHIP, 54);
        w.on_push(ev, &args, Some(SHIP), now);
        let (ev, args) = other(OTHER, ATTR_CHARGE);
        w.on_push(ev, &args, Some(SHIP), now);
        let (ev, args) = other(SHIP, ATTR_CHARGE);
        w.on_push(ev, &args, None, now);
        assert!((w.cap.fraction(now).unwrap() - 0.12).abs() < 1e-9);
        // The batch shape: a list of eight-tuples.
        let batch = vec![PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Bytes(b"OnModuleAttributeChange".to_vec()),
            PyValue::Int(1),
            PyValue::Int(SHIP),
            PyValue::Int(ATTR_CHARGE),
            PyValue::Int(0),
            PyValue::Real(300.0),
            PyValue::Real(120.0),
            PyValue::None,
        ])])];
        w.on_push("OnModuleAttributeChanges", &batch, Some(SHIP), now);
        assert!((w.cap.fraction(now).unwrap() - 0.3).abs() < 1e-9);
        // A neutralizer aimed at us, then ended; one aimed at another ship.
        let ewar = |target, kind: &str| {
            vec![
                PyValue::Int(NPC),
                PyValue::Int(NPC),
                PyValue::Int(target),
                PyValue::Bytes(kind.as_bytes().to_vec()),
            ]
        };
        w.on_push("OnEwarStart", &ewar(OTHER, NEUT_TYPE), Some(SHIP), now);
        assert!(w.scene(now).neuts.is_empty());
        w.on_push("OnEwarStart", &ewar(SHIP, NEUT_TYPE), Some(SHIP), now);
        assert_eq!(w.scene(now).neuts, vec![NPC]);
        w.on_push("OnEwarEnd", &ewar(SHIP, NEUT_TYPE), Some(SHIP), now);
        assert!(w.scene(now).neuts.is_empty());
    }
}
