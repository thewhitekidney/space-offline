//! Crimewatch: the weapons, log-off, suspect and criminal timers, limited
//! engagements, the safety setting, kill rights and duels, and who is told.
//!
//! Owned by the crimewatch lanes (`docs/lanes/crimewatch.md`, then
//! `docs/lanes/cw-1.md`). Spec: `docs/re/damage-death.md` §7 and those notes.
//!
//! **What the client wants from a server, and all of it.** The client's
//! `crimewatchSvc` keeps five timers, a table of flagged characters, a dict
//! of limited engagements and a safety level, and the server moves them by
//! these doors (`crimewatchSvc.py`) **[V]**:
//!
//! - `CharGetCrimewatchLocation().GetClientStates()` on every `locationid` /
//!   `charid` change (`ProcessSessionChange:124-140`) and on every
//!   `solarsystemid` / `shipid` change for the safety alone
//!   (`OnSessionChanged`): the whole state, [`client_states`];
//! - `OnWeaponsTimerUpdate` / `OnPvpTimerUpdate` / `OnNpcTimerUpdate` /
//!   `OnCriminalTimerUpdate(state, expiryTime)` (`:280-299`): one timer of
//!   one's own, [`TimerUpdate`];
//! - `OnSystemCriminalFlagUpdates(newIdles, newSuspects, newCriminals)`
//!   (`:306-308`): somebody else's flag, [`FlagUpdate`] -- **what colours the
//!   overview**: `state.CheckCriminal` / `CheckSuspect` ask
//!   `crimewatchSvc.IsCriminal(charID)` / `IsSuspect(charID)` of the slim's
//!   `charID` (`parklife/state.py`), red and yellow;
//! - `OnCrimewatchEngagementCreated(otherCharId, timeout)`,
//!   `...StartTimeout(otherCharId, timeout)`, `...StopTimeout(otherCharId)`
//!   and `...Ended(otherCharId)` (`:314-335`): limited engagements, which
//!   colour the overview turquoise (`CheckLimitedEngagement`);
//! - `SetSafetyLevel(level)` from the client, stored and answered back as
//!   `GetClientStates`' fourth element ([`service`]);
//! - kill rights through `bountyProxy` and `killRightMgr` ([`killright`]),
//!   duels through `StartDuelChallenge` / `RespondToDuelChallenge`
//!   ([`duel`]).
//!
//! **What each timer means is the client's own tooltip** (shipped
//! localisation, `tools/loctable.py id`) **[V]**:
//!
//! | timer | tooltip (message id) | cause | consequence |
//! |---|---|---|---|
//! | weapons | 283268 | "Aggression against another capsuleer" | "Unable to dock or use stargates or fitting services" |
//! | pvp | 283269 "Capsuleer Log-Off Timer" | "Combat with another capsuleer" | "Ship will remain in space on log-off" |
//! | npc | 283270 "Non-Capsuleer Log-Off Timer" | "Combat with a non-capsuleer" | the same |
//! | suspect | 283271 | a "suspect"-level offense | "Capsuleers may freely engage" |
//! | criminal | 283272 | a "criminal"-level offense | "... CONCORD will engage in high-sec space" |
//!
//! and how long each runs is `crimewatch/const.py:32-37` **[V]**: weapons
//! 60 s, pvp 15 min, npc 5 min, criminal/suspect 15 min, an engagement 5 min
//! (`crimewatchEngagementDuration`), in blue time (`SEC` = 10 000 000).
//!
//! **Who becomes what** is [`rules::crime`], pure, and the client's own
//! rules; [`aggression`], [`assistance`] and [`theft_from`] apply it, and
//! every crime is offered to [`ON_CRIME`], **the seam CONCORD-1 and SEC-1
//! subscribe to** ([`CrimeEvent`]).
//!
//! **The docking and jump refusals are ours to raise.** The client checks
//! nothing before `CmdDock` / `CmdStargateJump`: `HaveWeaponsTimer` has two
//! readers in the whole tree, both menu hints (`baseCheckers.py:51`,
//! `celestialCheckers.OfferAccessHangarTransfer:184`) **[V]**, and
//! `CantDockAfterAggression` / `CantJumpAfterAggression` have **no** raise
//! site in the client (`findings/user-errors.tsv:999`, `:1038`, column
//! `raise_sites` = 0) **[V]** -- so they are the server's words, and their
//! text says so: "The station denies you permission to dock for the moment
//! due to your recent acts of aggression" (260094) **[V]**. [`refuse_dock`],
//! [`refuse_jump`]. So are the safety refusals ([`attack_refusal`],
//! [`theft_refusal`]): the client refuses first, the server is the backstop
//! for a bot or a client that did not.
//!
//! **Where the state lives**: inside `combat::Combat` (`Combat::crimewatch`),
//! because combat is what moves it and that keeps `Registry` unchanged.
//! Not persisted, for `Combat`'s reason: a restart takes every ship out of
//! space, and the timers with them `[A]`. **Persisted** as delta rows: the
//! safety setting ([`SAFETY_KIND`]) and kill rights ([`killright::KIND`]).

pub mod duel;
pub mod killright;
pub mod rules;
pub mod service;

pub use rules::{Act, CrimeKind, Safety, Space, Target, crime};

use crate::damage::BlueTime;
use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// `carbon.common.lib.const.SEC`: one second of blue time.
pub const SEC: BlueTime = 10_000_000;
/// `carbon.common.lib.const.MIN`.
pub const MIN: BlueTime = 60 * SEC;
/// One day of blue time.
pub const DAY: BlueTime = 24 * 60 * MIN;

/// `crimewatch/const.py:32-37` **[V]**.
pub const WEAPONS_TIMEOUT: BlueTime = 60 * SEC;
pub const PVP_TIMEOUT: BlueTime = 15 * MIN;
pub const NPC_TIMEOUT: BlueTime = 5 * MIN;
pub const CRIMINAL_TIMEOUT: BlueTime = 15 * MIN;
/// `crimewatchEngagementDuration` **[V]**: how long a limited engagement
/// lasts after the last aggression between the two.
pub const ENGAGEMENT_DURATION: BlueTime = 5 * MIN;
/// `crimewatchEngagementTimeoutOngoing` **[V]**: the timeout an engagement
/// carries while the fight is on.
pub const ENGAGEMENT_ONGOING: i64 = -1;

/// How long after the last volley a timer still reads **active** -- full
/// gauge, "you are fighting now" -- before it starts counting down.
///
/// `[A]`. TQ holds a timer active while an offensive module is running; we
/// see volleys, not modules, so "a volley in the last ten seconds" stands in
/// for "a gun is cycling" (the civilian guns cycle in 2-4 s). The value only
/// decides when the HUD's ring starts to empty: the **expiry** it then shows
/// is the last volley plus the timeout either way, which is also what the
/// refusals read. Falsified by a ring that empties while the guns still
/// fire (raise this), or one that sits full long after they stopped. An
/// engagement reads "ongoing" for the same window.
pub const ACTIVE_GRACE: BlueTime = 10 * SEC;

/// `crimewatch/const.py:7-29` **[V]**: each timer's own state numbers.
/// `0` is none of them -- and `HaveWeaponsTimer` is `state !=
/// weaponsTimerStateIdle` (`crimewatchSvc.py:424`), so a `0` reads as
/// **armed**.
pub mod state {
    pub const WEAPONS_IDLE: i64 = 100;
    pub const WEAPONS_ACTIVE: i64 = 101;
    pub const WEAPONS_TIMER: i64 = 102;
    pub const WEAPONS_INHERITED: i64 = 103;
    pub const PVP_IDLE: i64 = 200;
    pub const PVP_ACTIVE: i64 = 201;
    pub const PVP_TIMER: i64 = 202;
    pub const PVP_INHERITED: i64 = 203;
    pub const CRIMINAL_IDLE: i64 = 300;
    pub const ACTIVE_CRIMINAL: i64 = 301;
    pub const ACTIVE_SUSPECT: i64 = 302;
    pub const TIMER_CRIMINAL: i64 = 303;
    pub const TIMER_SUSPECT: i64 = 304;
    pub const INHERITED_CRIMINAL: i64 = 305;
    pub const INHERITED_SUSPECT: i64 = 306;
    pub const NPC_IDLE: i64 = 400;
    pub const NPC_ACTIVE: i64 = 401;
    pub const NPC_TIMER: i64 = 402;
    pub const NPC_INHERITED: i64 = 403;
    pub const DISAPPROVAL_IDLE: i64 = 500;
}

/// The four timers this server runs. Disapproval (`500`s) is always idle:
/// it is an insurgency standing penalty nothing here incurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timer {
    Weapons,
    Pvp,
    Npc,
    /// The criminal timer, in its suspect or its criminal states.
    Criminal,
}

impl Timer {
    pub const ALL: [Timer; 4] = [Timer::Weapons, Timer::Pvp, Timer::Npc, Timer::Criminal];

    fn index(self) -> usize {
        self as usize
    }

    pub fn timeout(self) -> BlueTime {
        match self {
            Timer::Weapons => WEAPONS_TIMEOUT,
            Timer::Pvp => PVP_TIMEOUT,
            Timer::Npc => NPC_TIMEOUT,
            Timer::Criminal => CRIMINAL_TIMEOUT,
        }
    }

    /// `None`: a suspect or criminal flag has no "active" phase here -- it
    /// starts counting the moment the offence is done (`TimerSuspect` /
    /// `TimerCriminal`, which the HUD draws as a countdown,
    /// `crimewatchTimers.OnCriminalTimerUpdate`) `[A]`. Each new offence
    /// starts it again.
    fn grace(self) -> Option<BlueTime> {
        match self {
            Timer::Criminal => None,
            _ => Some(ACTIVE_GRACE),
        }
    }

    /// The push that moves it, `(state, expiryTime)` for all four
    /// (`crimewatchSvc.py:280-299`) **[V]**.
    pub fn event(self) -> &'static str {
        match self {
            Timer::Weapons => "OnWeaponsTimerUpdate",
            Timer::Pvp => "OnPvpTimerUpdate",
            Timer::Npc => "OnNpcTimerUpdate",
            Timer::Criminal => "OnCriminalTimerUpdate",
        }
    }

    /// `(idle, active, counting, inherited)` for the level the timer runs at
    /// (only the criminal timer has two).
    fn states(self, level: CrimeKind) -> (i64, i64, i64, i64) {
        use state::*;
        match (self, level) {
            (Timer::Weapons, _) => (
                WEAPONS_IDLE,
                WEAPONS_ACTIVE,
                WEAPONS_TIMER,
                WEAPONS_INHERITED,
            ),
            (Timer::Pvp, _) => (PVP_IDLE, PVP_ACTIVE, PVP_TIMER, PVP_INHERITED),
            (Timer::Npc, _) => (NPC_IDLE, NPC_ACTIVE, NPC_TIMER, NPC_INHERITED),
            (Timer::Criminal, CrimeKind::Suspect) => (
                CRIMINAL_IDLE,
                ACTIVE_SUSPECT,
                TIMER_SUSPECT,
                INHERITED_SUSPECT,
            ),
            (Timer::Criminal, CrimeKind::Criminal) => (
                CRIMINAL_IDLE,
                ACTIVE_CRIMINAL,
                TIMER_CRIMINAL,
                INHERITED_CRIMINAL,
            ),
        }
    }
}

/// Where one timer is, as the client draws it: the HUD shows a full,
/// animated ring for an active (or inherited) state with `expiryTime =
/// None`, a countdown for a timer state with an expiry, and deletes the ring
/// otherwise (`crimewatchTimers.OnWeaponsTimerUpdate:659-666`) **[V]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Active,
    /// Active because of somebody else's fight: assisting a pilot under a
    /// timer gives the helper the same timer in its `Inherited` state, which
    /// the HUD draws as an active one (`state in (Active, Inherited)`,
    /// `crimewatchTimers.py`) **[V]**.
    Inherited,
    Counting(BlueTime),
}

impl Phase {
    /// `(state, expiryTime)`, the pair every door carries. `level` matters
    /// for the criminal timer only.
    pub fn wire(self, timer: Timer, level: CrimeKind) -> (i64, Option<BlueTime>) {
        let (idle, active, counting, inherited) = timer.states(level);
        match self {
            Phase::Idle => (idle, None),
            Phase::Active => (active, None),
            Phase::Inherited => (inherited, None),
            Phase::Counting(at) => (counting, Some(at)),
        }
    }
}

