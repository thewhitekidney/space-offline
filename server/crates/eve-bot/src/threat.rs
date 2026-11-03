//! **A civilian's threat sense** (THREAT, S3 of the EvEJS bot comparison): a
//! miner, foreman, hauler or any other non-combat bot that a **player-style
//! pilot** (a human, or a bot the relations call hostile; never a friend, a
//! fleet mate or an NPC) locks or shoots leaves the way a person would --
//! warps to its station and docks, calls up to [`MAX_GUARDS`] armed corp mates
//! with the existing `NeedBackup` and says an under-attack line in Local
//! ([`crate::chatter::under_attack`]). Corp mates on the grid of a shot corp
//! mate leave too ([`Cause::Mate`]).
//!
//! **EvEJS first, as a hint** (`brain/src/bubble/threat.mts`,
//! `orders/distress.mts`): EvEJS detects a lock list (`GetTargeters`) and a
//! shield/armour drop with a stranger on grid, lies low for a window chosen
//! by cause (locked 4-8 min, shot 5-10, mate 3-6, bystander 2-4, rats 1-2)
//! and has an attacked corp member call up to 3 awake guards of its corp.
//! Those windows are **[A]** here (falsified by a human round in which the
//! bots sit docked long after a harmless lock, or are caught undocking into
//! the same gun). What is ours: the owner's addition, below.
//!
//! **The decision once docked.** The window is a *minimum*, not a sentence.
//! The bot reads Local's roster ([`Scene::local`], the chat's presences):
//!
//! - none of the aggressors (by character) is in Local any more: undock and
//!   go back to work **at once**;
//! - one is still there when the window ends: extend once by half a window;
//! - still there after that: leave the belt it was hit at for another
//!   ([`State::avoid_belt`]) and go back to work.
//!
//! With no chat there is no roster, and the window alone decides. Traits tilt
//! the window ([`window`]): timid bots (low Risk) and content ones (low
//! Greed) stay in longer. **[A]**
//!
//! **What it does not touch**: NPC fire (`defend.rs` drones and the flee on
//! bars), pilots of the bot's own corporation and its fleet boss (the
//! driver never offers them as foes, so an allied dev roster never runs from
//! the owner), and any policy with [`crate::brain::Policy::threat`] off
//! ([`crate::brain::Policy::fast`]).

use crate::brain::{Act, Homing, Memory, Observation, Place};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// Guards a call brings, at most (EvEJS `MAX_RESPONDERS`) **[A]**.
pub const MAX_GUARDS: usize = 3;
/// A call holds this long on the board **[A]** (EvEJS `DISTRESS_MINUTES`).
pub const CALL_HOLDS: Duration = Duration::from_secs(10 * 60);
/// `R_eff` at or above which a pilot is a friend and never an aggressor:
/// [`crate::society::FRIEND_AT`].
pub const FRIEND_AT: f64 = crate::society::FRIEND_AT;
/// How often a docked bot looks at Local again while it lies low **[A]**.
pub const LIE_LOW_POLL: Duration = Duration::from_secs(5);
/// Locks are asked for (`GetTargeters`) at most this often **[A]**.
pub const LOCK_POLL: Duration = Duration::from_secs(2);
/// A player's gun effect is believed this long after its last word, when no
/// stop came **[A]**.
pub const FIRE_STALE: Duration = Duration::from_secs(60);

/// Why the bot leaves, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    /// A corp mate on the grid is being shot.
    Mate,
    /// A foe's ball appeared in the park with `effects.Uncloak` (CLOAK-2):
    /// he was cloaked beside us. Counts like a lock ([`Cause::Locked`]) for
    /// the flight and the call, with a shorter window.
    Decloak,
    /// A player has a lock on this ship.
    Locked,
    /// A player's gun effect names this ship.
    Shot,
}

impl Cause {
    /// The lie-low window in seconds by cause, `[min, max)` **[A]**
    /// (EvEJS: locked 4-8 min, shot 5-10, mate 3-6).
    pub fn window_secs(self) -> (u64, u64) {
        match self {
            Cause::Decloak => (2 * 60, 4 * 60),
            Cause::Locked => (4 * 60, 8 * 60),
            Cause::Shot => (5 * 60, 10 * 60),
            Cause::Mate => (3 * 60, 6 * 60),
        }
    }

