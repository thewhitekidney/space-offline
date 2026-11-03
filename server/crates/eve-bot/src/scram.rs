//! **A scrambled bot stays and deals with it** (BOT-SCRAM). A warp scrambler
//! (an NPC's `warpScrambler`, a player's `warpScramblerMWD` or
//! `warpScrambler`) makes the server refuse every warp with `WarpScrambled`
//! (gates with `GateScrambled`). A bot that ignored that counted the refusals
//! as failures, and the third sent it to `Reconnect`, which the server
//! answers with an emergency warp for a ship with no weapons timer: a
//! tackled bot escaped by logging off.
//!
//! - **Sense** ([`Sense`]): `OnEwarStart(source, module, ship, jamming)` on
//!   this ship with a [`JAMMING_PREFIX`] name holds for [`EWAR_HOLD`]
//!   (sent once per start, not per cycle); `OnEwarEnd` lets go. A refusal in
//!   [`REFUSALS`] also marks the ship scrambled for [`REFUSAL_FRESH`], and is
//!   not a failure ([`counts`]).
//! - **Response** ([`decide`]): with drones or guns and the scrambler on
//!   grid, fight it for up to [`FIGHT`]; otherwise align to the home station
//!   once and wait, trying the role's own act again after a back-off that
//!   grows from [`RETRY_FIRST`] by [`RETRY_GROWTH`] to [`RETRY_MAX`]. After
//!   [`BUDGET`] the fought scramblers are fought again.
//! - **Never `Reconnect` in a fight** ([`reconnect_allowed`]): the driver
//!   swaps it for a wait while the ship is scrambled, locked, shot at, or
//!   has attackers.
//!
//! Every number here is **[A]** policy; EvEJS's scram bot is the hint
//! (a belt-rat fight of at most 120 s, an episode of 240 s, a retry of 8 s
//! x1.5 to 30 s). Falsifiers: bots dying to every belt rat (shorten
//! [`FIGHT`]); bots stuck aligned for minutes with nothing on grid (shorten
//! [`REFUSAL_FRESH`]).

use crate::brain::{Act, Memory, Observation, Place, SHORT_WAIT};
use crate::defend::{self, Verdict};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// The `jammingType` prefix of a warp scrambler's `OnEwarStart` **[V]**
/// (`warpScrambler` for an NPC's, `warpScramblerMWD` for a player's).
pub const JAMMING_PREFIX: &str = "warpScrambler";
/// The player scrambler's `jammingType` **[V]**: it also blocks the MWD and the MJD.
const HARD_KIND: &str = "warpScramblerMWD";
/// The refusal keys of a scrambled ship: a warp, and a gate jump **[V]**.
pub const REFUSALS: [&str; 2] = ["WarpScrambled", "GateScrambled"];

/// A scramble is believed this long after its start **[A]**: a safety net
/// against a lost `OnEwarEnd`. The server sends `OnEwarStart` once, not per
/// cycle (`tackle::tell`), so the hold must outlast a fight. Falsified by a
/// bot that waits on a scramble that has gone; the cost is small, since
/// `decide` still retries the role's warp.
pub const EWAR_HOLD: Duration = Duration::from_secs(180);
/// A refusal marks the ship scrambled this long **[A]**.
pub const REFUSAL_FRESH: Duration = Duration::from_secs(10);
/// The longest a bot fights one scrambler before it aligns instead **[A]**.
pub const FIGHT: Duration = Duration::from_secs(120);
/// The episode's budget **[A]**: past it the fought scramblers are fought
/// again, and the bot keeps aligned and trying. It never reconnects.
pub const BUDGET: Duration = Duration::from_secs(240);
/// The first wait before the role tries its warp again **[A]**.
pub const RETRY_FIRST: Duration = Duration::from_secs(8);
/// The growth of that wait per retry **[A]**.
pub const RETRY_GROWTH: f64 = 1.5;
/// The longest that wait gets **[A]**.
pub const RETRY_MAX: Duration = Duration::from_secs(30);
/// The longest single `Wait` while scrambled **[A]**: the driver wakes on a
/// push anyway.
pub const WAIT_STEP: Duration = Duration::from_secs(2);