/// The `(state, expiryTime)` pair a client was last sent.
type Wire = (i64, Option<BlueTime>);

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Clock {
    /// The last act that (re)started it.
    last: Option<BlueTime>,
    /// That act was somebody else's fight, inherited by assisting
    /// ([`assistance`]).
    inherited: bool,
    /// What the client was last told; `None` is "nothing yet", which reads
    /// as idle.
    told: Option<Wire>,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Record {
    clocks: [Clock; 4],
    /// The criminal timer's level while it runs: the worst offence since it
    /// last ran out.
    level: Option<CrimeKind>,
    /// The system whose pilots were told this character's flag, and which.
    announced: Option<(i64, CrimeKind)>,
}

/// One limited engagement between two characters (`(low id, high id)`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Engagement {
    /// The last aggression between the two (or the duel's acceptance).
    last: BlueTime,
    /// The timeout both were last told; `None` before the creation push.
    told: Option<i64>,
}

/// A kill right being exercised: its target is a suspect for the
/// activation's fifteen minutes, and a kill of the target's ship in that
/// window uses the right up ([`killright`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activation {
    pub kill_right_id: i64,
    pub activator: i64,
    pub until: BlueTime,
}

/// Every character with a timer running, or a flag somebody was told about;
/// every engagement, duel offer and kill-right activation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Crimewatch {
    records: BTreeMap<i64, Record>,
    /// Ships left in space by a pilot with no session: character id to the
    /// instant the hold is next looked at ([`logged_off`], [`release_holds`]).
    held: BTreeMap<i64, BlueTime>,
    /// Pilots back in a held ship whose ballpark bind has not come yet
    /// ([`reclaim`], [`take_rejoin`]).
    rejoining: BTreeSet<i64>,
    engagements: BTreeMap<(i64, i64), Engagement>,
    /// `(offender, victim) -> when`: the last criminal act of one capsuleer
    /// against another, for the kill right a kill then earns
    /// ([`killright::on_kill`]). Forgotten after [`CRIMINAL_TIMEOUT`].
    criminal_acts: BTreeMap<(i64, i64), BlueTime>,
    /// Kill-right activations, by target.
    activations: BTreeMap<i64, Activation>,
    /// Duel offers: `(challenger, challenged) -> expiry` ([`duel`]).
    duels: BTreeMap<(i64, i64), BlueTime>,
}

fn pair(a: i64, b: i64) -> (i64, i64) {
    if a <= b { (a, b) } else { (b, a) }
}

impl Crimewatch {
    /// When `character_id`'s logged-off ship is next due to leave space, if
    /// it is being held there.
    pub fn held_until(&self, character_id: i64) -> Option<BlueTime> {
        self.held.get(&character_id).copied()
    }

    /// `timer`'s phase for `character_id` at `now`.
    pub fn phase(&self, character_id: i64, timer: Timer, now: BlueTime) -> Phase {
        self.records.get(&character_id).map_or(Phase::Idle, |r| {
            phase_at(timer, &r.clocks[timer.index()], now)
        })
    }

    /// When `timer` runs out for `character_id`, if it is running at `now`.
    pub fn expiry(&self, character_id: i64, timer: Timer, now: BlueTime) -> Option<BlueTime> {
        let last = self
            .records
            .get(&character_id)?
            .clocks
            .get(timer.index())?
            .last?;
        Some(last + timer.timeout()).filter(|end| now < *end)
    }

    /// `character_id`'s criminal flag at `now`: suspect, criminal, or none.
    pub fn flag(&self, character_id: i64, now: BlueTime) -> Option<CrimeKind> {
        self.expiry(character_id, Timer::Criminal, now)?;
        self.records.get(&character_id)?.level
    }

    /// Is there a limited engagement between `a` and `b` at `now`?
    pub fn engaged(&self, a: i64, b: i64, now: BlueTime) -> bool {
        self.engagements
            .get(&pair(a, b))
            .is_some_and(|e| now < e.last + ENGAGEMENT_DURATION)
    }

    /// Is `character_id` in any limited engagement at `now`?
    pub fn in_engagement(&self, character_id: i64, now: BlueTime) -> bool {
        self.engagements_of(character_id, now).next().is_some()
    }

    /// `(other, timeout)` for every engagement of `character_id` at `now`,
    /// the timeout as the client is told it.
    pub fn engagements_of(
        &self,
        character_id: i64,
        now: BlueTime,
    ) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.engagements.iter().filter_map(move |(&(a, b), e)| {
            let other = if a == character_id {
                b
            } else if b == character_id {
                a
            } else {
                return None;
            };
            engagement_wire(e.last, now).map(|t| (other, t))
        })
    }

    /// The live kill-right activation on `target`, if any.
    pub fn activation(&self, target: i64, now: BlueTime) -> Option<Activation> {
        self.activations
            .get(&target)
            .copied()
            .filter(|a| now < a.until)
    }

    pub(crate) fn start(&mut self, character_id: i64, timer: Timer, at: BlueTime, inherited: bool) {
        let clock = &mut self.records.entry(character_id).or_default().clocks[timer.index()];
        if clock.last.is_none_or(|l| at >= l) {
            clock.last = Some(at);
            clock.inherited = inherited;
        }
    }

    /// Raise `character_id`'s criminal timer to `kind` (never lower it), from
    /// `at`.
    fn flag_at(&mut self, character_id: i64, kind: CrimeKind, at: BlueTime, now: BlueTime) {
        let running = self.flag(character_id, now);
        self.start(character_id, Timer::Criminal, at, false);
        let r = self.records.entry(character_id).or_default();
        r.level = Some(running.map_or(kind, |k| k.max(kind)));
    }

    fn touch_engagement(&mut self, a: i64, b: i64, at: BlueTime) {
        let e = self.engagements.entry(pair(a, b)).or_default();
        e.last = e.last.max(at);
    }
}

/// The timeout an engagement whose last aggression was `last` shows at
/// `now`: ongoing inside [`ACTIVE_GRACE`], then the end; `None` once over.
fn engagement_wire(last: BlueTime, now: BlueTime) -> Option<i64> {
    let end = last + ENGAGEMENT_DURATION;
    if now >= end {
        None
    } else if now < last + ACTIVE_GRACE {
        Some(ENGAGEMENT_ONGOING)
    } else {
        Some(end)
    }
}

fn phase_at(timer: Timer, clock: &Clock, now: BlueTime) -> Phase {
    let Some(last) = clock.last else {
        return Phase::Idle;
    };
    let end = last + timer.timeout();
    if now >= end {
        return Phase::Idle;
    }
    match timer.grace() {
        Some(g) if now < last + g && clock.inherited => Phase::Inherited,
        Some(g) if now < last + g => Phase::Active,
        _ => Phase::Counting(end),
    }
}

// ---------------------------------------------------------------------------
// The seam: every crime, for CONCORD-1 and SEC-1
// ---------------------------------------------------------------------------

/// What kind of act a [`CrimeEvent`] was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActKind {
    Attack,
    Assist,
    Theft,
}

/// **One crime, as it is committed** -- what CONCORD-1 (the response in high
/// sec) and SEC-1 (the security status loss) subscribe to through
/// [`ON_CRIME`]. Fired after the offender's flag is raised and told.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrimeEvent {
    pub offender: i64,
    /// The capsuleer wronged: the target's owner, the assisted pilot, the
    /// container's owner. `None` when nobody is.
    pub victim: Option<i64>,
    pub kind: CrimeKind,
    pub act: ActKind,
    pub solar_system_id: i64,
    pub space: Space,
    pub at: BlueTime,
}

/// A subscriber to [`ON_CRIME`].
pub type CrimeHook = fn(&mut Registry, &CrimeEvent);

/// **Every subscriber to a crime, in order. Coordinator-only**, like
/// `kill_hooks::PLAYER`: CONCORD-1 exports `concord::on_crime` and names the
/// row in its report. SEC-1's status loss is the first row: a criminal act
/// costs security status (`sec_status::on_crime_event`).
pub const ON_CRIME: &[CrimeHook] = &[
    crate::sec_status::on_crime_event,
    // CONCORD answers a criminal act in high sec (CONCORD-1a).
    crate::concord::on_crime,
];

/// Run `hooks` for `ev`, in order, each once.
pub fn fire_crime(hooks: &[CrimeHook], w: &mut Registry, ev: &CrimeEvent) {
    #[cfg(test)]
    CRIMES.with(|c| c.borrow_mut().push(*ev));
    for hook in hooks {
        hook(w, ev);
    }
}