    fn word(self) -> &'static str {
        match self {
            Cause::Decloak => "decloaked beside it",
            Cause::Locked => "locked",
            Cause::Shot => "shot",
            Cause::Mate => "a corp mate shot",
        }
    }
}

/// One aggressor this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Foe {
    pub char_id: i64,
    pub cause: Cause,
}

/// What the driver saw this turn (empty by default).
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// Pilots locking or shooting this ship (or a corp mate on the grid).
    pub foes: Vec<Foe>,
    /// Characters in Local now; `None` without a chat (or before its first
    /// presences).
    pub local: Option<BTreeSet<i64>>,
    /// The bot's Risk trait, `0 ... 1`.
    pub risk: f64,
    /// The bot's Greed trait, `0 ... 1`.
    pub greed: f64,
    /// The bot's Diligence trait, `0 ... 1` (REACT-JITTER: an alert pilot
    /// thinks for less, [`crate::react::think`]).
    pub diligence: f64,
}

impl Default for Scene {
    fn default() -> Scene {
        Scene {
            foes: Vec::new(),
            local: None,
            risk: 0.5,
            greed: 0.5,
            diligence: 0.5,
        }
    }
}

/// Whether the guard call is out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    /// Not made.
    No,
    /// Made, the driver has not posted it yet (a bot outside a fleet has no
    /// fleet to broadcast to; the hall's board takes it).
    Pending,
    /// Posted or broadcast.
    Sent,
}

/// One alert: from the first sighting to the decision to go back to work.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub cause: Cause,
    /// The aggressors, by character.
    pub foes: BTreeSet<i64>,
    /// The belt the bot was on when it was hit.
    pub belt: Option<i64>,
    pub call: Call,
    pub said: bool,
    /// The corp-mate line was said (CHATTER-3), once per alert.
    pub said_mate: bool,
    /// When it docked, and the window's end from then.
    pub docked_at: Option<Instant>,
    pub until: Option<Instant>,
    /// The window was extended once.
    pub extended: bool,
}

/// The memory: the alert in force, and the belt to avoid after a switch.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub alert: Option<Alert>,
    pub avoid_belt: Option<i64>,
}

/// **The window** (pure): the cause's range drawn at `draw` in `[0, 1)`,
/// scaled by the traits -- `1.3 - 0.6 risk` (timid 1.3x, reckless 0.7x) times
/// `1.1 - 0.2 greed` -- and kept within `0.5x .. 1.5x` of the range **[A]**.
pub fn window(cause: Cause, draw: f64, risk: f64, greed: f64) -> Duration {
    let (lo, hi) = cause.window_secs();
    let base = lo as f64 + (hi - lo) as f64 * draw.clamp(0.0, 1.0);
    let tilt =
        ((1.3 - 0.6 * risk.clamp(0.0, 1.0)) * (1.1 - 0.2 * greed.clamp(0.0, 1.0))).clamp(0.5, 1.5);
    Duration::from_secs_f64(base * tilt)
}

/// **May this pilot be an aggressor?** (pure) `r` is the bot's `R_eff` toward
/// the pilot, when it has relations. A friend (`r >= FRIEND_AT`) never is; a
/// bot is one only when the relations call it wary or worse; without
/// relations a human is one and a bot is not **[A]**.
pub fn is_foe(r: Option<f64>, is_bot: bool) -> bool {
    match (r, is_bot) {
        (Some(r), true) => r <= crate::society::WARY_AT,
        (Some(r), false) => r < FRIEND_AT,
        (None, bot) => !bot,
    }
}

/// How near a decloaking foe must be to count **[A]**. Falsifier: a human
/// round in which bots flee a decloak they could not have known of, or sit on
/// one that landed on them.
pub const DECLOAK_RANGE: f64 = 100_000.0;
/// An `effects.Uncloak` is believed this long **[A]**: the server sends the
/// ball and the effect in one flush.
pub const DECLOAK_FRESH: Duration = Duration::from_secs(10);

