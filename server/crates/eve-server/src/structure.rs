//! The Upwell structure model: state table, timers, upkeep, reinforcement
//! timing and the deployment conflict check (lane U1, issue #61; spec
//! `handback/r-upwell` §2, §4, §7).
//!
//! **A port of the shared client package `structures`** (`structure.py`,
//! `const.py`, `reinforce.py`, `deployment.py`), which is CCP's logic on both
//! sides of the wire **[V]**, so the transitions here are the client's own
//! and a later lane's slim states line up with what the client reads. It is
//! pure: no wire, no session, no Destiny. The Python object fires `Signal`s
//! and saves through hooks; here every change is an [`Event`] on
//! [`Structure::events`] for the caller to turn into pushes, and the one
//! persistence is the delta row below.
//!
//! **Time** is a FILETIME (`i64`, 100 ns), passed in through [`Env::now`];
//! this module never reads a clock. The hooks the Python leaves abstract
//! (`HasOnlineServiceModule`, `CalculateDeedState`, the random draws of
//! `randint`) are fields of [`Env`] or [`Structure::set_deed`], so a test
//! decides every one of them.
//!
//! **One row per structure**, kind [`KIND`] (`delta_kinds::KINDS`), the key
//! the structure's item id (a real item from the allocator, spec §4):
//!
//! ```text
//! structure <id> -> type owner_corp system x y z yaw name bio profile state
//!     upkeep shield armor hull timer_start timer_end timer_paused
//!     unanchor_at reinforce_weekday reinforce_hour deed_present
//!     [next_weekday next_hour apply_at vulnerable repairing first_aggressed
//!      last_aggressed notified abandon_at abandon_warned deploy_start
//!      deploy_end]
//! ```
//!
//! A `-` is `None`. The first 22 fields are the spec's; the bracketed ones
//! are the rest of the Python object's state and read as `None`/false when a
//! row is shorter, so a row can grow at its end. A row that does not parse
//! is logged and skipped: one structure lost, not the world.
//!
//! **Not ported**: `Deploy`'s auto moon miner beacon rule and the drilling
//! platform exemption near a moon (`FindMoonBeaconInRangeOfPosition`,
//! `GetMiningBeaconPositionForMoon`: moon mining is MN-H, not the home set),
//! so a drill is refused near a moon, which is the stricter side; and every
//! `PlaySpecialFX` and notification text (events only).
//!
//! **[A] assumptions** (falsifier in each):
//! - The reinforce exit lands on the next `hour:00` UTC at or after the
//!   timer's end ([`earliest_at_hour`]): `datetimeutils
//!   .find_earliest_time_after_datetime` is not in the tree, and
//!   `gametime.GetTimeOffsetInHours` is taken as 0. Falsifier: an exit an
//!   hour off, or a day late on the exact hour, in a reinforcement test.
//! - A new structure starts at [`State::AnchorVulnerable`] (spec A2).
//!   Falsifier: a client animation or timer mismatch right after `Anchor`.
//! - `ImpendingAbandon` fires once, when [`ABANDON_WARNING_SECS`] remain;
//!   the Python hook is abstract. Falsifier: none a client can show.

use crate::delta::Deltas;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "structure";

/// One second in FILETIME ticks.
pub const SECOND: i64 = 10_000_000;
/// One day in FILETIME ticks (the FILETIME epoch starts at midnight).
pub const DAY: i64 = 86_400 * SECOND;

// -- const.py ---------------------------------------------------------------

/// `structures.STATE_*` **[V]**, by the client's own numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Unknown,
    Unanchored,
    Anchoring,
    OnlineDeprecated,
    FittingInvulnerable,
    OnliningVulnerable,
    ShieldVulnerable,
    ArmorReinforce,
    ArmorVulnerable,
    HullReinforce,
    HullVulnerable,
    AnchorVulnerable,
    DeployVulnerable,
    FobInvulnerable,
}

impl State {
    /// Every state, in code order.
    pub const ALL: [State; 14] = [
        State::Unknown,
        State::Unanchored,
        State::Anchoring,
        State::OnlineDeprecated,
        State::FittingInvulnerable,
        State::OnliningVulnerable,
        State::ShieldVulnerable,
        State::ArmorReinforce,
        State::ArmorVulnerable,
        State::HullReinforce,
        State::HullVulnerable,
        State::AnchorVulnerable,
        State::DeployVulnerable,
        State::FobInvulnerable,
    ];

    /// The number the client's slim `state` carries.
    pub fn code(self) -> i64 {
        match self {
            State::Unknown => 0,
            State::Unanchored => 1,
            State::Anchoring => 2,
            State::OnlineDeprecated => 100,
            State::FittingInvulnerable => 101,
            State::OnliningVulnerable => 102,
            State::ShieldVulnerable => 110,
            State::ArmorReinforce => 111,
            State::ArmorVulnerable => 112,
            State::HullReinforce => 113,
            State::HullVulnerable => 114,
            State::AnchorVulnerable => 115,
            State::DeployVulnerable => 116,
            State::FobInvulnerable => 118,
        }
    }

    pub fn from_code(code: i64) -> Option<State> {
        State::ALL.into_iter().find(|s| s.code() == code)
    }

    /// `structures.OFFLINE_STATES`: a pilot cannot dock (`OfferDock`).
    pub fn is_offline(self) -> bool {
        matches!(
            self,
            State::Unknown
                | State::Unanchored
                | State::Anchoring
                | State::AnchorVulnerable
                | State::DeployVulnerable
        )
    }

    /// `structures.DISABLED_STATES`.
    pub fn is_disabled(self) -> bool {
        self.is_offline()
            || matches!(
                self,
                State::HullReinforce | State::HullVulnerable | State::OnliningVulnerable
            )
    }

    /// `structures.OPERATIONAL_STATES`: neither offline nor disabled.
    pub fn is_operational(self) -> bool {
        !self.is_disabled()
    }

    /// `structures.TETHER_ENABLED_STATES`.
    pub fn tether_enabled(self) -> bool {
        matches!(
            self,
            State::OnliningVulnerable
                | State::ShieldVulnerable
                | State::ArmorReinforce
                | State::ArmorVulnerable
                | State::HullReinforce
                | State::HullVulnerable
                | State::FobInvulnerable
        )
    }

    /// `structures.CAN_ACTIVATE_EQUIPMENT_IN_STATE`.
    pub fn can_activate_equipment(self) -> bool {
        matches!(
            self,
            State::ShieldVulnerable
                | State::ArmorReinforce
                | State::ArmorVulnerable
                | State::HullReinforce
                | State::HullVulnerable
                | State::FobInvulnerable
        )
    }

    /// `structures.STATE_SHIELD_ARMOR_HULL`: the damage fractions a state
    /// forces (1 intact, 0 gone). `Unknown` has no row.
    pub fn shield_armor_hull(self) -> Option<(f64, f64, f64)> {
        Some(match self {
            State::Unknown => return None,
            State::Unanchored | State::OnlineDeprecated | State::ShieldVulnerable => {
                (1.0, 1.0, 1.0)
            }
            State::FobInvulnerable => (1.0, 1.0, 1.0),
            State::ArmorReinforce | State::ArmorVulnerable => (0.0, 1.0, 1.0),
            State::Anchoring
            | State::FittingInvulnerable
            | State::OnliningVulnerable
            | State::HullReinforce
            | State::HullVulnerable
            | State::AnchorVulnerable
            | State::DeployVulnerable => (0.0, 0.0, 1.0),
        })
    }

    /// `structures.STATE_VULNERABILITY`. `Unknown` has no row.
    pub fn vulnerability(self) -> Option<bool> {
        Some(match self {
            State::Unknown => return None,
            State::Unanchored
            | State::Anchoring
            | State::OnlineDeprecated
            | State::FittingInvulnerable
            | State::ArmorReinforce
            | State::HullReinforce
            | State::FobInvulnerable => false,
            State::OnliningVulnerable
            | State::ShieldVulnerable
            | State::ArmorVulnerable
            | State::HullVulnerable
            | State::AnchorVulnerable
            | State::DeployVulnerable => true,
        })
    }

    /// `structures.STATE_CANCELS_UNANCHOR`: `Some(true)` cancels. The
    /// client's table has no row for `FobInvulnerable`.
    pub fn cancels_unanchor(self) -> Option<bool> {
        Some(match self {
            State::OnlineDeprecated | State::OnliningVulnerable | State::ShieldVulnerable => false,
            State::FobInvulnerable => return None,
            _ => true,
        })
    }

    /// `structures.STATE_REPAIR_TIME`, seconds; `None` falls back to
    /// [`DEFAULT_REPAIR_TIMER`] (`GetRepairTime`).
    pub fn repair_secs(self) -> i64 {
        match self {
            State::HullVulnerable => 1800,
            _ => DEFAULT_REPAIR_TIMER,
        }
    }

    /// `STATES_THAT_REQUIRE_AUTO_REPAIR_TO_REPAIR`.
    fn needs_auto_repair(self) -> bool {
        matches!(
            self,
            State::ShieldVulnerable | State::ArmorVulnerable | State::HullVulnerable
        )
    }
}

/// `structures.OFFLINE_STATES` by code, for a slim `state` the caller holds
/// as a number (spec §2: `OfferDock` tests it).
pub fn code_is_offline(code: i64) -> bool {
    State::from_code(code).is_none_or(State::is_offline)
}