#[cfg(test)]
thread_local! {
    /// Every event [`fire_crime`] was given on this test thread, in order.
    pub(crate) static CRIMES: std::cell::RefCell<Vec<CrimeEvent>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
mod cw1_tests;

/// `offender` committed `kind` at `at`: flag them, remember a criminal act
/// against a victim (for the kill right), tell everybody, and fire
/// [`ON_CRIME`].
fn commit(w: &mut Registry, ev: CrimeEvent, now: BlueTime) {
    let cw = w.combat_mut().crimewatch_mut();
    cw.flag_at(ev.offender, ev.kind, ev.at, now);
    if let (CrimeKind::Criminal, Some(v)) = (ev.kind, ev.victim) {
        let at = cw.criminal_acts.entry((ev.offender, v)).or_default();
        *at = (*at).max(ev.at);
    }
    crate::outln!(
        "[crimewatch] {} is a {:?}: {:?} against {:?} in {} ({:?})",
        ev.offender,
        ev.kind,
        ev.act,
        ev.victim,
        ev.solar_system_id,
        ev.space
    );
    sync(w, ev.offender, now);
    fire_crime(ON_CRIME, w, &ev);
}

// ---------------------------------------------------------------------------
// What moves the timers
// ---------------------------------------------------------------------------

/// The capsuleer who owns a roster key: the pilot of a ship or capsule, or
/// the owner of a drone. `None` for an NPC.
fn owner_capsuleer(w: &Registry, roster_key: i64) -> Option<i64> {
    let p = w.pilot(roster_key)?;
    p.char_id
        .or_else(|| w.character(p.owner_id).map(|c| c.character_id))
}

/// Is the ball behind `roster_key` a capsule?
fn is_capsule(w: &Registry, roster_key: i64) -> bool {
    w.pilot(roster_key)
        .is_some_and(|p| p.char_id.is_some() && p.ship_type_id == crate::damage::CAPSULE_TYPE_ID)
}

/// The roster key of the ball `ball_id` (a module's or a drone's target):
/// its pilot's character, an NPC's or a drone's own id.
fn roster_key(w: &Registry, ball_id: i64) -> i64 {
    w.roster()
        .by_ship(ball_id)
        .map_or(ball_id, |p| p.character_id)
}

/// What the rules know about `owner` (a capsuleer, or `None`) as the other
/// side of an act by `actor`, at `now`.
fn target_facts(
    w: &Registry,
    actor: i64,
    owner: Option<i64>,
    capsule: bool,
    now: BlueTime,
) -> Target {
    let Some(v) = owner else {
        return Target::default();
    };
    let cw = w.combat().crimewatch();
    let me = w.character(actor);
    let them = w.character(v);
    let corp_friendly_fire = match (me, them) {
        (Some(a), Some(b)) => {
            a.corporation_id == b.corporation_id
                && !is_npc_corporation(a.corporation_id)
                && w.corp_friendly_fire(a.corporation_id)
        }
        _ => false,
    };
    Target {
        capsuleer: true,
        capsule,
        own: v == actor,
        flag: cw.flag(v, now),
        engaged: cw.engaged(actor, v, now),
        in_engagement: cw.in_engagement(v, now),
        outlaw: rules::is_outlaw(crate::npc::bounty::security_status(w, v)),
        fw_enemy: fw_enemies(w, actor, v),
        corp_friendly_fire,
        at_war: at_war(w, actor, v, now),
    }
}

/// `IsCombatEnemyFaction` of the two pilots' militias `[I]`: both enlisted,
/// in different militias that share a warzone (the two sides of every
/// warzone system, `facwar::militias_by_system`).
pub(crate) fn fw_enemies(w: &Registry, a: i64, b: i64) -> bool {
    let fa = crate::service::facwar_mgr::war_faction(w, a);
    let fb = crate::service::facwar_mgr::war_faction(w, b);
    let (Some(fa), Some(fb)) = (fa, fb) else {
        return false;
    };
    fa != fb
        && crate::facwar::militias_by_system(w.content())
            .values()
            .any(|[x, y]| (*x == fa && *y == fb) || (*x == fb && *y == fa))
}

/// Are the two pilots at war (`warRelationshipAtWarCanFight`)? C8's
/// [`crate::war::legal`]: the first war in which the attacker's war entity
/// (corp, and alliance) fights the victim's, cooldown counted.
fn at_war(w: &Registry, a: i64, b: i64, now: BlueTime) -> bool {
    crate::war::legal(w, a, b, now).is_some()
}

/// The security band of `solar_system_id` as the rules read it: zero where
/// flags do not apply ([`flags_apply`]), else the table's class; a system
/// the table lacks is low sec `[A]` (a suspect act rather than a criminal
/// one when nothing says which; falsified by a high-sec system served with
/// no bundle, which then shows the client's criminal warning but earns only
/// a suspect flag).
pub fn space_of(w: &Registry, solar_system_id: i64) -> Space {
    if !flags_apply(w, solar_system_id) {
        return Space::Zero;
    }
    w.content()
        .get::<crate::celestial::Universe>()
        .security_class(solar_system_id)
        .map_or(Space::Low, Space::of)
}

/// The system `character_id` is in (`Registry::session_system`).
fn system_of(w: &Registry, character_id: i64) -> Option<i64> {
    w.session_system(character_id)
}

/// One volley from `attacker` at `victim` (roster keys: a character, an
/// NPC's own id, a drone), at `at`. Hit or miss: the tooltips say
/// *aggression* and *combat*, not damage.
///
/// - a capsuleer at a capsuleer's ship, capsule or drone: the shooter's
///   **weapons** timer ("aggression against another capsuleer"), and
///   **both** pilots' pvp log-off timer ("combat with another capsuleer");
///   then **the verdict** ([`rules::crime`], judged on the flags as they were
///   before this volley): a suspect or criminal flag for the shooter where
///   the act is a crime, and a limited engagement where the target was
///   flagged or the two were already engaged ([`rules::engages`]);
/// - a capsuleer at an NPC, or an NPC at a capsuleer: that capsuleer's npc
///   log-off timer ("combat with a non-capsuleer"), **no** weapons timer.
///
/// All three causes are the tooltips' words **[V]** (the module header).
pub fn aggression(w: &mut Registry, attacker: i64, victim: i64, at: BlueTime, now: BlueTime) {
    let shooter = w.pilot(attacker).and_then(|p| p.char_id);
    let target = owner_capsuleer(w, victim);
    match (shooter, target) {
        // One's own drone, or one's own ship: no fight with anybody.
        (Some(a), Some(v)) if a == v => {}
        (Some(a), Some(v)) => {
            let system = system_of(w, a).unwrap_or(0);
            let space = space_of(w, system);
            // Zero sec judges nothing: skip the lookups on the hot path.
            let facts = if space == Space::Zero {
                Target {
                    capsuleer: true,
                    ..Target::default()
                }
            } else {
                target_facts(w, a, Some(v), is_capsule(w, victim), at.min(now))
            };
            let verdict = crime(space, &Act::Attack(facts));
            let cw = w.combat_mut().crimewatch_mut();
            cw.start(a, Timer::Weapons, at, false);
            cw.start(a, Timer::Pvp, at, false);
            cw.start(v, Timer::Pvp, at, false);
            if rules::engages(space, &facts) {
                cw.touch_engagement(a, v, at);
            }
            sync(w, v, now);
            match verdict {
                Some(kind) => commit(
                    w,
                    CrimeEvent {
                        offender: a,
                        victim: Some(v),
                        kind,
                        act: ActKind::Attack,
                        solar_system_id: system,
                        space,
                        at,
                    },
                    now,
                ),
                None => sync(w, a, now),
            }
            sync_engagements(w, now);
        }
        (Some(a), None) => {
            w.combat_mut()
                .crimewatch_mut()
                .start(a, Timer::Npc, at, false);
            sync(w, a, now);
            // CONCORD-1b: a shot at the law is a crime (high sec).
            crate::concord::attack_on_law(w, a, victim, at, now);
        }
        (None, Some(v)) => {
            w.combat_mut()
                .crimewatch_mut()
                .start(v, Timer::Npc, at, false);
            sync(w, v, now);
        }
        (None, None) => {}
    }
}

/// `helper` assists the ball `target` (a pilot's ship or capsule, or a
/// drone: the module's target id) at `at` -- a remote repair, a shield
/// transfer, any `isAssistance` effect. **For the remote-assistance modules
/// to call** when they exist (none is served yet).
///
/// - the verdict ([`rules::crime`], `Act::Assist`): the target's criminal
///   flag, inherited -- a criminal makes a criminal, a suspect a suspect
///   (`GetSafetyLevelRestrictionForAidingTarget`) **[V]**; a pilot in a
///   limited engagement a suspect `[A]`;
/// - the target's running weapons, pvp and npc timers, **inherited**: the
///   helper's own clocks restart at `at` and read `Inherited` while the
///   target's do (`*TimerStateInherited`, `crimewatch/const.py`) **[V]**
///   states; that assistance passes them on is TQ's rule `[A]`.
pub fn assistance(w: &mut Registry, helper: i64, target: i64, at: BlueTime, now: BlueTime) {
    let Some(v) = owner_capsuleer(w, roster_key(w, target)) else {
        return;
    };
    if v == helper || w.character(helper).is_none() {
        return;
    }
    let system = system_of(w, helper).unwrap_or(0);
    let space = space_of(w, system);
    let facts = target_facts(w, helper, Some(v), false, at.min(now));
    let verdict = crime(space, &Act::Assist(facts));
    let cw = w.combat_mut().crimewatch_mut();
    for timer in [Timer::Weapons, Timer::Pvp, Timer::Npc] {
        if cw.expiry(v, timer, now).is_some() {
            cw.start(helper, timer, at, true);
        }
    }
    match verdict {
        Some(kind) => commit(
            w,
            CrimeEvent {
                offender: helper,
                victim: Some(v),
                kind,
                act: ActKind::Assist,
                solar_system_id: system,
                space,
                at,
            },
            now,
        ),
        None => sync(w, helper, now),
    }
}

/// `thief` took from a container they had no loot rights to, in
/// `solar_system_id`, the owner unknown: [`theft_from`] with no owner.
pub fn theft(w: &mut Registry, thief: i64, solar_system_id: i64, now: BlueTime) {
    theft_from(w, thief, None, solar_system_id, now);
}

/// `thief` took from a container `owner` has the loot rights to, in
/// `solar_system_id` (the caller has already found that the thief has no
/// rights of their own, `combat::has_loot_right`).
///
/// **Not in a zero-sec system** ([`flags_apply`]): there the client's own
/// `CanAttackFreely` answers `True` for everybody (`crimewatchSvc.py
/// :541-543`, `securityClassZeroSec`) **[V]**, so its `HaveLootRight`
/// (`michelle.py:1987`) gives everyone the rights and the take is not
/// theft. **Nor from an owner the thief may attack freely** -- flagged,
/// engaged, a corp mate under friendly fire -- for the same `HaveLootRight`
/// reason. Everywhere else, a suspect act ([`rules::crime`], `Act::Theft`).
pub fn theft_from(
    w: &mut Registry,
    thief: i64,
    owner: Option<i64>,
    solar_system_id: i64,
    now: BlueTime,
) {
    let space = space_of(w, solar_system_id);
    let owner = owner.filter(|o| w.character(*o).is_some());
    let facts = match owner {
        Some(_) => target_facts(w, thief, owner, false, now),
        None => Target {
            capsuleer: true,
            ..Target::default()
        },
    };
    let Some(kind) = crime(space, &Act::Theft(facts)) else {
        return;
    };
    commit(
        w,
        CrimeEvent {
            offender: thief,
            victim: owner,
            kind,
            act: ActKind::Theft,
            solar_system_id,
            space,
            at: now,
        },
        now,
    );
}

/// Make `character_id` a suspect from `now` for [`CRIMINAL_TIMEOUT`]: their
/// own HUD counts it down, and every pilot in their system sees the flag. A
/// criminal stays a criminal. No [`ON_CRIME`] event: this is a flag set by
/// something other than a crime (a kill right's activation, a GM).
pub fn suspect(w: &mut Registry, character_id: i64, now: BlueTime) {
    w.combat_mut()
        .crimewatch_mut()
        .flag_at(character_id, CrimeKind::Suspect, now, now);
    sync(w, character_id, now);
}

/// Make `character_id` a criminal from `at` for [`CRIMINAL_TIMEOUT`]. No
/// [`ON_CRIME`] event: the caller (`concord::attack_on_law`, an attack at
/// CONCORD, a station or a gate) answers the act itself.
pub fn flag_criminal(w: &mut Registry, character_id: i64, at: BlueTime, now: BlueTime) {
    w.combat_mut()
        .crimewatch_mut()
        .flag_at(character_id, CrimeKind::Criminal, at, now);
    sync(w, character_id, now);
}

/// `character_id`'s criminal flag at `now`.
pub fn flag_of(w: &Registry, character_id: i64, now: BlueTime) -> Option<CrimeKind> {
    w.combat().crimewatch().flag(character_id, now)
}

/// Is `character_id` a criminal at `now`? What CONCORD-1 answers.
pub fn is_criminal(w: &Registry, character_id: i64, now: BlueTime) -> bool {
    flag_of(w, character_id, now) == Some(CrimeKind::Criminal)
}

/// Is `character_id` a suspect at `now`? (A criminal is not.)
pub fn is_suspect(w: &Registry, character_id: i64, now: BlueTime) -> bool {
    flag_of(w, character_id, now) == Some(CrimeKind::Suspect)
}

/// Open (or keep open) the limited engagement between `a` and `b` at `at`,
/// and tell both. What an accepted duel does ([`duel`]).
pub fn engage(w: &mut Registry, a: i64, b: i64, at: BlueTime, now: BlueTime) {
    if a == b {
        return;
    }
    w.combat_mut().crimewatch_mut().touch_engagement(a, b, at);
    sync_engagements(w, now);
}

/// End every engagement of `character_id` at once, and tell both sides --
/// a death ends a limited engagement `[A]` (the packet's rule; TQ ends one
/// when either party is destroyed).
pub fn end_engagements(w: &mut Registry, character_id: i64) {
    let ended: Vec<(i64, i64)> = w
        .combat()
        .crimewatch()
        .engagements
        .keys()
        .copied()
        .filter(|(a, b)| *a == character_id || *b == character_id)
        .collect();
    for key in ended {
        let told = w
            .combat_mut()
            .crimewatch_mut()
            .engagements
            .remove(&key)
            .is_some_and(|e| e.told.is_some());
        if told {
            tell_engagement_ended(w, key);
        }
    }
}

fn tell_engagement_ended(w: &mut Registry, (a, b): (i64, i64)) {
    for (to, other) in [(a, b), (b, a)] {
        w.schedule(
            Duration::ZERO,
            Notice::new(to, "OnCrimewatchEngagementEnded", vec![PyValue::Int(other)]),
        );
    }
}

/// **The world clock's hook**, from `combat::tick`: tell every pilot whose
/// timer changed phase, move a flag with its pilot between systems, move
/// every engagement on, forget whoever is clean, drop old criminal acts,
/// stale duel offers and spent activations, and let go of held ships.
pub fn tick(w: &mut Registry, now: BlueTime) {
    let ids: Vec<i64> = w.combat().crimewatch().records.keys().copied().collect();
    for c in ids {
        sync(w, c, now);
    }
    sync_engagements(w, now);
    let cw = w.combat_mut().crimewatch_mut();
    cw.criminal_acts
        .retain(|_, at| now < *at + CRIMINAL_TIMEOUT);
    cw.duels.retain(|_, until| now < *until);
    cw.activations.retain(|_, a| now < a.until);
    release_holds(w, now);
}

/// Bring one character's client, and their system's view of them, up to
/// `now`.
fn sync(w: &mut Registry, character_id: i64, now: BlueTime) {
    let Some(mut record) = w.combat().crimewatch().records.get(&character_id).cloned() else {
        return;
    };
    let criminal_phase = phase_at(
        Timer::Criminal,
        &record.clocks[Timer::Criminal.index()],
        now,
    );
    if criminal_phase == Phase::Idle {
        record.level = None;
    }
    let level = record.level.unwrap_or(CrimeKind::Suspect);
    for timer in Timer::ALL {
        let clock = &mut record.clocks[timer.index()];
        let wire = phase_at(timer, clock, now).wire(timer, level);
        let idle = Phase::Idle.wire(timer, level);
        if wire != clock.told.unwrap_or(idle) {
            clock.told = Some(wire);
            w.schedule(
                Duration::ZERO,
                TimerUpdate {
                    to: character_id,
                    timer,
                    state: wire.0,
                    expiry: wire.1,
                },
            );
        }
    }
    let here = system_of(w, character_id);
    let wanted = here.zip(record.level);
    if wanted != record.announced {
        if let Some((old, _)) = record.announced
            && wanted.is_none_or(|(new, _)| new != old)
        {
            let to = pilots_in(w, old);
            if !to.is_empty() {
                w.schedule(
                    Duration::ZERO,
                    FlagUpdate {
                        to,
                        idles: vec![character_id],
                        suspects: vec![],
                        criminals: vec![],
                    },
                );
            }
        }
        if let Some((new, kind)) = wanted {
            let to = pilots_in(w, new);
            if !to.is_empty() {
                let (suspects, criminals) = match kind {
                    CrimeKind::Suspect => (vec![character_id], vec![]),
                    CrimeKind::Criminal => (vec![], vec![character_id]),
                };
                w.schedule(
                    Duration::ZERO,
                    FlagUpdate {
                        to,
                        idles: vec![],
                        suspects,
                        criminals,
                    },
                );
            }
        }
        record.announced = wanted;
    }
    let clean = record.announced.is_none()
        && record.clocks.iter().enumerate().all(|(i, c)| {
            c.told
                .is_none_or(|t| t == Phase::Idle.wire(Timer::ALL[i], level))
        });
    let cw = w.combat_mut().crimewatch_mut();
    if clean {
        cw.records.remove(&character_id);
    } else {
        cw.records.insert(character_id, record);
    }
}

/// Bring every engagement's two clients up to `now`: created, timing out,
/// ongoing again, ended (`crimewatchSvc.OnCrimewatchEngagement*`) **[V]**.
fn sync_engagements(w: &mut Registry, now: BlueTime) {
    let all: Vec<((i64, i64), Engagement)> = w
        .combat()
        .crimewatch()
        .engagements
        .iter()
        .map(|(k, e)| (*k, *e))
        .collect();
    for (key, e) in all {
        let (a, b) = key;
        match (e.told, engagement_wire(e.last, now)) {
            (_, None) => {
                w.combat_mut().crimewatch_mut().engagements.remove(&key);
                if e.told.is_some() {
                    tell_engagement_ended(w, key);
                }
                continue;
            }
            (Some(was), Some(t)) if was == t => continue,
            (None, Some(t)) => {
                for (to, other) in [(a, b), (b, a)] {
                    w.schedule(
                        Duration::ZERO,
                        Notice::new(
                            to,
                            "OnCrimewatchEngagementCreated",
                            vec![PyValue::Int(other), PyValue::Int(t)],
                        ),
                    );
                }
            }
            (Some(_), Some(ENGAGEMENT_ONGOING)) => {
                for (to, other) in [(a, b), (b, a)] {
                    w.schedule(
                        Duration::ZERO,
                        Notice::new(
                            to,
                            "OnCrimewatchEngagementStopTimeout",
                            vec![PyValue::Int(other)],
                        ),
                    );
                }
            }
            (Some(_), Some(t)) => {
                for (to, other) in [(a, b), (b, a)] {
                    w.schedule(
                        Duration::ZERO,
                        Notice::new(
                            to,
                            "OnCrimewatchEngagementStartTimeout",
                            vec![PyValue::Int(other), PyValue::Int(t)],
                        ),
                    );
                }
            }
        }
        if let Some(rec) = w.combat_mut().crimewatch_mut().engagements.get_mut(&key) {
            rec.told = engagement_wire(e.last, now);
        }
    }
}

/// Characters in space in `solar_system_id` -- whose overview a flag colours.
/// A docked pilot learns the flags from [`client_states`] on undock.
fn pilots_in(w: &Registry, solar_system_id: i64) -> Vec<i64> {
    w.roster()
        .in_system(solar_system_id)
        .iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

// ---------------------------------------------------------------------------
// What the timers refuse
// ---------------------------------------------------------------------------

/// Is `character_id`'s weapons timer running at `now`?
pub fn has_weapons_timer(w: &Registry, character_id: i64, now: BlueTime) -> bool {
    w.combat()
        .crimewatch()
        .expiry(character_id, Timer::Weapons, now)
        .is_some()
}

/// `beyonce.CmdDock`'s gate: `CantDockAfterAggression` under a weapons
/// timer. Raised before anything moves, so the session change that *is* the
/// dock (`dispatch::session_change_for`) is never sent -- a raise returns
/// from `handle_bound_call` ahead of `with_session_change`.
pub fn refuse_dock(w: &Registry, actor: Option<i64>, now: BlueTime) -> Result<(), ServiceError> {
    refuse(w, actor, now, "CantDockAfterAggression")
}

/// `beyonce.CmdStargateJump`'s gate: `CantJumpAfterAggression`, the same
/// rule (260097: "The stargate denies you permission to jump for the moment
/// due to your recent acts of aggression") **[V]**.
pub fn refuse_jump(w: &Registry, actor: Option<i64>, now: BlueTime) -> Result<(), ServiceError> {
    refuse(w, actor, now, "CantJumpAfterAggression")
}

fn refuse(w: &Registry, actor: Option<i64>, now: BlueTime, key: &str) -> Result<(), ServiceError> {
    match actor {
        Some(c) if has_weapons_timer(w, c, now) => {
            Err(ServiceError::Raise(eve_proto::user_error(key, vec![])))
        }
        _ => Ok(()),
    }
}

/// How long `character_id`'s ship must stay in space if they log off now:
/// the later of the two log-off timers' expiries ("Ship will remain in
/// space on log-off until timer expires", 283269/283270) **[V]**.
///
/// What [`logged_off`] holds the ship for, and what [`release_holds`]
/// re-reads before it lets one go -- so a logged-off ship that is shot again
/// stays for the new timer, as the tooltip's "until timer expires" says.
pub fn logoff_hold(w: &Registry, character_id: i64, now: BlueTime) -> Option<BlueTime> {
    let cw = w.combat().crimewatch();
    [Timer::Pvp, Timer::Npc]
        .into_iter()
        .filter_map(|t| cw.expiry(character_id, t, now))
        .max()
}

// ---------------------------------------------------------------------------
// Log-off: the ship stays in space
// ---------------------------------------------------------------------------

/// The least time a ship stays in space after its pilot's last session
/// closes, with no timer running at all.
///
/// `[A]`: TQ's "about a minute" for a ship that logs off in space, which is
/// what makes pulling the plug mid-fight no escape even before the first
/// volley lands. The timers' own holds are [`logoff_hold`]'s and **[V]**.
/// Falsified by a peaceful logged-off ship that a second client still sees
/// after several minutes, or one gone the moment the pilot quits.
pub const LOGOFF_MINIMUM: BlueTime = 60 * SEC;

/// How far the emergency warp goes: 1,000,000 km, in metres. `[A]` --
/// TQ's figure as players report it; nothing in the client names it.
/// Far enough that nobody on grid can follow without probes, and a short
/// warp (well under a tenth of an AU), so the ship is at its spot within
/// seconds of the drop.
pub const EMERGENCY_WARP_DISTANCE: f64 = 1.0e9;

/// A pilot's last session just closed (`Registry::session_closed`): if their
/// ship is in space, **it stays there**, still a ball in every park, still
/// targetable and still killable through `crate::combat`, until
/// [`release_holds`] lets it go -- the later of [`LOGOFF_MINIMUM`] and the
/// log-off timers ("Ship will remain in space on log-off until timer
/// expires", 283269/283270) **[V]**.
///
/// What changes at once:
///
/// - **the pilot's own locks go.** Nobody is at the controls; the victims'
///   brackets clear with an `'otherlost'`, on the clock. Everyone *else's*
///   locks on the ship stay -- that is the point;
/// - **an emergency warp, unless the weapons timer runs** (the return value;
///   `service/space.rs`'s `emergency_warp` flies it). `[A]`: a pilot who
///   pulls the plug mid-fight -- the weapons timer is "aggression against
///   another capsuleer", 283268 -- leaves the ship where it is, in the fight
///   they started; one who was only shot at warps off and waits out the
///   pvp/npc timer at the far end. Falsified by a ship that warps away from
///   the fight its own pilot opened, or one that never warps when it was
///   merely the victim.
///
/// The running modules were already stopped by the caller. Nothing here is
/// pushed to the pilot: there is nobody to push to.
pub fn logged_off(w: &mut Registry, character_id: i64, now: BlueTime) -> bool {
    let Some(here) = w.roster().get(character_id).copied() else {
        return false;
    };
    let until = logoff_hold(w, character_id, now)
        .unwrap_or(0)
        .max(now.saturating_add(LOGOFF_MINIMUM));
    w.combat_mut()
        .crimewatch_mut()
        .held
        .insert(character_id, until);
    // Restart safety: the spot is also written at the real release.
    crate::logoff_spot::record(w, character_id);
    // **No protection through a log-off**: a restoring (or undock) grant
    // that outlived the disconnect would make the hold expose nothing, and a
    // pilot could drop and return every minute to renew it.
    crate::invuln::break_for(w, character_id);
    drop_own_locks(w, here.ship_id);
    crate::outln!(
        "[crimewatch] character {character_id} logged off in space: ship {} held for {} s",
        here.ship_id,
        (until - now) / SEC
    );
    !has_weapons_timer(w, character_id, now)
}

/// The pilot is back (`Registry::session_selected_character`): no hold any
/// more, and the ship -- if it is still in space -- is theirs to fly. The
/// login's session change puts them in it (`Registry::character_change`).
/// Returns whether a hold was cancelled.
pub fn reclaim(w: &mut Registry, character_id: i64) -> bool {
    let cw = w.combat_mut().crimewatch_mut();
    let held = cw.held.remove(&character_id).is_some();
    if held {
        cw.rejoining.insert(character_id);
        // The ship never left: a rejoin, not a restore. (An abyssal run's
        // origin row is not a restore either; it is the way home.)
        if !crate::abyss::in_run(w, character_id) {
            crate::logoff_spot::clear(w, character_id);
        }
    }
    held
}

/// Is this bind the rejoin of a held ship? Consumed by the first bind after
/// [`reclaim`], whatever the answer, so it can never outlive the login it
/// belongs to: `destiny::on_ballpark_bind` then leaves the ship where it is
/// instead of re-seeding it at the station, and tells nobody else (they
/// never lost its ball).
pub fn take_rejoin(w: &mut Registry, character_id: i64) -> bool {
    w.combat_mut()
        .crimewatch_mut()
        .rejoining
        .remove(&character_id)
        && w.roster().is_in_space(character_id)
}

/// Let go of every held ship whose time is up. A ship shot since it was held
/// has a new log-off timer, and stays for it. One that has already left
/// space -- its capsule killed, the pilot revived at home -- is simply
/// forgotten.
pub fn release_holds(w: &mut Registry, now: BlueTime) {
    let due: Vec<i64> = w
        .combat()
        .crimewatch()
        .held
        .iter()
        .filter(|(_, until)| **until <= now)
        .map(|(c, _)| *c)
        .collect();
    for c in due {
        if let Some(later) = logoff_hold(w, c, now) {
            w.combat_mut().crimewatch_mut().held.insert(c, later);
            continue;
        }
        w.combat_mut().crimewatch_mut().held.remove(&c);
        if w.sessions().is_online(c) {
            // Back without a `reclaim` (a session opened by a path that
            // does not select a character the usual way): theirs, not ours.
            // A pilot in an abyssal run keeps the origin row it holds.
            if !crate::abyss::in_run(w, c) {
                crate::logoff_spot::clear(w, c);
            }
            continue;
        }
        // **A pocket is no exit** (ABYSS-LOGOFF): a pilot who dropped inside an
        // abyssal run keeps the ship in the roster until the run ends, and the
        // run's clock kills ship and pod as for anyone still there. The
        // hold stays, checked again each tick; the run goes (or the ship is
        // gone) and the usual release follows. A restart has no run.
        if crate::abyss::in_run(w, c) && w.roster().get(c).is_some() {
            w.combat_mut().crimewatch_mut().held.insert(c, now);
            continue;
        }
        crate::outln!("[crimewatch] character {c}: log-off hold over, ship leaves space");
        // Where it is now is where the pilot comes back (no roster entry,
        // a killed ship, clears the row). A warp still running settles here.
        crate::logoff_spot::record(w, c);
        if let Some(ship) = w.character(c).map(|ch| ch.ship_id) {
            w.cancel_scheduled(|p| crate::service::space::is_warp_settle(p, ship));
        }
        leave_space(w, c);
    }
}

/// Take a pilot-less ship out of space for good: out of the lock graph both
/// ways, out of the roster, and a `RemoveBalls` to everyone still watching.
///
/// What `Registry::session_closed` did at the moment of the disconnect
/// before the hold existed, moved here unchanged and with its reasons: the
/// locks first, while the roster still knows the ship; and **all of it on
/// the clock**, because the world ticker is the caller and there is no turn
/// to ride on.
pub fn leave_space(w: &mut Registry, character_id: i64) {
    crate::targeting::left_space(
        w,
        character_id,
        crate::targeting::Reason::Unlocked,
        crate::targeting::Delivery::OnTheClock,
    );
    if let Some(gone) = w.roster_mut().leave(character_id) {
        let watching = w
            .roster()
            .others_in_system(gone.solar_system_id, character_id);
        let stamp = w.next_destiny_stamp();
        if let Some(removed) =
            crate::destiny::BallsRemoved::new(watching, stamp, vec![gone.ship_id])
        {
            w.schedule(Duration::ZERO, removed);
        }
    }
}

/// Drop every lock `ship` holds, and tell each victim. The locks *on* it
/// stay.
fn drop_own_locks(w: &mut Registry, ship: i64) {
    for target_ship in w.locks_mut().clear(ship) {
        let victim = w
            .roster()
            .by_ship(target_ship)
            .map(|p| p.character_id)
            .filter(|id| w.character(*id).is_some());
        if let Some(victim) = victim {
            w.schedule(
                Duration::ZERO,
                crate::targeting::TargetChange {
                    to: vec![victim],
                    what: crate::targeting::What::OtherLost,
                    ship: Some(ship),
                    reason: Some(crate::targeting::Reason::Unlocked),
                },
            );
        }
    }
}

/// The emergency warp's direction: a unit vector drawn from the ship and
/// the instant, so it differs between ships and between drops and nobody can
/// know it in advance. `[A]` (TQ's is random; this is a std-only stand-in,
/// splitmix64 into a uniform point on the sphere).
pub fn emergency_heading(ship_id: i64, now: BlueTime) -> [f64; 3] {
    let mut x = (ship_id as u64) ^ (now as u64).rotate_left(29);
    let mut next = || {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    };
    let z = 2.0 * next() - 1.0;
    let theta = 2.0 * std::f64::consts::PI * next();
    let r = (1.0 - z * z).max(0.0).sqrt();
    [r * theta.cos(), r * theta.sin(), z]
}

// ---------------------------------------------------------------------------
// Security
// ---------------------------------------------------------------------------

/// The systems this server tells the client are exactly `0.0` security,
/// through `securityMgr.get_modified_systems` (`service/base.rs`): every
/// character's station system **that the universe table does not know**.
/// One list, so the client's view and [`flags_apply`] cannot drift apart.
///
/// **CW-1 narrowed it** (it was every character's station system). The
/// declaration was §19's fix: with no safety served the client refused
/// every attack in our high-sec test system before calling us. CW-1 serves
/// the safety setting, so a pilot can fight anywhere by lowering it -- and
/// the declaration had become the opposite problem: every system a pilot
/// (or a bot) is stationed in was lawless, so no crime could ever happen
/// where anyone lives. Now a system with a row in the bundle's universe
/// table keeps its real security on both sides (the client's
/// `systems.static` and our table are the same numbers), and the
/// declaration remains only for a server with no bundle, where the table
/// cannot say. To restore the old sandbox (free fire at every home), drop
/// the `filter` below. `[A]` owner-visible: falsified by the owner wanting
/// home systems lawless again.
pub fn declared_null_systems(w: &Registry) -> Vec<i64> {
    let universe = w.content().get::<crate::celestial::Universe>();
    let mut systems: Vec<i64> = w
        .config()
        .characters
        .iter()
        .filter_map(|c| c.solar_system_id)
        .filter(|s| universe.system(*s).is_none())
        .collect();
    systems.sort_unstable();
    systems.dedup();
    systems
}

/// Do suspect flags exist in `solar_system_id`?
///
/// The client's own rule: its crimewatch tree runs only where
/// `is_low_sec_or_higher(get_modified_security_class(system))`
/// (`crimewatchSvc.py:468-472`) **[V]**, so:
///
/// - the system's class from the universe table
///   ([`crate::celestial::System::security_class`], CP3b): **no** where the
///   client's rounded security is `<= 0.0`, **yes** above it **[V]**
///   (`docs/re/damage-death.md` §7.2.1);
/// - for a system the table lacks: **no** where we declare it zero-sec
///   ([`declared_null_systems`]), because the client takes our `0.0` over
///   its own number (`securitySvc.py:73-76`) **[V]**;
/// - **yes** for any other system the table lacks -- `[A]`, the pre-CP3b
///   rule, kept for a server with no bundle and a world outside the
///   fixture. Falsified by a flag in a null-sec system when no bundle is
///   loaded; the fix is to load one (`tools/up.sh`).
///
/// The table is asked first: a declared system is one the table lacks, so
/// the order changes no answer, and a volley in a known system costs no
/// walk over the characters.
pub fn flags_apply(w: &Registry, solar_system_id: i64) -> bool {
    match w
        .content()
        .get::<crate::celestial::Universe>()
        .security_class(solar_system_id)
    {
        Some(class) => class.is_low_sec_or_higher(),
        None => !declared_null_systems(w).contains(&solar_system_id),
    }
}

/// `idCheckers.IsNPCCorporation`: `minNPCCorporation <= id <=
/// maxNPCCorporation` (`idCheckers.py:323`; 1000000 and 1999999,
/// `inventorycommon/const.py:10-11`) **[V]** -- `CanAttackFreely`'s corp
/// branch runs only for a player corporation (`not IsNPC(session.corpid)`).
fn is_npc_corporation(id: i64) -> bool {
    (1_000_000..=1_999_999).contains(&id)
}

// ---------------------------------------------------------------------------
// The safety setting
// ---------------------------------------------------------------------------

/// The delta kind of the safety setting: `char -> level`, only for a pilot
/// whose setting is not [`DEFAULT_SAFETY`].
pub const SAFETY_KIND: &str = "cwsafety";

/// A pilot who never touched the setting is **green**: the client's own
/// `Initialize` sets `shipSafetyLevelFull` (`crimewatchSvc.py`) **[V]**,
/// and `GetClientStates`' answer overrides it on every undock and jump. TQ
/// starts every pilot green; that the setting then **persists** across
/// sessions is `[A]` (falsified by a TQ client that comes back green after a
/// relog).
pub const DEFAULT_SAFETY: Safety = Safety::Full;

/// `character_id`'s safety setting.
pub fn safety(w: &Registry, character_id: i64) -> Safety {
    w.deltas()
        .of(SAFETY_KIND)
        .get(&vec![character_id.to_string()])
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(Safety::from_wire)
        .unwrap_or(DEFAULT_SAFETY)
}

/// Store `character_id`'s safety setting (`SetSafetyLevel`).
pub fn set_safety(w: &mut Registry, character_id: i64, level: Safety) {
    let key = vec![character_id.to_string()];
    if level == DEFAULT_SAFETY {
        if w.deltas().of(SAFETY_KIND).contains_key(&key) {
            w.deltas_mut().del(SAFETY_KIND, &key);
        }
    } else {
        w.deltas_mut()
            .put(SAFETY_KIND, key, vec![level.wire().to_string()]);
    }
}

/// A refusal in the client's own words: a `UserError` key and its dict.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub key: &'static str,
    pub args: Vec<(&'static str, PyValue)>,
}

impl Refusal {
    pub fn into_error(self) -> ServiceError {
        ServiceError::Raise(eve_proto::user_error(
            self.key,
            crate::pyutil::pypairs(self.args),
        ))
    }
}

/// `(UE_TYPEID, typeID)`: how a dialog names a type **[V]**.
fn type_arg(type_id: i64) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(crate::modules::UE_TYPEID),
        PyValue::Int(type_id),
    ])
}