/// **Does a decloak count?** (pure) `age` is the time since the
/// `effects.Uncloak` FX named the ball, `distance` the metres to it and
/// `foe` the result of [`is_foe`]: a fleet mate, a friend or an NPC never.
pub fn decloak_cause(age: Duration, distance: f64, foe: bool) -> Option<Cause> {
    (foe && age < DECLOAK_FRESH && distance <= DECLOAK_RANGE).then_some(Cause::Decloak)
}

/// The aggressors of `foes` that are in Local (pure).
pub fn present(foes: &BTreeSet<i64>, local: &BTreeSet<i64>) -> BTreeSet<i64> {
    foes.intersection(local).copied().collect()
}

/// **The rule** (module header), for both places. `None`: the role decides.
/// Called before the role's own rules (and before a friend's call for help).
pub fn act(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if !m.policy.threat || !m.policy.role.civilian {
        m.threat.alert = None;
        return None;
    }
    match o.place {
        Place::Docked { .. } => docked(m, o),
        Place::InSpace { bound: true, .. } => in_space(m, o),
        _ => None,
    }
}

fn in_space(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    let park = o.park.as_ref()?;
    if !o.threat.foes.is_empty() {
        // REACT-JITTER: the first sight of a new aggressor takes a moment
        // (cut to the low end once he is on us, `brain::under_fire`); the
        // role carries on meanwhile.
        if m.threat.alert.is_none()
            && let Some(first) = o.threat.foes.iter().map(|f| f.char_id).min()
            && m.thinking(crate::react::Class::Threat, first, o).is_some()
        {
            return None;
        }
        let cause = o.threat.foes.iter().map(|f| f.cause).max()?;
        let belt = m.belt;
        let alert = m.threat.alert.get_or_insert_with(|| Alert {
            cause,
            foes: BTreeSet::new(),
            belt,
            call: Call::No,
            said: false,
            said_mate: false,
            docked_at: None,
            until: None,
            extended: false,
        });
        alert.cause = alert.cause.max(cause);
        alert.foes.extend(o.threat.foes.iter().map(|f| f.char_id));
    }
    let alert = m.threat.alert.as_mut()?;
    let on_me = alert.cause >= Cause::Decloak;
    // CHATTER-3: a corp mate being shot is worth a line too, once.
    if !on_me && !alert.said_mate {
        alert.said_mate = true;
        let line = (o.chat).then(|| say_mate(m, o)).flatten();
        if let Some(line) = line {
            return Some(vec![Act::Say(line)]);
        }
    }
    let alert = m.threat.alert.as_mut()?;
    // The line first: a player types as the alarm sounds. [A]
    if on_me && !alert.said {
        alert.said = true;
        let attacker = alert.foes.iter().next().copied();
        let line = (o.chat).then(|| say(m, o, attacker)).flatten();
        if let Some(line) = line {
            return Some(vec![Act::Say(line)]);
        }
    }
    let alert = m.threat.alert.as_mut()?;
    if on_me && alert.call == Call::No {
        match (o.fleet, o.ship) {
            (Some(_), Some(ship)) => {
                alert.call = Call::Sent;
                return Some(vec![Act::Fleet(crate::fleet::Order::Broadcast {
                    name: crate::fleet::BROADCAST_NEED_BACKUP,
                    item: ship.ship_id,
                    type_id: None,
                })]);
            }
            // The hall's board, posted by the driver after this turn.
            _ => alert.call = Call::Pending,
        }
    }
    let why = format!(
        "threat: {} by {} pilot(s) in the park",
        alert.cause.word(),
        alert.foes.len()
    );
    if m.homing.is_none() || m.homing == Some(Homing::Threat) {
        m.homing = Some(Homing::Threat);
        m.home_why = Some((Homing::Threat, why));
    }
    m.claim = None;
    Some(crate::brain::go_home(m, o, park))
}

fn say(m: &mut Memory, o: &Observation, attacker: Option<i64>) -> Option<String> {
    let cx = crate::brain::chatter_ctx(o);
    crate::chatter::under_attack(&mut m.chatter, &cx, attacker)
}