/// What the brain sees of the scramble this turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct View {
    pub scrambled: bool,
    /// The scramblers' ball ids, the newest first.
    pub sources: Vec<i64>,
    /// A running *scrambler* (`warpScramblerMWD`) holds the ship, or a refusal
    /// came with no push to say which: the kind that blocks a micro jump
    /// drive (a disruptor does not, BP-KIT). **[V]** `tackle.rs`.
    pub hard: bool,
    /// Every module aimed at this ship, scramblers too: `(source ball, kind)`,
    /// the newest first (BP-EWAR-AIM).
    pub ewar: Vec<(i64, crate::brain::GunKind)>,
}

/// Whether a server error text is a scramble refusal, which is a fact about
/// the ship and not a failure of the act.
pub fn is_refusal(why: &str) -> bool {
    // The field generator's key contains `WarpScrambled`: the bubble is asked first.
    !crate::warp_bubble::is_refusal(why) && REFUSALS.iter().any(|k| why.contains(k))
}

/// Whether a refusal counts as a failure of its kind (the driver's tally).
pub fn counts(why: &str) -> bool {
    !is_refusal(why) && !crate::warp_bubble::is_refusal(why)
}

/// The driver's side: who scrambles this ship.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sense {
    /// `source ball -> (started, believed until)`.
    holds: BTreeMap<i64, (Instant, Instant)>,
    /// The sources whose last start was a scrambler (`warpScramblerMWD`).
    hard: BTreeSet<i64>,
    refused_at: Option<Instant>,
    /// The ship the holds belong to; a change drops them.
    ship: Option<i64>,
    /// The other modules aimed at this ship: `(source, kind, started, until)`.
    others: Vec<(i64, crate::brain::GunKind, Instant, Instant)>,
}

impl Sense {
    /// Fold in a push: `OnEwarStart`/`OnEwarEnd(source, module, ship,
    /// jammingType)` aimed at `my_ship`.
    pub fn on_push(&mut self, event: &str, args: &[PyValue], my_ship: Option<i64>, now: Instant) {
        self.on_ship(my_ship);
        if event != "OnEwarStart" && event != "OnEwarEnd" {
            return;
        }
        let (Some(PyValue::Int(src)), Some(PyValue::Int(target)), Some(kind)) =
            (args.first(), args.get(2), args.get(3).and_then(crate::text))
        else {
            return;
        };
        if Some(*target) != my_ship {
            return;
        }
        if !kind.starts_with(JAMMING_PREFIX) {
            // BP-EWAR-AIM: a web, a jam, a damp, a paint, a disruptor or a neut.
            if let Some(k) = crate::brain::GunKind::of_jamming(&kind) {
                self.others.retain(|(s, c, _, _)| !(s == src && *c == k));
                if event == "OnEwarStart" {
                    self.others.push((*src, k, now, now + EWAR_HOLD));
                }
            }
            return;
        }
        if event == "OnEwarStart" {
            self.holds.insert(*src, (now, now + EWAR_HOLD));
            if kind == HARD_KIND {
                self.hard.insert(*src);
            } else {
                self.hard.remove(src);
            }
        } else {
            self.holds.remove(src);
            self.hard.remove(src);
        }
    }

    /// The ship this bot flies now: a different one lets go of every hold
    /// (an `OnEwarEnd` for the old hull is ignored).
    pub fn on_ship(&mut self, ship: Option<i64>) {
        if self.ship != ship {
            self.ship = ship;
            self.holds.clear();
            self.hard.clear();
            self.others.clear();
            self.refused_at = None;
        }
    }

    /// A warp or gate was refused for the scramble.
    pub fn refused(&mut self, now: Instant) {
        self.refused_at = Some(now);
    }

    /// Scrambled at `now`: a hold not run out, or a fresh refusal.
    pub fn scrambled(&self, now: Instant) -> bool {
        self.holds.values().any(|(_, until)| now < *until)
            || self
                .refused_at
                .is_some_and(|r| now.saturating_duration_since(r) < REFUSAL_FRESH)
    }

    /// The scramblers holding at `now`, newest first.
    pub fn sources(&self, now: Instant) -> Vec<i64> {
        let mut live: Vec<(Instant, i64)> = self
            .holds
            .iter()
            .filter(|(_, (_, until))| now < *until)
            .map(|(s, (at, _))| (*at, *s))
            .collect();
        live.sort_by(|a, b| b.cmp(a));
        live.into_iter().map(|(_, s)| s).collect()
    }