/// `(UE_OWNERID, ownerID)`: how a dialog names a pilot `[A]` (the target's
/// name in the refusal; the client's own text takes any name there).
fn owner_arg(owner_id: i64) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(crate::damage::UE_OWNERID),
        PyValue::Int(owner_id),
    ])
}

/// **The server's safety gate on an attack**: `Some` when `actor`'s safety
/// forbids what activating `module_type` (or engaging drones, `None`) on
/// the ball `target` would make them -- `ModuleActivationDeniedSafetyPreventsSuspect`
/// or `...Criminal`, `(moduleName, targetName)` (283544, 283545: "This
/// action would make you a Suspect, and your safeties are engaged") **[V]**
/// keys and arguments; both have `raise_sites` 0 in the client, which
/// refuses first by its own `consider.DoAttackConfirmations` -- so this is
/// the backstop for a bot, or for a client whose view of a flag lagged.
///
/// For `modules::activate`'s offensive effects and `drones::engage`
/// (coordinator seam lines).
pub fn attack_refusal(
    w: &Registry,
    actor: i64,
    module_type: Option<i64>,
    target: i64,
    now: BlueTime,
) -> Option<Refusal> {
    let key = roster_key(w, target);
    // The law is no capsuleer, so the verdict below never sees it; a volley
    // at it is a criminal act in high sec ([`crate::concord::illegal_on_law`]).
    if crate::concord::illegal_on_law(w, actor, target) {
        let owner = w.pilot(key).map_or(0, |p| p.corporation_id);
        return refusal_for(
            w,
            actor,
            Some(CrimeKind::Criminal),
            module_type,
            owner,
            false,
        );
    }
    let owner = owner_capsuleer(w, key)?;
    let space = space_of(w, system_of(w, actor)?);
    let facts = target_facts(w, actor, Some(owner), is_capsule(w, key), now);
    let verdict = crime(space, &Act::Attack(facts));
    refusal_for(w, actor, verdict, module_type, owner, false)
}