pub const DEFAULT_REPAIR_TIMER: i64 = 900;
pub const ANCHORING_TIME: i64 = 86_400;
pub const FLEX_DEPLOYING_TIME: i64 = 2_700;
pub const UNANCHORING_TIME: i64 = 604_800;
pub const FLEX_UNANCHORING_TIME: i64 = 2_700;
pub const FITTING_INVULNERABLE_TIME: i64 = 300;
pub const MIN_STRUCTURE_NAME_LEN: usize = 3;
pub const REINFORCE_DURATION_ARMOR_DEFAULT: i64 = 86_400;
pub const REINFORCE_DURATION_HULL_DEFAULT: i64 = 129_600;
pub const FLEX_REINFORCE_DURATION_HULL: i64 = 3_600;
pub const REINFORCE_EXIT_JITTER_DEFAULT: i64 = 10_800;
pub const REINFORCE_EXIT_JITTER_MEDIUM: i64 = 5_400;
pub const REINFORCE_EXIT_JITTER_FLEX: i64 = 1_800;
pub const DEFAULT_REINFORCE_HOUR: i64 = 18;
/// `DAY_SATURDAY`, Monday being 0.
pub const DEFAULT_REINFORCE_WEEKDAY: i64 = 5;
pub const NO_REINFORCEMENT_WEEKDAY: i64 = 255;
pub const ABANDONING_TIME_MIN: i64 = 604_800;
pub const ABANDONING_TIME_MAX: i64 = 691_200;
/// `ABANDONDED_ADVANCE_WARNING_TIME`.
pub const ABANDON_WARNING_SECS: i64 = 259_200;
/// `ALMOST_MAX_SHIELD_LEVEL` in `IsShieldDamaged`.
const ALMOST_MAX_SHIELD: f64 = 0.995;
/// The notification threshold in `IsDamageBelowNotificationThreshold`.
const NOTIFY_SHIELD_BELOW: f64 = 0.95;

/// `structures.UPKEEP_STATE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upkeep {
    Full,
    Low,
    Abandoned,
}

impl Upkeep {
    pub fn code(self) -> i64 {
        match self {
            Upkeep::Full => 1,
            Upkeep::Low => 2,
            Upkeep::Abandoned => 3,
        }
    }

    pub fn from_code(code: i64) -> Option<Upkeep> {
        [Upkeep::Full, Upkeep::Low, Upkeep::Abandoned]
            .into_iter()
            .find(|u| u.code() == code)
    }
}

/// `structures.STRUCTURE_SIZE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Flex,
    Medium,
    Large,
    ExtraLarge,
    Undefined,
}

/// `structures.STRUCTURE_SIZE_BY_TYPE` **[V]**; any other type is
/// [`Size::Undefined`].
pub fn size_of_type(type_id: i64) -> Size {
    match type_id {
        35841 | 35840 | 37534 | 81826 => Size::Flex,
        35832 | 35825 | 35835 => Size::Medium,
        35833 | 47512..=47516 | 35826 | 35836 => Size::Large,
        35834 | 40340 | 35827 => Size::ExtraLarge,
        _ => Size::Undefined,
    }
}

/// `structures.TYPES_THAT_NEVER_GO_ABANDONED` (the five Fortizar variants).
pub fn never_abandoned(type_id: i64) -> bool {
    matches!(type_id, 47512..=47516)
}

/// `structures.CheckReinforceTime` for a type with no
/// `NO_REINFORCEMENT_WEEKDAY` rule (`typeID` omitted).
pub fn check_reinforce_time(weekday: i64, hour: i64) -> Result<(), &'static str> {
    if !(0..=6).contains(&weekday) {
        return Err("reinforceWeekday must be 0..6");
    }
    check_reinforce_hour(hour)
}

/// `structures.CheckReinforceHour`.
pub fn check_reinforce_hour(hour: i64) -> Result<(), &'static str> {
    if (0..=23).contains(&hour) {
        Ok(())
    } else {
        Err("reinforceHour must be 0..23")
    }
}

/// `ConfirmDeployment`'s name rule: at least [`MIN_STRUCTURE_NAME_LEN`]
/// characters (spec §2).
pub fn valid_name(name: &str) -> bool {
    name.trim().chars().count() >= MIN_STRUCTURE_NAME_LEN
}

// -- reinforce.py -----------------------------------------------------------

/// `structures.reinforce.ReinforceTiming`: the weekly vulnerability hour and
/// a pending change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReinforceTiming {
    pub weekday: Option<i64>,
    pub hour: Option<i64>,
    pub next_weekday: Option<i64>,
    pub next_hour: Option<i64>,
    /// FILETIME at which the pending change applies.
    pub apply_at: Option<i64>,
}

impl ReinforceTiming {
    /// `SetReinforceTimes`: a direct change, cancelling a pending one.
    pub fn set(&mut self, weekday: i64, hour: i64) {
        *self = ReinforceTiming {
            weekday: Some(weekday),
            hour: Some(hour),
            ..ReinforceTiming::default()
        };
    }

    /// `SetNextReinforceTimes`.
    pub fn set_next(&mut self, weekday: i64, hour: i64, apply_at: i64) {
        self.next_weekday = Some(weekday);
        self.next_hour = Some(hour);
        self.apply_at = Some(apply_at);
    }

    /// `GetEarliestTimeAfterDatetime`: no hour set leaves the time alone.
    pub fn earliest_after(&self, t: i64) -> i64 {
        match self.hour {
            Some(h) => earliest_at_hour(t, h),
            None => t,
        }
    }

    /// `IsNextChangeReady`.
    pub fn next_change_ready(&self, now: i64) -> bool {
        self.apply_at.is_some_and(|a| a < now)
    }

    /// `ApplyNextTiming`.
    pub fn apply_next(&mut self) {
        self.weekday = self.next_weekday;
        self.hour = self.next_hour;
        self.next_weekday = None;
        self.next_hour = None;
        self.apply_at = None;
    }

    /// `SecondsUntilReinforceRollover`.
    pub fn seconds_until_rollover(&self, now: i64) -> Option<i64> {
        self.apply_at.map(|a| (a - now) / SECOND)
    }
}

/// The first instant at or after `t` whose time of day is `hour:00:00` UTC
/// **[A]** (module header: boundary and offset).
pub fn earliest_at_hour(t: i64, hour: i64) -> i64 {
    let day_start = t - t.rem_euclid(DAY);
    let candidate = day_start + hour * 3_600 * SECOND;
    if candidate >= t {
        candidate
    } else {
        candidate + DAY
    }
}

// -- structure.py -----------------------------------------------------------

/// The hooks and numbers the Python object reads from its subclass or from
/// the clock. Built per call by the caller; [`Env::new`] has the client's
/// defaults.
#[derive(Debug, Clone, Copy)]
pub struct Env {
    /// FILETIME now.
    pub now: i64,
    /// `HasOnlineServiceModule`.
    pub has_service_module: bool,
    /// `CanHaveAutoRepair`.
    pub auto_repair: bool,
    /// `GetAnchoringTime`, seconds. A world setting (spec §10.2).
    pub anchoring_secs: i64,
    /// `GetUnanchoringTime`.
    pub unanchoring_secs: i64,
    /// `GetReinforceDurationArmor` / `Hull`.
    pub reinforce_armor_secs: i64,
    pub reinforce_hull_secs: i64,
    /// Stands for `randint(-jitter, jitter)` in the reinforce exit: any
    /// number, reduced into the range.
    pub jitter_roll: u64,
    /// Stands for `randint(ABANDONING_TIME_MIN, MAX)`.
    pub abandon_roll: u64,
    /// The structure is being shot right now: the rolling DPS on its current
    /// layer is above the pause-repair threshold. `[I]` the client's
    /// `Update` pauses the repair timer on this, not on the layer merely
    /// being damaged (a damaged layer is what repair exists to clear). The
    /// caller owns the window; its length is `[A]`, falsified by a client
    /// capture of a structure's repair timer after the last hit. Default
    /// false: nothing shoots structures yet.
    pub under_fire: bool,
}

impl Env {
    pub fn new(now: i64) -> Env {
        Env {
            now,
            has_service_module: false,
            auto_repair: true,
            anchoring_secs: ANCHORING_TIME,
            unanchoring_secs: UNANCHORING_TIME,
            reinforce_armor_secs: REINFORCE_DURATION_ARMOR_DEFAULT,
            reinforce_hull_secs: REINFORCE_DURATION_HULL_DEFAULT,
            jitter_roll: 0,
            abandon_roll: 0,
            under_fire: false,
        }
    }
}

/// What the Python object signals (`OnStateChanged` and the rest), in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    StateChanged { from: State, to: State },
    DamageChanged,
    VulnerabilityChanged { to: bool },
    RepairingChanged { to: Option<bool> },
    RepairCompleted,
    DeployTimeChanged,
    AggressionChanged,
    FirstAggressed,
    UnanchoringChanged,
    ReinforceTimeChanged,
    TimerChanged,
    UpkeepChanged { from: Option<Upkeep>, to: Upkeep },
    ImpendingAbandon,
    DeedChanged,
}

/// `structures.structure.CountdownTimer` without its owner: every method
/// that reads the clock takes `now`. `set` and the others answer whether
/// the timer changed, for the owner to signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timer {
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub paused: Option<i64>,
}

impl Timer {
    /// `IsRunning`.
    pub fn is_running(&self) -> bool {
        self.start.is_some() && self.end.is_some()
    }

    /// `IsExpired`: a timer with no end is expired.
    pub fn is_expired(&self, now: i64) -> bool {
        self.end.is_none_or(|e| e <= now)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.is_some()
    }

    /// `GetTimerRemaining`, whole seconds.
    pub fn remaining_secs(&self, now: i64) -> Option<i64> {
        self.end.map(|e| (e - now) / SECOND)
    }

    /// `ClearTimer`.
    pub fn clear(&mut self) -> bool {
        if *self == Timer::default() {
            return false;
        }
        *self = Timer::default();
        true
    }

    /// `SetTimer(end, start, paused)`. The Python compares the arguments,
    /// not the result, so a call with no `start` always counts as a change
    /// unless the stored start is also `None`; kept.
    pub fn set(&mut self, now: i64, end: i64, start: Option<i64>, paused: Option<i64>) -> bool {
        if Some(end) == self.end && start == self.start && paused == self.paused {
            return false;
        }
        self.start = Some(start.unwrap_or(now));
        self.end = Some(end);
        self.paused = paused;
        true
    }

    /// `Pause`.
    pub fn pause(&mut self, now: i64) -> bool {
        if !self.is_running() || self.paused.is_some() {
            return false;
        }
        let (end, start) = (self.end.unwrap_or(now), self.start);
        self.set(now, end, start, Some(now))
    }