    /// The turn's view at `now`; spent holds are dropped.
    pub fn view(&mut self, now: Instant) -> View {
        self.holds.retain(|_, (_, until)| now < *until);
        self.others.retain(|(_, _, _, until)| now < *until);
        let mut ewar: Vec<(Instant, i64, crate::brain::GunKind)> = self
            .others
            .iter()
            .map(|(s, k, at, _)| (*at, *s, *k))
            .collect();
        for (s, (at, _)) in &self.holds {
            let k = if self.hard.contains(s) {
                crate::brain::GunKind::Scram
            } else {
                crate::brain::GunKind::Point
            };
            ewar.push((*at, *s, k));
        }
        ewar.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let hard = self.holds.keys().any(|s| self.hard.contains(s))
            || (self.holds.is_empty() && self.scrambled(now));
        View {
            scrambled: self.scrambled(now),
            sources: self.sources(now),
            hard,
            ewar: ewar.into_iter().map(|(_, s, k)| (s, k)).collect(),
        }
    }
}

/// One scramble, from its first turn to the turn it dropped.
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub started: Instant,
    /// Not before this, the role's own warp is tried again.
    pub retry_at: Instant,
    /// The wait that `retry_at` was set with.
    pub delay: Duration,
    /// Scramblers fought for [`FIGHT`] already.
    pub fought: BTreeSet<i64>,
    /// The ball being fought and since when.
    pub fight: Option<(i64, Instant)>,
    /// The align was sent.
    pub aligned: bool,
    /// The budget was spent and noted.
    pub spent: bool,
    /// The turn the role was let through (once per turn, whoever asks).
    pub passed: Option<Instant>,
}

impl Episode {
    pub fn new(now: Instant) -> Episode {
        Episode {
            started: now,
            retry_at: now + RETRY_FIRST,
            delay: RETRY_FIRST,
            fought: BTreeSet::new(),
            fight: None,
            aligned: false,
            spent: false,
            passed: None,
        }
    }
}

/// The wait after `d`: [`RETRY_GROWTH`] times it, to [`RETRY_MAX`].
pub fn next_delay(d: Duration) -> Duration {
    d.mul_f64(RETRY_GROWTH).min(RETRY_MAX)
}

/// Whether the bot can fight at all: drones in space or in the bay, or guns.
fn armed(m: &Memory, o: &Observation) -> bool {
    !m.combat.out.is_empty()
        || (m.policy.combat_drones && !m.combat.dry)
        || o.guns.iter().any(|g| g.online)
}

/// **The scrambler to fight** (pure): the newest source on grid, else the
/// nearest NPC shooting the ship; never one fought out.
fn target(m: &Memory, o: &Observation, fought: &BTreeSet<i64>) -> Option<i64> {
    let park = o.park.as_ref()?;
    o.scram
        .sources
        .iter()
        .copied()
        .find(|s| !fought.contains(s) && park.hostiles.iter().any(|h| h.ball.id == *s))
        .or_else(|| {
            defend::attackers(m, park)
                .into_iter()
                .filter(|h| !fought.contains(&h.ball.id))
                .min_by(|a, b| {
                    park.ego
                        .surface_to(&a.ball)
                        .total_cmp(&park.ego.surface_to(&b.ball))
                        .then(a.ball.id.cmp(&b.ball.id))
                })
                .map(|h| h.ball.id)
        })
}