/// The same gate on an assistance effect: a criminal or suspect target, or
/// one in a limited engagement (`ModuleActivationDeniedSafetyPreventsLimitedEngagement`,
/// 517301) **[V]** keys. For the remote-assistance modules when they exist.
pub fn assist_refusal(
    w: &Registry,
    actor: i64,
    module_type: Option<i64>,
    target: i64,
    now: BlueTime,
) -> Option<Refusal> {
    let owner = owner_capsuleer(w, roster_key(w, target))?;
    let space = space_of(w, system_of(w, actor)?);
    let facts = target_facts(w, actor, Some(owner), false, now);
    let verdict = crime(space, &Act::Assist(facts));
    let engagement_only = facts.flag.is_none() && facts.in_engagement;
    refusal_for(w, actor, verdict, module_type, owner, engagement_only)
}

fn refusal_for(
    w: &Registry,
    actor: i64,
    verdict: Option<CrimeKind>,
    module_type: Option<i64>,
    owner: i64,
    engagement: bool,
) -> Option<Refusal> {
    if safety(w, actor).allows(verdict) {
        return None;
    }
    let key = match verdict? {
        CrimeKind::Suspect if engagement => "ModuleActivationDeniedSafetyPreventsLimitedEngagement",
        CrimeKind::Suspect => "ModuleActivationDeniedSafetyPreventsSuspect",
        CrimeKind::Criminal => "ModuleActivationDeniedSafetyPreventsCriminal",
    };
    let module = module_type.map_or(PyValue::None, type_arg);
    Some(Refusal {
        key,
        args: vec![("moduleName", module), ("targetName", owner_arg(owner))],
    })
}

/// **The server's safety gate on a take**: `Some` when taking from
/// `container_id` (a wreck or a hacked can) would make `actor` a suspect and
/// their safety forbids it -- `LootTheftDeniedSafetyPreventsSuspect
/// {containerName: (UE_TYPEID, typeID)}`, the client's own key and argument
/// (`invControllers.py:485`, `:1323`) **[V]**, which it raises itself when
/// its safety is green (`CheckCanTakeItems`) and which we raise for anyone
/// who reaches us anyway. For `service/inventory.rs`'s loot path
/// (coordinator seam line).
pub fn theft_refusal(
    w: &Registry,
    actor: i64,
    container_id: i64,
    now: BlueTime,
) -> Option<Refusal> {
    let wr = crate::hacking_loot::loot_source(w, container_id)?;
    if crate::combat::has_loot_right(w, actor, &wr) {
        return None;
    }
    let space = space_of(w, wr.solar_system_id);
    let owner = Some(wr.owner_id).filter(|o| w.character(*o).is_some());
    let facts = match owner {
        Some(_) => target_facts(w, actor, owner, false, now),
        None => Target {
            capsuleer: true,
            ..Target::default()
        },
    };
    let verdict = crime(space, &Act::Theft(facts));
    if safety(w, actor).allows(verdict) {
        return None;
    }
    Some(Refusal {
        key: "LootTheftDeniedSafetyPreventsSuspect",
        args: vec![("containerName", type_arg(wr.type_id))],
    })
}