    /// `Unpause`: the paused span moves both ends later.
    pub fn unpause(&mut self, now: i64) -> bool {
        if !self.is_running() {
            return false;
        }
        let Some(p) = self.paused else { return false };
        let elapsed = now - p;
        let (Some(s), Some(e)) = (self.start, self.end) else {
            return false;
        };
        self.set(now, e + elapsed, Some(s + elapsed), None)
    }
}

/// One Upwell structure's model state (`structures.structure.Structure`
/// plus the row fields the server owns).
#[derive(Debug, Clone, PartialEq)]
pub struct Structure {
    pub id: i64,
    pub type_id: i64,
    pub owner_corp: i64,
    pub system: i64,
    pub pos: [f64; 3],
    /// Radians **[I]** (spec §2).
    pub yaw: f64,
    pub name: String,
    pub bio: String,
    pub profile: i64,
    pub state: State,
    pub upkeep: Option<Upkeep>,
    /// `(shield, armor, hull)`, each 0..=1; `None` is the unset
    /// `(None, None, None)`.
    pub damage: Option<(f64, f64, f64)>,
    pub vulnerable: Option<bool>,
    pub repairing: Option<bool>,
    pub timer: Timer,
    pub deploy_start: Option<i64>,
    pub deploy_end: Option<i64>,
    pub unanchoring: Option<i64>,
    pub reinforce: ReinforceTiming,
    pub first_aggressed: Option<i64>,
    pub last_aggressed: Option<i64>,
    pub notified: bool,
    pub abandon_at: Option<i64>,
    pub abandon_warned: bool,
    /// `HasDeedInSlot`: the deed fitted in its slot (spec §2).
    pub deed_present: bool,
    /// Signals since the last [`Structure::take_events`]. Not persisted.
    pub events: Vec<Event>,
}

impl Structure {
    /// A structure that has just been anchored: state `Unknown` moved to
    /// [`State::AnchorVulnerable`] **[A]** (module header), the default
    /// reinforcement hour set, damage set by the state.
    #[allow(clippy::too_many_arguments)] // the Anchor call's own fields
    pub fn anchored(
        id: i64,
        type_id: i64,
        owner_corp: i64,
        system: i64,
        pos: [f64; 3],
        yaw: f64,
        name: &str,
        profile: i64,
        env: &Env,
    ) -> Structure {
        let mut s = Structure {
            id,
            type_id,
            owner_corp,
            system,
            pos,
            yaw,
            name: name.to_owned(),
            bio: String::new(),
            profile,
            state: State::Unknown,
            upkeep: None,
            damage: None,
            vulnerable: None,
            repairing: None,
            timer: Timer::default(),
            deploy_start: None,
            deploy_end: None,
            unanchoring: None,
            reinforce: ReinforceTiming::default(),
            first_aggressed: None,
            last_aggressed: None,
            notified: false,
            abandon_at: None,
            abandon_warned: false,
            deed_present: false,
            events: Vec::new(),
        };
        s.reinforce
            .set(DEFAULT_REINFORCE_WEEKDAY, DEFAULT_REINFORCE_HOUR);
        s.set_state(State::AnchorVulnerable, env);
        s
    }