/// **The scramble step** (pure but for the log), asked first by the in-space
/// rules. `None`: not scrambled, or the role's own act may run this turn.
pub fn decide(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if !matches!(o.place, Place::InSpace { bound: true, .. }) {
        return None;
    }
    if !o.scram.scrambled {
        if let Some(ep) = m.scram.take() {
            eprintln!(
                "[scram] {}: dropped after {} s",
                m.name,
                o.now.saturating_duration_since(ep.started).as_secs()
            );
        }
        return None;
    }
    let park = o.park.as_ref()?;
    let now = o.now;
    let mut ep = m.scram.take().unwrap_or_else(|| {
        eprintln!("[scram] {}: scrambled", m.name);
        Episode::new(now)
    });
    // The role's turn already went through this pass.
    if ep.passed == Some(now) {
        m.scram = Some(ep);
        return None;
    }
    if !ep.spent && now.saturating_duration_since(ep.started) >= BUDGET {
        ep.spent = true;
        ep.fought.clear();
        eprintln!(
            "[scram] {}: budget spent, fighting and trying on (no reconnect)",
            m.name
        );
    }

    // BP-KIT: going home, held by disruptors alone, a micro jump drive is
    // not blocked and takes the ship out of their reach.
    if let Some(home) = park.station.map(|b| b.id).or(o.home)
        && let Some(acts) = crate::pvp_kit::disengage(m, o, home)
    {
        m.scram = Some(ep);
        return Some(acts);
    }

    // A fight: the drones (and guns) at the scrambler.
    if armed(m, o) {
        let mut t = target(m, o, &ep.fought);
        if let Some(tt) = t {
            let since = match ep.fight {
                Some((b, at)) if b == tt => at,
                _ => now,
            };
            ep.fight = Some((tt, since));
            if now.saturating_duration_since(since) >= FIGHT {
                ep.fought.insert(tt);
                ep.fight = None;
                t = None;
            }
        }
        if let Some(t) = t {
            let mut acts = defend::steer(m, now, park.ego_warping, Verdict::Fight(t), Vec::new());
            let in_range = park
                .hostiles
                .iter()
                .find(|h| h.ball.id == t)
                .is_some_and(|h| {
                    park.ego.surface_to(&h.ball) <= o.lock_range * crate::brain::RANGE_MARGIN
                });
            if o.guns.iter().any(|g| g.online) && in_range {
                if !o.locked.contains(&t) {
                    if !o.pending.contains(&t) {
                        acts.push(Act::Lock(t));
                    }
                } else {
                    acts.extend(
                        o.guns
                            .iter()
                            .filter(|g| g.online && !g.active && g.rounds != Some(0))
                            .map(|g| Act::Fire {
                                gun: g.item_id,
                                target: t,
                            }),
                    );
                }
            }
            acts.push(Act::Wait(SHORT_WAIT));
            m.scram = Some(ep);
            return Some(acts);
        }
    }

    // Else align once, wait, and let the role try again when the wait is up.
    let mut acts = Vec::new();
    if !ep.aligned {
        ep.aligned = true;
        if let Some(home) = park.station.map(|b| b.id).or(o.home) {
            acts.push(Act::AlignTo(home));
        }
    }
    if now < ep.retry_at {
        acts.push(Act::Wait(
            ep.retry_at.saturating_duration_since(now).min(WAIT_STEP),
        ));
        m.scram = Some(ep);
        return Some(acts);
    }
    ep.delay = next_delay(ep.delay);
    ep.retry_at = now + ep.delay;
    ep.passed = Some(now);
    m.scram = Some(ep);
    if acts.is_empty() { None } else { Some(acts) }
}