fn say_mate(m: &mut Memory, o: &Observation) -> Option<String> {
    let cx = crate::brain::chatter_ctx(o);
    crate::chatter::mate_attacked(&mut m.chatter, &cx)
}

fn docked(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    let draw = if m.threat.alert.as_ref().is_some_and(|a| a.until.is_none()) {
        m.draw()
    } else {
        0.0
    };
    let alert = m.threat.alert.as_mut()?;
    let until = match alert.until {
        Some(t) => t,
        None => {
            // First look docked: the window starts now.
            let t = o.now + window(alert.cause, draw, o.threat.risk, o.threat.greed);
            alert.docked_at = Some(o.now);
            alert.until = Some(t);
            t
        }
    };
    // The owner's addition: are the aggressors still in Local?
    let still = match &o.threat.local {
        Some(local) => !present(&alert.foes, local).is_empty(),
        None => true,
    };
    if o.threat.local.is_some() && !still {
        return release(m, o, false);
    }
    if o.now < until {
        return Some(vec![Act::Wait(LIE_LOW_POLL.min(until - o.now))]);
    }
    if o.threat.local.is_none() {
        // No roster to look at: the window was all there was.
        return release(m, o, false);
    }
    if !alert.extended {
        alert.extended = true;
        let (lo, hi) = alert.cause.window_secs();
        alert.until = Some(until + Duration::from_secs((lo + hi) / 4));
        return Some(vec![Act::Wait(LIE_LOW_POLL)]);
    }
    release(m, o, true)
}

/// Back to work. `switch`: the aggressors are still there, so the belt where
/// they struck is left alone.
fn release(m: &mut Memory, o: &Observation, switch: bool) -> Option<Vec<Act>> {
    let alert = m.threat.alert.take()?;
    if switch {
        m.threat.avoid_belt = alert.belt;
        m.belt = None;
    }
    // Undock without the policy's dwell: the decision is made.
    m.dwell_until = Some(o.now);
    m.idle_next = false;
    None
}

/// A call on the hall's board (a bot outside a fleet cannot broadcast).
#[derive(Debug, Clone, PartialEq)]
pub struct Distress {
    pub caller: i64,
    pub corp: i64,
    pub system: i64,
    pub at: Instant,
    /// The guards that took it.
    pub taken: Vec<i64>,
}

/// Prune the board and let `guard` take a call of its corp in its system,
/// until [`MAX_GUARDS`] have (pure). The caller's character, if any.
pub fn claim(
    board: &mut Vec<Distress>,
    guard: i64,
    corp: i64,
    system: i64,
    now: Instant,
) -> Option<i64> {
    board.retain(|d| now.saturating_duration_since(d.at) < CALL_HOLDS);
    let d = board.iter_mut().find(|d| {
        d.corp == corp
            && d.system == system
            && d.caller != guard
            && d.taken.len() < MAX_GUARDS
            && !d.taken.contains(&guard)
    })?;
    d.taken.push(guard);
    Some(d.caller)
}