    /// Drain the signals.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn size(&self) -> Size {
        size_of_type(self.type_id)
    }

    pub fn is_flex(&self) -> bool {
        self.size() == Size::Flex
    }

    pub fn is_medium(&self) -> bool {
        self.size() == Size::Medium
    }

    /// `IsOnline`.
    pub fn is_online(&self) -> bool {
        !self.state.is_offline()
    }

    /// `IsDisabled`.
    pub fn is_disabled(&self) -> bool {
        self.state.is_disabled()
    }

    /// Whether a pilot can dock now (spec §2: dock is possible from
    /// onlining on, i.e. the state is not offline).
    pub fn can_dock(&self) -> bool {
        self.is_online()
    }

    /// `Update`: one tick. Idempotent once the structure has settled.
    pub fn update(&mut self, env: &Env) {
        self.update_vulnerability();
        self.update_state(env);
        self.update_timers(env);
        self.update_repair(env);
        self.update_unanchoring(env);
        self.update_reinforce_timer(env);
        self.update_upkeep(env);
    }

    // -- state --

    /// `SetState`.
    pub fn set_state(&mut self, state: State, env: &Env) {
        let current = self.state;
        if state != current {
            self.state = state;
            self.handle_state_changed(env);
            self.events.push(Event::StateChanged {
                from: current,
                to: state,
            });
        }
    }

    fn handle_state_changed(&mut self, env: &Env) {
        if self.timer.clear() {
            self.events.push(Event::TimerChanged);
        }
        self.set_deploying_timer(None, None);
        self.set_repairing(None);
        self.update_vulnerability();
        self.update_timers(env);
        self.set_aggression(None, None, env);
        if let Some((s, a, h)) = self.state.shield_armor_hull() {
            self.set_damage(s, a, h, env);
        }
        self.update_repair(env);
        self.update_unanchoring(env);
        self.update_reinforce_timer(env);
    }

    fn update_vulnerability(&mut self) {
        if let Some(v) = self.state.vulnerability() {
            self.set_vulnerable(v);
        }
    }

    fn set_vulnerable(&mut self, v: bool) {
        if self.vulnerable != Some(v) {
            self.vulnerable = Some(v);
            self.events.push(Event::VulnerabilityChanged { to: v });
        }
    }

    fn update_state(&mut self, env: &Env) {
        let now = env.now;
        match self.state {
            State::ShieldVulnerable => {
                if self.is_armor_damaged() {
                    // CanHaveArmorPhases is True.
                    if self.can_have_armor_reinforce() {
                        self.set_state(State::ArmorReinforce, env);
                    } else {
                        self.set_state(State::ArmorVulnerable, env);
                    }
                } else if self.is_hull_damaged() {
                    self.hull_phase(env);
                }
            }
            State::ArmorReinforce => {
                if self.timer.is_expired(now) {
                    self.set_state(State::ArmorVulnerable, env);
                }
            }
            State::ArmorVulnerable => {
                if !self.is_damaged() {
                    self.set_state(State::ShieldVulnerable, env);
                } else if self.is_hull_damaged() {
                    self.hull_phase(env);
                }
            }
            State::HullReinforce => {
                if self.timer.is_expired(now) {
                    self.set_state(State::HullVulnerable, env);
                }
            }
            State::HullVulnerable => {
                if self.is_shield_or_armor_fully_healed() {
                    self.set_state(State::ShieldVulnerable, env);
                }
            }
            State::AnchorVulnerable => {
                if !self.is_damaged() {
                    self.set_state(State::Anchoring, env);
                }
            }
            State::Anchoring => {
                if self.timer.is_expired(now) {
                    self.set_state(State::OnliningVulnerable, env);
                }
            }
            State::FittingInvulnerable => {
                if self.timer.is_expired(now) {
                    self.set_state(State::ShieldVulnerable, env);
                }
            }
            State::OnliningVulnerable => {
                if !self.is_damaged() && self.deed_present {
                    self.set_state(State::ShieldVulnerable, env);
                }
            }
            State::DeployVulnerable => {
                if self.deploy_time_completed(now) && !self.is_hull_damaged() {
                    self.set_state(State::ShieldVulnerable, env);
                }
            }
            State::OnlineDeprecated => self.set_state(State::ShieldVulnerable, env),
            State::Unknown | State::Unanchored | State::FobInvulnerable => {}
        }
    }

    fn hull_phase(&mut self, env: &Env) {
        if self.can_have_hull_reinforce() {
            self.set_state(State::HullReinforce, env);
        } else {
            self.set_state(State::HullVulnerable, env);
        }
    }

    fn can_have_armor_reinforce(&self) -> bool {
        if self.upkeep == Some(Upkeep::Abandoned) || self.is_flex() {
            return false;
        }
        self.is_medium() || self.upkeep != Some(Upkeep::Low)
    }

    fn can_have_hull_reinforce(&self) -> bool {
        if self.upkeep == Some(Upkeep::Abandoned) {
            return false;
        }
        if self.is_flex() && self.upkeep == Some(Upkeep::Low) {
            return false;
        }
        !self.is_medium()
    }

    fn deploy_time_completed(&self, now: i64) -> bool {
        self.deploy_end.is_some_and(|e| e < now)
    }

    // -- damage --

    /// `SetDamage`: each fraction clamped to 0..=1.
    pub fn set_damage(&mut self, shield: f64, armor: f64, hull: f64, env: &Env) {
        let c = |v: f64| v.clamp(0.0, 1.0);
        let damage = Some((c(shield), c(armor), c(hull)));
        if damage != self.damage {
            self.damage = damage;
            // HandleDamageChanged
            self.update_aggression(env);
            self.update(env);
            self.events.push(Event::DamageChanged);
        }
    }

    /// Py2's `None < x` is true, so an unset damage reads as damaged here
    /// (callers guard with [`Self::is_damaged`] as the Python does).
    fn is_shield_damaged(&self) -> bool {
        self.damage.is_none_or(|d| d.0 < ALMOST_MAX_SHIELD)
    }

    fn is_armor_damaged(&self) -> bool {
        self.damage.is_none_or(|d| d.1 < 1.0)
    }

    fn is_hull_damaged(&self) -> bool {
        self.damage.is_none_or(|d| d.2 < 1.0)
    }

    /// `IsDamaged`.
    pub fn is_damaged(&self) -> bool {
        self.damage.is_some()
            && (self.is_shield_damaged() || self.is_armor_damaged() || self.is_hull_damaged())
    }

    fn is_shield_or_armor_fully_healed(&self) -> bool {
        self.damage.is_none() || !self.is_shield_damaged() || !self.is_armor_damaged()
    }

    // -- aggression --

    fn is_aggressed(&self) -> bool {
        match self.state {
            State::ShieldVulnerable => self.is_shield_damaged(),
            State::ArmorVulnerable => self.is_armor_damaged(),
            State::HullVulnerable | State::AnchorVulnerable | State::DeployVulnerable => {
                self.is_hull_damaged()
            }
            _ => false,
        }
    }

    fn update_aggression(&mut self, env: &Env) {
        if self.is_aggressed() {
            let first = self.first_aggressed.unwrap_or(env.now);
            self.set_aggression(Some(first), Some(env.now), env);
        } else {
            self.set_aggression(None, None, env);
        }
    }

    /// `SetAggression(aggressed, latest)`.
    fn set_aggression(&mut self, first: Option<i64>, latest: Option<i64>, _env: &Env) {
        if first != self.first_aggressed || latest != self.last_aggressed {
            self.first_aggressed = first;
            self.last_aggressed = latest;
            self.events.push(Event::AggressionChanged);
        }
        if self.first_aggressed.is_none() {
            self.notified = false;
        }
        if self.first_aggressed.is_some() && !self.notified && self.below_notification_threshold() {
            self.notified = true;
            self.events.push(Event::FirstAggressed);
        }
    }

    fn below_notification_threshold(&self) -> bool {
        if self.state == State::ShieldVulnerable {
            return self.damage.is_none_or(|d| d.0 < NOTIFY_SHIELD_BELOW);
        }
        true
    }

    // -- repair and timers --

    fn should_repair(&self, env: &Env) -> bool {
        if self.state.needs_auto_repair() && !env.auto_repair {
            return false;
        }
        if self.vulnerable != Some(true) {
            return false;
        }
        match self.state {
            State::DeployVulnerable => self.is_hull_damaged(),
            State::OnliningVulnerable => self.is_damaged() && self.deed_present,
            _ => self.is_damaged(),
        }
    }

    fn set_repairing(&mut self, r: Option<bool>) {
        if r != self.repairing {
            self.repairing = r;
            self.events.push(Event::RepairingChanged { to: r });
        }
    }

    fn update_repair(&mut self, env: &Env) {
        if !self.should_repair(env) {
            self.set_repairing(None);
            return;
        }
        let now = env.now;
        if self.repairing.is_none() {
            let end = now + self.state.repair_secs() * SECOND;
            if self.timer.set(now, end, Some(now), None) {
                self.events.push(Event::TimerChanged);
            }
            self.set_repairing(Some(true));
        } else if env.under_fire {
            // Not `last_aggressed.is_some()`: that stays set while a layer is
            // damaged, so the repair would pause for as long as it is needed.
            if self.timer.pause(now) {
                self.events.push(Event::TimerChanged);
            }
            self.set_repairing(Some(false));
        } else {
            if self.timer.unpause(now) {
                self.events.push(Event::TimerChanged);
            }
            self.set_repairing(Some(true));
        }
        if self.repairing == Some(true) && self.timer.is_expired(now) {
            if self.timer.clear() {
                self.events.push(Event::TimerChanged);
            }
            if self.state == State::DeployVulnerable {
                self.set_damage(0.0, 0.0, 1.0, env); // RepairHullOnly
            } else {
                self.set_damage(1.0, 1.0, 1.0, env); // FullyRepairSelf
            }
            self.events.push(Event::RepairCompleted);
        }
    }

    fn update_timers(&mut self, env: &Env) {
        if self.should_repair(env) {
            return;
        }
        let now = env.now;
        let changed = match self.state {
            State::ShieldVulnerable
            | State::ArmorVulnerable
            | State::HullVulnerable
            | State::AnchorVulnerable
            | State::OnliningVulnerable
            | State::OnlineDeprecated
            | State::FobInvulnerable => self.timer.clear(),
            State::ArmorReinforce => {
                !self.timer.is_running() && {
                    let end = self.reinforce_exit(now, env.reinforce_armor_secs, env);
                    self.timer.set(now, end, None, None)
                }
            }
            State::HullReinforce => {
                !self.timer.is_running() && {
                    let secs = if self.is_flex() {
                        FLEX_REINFORCE_DURATION_HULL
                    } else {
                        env.reinforce_hull_secs
                    };
                    let end = self.reinforce_exit(now, secs, env);
                    self.timer.set(now, end, None, None)
                }
            }
            State::Anchoring => {
                !self.timer.is_running()
                    && self
                        .timer
                        .set(now, now + env.anchoring_secs * SECOND, None, None)
            }
            State::FittingInvulnerable => {
                !self.timer.is_running()
                    && self
                        .timer
                        .set(now, now + FITTING_INVULNERABLE_TIME * SECOND, None, None)
            }
            State::DeployVulnerable => {
                if self.deploy_start.is_none() || self.deploy_end.is_none() {
                    self.set_deploying_timer(Some(now), Some(now + FLEX_DEPLOYING_TIME * SECOND));
                }
                false
            }
            State::Unknown | State::Unanchored => false,
        };
        if changed {
            self.events.push(Event::TimerChanged);
        }
    }

    /// `Generate{Armor,Hull}ReinforceExitDateTime`: the end of the phase,
    /// pushed to the weekly hour, then jittered.
    fn reinforce_exit(&self, start: i64, secs: i64, env: &Env) -> i64 {
        let exit = self.reinforce.earliest_after(start + secs * SECOND);
        let range = match self.size() {
            Size::Flex => REINFORCE_EXIT_JITTER_FLEX,
            Size::Medium => REINFORCE_EXIT_JITTER_MEDIUM,
            _ => REINFORCE_EXIT_JITTER_DEFAULT,
        };
        let jitter = (env.jitter_roll % (2 * range as u64 + 1)) as i64 - range;
        exit + jitter * SECOND
    }

    fn set_deploying_timer(&mut self, start: Option<i64>, end: Option<i64>) {
        if start == self.deploy_start && end == self.deploy_end {
            return;
        }
        self.deploy_start = start;
        self.deploy_end = end;
        self.events.push(Event::DeployTimeChanged);
    }

    // -- unanchoring --

    fn set_unanchoring(&mut self, at: Option<i64>) {
        if at != self.unanchoring {
            self.unanchoring = at;
            self.events.push(Event::UnanchoringChanged);
        }
    }

    fn update_unanchoring(&mut self, env: &Env) {
        if self.state.cancels_unanchor() == Some(true) {
            self.set_unanchoring(None);
        }
        if self.unanchoring.is_some_and(|u| u <= env.now) {
            self.set_state(State::Unanchored, env);
        }
    }

    /// `StartUnanchoring`: a no-op if one is running.
    pub fn start_unanchoring(&mut self, env: &Env) {
        if self.unanchoring.is_none() {
            let secs = if self.is_flex() {
                FLEX_UNANCHORING_TIME
            } else {
                env.unanchoring_secs
            };
            self.set_unanchoring(Some(env.now + secs * SECOND));
        }
    }

    /// `StopUnanchoring`.
    pub fn stop_unanchoring(&mut self) {
        if self.unanchoring.is_some() {
            self.set_unanchoring(None);
        }
    }

    /// `GetUnanchoringRemaining`, whole seconds.
    pub fn unanchoring_remaining(&self, now: i64) -> Option<i64> {
        self.unanchoring.map(|u| (u - now) / SECOND)
    }

    // -- reinforcement hour --

    fn update_reinforce_timer(&mut self, env: &Env) {
        // CanApplyNextReinforceTime
        if self.state == State::ShieldVulnerable
            && !self.is_damaged()
            && self.reinforce.next_change_ready(env.now)
        {
            self.reinforce.apply_next();
            self.events.push(Event::ReinforceTimeChanged);
        }
    }

    /// `SetReinforceTimes`.
    pub fn set_reinforce_times(&mut self, weekday: i64, hour: i64) {
        self.reinforce.set(weekday, hour);
        self.events.push(Event::ReinforceTimeChanged);
    }

    /// `SetNextReinforceTimes`: applies `delay_secs` from now.
    pub fn set_next_reinforce_times(&mut self, weekday: i64, hour: i64, delay_secs: i64, now: i64) {
        self.reinforce
            .set_next(weekday, hour, now + delay_secs * SECOND);
        self.events.push(Event::ReinforceTimeChanged);
    }

    // -- upkeep --

    fn allows_abandonment(&self) -> bool {
        !never_abandoned(self.type_id)
    }

    fn update_upkeep(&mut self, env: &Env) {
        match self.upkeep {
            Some(Upkeep::Full) => {
                if !env.has_service_module {
                    self.set_upkeep(Upkeep::Low, env);
                } else if self.abandon_at.is_some() {
                    self.abandon_at = None;
                    self.abandon_warned = false;
                }
            }
            Some(Upkeep::Low) => {
                if env.has_service_module {
                    self.set_upkeep(Upkeep::Full, env);
                } else if self.abandon_at.is_none() {
                    if self.allows_abandonment() {
                        let span = (ABANDONING_TIME_MAX - ABANDONING_TIME_MIN + 1) as u64;
                        let secs = ABANDONING_TIME_MIN + (env.abandon_roll % span) as i64;
                        self.abandon_at = Some(env.now + secs * SECOND);
                        self.abandon_warned = false;
                    }
                } else if self.abandon_at.is_some_and(|a| a < env.now) {
                    self.set_upkeep(Upkeep::Abandoned, env);
                } else if let Some(a) = self.abandon_at
                    && !self.abandon_warned
                    && a - env.now <= ABANDON_WARNING_SECS * SECOND
                {
                    self.abandon_warned = true;
                    self.events.push(Event::ImpendingAbandon);
                }
            }
            Some(Upkeep::Abandoned) => {
                if env.has_service_module {
                    self.set_upkeep(Upkeep::Full, env);
                }
            }
            // The client's fallback for an unrecognised (here: unset) state.
            None => self.set_upkeep(Upkeep::Full, env),
        }
    }

    fn set_upkeep(&mut self, to: Upkeep, env: &Env) {
        if self.upkeep != Some(to) {
            let from = self.upkeep;
            self.upkeep = Some(to);
            self.events.push(Event::UpkeepChanged { from, to });
            self.update_upkeep(env);
        }
    }

    // -- deed --

    /// `UpdateDeed` as a push: the caller says whether the deed is in its
    /// slot. A change runs [`Self::update`] (onlining waits for it) and
    /// signals.
    pub fn set_deed(&mut self, present: bool, env: &Env) {
        if present != self.deed_present {
            self.deed_present = present;
            self.update(env);
            self.events.push(Event::DeedChanged);
        }
    }
}

// -- deployment.py ----------------------------------------------------------