/// **The reconnect guard** (pure): whether a `Reconnect` may go ahead. In
/// space and scrambled, locked or shot at, or with attackers, it may not:
/// the server answers a dropped session with an emergency warp, so a
/// reconnect there is an escape.
pub fn reconnect_allowed(m: &Memory, o: &Observation) -> bool {
    if !matches!(o.place, Place::InSpace { bound: true, .. }) {
        return true;
    }
    if o.scram.scrambled || o.warp_bubble.held || !o.threat.foes.is_empty() {
        return false;
    }
    o.park
        .as_ref()
        .is_none_or(|p| defend::attackers(m, p).is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{self, ActKind, Ball, Hostile, ParkView, Policy};
    use crate::drone_use::DroneAct;

    const SHIP: i64 = 1_000_101;
    const OTHER: i64 = 1_000_102;
    const NPC: i64 = 1_000_201;
    const NPC2: i64 = 1_000_202;
    const STATION: i64 = 1_000_301;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    fn ewar(src: i64, target: i64, kind: &str) -> Vec<PyValue> {
        vec![
            PyValue::Int(src),
            PyValue::Int(src),
            PyValue::Int(target),
            PyValue::Bytes(kind.as_bytes().to_vec()),
        ]
    }

    /// In space, scrambled by `NPC` (on grid when `on_grid`).
    fn obs(now: Instant, scrambled: bool, on_grid: bool) -> Observation {
        let mut o = Observation::empty(now);
        o.place = brain::Place::InSpace {
            system: 1_000_001,
            bound: true,
        };
        o.home = Some(STATION);
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: Some(ball(STATION, 1_000_000.0)),
            boss: None,
            taken: Default::default(),
            hostiles: if on_grid {
                vec![Hostile {
                    ball: ball(NPC, 3_000.0),
                    type_id: 990_000_001,
                    name: "Fixture Scrambler".to_owned(),
                    shooting: None,
                }]
            } else {
                Vec::new()
            },
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o.scram = View {
            scrambled,
            sources: if scrambled { vec![NPC] } else { Vec::new() },
            hard: scrambled,
            ewar: Vec::new(),
        };
        o
    }

    #[test]
    fn the_sense_keeps_every_kind_of_ewar_aimed_at_the_ship() {
        use crate::brain::{EwarKind, GunKind};
        let t = Instant::now();
        let mut s = Sense::default();
        let text = |k: &str| PyValue::Bytes(k.as_bytes().to_vec());
        let start = |s: &mut Sense, src: i64, kind: &str| {
            let args = [
                PyValue::Int(src),
                PyValue::Int(0),
                PyValue::Int(77),
                text(kind),
            ];
            s.on_push("OnEwarStart", &args, Some(77), t);
        };
        start(&mut s, 11, "webify");
        start(&mut s, 12, "ewTrackingDisrupt");
        start(&mut s, 13, "warpScramblerMWD");
        let v = s.view(t);
        assert!(v.ewar.contains(&(11, GunKind::Web)));
        assert!(v.ewar.contains(&(12, GunKind::Ewar(EwarKind::Track))));
        assert!(v.ewar.contains(&(13, GunKind::Scram)));
        // A web is not a scramble.
        assert_eq!(v.sources, vec![13]);
        let end = [
            PyValue::Int(11),
            PyValue::Int(0),
            PyValue::Int(77),
            text("webify"),
        ];
        s.on_push("OnEwarEnd", &end, Some(77), t);
        assert!(!s.view(t).ewar.iter().any(|(src, _)| *src == 11));
    }

    fn miner(drones: bool) -> Memory {
        let mut m = Memory::new("Miner1", Policy::fast());
        m.policy.combat_drones = drones;
        m
    }

    #[test]
    fn sense_holds_on_a_scram_push_and_drops_on_its_end() {
        let t = Instant::now();
        let mut s = Sense::default();
        // The player's name, an NPC's, another ship's, and a web.
        s.on_push(
            "OnEwarStart",
            &ewar(NPC, OTHER, "warpScramblerMWD"),
            Some(SHIP),
            t,
        );
        assert!(!s.scrambled(t), "aimed at another ship");
        s.on_push("OnEwarStart", &ewar(NPC, SHIP, "webify"), Some(SHIP), t);
        assert!(!s.scrambled(t), "not a scrambler");
        s.on_push(
            "OnEwarStart",
            &ewar(NPC, SHIP, "warpScramblerMWD"),
            Some(SHIP),
            t,
        );
        assert!(s.scrambled(t));
        assert_eq!(s.sources(t), vec![NPC]);
        s.on_push(
            "OnEwarStart",
            &ewar(NPC2, SHIP, "warpScrambler"),
            Some(SHIP),
            t + Duration::from_secs(1),
        );
        assert_eq!(s.sources(t + Duration::from_secs(1)), vec![NPC2, NPC]);
        s.on_push(
            "OnEwarEnd",
            &ewar(NPC, SHIP, "warpScramblerMWD"),
            Some(SHIP),
            t,
        );
        s.on_push(
            "OnEwarEnd",
            &ewar(NPC2, SHIP, "warpScrambler"),
            Some(SHIP),
            t,
        );
        assert!(!s.scrambled(t));
        // A hold runs out; a refusal marks it for its own window.
        s.on_push(
            "OnEwarStart",
            &ewar(NPC, SHIP, "warpScrambler"),
            Some(SHIP),
            t,
        );
        assert!(!s.scrambled(t + EWAR_HOLD));
        let mut s = Sense::default();
        s.refused(t);
        assert!(s.scrambled(t + REFUSAL_FRESH - Duration::from_millis(1)));
        assert!(!s.scrambled(t + REFUSAL_FRESH));
    }

    #[test]
    fn a_soft_hold_outlasts_a_fight_and_ends_on_its_end_or_a_ship_change() {
        let t = Instant::now();
        let later = t + Duration::from_secs(30);
        let mut s = Sense::default();
        let start = ewar(NPC, SHIP, "warpScrambler");
        s.on_push("OnEwarStart", &start, Some(SHIP), t);
        assert!(s.scrambled(later));
        assert!(!s.view(later).hard, "a disruptor is soft");
        s.on_push("OnEwarEnd", &start, Some(SHIP), later);
        assert!(!s.scrambled(later));
        s.on_push("OnEwarStart", &start, Some(SHIP), t);
        s.on_ship(Some(SHIP + 1));
        assert!(!s.scrambled(t));
    }

    #[test]
    fn a_scramble_refusal_is_not_a_failure() {
        for key in REFUSALS {
            assert!(is_refusal(&format!("beyonce.CmdWarpToStuff: {key}")));
            assert!(!counts(&format!("refused: {key}")));
        }
        assert!(counts("refused: NotEnoughEnergy"));
    }

    #[test]
    fn drones_and_a_scrambler_on_grid_fight_and_never_reconnect() {
        let t = Instant::now();
        let mut m = miner(true);
        let mut o = obs(t, true, true);
        o.failures.insert(ActKind::WarpToItem, brain::GIVE_UP_AFTER);
        let (m2, acts) = brain::decide(&m, &o);
        assert!(
            acts.contains(&Act::Drone(DroneAct::Engage(NPC))),
            "{acts:?}"
        );
        assert!(!acts.contains(&Act::Reconnect));
        assert!(m2.scram.is_some());
        // Past the fight window the scrambler is let go and the bot aligns.
        m = m2;
        let late = t + FIGHT;
        let mut o = obs(late, true, true);
        o.failures.insert(ActKind::WarpToItem, brain::GIVE_UP_AFTER);
        let (m3, acts) = brain::decide(&m, &o);
        assert!(!acts.contains(&Act::Reconnect));
        assert!(!acts.contains(&Act::Drone(DroneAct::Engage(NPC))));
        assert!(m3.scram.is_some_and(|e| e.fought.contains(&NPC)));
    }

    #[test]
    fn unarmed_it_aligns_once_then_waits_with_a_growing_back_off() {
        let t = Instant::now();
        let mut m = miner(false);
        let o = obs(t, true, false);
        let acts = decide(&mut m, &o).expect("acts");
        assert_eq!(acts[0], Act::AlignTo(STATION));
        assert!(matches!(acts[1], Act::Wait(_)));
        // Again before the retry: a wait, no second align.
        let o = obs(t + Duration::from_secs(1), true, false);
        let acts = decide(&mut m, &o).expect("acts");
        assert!(matches!(acts.as_slice(), [Act::Wait(_)]), "{acts:?}");
        // At each retry the role is let through, and the wait grows.
        let mut at = t;
        let mut delay = RETRY_FIRST;
        for _ in 0..6 {
            at += delay;
            assert_eq!(m.scram.as_ref().map(|e| e.retry_at), Some(at));
            let o = obs(at, true, false);
            assert_eq!(decide(&mut m, &o), None, "the role tries");
            // The role's own pass in the same turn is let through too.
            assert_eq!(decide(&mut m, &o), None);
            delay = next_delay(delay);
            assert_eq!(m.scram.as_ref().map(|e| e.delay), Some(delay));
        }
        assert_eq!(delay, RETRY_MAX);
    }

    #[test]
    fn the_episode_ends_when_the_scramble_does() {
        let t = Instant::now();
        let mut m = miner(false);
        decide(&mut m, &obs(t, true, false));
        assert!(m.scram.is_some());
        assert_eq!(decide(&mut m, &obs(t + RETRY_FIRST, false, false)), None);
        assert!(m.scram.is_none());
    }

    #[test]
    fn reconnect_is_held_in_space_under_a_scramble_or_fire_only() {
        let t = Instant::now();
        let m = miner(false);
        assert!(!reconnect_allowed(&m, &obs(t, true, false)));
        assert!(reconnect_allowed(&m, &obs(t, false, false)));
        // Shot at, not scrambled.
        let mut o = obs(t, false, true);
        o.park.as_mut().unwrap().hostiles[0].shooting = Some(SHIP);
        assert!(!reconnect_allowed(&m, &o));
        // Docked is unchanged.
        let mut o = obs(t, true, true);
        o.place = brain::Place::Docked { station: STATION };
        assert!(reconnect_allowed(&m, &o));
    }

    #[test]
    fn a_failing_warp_and_a_live_scram_do_not_reconnect_in_the_brain() {
        let t = Instant::now();
        let m = miner(false);
        let mut o = obs(t, true, false);
        o.failures.insert(ActKind::WarpToItem, brain::GIVE_UP_AFTER);
        let (_, acts) = brain::decide(&m, &o);
        assert!(!acts.contains(&Act::Reconnect), "{acts:?}");
        // The same failures with no scramble still reconnect (unchanged).
        let mut o = obs(t, false, false);
        o.failures.insert(ActKind::WarpToItem, brain::GIVE_UP_AFTER);
        let (_, acts) = brain::decide(&m, &o);
        assert_eq!(acts, vec![Act::Reconnect]);
    }
}