// ---------------------------------------------------------------------------
// GetClientStates
// ---------------------------------------------------------------------------

/// `GetClientStates()`, the whole state, for `actor` at `now`:
///
/// ```text
/// ((weapons, pvp, npc, criminal, disapproval), engagements, (criminals, suspects), safetyLevel)
/// ```
///
/// unpacked at `ProcessSessionChange:126-140` **[V]**: each timer a
/// `(state, expiryTime)` pair, in that order; `engagements` a **dict**
/// `{otherCharID: timeout}` (`GetMyEngagements` is
/// `self.myEngagements.copy()`, `:342` -- a list there was an
/// `AttributeError` for several runs); the flagged characters iterated
/// (`UpdateSuspectsAndCriminals:258-261`), every one of them in the caller's
/// system; and the caller's safety setting ([`safety`]).
///
/// **An idle timer is its idle state, not `0`.** This answered `(0, None)`
/// five times for a long while; `HaveWeaponsTimer` is `state !=
/// weaponsTimerStateIdle` (`:424`) **[V]**, so every pilot read as armed to
/// the two menu checks that ask.
pub fn client_states(w: &Registry, actor: Option<i64>, now: BlueTime) -> PyValue {
    let cw = w.combat().crimewatch();
    let level = actor
        .and_then(|c| cw.flag(c, now))
        .unwrap_or(CrimeKind::Suspect);
    let pair = |timer: Timer| {
        let phase = actor.map_or(Phase::Idle, |c| cw.phase(c, timer, now));
        let (state, expiry) = phase.wire(timer, level);
        PyValue::Tuple(vec![
            PyValue::Int(state),
            expiry.map_or(PyValue::None, PyValue::Int),
        ])
    };
    let here = actor.and_then(|c| system_of(w, c));
    let flagged = |want: CrimeKind| -> Vec<PyValue> {
        cw.records
            .keys()
            .copied()
            .filter(|c| cw.flag(*c, now) == Some(want))
            .filter(|c| here.is_some() && system_of(w, *c) == here)
            .map(PyValue::Int)
            .collect()
    };
    let engagements: Vec<(PyValue, PyValue)> = actor
        .map(|c| {
            cw.engagements_of(c, now)
                .map(|(other, t)| (PyValue::Int(t), PyValue::Int(other)))
                .collect()
        })
        .unwrap_or_default();
    let safety_level = actor.map_or(DEFAULT_SAFETY, |c| safety(w, c));
    PyValue::Tuple(vec![
        PyValue::Tuple(vec![
            pair(Timer::Weapons),
            pair(Timer::Pvp),
            pair(Timer::Npc),
            pair(Timer::Criminal),
            PyValue::Tuple(vec![PyValue::Int(state::DISAPPROVAL_IDLE), PyValue::None]),
        ]),
        PyValue::Dict(engagements),
        PyValue::Tuple(vec![
            PyValue::List(flagged(CrimeKind::Criminal)),
            PyValue::List(flagged(CrimeKind::Suspect)),
        ]),
        PyValue::Int(safety_level.wire()),
    ])
}

// ---------------------------------------------------------------------------
// Deaths
// ---------------------------------------------------------------------------