/// `deployment.DEPLOY_DIST_MAX`, metres: how far from the ship `Anchor` may
/// place a structure **[I]** (spec §2).
pub const DEPLOY_DIST_MAX: f64 = 800_000.0;
/// `deployment.MINIMUM_MOON_DISTANCE`.
pub const MINIMUM_MOON_DISTANCE: f64 = 4_000_000.0;

const GROUP_MOON: i64 = 8;
const GROUP_STARGATE: i64 = 10;
const CATEGORY_STRUCTURE: i64 = 65;
/// `GROUP_MIN_DISTANCE_TO_STARGATE`: only the jump bridge group has one.
const GROUP_JUMP_BRIDGE: i64 = 1_408;
const JUMP_BRIDGE_STARGATE_DISTANCE: f64 = 100_000_000.0;

/// `deployment.GROUP_DISTANCE`, metres, by group id **[V]**.
pub fn group_distance(group_id: i64) -> Option<f64> {
    Some(match group_id {
        1915 => 0.0,
        14 | 186 | 12 | 306 | 340 | 448 | 649 | 1978 => 5_000.0,
        1998 | 226 => 100_000.0,
        4079 | 925 => 1_000_000.0,
        1876 => 2_000_000.0,
        1408 | 2016 => 500_000.0,
        2017 => 200_000.0,
        1012 => 10_000_000.0,
        _ => return None,
    })
}

/// `deployment.CATEGORY_DISTANCES`, metres, by category id **[V]**.
pub fn category_distance(category_id: i64) -> Option<f64> {
    Some(match category_id {
        11 | 6 | 18 | 87 | 22 | 25 => 5_000.0,
        2 | 46 | 40 | 3 | 23 | CATEGORY_STRUCTURE => 1_000_000.0,
        _ => return None,
    })
}

/// `GetDeploymentDistance`: the group's, else the category's, else 0.
pub fn deployment_distance(group_id: i64, category_id: i64) -> f64 {
    group_distance(group_id)
        .or_else(|| category_distance(category_id))
        .unwrap_or(0.0)
}

/// What the structure being placed is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeployType {
    pub type_id: i64,
    pub group_id: i64,
    pub category_id: i64,
    pub radius: f64,
}

/// `deployment.WARPIN_POINT` / `REAL_BALL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointKind {
    WarpinPoint,
    RealBall,
}

/// One thing in the system the placement is checked against: a ball with
/// its type's group and category, or a warp-in point (radius 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpaceBall {
    pub type_id: i64,
    pub group_id: i64,
    pub category_id: i64,
    pub pos: [f64; 3],
    pub radius: f64,
    pub kind: PointKind,
}

/// `FindDeploymentConflict`'s answer: the blocker's type, the distance that
/// was required (what the client's message names), and its kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conflict {
    pub type_id: i64,
    pub min_distance: f64,
    pub kind: PointKind,
}

/// `DistanceBetween`.
pub fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// `FindDeploymentConflict`: the first ball, in order, that `subject` at
/// `location` is too near, or `None` (module header: the moon-mining
/// branches are not ported).
pub fn find_deployment_conflict(
    subject: &DeployType,
    location: [f64; 3],
    balls: &[SpaceBall],
) -> Option<Conflict> {
    let mine = deployment_distance(subject.group_id, subject.category_id);
    for b in balls {
        let d = distance(location, b.pos);
        if b.kind == PointKind::RealBall
            && b.group_id == GROUP_MOON
            && d < MINIMUM_MOON_DISTANCE + subject.radius + b.radius
        {
            return Some(Conflict {
                type_id: b.type_id,
                min_distance: MINIMUM_MOON_DISTANCE,
                kind: b.kind,
            });
        }
        if subject.group_id == GROUP_JUMP_BRIDGE
            && b.group_id == GROUP_STARGATE
            && d < JUMP_BRIDGE_STARGATE_DISTANCE + subject.radius + b.radius
        {
            return Some(Conflict {
                type_id: b.type_id,
                min_distance: JUMP_BRIDGE_STARGATE_DISTANCE,
                kind: b.kind,
            });
        }
        let mut min = deployment_distance(b.group_id, b.category_id);
        if b.category_id == CATEGORY_STRUCTURE {
            min = min.min(mine);
        }
        if min != 0.0 && d < min + subject.radius + b.radius {
            return Some(Conflict {
                type_id: b.type_id,
                min_distance: min,
                kind: b.kind,
            });
        }
    }
    None
}

/// `EnforcePositionRestrictions` as a distance test **[I]**: the offset
/// from the deploying ship must be within [`DEPLOY_DIST_MAX`].
pub fn within_deploy_range(ship: [f64; 3], location: [f64; 3]) -> bool {
    distance(ship, location) <= DEPLOY_DIST_MAX
}

// -- the delta row ------------------------------------------------------------

fn opt(v: Option<i64>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| v.to_string())
}

fn opt_f(v: Option<f64>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| v.to_string())
}

fn flag(v: Option<bool>) -> String {
    match v {
        None => "-".to_owned(),
        Some(b) => u8::from(b).to_string(),
    }
}

fn field(v: &[String], i: usize) -> Result<&str, String> {
    v.get(i)
        .map(String::as_str)
        .ok_or_else(|| format!("field {i} missing"))
}

fn num<T: std::str::FromStr>(v: &[String], i: usize) -> Result<T, String> {
    field(v, i)?
        .parse()
        .map_err(|_| format!("field {i} is not a number"))
}

fn opt_num<T: std::str::FromStr>(v: &[String], i: usize) -> Result<Option<T>, String> {
    match v.get(i).map(String::as_str) {
        None | Some("-") => Ok(None),
        Some(s) => s
            .parse()
            .map(Some)
            .map_err(|_| format!("field {i} is not a number")),
    }
}

fn opt_flag(v: &[String], i: usize) -> Option<bool> {
    match v.get(i).map(String::as_str) {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

/// The key of a structure's row.
pub fn row_key(id: i64) -> Vec<String> {
    vec![id.to_string()]
}

impl Structure {
    /// The row's value fields (module header).
    pub fn to_row(&self) -> Vec<String> {
        let (sh, ar, hu) = match self.damage {
            Some((s, a, h)) => (Some(s), Some(a), Some(h)),
            None => (None, None, None),
        };
        vec![
            self.type_id.to_string(),
            self.owner_corp.to_string(),
            self.system.to_string(),
            self.pos[0].to_string(),
            self.pos[1].to_string(),
            self.pos[2].to_string(),
            self.yaw.to_string(),
            self.name.clone(),
            self.bio.clone(),
            self.profile.to_string(),
            self.state.code().to_string(),
            opt(self.upkeep.map(Upkeep::code)),
            opt_f(sh),
            opt_f(ar),
            opt_f(hu),
            opt(self.timer.start),
            opt(self.timer.end),
            opt(self.timer.paused),
            opt(self.unanchoring),
            opt(self.reinforce.weekday),
            opt(self.reinforce.hour),
            u8::from(self.deed_present).to_string(),
            opt(self.reinforce.next_weekday),
            opt(self.reinforce.next_hour),
            opt(self.reinforce.apply_at),
            flag(self.vulnerable),
            flag(self.repairing),
            opt(self.first_aggressed),
            opt(self.last_aggressed),
            u8::from(self.notified).to_string(),
            opt(self.abandon_at),
            u8::from(self.abandon_warned).to_string(),
            opt(self.deploy_start),
            opt(self.deploy_end),
        ]
    }

    /// Read a row back. The first 22 fields are required; the rest default.
    pub fn from_row(key: &[String], v: &[String]) -> Result<Structure, String> {
        let id: i64 = key
            .first()
            .and_then(|k| k.parse().ok())
            .ok_or("key is not an id")?;
        let state = State::from_code(num(v, 10)?).ok_or("unknown state")?;
        let upkeep = match opt_num::<i64>(v, 11)? {
            None => None,
            Some(c) => Some(Upkeep::from_code(c).ok_or("unknown upkeep")?),
        };
        let damage = match (opt_num(v, 12)?, opt_num(v, 13)?, opt_num(v, 14)?) {
            (Some(s), Some(a), Some(h)) => Some((s, a, h)),
            _ => None,
        };
        if v.len() < 22 {
            return Err("row is shorter than 22 fields".to_owned());
        }
        Ok(Structure {
            id,
            type_id: num(v, 0)?,
            owner_corp: num(v, 1)?,
            system: num(v, 2)?,
            pos: [num(v, 3)?, num(v, 4)?, num(v, 5)?],
            yaw: num(v, 6)?,
            name: field(v, 7)?.to_owned(),
            bio: field(v, 8)?.to_owned(),
            profile: num(v, 9)?,
            state,
            upkeep,
            damage,
            vulnerable: opt_flag(v, 25),
            repairing: opt_flag(v, 26),
            timer: Timer {
                start: opt_num(v, 15)?,
                end: opt_num(v, 16)?,
                paused: opt_num(v, 17)?,
            },
            deploy_start: opt_num(v, 32)?,
            deploy_end: opt_num(v, 33)?,
            unanchoring: opt_num(v, 18)?,
            reinforce: ReinforceTiming {
                weekday: opt_num(v, 19)?,
                hour: opt_num(v, 20)?,
                next_weekday: opt_num(v, 22)?,
                next_hour: opt_num(v, 23)?,
                apply_at: opt_num(v, 24)?,
            },
            first_aggressed: opt_num(v, 27)?,
            last_aggressed: opt_num(v, 28)?,
            notified: opt_flag(v, 29).unwrap_or(false),
            abandon_at: opt_num(v, 30)?,
            abandon_warned: opt_flag(v, 31).unwrap_or(false),
            deed_present: opt_flag(v, 21).unwrap_or(false),
            events: Vec::new(),
        })
    }
}

/// One structure's row, if it has one. A row that does not parse is logged
/// and read as absent.
pub fn get(d: &Deltas, id: i64) -> Option<Structure> {
    let key = row_key(id);
    let value = d.of(KIND).get(&key)?;
    Structure::from_row(&key, value)
        .map_err(|e| crate::errln!("[structure] skipping row {key:?}: {e}"))
        .ok()
}

/// Every structure, in id order.
pub fn all(d: &Deltas) -> Vec<Structure> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            Structure::from_row(k, v)
                .map_err(|e| crate::errln!("[structure] skipping row {k:?}: {e}"))
                .ok()
        })
        .collect()
}