/// Post a call: one per caller at a time.
pub fn post(board: &mut Vec<Distress>, d: Distress) {
    board.retain(|x| x.caller != d.caller);
    board.push(d);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{self, Ball, FleetView, Hostile, ParkView, Policy, ShipInfo};
    use std::collections::BTreeMap;

    const STATION: i64 = 60_000_001;
    const SHIP: i64 = 100_000_101;
    const BELT: i64 = 100_000_201;
    const ALICE: i64 = 90_000_002;
    const BOB: i64 = 90_000_003;
    const GUARD: i64 = 90_000_004;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 100.0,
        }
    }

    fn playing() -> Memory {
        let mut p = Policy::fast();
        p.threat = true;
        p.dwell = (Duration::from_secs(30), Duration::from_secs(30));
        Memory::new("Miner1", p)
    }

    fn space(foes: Vec<Foe>) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: true,
        };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(brain::MERLIN),
        });
        o.home = Some(STATION);
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: vec![ball(BELT, 5_000.0)],
            station: Some(ball(STATION, 900_000.0)),
            boss: None,
            taken: BTreeMap::new(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o.threat.foes = foes;
        o
    }

    fn docked(local: Option<&[i64]>) -> Observation {
        let mut o = space(Vec::new());
        o.place = Place::Docked { station: STATION };
        o.park = None;
        o.miner = Some(brain::ModuleInfo {
            item_id: 100_000_102,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            range: 10_000.0,
        });
        o.threat.local = local.map(|l| l.iter().copied().collect());
        o
    }

    fn lock(c: i64) -> Foe {
        Foe {
            char_id: c,
            cause: Cause::Locked,
        }
    }

    /// Run the brain for one turn and keep the memory.
    fn turn(m: &mut Memory, o: &Observation) -> Vec<Act> {
        let (next, acts) = brain::decide(m, o);
        *m = next;
        acts
    }

    #[test]
    fn a_player_lock_sends_a_miner_home_and_posts_a_guard_call() {
        let mut m = playing();
        let acts = turn(&mut m, &space(vec![lock(ALICE)]));
        assert!(
            matches!(acts[..], [Act::WarpToItem { id: STATION, .. }]),
            "{acts:?}"
        );
        assert_eq!(m.homing, Some(Homing::Threat));
        let a = m.threat.alert.as_ref().expect("an alert");
        assert_eq!(a.cause, Cause::Locked);
        assert_eq!(a.call, Call::Pending);
        assert!(a.foes.contains(&ALICE));
    }

    #[test]
    fn a_foe_who_decloaks_beside_a_miner_sends_it_home_but_a_mate_does_not() {
        // The driver's verdict, then the brain's.
        let fresh = Duration::from_secs(1);
        assert_eq!(decloak_cause(fresh, 5_000.0, true), Some(Cause::Decloak));
        assert_eq!(decloak_cause(fresh, 5_000.0, false), None, "a fleet mate");
        assert_eq!(decloak_cause(fresh, DECLOAK_RANGE * 2.0, true), None);
        assert_eq!(decloak_cause(DECLOAK_FRESH, 5_000.0, true), None, "stale");
        let mut m = playing();
        let acts = turn(
            &mut m,
            &space(vec![Foe {
                char_id: ALICE,
                cause: Cause::Decloak,
            }]),
        );
        assert!(
            matches!(acts[..], [Act::WarpToItem { id: STATION, .. }]),
            "{acts:?}"
        );
        let a = m.threat.alert.as_ref().expect("an alert");
        assert_eq!(a.cause, Cause::Decloak);
        assert_eq!(a.call, Call::Pending, "it counts like a lock");
        let (lo, hi) = Cause::Decloak.window_secs();
        assert!(lo < hi && hi <= Cause::Locked.window_secs().0);
    }

    #[test]
    fn in_a_fleet_the_call_is_the_needbackup_broadcast_first() {
        let mut m = playing();
        let mut o = space(vec![lock(ALICE)]);
        o.fleet = Some(FleetView {
            fleet_id: 7,
            boss: None,
        });
        let acts = turn(&mut m, &o);
        assert!(
            matches!(
                &acts[..],
                [Act::Fleet(crate::fleet::Order::Broadcast { name, item: SHIP, .. })]
                    if *name == crate::fleet::BROADCAST_NEED_BACKUP
            ),
            "{acts:?}"
        );
        // Then it goes.
        let acts = turn(&mut m, &o);
        assert!(matches!(acts[..], [Act::WarpToItem { .. }]), "{acts:?}");
    }

    #[test]
    fn it_says_the_line_when_chat_is_up_and_a_human_is_in_local() {
        let mut m = playing();
        let mut o = space(vec![Foe {
            char_id: ALICE,
            cause: Cause::Shot,
        }]);
        o.chat = true;
        o.talk.local_humans = 1;
        let acts = turn(&mut m, &o);
        assert!(matches!(acts[..], [Act::Say(_)]), "{acts:?}");
        // The next turn is the call and the flight.
        let acts = turn(&mut m, &o);
        assert!(matches!(acts[..], [Act::WarpToItem { .. }]), "{acts:?}");
    }

    #[test]
    fn it_undocks_at_once_when_the_aggressor_has_left_local() {
        let mut m = playing();
        turn(&mut m, &space(vec![lock(ALICE)]));
        // Docked, Alice still in Local: waits.
        let acts = turn(&mut m, &docked(Some(&[ALICE, BOB])));
        assert!(matches!(acts[..], [Act::Wait(_)]), "{acts:?}");
        assert!(m.threat.alert.is_some());
        // Alice gone: the dwell is skipped and it undocks now.
        let acts = turn(&mut m, &docked(Some(&[BOB])));
        assert!(matches!(acts[..], [Act::Undock]), "{acts:?}");
        assert!(m.threat.alert.is_none());
    }

    #[test]
    fn it_waits_while_the_aggressor_stays_then_extends_once_then_switches_belt() {
        let mut m = playing();
        let mut o = space(vec![lock(ALICE)]);
        m.belt = Some(BELT);
        turn(&mut m, &o);
        let t0 = Instant::now();
        let mut d = docked(Some(&[ALICE]));
        d.now = t0;
        let acts = turn(&mut m, &d);
        assert!(matches!(acts[..], [Act::Wait(_)]), "{acts:?}");
        let until = m
            .threat
            .alert
            .as_ref()
            .and_then(|a| a.until)
            .expect("window");
        let (lo, hi) = Cause::Locked.window_secs();
        let len = until - t0;
        assert!(
            len >= Duration::from_secs(lo / 2) && len <= Duration::from_secs(hi * 3 / 2),
            "{len:?}"
        );
        // Window over, still there: one extension.
        d.now = until + Duration::from_secs(1);
        turn(&mut m, &d);
        let a = m.threat.alert.as_ref().expect("still lying low");
        assert!(a.extended);
        let until2 = a.until.expect("extended window");
        assert!(until2 > until);
        // Over again, still there: another belt, and back to work.
        d.now = until2 + Duration::from_secs(1);
        let acts = turn(&mut m, &d);
        assert!(matches!(acts[..], [Act::Undock]), "{acts:?}");
        assert_eq!(m.threat.avoid_belt, Some(BELT));
        assert_eq!(m.belt, None);
        o.threat.foes.clear();
    }

    #[test]
    fn without_a_chat_the_window_alone_decides() {
        let mut m = playing();
        turn(&mut m, &space(vec![lock(ALICE)]));
        let t0 = Instant::now();
        let mut d = docked(None);
        d.now = t0;
        assert!(matches!(turn(&mut m, &d)[..], [Act::Wait(_)]));
        let until = m
            .threat
            .alert
            .as_ref()
            .and_then(|a| a.until)
            .expect("window");
        d.now = until + Duration::from_secs(1);
        assert!(matches!(turn(&mut m, &d)[..], [Act::Undock]));
    }

    #[test]
    fn a_corp_mate_shot_makes_it_say_one_mate_line_and_leave_without_a_call() {
        let mut m = playing();
        let mut o = space(vec![Foe {
            char_id: ALICE,
            cause: Cause::Mate,
        }]);
        o.chat = true;
        o.talk.local_humans = 1;
        let acts = turn(&mut m, &o);
        assert!(matches!(acts[..], [Act::Say(_)]), "{acts:?}");
        let acts = turn(&mut m, &o);
        assert!(matches!(acts[..], [Act::WarpToItem { .. }]), "{acts:?}");
        assert_eq!(m.threat.alert.as_ref().map(|a| a.call), Some(Call::No));
    }

    #[test]
    fn npc_fire_keeps_the_old_path() {
        let mut m = playing();
        let mut o = space(Vec::new());
        o.park.as_mut().expect("a park").hostiles = vec![Hostile {
            ball: ball(100_000_900, 3_000.0),
            type_id: 1,
            name: "Rat".to_owned(),
            shooting: Some(SHIP),
        }];
        assert_eq!(act(&mut m, &o), None);
        assert!(m.threat.alert.is_none());
    }

    #[test]
    fn a_non_civilian_and_a_policy_with_the_sense_off_ignore_it() {
        let o = space(vec![lock(ALICE)]);
        let mut off = Memory::new("Miner2", Policy::fast());
        assert_eq!(act(&mut off, &o), None);
        let mut p = Policy::fast();
        p.threat = true;
        p.role = crate::roles::parse("escort").expect("the escort row");
        let mut escort = Memory::new("Escort1", p);
        assert!(!escort.policy.role.civilian);
        assert_eq!(act(&mut escort, &o), None);
    }

    #[test]
    fn a_dev_roster_bot_runs_from_a_stranger_too() {
        let mut p = Policy::fast();
        p.threat = true;
        p.dev = Some(std::sync::Arc::new(crate::dev::DevBot {
            group: "g".to_owned(),
            ticker: "T".to_owned(),
            skills: crate::dev::Skills::All5,
            skill_ids: None,
            hull: None,
            home: None,
            boss: false,
            hall: std::sync::Arc::default(),
        }));
        let mut m = Memory::new("Dev1", p);
        assert!(act(&mut m, &space(vec![lock(ALICE)])).is_some());
        assert!(m.threat.alert.is_some());
    }

    #[test]
    fn a_friend_is_never_an_aggressor_and_a_stranger_is() {
        assert!(!is_foe(Some(FRIEND_AT), false));
        assert!(!is_foe(Some(9.0), false));
        assert!(is_foe(Some(0.0), false));
        assert!(is_foe(Some(-1.0), false));
        // A bot only when the relations call it wary or worse.
        assert!(!is_foe(Some(0.0), true));
        assert!(is_foe(Some(crate::society::WARY_AT), true));
        // No relations: a human is, a bot is not.
        assert!(is_foe(None, false));
        assert!(!is_foe(None, true));
    }

    #[test]
    fn the_window_follows_the_cause_and_the_traits() {
        let mid = |c, risk, greed| window(c, 0.5, risk, greed);
        assert!(mid(Cause::Shot, 0.5, 0.5) > mid(Cause::Locked, 0.5, 0.5));
        assert!(mid(Cause::Locked, 0.5, 0.5) > mid(Cause::Mate, 0.5, 0.5) / 2);
        assert!(
            mid(Cause::Shot, 0.0, 0.5) > mid(Cause::Shot, 1.0, 0.5),
            "timid sits longer"
        );
        assert!(
            mid(Cause::Shot, 0.5, 0.0) > mid(Cause::Shot, 0.5, 1.0),
            "greedy sits shorter"
        );
        let (lo, hi) = Cause::Shot.window_secs();
        let w = window(Cause::Shot, 1.0, 0.0, 0.0);
        assert!(w <= Duration::from_secs_f64(hi as f64 * 1.5));
        assert!(window(Cause::Shot, 0.0, 1.0, 1.0) >= Duration::from_secs_f64(lo as f64 * 0.5));
    }

    #[test]
    fn the_board_gives_a_call_to_three_armed_corp_mates_of_its_system() {
        let now = Instant::now();
        let mut board = Vec::new();
        post(
            &mut board,
            Distress {
                caller: ALICE,
                corp: 98_000_001,
                system: 30_000_001,
                at: now,
                taken: Vec::new(),
            },
        );
        // Another corp, another system, the caller itself: nothing.
        assert_eq!(claim(&mut board, GUARD, 98_000_002, 30_000_001, now), None);
        assert_eq!(claim(&mut board, GUARD, 98_000_001, 30_000_002, now), None);
        assert_eq!(claim(&mut board, ALICE, 98_000_001, 30_000_001, now), None);
        for g in 0..MAX_GUARDS as i64 {
            assert_eq!(
                claim(&mut board, GUARD + g, 98_000_001, 30_000_001, now),
                Some(ALICE)
            );
        }
        assert_eq!(claim(&mut board, BOB, 98_000_001, 30_000_001, now), None);
        // A call goes stale.
        assert_eq!(
            claim(&mut board, BOB, 98_000_001, 30_000_001, now + CALL_HOLDS),
            None
        );
        assert!(board.is_empty());
    }
}