/// **The `kill_hooks::PLAYER` row** (coordinator): a capsuleer's ship or
/// capsule died. Its pilot's limited engagements end ([`end_engagements`],
/// `[A]`); a criminal kill earns the victim a kill right, and a kill under
/// an activated kill right uses that right up ([`killright::on_kill`]). A
/// drone's death is none of this.
pub fn on_kill(w: &mut Registry, ev: &crate::kill_hooks::KillEvent) {
    if w.character(ev.victim_character).is_none() {
        return;
    }
    end_engagements(w, ev.victim_character);
    killright::on_kill(w, ev);
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// One of one's own timers moved: `On*TimerUpdate(state, expiryTime)`,
/// two positionals (`crimewatchSvc.py:280-299`) **[V]**, `expiryTime` a blue
/// time or `None`. The HUD lists all four in `__notifyevents__`
/// (`crimewatchTimers.py:587-590`) and redraws from the same pair.
#[derive(Debug, Clone, PartialEq)]
pub struct TimerUpdate {
    pub to: i64,
    pub timer: Timer,
    pub state: i64,
    pub expiry: Option<BlueTime>,
}

impl Push for TimerUpdate {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            self.timer.event(),
            vec![
                PyValue::Int(self.state),
                self.expiry.map_or(PyValue::None, PyValue::Int),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// Somebody else's flag: `OnSystemCriminalFlagUpdates(newIdles,
/// newSuspects, newCriminals)` (`crimewatchSvc.py:306`) **[V]**, three
/// lists of character ids -- iterated, and the first passed through
/// `set()` (`UpdateSuspectsAndCriminals:251-265`), which then scatters
/// `OnSuspectsAndCriminalsUpdate` for the overview.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagUpdate {
    pub to: Vec<i64>,
    pub idles: Vec<i64>,
    pub suspects: Vec<i64>,
    pub criminals: Vec<i64>,
}

impl Push for FlagUpdate {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let ids = |v: &[i64]| PyValue::List(v.iter().copied().map(PyValue::Int).collect());
        (
            "OnSystemCriminalFlagUpdates",
            vec![ids(&self.idles), ids(&self.suspects), ids(&self.criminals)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Any other crimewatch event, to one character: the engagement four, the
/// kill-right events, a duel's challenge and its answers. Each handler's
/// arguments are positional, named at the call site **[V]** per event.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub to: i64,
    pub name: &'static str,
    pub args: Vec<PyValue>,
}

impl Notice {
    pub fn new(to: i64, name: &'static str, args: Vec<PyValue>) -> Notice {
        Notice { to, name, args }
    }
}

impl Push for Notice {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.name, self.args.clone())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const ALICE_SHIP: i64 = 100_000_001;
    const BOB_SHIP: i64 = 100_000_002;
    /// `Character::default()`'s system, and so declared zero-sec.
    const HOME: i64 = 30_000_001;
    /// A system nobody starts in.
    const AWAY: i64 = 30_000_003;
    const T0: BlueTime = 133_000_000_000_000_000;

    /// Alice and Bob, both in space in `system`.
    fn both_in_space(system: i64) -> Registry {
        place_both(system, None)
    }

    /// The same, on the fixture content with `universe` in place of the
    /// fixture universe when one is given (CP3b).
    fn place_both(system: i64, universe: Option<crate::celestial::Universe>) -> Registry {
        let pilot = |id: i64, ship: i64| crate::model::Character {
            character_id: id,
            name: format!("pilot {id}"),
            ship_id: ship,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![pilot(ALICE, ALICE_SHIP), pilot(BOB, BOB_SHIP)],
            ..crate::model::SandboxConfig::default()
        });
        if let Some(u) = universe {
            let mut content = crate::content::Content::fixture();
            content.insert("universe", u);
            reg = reg.with_content(std::sync::Arc::new(content));
        }
        for (id, ship, x) in [(ALICE, ALICE_SHIP, 0.0), (BOB, BOB_SHIP, 5_000.0)] {
            reg.character_mut(id).expect("a pilot").solar_system_id = Some(system);
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    id,
                    ship,
                    system,
                    crate::space_state::Motion::arrived([x, 0.0, 0.0], [0.0; 3]),
                ));
        }
        let _ = reg.take_pending();
        reg
    }

    /// Everything on the clock queue, as `(audience, name, args)`.
    fn due(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
        reg.take_due(Instant::now() + Duration::from_secs(1))
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (p.audience().unwrap_or_default(), name, args)
            })
            .collect()
    }

    /// The `(state, expiry)` pairs of one event sent to one pilot, in order.
    fn sent(
        out: &[(Vec<i64>, &'static str, Vec<PyValue>)],
        to: i64,
        event: &str,
    ) -> Vec<(i64, Option<i64>)> {
        out.iter()
            .filter(|(a, n, _)| *n == event && a == &vec![to])
            .map(|(_, _, args)| {
                let PyValue::Int(s) = args[0] else {
                    panic!("a state")
                };
                let e = match args[1] {
                    PyValue::Int(e) => Some(e),
                    PyValue::None => None,
                    ref other => panic!("an expiry, got {other:?}"),
                };
                (s, e)
            })
            .collect()
    }

    fn timers(states: &PyValue) -> Vec<(i64, Option<i64>)> {
        let PyValue::Tuple(parts) = states else {
            panic!("a tuple")
        };
        let PyValue::Tuple(t) = &parts[0] else {
            panic!("five timers")
        };
        t.iter()
            .map(|p| match p {
                PyValue::Tuple(p) => (
                    match p[0] {
                        PyValue::Int(s) => s,
                        _ => panic!("a state"),
                    },
                    match p[1] {
                        PyValue::Int(e) => Some(e),
                        _ => None,
                    },
                ),
                _ => panic!("a pair"),
            })
            .collect()
    }

    fn raised_key(err: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = err else {
            panic!("not a raised exception: {err:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("a header tuple")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError's args")
        };
        String::from_utf8_lossy(crate::pyutil::as_bytes(&args[0]).expect("a key")).into_owned()
    }

    /// **A peaceful pilot's five timers are the five idle states**, because
    /// `HaveWeaponsTimer` is `state != weaponsTimerStateIdle`
    /// (`crimewatchSvc.py:424`) and the idle numbers are
    /// `crimewatch/const.py:7-26` [V] -- the `0` this used to send read as
    /// armed. And the shape `ProcessSessionChange` unpacks: four parts, a
    /// dict of engagements, two lists of flagged pilots.
    #[test]
    fn a_peaceful_pilot_reads_idle_on_every_timer() {
        let reg = both_in_space(HOME);
        let got = client_states(&reg, Some(ALICE), T0);
        assert_eq!(
            timers(&got),
            vec![
                (100, None),
                (200, None),
                (400, None),
                (300, None),
                (500, None)
            ]
        );
        let PyValue::Tuple(parts) = &got else {
            unreachable!()
        };
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[1], PyValue::Dict(vec![]));
        assert_eq!(
            parts[2],
            PyValue::Tuple(vec![PyValue::List(vec![]), PyValue::List(vec![])])
        );
    }

    /// **Shooting a capsuleer** arms the shooter's weapons timer -- the
    /// tooltip's "aggression against another capsuleer" -- and both pilots'
    /// capsuleer log-off timer -- "combat with another capsuleer" (283268,
    /// 283269) [V]. The victim gets no weapons timer: being shot is not
    /// aggression. Each runs out after its own `crimewatch/const.py`
    /// timeout [V].
    #[test]
    fn shooting_a_capsuleer_arms_the_shooter_and_logs_both() {
        let mut reg = both_in_space(HOME);
        aggression(&mut reg, ALICE, BOB, T0, T0);
        let cw = reg.combat().crimewatch();
        assert_eq!(cw.expiry(ALICE, Timer::Weapons, T0), Some(T0 + 60 * SEC));
        assert_eq!(cw.expiry(ALICE, Timer::Pvp, T0), Some(T0 + 15 * MIN));
        assert_eq!(cw.expiry(BOB, Timer::Pvp, T0), Some(T0 + 15 * MIN));
        assert_eq!(cw.expiry(BOB, Timer::Weapons, T0), None);
        assert_eq!(cw.expiry(ALICE, Timer::Npc, T0), None);
        assert!(has_weapons_timer(&reg, ALICE, T0 + 59 * SEC));
        assert!(!has_weapons_timer(&reg, ALICE, T0 + 60 * SEC));
    }

    /// **Shooting a rat** is "combat with a non-capsuleer" (283270) [V]: the
    /// npc log-off timer, five minutes, and no weapons timer -- the weapons
    /// tooltip names a capsuleer. Being shot by one is the same combat.
    #[test]
    fn combat_with_an_npc_is_the_npc_timer_only() {
        let mut reg = both_in_space(HOME);
        let rat = crate::npc::spawn(&mut reg, &crate::npc::TYPES[0], HOME, [9_000.0, 0.0, 0.0])
            .expect("an NPC");
        aggression(&mut reg, ALICE, rat, T0, T0);
        aggression(&mut reg, rat, BOB, T0, T0);
        let cw = reg.combat().crimewatch();
        for pilot in [ALICE, BOB] {
            assert_eq!(cw.expiry(pilot, Timer::Npc, T0), Some(T0 + 5 * MIN));
            assert_eq!(cw.expiry(pilot, Timer::Weapons, T0), None);
            assert_eq!(cw.expiry(pilot, Timer::Pvp, T0), None);
        }
    }

    /// **The station and the gate say no under a weapons timer, and yes the
    /// moment it runs out**, with the client's own keys -- which it never
    /// raises itself (`user-errors.tsv` `raise_sites` 0) [V]. The victim,
    /// who has no weapons timer, may dock.
    #[test]
    fn a_weapons_timer_refuses_docking_and_jumping_until_it_runs_out() {
        let mut reg = both_in_space(HOME);
        aggression(&mut reg, ALICE, BOB, T0, T0);
        let during = T0 + 30 * SEC;
        let dock = refuse_dock(&reg, Some(ALICE), during).expect_err("refused");
        assert_eq!(raised_key(&dock), "CantDockAfterAggression");
        let jump = refuse_jump(&reg, Some(ALICE), during).expect_err("refused");
        assert_eq!(raised_key(&jump), "CantJumpAfterAggression");
        assert!(refuse_dock(&reg, Some(BOB), during).is_ok());
        let after = T0 + WEAPONS_TIMEOUT;
        assert!(refuse_dock(&reg, Some(ALICE), after).is_ok());
        assert!(refuse_jump(&reg, Some(ALICE), after).is_ok());
    }

    /// **What the HUD is told**: whatever phases a timer passes through,
    /// each push is the pair `On*TimerUpdate(state, expiryTime)` unpacks
    /// (`crimewatchSvc.py:280`) [V]; any expiry it carries is the act plus
    /// the timeout; and once the timeout has passed the last word is idle,
    /// so the ring is deleted (`crimewatchTimers.py:666`) [V].
    #[test]
    fn the_hud_hears_every_timer_start_and_end() {
        let mut reg = both_in_space(HOME);
        aggression(&mut reg, ALICE, BOB, T0, T0);
        let mut out = due(&mut reg);
        for s in 1..=(16 * 60) {
            tick(&mut reg, T0 + s * SEC);
            out.extend(due(&mut reg));
        }
        for (pilot, event, timeout, idle) in [
            (ALICE, "OnWeaponsTimerUpdate", WEAPONS_TIMEOUT, 100),
            (ALICE, "OnPvpTimerUpdate", PVP_TIMEOUT, 200),
            (BOB, "OnPvpTimerUpdate", PVP_TIMEOUT, 200),
        ] {
            let got = sent(&out, pilot, event);
            assert!(got.len() >= 2, "{pilot} {event}: {got:?}");
            assert!(got[0].0 != idle, "starts armed");
            assert_eq!(got.last(), Some(&(idle, None)), "{event} ends idle");
            assert!(
                got.iter()
                    .filter_map(|(_, e)| *e)
                    .all(|e| e == T0 + timeout)
            );
        }
        assert!(sent(&out, BOB, "OnWeaponsTimerUpdate").is_empty());
        assert!(
            reg.combat().crimewatch().records.is_empty(),
            "clean pilots are forgotten"
        );
    }

    /// **A suspect**: their own HUD counts the criminal timer down in its
    /// suspect state (`TimerSuspect` 304, drawn as a countdown at
    /// `crimewatchTimers.py:715-717`) for `criminalTimerTimeout` [V]; every
    /// other pilot in the system is told by `OnSystemCriminalFlagUpdates`,
    /// and finds them in `GetClientStates`' suspects; and when it runs out
    /// both are taken back -- the pilot's timer idle, the system's flag in
    /// `newIdles`.
    #[test]
    fn a_suspect_is_flagged_to_the_system_and_cleared_on_expiry() {
        let mut reg = both_in_space(AWAY);
        suspect(&mut reg, ALICE, T0);
        let out = due(&mut reg);
        assert_eq!(
            sent(&out, ALICE, "OnCriminalTimerUpdate"),
            vec![(304, Some(T0 + CRIMINAL_TIMEOUT))]
        );
        let flag = out
            .iter()
            .find(|(_, n, _)| *n == "OnSystemCriminalFlagUpdates")
            .expect("the system is told");
        let mut to = flag.0.clone();
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB]);
        assert_eq!(
            flag.2,
            vec![
                PyValue::List(vec![]),
                PyValue::List(vec![PyValue::Int(ALICE)]),
                PyValue::List(vec![]),
            ]
        );
        let PyValue::Tuple(parts) = client_states(&reg, Some(BOB), T0) else {
            unreachable!()
        };
        assert_eq!(
            parts[2],
            PyValue::Tuple(vec![
                PyValue::List(vec![]),
                PyValue::List(vec![PyValue::Int(ALICE)])
            ])
        );

        tick(&mut reg, T0 + CRIMINAL_TIMEOUT);
        let out = due(&mut reg);
        assert_eq!(
            sent(&out, ALICE, "OnCriminalTimerUpdate"),
            vec![(300, None)]
        );
        let flag = out
            .iter()
            .find(|(_, n, _)| *n == "OnSystemCriminalFlagUpdates")
            .expect("the system is told it is over");
        assert_eq!(flag.2[0], PyValue::List(vec![PyValue::Int(ALICE)]));
        assert_eq!(flag.2[1], PyValue::List(vec![]));
    }

    /// **No theft in a system we declare zero-sec**: there the client's
    /// `CanAttackFreely` is `True` for everyone (`crimewatchSvc.py:541-543`)
    /// and so is its `HaveLootRight` (`michelle.py:1987`) [V] -- the take is
    /// not an offence, and nobody is told anything.
    #[test]
    fn a_take_in_declared_zero_sec_is_not_theft() {
        let mut reg = both_in_space(HOME);
        assert!(declared_null_systems(&reg).contains(&HOME));
        theft(&mut reg, ALICE, HOME, T0);
        assert!(due(&mut reg).is_empty());
        assert_eq!(
            reg.combat().crimewatch().phase(ALICE, Timer::Criminal, T0),
            Phase::Idle
        );
    }

    /// Does any batch on the clock carry the `RemoveBalls` action, and to whom?
    fn removals(out: &[(Vec<i64>, &'static str, Vec<PyValue>)]) -> Vec<Vec<i64>> {
        fn mentions(v: &PyValue, want: &[u8]) -> bool {
            match v {
                PyValue::Bytes(b) => b.as_slice() == want,
                PyValue::List(items) | PyValue::Tuple(items) => {
                    items.iter().any(|i| mentions(i, want))
                }
                _ => false,
            }
        }
        out.iter()
            .filter(|(_, n, args)| {
                *n == "DoDestinyUpdate" && args.iter().any(|a| mentions(a, b"RemoveBalls"))
            })
            .map(|(a, _, _)| a.clone())
            .collect()
    }

    /// **A pilot who logs off in a fight leaves the ship in space until the
    /// log-off timer runs out** -- "Ship will remain in space on log-off
    /// until timer expires" (283269) [V], fifteen minutes for a capsuleer
    /// fight (`crimewatch/const.py`) [V] -- and then it leaves, with a
    /// `RemoveBalls` on the clock to whoever is still watching.
    #[test]
    fn a_logged_off_ship_stays_in_space_until_its_log_off_timer_ends() {
        let mut reg = both_in_space(HOME);
        aggression(&mut reg, ALICE, BOB, T0, T0);
        let _ = logged_off(&mut reg, ALICE, T0);
        let until = reg.combat().crimewatch().held_until(ALICE).expect("held");
        assert!(until >= T0 + PVP_TIMEOUT, "at least the pvp timer");

        tick(&mut reg, T0 + PVP_TIMEOUT - SEC);
        assert!(reg.roster().get(ALICE).is_some(), "still in space");
        assert!(removals(&due(&mut reg)).is_empty());

        tick(&mut reg, until);
        assert!(reg.roster().get(ALICE).is_none(), "gone when it ends");
        assert_eq!(removals(&due(&mut reg)), vec![vec![BOB]]);
        assert_eq!(reg.combat().crimewatch().held_until(ALICE), None);
    }

    /// **A held ship is still a ship in space**: the locks on it survive the
    /// log-off, so it can be shot, and a volley at it starts its pilot's
    /// log-off timer again -- which the hold then honours: "until timer
    /// expires" is the timer as it is, not as it was at the log-off [V].
    #[test]
    fn a_held_ship_can_be_shot_and_a_new_volley_keeps_it_in_space() {
        let mut reg = both_in_space(HOME);
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        aggression(&mut reg, ALICE, BOB, T0, T0);
        let _ = logged_off(&mut reg, BOB, T0);
        assert_eq!(reg.locks().targets_of(ALICE_SHIP), vec![BOB_SHIP]);

        let later = T0 + 10 * MIN;
        aggression(&mut reg, ALICE, BOB, later, later);
        tick(&mut reg, T0 + PVP_TIMEOUT);
        assert!(reg.roster().get(BOB).is_some(), "the new timer holds it");
        assert_eq!(
            reg.combat().crimewatch().held_until(BOB),
            Some(later + PVP_TIMEOUT)
        );
        tick(&mut reg, later + PVP_TIMEOUT);
        assert!(reg.roster().get(BOB).is_none());
    }

    /// **The pilot comes back before the hold ends**: the hold is cancelled,
    /// the clock never takes the ship away, the login's session change puts
    /// them in space -- `stationid` absent and `solarsystemid` set, which
    /// `gameui.OnSessionChanged:592-597` routes to the inflight view [V] --
    /// and the ballpark bind finds the ship where it is, re-seeding nothing
    /// and telling nobody it arrived (everyone had its ball all along).
    #[test]
    fn a_pilot_who_logs_back_in_rejoins_the_held_ship_in_space() {
        let mut reg = both_in_space(HOME);
        aggression(&mut reg, ALICE, BOB, T0, T0);
        let _ = logged_off(&mut reg, ALICE, T0);
        let where_ = reg.roster().get(ALICE).expect("held").motion.position;

        let change = reg.character_change(ALICE).expect("a login");
        // A marshal dict is `(value, key)` (`crate::pyutil::pydict`).
        let get = |k: &str| {
            change
                .iter()
                .find(|(_, key)| crate::pyutil::as_bytes(key) == Some(k.as_bytes()))
                .map(|(v, _)| v.clone())
        };
        let new = |k: &str| match get(k) {
            Some(PyValue::Tuple(p)) => p[1].clone(),
            other => panic!("{k}: {other:?}"),
        };
        assert_eq!(new("solarsystemid"), PyValue::Int(HOME));
        assert_eq!(new("stationid"), PyValue::None);
        assert_eq!(new("shipid"), PyValue::Int(ALICE_SHIP));

        assert!(reclaim(&mut reg, ALICE));
        let _ = due(&mut reg);
        let _ = reg.take_pending();
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        assert_eq!(
            reg.roster().get(ALICE).expect("in space").motion.position,
            where_,
            "not re-seeded at the station"
        );
        let told: Vec<Vec<i64>> = reg
            .take_pending()
            .into_iter()
            .filter_map(|(_, p)| p.audience())
            .collect();
        assert!(
            told.iter().all(|a| a == &vec![ALICE]),
            "the state to the pilot, nothing to anyone else: {told:?}"
        );

        tick(&mut reg, T0 + 2 * PVP_TIMEOUT);
        assert!(reg.roster().get(ALICE).is_some(), "never released");
    }

    /// A docked pilot's log-off holds nothing: there is no ship in space.
    #[test]
    fn a_docked_log_off_holds_nothing() {
        let mut reg = both_in_space(HOME);
        reg.roster_mut().leave(ALICE);
        assert!(!logged_off(&mut reg, ALICE, T0));
        assert_eq!(reg.combat().crimewatch().held_until(ALICE), None);
    }

    /// The emergency warp's heading is a direction: unit length, whatever
    /// the ship and the instant.
    #[test]
    fn the_emergency_heading_is_a_unit_vector() {
        for (ship, now) in [
            (1, 0),
            (ALICE_SHIP, T0),
            (BOB_SHIP, T0 + 7),
            (i64::MAX, i64::MAX),
        ] {
            let h = emergency_heading(ship, now);
            let len = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-9, "{h:?}");
        }
    }

    // ---- LOGIN-SPACE: the spot a released ship comes back to.

    /// One field of `character_change`, new value.
    fn change_field(reg: &Registry, k: &str) -> Option<PyValue> {
        let change = reg.character_change(ALICE).expect("a login");
        change
            .iter()
            .find(|(_, key)| crate::pyutil::as_bytes(key) == Some(k.as_bytes()))
            .map(|(v, _)| match v {
                PyValue::Tuple(p) => p[1].clone(),
                other => panic!("{k}: {other:?}"),
            })
    }

    /// Alice logs off in space and the hold runs out: she is out of the
    /// roster with a spot written; Bob stays.
    fn two_systems() -> crate::celestial::Universe {
        let row = |id: i64| format!("{id}\t19001\t29001\t\t\t0.75\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx");
        crate::celestial::Universe::parse(&[row(HOME), row(AWAY)], &[], &[]).unwrap()
    }

    fn released(system: i64, at: [f64; 3]) -> Registry {
        let mut reg = place_both(HOME, Some(two_systems()));
        reg.roster_mut().leave(ALICE);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ALICE,
                ALICE_SHIP,
                system,
                crate::space_state::Motion::arrived(at, [0.0; 3]),
            ));
        let _ = logged_off(&mut reg, ALICE, T0);
        tick(&mut reg, T0 + LOGOFF_MINIMUM);
        assert!(reg.roster().get(ALICE).is_none(), "released");
        let _ = reg.take_pending();
        let _ = due(&mut reg);
        reg
    }

    fn adds_for(out: &[(Vec<i64>, &'static str, Vec<PyValue>)], who: i64) -> usize {
        fn mentions(v: &PyValue) -> bool {
            match v {
                v if *v == crate::pyutil::bstr("AddBalls2") => true,
                PyValue::List(i) | PyValue::Tuple(i) => i.iter().any(mentions),
                _ => false,
            }
        }
        out.iter()
            .filter(|(a, n, args)| {
                *n == "DoDestinyUpdate" && a == &vec![who] && args.iter().any(mentions)
            })
            .count()
    }

    #[test]
    fn a_released_ship_comes_back_where_it_was() {
        let reg = released(HOME, [7_000.0, 0.0, 0.0]);
        assert_eq!(change_field(&reg, "stationid"), Some(PyValue::None));
        assert_eq!(
            change_field(&reg, "solarsystemid"),
            Some(PyValue::Int(HOME))
        );
        assert_eq!(change_field(&reg, "shipid"), Some(PyValue::Int(ALICE_SHIP)));
    }

    #[test]
    fn the_system_is_the_spot_after_a_jump() {
        let reg = released(AWAY, [1.0, 2.0, 3.0]);
        assert_eq!(
            change_field(&reg, "solarsystemid"),
            Some(PyValue::Int(AWAY))
        );
        assert_eq!(
            change_field(&reg, "solarsystemid2"),
            Some(PyValue::Int(AWAY))
        );
    }

    #[test]
    fn session_system_and_docked_station_follow_the_spot() {
        let reg = released(AWAY, [1.0, 2.0, 3.0]);
        assert_eq!(reg.session_system(ALICE), Some(AWAY));
        assert_eq!(reg.docked_station(ALICE), None);
        let mut plain = place_both(HOME, Some(two_systems()));
        plain.roster_mut().leave(ALICE);
        assert_eq!(
            plain.session_system(ALICE),
            Some(HOME),
            "no spot, no change"
        );
    }

    /// Local and the session keys read the spot's system in the bind gap.
    #[test]
    fn local_and_where_now_follow_the_spot() {
        let reg = released(AWAY, [1.0, 2.0, 3.0]);
        assert_eq!(reg.present_system(ALICE), Some(AWAY));
        let rooms = crate::chat::system_rooms(&reg, ALICE);
        assert!(rooms.contains(&format!("local_{AWAY}")), "{rooms:?}");
        assert!(!rooms.contains(&format!("local_{HOME}")), "{rooms:?}");
        let at = reg.where_now(ALICE).expect("a place");
        assert_eq!(
            (at.station, at.system, at.system2),
            (None, Some(AWAY), AWAY)
        );
    }

    /// **No free escape**: the restoring protection ends with the log-off,
    /// so the hold exposes the ship, and the release cannot be renewed by
    /// dropping and returning.
    #[test]
    fn a_log_off_ends_the_protection_of_a_restored_ship() {
        let mut reg = both_in_space(HOME);
        crate::invuln::set_enabled(&mut reg, true);
        crate::invuln::grant_restoring(&mut reg, ALICE, ALICE_SHIP, T0);
        assert!(crate::invuln::ship_protected(&reg, ALICE_SHIP, T0 + SEC));
        let _ = logged_off(&mut reg, ALICE, T0 + SEC);
        assert!(!crate::invuln::ship_protected(
            &reg,
            ALICE_SHIP,
            T0 + 2 * SEC
        ));
    }

    #[test]
    fn bootstrap_before_the_bind_seeds_nothing() {
        let mut reg = released(HOME, [7_000.0, 0.0, 0.0]);
        assert!(crate::destiny::bootstrap_for(&mut reg, ALICE).is_none());
        assert!(reg.roster().get(ALICE).is_none(), "nobody seated early");
    }

    #[test]
    fn the_bind_lands_at_the_spot_and_tells_observers() {
        let mut reg = released(HOME, [7_000.0, 0.0, 0.0]);
        assert!(
            !reclaim(&mut reg, ALICE),
            "the hold is over: nothing to reclaim"
        );
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let p = reg.roster().get(ALICE).expect("seated");
        assert_eq!(p.motion.position, [7_000.0, 0.0, 0.0]);
        let mut out = due(&mut reg);
        out.extend(reg.take_pending().into_iter().map(|(_, p)| {
            let (n, a) = p.to_push();
            (p.audience().unwrap_or_default(), n, a)
        }));
        assert_eq!(
            adds_for(&out, BOB),
            1,
            "Bob saw the ball leave, sees it return"
        );
        assert!(crate::logoff_spot::get(&reg, ALICE).is_none(), "taken");
    }

    #[test]
    fn a_restored_ship_gets_the_restoring_push_for_sixty_seconds() {
        let mut reg = released(HOME, [7_000.0, 0.0, 0.0]);
        crate::invuln::set_enabled(&mut reg, true);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let out = due(&mut reg);
        let (_, _, args) = out
            .iter()
            .find(|(_, n, _)| *n == "OnRestoringInvulnUpdated")
            .expect("the restoring push");
        assert_eq!(args[0], PyValue::Int(ALICE_SHIP));
        assert_eq!(args[2], PyValue::Int(crate::invuln::RESTORING));
        assert!(
            out.iter()
                .all(|(_, n, _)| *n != "OnInvulunOnUndockingUpdated")
        );
    }

    #[test]
    fn a_rejoin_during_the_hold_gets_no_restoring_invuln_and_no_spot() {
        let mut reg = both_in_space(HOME);
        let _ = logged_off(&mut reg, ALICE, T0);
        assert!(
            crate::logoff_spot::get(&reg, ALICE).is_some(),
            "written at the log-off"
        );
        crate::invuln::set_enabled(&mut reg, true);
        assert!(reclaim(&mut reg, ALICE));
        assert!(crate::logoff_spot::get(&reg, ALICE).is_none());
        let _ = due(&mut reg);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let out = due(&mut reg);
        assert!(out.iter().all(|(_, n, _)| *n != "OnRestoringInvulnUpdated"));
    }

    #[test]
    fn a_ship_shot_in_the_hold_records_the_spot_at_the_end() {
        let mut reg = both_in_space(HOME);
        aggression(&mut reg, ALICE, BOB, T0, T0);
        let _ = logged_off(&mut reg, ALICE, T0);
        let end = reg.combat().crimewatch().held_until(ALICE).expect("held");
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("held")
            .motion
            .position = [9_000.0, 0.0, 0.0];
        tick(&mut reg, end);
        let spot = crate::logoff_spot::get(&reg, ALICE).expect("recorded at the end");
        assert_eq!(spot.position[0], 9_000.0);
    }

    #[test]
    fn a_ship_killed_in_the_hold_leaves_no_spot() {
        let mut reg = both_in_space(HOME);
        let _ = logged_off(&mut reg, ALICE, T0);
        assert!(crate::logoff_spot::get(&reg, ALICE).is_some());
        leave_space(&mut reg, ALICE);
        tick(&mut reg, T0 + LOGOFF_MINIMUM);
        assert!(crate::logoff_spot::get(&reg, ALICE).is_none());
        assert!(reg.deltas().of(crate::logoff_spot::KIND).is_empty());
    }

    #[test]
    fn the_spot_survives_a_save_and_load() {
        let reg = released(AWAY, [4.0, 5.0, 6.0]);
        let snap = reg.to_snapshot();
        let mut fresh = place_both(HOME, Some(two_systems()));
        fresh.roster_mut().leave(ALICE);
        fresh.apply_snapshot(snap);
        let spot = crate::logoff_spot::get(&fresh, ALICE).expect("loaded");
        assert_eq!((spot.system, spot.position), (AWAY, [4.0, 5.0, 6.0]));
    }

    #[test]
    fn an_undock_after_a_restore_is_ordinary() {
        let mut reg = released(HOME, [7_000.0, 0.0, 0.0]);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        reg.roster_mut().leave(ALICE);
        let _ = due(&mut reg);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let p = reg.roster().get(ALICE).expect("seated");
        assert_ne!(
            p.motion.position,
            [7_000.0, 0.0, 0.0],
            "the station's mouth, not the old spot"
        );
    }

    // ---- CP3b: security from the universe table.

    /// **`flags_apply` is false where the client's rounded security is
    /// `<= 0.0` and true above** [V rule], for every system of the fixture
    /// table -- which holds an exactly-zero system and one just above zero
    /// that the client lifts to low sec.
    #[test]
    fn flags_follow_the_tables_security() {
        let u = crate::celestial::Universe::fixture();
        let reg = Registry::new(crate::model::SandboxConfig::default());
        let mut seen = (false, false);
        for s in u.systems() {
            let rounded = crate::celestial::pseudo_security(s.security);
            assert_eq!(flags_apply(&reg, s.id), rounded > 0.0, "{s:?}");
            if rounded > 0.0 {
                seen.0 = true
            } else {
                seen.1 = true
            }
        }
        assert!(seen.0 && seen.1, "the fixture has both sides of the line");
    }

    /// A system the table lacks keeps today's rule: flags apply unless we
    /// declare it zero-sec (the client takes the `0.0` we send over its own
    /// number [V]). **CW-1: a system the table knows is never declared**, so
    /// a pilot stationed in a high-sec system lives under its law.
    #[test]
    fn a_missing_system_falls_back_and_a_known_one_is_never_declared() {
        let reg = Registry::new(crate::model::SandboxConfig::default());
        let u = reg.content().get::<crate::celestial::Universe>();
        assert!(u.system(AWAY).is_none() && u.system(HOME).is_none());
        assert!(flags_apply(&reg, AWAY));
        assert!(!flags_apply(&reg, HOME));

        // An invented high-sec row for our declared system.
        let u = crate::celestial::Universe::parse(
            &[format!(
                "{HOME}\t19000001\t29000001\t\t\t0.75\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx"
            )],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            u.security_class(HOME),
            Some(crate::celestial::SecurityClass::High)
        );
        let reg = place_both(HOME, Some(u));
        assert!(!declared_null_systems(&reg).contains(&HOME));
        assert!(flags_apply(&reg, HOME), "the table's high sec");
        assert_eq!(space_of(&reg, HOME), Space::High);
    }

    /// Theft follows the table: a take in a table null-sec system flags
    /// nobody, and one in a low-sec system makes the thief a suspect.
    #[test]
    fn theft_in_table_null_sec_is_not_theft_and_in_low_sec_is() {
        let u = crate::celestial::Universe::fixture();
        let null = u
            .systems()
            .find(|s| s.security < 0.0)
            .map(|s| s.id)
            .expect("a fixture null-sec system");
        let low = u
            .systems()
            .find(|s| s.security_class() == crate::celestial::SecurityClass::Low)
            .map(|s| s.id)
            .expect("a fixture low-sec system");

        // The pilots are homed in AWAY: a configured character's system is
        // declared zero-sec, which would hide the table's answer.
        let mut reg = place_both(AWAY, Some(u.clone()));
        theft(&mut reg, ALICE, null, T0);
        assert_eq!(
            reg.combat().crimewatch().phase(ALICE, Timer::Criminal, T0),
            Phase::Idle
        );

        let mut reg = place_both(AWAY, Some(u));
        theft(&mut reg, ALICE, low, T0);
        assert_ne!(
            reg.combat().crimewatch().phase(ALICE, Timer::Criminal, T0),
            Phase::Idle
        );
    }
}