/// The structures owned by `corp`.
pub fn of_corp(d: &Deltas, corp: i64) -> Vec<Structure> {
    all(d)
        .into_iter()
        .filter(|s| s.owner_corp == corp)
        .collect()
}

/// The structures in `system`.
pub fn in_system(d: &Deltas, system: i64) -> Vec<Structure> {
    all(d).into_iter().filter(|s| s.system == system).collect()
}

/// Write one structure's row. Pending [`Structure::events`] are not stored.
pub fn put(d: &mut Deltas, s: &Structure) {
    d.put(KIND, row_key(s.id), s.to_row());
}

/// Remove a structure's row (it left space).
pub fn remove(d: &mut Deltas, id: i64) -> bool {
    d.del(KIND, &row_key(id)).is_some()
}

/// How many updates one tick runs at most while a structure still moves.
pub const SETTLE_PASSES: usize = 8;

/// The tick: [`Structure::update`] on every structure, saving those whose
/// row changed. `env_for` builds each one's [`Env`] (the caller knows its
/// modules and settings); the answer is each changed structure's id with the
/// signals the tick raised, for the caller's pushes and slim changes. An
/// unchanged row writes nothing, so a quiet world does not touch the
/// kind's revision.
pub fn tick(d: &mut Deltas, mut env_for: impl FnMut(&Structure) -> Env) -> Vec<(i64, Vec<Event>)> {
    let mut out = Vec::new();
    for mut s in all(d) {
        let env = env_for(&s);
        let before = s.to_row();
        // Some steps move one state per update (the client polls); settle.
        for _ in 0..SETTLE_PASSES {
            let row = s.to_row();
            s.update(&env);
            if s.to_row() == row {
                break;
            }
        }
        if s.to_row() != before {
            put(d, &s);
        }
        let events = s.take_events();
        if !events.is_empty() {
            out.push((s.id, events));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 134_000_000_000_000_000;
    const ASTRAHUS: i64 = 35_832;

    fn env(now: i64) -> Env {
        Env::new(now)
    }

    fn at(secs: i64) -> i64 {
        T0 + secs * SECOND
    }

    fn fresh(e: &Env) -> Structure {
        Structure::anchored(7, ASTRAHUS, 11, 22, [1.0, 2.0, 3.0], 0.5, "Home", 3, e)
    }

    /// Run updates until nothing moves.
    fn settle(s: &mut Structure, e: &Env) {
        for _ in 0..16 {
            let row = s.to_row();
            s.update(e);
            if s.to_row() == row {
                return;
            }
        }
        panic!("did not settle");
    }

    #[test]
    fn state_codes_round_trip_and_tables_agree() {
        for st in State::ALL {
            assert_eq!(State::from_code(st.code()), Some(st));
        }
        assert_eq!(State::from_code(5), None);
        assert!(State::AnchorVulnerable.is_offline());
        assert!(State::Anchoring.is_offline());
        assert!(!State::OnliningVulnerable.is_offline());
        assert!(State::OnliningVulnerable.is_disabled());
        assert!(State::ShieldVulnerable.is_operational());
        assert!(code_is_offline(115) && !code_is_offline(102));
        // An invulnerable state is never also vulnerable.
        assert_eq!(State::FittingInvulnerable.vulnerability(), Some(false));
        assert_eq!(State::ShieldVulnerable.vulnerability(), Some(true));
        assert_eq!(State::ShieldVulnerable.cancels_unanchor(), Some(false));
        assert_eq!(State::FobInvulnerable.cancels_unanchor(), None);
    }

    #[test]
    fn anchoring_runs_repair_then_the_anchor_timer_then_waits_for_the_deed() {
        let e0 = env(at(0));
        let mut s = fresh(&e0);
        assert_eq!(s.state, State::AnchorVulnerable);
        assert!(!s.can_dock());
        settle(&mut s, &e0);
        // Hull-only damage: the repair timer runs (900 s).
        assert_eq!(s.state, State::AnchorVulnerable);
        assert_eq!(s.repairing, Some(true));
        assert_eq!(s.timer.end, Some(at(DEFAULT_REPAIR_TIMER)));

        let e1 = env(at(DEFAULT_REPAIR_TIMER + 1));
        settle(&mut s, &e1);
        assert_eq!(s.state, State::Anchoring);
        assert_eq!(s.repairing, None);
        assert_eq!(
            s.timer.end,
            Some(at(DEFAULT_REPAIR_TIMER + 1 + ANCHORING_TIME))
        );

        // Anchoring is not over yet.
        settle(&mut s, &env(at(DEFAULT_REPAIR_TIMER + ANCHORING_TIME)));
        assert_eq!(s.state, State::Anchoring);

        // Timer out: onlining, which is dockable but waits for the deed.
        let e2 = env(at(DEFAULT_REPAIR_TIMER + 2 + ANCHORING_TIME));
        settle(&mut s, &e2);
        assert_eq!(s.state, State::OnliningVulnerable);
        assert!(s.can_dock());
        assert!(!s.timer.is_running());
        settle(&mut s, &env(at(10_000_000)));
        assert_eq!(s.state, State::OnliningVulnerable);

        // The deed arrives: the hull repair runs? No, damage is hull-only
        // and a deed makes it repair first, then shield vulnerable.
        let e3 = env(at(10_000_000));
        s.set_deed(true, &e3);
        settle(&mut s, &e3);
        assert_eq!(s.state, State::OnliningVulnerable);
        assert_eq!(s.repairing, Some(true));
        let e4 = env(at(10_000_000 + DEFAULT_REPAIR_TIMER + 1));
        settle(&mut s, &e4);
        assert_eq!(s.state, State::ShieldVulnerable);
        assert_eq!(s.damage, Some((1.0, 1.0, 1.0)));
        assert!(s.events.contains(&Event::DeedChanged));
    }

    #[test]
    fn anchor_time_is_the_envs() {
        let mut e = env(at(0));
        e.anchoring_secs = 3_600;
        let mut s = fresh(&e);
        settle(&mut s, &e);
        e.now = at(DEFAULT_REPAIR_TIMER + 1);
        settle(&mut s, &e);
        assert_eq!(s.timer.end, Some(e.now + 3_600 * SECOND));
    }

    #[test]
    fn deed_removed_does_not_leave_shield_vulnerable() {
        let e = env(at(0));
        let mut s = fresh(&e);
        s.set_state(State::ShieldVulnerable, &e);
        s.set_deed(true, &e);
        settle(&mut s, &e);
        s.set_deed(false, &e);
        settle(&mut s, &e);
        // Only OnliningVulnerable waits on the deed; the client does not
        // move a running structure back.
        assert_eq!(s.state, State::ShieldVulnerable);
    }

    #[test]
    fn timer_pause_and_unpause_shift_both_ends() {
        let mut t = Timer::default();
        assert!(!t.pause(at(0)), "a stopped timer cannot pause");
        assert!(t.set(at(0), at(100), Some(at(0)), None));
        assert!(!t.set(at(0), at(100), Some(at(0)), None), "same arguments");
        assert!(t.pause(at(10)));
        assert!(t.is_paused());
        assert!(!t.pause(at(20)), "already paused");
        assert!(t.unpause(at(40)));
        assert!(!t.is_paused());
        assert_eq!(t.start, Some(at(30)));
        assert_eq!(t.end, Some(at(130)));
        assert_eq!(t.remaining_secs(at(40)), Some(90));
        assert!(!t.unpause(at(50)), "not paused");
        assert!(!t.is_expired(at(129)) && t.is_expired(at(130)));
        assert!(t.clear() && !t.clear());
        assert!(t.is_expired(at(0)), "no end is expired");
    }

    #[test]
    fn repair_pauses_while_aggressed_and_resumes() {
        let e = env(at(0));
        let mut s = fresh(&e);
        s.set_state(State::ShieldVulnerable, &e);
        settle(&mut s, &e);
        assert_eq!(s.repairing, None, "undamaged: nothing to repair");
        // Shot to 50%: aggression starts, the repair timer starts paused-free.
        s.set_damage(0.5, 1.0, 1.0, &e);
        assert!(s.first_aggressed.is_some());
        assert!(s.events.contains(&Event::FirstAggressed));
        assert_eq!(s.repairing, Some(true));
        // Another hit while the clock is aggressed pauses the timer.
        // (Pausing reads `Env::under_fire`; a merely damaged layer does not
        // pause: see `a_structure_shot_once_and_left_alone_finishes_repairing`.)
        let e1 = Env {
            under_fire: true,
            ..env(at(100))
        };
        s.set_damage(0.4, 1.0, 1.0, &e1);
        assert_eq!(s.repairing, Some(false));
        assert!(s.timer.is_paused());
        // Same-state update with aggression still set keeps it paused.
        s.update(&e1);
        assert!(s.timer.is_paused());
    }

    #[test]
    fn a_structure_shot_once_and_left_alone_finishes_repairing() {
        let e = env(at(0));
        let mut s = fresh(&e);
        s.set_state(State::ShieldVulnerable, &e);
        s.set_damage(0.5, 1.0, 1.0, &e);
        // Ticks while nobody shoots: aggression is still set (the layer is
        // damaged) but the timer must run.
        for secs in [10, 100, 500] {
            s.update(&env(at(secs)));
            assert_eq!(s.repairing, Some(true));
            assert!(!s.timer.is_paused());
        }
        assert!(s.last_aggressed.is_some());
        s.update(&env(at(DEFAULT_REPAIR_TIMER + 1)));
        assert_eq!(s.damage, Some((1.0, 1.0, 1.0)));
        assert!(s.events.contains(&Event::RepairCompleted));
    }

    #[test]
    fn a_structure_still_under_fire_does_not_repair() {
        let e = env(at(0));
        let mut s = fresh(&e);
        s.set_state(State::ShieldVulnerable, &e);
        s.set_damage(0.5, 1.0, 1.0, &e);
        for secs in [10, 500, DEFAULT_REPAIR_TIMER + 100] {
            let f = Env {
                under_fire: true,
                ..env(at(secs))
            };
            s.update(&f);
            assert_eq!(s.repairing, Some(false));
            assert!(s.timer.is_paused());
        }
        assert_eq!(s.damage, Some((0.5, 1.0, 1.0)));
        assert!(!s.events.contains(&Event::RepairCompleted));
        // The fire stops: the timer resumes and the layer is made whole.
        let quiet = at(DEFAULT_REPAIR_TIMER + 200);
        s.update(&env(quiet));
        assert_eq!(s.repairing, Some(true));
        s.update(&env(quiet + (DEFAULT_REPAIR_TIMER + 1) * SECOND));
        assert_eq!(s.damage, Some((1.0, 1.0, 1.0)));
    }

    #[test]
    fn repair_completion_heals_fully() {
        let e = env(at(0));
        let mut s = fresh(&e);
        s.set_state(State::ShieldVulnerable, &e);
        s.set_damage(0.5, 1.0, 1.0, &e);
        s.take_events();
        // Aggression stays set while the layer is damaged; repair does not
        // wait on it, only on `under_fire`.
        let e1 = env(at(DEFAULT_REPAIR_TIMER + 1));
        s.update(&e1);
        assert_eq!(s.damage, Some((1.0, 1.0, 1.0)));
        assert!(s.events.contains(&Event::RepairCompleted));
    }

    #[test]
    fn armor_damage_reinforces_a_large_structure_and_not_a_medium_one() {
        let e = env(at(0));
        let mut large = Structure::anchored(1, 35_833, 11, 22, [0.0; 3], 0.0, "L", 1, &e);
        large.set_state(State::ShieldVulnerable, &e);
        large.upkeep = Some(Upkeep::Full);
        large.set_damage(0.0, 0.5, 1.0, &e);
        assert_eq!(large.state, State::ArmorReinforce);
        assert!(large.timer.is_running());
        assert_eq!(large.vulnerable, Some(false));

        let mut medium = fresh(&e);
        medium.set_state(State::ShieldVulnerable, &e);
        medium.upkeep = Some(Upkeep::Full);
        medium.set_damage(0.0, 0.5, 1.0, &e);
        assert_eq!(
            medium.state,
            State::ArmorReinforce,
            "medium has armor reinforce"
        );
        // Hull damage: a medium never reinforces its hull.
        let mut m2 = fresh(&e);
        m2.set_state(State::ShieldVulnerable, &e);
        m2.upkeep = Some(Upkeep::Full);
        m2.set_damage(0.0, 1.0, 0.5, &e);
        assert_eq!(m2.state, State::HullVulnerable);
        let mut l2 = Structure::anchored(1, 35_833, 11, 22, [0.0; 3], 0.0, "L", 1, &e);
        l2.set_state(State::ShieldVulnerable, &e);
        l2.upkeep = Some(Upkeep::Full);
        l2.set_damage(0.0, 1.0, 0.5, &e);
        assert_eq!(l2.state, State::HullReinforce);
    }

    #[test]
    fn low_power_removes_reinforcement_on_a_large_structure() {
        let e = env(at(0));
        let mut s = Structure::anchored(1, 35_833, 11, 22, [0.0; 3], 0.0, "L", 1, &e);
        s.set_state(State::ShieldVulnerable, &e);
        s.upkeep = Some(Upkeep::Low);
        s.set_damage(0.0, 0.5, 1.0, &e);
        assert_eq!(s.state, State::ArmorVulnerable);
    }

    #[test]
    fn reinforce_exit_is_at_the_weekly_hour_within_the_jitter() {
        let mut e = env(at(0));
        e.jitter_roll = 0; // -range
        let mut s = fresh(&e);
        s.upkeep = Some(Upkeep::Full);
        s.set_state(State::ShieldVulnerable, &e);
        s.set_damage(0.0, 0.5, 1.0, &e);
        assert_eq!(s.state, State::ArmorReinforce);
        let hour = DEFAULT_REINFORCE_HOUR * 3_600 * SECOND;
        let exit = s.timer.end.expect("a timer");
        let unjittered = exit + REINFORCE_EXIT_JITTER_MEDIUM * SECOND;
        assert_eq!(unjittered.rem_euclid(DAY), hour);
        assert!(unjittered >= e.now + REINFORCE_DURATION_ARMOR_DEFAULT * SECOND);
        // The other end of the jitter range.
        let mut e2 = e;
        e2.jitter_roll = 2 * REINFORCE_EXIT_JITTER_MEDIUM as u64;
        let mut s2 = fresh(&e2);
        s2.upkeep = Some(Upkeep::Full);
        s2.set_state(State::ShieldVulnerable, &e2);
        s2.set_damage(0.0, 0.5, 1.0, &e2);
        assert_eq!(
            s2.timer.end,
            Some(unjittered + REINFORCE_EXIT_JITTER_MEDIUM * SECOND)
        );
        // The timer running out ends the reinforcement.
        s2.update(&env(s2.timer.end.expect("end") + SECOND));
        assert_eq!(s2.state, State::ArmorVulnerable);
    }

    #[test]
    fn earliest_hour_never_goes_backwards() {
        let day = 150_000 * DAY;
        assert_eq!(
            earliest_at_hour(day + 3_600 * SECOND, 5),
            day + 5 * 3_600 * SECOND
        );
        assert_eq!(
            earliest_at_hour(day + 20 * 3_600 * SECOND, 5),
            day + DAY + 5 * 3_600 * SECOND
        );
        let t = day + 1_234;
        assert!(earliest_at_hour(t, 0) >= t);
        let timing = ReinforceTiming::default();
        assert_eq!(timing.earliest_after(t), t, "no hour set leaves it alone");
    }

    #[test]
    fn a_pending_reinforce_hour_applies_when_due_and_undamaged() {
        let e = env(at(0));
        let mut s = fresh(&e);
        s.set_state(State::ShieldVulnerable, &e);
        s.set_next_reinforce_times(1, 4, 3_600, e.now);
        assert_eq!(s.reinforce.seconds_until_rollover(e.now), Some(3_600));
        s.update(&env(at(3_599)));
        assert_eq!(s.reinforce.hour, Some(DEFAULT_REINFORCE_HOUR));
        s.update(&env(at(3_601)));
        assert_eq!((s.reinforce.weekday, s.reinforce.hour), (Some(1), Some(4)));
        assert_eq!(s.reinforce.apply_at, None);
        assert!(check_reinforce_time(6, 23).is_ok());
        assert!(check_reinforce_time(7, 0).is_err() && check_reinforce_hour(24).is_err());
    }

    #[test]
    fn unanchoring_runs_out_and_is_cancelled_by_a_reinforced_state() {
        let e = env(at(0));
        let mut s = fresh(&e);
        s.set_state(State::ShieldVulnerable, &e);
        s.start_unanchoring(&e);
        assert_eq!(s.unanchoring_remaining(e.now), Some(UNANCHORING_TIME));
        s.start_unanchoring(&env(at(5)));
        assert_eq!(
            s.unanchoring,
            Some(at(UNANCHORING_TIME)),
            "a second start keeps the first"
        );
        s.update(&env(at(UNANCHORING_TIME - 1)));
        assert_eq!(s.state, State::ShieldVulnerable);
        s.update(&env(at(UNANCHORING_TIME)));
        assert_eq!(s.state, State::Unanchored);
        assert_eq!(s.unanchoring, None, "unanchored cancels the countdown");

        let mut c = fresh(&e);
        c.upkeep = Some(Upkeep::Full);
        c.set_state(State::ShieldVulnerable, &e);
        c.start_unanchoring(&e);
        c.set_damage(0.0, 0.5, 1.0, &e);
        assert_eq!(c.unanchoring, None);
        c.start_unanchoring(&e);
        c.stop_unanchoring();
        assert_eq!(c.unanchoring, None);
    }

    #[test]
    fn upkeep_goes_low_then_abandoned_and_recovers_with_a_service_module() {
        let mut e = env(at(0));
        e.abandon_roll = 0;
        let mut s = fresh(&e);
        s.set_state(State::ShieldVulnerable, &e);
        s.update(&e);
        // No module: Full (the unset fallback) falls to Low, with a timer.
        assert_eq!(s.upkeep, Some(Upkeep::Low));
        assert_eq!(s.abandon_at, Some(at(ABANDONING_TIME_MIN)));
        // The warning fires once, three days out.
        s.take_events();
        s.update(&env(at(ABANDONING_TIME_MIN - ABANDON_WARNING_SECS - 1)));
        assert!(!s.events.contains(&Event::ImpendingAbandon));
        s.update(&env(at(ABANDONING_TIME_MIN - ABANDON_WARNING_SECS + 1)));
        s.update(&env(at(ABANDONING_TIME_MIN - ABANDON_WARNING_SECS + 2)));
        assert_eq!(
            s.events
                .iter()
                .filter(|x| **x == Event::ImpendingAbandon)
                .count(),
            1
        );
        s.update(&env(at(ABANDONING_TIME_MIN + 1)));
        assert_eq!(s.upkeep, Some(Upkeep::Abandoned));
        let mut e3 = env(at(ABANDONING_TIME_MIN + 2));
        e3.has_service_module = true;
        s.update(&e3);
        assert_eq!(s.upkeep, Some(Upkeep::Full));
        assert_eq!(s.abandon_at, None, "Full clears the abandon timer");
        s.update(&e3);
        assert_eq!(s.upkeep, Some(Upkeep::Full));
    }

    #[test]
    fn full_power_with_a_module_clears_the_abandon_timer() {
        let mut e = env(at(0));
        e.has_service_module = true;
        let mut s = fresh(&e);
        s.upkeep = Some(Upkeep::Full);
        s.abandon_at = Some(at(100));
        s.update(&e);
        assert_eq!(s.abandon_at, None);
    }

    #[test]
    fn some_types_never_go_abandoned() {
        let e = env(at(0));
        let mut s = Structure::anchored(1, 47_512, 11, 22, [0.0; 3], 0.0, "F", 1, &e);
        s.upkeep = Some(Upkeep::Low);
        s.update(&e);
        assert_eq!(s.abandon_at, None);
    }

    #[test]
    fn a_flex_structure_deploys_on_its_own_timer() {
        let e = env(at(0));
        let mut s = Structure::anchored(1, 35_841, 11, 22, [0.0; 3], 0.0, "G", 1, &e);
        s.set_state(State::DeployVulnerable, &e);
        assert_eq!(s.deploy_end, Some(at(FLEX_DEPLOYING_TIME)));
        s.update(&env(at(FLEX_DEPLOYING_TIME - 1)));
        assert_eq!(s.state, State::DeployVulnerable);
    }

    #[test]
    fn fitting_state_ends_after_five_minutes() {
        let e = env(at(0));
        let mut s = fresh(&e);
        s.set_state(State::FittingInvulnerable, &e);
        assert_eq!(s.timer.end, Some(at(FITTING_INVULNERABLE_TIME)));
        s.update(&env(at(FITTING_INVULNERABLE_TIME + 1)));
        assert_eq!(s.state, State::ShieldVulnerable);
    }

    #[test]
    fn the_name_rule_counts_characters() {
        assert!(!valid_name("ab") && !valid_name("  a ") && valid_name("abc"));
        assert!(valid_name("日本語"));
    }

    // -- deployment --

    fn ball(type_id: i64, group_id: i64, category_id: i64, x: f64, radius: f64) -> SpaceBall {
        SpaceBall {
            type_id,
            group_id,
            category_id,
            pos: [x, 0.0, 0.0],
            radius,
            kind: PointKind::RealBall,
        }
    }

    fn astrahus_like() -> DeployType {
        DeployType {
            type_id: ASTRAHUS,
            group_id: 1_657,
            category_id: CATEGORY_STRUCTURE,
            radius: 1_000.0,
        }
    }

    #[test]
    fn deployment_keeps_its_distance_from_moons_stations_and_structures() {
        let me = astrahus_like();
        // A moon (group 8, category 2): 4,000 km plus both radii.
        let moon = ball(9_001, 8, 2, 0.0, 1_500.0);
        let c = find_deployment_conflict(&me, [4_001_000.0, 0.0, 0.0], &[moon]).expect("too near");
        assert_eq!((c.type_id, c.min_distance), (9_001, MINIMUM_MOON_DISTANCE));
        assert!(find_deployment_conflict(&me, [4_002_501.0, 0.0, 0.0], &[moon]).is_none());

        // A station (category 3): 1,000 km plus radii.
        let station = ball(9_002, 15, 3, 0.0, 10_000.0);
        assert!(find_deployment_conflict(&me, [1_010_000.0, 0.0, 0.0], &[station]).is_some());
        assert!(find_deployment_conflict(&me, [1_011_001.0, 0.0, 0.0], &[station]).is_none());

        // Another structure (category 65): the smaller of the two distances.
        let other = ball(9_003, 1_657, CATEGORY_STRUCTURE, 0.0, 1_000.0);
        assert!(find_deployment_conflict(&me, [900_000.0, 0.0, 0.0], &[other]).is_some());
        assert!(find_deployment_conflict(&me, [1_002_001.0, 0.0, 0.0], &[other]).is_none());
        // A jump bridge's own 500 km wins over the 1,000 km category for a structure.
        let bridge = ball(9_004, 1_408, CATEGORY_STRUCTURE, 0.0, 0.0);
        assert!(find_deployment_conflict(&me, [502_000.0, 0.0, 0.0], &[bridge]).is_none());
        assert!(find_deployment_conflict(&me, [400_000.0, 0.0, 0.0], &[bridge]).is_some());
    }

    #[test]
    fn deployment_ignores_what_has_no_distance_and_small_things_keep_5_km() {
        let me = astrahus_like();
        let belt_rock = ball(9_005, 9, 25, 0.0, 0.0);
        assert!(find_deployment_conflict(&me, [6_500.0, 0.0, 0.0], &[belt_rock]).is_none());
        assert!(find_deployment_conflict(&me, [5_900.0, 0.0, 0.0], &[belt_rock]).is_some());
        let unknown = ball(9_006, 99_999, 99_998, 0.0, 0.0);
        assert!(find_deployment_conflict(&me, [0.0; 3], &[unknown]).is_none());
        // The first conflicting ball in list order is the one reported.
        let far = ball(9_007, 15, 3, 50_000_000.0, 0.0);
        let near = ball(9_008, 15, 3, 0.0, 0.0);
        let c = find_deployment_conflict(&me, [10.0, 0.0, 0.0], &[far, near]).expect("conflict");
        assert_eq!(c.type_id, 9_008);
        // A warp-in point has radius 0.
        let wp = SpaceBall {
            kind: PointKind::WarpinPoint,
            ..near
        };
        assert_eq!(
            find_deployment_conflict(&me, [10.0, 0.0, 0.0], &[wp]).map(|c| c.kind),
            Some(PointKind::WarpinPoint)
        );
    }

    #[test]
    fn a_jump_bridge_keeps_100_000_km_from_a_stargate() {
        let me = DeployType {
            type_id: 9_100,
            group_id: 1_408,
            category_id: CATEGORY_STRUCTURE,
            radius: 0.0,
        };
        let gate = ball(9_009, 10, 5, 0.0, 0.0);
        assert!(find_deployment_conflict(&me, [99_000_000.0, 0.0, 0.0], &[gate]).is_some());
        assert!(find_deployment_conflict(&me, [100_000_001.0, 0.0, 0.0], &[gate]).is_none());
    }

    #[test]
    fn placement_range_is_800_km() {
        assert!(within_deploy_range([0.0; 3], [800_000.0, 0.0, 0.0]));
        assert!(!within_deploy_range([0.0; 3], [800_001.0, 0.0, 0.0]));
    }

    // -- the delta round trip --

    #[test]
    fn every_field_survives_a_row_round_trip() {
        let e = env(at(0));
        let mut s = fresh(&e);
        s.name = "Ho\tme %x".to_owned();
        s.bio = "line\nbreak".to_owned();
        s.yaw = -0.123_456_789_012_345;
        s.pos = [1.5e12, -2.25, 0.1];
        s.upkeep = Some(Upkeep::Low);
        s.abandon_at = Some(at(5));
        s.abandon_warned = true;
        s.unanchoring = Some(at(6));
        s.notified = true;
        s.first_aggressed = Some(at(1));
        s.last_aggressed = Some(at(2));
        s.deed_present = true;
        s.reinforce.set_next(2, 3, at(9));
        s.timer = Timer {
            start: Some(at(1)),
            end: Some(at(8)),
            paused: Some(at(3)),
        };
        s.damage = Some((0.25, 0.5, 0.75));
        s.take_events();
        let back = Structure::from_row(&row_key(s.id), &s.to_row()).expect("parses");
        assert_eq!(back, s);

        // A fresh anchor, with unset damage and upkeep, round trips too.
        let mut u = fresh(&e);
        u.damage = None;
        u.upkeep = None;
        u.take_events();
        assert_eq!(
            Structure::from_row(&row_key(u.id), &u.to_row()).expect("parses"),
            u
        );
    }

    #[test]
    fn a_short_or_broken_row_is_refused_and_a_longer_one_still_loads() {
        let e = env(at(0));
        let mut s = fresh(&e);
        s.take_events();
        let row = s.to_row();
        assert!(Structure::from_row(&row_key(7), &row[..21]).is_err());
        let mut bad = row.clone();
        bad[10] = "77".to_owned();
        assert!(Structure::from_row(&row_key(7), &bad).is_err());
        assert!(Structure::from_row(&["x".to_owned()], &row).is_err());
        // The 22-field row of the spec: later fields read as unset.
        let short = Structure::from_row(&row_key(7), &row[..22]).expect("loads");
        assert_eq!(short.first_aggressed, None);
        assert_eq!(short.vulnerable, None);
        // A newer, longer row keeps loading.
        let mut longer = row;
        longer.push("future".to_owned());
        assert!(Structure::from_row(&row_key(7), &longer).is_ok());
    }

    #[test]
    fn the_store_helpers_put_get_list_and_remove() {
        let e = env(at(0));
        let mut d = Deltas::default();
        let mut a = fresh(&e);
        a.take_events();
        let mut b = Structure::anchored(8, 35_825, 12, 23, [0.0; 3], 0.0, "Other", 1, &e);
        b.take_events();
        put(&mut d, &a);
        put(&mut d, &b);
        assert_eq!(get(&d, 7), Some(a.clone()));
        assert_eq!(all(&d).len(), 2);
        assert_eq!(of_corp(&d, 12).len(), 1);
        assert_eq!(in_system(&d, 22).len(), 1);
        assert!(remove(&mut d, 7) && !remove(&mut d, 7));
        assert_eq!(get(&d, 7), None);
        // A broken row is skipped, not fatal.
        d.put(KIND, vec!["9".to_owned()], vec!["junk".to_owned()]);
        assert_eq!(all(&d).len(), 1);
    }

    #[test]
    fn the_tick_settles_a_structure_and_writes_only_what_changed() {
        let e = env(at(0));
        let mut d = Deltas::default();
        let mut s = fresh(&e);
        s.take_events();
        put(&mut d, &s);
        // The anchor already started its repair timer: nothing is due.
        assert!(tick(&mut d, |_| env(at(0))).is_empty());
        let rev = d.kind_revision(KIND);
        // A second tick with nothing due writes nothing and signals nothing.
        let quiet = tick(&mut d, |_| env(at(1)));
        assert!(quiet.is_empty());
        assert_eq!(d.kind_revision(KIND), rev);
        // After the repair, one tick walks to the anchoring state.
        let out = tick(&mut d, |_| env(at(DEFAULT_REPAIR_TIMER + 1)));
        assert!(out[0].1.iter().any(|ev| matches!(
            ev,
            Event::StateChanged {
                to: State::Anchoring,
                ..
            }
        )));
        assert_eq!(get(&d, 7).map(|s| s.state), Some(State::Anchoring));
    }
}
