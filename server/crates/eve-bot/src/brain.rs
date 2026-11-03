//! The bot's brain: a pure function from what it remembers and what it sees
//! to what it does next.
//!
//! **No I/O, by construction.** [`decide`] takes a [`Memory`] and an
//! [`Observation`] and returns the next [`Memory`] and a list of [`Act`]s,
//! the shape `npc::decide` has (`docs/lanes/bots.md` §2.3) -- and, like it,
//! it is tested as tables of state -> action (T1, below). It is **not**
//! `npc::decide`: that one's inputs are server tables and a rat's policy, and
//! this one's are what a player's client can know.
//!
//! **What lives where.** The world -- place, ship, hangar, hold, laser,
//! park, locks, fleet, failures -- is in the [`Observation`], which the
//! driver rebuilds from the server on every reconnect, so a reconnect cannot
//! leave a stale belief behind. [`Memory`] holds the bot's identity and
//! policy, and only the *intentions* that no server answer can restate: the
//! claimed rock, "going home", the dwell deadline, which stop push has been
//! handled.
//!
//! **The rules, in priority order** (`docs/lanes/bots.md` §4.2). The first
//! that matches wins. Everything below rule 1 is **[A] policy**: a guess at
//! what a player would do, which the human round falsifies.
//!
//! 1. In transit (no place, or the ship is in warp) -> wait.
//! 2. A fleet invite the policy accepts -> accept it.
//! 3. Docked: too many failures -> reconnect; ore in the hold or the
//!    mining hold -> unload;
//!    goods in the hangar, not yet offered this visit -> sell them
//!    ([`Act::Sell`], FX6: refine the ore, sell into bids at the floor or
//!    better; at most once per [`Policy::sell_every`], E8); a better
//!    miner's hull in the hangar ([`MINER_HULLS`]: a Venture, else a
//!    Merlin, GAPS-B) -> board it; in the capsule -> set one up (GM,
//!    or [`Act::Acquire`] in the closed economy) or say `MISSING`; no mining
//!    laser fitted (a Miner I, or the rookie ship's Civilian Miner) -> fit
//!    one (same); laser offline -> online it; mining drones in the hangar
//!    -> into the drone bay, once a visit (SOC-B10, [`crate::ore`]); else
//!    dwell, then undock.
//!
//! **The closed economy** (E8, [`Policy::closed`], [`crate::acquire`]):
//! where GM setup would run (and after it, with `--bootstrap`), the bot asks
//! for one [`Act::Acquire`] instead, at most once per
//! [`crate::acquire::ACQUIRE_EVERY`], and says `MISSING` in between. A miner
//! in its rookie ship mines with the Civilian Miner and looks for a Merlin
//! on the same cadence.
//! 4. In space, nothing bound -> bind the ballpark.
//! 5. In space: going home (hold full, belt empty, recovery) -> warp,
//!    approach, dock (drones out recalled first); the fleet boss more than
//!    a warp away -> warp to him; no rock in the park -> an ore anomaly,
//!    once a trip, then a belt (SOC-B10); no claimed rock -> claim one near
//!    the boss (or us) and go to it; claim locked and laser idle -> mine;
//!    laser running -> mining drones onto the same rock, once; then wait
//!    for the next push.
//! 6. Three consecutive failures of one kind -> recovery: in space, home;
//!    docked, reconnect.
//!
//! **Two roles** (`Policy::role`). Rules 1, 2 and 4 are every bot's; rules 3
//! and 5 above are the miner's, and the escort's own are in
//! [`crate::escort`] -- the same priority shape, a Rifter instead of a
//! Merlin, NPCs instead of rocks. A bot with a chat connection
//! ([`Observation::chat`]) also says a line when it joins a fleet; the
//! escort says more ([`crate::escort`]'s header). Every line is **[A]**.
//!
//! **Courtesy** (CP39, ruling 5; C7 §6.3). Between rules 4 and 5, a bot
//! whose role is marked `civilian` ([`crate::roles::RoleSpec::civilian`])
//! steps aside when a non-fleet human is within [`COURTESY_RANGE`]: it goes
//! home ([`Homing::Courtesy`]) and dwells the longer dwell, leaving the belt
//! or site to the player. A role that is not civilian never yields. Who is
//! a human is the driver's call, from a bot-side marker
//! ([`Observation::humans`]); nothing on the wire marks a bot.

use crate::roles::{self, RoleSpec};
use crate::verbs::ItemRow;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// A Merlin's type id **[V-data]**: the hull the loop flies.
pub const MERLIN: i64 = 603;
/// The miner's hulls, best first (GAPS-B, PROG-MINE) **[A]**: the ladder
/// ([`crate::mine_ladder::HULLS`]: exhumers, T1 barges, a Venture, the
/// Merlin) -- a hull in the hangar better than the one flown is boarded, and
/// the closed economy buys the best it can fly and pay for
/// ([`crate::acquire::hulls_in`]). Falsified by a soak whose miners in a
/// higher hull earn less than the ones in a lower.
pub const MINER_HULLS: [i64; 8] = crate::mine_ladder::ORDER;
/// The rank of a miner's hull in [`MINER_HULLS`]; `None` for any other.
pub fn miner_hull_rank(type_id: i64) -> Option<usize> {
    MINER_HULLS.iter().position(|&h| h == type_id)
}
/// `flagGeneralMiningHold` **[V]** (`inventorycommon/const`: 134): the
/// mining hold, where a Venture's ore goes first (the server's
/// `mining_hold`).
pub const FLAG_MINING_HOLD: i64 = 134;
/// A Miner I's type id **[V-data]**.
pub const MINER_I: i64 = 483;
/// The capsule's type id **[V-data]** (`service.rs::TYPE_CAPSULE`).
pub const CAPSULE: i64 = 670;
/// The Civilian Miner's type id **[V-data]**: the mining laser every rookie
/// ship carries (`content/econ/starter-fit.tsv`, E7), so a miner in its
/// rookie ship mines with it (R-ECON §2.4, "rookie miners mine with the
/// civilian miner").
pub const CIVILIAN_MINER: i64 = 3651;
/// How often the miner offers its goods at most (R-ECON §1.2, P3: one
/// batched sale, not one a visit). **[A]** about two hours: falsified by
/// `[save]` showing the miners' trade rows still dominate the writes (too
/// short), or by miners whose hangars outgrow the buyers' bids (too long).
pub const SELL_EVERY: Duration = Duration::from_secs(2 * 3600);

/// Is `type_id` a mining laser this bot mines with: a Miner I, or the
/// rookie ship's Civilian Miner.
pub fn is_mining_laser(type_id: i64) -> bool {
    type_id == MINER_I || type_id == CIVILIAN_MINER || type_id == crate::mine_ladder::STRIP_MINER_I
}
/// `const.categoryAsteroid` **[V-data]**: what counts as ore in the hold.
pub const CATEGORY_ASTEROID: i64 = 25;

/// Where to land when warping to a rock or to the boss, metres. **[A]**
pub const MINING_STANDOFF: f64 = 5_000.0;
/// The range a bot keeps from a rock it approaches, metres. **[A]**
pub const APPROACH_RANGE: f64 = 2_000.0;
/// Lock and mine only inside this fraction of the laser's range, so a
/// little drift does not turn into `TargetTooFar`. **[A]**
pub const RANGE_MARGIN: f64 = 0.9;
/// How many of the nearest rocks a bot chooses among, by its name's hash,
/// so N bots spread out. **[A]**
pub const SPREAD: usize = 5;
/// Consecutive failures of one kind before recovery. **[A]**
pub const GIVE_UP_AFTER: u32 = 3;

/// How long a bot leaves its fleet boss alone after a refused warp to him
/// (docked, or too close to warp to): it mines on its own meanwhile.
/// **[A]** falsifier: a boss who undocks and is not rejoined for a minute.
pub const BOSS_RETRY: Duration = Duration::from_secs(45);
/// How long to wait while something is moving or pending: short, because
/// the driver wakes on the first push anyway. **[A]**
pub const SHORT_WAIT: Duration = Duration::from_millis(500);
/// How long to wait while the laser runs; the driver wakes on the next push,
/// so this is a ceiling, not a period. **[A]**
pub const MINING_WAIT: Duration = Duration::from_secs(5);
/// How long to idle after saying `MISSING`. **[A]**
pub const MISSING_WAIT: Duration = Duration::from_secs(60);
/// A civilian yields to a non-fleet human whose ship is this close, surface
/// to surface, metres. **[A]** (C7 §6.3's 100 km): falsified by "the bots
/// act scripted" (too large) or "a bot mined my rock" (too small).
pub const COURTESY_RANGE: f64 = 100_000.0;

/// What a bot does with its time: a row of [`crate::roles::ROLES`].
pub type RoleRef = &'static RoleSpec;

/// **The pre-S11 spellings** `Role::Miner` and `Role::Escort`, kept so
/// callers (the harness tests among them) read unchanged. Each is a row of
/// [`crate::roles::ROLES`]; a new role needs no name here.
pub struct Role;

#[allow(non_upper_case_globals)]
impl Role {
    /// [`crate::roles::MINE`].
    pub const Miner: RoleRef = &roles::MINE;
    /// [`crate::roles::ESCORT`].
    pub const Escort: RoleRef = &roles::ESCORT;
}

/// Whose fleet invites to accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptFleet {
    /// Anybody's: a friend invites, the bot joins.
    Any,
    /// Only this character's, by name.
    From(String),
    /// Nobody's.
    Nobody,
    /// Its own corporation's or alliance's pilots (SOC-C7,
    /// [`crate::fleet::may_invite`]; the driver asks `GetPublicInfo`).
    Allied,
    /// FC-1: a wing bot's: its FC's invite (or the invite into the fleet it
    /// applied to through the finder, [`crate::wing::accepts`]).
    Fc(i64),
}

/// What kind of bot this is. Every field is **[A]**.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub role: RoleRef,
    /// May the bot give itself a skill, a hull and a laser with GM slash
    /// commands when its hangar lacks them? Setup only, never in the loop
    /// (`docs/lanes/bots.md` §2.1).
    pub gm_setup: bool,
    /// **The closed economy** (`eve-bots --closed`, E8, [`crate::acquire`]):
    /// a bot missing its ship, modules, rounds or skill gets them the
    /// player's way -- the rookie ship (`CreateNewbieShip`), skillbooks at
    /// the school, everything else from others' asks -- instead of saying
    /// `MISSING` (after GM setup, when [`Policy::gm_setup`] is also on:
    /// `--bootstrap`).
    pub closed: bool,
    /// The miner offers its goods at most once in this long
    /// ([`SELL_EVERY`]; zero: once a visit, the FX6 rule).
    pub sell_every: Duration,
    pub accept_fleet: AcceptFleet,
    /// On `TargetTooFar`, ask for a fresh state (`UpdateStateRequest`)
    /// before approaching again.
    pub resync: bool,
    /// How long to sit docked before undocking: a range, jittered per bot.
    pub dwell: (Duration, Duration),
    /// The same, after a trip that found the belt empty.
    pub idle_dwell: (Duration, Duration),
    /// The agent a courier hauls for (`eve-bots --agent ID`); `None`: the
    /// courier says `MISSING`.
    pub agent: Option<i64>,
    /// Does a civilian role step aside for humans ([`courtesy`])? **Off by
    /// default** (NAMES, S4 of the EVEJS-BOTS report: EvEJS's bots stay,
    /// share the belt and flee only when attacked, and the vanishing was the
    /// owner's churn); the rule stays behind this flag for a future
    /// belt-sharing rule. Tests that exercise it switch it on.
    pub courtesy: bool,
    /// Does a civilian role run from a player's lock or gun
    /// ([`crate::threat`])? On at a person's pace; off in [`Policy::fast`].
    pub threat: bool,
    /// The client's own data about each NPC type (`eve-bots --content DIR`,
    /// CP46); `None`: the escort picks by distance alone and never retreats
    /// on numbers ([`crate::escort`]).
    pub profiles: Option<std::sync::Arc<crate::profile::Profiles>>,
    /// The hostile bot's rules (`eve-bots --hostile`, CP56,
    /// [`crate::hostile::Rules`]); `None`: no bot may engage a human, and a
    /// `--loop hostile` bot says `MISSING` docked.
    pub hostile: Option<std::sync::Arc<crate::hostile::Rules>>,
    /// DOC-8: the traders' appetite at the run's difficulty
    /// ([`crate::difficulty::TraderKnobs`]); normal is today's constants.
    pub trader: crate::difficulty::TraderKnobs,
    /// BP-COUNTER: how well the bot keeps the operational counters
    /// (`KIT_DISCIPLINE`); level 0 keeps none.
    pub counter: crate::counter::CounterKnobs,
    /// `(typeID, units)` a builder buys per market cycle
    /// ([`crate::builder::wants_from`]); empty: it only lists its goods.
    pub wants: std::sync::Arc<Vec<(i64, i64)>>,
    /// The map a bot routes over (`eve-bots --celestials`/`--content`,
    /// [`crate::travel::Map`]); `None`: no trips, and what its station does
    /// not sell stays missing.
    pub travel: Option<std::sync::Arc<crate::travel::Map>>,
    /// The least security a route passes through (`eve-bots
    /// --security-floor`, [`crate::travel::SECURITY_FLOOR`]).
    pub security_floor: f64,
    /// Does the miner try an ore anomaly before a belt, once a trip
    /// ([`crate::ore::to_an_anomaly`], SOC-B10)? On at a person's pace; off
    /// in [`Policy::fast`], whose harness bots mine the fixture's belts.
    pub ore_anomalies: bool,
    /// FAST-KIT: a dev spawn (`/bots`) skips the docked dwell and undocks as
    /// soon as it is armed. Off for a person-paced bot.
    pub quick: bool,
    /// BRAWL (`eve-bots --brawl`, a fleet-fight bench): a hunter takes any
    /// odds, never breaks off, and closes on the nearest foe on the grid
    /// instead of patrolling ([`crate::hunt`]). Off in every ordinary run.
    pub brawl: bool,
    /// Does the miner stock, launch and recall mining drones
    /// ([`crate::ore`], SOC-B10)? On at a person's pace; off in
    /// [`Policy::fast`].
    pub mining_drones: bool,
    /// Do the fighters carry and use combat drones ([`crate::drone_use`],
    /// K-DRONE)? On at a person's pace; off in [`Policy::fast`], whose
    /// harness bots fight with their guns in a Rifter.
    pub combat_drones: bool,
    /// The hunter's doctrine (`eve-bots --society-pvp`, SOC-C5,
    /// [`crate::hunt::Ground`]); `None`: no bot hunts pilots, and a
    /// `--loop hunter` bot says `MISSING` docked.
    pub pvp: Option<std::sync::Arc<crate::hunt::Ground>>,
    /// The geode crate types and the SKIN licences of the content bundle
    /// ([`crate::geode_haul::Table`], L7 BOT-ECON); empty without `--content`,
    /// and then no bot opens a geode or buys a SKIN.
    pub econ: std::sync::Arc<crate::geode_haul::Table>,
    /// The other half of a capital pair, by character id (CAP-L,
    /// [`crate::capital`]): the cyno alt's pilot, or the pilot's alt. `None`:
    /// no pair, and a `capital` or `cyno-alt` bot says `MISSING` docked.
    pub partner: Option<i64>,
    /// The alliance's sov hall (SOC-D5, [`crate::sov_bot::SovHall`]); `None`:
    /// a `sov` bot says `MISSING` docked.
    pub sov: Option<std::sync::Arc<crate::sov_bot::SovHall>>,
    /// T3-SALVAGE: a floor under the hull's own difficulty cap for the
    /// J-space raider ([`crate::wh_raid::cap`]). `None` in play, where the
    /// hull alone decides; a test harness fixes it, since its hulls are not
    /// the combat ladder's.
    pub raid_cap: Option<i64>,
    /// DEV-MODE (`eve-bots --roster`, [`crate::dev`]): this bot's roster
    /// settings; `None` in every ordinary run.
    pub dev: Option<std::sync::Arc<crate::dev::DevBot>>,
    /// ROAM-GANG: this bot's seat in a gang ([`crate::roam_gang`]); `None`
    /// for every other bot.
    pub gang: Option<crate::roam_gang::Seat>,
    /// FC-1: this bot's seat in a wing that follows a human FC
    /// ([`crate::wing`]); `None` for every other bot.
    pub wing: Option<crate::wing::Seat>,
    /// REACT-JITTER (S6, [`crate::react`]): think before the first reaction
    /// to a new event. On for play; off for the fast pace of tests.
    pub react: bool,
}

impl Policy {
    /// Paced like a person: half a minute in station, longer after an empty
    /// belt. **[A]**, all of it.
    pub fn human() -> Policy {
        Policy {
            role: roles::default(),
            gm_setup: true,
            closed: false,
            sell_every: SELL_EVERY,
            accept_fleet: AcceptFleet::Any,
            resync: true,
            dwell: (Duration::from_secs(5), Duration::from_secs(40)),
            idle_dwell: (Duration::from_secs(60), Duration::from_secs(300)),
            agent: None,
            courtesy: false,
            threat: true,
            profiles: None,
            hostile: None,
            trader: crate::difficulty::TraderKnobs::NORMAL,
            counter: crate::counter::CounterKnobs::NORMAL,
            wants: std::sync::Arc::new(Vec::new()),
            travel: None,
            security_floor: crate::travel::SECURITY_FLOOR,
            ore_anomalies: true,
            quick: false,
            brawl: false,
            mining_drones: true,
            combat_drones: true,
            pvp: None,
            econ: std::sync::Arc::new(crate::geode_haul::Table::default()),
            partner: None,
            sov: None,
            raid_cap: None,
            dev: None,
            gang: None,
            wing: None,
            react: true,
        }
    }

    /// No dwell at all: for tests.
    pub fn fast() -> Policy {
        Policy {
            dwell: (Duration::ZERO, Duration::ZERO),
            idle_dwell: (Duration::ZERO, Duration::ZERO),
            courtesy: false,
            threat: false,
            react: false,
            sell_every: Duration::ZERO,
            ore_anomalies: false,
            mining_drones: false,
            combat_drones: false,
            ..Policy::human()
        }
    }
}

/// Why the bot is heading home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Homing {
    /// The laser stopped with a full hold.
    HoldFull,
    /// The park has no rock.
    NoRocks,
    /// Three failures of one kind in space.
    Recovery,
    /// No laser to mine with (a capsule in space, say), or no gun.
    Unarmed,
    /// The escort's armour or hull is hurt: docking is the repair
    /// (`combat::Combat` forgets a ship's damage when it leaves space).
    Repair,
    /// The escort's guns are empty and its hold holds no rounds.
    Rearm,
    /// A civilian stepping aside for a human (CP39): see the module header.
    Courtesy,
    /// The capacitor is drained by a neutralizer the ratter cannot kill
    /// (BOT-CAP, [`crate::cap::answer`]).
    Drained,
    /// A player locked or shot this ship, or a corp mate (THREAT,
    /// [`crate::threat`]).
    Threat,
    /// A geode is aboard and the pilot's risk trait says it is worth the
    /// trip to open it (L7 BOT-ECON, [`crate::geode_haul::should_dock`]).
    Geode,
    /// The bot's session is over (SOC-A2, `driver::Timetable`): it docks,
    /// then logs off.
    LogOff,
}

impl Memory {
    /// **Why this bot docks** (BOT-CHURN), for the act log: the homing
    /// reason, with its numbers when the decision recorded them.
    pub fn dock_reason(&self) -> String {
        match (&self.homing, &self.home_why) {
            (Some(h), Some((w, why))) if h == w => why.clone(),
            (Some(h), _) => format!("homing {h:?}"),
            (None, _) => "no homing reason set".to_owned(),
        }
    }
}

/// What the bot remembers between decisions: identity, policy, intentions.
/// **No world state** -- see the module header.
#[derive(Debug, Clone, PartialEq)]
pub struct Memory {
    pub name: String,
    /// `hash(name)`: the bot's own spread and jitter, the same every run.
    pub seed: u64,
    pub policy: Policy,
    rng: u64,
    /// The rock this bot means to mine.
    pub claim: Option<i64>,
    pub homing: Option<Homing>,
    /// Why the bot is homing, in words, for the act log (BOT-CHURN): set
    /// where [`Self::homing`] is, read by the driver's `Dock` note.
    pub home_why: Option<(Homing, String)>,
    /// When to undock, once decided.
    pub dwell_until: Option<Instant>,
    /// The last trip found no rock: dwell longer.
    pub idle_next: bool,
    /// The belt this trip is going to for rocks, while none is in the park.
    pub belt: Option<i64>,
    /// Decisions spent at that belt with no rock in the park yet.
    pub belt_waits: u32,
    /// Belts found empty, and when to forget it ([`DRY_FOR`]); `to_a_belt`
    /// leaves them alone while any other belt is left.
    pub dry: Vec<(i64, Instant)>,
    /// MINER-PATIENCE: a belt a defend flee came from (or a jammed lock gave
    /// up on), and when to forget it; `to_a_belt` and the jam's next belt
    /// leave it alone meanwhile.
    pub npc_avoid: Option<(i64, Instant)>,
    /// GM setup has been asked for on this connection.
    pub setup_done: bool,
    /// The newest [`Stop`] already acted on.
    pub stop_handled: u64,
    /// A `Deactivate` is out and its stop push has not come yet.
    pub stop_sent: bool,
    /// Fleet invites decided against, by fleet id, so they are not
    /// re-decided every turn.
    pub declined: Vec<i64>,
    /// The fleet already greeted in chat.
    pub greeted: Option<i64>,
    /// What the bot says in Local and fleet chat, and when (S1,
    /// [`crate::chatter`]).
    pub chatter: crate::chatter::State,
    /// The civilian's threat sense (THREAT, [`crate::threat`]).
    pub threat: crate::threat::State,
    /// The ratter's: when it began to fight a neut rat with a drained
    /// capacitor (BOT-CAP, [`crate::cap::answer`]).
    pub neut_fight: Option<Instant>,
    /// The events this bot is still thinking about, or has reacted to
    /// (REACT-JITTER, [`crate::react`]).
    pub react: crate::react::State,
    /// The escort's: the NPC it is fighting, and its name for the chat.
    pub target: Option<(i64, String)>,
    /// The escort's: wrecks already looted (or tried), so each is opened
    /// once.
    pub looted: Vec<i64>,
    /// The ratter's salvaging (X-SALV): attempts and wrecks let go.
    pub salvage: crate::salvage::State,
    /// The J-space raider's dive (T3-SALVAGE, [`crate::wh_raid`]).
    pub raid: crate::wh_raid::State,
    /// What the bot remembers about rigs (X-RIG, [`crate::rig::outfit`]).
    pub rig: crate::rig::State,
    /// The escort's: guns told to stop whose stop push has not come yet.
    pub ceasing: Vec<i64>,
    /// The escort's: no gun fires before this -- a reload in space locks the
    /// gun for its `reloadTime` (`modules::load_ammo`).
    pub reload_until: Option<Instant>,
    /// The escort's kills, this run.
    pub kills: u32,
    /// The miner's (and the ratter's, [`Act::SellLoot`]): this visit's
    /// goods have been offered ([`Act::Sell`]),
    /// so a stack nobody bids for is not offered again until the next.
    pub sold: bool,
    /// The builder's: no [`Act::Market`] before this.
    pub market_next: Option<Instant>,
    /// The miner's and the ratter's: when it last offered its goods
    /// ([`Policy::sell_every`]).
    pub sold_at: Option<Instant>,
    /// The closed economy's: no [`Act::Acquire`] before this
    /// ([`crate::acquire::ACQUIRE_EVERY`]).
    pub acquire_next: Option<Instant>,
    /// `MISSING` answers since the last [`Act::Acquire`] ([`acquire_if_due`]):
    /// enough of them bring the next acquire forward.
    pub missing_streak: u32,
    /// The clock of the turn being decided (`Observation::now`), for rules
    /// that are handed only the memory ([`gm_or_missing`]); `None` between
    /// turns, so it is never remembered.
    pub now: Option<Instant>,
    /// The miner's: this trip's ore-anomaly step ([`crate::ore`], SOC-B10).
    pub ore_site: crate::ore::SiteStep,
    /// The miner's: its mining drones in space, as the last
    /// [`crate::ore::OreAct`] left them (the driver's arm writes this).
    pub drones: Vec<i64>,
    /// The rock those drones were last ordered onto.
    pub drone_rock: Option<i64>,
    /// The drones have been tried this trip (launched, or found none).
    pub drones_tried: bool,
    /// The drone bay has been stocked this visit.
    pub drones_stocked: bool,
    /// The foreman's belt, fleet and invitations (PROG-MINE).
    pub foreman: crate::foreman::State,
    /// The miner's deep-belt deployment (PROG-NULL, [`crate::rare_ore`]).
    pub rare: crate::rare_ore::State,
    /// L7 BOT-ECON: the new economy's docked turn ([`crate::nes_use`]), the
    /// motherlodes this bot heard of and the one it was told of
    /// ([`crate::geode_haul`]).
    pub econ: crate::nes_use::State,
    pub heard: crate::geode_haul::Heard,
    /// L7b: the escort's job from the request board ([`crate::escort_board`]).
    pub job: crate::escort_board::Mind,
    pub told: crate::geode_haul::Told,
    /// The miner's compression tries (PROG-MINE, [`crate::ore::Compress`]).
    pub compress: crate::ore::Compress,
    /// The miner's drops into its fleet boss's fleet hangar (FLEET-HANGAR,
    /// [`crate::fleet_hangar`]).
    pub hangar_drop: crate::fleet_hangar::Drop,
    /// The fighter's combat drones ([`crate::drone_use`], K-DRONE).
    pub combat: crate::drone_use::Mind,
    /// The fleet this bot commands, if any (SOC-C7, [`crate::fleet::Command`]).
    pub fleet: crate::fleet::Command,
    /// The hunter's intentions (SOC-C5, [`crate::hunt::Hunt`]).
    pub hunt: crate::hunt::Hunt,
    /// A gang member's duty (ROAM-GANG, [`crate::roam_gang::Mind`]).
    pub gang: crate::roam_gang::Mind,
    /// A wing member's duty (FC-1, [`crate::wing::Mind`]).
    pub wing: crate::wing::Mind,
    /// A capital pair member's memory: the beacons its fleet holds and the
    /// pair's pace (CAP-L, [`crate::capital::Mind`]).
    pub capital: crate::capital::Mind,
    /// FE-TRADE: the trader's price memory, holdings and knobs.
    pub desk: crate::price_memory::Desk,
    /// A sov pilot's duty, post and sorties (SOC-D5, [`crate::sov_bot::Mind`]).
    pub sov: crate::sov_bot::Mind,
    /// A warp scramble being dealt with (BOT-SCRAM, [`crate::scram`]).
    pub scram: Option<crate::scram::Episode>,
    /// A warp bubble being burned out of (BP-HOLD, [`crate::warp_bubble`]).
    pub warp_bubble: Option<crate::warp_bubble::Episode>,
    /// The micro jump drive's plan, cooldown and the ship's speed (BP-KIT,
    /// [`crate::pvp_kit`]).
    pub kit: crate::pvp_kit::Mem,
    /// The dictor's probes and cooldowns (BP-CAMP, [`crate::camp`]).
    pub camp: crate::camp::Mem,
    /// The fleet boss's standing order (BOT-FLEET-ORDERS,
    /// [`crate::fleet_orders`]).
    pub order: Option<crate::fleet_orders::Order>,
    /// Orders heard so far: each gets the next number (the REACT-JITTER key).
    pub order_seq: u64,
    /// A fleet mate's call for reps, for the logi.
    pub heal: Option<crate::fleet_orders::HealCall>,
}

impl Memory {
    pub fn new(name: &str, policy: Policy) -> Memory {
        let seed = name_hash(name);
        Memory {
            name: name.to_owned(),
            seed,
            policy,
            rng: seed | 1,
            claim: None,
            homing: None,
            home_why: None,
            dwell_until: None,
            idle_next: false,
            belt: None,
            belt_waits: 0,
            dry: Vec::new(),
            npc_avoid: None,
            setup_done: false,
            stop_handled: 0,
            stop_sent: false,
            declined: Vec::new(),
            greeted: None,
            chatter: crate::chatter::State::new(seed),
            threat: crate::threat::State::default(),
            neut_fight: None,
            react: crate::react::State::default(),
            target: None,
            looted: Vec::new(),
            salvage: crate::salvage::State::default(),
            raid: crate::wh_raid::State::default(),
            foreman: crate::foreman::State::default(),
            rare: crate::rare_ore::State::default(),
            econ: crate::nes_use::State::default(),
            heard: crate::geode_haul::Heard::default(),
            job: crate::escort_board::Mind::default(),
            told: crate::geode_haul::Told::default(),
            compress: crate::ore::Compress::default(),
            hangar_drop: crate::fleet_hangar::Drop::default(),
            rig: crate::rig::State::default(),
            ceasing: Vec::new(),
            reload_until: None,
            kills: 0,
            sold: false,
            market_next: None,
            sold_at: None,
            acquire_next: None,
            missing_streak: 0,
            now: None,
            ore_site: crate::ore::SiteStep::Untried,
            drones: Vec::new(),
            drone_rock: None,
            drones_tried: false,
            drones_stocked: false,
            combat: crate::drone_use::Mind::default(),
            fleet: crate::fleet::Command::default(),
            hunt: crate::hunt::Hunt::default(),
            gang: crate::roam_gang::Mind::default(),
            wing: crate::wing::Mind::default(),
            capital: crate::capital::Mind::default(),
            desk: crate::price_memory::Desk::default(),
            sov: crate::sov_bot::Mind::default(),
            scram: None,
            warp_bubble: None,
            kit: crate::pvp_kit::Mem::default(),
            camp: crate::camp::Mem::default(),
            order: None,
            order_seq: 0,
            heal: None,
        }
    }

    /// A duration in `range`, from this bot's own stream.
    /// A draw in `[0, 1)` from this bot's own stream.
    pub(crate) fn draw(&mut self) -> f64 {
        unit(xorshift(&mut self.rng))
    }

    /// **The think gate** (REACT-JITTER): `Some(left)` while this bot is
    /// still thinking about the event `(class, key)`, `None` once it may
    /// act. The delay is drawn once, when the event is first seen. Always
    /// `None` when the policy has reaction jitter off.
    pub(crate) fn thinking(
        &mut self,
        class: crate::react::Class,
        key: i64,
        o: &Observation,
    ) -> Option<Duration> {
        if !self.policy.react {
            return None;
        }
        let under_fire = under_fire(o);
        let (seed, dil) = (self.seed, o.threat.diligence);
        let rng = &mut self.rng;
        self.react.gate(class, key, o.now, || {
            crate::react::think(seed, dil, rng, under_fire)
        })
    }

    pub(crate) fn jitter(&mut self, (lo, hi): (Duration, Duration)) -> Duration {
        let span = hi.saturating_sub(lo);
        if span.is_zero() {
            return lo;
        }
        lo + span.mul_f64(unit(xorshift(&mut self.rng)))
    }
}

/// A player is locking or shooting this ship, or its bars are hurt: the
/// one case in which a reaction is not delayed past [`crate::react::LO`].
pub(crate) fn under_fire(o: &Observation) -> bool {
    o.threat
        .foes
        .iter()
        .any(|f| f.cause >= crate::threat::Cause::Locked)
        || o.health
            .is_some_and(|h| h.shield < 1.0 || h.armour < 1.0 || h.hull < 1.0)
}

/// FNV-1a over the name: deterministic per bot, std-only.
pub fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// xorshift64: the bot's dice. Not cryptographic and does not need to be.
pub fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// A draw in `[0, 1)`.
pub fn unit(draw: u64) -> f64 {
    (draw >> 11) as f64 / (1_u64 << 53) as f64
}

/// Where the session says the bot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Docked {
        station: i64,
    },
    InSpace {
        system: i64,
        bound: bool,
    },
    /// Neither: mid-session-change, or not yet told.
    Transit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipInfo {
    pub ship_id: i64,
    /// `None` when nothing has said yet.
    pub type_id: Option<i64>,
}

/// The fitted mining laser.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModuleInfo {
    pub item_id: i64,
    pub flag_id: i64,
    pub online: bool,
    pub active: bool,
    pub target: Option<i64>,
    /// `maxRange` (attribute 54), metres.
    pub range: f64,
}

/// One fitted gun.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GunInfo {
    pub item_id: i64,
    pub type_id: i64,
    pub flag_id: i64,
    pub online: bool,
    pub active: bool,
    pub target: Option<i64>,
    /// Rounds loaded, when known.
    pub rounds: Option<i64>,
    /// How many rounds it holds of the bot's ammo (`modules::rounds_capacity`).
    pub capacity: i64,
    /// The range to fight at, metres: optimal plus half the falloff, from
    /// the module's own dogma row, else the bundle, else the invented
    /// fixture (FX11, [`crate::modules::gun_range`]). **[A]** as a policy;
    /// the numbers are [V-data].
    pub range: f64,
    /// Its `reloadTime` (1795): a reload in space locks it this long
    /// (`modules::load_ammo`), from the same sources as `range`
    /// ([`crate::modules::reload_time`]).
    pub reload: std::time::Duration,
    /// What the module does (BP-EWAR-AIM), from its effect id.
    pub kind: GunKind,
    /// How far it reaches, metres: a weapon's `range`; an ewar module's
    /// optimal plus its `falloffEffectiveness` (2044). **[A]** as a policy.
    pub reach: f64,
}

/// What a fitted target module is (BP-EWAR-AIM): the effect id it runs
/// says it ([`GunKind::of_effect`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GunKind {
    /// A turret or a launcher: the damage.
    Weapon,
    /// A warp disruptor (stops the warp, blocks nothing else).
    Point,
    /// A warp scrambler (also blocks the MWD and the MJD).
    Scram,
    /// A stasis webifier.
    Web,
    Ewar(EwarKind),
    /// A neutralizer or a nosferatu.
    Neut,
}

/// The electronic warfare a module does besides tackle and neuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EwarKind {
    Ecm,
    Damp,
    Paint,
    /// A tracking disruptor: the target's turrets.
    Track,
    /// A guidance disruptor: the target's missiles.
    Guide,
}

/// `warpDisrupt` and the focused disruption script **[V]** (`tackle.rs`, which the
/// server keeps private).
const EFFECT_DISRUPT: i64 = 39;
const EFFECT_FOCUSED_DISRUPT: i64 = 6849;
/// The player scrambler and the focused scrambling script **[V]**.
const EFFECT_SCRAMBLE: i64 = 5934;
const EFFECT_FOCUSED_SCRAMBLE: i64 = 6848;

impl GunKind {
    /// The kind a module with this default effect is; any effect that is
    /// not tackle or ewar is a [`GunKind::Weapon`] (the old rule).
    pub fn of_effect(effect_id: i64) -> GunKind {
        use eve_server::ewar::Kind;
        match effect_id {
            EFFECT_DISRUPT | EFFECT_FOCUSED_DISRUPT => GunKind::Point,
            EFFECT_SCRAMBLE | EFFECT_FOCUSED_SCRAMBLE => GunKind::Scram,
            id => match Kind::of(id) {
                Some(Kind::Web) => GunKind::Web,
                Some(Kind::Ecm) => GunKind::Ewar(EwarKind::Ecm),
                Some(Kind::Damp) => GunKind::Ewar(EwarKind::Damp),
                Some(Kind::Paint) => GunKind::Ewar(EwarKind::Paint),
                Some(Kind::Tracking) => GunKind::Ewar(EwarKind::Track),
                Some(Kind::Guidance) => GunKind::Ewar(EwarKind::Guide),
                // A nosferatu is aimed like a neut. **[A]**
                Some(Kind::Neut | Kind::Nos) => GunKind::Neut,
                _ => GunKind::Weapon,
            },
        }
    }

    /// The kind of module that sent an `OnEwarStart` with this
    /// `jammingType`; `None` for help effects and unknown names.
    pub fn of_jamming(jamming: &str) -> Option<GunKind> {
        Some(match jamming {
            "warpScrambler" => GunKind::Point,
            "warpScramblerMWD" => GunKind::Scram,
            "webify" => GunKind::Web,
            "electronic" => GunKind::Ewar(EwarKind::Ecm),
            "ewRemoteSensorDamp" => GunKind::Ewar(EwarKind::Damp),
            "ewTargetPaint" => GunKind::Ewar(EwarKind::Paint),
            "ewTrackingDisrupt" => GunKind::Ewar(EwarKind::Track),
            "ewGuidanceDisrupt" => GunKind::Ewar(EwarKind::Guide),
            "ewEnergyNeut" | "ewEnergyVampire" => GunKind::Neut,
            _ => return None,
        })
    }

    /// Does it hold the target back (a point, a scrambler or a web)?
    pub fn is_tackle(self) -> bool {
        matches!(self, GunKind::Point | GunKind::Scram | GunKind::Web)
    }
}

/// The ship's own bars, fractions in `[0, 1]`, from `OnDamageStateChange`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Health {
    pub shield: f64,
    pub armour: f64,
    pub hull: f64,
}

/// How long a bot tries a jammed lock again before it leaves the belt for
/// the next one **[A]** (EvEJS: 90 s). Falsified by miners idling under ECM
/// for minutes (shorten) or leaving belts over a jam that was about to end.
pub const JAM_PATIENCE: Duration = Duration::from_secs(90);
/// The gap between two tries of a lock the jam refused **[A]**.
pub const JAM_RETRY: Duration = Duration::from_secs(5);
/// A belt given up over a jam is left alone this long **[A]**.
pub const JAM_AVOID: Duration = Duration::from_secs(5 * 60);
/// A belt a defend flee came from is left alone this long **[A]**: the
/// rats that drove the bot out are likely still there. Falsified by miners
/// that never return to a belt that has cleared (shorten) or that flee the
/// same belt twice in a row (lengthen).
pub const NPC_AVOID: Duration = Duration::from_secs(20 * 60);
/// The refusal keys of a lock refused for ECM: the key of the ECM gate
/// (`TargetingSystemsBeingJammed`, ewar-icons) and the key the gate gave
/// before it, a zero limit answered as a full target list **[V]**
/// (`targeting::lock`).
pub const JAM_KEYS: [&str; 2] = [
    "TargetingSystemsBeingJammed",
    "TargetingSystemsAlreadyFullyUtilized",
];

/// Whether a lock refusal is the jam's. The old key is the same text a full
/// target list gives, so it counts only when the bot holds no lock
/// (`held == 0`): a full list needs locks. The new key is the jam always.
pub fn jam_refusal(why: &str, held: usize) -> bool {
    why.contains(JAM_KEYS[0]) || (held == 0 && why.contains(JAM_KEYS[1]))
}

/// Whether a lock refusal says the target left (WARP-LOCK): the server
/// refuses a lock on a ship in active warp with `TargetingAttemptCancelled`.
/// That is a fact about the target, not a failure of the bot: it must not
/// count toward Recovery.
pub fn target_left_refusal(why: &str) -> bool {
    why.contains("TargetingAttemptCancelled")
}

/// A jammed lock: when the first refusal came and the latest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockJam {
    pub since: Instant,
    pub last: Instant,
}

impl LockJam {
    /// What to do about the jam at `now` (pure).
    pub fn verdict(&self, now: Instant) -> JamStep {
        if now.saturating_duration_since(self.since) >= JAM_PATIENCE {
            JamStep::NextBelt
        } else if now.saturating_duration_since(self.last) < JAM_RETRY {
            JamStep::Wait(JAM_RETRY - now.saturating_duration_since(self.last))
        } else {
            JamStep::Retry
        }
    }
}

/// [`LockJam::verdict`]'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JamStep {
    Wait(Duration),
    Retry,
    NextBelt,
}

/// Whether `belt` may be gone to at `now`: not the one the threat sense
/// struck at, nor one on the avoid list, unless it is the only belt (pure).
pub fn belt_allowed(
    threat_avoid: Option<i64>,
    npc_avoid: Option<(i64, Instant)>,
    belt: i64,
    now: Instant,
    belts: usize,
) -> bool {
    belts < 2
        || (threat_avoid != Some(belt)
            && !npc_avoid.is_some_and(|(b, until)| b == belt && now < until))
}

/// An NPC in the park.
#[derive(Debug, Clone, PartialEq)]
pub struct Hostile {
    pub ball: Ball,
    pub type_id: i64,
    pub name: String,
    /// The ship its gun is on, from its `OnSpecialFX` start.
    pub shooting: Option<i64>,
}

/// One ball, as far as a decision needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ball {
    pub id: i64,
    pub position: [f64; 3],
    pub radius: f64,
}

impl Ball {
    /// Centre distance minus both radii: the client's `surfaceDist`.
    pub fn surface_to(&self, other: &Ball) -> f64 {
        let d = (0..3)
            .map(|i| (self.position[i] - other.position[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        d - self.radius - other.radius
    }
}

/// The park, cut down to what the loop reads.
#[derive(Debug, Clone, PartialEq)]
pub struct ParkView {
    pub ego: Ball,
    pub ego_warping: bool,
    pub rocks: Vec<Ball>,
    /// The asteroid belts' own balls. **Global** (GRID-B): in every park in
    /// the system, while a belt's rocks are only in the parks on its grid --
    /// so a bot at the station sees belts and no rock, and warps to a belt
    /// to find some ([`to_a_belt`]).
    pub belts: Vec<Ball>,
    /// The home station's ball, when it is in the park.
    pub station: Option<Ball>,
    /// The fleet boss's ship, when it is in the park.
    pub boss: Option<Ball>,
    /// Rocks under another ship's laser: `rock -> that ship's id`, from
    /// `OnSpecialFX`.
    pub taken: BTreeMap<i64, i64>,
    /// NPCs: what the escort fights.
    pub hostiles: Vec<Hostile>,
    /// NPC wrecks: what the escort loots.
    pub wrecks: Vec<Ball>,
    /// What the ship is following or orbiting, from its own mode.
    pub ego_tether: Option<i64>,
}

/// Why the laser stopped, from `OnGodmaShipEffect`'s `error[0]`
/// (`mining.md` §1.4-1.5) **[V]**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Depleted,
    HoldFull,
    TooFar,
    /// Stopped with no error: asked to, or its target went.
    Stopped,
    Other(String),
}

impl StopReason {
    /// From a stop push's or a refusal's key.
    pub fn from_key(key: Option<&str>) -> StopReason {
        match key {
            None => StopReason::Stopped,
            Some("MiningItemDepleted") => StopReason::Depleted,
            Some(
                "MiningDronesDeactivatedCargoHoldFull"
                | "MiningDronesDeactivatedCargoHoldNowFull"
                // An `Activate` refused for want of room for one unit
                // (`mining::check_activation`): a player goes home. [A]
                | "NotEnoughCargoSpace",
            ) => StopReason::HoldFull,
            Some("TargetTooFar") => StopReason::TooFar,
            Some(other) => StopReason::Other(other.to_owned()),
        }
    }
}

/// One stop, numbered so it is acted on once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub seq: u64,
    pub reason: StopReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub fleet_id: i64,
    pub inviter_id: i64,
    /// From the select screen's list, when the inviter is on it.
    pub inviter_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FleetView {
    pub fleet_id: i64,
    /// **[A]** the inviter: the bot follows whoever invited it. The fleet
    /// window's boss is not read.
    pub boss: Option<i64>,
}

/// Everything the bot can see this turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub now: Instant,
    pub place: Place,
    pub ship: Option<ShipInfo>,
    /// The station hangar, when docked and listed.
    pub hangar: Vec<ItemRow>,
    /// The active ship's hold (flag 5).
    pub cargo: Vec<ItemRow>,
    /// The active ship's mining hold ([`FLAG_MINING_HOLD`], GAPS-B): empty
    /// for a hull without one.
    pub ore_hold: Vec<ItemRow>,
    pub miner: Option<ModuleInfo>,
    /// The fitted salvager (X-SALV, [`crate::salvage::read`]).
    pub salvager: Option<ModuleInfo>,
    /// The bot is in a J-space system (T3-SALVAGE; the driver reads the
    /// session's system).
    pub j_space: bool,
    /// The logi's remote repairers, the park's bars and its wards
    /// (K-LOGI, [`crate::logi`]).
    pub logi: crate::logi::Scene,
    /// `None` until bound and stated.
    pub park: Option<ParkView>,
    /// Targets locked, and locks pending.
    pub locked: Vec<i64>,
    pub pending: Vec<i64>,
    pub last_stop: Option<Stop>,
    pub invite: Option<Invite>,
    pub fleet: Option<FleetView>,
    /// The station to go home to: the last one docked in.
    pub home: Option<i64>,
    /// Consecutive failures, per kind.
    pub failures: BTreeMap<ActKind, u32>,
    /// MINER-PATIENCE: a lock was refused because the ship is jammed
    /// ([`jam_refusal`]); the driver sets it, a lock that works clears it.
    pub lock_jam: Option<LockJam>,
    /// A warp to the fleet boss was refused (he is docked, or within the
    /// minimum warp of us but off this grid): do not try again before this
    /// instant, work on our own meanwhile. Never a failure (BOT-REDOCK).
    pub boss_unreachable_until: Option<Instant>,
    /// The escort's guns.
    pub guns: Vec<GunInfo>,
    /// The ship's bars; `None` until something hurt it (full).
    pub health: Option<Health>,
    /// The hull's `maxTargetRange`, metres.
    pub lock_range: f64,
    /// `maxLockedTargets` (192); 0 until read (BP-EWAR-AIM).
    pub lock_max: usize,
    /// A chat connection is up: the bot may say things.
    pub chat: bool,
    /// This pilot's character id (0 until the session names it): the key an
    /// escort's job is held under ([`crate::escort_board`]).
    pub me: i64,
    /// The last loot was refused for room: the hold takes no more.
    pub hold_full: bool,
    /// Other pilots' ships in the park that are **humans outside the
    /// bot's fleet**: not NPCs, not the bot marker's, not the fleet boss
    /// (the driver's `is_human`).
    pub humans: Vec<Ball>,
    /// The hull type of each of [`Self::humans`], by ship id, from its slim
    /// (CP56: the hostile bot's policy is by the target's class).
    pub human_types: BTreeMap<i64, i64>,
    /// The primary the fleet boss called (SOC-C7, [`crate::fleet::primary_call`]).
    pub primary: Option<i64>,
    /// R-REL-3: a friend's `NeedBackup` this bot will answer -- the
    /// character to warp to (the driver's [`crate::effects::help_decision`]).
    pub help: Option<i64>,
    /// What the room said and who came in (S1, [`crate::chatter::Scene`]);
    /// empty unless [`Self::chat`].
    pub talk: crate::chatter::Scene,
    /// Who locks or shoots this ship, and who is in Local (THREAT).
    pub threat: crate::threat::Scene,
    /// The capacitor, the hold and the neuts on this ship (BOT-CAP).
    pub cap: crate::cap::Scene,
    /// Who warp-scrambles this ship (BOT-SCRAM).
    pub scram: crate::scram::View,
    /// Whether a warp bubble holds this ship, and its spheres (BP-HOLD).
    pub warp_bubble: crate::warp_bubble::View,
    /// The fitted smartbombs and micro jump drive (BP-KIT).
    pub kit: crate::pvp_kit::Kit,
    /// The enemy utility seen this turn: bombers and drives spooling
    /// (BP-COUNTER).
    pub counter: crate::counter::Seen,
    /// What the hunter sees: the pilots on the grid and the last
    /// directional scan (SOC-C5, [`crate::hunt::Scene`]).
    pub hunt: crate::hunt::Scene,
    /// Where the wing's FC is (FC-1, [`crate::wing::Scene`]).
    pub wing: crate::wing::Scene,
}

impl Observation {
    /// Nothing known yet, at `now`.
    pub fn empty(now: Instant) -> Observation {
        Observation {
            now,
            place: Place::Transit,
            ship: None,
            hangar: Vec::new(),
            cargo: Vec::new(),
            ore_hold: Vec::new(),
            miner: None,
            salvager: None,
            j_space: false,
            logi: crate::logi::Scene::default(),
            park: None,
            locked: Vec::new(),
            pending: Vec::new(),
            last_stop: None,
            invite: None,
            fleet: None,
            home: None,
            failures: BTreeMap::new(),
            lock_jam: None,
            boss_unreachable_until: None,
            guns: Vec::new(),
            health: None,
            lock_range: 0.0,
            lock_max: 0,
            chat: false,
            me: 0,
            hold_full: false,
            humans: Vec::new(),
            human_types: BTreeMap::new(),
            primary: None,
            help: None,
            talk: crate::chatter::Scene::default(),
            threat: crate::threat::Scene::default(),
            cap: crate::cap::Scene::default(),
            scram: crate::scram::View::default(),
            warp_bubble: crate::warp_bubble::View::default(),
            kit: crate::pvp_kit::Kit::default(),
            counter: crate::counter::Seen::default(),
            hunt: crate::hunt::Scene::default(),
            wing: crate::wing::Scene::default(),
        }
    }

    pub(crate) fn failed(&self, kind: ActKind) -> u32 {
        self.failures.get(&kind).copied().unwrap_or(0)
    }
}

/// One thing to do. Each is one verb (`verbs.rs`) or a wait.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// GM: the role's skills, and a hull, its module(s) and its ammo when
    /// `ship`/`module`/`ammo` say so.
    GmSetup {
        ship: bool,
        module: bool,
        ammo: bool,
    },
    /// The closed economy's setup (E8): the role's skill from a school
    /// book, the rookie ship for a pilot in a capsule, and the hull, modules
    /// and rounds `ship`/`module`/`ammo` say are lacking from others' asks:
    /// one [`crate::acquire::run`], docked.
    Acquire {
        ship: bool,
        module: bool,
        ammo: bool,
    },
    Board(i64),
    FitAuto(i64),
    /// FAST-KIT: `n` units of one stack (or one item `n` times) in one
    /// `MultiAdd([item; n])`; the server splits one unit off per id.
    FitAutoN {
        item: i64,
        n: usize,
    },
    Online(i64),
    /// FAST-KIT: every listed module online, in one turn.
    OnlineMany(Vec<i64>),
    Undock,
    Bind,
    WarpToItem {
        id: i64,
        range: f64,
    },
    WarpToMember {
        char_id: i64,
        range: f64,
    },
    Follow {
        id: i64,
        range: f64,
    },
    Lock(i64),
    Unlock(i64),
    Mine {
        module: i64,
        rock: i64,
    },
    StopMining(i64),
    Dock(i64),
    /// `(itemID, quantity)` per stack.
    Unload(Vec<(i64, i64)>),
    /// Orbit a ball at a range: the escort's fighting stance.
    Orbit {
        id: i64,
        range: f64,
    },
    /// One gun on one target.
    Fire {
        gun: i64,
        target: i64,
    },
    CeaseFire(i64),
    /// `(gunID, flagID)` per gun, the stacks to load from, and where they
    /// are (the station's hangar, or the ship's hold).
    LoadAmmo {
        guns: Vec<(i64, i64)>,
        charges: Vec<i64>,
        location: i64,
    },
    /// Hangar -> hold: spare rounds for a reload in space.
    Stock {
        item: i64,
        quantity: i64,
    },
    /// Loot All from a wreck.
    Loot(i64),
    /// One salvager attempt on one locked wreck (X-SALV,
    /// [`crate::salvage::run`]).
    /// The logi's own act: run a remote repairer on a ward, or stop it
    /// (K-LOGI, [`crate::logi::execute`]).
    Logi(crate::logi::LogiAct),
    Salvage {
        module: i64,
        wreck: i64,
    },
    /// A line in fleet chat, or Local outside a fleet.
    Say(String),
    AcceptInvite {
        fleet: i64,
    },
    /// Take a courier mission from `agent`, haul it and turn it in: one
    /// [`crate::courier::run`], docked.
    Courier {
        agent: i64,
    },
    /// Read the Agency, pick a combat anomaly and warp to it: one
    /// [`crate::site_runner::scout`], in space.
    Sites,
    /// One gas trip, dock to dock: one [`crate::huffer::run`], docked
    /// (SOC-B2).
    Huff,
    /// One gas dive into wormhole space, dock to dock: one
    /// [`crate::wh_dive::run_gas`], docked (Y-WH).
    Dive,
    /// The J-space raider's step: dive in, sweep a guarded relic or data
    /// site, or come out (T3-SALVAGE, [`crate::wh_raid::act`]), in space.
    Raid(crate::wh_raid::Op),
    /// One ice trip, dock to dock: find a field, harvest, home, refine and
    /// sell: one [`crate::ice::run`], docked (SOC-B1).
    Ice,
    /// The miner's ore anomalies and mining drones (SOC-B10): one
    /// [`crate::ore::act`], docked or in space.
    Ore(crate::ore::OreAct),
    /// The fighters' combat drones (K-DRONE): one [`crate::drone_use::act`],
    /// docked or in space.
    Drone(crate::drone_use::DroneAct),
    /// Run an agent's missions: one [`crate::missioner::run`] -- docked,
    /// the agents' desk; in space, the kill under way. `level` is the
    /// ship's ([`crate::missioner::ship_level`]), 0 in space.
    Mission {
        agent: Option<i64>,
        level: i64,
    },
    /// Docked, fit a probe launcher and load probes; in space, scan down a
    /// signature, warp to it and save it: one [`crate::probe::explore`]
    /// (SOC-B3).
    Explore,
    /// Enlist if need be, then warp to an enemy FW complex here or fly to
    /// the next front system: one [`crate::plexer::sortie`] (SOC-B9).
    Plex,
    /// Docked, tend the colonies (restart, launch), sell the commodities
    /// and found a colony when the skill allows: one [`crate::pi::run`]
    /// (SOC-B6).
    Pi,
    /// Docked, sell the loot, outfit, find a data or relic site, hack its
    /// cans, loot, home: one [`crate::hacker::run`] (HK4).
    Hack,
    /// Refine the hangar's ore and sell its goods into bids at the floor or
    /// better: one [`crate::builder::sell_goods`], docked (FX6).
    Sell,
    /// Sell the hangar's loot into bids, list or refine the rest, keeping
    /// the rounds the ratter flies with: one [`crate::escort::sell_loot`],
    /// docked (E10b).
    SellLoot,
    /// One builder cycle: list the hangar's goods, buy the policy's
    /// [`Policy::wants`] inside the demand budget: one
    /// [`crate::builder::run`], docked (FX6).
    Market,
    /// One trader cycle: the best arbitrage or courier trip, flown, and
    /// home: one [`crate::trader::run`], docked (SOC-B8).
    Trade,
    /// One turn of a capital pair member: the pilot's stock-and-jump, or the
    /// alt's fleet, ozone and leg: one [`crate::capital::turn`] (CAP-L).
    Capital,
    /// One foreman turn: the mining burst and compressor outfit, the belt,
    /// the fleet and the boost: one [`crate::foreman::turn`] (PROG-MINE).
    Foreman,
    /// One rare-ore deployment turn: out to a deep belt, or the stock home:
    /// one [`crate::rare_ore::turn`] (PROG-NULL).
    Rare,
    /// One docked turn of the new economy: open geodes, wear a SKIN, inject
    /// or extract skill points, sell surplus PLEX: one [`crate::nes_use::turn`]
    /// (L7 BOT-ECON).
    Econ,
    /// One sov turn: the alliance's plan, then this pilot's duty flown: one
    /// [`crate::sov_bot::turn`] (SOC-D5).
    Sov,
    /// One freighter cycle: the best bulk trade over the gates, flown in a
    /// freighter the bot owns: one [`crate::hauler::run`] (CAP-L).
    Haul,
    /// One fleet commander's order (SOC-C7): one [`crate::fleet::execute`].
    Fleet(crate::fleet::Order),
    /// The hunter's own: a directional scan, a roam to another system, or
    /// reading the killmails into the society (SOC-C5,
    /// [`crate::hunt::HuntAct`]).
    Hunt(crate::hunt::HuntAct),
    /// A smartbomb cycle or a micro jump ([`crate::pvp_kit`], BP-KIT).
    Kit(crate::pvp_kit::KitAct),
    /// The gang's own act: fly to a system ([`crate::roam_gang::GangAct`]).
    Gang(crate::roam_gang::GangAct),
    /// The wing's own act: apply to the FC's fleet advert, or dock where he
    /// docked ([`crate::wing::WingAct`]).
    Wing(crate::wing::WingAct),
    /// A Tech III destroyer's stance for the fight (PROG-PVE2): the driver
    /// asks [`crate::t3::stance_wanted`] for the hull's pick and sends
    /// [`crate::t3::set_stance`] when it differs from the one set. `hurt` is
    /// the share of shield and armour lost, `range` the distance to the
    /// target, `reach` the guns' fighting range.
    Stance {
        hurt: f64,
        range: Option<f64>,
        reach: f64,
    },
    /// Point the ship at a ball without warping (`CmdAlignTo`, BOT-SCRAM).
    AlignTo(i64),
    /// Stop the ship (`CmdStop`): a Hold order's first act (BOT-FLEET-ORDERS).
    Stop,
    /// Fly to the first station of a solar system, over the gates, without
    /// rehoming (a fleet's `TravelTo`; the driver's stranded way, with the
    /// PULL-LEASE `lend` hook).
    TravelTo {
        system: i64,
    },
    /// Move toward a stargate and jump it (a fleet's `JumpTo`): one step per
    /// turn, the driver's.
    JumpGate {
        gate: i64,
    },
    Resync,
    Wait(Duration),
    /// Cannot do this the player way: say so and idle. The line is a
    /// feature request (`docs/lanes/bots.md` §2.1).
    Missing(&'static str),
    Reconnect,
}

/// An [`Act`]'s kind, for failure counts and the act log -- plus the
/// driver's own entries, which the brain never emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActKind {
    GmSetup,
    Acquire,
    Board,
    FitAuto,
    Online,
    Undock,
    Bind,
    WarpToItem,
    WarpToMember,
    Follow,
    Lock,
    Unlock,
    Mine,
    StopMining,
    Dock,
    Unload,
    Orbit,
    Fire,
    CeaseFire,
    LoadAmmo,
    Stock,
    Loot,
    Salvage,
    Logi,
    Say,
    AcceptInvite,
    Courier,
    Sites,
    Huff,
    Dive,
    Raid,
    Ice,
    Ore,
    Drone,
    Mission,
    Explore,
    Plex,
    Pi,
    Hack,
    Sell,
    SellLoot,
    Market,
    Trade,
    Capital,
    Foreman,
    Sov,
    Haul,
    Fleet,
    Hunt,
    Kit,
    Gang,
    Wing,
    Stance,
    Rare,
    Econ,
    Align,
    /// A fleet boss's order carried out ([`crate::fleet_orders`]).
    Order,
    Resync,
    Wait,
    Missing,
    Reconnect,
    /// Driver: one GM slash command, inside `GmSetup`.
    Slash,
    /// Driver: connected, logged in, selected.
    Connect,
    /// Driver: the observation rebuilt from the server.
    Observe,
    /// Driver: the connection failed.
    Lost,
    /// Driver: the chat connection, up or not.
    Chat,
    /// Driver: a career decision at a docked decision point (SOC-A2).
    Career,
    /// Driver: a session's end or a wait for the next (SOC-A2).
    Session,
    /// Driver: the trip home after an errand that failed mid-way (SOC-A2).
    Travel,
    /// Driver: a social turn -- corp, alliance, contacts (SOC-C4).
    Corp,
}

impl ActKind {
    pub fn name(self) -> &'static str {
        match self {
            ActKind::GmSetup => "gm-setup",
            ActKind::Acquire => "acquire",
            ActKind::Board => "board",
            ActKind::FitAuto => "fit",
            ActKind::Online => "online",
            ActKind::Undock => "undock",
            ActKind::Bind => "bind",
            ActKind::WarpToItem => "warp",
            ActKind::WarpToMember => "warp-member",
            ActKind::Follow => "approach",
            ActKind::Lock => "lock",
            ActKind::Unlock => "unlock",
            ActKind::Mine => "mine",
            ActKind::StopMining => "stop-mining",
            ActKind::Dock => "dock",
            ActKind::Unload => "unload",
            ActKind::Orbit => "orbit",
            ActKind::Fire => "fire",
            ActKind::CeaseFire => "cease-fire",
            ActKind::LoadAmmo => "load-ammo",
            ActKind::Stock => "stock",
            ActKind::Loot => "loot",
            ActKind::Salvage => "salvage",
            ActKind::Logi => "logi",
            ActKind::Say => "say",
            ActKind::AcceptInvite => "join-fleet",
            ActKind::Courier => "courier",
            ActKind::Sites => "sites",
            ActKind::Huff => "huff",
            ActKind::Dive => "dive",
            ActKind::Raid => "raid",
            ActKind::Ice => "ice",
            ActKind::Ore => "ore",
            ActKind::Drone => "drone",
            ActKind::Mission => "mission",
            ActKind::Explore => "explore",
            ActKind::Plex => "plex",
            ActKind::Pi => "pi",
            ActKind::Hack => "hack",
            ActKind::Sell => "sell",
            ActKind::SellLoot => "sell-loot",
            ActKind::Market => "market",
            ActKind::Trade => "trade",
            ActKind::Capital => "capital",
            ActKind::Foreman => "foreman",
            ActKind::Sov => "sov",
            ActKind::Haul => "haul",
            ActKind::Fleet => "fleet",
            ActKind::Hunt => "hunt",
            ActKind::Kit => "kit",
            ActKind::Gang => "gang",
            ActKind::Wing => "wing",
            ActKind::Stance => "stance",
            ActKind::Rare => "rare",
            ActKind::Econ => "econ",
            ActKind::Align => "align",
            ActKind::Order => "order",
            ActKind::Resync => "resync",
            ActKind::Wait => "wait",
            ActKind::Missing => "MISSING",
            ActKind::Reconnect => "reconnect",
            ActKind::Slash => "slash",
            ActKind::Connect => "connect",
            ActKind::Observe => "observe",
            ActKind::Lost => "lost",
            ActKind::Chat => "chat",
            ActKind::Career => "career",
            ActKind::Session => "session",
            ActKind::Travel => "travel",
            ActKind::Corp => "corp",
        }
    }

    /// The acts that move the ship toward somewhere: when these keep
    /// failing, going home cannot be the cure.
    pub(crate) fn is_travel(self) -> bool {
        matches!(
            self,
            ActKind::WarpToItem | ActKind::Follow | ActKind::Orbit | ActKind::Dock | ActKind::Bind
        )
    }

    /// Acts whose failure is not counted: a line not said, a wreck not
    /// looted, goods not sold (FX6: the miner tries once a visit anyway),
    /// an ore anomaly not found or drones not launched (SOC-B10: the laser
    /// and the belts go on). None is a reason to go home or reconnect.
    /// **[A]**
    pub fn is_incidental(self) -> bool {
        matches!(
            self,
            ActKind::Say
                | ActKind::Loot
                | ActKind::Salvage
                | ActKind::Logi
                | ActKind::Sell
                | ActKind::SellLoot
                | ActKind::Acquire
                | ActKind::Ore
                | ActKind::Hunt
                | ActKind::Kit
                | ActKind::Gang
                | ActKind::Wing
                | ActKind::Drone
                | ActKind::Stance
                | ActKind::Align
                | ActKind::Order
                | ActKind::WarpToMember
        )
    }
}

impl Act {
    pub fn kind(&self) -> ActKind {
        match self {
            Act::GmSetup { .. } => ActKind::GmSetup,
            Act::Acquire { .. } => ActKind::Acquire,
            Act::Board(_) => ActKind::Board,
            Act::FitAuto(_) | Act::FitAutoN { .. } => ActKind::FitAuto,
            Act::Online(_) | Act::OnlineMany(_) => ActKind::Online,
            Act::Undock => ActKind::Undock,
            Act::Bind => ActKind::Bind,
            Act::WarpToItem { .. } => ActKind::WarpToItem,
            Act::WarpToMember { .. } => ActKind::WarpToMember,
            Act::Follow { .. } => ActKind::Follow,
            Act::Lock(_) => ActKind::Lock,
            Act::Unlock(_) => ActKind::Unlock,
            Act::Mine { .. } => ActKind::Mine,
            Act::StopMining(_) => ActKind::StopMining,
            Act::Dock(_) => ActKind::Dock,
            Act::Unload(_) => ActKind::Unload,
            Act::Orbit { .. } => ActKind::Orbit,
            Act::Fire { .. } => ActKind::Fire,
            Act::CeaseFire(_) => ActKind::CeaseFire,
            Act::LoadAmmo { .. } => ActKind::LoadAmmo,
            Act::Stock { .. } => ActKind::Stock,
            Act::Loot(_) => ActKind::Loot,
            Act::Salvage { .. } => ActKind::Salvage,
            Act::Logi(_) => ActKind::Logi,
            Act::Say(_) => ActKind::Say,
            Act::AcceptInvite { .. } => ActKind::AcceptInvite,
            Act::Courier { .. } => ActKind::Courier,
            Act::Sites => ActKind::Sites,
            Act::Huff => ActKind::Huff,
            Act::Dive => ActKind::Dive,
            Act::Raid(_) => ActKind::Raid,
            Act::Ice => ActKind::Ice,
            Act::Ore(_) => ActKind::Ore,
            Act::Drone(_) => ActKind::Drone,
            Act::Mission { .. } => ActKind::Mission,
            Act::Explore => ActKind::Explore,
            Act::Plex => ActKind::Plex,
            Act::Pi => ActKind::Pi,
            Act::Hack => ActKind::Hack,
            Act::Sell => ActKind::Sell,
            Act::SellLoot => ActKind::SellLoot,
            Act::Market => ActKind::Market,
            Act::Trade => ActKind::Trade,
            Act::Capital => ActKind::Capital,
            Act::Foreman => ActKind::Foreman,
            Act::Sov => ActKind::Sov,
            Act::Haul => ActKind::Haul,
            Act::Fleet(_) => ActKind::Fleet,
            Act::Hunt(_) => ActKind::Hunt,
            Act::Kit(_) => ActKind::Kit,
            Act::Gang(_) => ActKind::Gang,
            Act::Wing(_) => ActKind::Wing,
            Act::Stance { .. } => ActKind::Stance,
            Act::Rare => ActKind::Rare,
            Act::Econ => ActKind::Econ,
            Act::AlignTo(_) => ActKind::Align,
            Act::Stop | Act::TravelTo { .. } | Act::JumpGate { .. } => ActKind::Order,
            Act::Resync => ActKind::Resync,
            Act::Wait(_) => ActKind::Wait,
            Act::Missing(_) => ActKind::Missing,
            Act::Reconnect => ActKind::Reconnect,
        }
    }
}

/// **The whole brain.** Pure: same inputs, same outputs, no clock but
/// `o.now` and no dice but `m`'s own.
pub fn decide(m: &Memory, o: &Observation) -> (Memory, Vec<Act>) {
    let mut m = m.clone();
    // BP-KIT: the ship's speed, for the micro jump drive's heading.
    crate::pvp_kit::observe(&mut m, o);
    // FC-3: the owner's words for the wing take effect before the turn's
    // rules, so a hold is never starved by a fight; the lead's reply leads.
    let said = if m.policy.wing.is_some() {
        crate::wing::commands(&mut m, o)
    } else {
        Vec::new()
    };
    let mut acts = choose(&mut m, o);
    if !said.is_empty() {
        acts.splice(0..0, said);
    }
    // BP-KIT: nothing moves while the drive spools.
    let acts = crate::pvp_kit::spool_gate(&m, o, acts);
    m.now = None;
    (m, acts)
}

/// What the observation says about the bot for the chatter's [`Ctx`]
/// (everything but the scene): the turn's, or a farewell kit's snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct TalkCtx {
    system: Option<i64>,
    in_fleet: bool,
    foreman: bool,
    mining: bool,
    hull: Option<&'static str>,
}

impl TalkCtx {
    fn of(o: &Observation) -> TalkCtx {
        use crate::mine_ladder::{ORCA, PORPOISE, RORQUAL};
        let hull = o.ship.and_then(|s| s.type_id);
        TalkCtx {
            system: match o.place {
                Place::InSpace { system, .. } => Some(system),
                _ => None,
            },
            in_fleet: o.fleet.is_some(),
            foreman: hull.is_some_and(|t| t == ORCA || t == PORPOISE || t == RORQUAL),
            mining: o.miner.is_some_and(|mi| mi.active),
            hull: hull.and_then(|t| match t {
                ORCA => Some("Orca"),
                PORPOISE => Some("Porpoise"),
                RORQUAL => Some("Rorqual"),
                _ => None,
            }),
        }
    }

    fn cx<'a>(&self, now: Instant, scene: &'a crate::chatter::Scene) -> crate::chatter::Ctx<'a> {
        crate::chatter::Ctx {
            now,
            system: self.system,
            in_fleet: self.in_fleet,
            foreman: self.foreman,
            mining: self.mining,
            hostile: false,
            hull: self.hull,
            scene,
        }
    }
}

/// **Dock for a geode?** The summed expected contents of the geodes in the
/// hold against the pilot's `risk` ([`crate::geode_haul::should_dock`]); a
/// human on the grid docks it at once. The reason, when so.
fn geode_dock(m: &Memory, o: &Observation) -> Option<String> {
    let table = &m.policy.econ;
    if !table.has_geodes() {
        return None;
    }
    let value = crate::geode_haul::hold_value(
        table,
        o.cargo
            .iter()
            .chain(&o.ore_hold)
            .map(|r| (r.type_id, r.quantity)),
    );
    let risk = o.hunt.temper.risk;
    let humans = !o.humans.is_empty();
    crate::geode_haul::should_dock(risk, value, humans).then(|| {
        format!(
            "geode: about {value:.0} ISK aboard, risk {risk:.2} waits for {:.0}{}",
            crate::geode_haul::dock_threshold(risk),
            if humans { ", a human on the grid" } else { "" }
        )
    })
}

/// **To a motherlode** (L7 BOT-ECON): a docked miner or foreman flies to one
/// it heard of, within its risk's reach, once each.
fn to_motherlode(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    let Place::Docked { station } = o.place else {
        return None;
    };
    if m.heard.is_empty() || !(TalkCtx::of(o).foreman || m.policy.role.name() == roles::MINE.name())
    {
        return None;
    }
    let map = m.policy.travel.clone()?;
    let from = map.system_of(station)?;
    let system = m.heard.to_motherlode(
        &map,
        from,
        o.now,
        o.hunt.temper.risk,
        m.policy.security_floor,
    )?;
    Some(vec![Act::TravelTo { system }])
}

/// **To an escort job** (L7b): a docked escort outside a fleet keeps the
/// board's job it holds or takes the nearest open ask within its risk's
/// reach, and flies to the foreman's system. In that system it does nothing
/// here: it undocks and the escort's rules take it to the foreman's belt.
fn to_escort_job(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    let Place::Docked { station } = o.place else {
        return None;
    };
    if o.me == 0 || m.policy.role.name() != roles::ESCORT.name() {
        return None;
    }
    m.job.tend(o.me, o.fleet.is_some(), o.now);
    if o.fleet.is_some() {
        return None;
    }
    let map = m.policy.travel.clone()?;
    let from = map.system_of(station)?;
    if m.job.job.is_none() {
        m.job.look(
            o.me,
            &map,
            from,
            crate::geode_haul::reach(o.hunt.temper.risk),
            m.policy.security_floor,
            o.now,
        )?;
    }
    let system = m.job.system().filter(|&s| s != from)?;
    Some(vec![Act::TravelTo { system }])
}

/// [`crate::chatter::speak`] for this turn.
fn chatter_line(m: &mut Memory, o: &Observation) -> Option<String> {
    let mut cx = TalkCtx::of(o).cx(o.now, &o.talk);
    cx.hostile = m.policy.hostile.is_some();
    crate::chatter::speak(&mut m.chatter, &cx)
}

/// The chatter governor's context for this turn (THREAT's under-attack line).
pub(crate) fn chatter_ctx(o: &Observation) -> crate::chatter::Ctx<'_> {
    TalkCtx::of(o).cx(o.now, &o.talk)
}

/// **A farewell, to be asked for at the gate** (CHATTER-2): the bot's
/// chatter state and the scene as the last observation left them. The
/// stargate jump runs inside trip code that sees no observation, so the
/// driver takes this snapshot before an act and the jump asks it for the
/// line ([`crate::chatter::farewell`], governed as everything else); the
/// state goes back into the memory afterwards.
#[derive(Debug, Clone)]
pub struct FarewellKit {
    pub state: crate::chatter::State,
    scene: crate::chatter::Scene,
    cx: TalkCtx,
}

impl FarewellKit {
    pub fn of(m: &Memory, o: &Observation) -> FarewellKit {
        FarewellKit {
            state: m.chatter.clone(),
            scene: o.talk.clone(),
            cx: TalkCtx::of(o),
        }
    }

    /// The line to say before leaving, or `None` (the governor's no).
    pub fn line(&mut self, now: Instant) -> Option<String> {
        let cx = self.cx.cx(now, &self.scene);
        crate::chatter::farewell(&mut self.state, &cx)
    }
}

fn choose(m: &mut Memory, o: &Observation) -> Vec<Act> {
    m.now = Some(o.now);
    // L7 BOT-ECON: a motherlode line in Local is remembered.
    m.heard.hear(&o.talk.heard, o.now);
    // 1. In transit.
    if o.place == Place::Transit {
        return vec![Act::Wait(SHORT_WAIT)];
    }
    if let Some(p) = &o.park
        && p.ego_warping
    {
        return vec![Act::Wait(SHORT_WAIT)];
    }

    crate::fleet_orders::expire(m, o);

    // 2. A fleet invite.
    if let Some(invite) = &o.invite
        && !m.declined.contains(&invite.fleet_id)
    {
        let wanted = o.fleet.is_none()
            && o.failed(ActKind::AcceptInvite) < GIVE_UP_AFTER
            && match &m.policy.accept_fleet {
                AcceptFleet::Any => true,
                AcceptFleet::From(name) => invite.inviter_name.as_deref() == Some(name.as_str()),
                AcceptFleet::Nobody => false,
                // The driver checks the inviter (`crate::fleet::inviter_is_allied`).
                AcceptFleet::Allied => true,
                AcceptFleet::Fc(fc) => {
                    crate::wing::accepts(m, *fc, invite.inviter_id, invite.fleet_id)
                }
            };
        if wanted {
            // REACT-JITTER: reading the invite takes a moment.
            if let Some(left) = m.thinking(crate::react::Class::Invite, invite.fleet_id, o) {
                return vec![Act::Wait(left.min(SHORT_WAIT))];
            }
            return vec![Act::AcceptInvite {
                fleet: invite.fleet_id,
            }];
        }
        m.declined.push(invite.fleet_id);
    }

    // 2b. A new fleet: say hello, once. [A]
    if let Some(f) = o.fleet
        && o.chat
        && m.greeted != Some(f.fleet_id)
    {
        m.greeted = Some(f.fleet_id);
        return vec![Act::Say(m.policy.role.greeting.to_owned())];
    }

    // 2b'. Local and fleet talk (S1): at most one line, and only when the
    // governor lets it. [A]
    if o.chat
        && let Some(line) = chatter_line(m, o)
    {
        return vec![Act::Say(line)];
    }
    // 2b''. A foreman who was told of a motherlode says so in Local, once
    // (L7 BOT-ECON).
    if o.chat
        && let Some(line) = crate::geode_haul::announce(&mut m.told, TalkCtx::of(o).foreman, o.now)
    {
        return vec![Act::Say(line)];
    }

    let role = m.policy.role;
    match o.place {
        Place::Docked { .. } => {
            if let Some(acts) = crate::threat::act(m, o) {
                return acts;
            }
            // 2d. A fleet boss's order (BOT-FLEET-ORDERS): Hold keeps a
            // docked bot docked; Travel is carried out from the station.
            if crate::fleet_orders::holding(m, o).is_some() {
                let acts = crate::fleet_orders::hold_filter(
                    (role.docked)(m, o)
                        .into_iter()
                        .filter(|a| a.kind() != ActKind::Undock)
                        .collect(),
                );
                return acts;
            }
            // L7 BOT-ECON: geodes opened, a SKIN worn, skill points traded,
            // PLEX sold; then a miner or foreman flies to a motherlode it
            // heard of.
            if let Some(acts) = crate::nes_use::due(m, o) {
                return acts;
            }
            if let Some(acts) = to_motherlode(m, o) {
                return acts;
            }
            if let Some(acts) = to_escort_job(m, o) {
                return acts;
            }
            crate::fleet_orders::act(m, o).unwrap_or_else(|| (role.docked)(m, o))
        }
        Place::InSpace { bound: false, .. } => {
            if o.failed(ActKind::Bind) >= GIVE_UP_AFTER {
                return vec![Act::Reconnect];
            }
            vec![Act::Bind]
        }
        Place::InSpace { bound: true, .. } => {
            // 2c. A primary the fleet boss called (SOC-C7); before it, a
            // friend's call for backup (R-REL-3).
            let acts = if let Some(acts) = crate::threat::act(m, o) {
                acts
            } else if let Some(first) = crate::fleet_orders::holding(m, o) {
                // 2d. Hold (BOT-FLEET-ORDERS): still, but the guns, the
                // repairs and the drones go on.
                let acts = crate::fleet::engage(m, o).unwrap_or_else(|| (role.in_space)(m, o));
                stop_first(first, crate::fleet_orders::hold_filter(acts))
            } else {
                // FC-1: a wing serves its FC, not other people's backup calls.
                let help = if m.policy.wing.is_some() {
                    None
                } else {
                    crate::effects::help_acts(m, o)
                };
                help.or_else(|| crate::fleet::engage(m, o))
                    .or_else(|| crate::fleet_orders::act(m, o))
                    .or_else(|| courtesy(m, o))
                    .unwrap_or_else(|| (role.in_space)(m, o))
            };
            // BOT-CAP: no laser or gun while the capacitor is out.
            crate::cap::gate(o, acts)
        }
        Place::Transit => vec![Act::Wait(SHORT_WAIT)],
    }
}

/// A Hold order's first turn stops the ship before anything else.
fn stop_first(first: bool, mut acts: Vec<Act>) -> Vec<Act> {
    if first {
        acts.insert(0, Act::Stop);
    }
    acts
}

/// **Courtesy** (module header): `Some(the way home)` when this bot's role
/// is civilian (and its policy has courtesy on) and a human outside its
/// fleet is within [`COURTESY_RANGE`];
/// `None` -- the role decides -- otherwise, and always for a role that is
/// not civilian (ruling 5).
pub fn courtesy(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if !m.policy.role.civilian || !m.policy.courtesy {
        return None;
    }
    // BOT-CHURN: a dev roster's bots are there to be watched; the one human
    // near them is the owner, so stepping aside undocked them and docked
    // them again at the mouth, every dwell.
    if m.policy.dev.is_some() {
        return None;
    }
    let park = o.park.as_ref()?;
    let nearest = o
        .humans
        .iter()
        .map(|h| park.ego.surface_to(h))
        .min_by(f64::total_cmp)
        .filter(|d| *d <= COURTESY_RANGE)?;
    // REACT-JITTER: the role carries on while the bot notices the human.
    let arrived = o
        .humans
        .iter()
        .min_by(|a, b| park.ego.surface_to(a).total_cmp(&park.ego.surface_to(b)))
        .map(|h| h.id)?;
    if m.homing.is_none() && m.thinking(crate::react::Class::Human, arrived, o).is_some() {
        return None;
    }
    if m.homing.is_none() {
        m.homing = Some(Homing::Courtesy);
    }
    if m.homing == Some(Homing::Courtesy) {
        m.home_why = Some((
            Homing::Courtesy,
            format!(
                "courtesy: a human outside the fleet {:.1} km away (range {:.0} km)",
                nearest / 1_000.0,
                COURTESY_RANGE / 1_000.0
            ),
        ));
    }
    m.claim = None;
    m.idle_next = true;
    Some(go_home(m, o, park))
}

/// Rule 3, the miner's ([`crate::roles::MINE`]).
pub(crate) fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    // Whatever was meant in space is over.
    let came_home_full = m.homing == Some(Homing::HoldFull);
    m.claim = None;
    m.homing = None;
    m.home_why = None;
    m.stop_sent = false;
    // This trip's anomaly and drones are over too (SOC-B10); drones still
    // out were left in space.
    m.ore_site = crate::ore::SiteStep::Untried;
    m.drones.clear();
    m.drone_rock = None;
    m.drones_tried = false;
    // A stop that arrived on the way in has nothing left to say.
    if let Some(stop) = &o.last_stop {
        m.stop_handled = m.stop_handled.max(stop.seq);
    }

    // 6. Recovery, docked.
    if o.failures.values().any(|&n| n >= GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }

    // PROG-NULL: an eligible miner, every RECHECK, spends a turn on the
    // deep-belt deployment (out to rare ore, or the stock home).
    if let Some(acts) = crate::rare_ore::due(m, o) {
        return acts;
    }

    // a. Ore in the hold, or in the mining hold (GAPS-B).
    let ore: Vec<(i64, i64)> = o
        .cargo
        .iter()
        .chain(&o.ore_hold)
        .filter(|r| r.category_id == CATEGORY_ASTEROID)
        .map(|r| (r.item_id, r.quantity))
        .collect();
    if !ore.is_empty() {
        return vec![Act::Unload(ore)];
    }
    // a2. Goods in the hangar: offer them, once a visit (FX6) and at most
    // once per `sell_every` (E8: one batched sale).
    let sale_due = m.sold_at.is_none_or(|t| o.now >= t + m.policy.sell_every);
    if !m.sold && sale_due && o.hangar.iter().any(crate::builder::is_miner_goods) {
        m.sold = true;
        m.sold_at = Some(o.now);
        return vec![Act::Sell];
    }
    // Home with a full hold and no ore in it: the hold is full of something
    // the bot does not unload, and going out again would only be refused
    // again. Say so and idle, once per trip. [A]
    if came_home_full {
        return vec![Act::Missing("room in the hold"), Act::Wait(MISSING_WAIT)];
    }

    let ship = o.ship;
    let active = ship.map(|s| s.ship_id);
    let in_capsule = ship.is_none_or(|s| s.type_id.is_none_or(|t| t == CAPSULE));
    let flying = ship.and_then(|s| s.type_id);
    // The best miner's hull in the hangar, when it beats what is flown
    // (GAPS-B: a Venture before a Merlin, [`MINER_HULLS`]).
    let flown_rank = flying.and_then(miner_hull_rank);
    let hangar_hull = o
        .hangar
        .iter()
        .filter(|r| Some(r.item_id) != active)
        .filter(|r| !m.combat.untrained.contains(&r.type_id))
        .filter_map(|r| miner_hull_rank(r.type_id).map(|k| (k, r.item_id)))
        .filter(|&(k, _)| flown_rank.is_none_or(|f| k < f))
        .min()
        .map(|(_, id)| id);
    // PROG-MINE: the laser that suits the hull flown (a Strip Miner I on a
    // barge or an exhumer, a Miner I otherwise).
    let hangar_miner = o
        .hangar
        .iter()
        .find(|r| crate::mine_ladder::laser_fits(r.type_id, flying))
        .map(|r| r.item_id);

    // b. A hull: a miner's hull in the hangar is boarded from the capsule
    // or from the rookie ship (E8), and a Venture from a Merlin (GAPS-B).
    if let Some(hull) = hangar_hull {
        return vec![Act::Board(hull)];
    }
    if in_capsule {
        let need_miner = hangar_miner.is_none();
        return setup_or_missing(m, true, need_miner, "a Merlin in the hangar");
    }
    // b2. The closed economy: a miner in its rookie ship (or in a Merlin,
    // GAPS-B) looks for a better hull (and a Miner I) now and then, and
    // mines with what it has meanwhile.
    if m.policy.closed
        && flown_rank != Some(0)
        && let Some(acts) = acquire_if_due(
            m,
            true,
            // A Merlin with its laser needs no spare one for the look; a
            // Venture bought gets its laser by rule c. [A]
            hangar_miner.is_none() && flown_rank.is_none(),
            false,
        )
    {
        return acts;
    }

    // c. A laser.
    let Some(miner) = o.miner else {
        if let Some(item) = hangar_miner {
            return vec![Act::FitAuto(item)];
        }
        return setup_or_missing(m, false, true, "a Miner I in the hangar");
    };

    // d. Online.
    if !miner.online {
        return vec![Act::Online(miner.item_id)];
    }
    // d2. Mining drones from the hangar into the bay, once a visit
    // (SOC-B10).
    if let Some(acts) = crate::ore::stock_first(m, &o.hangar) {
        return acts;
    }

    // e. Dwell, then go.
    dwell_then_undock(m, o)
}

/// Rule 3e, every role's: sit docked for the policy's (jittered) dwell, then
/// undock.
pub(crate) fn dwell_then_undock(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if m.policy.quick {
        m.dwell_until = None;
        m.idle_next = false;
        m.sold = false;
        return vec![Act::Undock];
    }
    let until = match m.dwell_until {
        Some(t) => t,
        None => {
            let range = if m.idle_next {
                m.policy.idle_dwell
            } else {
                m.policy.dwell
            };
            let t = o.now + m.jitter(range);
            m.dwell_until = Some(t);
            t
        }
    };
    if o.now < until {
        return vec![Act::Wait(until - o.now)];
    }
    m.dwell_until = None;
    m.idle_next = false;
    m.sold = false;
    vec![Act::Undock]
}

fn setup_or_missing(m: &mut Memory, ship: bool, miner: bool, what: &'static str) -> Vec<Act> {
    gm_or_missing(m, ship, miner, false, what)
}

/// GM setup once per connection when the policy allows it, else `MISSING`.
pub(crate) fn gm_or_missing(
    m: &mut Memory,
    ship: bool,
    module: bool,
    ammo: bool,
    what: &'static str,
) -> Vec<Act> {
    if m.policy.gm_setup && !m.setup_done {
        m.setup_done = true;
        return vec![Act::GmSetup { ship, module, ammo }];
    }
    if m.policy.closed
        && let Some(acts) = acquire_if_due(m, ship, module, ammo)
    {
        return acts;
    }
    vec![Act::Missing(what), Act::Wait(MISSING_WAIT)]
}

/// `MISSING` answers in a row before [`acquire_if_due`] buys early.
pub const MISSING_BUY_AFTER: u32 = 5;

/// The closed economy's setup (E8): one [`Act::Acquire`] when the last was
/// at least [`crate::acquire::ACQUIRE_EVERY`] ago (asks come and go, so it
/// is tried again), else `None` and the caller says `MISSING`.
pub(crate) fn acquire_if_due(
    m: &mut Memory,
    ship: bool,
    module: bool,
    ammo: bool,
) -> Option<Vec<Act>> {
    let now = m.now?;
    if let Some(next) = m.acquire_next.filter(|t| now < *t) {
        // A module or round that stays missing is bought again after
        // `MISSING_RETRY`, not after the whole `ACQUIRE_EVERY` (the 197-line
        // MISSING loop of a ratter whose guns were gone).
        m.missing_streak += 1;
        let last = next.checked_sub(crate::acquire::ACQUIRE_EVERY);
        let early = m.missing_streak >= MISSING_BUY_AFTER
            && last.is_none_or(|l| now >= l + crate::acquire::MISSING_RETRY);
        if !early || !(module || ammo) {
            return None;
        }
    }
    m.missing_streak = 0;
    m.acquire_next = Some(now + crate::acquire::ACQUIRE_EVERY);
    Some(vec![Act::Acquire { ship, module, ammo }])
}

/// Rule 5, with 6's in-space half: the miner's ([`crate::roles::MINE`]).
pub(crate) fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(park) = &o.park else {
        return vec![Act::Wait(SHORT_WAIT)];
    };
    m.dwell_until = None;
    m.drones_stocked = false;
    // BOT-SCRAM: a scrambled ship fights or aligns and waits; it neither
    // counts the refused warps nor reconnects.
    if let Some(acts) = crate::scram::decide(m, o) {
        return acts;
    }
    // BP-HOLD: a ship held by a warp bubble burns out of it.
    if let Some(acts) = crate::warp_bubble::decide(m, o) {
        return acts;
    }
    let ego = park.ego;
    let miner = o.miner;

    // The laser is idle: whatever stop was asked for has happened.
    if miner.is_none_or(|l| !l.active) {
        m.stop_sent = false;
    }

    // 6. Recovery. A travel act that keeps failing cannot be cured by
    // travelling, so that one reconnects; anything else goes home.
    if o.failures
        .iter()
        .any(|(k, &n)| k.is_travel() && n >= GIVE_UP_AFTER)
    {
        return vec![Act::Reconnect];
    }
    if o.failures
        .iter()
        .any(|(k, &n)| !k.is_travel() && n >= GIVE_UP_AFTER)
    {
        m.homing = Some(Homing::Recovery);
    }

    // Stops: each acted on once.
    let mut after_stop = Vec::new();
    if let Some(stop) = &o.last_stop
        && stop.seq > m.stop_handled
    {
        m.stop_handled = stop.seq;
        match &stop.reason {
            // PROG-MINE: a fleet mate's compressor first -- the hold, a
            // hundredth the volume, takes the rest of the trip.
            // FLEET-HANGAR: then the boss's fleet hangar, before home.
            StopReason::HoldFull => {
                let compress = crate::ore::on_hold_full(m, o);
                let drop = crate::fleet_hangar::begin(m, o);
                match compress {
                    Some(acts) => after_stop.extend(acts),
                    None if drop => {}
                    None => m.homing = Some(Homing::HoldFull),
                }
            }
            StopReason::Depleted => m.claim = None,
            StopReason::TooFar => {
                if let Some(rock) = m.claim {
                    if m.policy.resync {
                        after_stop.push(Act::Resync);
                    }
                    after_stop.push(Act::Follow {
                        id: rock,
                        range: APPROACH_RANGE,
                    });
                }
            }
            StopReason::Stopped | StopReason::Other(_) => {}
        }
    }
    // FLEET-HANGAR: a drop under way flies to the boss and drops, before any
    // belt or rock; one given up sets `homing` and goes home below.
    if after_stop.is_empty()
        && m.homing.is_none()
        && let Some(acts) = crate::fleet_hangar::step(m, o, park)
    {
        return acts;
    }
    // A fleet boss who is not on this grid (GRID-B): rule b warps to him.
    let boss_char = o.fleet.and_then(|f| f.boss);
    let boss_unreachable = o.boss_unreachable_until.is_some_and(|t| o.now < t);
    let boss_away = boss_char.is_some()
        && !boss_unreachable
        && park
            .boss
            .is_none_or(|b| !crate::escort::warp_lands_short(ego.surface_to(&b), MINING_STANDOFF));
    // L7 BOT-ECON: a geode aboard that the risk trait will not carry on with.
    if m.homing.is_none()
        && let Some(why) = geode_dock(m, o)
    {
        m.homing = Some(Homing::Geode);
        m.home_why = Some((Homing::Geode, why));
    }
    if m.homing.is_none() {
        if miner.is_none() {
            m.homing = Some(Homing::Unarmed);
        } else if park.rocks.is_empty() && !boss_away {
            // An ore anomaly first, once a trip (SOC-B10); then a belt.
            if let Some(acts) = crate::ore::to_an_anomaly(m, park) {
                return acts;
            }
            if let Some(acts) = to_a_belt(m, park, o.now) {
                return acts;
            }
            m.homing = Some(Homing::NoRocks);
            m.idle_next = true;
        } else if !park.rocks.is_empty() {
            m.belt = None;
            m.belt_waits = 0;
        }
    }

    // a. Home.
    if m.homing == Some(Homing::Repair) {
        note_flee(m, park, o.now);
    }
    if m.homing.is_some() {
        m.claim = None;
        m.belt = None;
        m.belt_waits = 0;
        return go_home(m, o, park);
    }
    if !after_stop.is_empty() {
        return after_stop;
    }
    let Some(miner) = miner else {
        return vec![Act::Wait(SHORT_WAIT)];
    };

    // b. The boss, more than a warp away -- or off this grid altogether,
    // where his ship is not in the park at all (GRID-B).
    if let Some(char_id) = boss_char.filter(|_| boss_away) {
        m.claim = None;
        return vec![Act::WarpToMember {
            char_id,
            range: MINING_STANDOFF,
        }];
    }

    // c. A claim: still there, and not somebody else's.
    if let Some(rock) = m.claim {
        let gone = !park.rocks.iter().any(|r| r.id == rock);
        // Two bots on one rock: the higher ship id yields. Deterministic,
        // so both never yield at once and both never stay. [A]
        let yield_to = park
            .taken
            .get(&rock)
            .is_some_and(|&other| other != ego.id && other < ego.id);
        if gone || yield_to {
            m.claim = None;
        }
    }
    // The laser on anything but the claim stops first.
    if miner.active && miner.target != m.claim {
        if m.stop_sent {
            return vec![Act::Wait(SHORT_WAIT)];
        }
        m.stop_sent = true;
        return vec![Act::StopMining(miner.item_id)];
    }
    let rock = match m.claim {
        Some(r) => r,
        // With a boss in the park, only the rocks within a warp of him:
        // a follower stays with the fleet, and a belt-less boss is waited
        // beside, not left for the old belt -- which would be a warp back,
        // then rule b's warp to him again, for ever. [A]
        None => match pick_rock(m.seed, park, park.boss.as_ref().unwrap_or(&ego)) {
            Some(r) => {
                m.claim = Some(r);
                r
            }
            None => {
                // Every rock is under somebody's laser, or none is near the
                // boss: wait.
                return vec![Act::Wait(MINING_WAIT)];
            }
        },
    };
    let Some(target) = park.rocks.iter().find(|r| r.id == rock) else {
        return vec![Act::Wait(SHORT_WAIT)];
    };
    let gap = ego.surface_to(target);
    if !crate::escort::warp_lands_short(gap, MINING_STANDOFF) {
        return vec![Act::WarpToItem {
            id: rock,
            range: MINING_STANDOFF,
        }];
    }
    if gap > miner.range * RANGE_MARGIN {
        return vec![Act::Follow {
            id: rock,
            range: APPROACH_RANGE,
        }];
    }

    // d. Locked, and mining.
    if !o.locked.contains(&rock) {
        if o.pending.contains(&rock) {
            return vec![Act::Wait(SHORT_WAIT)];
        }
        // MINER-PATIENCE: a lock the jam refused is not a failure; wait out
        // the retry gap, and past the patience go to the next belt.
        if let Some(jam) = o.lock_jam {
            match jam.verdict(o.now) {
                JamStep::Wait(left) => return vec![Act::Wait(left.max(SHORT_WAIT))],
                JamStep::Retry => {}
                JamStep::NextBelt => {
                    if let Some(acts) = leave_belt(m, park, o.now) {
                        return acts;
                    }
                    // No other belt: keep trying here, at the retry gap.
                    let gap = o.now.saturating_duration_since(jam.last);
                    if gap < JAM_RETRY {
                        return vec![Act::Wait((JAM_RETRY - gap).max(SHORT_WAIT))];
                    }
                }
            }
        }
        let mut acts: Vec<Act> = o
            .locked
            .iter()
            .filter(|&&t| t != rock && crate::park::is_rock(t))
            .map(|&t| Act::Unlock(t))
            .collect();
        // REACT-JITTER: a new rock is looked over before the click.
        if let Some(left) = m.thinking(crate::react::Class::Target, rock, o) {
            return vec![Act::Wait(left.min(SHORT_WAIT))];
        }
        acts.push(Act::Lock(rock));
        return acts;
    }
    if !miner.active {
        return vec![Act::Mine {
            module: miner.item_id,
            rock,
        }];
    }
    // e. The laser runs: the drones onto the same rock (SOC-B10).
    if let Some(acts) = crate::ore::drones_on(m, rock) {
        return acts;
    }
    // f. Running.
    vec![Act::Wait(MINING_WAIT)]
}

/// The belt the bot is at: the nearest, when it is within a warp's landing
/// of it (pure).
fn belt_here(park: &ParkView) -> Option<Ball> {
    park.belts
        .iter()
        .map(|b| (park.ego.surface_to(b), *b))
        .filter(|(gap, _)| crate::escort::warp_lands_short(*gap, MINING_STANDOFF))
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.id.cmp(&b.1.id)))
        .map(|(_, b)| b)
}

/// MINER-PATIENCE: a defend flee records the belt it came from, once an
/// episode (an avoid still in force is kept).
fn note_flee(m: &mut Memory, park: &ParkView, now: Instant) {
    if m.npc_avoid.is_some_and(|(_, until)| now < until) {
        return;
    }
    if let Some(b) = belt_here(park) {
        m.npc_avoid = Some((b.id, now + NPC_AVOID));
    }
}

/// MINER-PATIENCE: the jam outlasted the patience: avoid this belt for
/// [`JAM_AVOID`] and warp to the nearest other one. `None` when there is no
/// other belt (the bot keeps trying here).
fn leave_belt(m: &mut Memory, park: &ParkView, now: Instant) -> Option<Vec<Act>> {
    let here = belt_here(park)?;
    let next = park
        .belts
        .iter()
        .filter(|b| b.id != here.id)
        .filter(|b| m.threat.avoid_belt != Some(b.id))
        .filter(|b| {
            !m.npc_avoid
                .is_some_and(|(a, until)| a == b.id && now < until)
        })
        .map(|b| (park.ego.surface_to(b), *b))
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.id.cmp(&b.1.id)))
        .map(|(_, b)| b)?;
    m.npc_avoid = Some((here.id, now + JAM_AVOID));
    m.claim = None;
    m.belt = Some(next.id);
    m.belt_waits = 0;
    Some(vec![Act::WarpToItem {
        id: next.id,
        range: MINING_STANDOFF,
    }])
}

/// Rule 5a: warp if far, dock if close, approach in between.
pub(crate) fn go_home(m: &mut Memory, o: &Observation, park: &ParkView) -> Vec<Act> {
    if let Some(l) = o.miner
        && l.active
    {
        if m.stop_sent {
            return vec![Act::Wait(SHORT_WAIT)];
        }
        m.stop_sent = true;
        return vec![Act::StopMining(l.item_id)];
    }
    // Drones out come home first (SOC-B10).
    if let Some(acts) = crate::ore::recall_first(m) {
        return acts;
    }
    let Some(station) = park.station else {
        return vec![
            Act::Missing("a station to go home to"),
            Act::Wait(MISSING_WAIT),
        ];
    };
    let gap = park.ego.surface_to(&station);
    if gap >= crate::MIN_WARP_DISTANCE {
        vec![Act::WarpToItem {
            id: station.id,
            range: 0.0,
        }]
    } else if gap < crate::DOCK_RANGE {
        vec![Act::Dock(station.id)]
    } else {
        vec![Act::Follow {
            id: station.id,
            range: 0.0,
        }]
    }
}

/// How many decisions a bot waits at a belt for its rocks before calling the
/// belt empty. **[A]** The server's grid reconcile adds a belt's rocks within
/// about a second of landing (`eve_server::grid::RECONCILE_EVERY`); four
/// [`SHORT_WAIT`]s is twice that.
pub const BELT_WAITS: u32 = 4;

/// How long a belt found empty is left alone **[A]**: about one daily reset
/// is 24 h, a drained null belt stays dry for hours. Falsified if a belt
/// found dry is regularly full again within the hour.
pub const DRY_FOR: Duration = Duration::from_secs(3 * 3600);

/// **No rock in the park: go to a belt for some** (GRID-B). A park holds the
/// rocks of its own grid only, so at the station there are none to pick;
/// the belts are global. One belt per trip, among the [`SPREAD`] nearest by
/// the bot's seed; warp there, wait for the grid's rocks, and only then call
/// it empty. `None`: the belt is reached and still empty (or there is no
/// belt), and the caller goes home.
pub fn to_a_belt(m: &mut Memory, park: &ParkView, now: Instant) -> Option<Vec<Act>> {
    if park.ego_warping {
        return Some(vec![Act::Wait(SHORT_WAIT)]);
    }
    let belt = match m.belt.and_then(|id| park.belts.iter().find(|b| b.id == id)) {
        Some(b) => *b,
        None => {
            m.dry.retain(|&(_, until)| now < until);
            let mut near: Vec<(f64, Ball)> = park
                .belts
                .iter()
                // Dry belts are dropped before the SPREAD nearest are cut,
                // so the choice widens past them; all dry: none.
                .filter(|b| !m.dry.iter().any(|&(d, _)| d == b.id))
                // THREAT: the belt a player struck at is left alone, unless
                // it is the only one; so is a belt a flee came from
                // (MINER-PATIENCE).
                .filter(|b| {
                    belt_allowed(
                        m.threat.avoid_belt,
                        m.npc_avoid,
                        b.id,
                        now,
                        park.belts.len(),
                    )
                })
                .map(|b| (park.ego.surface_to(b), *b))
                .collect();
            near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
            near.truncate(SPREAD);
            let (_, b) = *near.get((m.seed % near.len().max(1) as u64) as usize)?;
            m.belt = Some(b.id);
            m.belt_waits = 0;
            b
        }
    };
    if !crate::escort::warp_lands_short(park.ego.surface_to(&belt), MINING_STANDOFF) {
        return Some(vec![Act::WarpToItem {
            id: belt.id,
            range: MINING_STANDOFF,
        }]);
    }
    if m.belt_waits < BELT_WAITS {
        m.belt_waits += 1;
        return Some(vec![Act::Wait(SHORT_WAIT)]);
    }
    // Waited out and still no rock: this belt is dry for a while.
    if !m.dry.iter().any(|&(d, _)| d == belt.id) {
        m.dry.push((belt.id, now + DRY_FOR));
    }
    None
}

/// The rock to claim: among the [`SPREAD`] free rocks nearest `near`, the
/// one at `seed % n`, so bots with different names start apart. `None` when
/// every rock is under another laser -- or, when `near` is not the bot
/// itself (a fleet boss), when none is within a warp of `near`.
pub fn pick_rock(seed: u64, park: &ParkView, near: &Ball) -> Option<i64> {
    let leash = (near.id != park.ego.id).then_some(crate::MIN_WARP_DISTANCE);
    let mut free: Vec<(f64, i64)> = park
        .rocks
        .iter()
        .filter(|r| park.taken.get(&r.id).is_none_or(|&s| s == park.ego.id))
        .map(|r| (near.surface_to(r), r.id))
        .filter(|(gap, _)| leash.is_none_or(|l| *gap <= l))
        .collect();
    // A lit moon belt's rock first (MN-G): a miner prefers the richer ore.
    free = crate::moon_pref::prefer_moon(free);
    free.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    free.truncate(SPREAD);
    if free.is_empty() {
        return None;
    }
    Some(free[(seed % free.len() as u64) as usize].1)
}

#[cfg(test)]
mod tests {
    //! **T1** (`docs/lanes/bots.md` §7): each rule with a minimal
    //! observation. Asserts the **kind and order** of acts, never a delay or
    //! a range -- those are [A].
    use super::*;

    #[test]
    fn a_module_that_stays_missing_is_bought_again_before_the_half_hour() {
        let mut m = Memory::new("m", Policy::fast());
        let t0 = Instant::now();
        m.now = Some(t0);
        assert!(acquire_if_due(&mut m, false, true, false).is_some());
        // Inside the retry wait nothing is bought, however often it is asked.
        for _ in 0..(MISSING_BUY_AFTER * 2) {
            assert!(acquire_if_due(&mut m, false, true, false).is_none());
        }
        let later = t0 + crate::acquire::MISSING_RETRY;
        m.now = Some(later);
        assert!(
            acquire_if_due(&mut m, false, true, false).is_some(),
            "early"
        );
        assert_eq!(m.missing_streak, 0);
        // A hull alone waits for the half hour.
        m.missing_streak = MISSING_BUY_AFTER * 2;
        m.now = Some(later + crate::acquire::MISSING_RETRY);
        assert!(acquire_if_due(&mut m, true, false, false).is_none());
    }

    const STATION: i64 = 60_012_526;
    const SHIP: i64 = 100_000_101;
    const LASER: i64 = 100_000_102;
    const ROCK_A: i64 = eve_server::asteroid::MIN_ROCK_ID;
    const ROCK_B: i64 = eve_server::asteroid::MIN_ROCK_ID + 1;

    fn row(item_id: i64, type_id: i64, category_id: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: 90_000_001,
            location_id: STATION,
            flag_id: crate::FLAG_HANGAR,
            quantity: 1,
            group_id: 0,
            category_id,
            singleton: true,
        }
    }

    fn laser(active: bool, target: Option<i64>) -> ModuleInfo {
        ModuleInfo {
            item_id: LASER,
            flag_id: 27,
            online: true,
            active,
            target,
            range: 10_000.0,
        }
    }

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 100.0,
        }
    }

    /// Docked in a fitted, online Merlin with an empty hold: ready to go.
    fn docked_ready() -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: STATION };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(MERLIN),
        });
        o.miner = Some(laser(false, None));
        o.home = Some(STATION);
        o
    }

    /// In space beside the station, rocks 20 and 40 km off.
    fn in_space() -> Observation {
        let mut o = docked_ready();
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: true,
        };
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: vec![ball(ROCK_A, 20_000.0), ball(ROCK_B, 40_000.0)],
            belts: Vec::new(),
            station: Some(ball(STATION, -1_000.0)),
            boss: None,
            taken: BTreeMap::new(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o
    }

    fn fast(name: &str) -> Memory {
        Memory::new(name, Policy::fast())
    }

    fn kinds(acts: &[Act]) -> Vec<ActKind> {
        acts.iter().map(Act::kind).collect()
    }

    fn park(o: &mut Observation) -> &mut ParkView {
        o.park.as_mut().expect("a park")
    }

    /// BOT-FLEET-ORDERS: a Hold order stops the ship once, then the miner
    /// stays where it is -- no warp to a belt, no approach -- until the
    /// order runs out; and a Travel order from the boss is one `TravelTo`.
    #[test]
    fn a_hold_order_keeps_a_bot_still_and_a_travel_order_goes() {
        use crate::fleet_orders::{HOLD_FOR, Heard, hear};
        let mut o = in_space();
        o.fleet = Some(FleetView {
            fleet_id: 7,
            boss: Some(90_000_009),
        });
        let mut m = fast("Miner1");
        m.policy.react = false;
        // Free: the miner goes for a rock.
        let (_, free) = decide(&m, &o);
        assert!(
            free.iter().any(|a| matches!(
                a.kind(),
                ActKind::Follow | ActKind::WarpToMember | ActKind::WarpToItem | ActKind::Orbit
            )),
            "{free:?}"
        );
        hear(&mut m, Heard::Hold, o.now);
        let (m1, first) = decide(&m, &o);
        assert_eq!(first[0], Act::Stop, "{first:?}");
        let (_, held) = decide(&m1, &o);
        assert!(
            held.iter()
                .all(|a| crate::fleet_orders::HOLD_KEEPS.contains(&a.kind())),
            "{held:?}"
        );
        let mut late = o.clone();
        late.now += HOLD_FOR + Duration::from_secs(1);
        let (m2, acts) = decide(&m1, &late);
        assert_eq!(m2.order, None);
        assert!(!acts.contains(&Act::Stop));

        let mut m = fast("Miner1");
        m.policy.react = false;
        hear(&mut m, Heard::Travel { system: 30_000_042 }, o.now);
        let (m3, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::TravelTo { system: 30_000_042 }]);
        assert_eq!(m3.order, None, "spent");
    }

    /// WARP-LOCK: a lock refused because the target is in warp is not a failure.
    #[test]
    fn a_lock_refused_for_a_target_in_warp_is_not_a_failure() {
        assert!(target_left_refusal("... TargetingAttemptCancelled ..."));
        assert!(!target_left_refusal("TargetOutOfRange"));
    }

    /// MINER-PATIENCE: the jam's refusal keys (the new one always, the old
    /// one only with no lock held), and the verdict over the patience.
    #[test]
    fn a_jammed_lock_is_recognised_and_paced() {
        assert!(jam_refusal("... TargetingSystemsBeingJammed ...", 2));
        assert!(jam_refusal("... TargetingSystemsAlreadyFullyUtilized", 0));
        assert!(!jam_refusal("... TargetingSystemsAlreadyFullyUtilized", 3));
        assert!(!jam_refusal("TargetOutOfRange", 0));
        let t0 = Instant::now();
        let jam = LockJam {
            since: t0,
            last: t0,
        };
        assert!(matches!(jam.verdict(t0), JamStep::Wait(w) if w <= JAM_RETRY));
        assert_eq!(jam.verdict(t0 + JAM_RETRY), JamStep::Retry);
        assert_eq!(jam.verdict(t0 + JAM_PATIENCE), JamStep::NextBelt);
        let (a, b) = (1, 2);
        let until = t0 + NPC_AVOID;
        assert!(!belt_allowed(None, Some((a, until)), a, t0, 2));
        assert!(belt_allowed(None, Some((a, until)), b, t0, 2));
        assert!(belt_allowed(None, Some((a, until)), a, until, 2), "expired");
        assert!(
            belt_allowed(None, Some((a, until)), a, t0, 1),
            "the only belt"
        );
        assert!(!belt_allowed(Some(a), None, a, t0, 2));
    }

    /// MINER-PATIENCE: with a jam on, the miner waits out the retry gap,
    /// retries the lock, and after the patience warps to another belt,
    /// leaving this one on the avoid list. With no other belt it keeps
    /// trying here.
    #[test]
    fn a_jammed_miner_retries_then_moves_to_the_next_belt() {
        const HERE: i64 = 40_000_301;
        const NEXT: i64 = 40_000_302;
        let mut o = in_space();
        park(&mut o).rocks = vec![ball(ROCK_A, 2_000.0)];
        park(&mut o).belts = vec![ball(HERE, 0.0), ball(NEXT, 1.0e10)];
        let mut m = fast("Miner1");
        m.policy.react = false;
        let t0 = o.now;
        o.lock_jam = Some(LockJam {
            since: t0,
            last: t0,
        });
        let (_, acts) = decide(&m, &o);
        assert!(matches!(acts[..], [Act::Wait(_)]), "{acts:?}");
        o.now = t0 + JAM_RETRY;
        let (m1, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Lock(ROCK_A)]);
        assert_eq!(m1.npc_avoid, None);
        o.now = t0 + JAM_PATIENCE;
        let (m2, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![Act::WarpToItem {
                id: NEXT,
                range: MINING_STANDOFF
            }]
        );
        assert_eq!(m2.npc_avoid.map(|(b, _)| b), Some(HERE));
        park(&mut o).belts = vec![ball(HERE, 0.0)];
        let (_, acts) = decide(&m, &o);
        assert!(
            matches!(acts[..], [Act::Wait(_)] | [Act::Lock(_)]),
            "no other belt: {acts:?}"
        );
    }

    /// MINER-PATIENCE: a defend flee records the belt it came from, and a
    /// later trip for rocks leaves it alone until the memory runs out.
    #[test]
    fn a_belt_fled_from_is_avoided_for_a_while() {
        const HERE: i64 = 40_000_301;
        const NEXT: i64 = 40_000_302;
        let mut o = in_space();
        park(&mut o).belts = vec![ball(HERE, 0.0), ball(NEXT, 1.0e10)];
        let mut m = fast("Miner1");
        m.homing = Some(Homing::Repair);
        let (m1, _) = decide(&m, &o);
        let (belt, until) = m1.npc_avoid.expect("the belt is remembered");
        assert_eq!(belt, HERE);
        assert!(until > o.now);
        // Back out for rocks with none in the park: not that belt, for any seed.
        park(&mut o).rocks.clear();
        park(&mut o).ego = ball(SHIP, 5.0e9);
        for seed in 0..8 {
            let mut m = fast("Miner1");
            m.seed = seed;
            m.npc_avoid = Some((HERE, o.now + NPC_AVOID));
            let (_, acts) = decide(&m, &o);
            assert_eq!(
                acts,
                vec![Act::WarpToItem {
                    id: NEXT,
                    range: MINING_STANDOFF
                }]
            );
        }
    }

    #[test]
    fn an_invite_beats_everything() {
        let mut o = docked_ready();
        o.cargo = vec![row(1, 1230, CATEGORY_ASTEROID)];
        o.invite = Some(Invite {
            fleet_id: 7,
            inviter_id: 90_000_009,
            inviter_name: Some("Alice".to_owned()),
        });
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::AcceptInvite { fleet: 7 }]);

        // ...unless the policy says no, and then it is decided once.
        let mut m = fast("Miner1");
        m.policy.accept_fleet = AcceptFleet::From("Bob".to_owned());
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Unload]);
        assert_eq!(m.declined, vec![7]);
        let (m2, _) = decide(&m, &o);
        assert_eq!(m2.declined, vec![7], "not re-decided");
    }

    #[test]
    fn ore_in_the_hold_is_unloaded_before_undocking() {
        let mut o = docked_ready();
        o.cargo = vec![row(11, 1230, CATEGORY_ASTEROID), row(12, 34, 4)];
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Unload(vec![(11, 1)])], "ore only");
        o.cargo.clear();
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Undock]);
    }

    /// **GAPS-B**: ore in the mining hold is unloaded with the cargo's.
    #[test]
    fn the_mining_hold_is_unloaded_with_the_cargo() {
        let mut o = docked_ready();
        o.cargo = vec![row(11, 1230, CATEGORY_ASTEROID)];
        let mut held = row(13, 1230, CATEGORY_ASTEROID);
        held.flag_id = FLAG_MINING_HOLD;
        o.ore_hold = vec![held];
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Unload(vec![(11, 1), (13, 1)])]);
        o.cargo.clear();
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Unload(vec![(13, 1)])], "the hold alone");
    }

    /// **GAPS-B**: a Venture in the hangar is boarded from a Merlin; one
    /// flown is kept with a Merlin beside it; and a closed-economy miner in
    /// a Merlin looks for the better hull once due.
    #[test]
    fn a_miner_boards_a_venture_over_a_merlin_and_a_closed_one_looks_for_it() {
        let venture = crate::huffer::VENTURE;
        let mut o = docked_ready();
        o.hangar = vec![row(SHIP, MERLIN, 6), row(30, venture, 6)];
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Board(30)]);

        o.ship = Some(ShipInfo {
            ship_id: 30,
            type_id: Some(venture),
        });
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert!(!kinds(&acts).contains(&ActKind::Board), "{acts:?}");

        let mut o = docked_ready();
        o.hangar = vec![row(SHIP, MERLIN, 6)];
        let mut m = fast("Miner1");
        m.policy.gm_setup = false;
        m.policy.closed = true;
        let (m, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![Act::Acquire {
                ship: true,
                module: false,
                ammo: false
            }]
        );
        let (_, acts) = decide(&m, &o);
        assert!(!kinds(&acts).contains(&ActKind::Acquire), "{acts:?}");
        // PROG-MINE: the ladder ranks the Venture above the Merlin and
        // below every barge and exhumer.
        assert!(miner_hull_rank(venture) < miner_hull_rank(MERLIN));
        assert_eq!(miner_hull_rank(MERLIN), Some(MINER_HULLS.len() - 1));
        assert_eq!(miner_hull_rank(crate::mine_ladder::HULK), Some(0));
    }

    /// **PROG-MINE**: a barge in the hangar is boarded from a Venture, a
    /// closed Venture pilot looks for a better hull, the best hull's pilot
    /// does not, and a barge takes a Strip Miner I where a Venture takes a
    /// Miner I.
    #[test]
    fn a_miner_climbs_the_ladder_and_fits_the_laser_that_suits_the_hull() {
        use crate::mine_ladder::{HULK, RETRIEVER, STRIP_MINER_I};
        let venture = crate::huffer::VENTURE;
        let mut o = docked_ready();
        o.hangar = vec![row(SHIP, venture, 6), row(31, RETRIEVER, 6)];
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(venture),
        });
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Board(31)]);

        // Flying the barge, the laser to fit is the strip miner, not the
        // Miner I in the hangar.
        let mut o = docked_ready();
        o.ship = Some(ShipInfo {
            ship_id: 31,
            type_id: Some(RETRIEVER),
        });
        o.hangar = vec![
            row(31, RETRIEVER, 6),
            row(66, MINER_I, 7),
            row(67, STRIP_MINER_I, 7),
        ];
        o.miner = None;
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::FitAuto(67)]);
        o.hangar = vec![row(31, RETRIEVER, 6), row(66, MINER_I, 7)];
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert!(!acts.contains(&Act::FitAuto(66)), "{acts:?}");

        // A closed Venture pilot looks; a Hulk pilot has nothing to look for.
        let mut m = fast("Miner1");
        m.policy.gm_setup = false;
        m.policy.closed = true;
        let mut o = docked_ready();
        o.hangar = vec![row(SHIP, venture, 6)];
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(venture),
        });
        let (_, acts) = decide(&m, &o);
        assert!(kinds(&acts).contains(&ActKind::Acquire), "{acts:?}");
        o.hangar = vec![row(SHIP, HULK, 6)];
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(HULK),
        });
        let (_, acts) = decide(&m, &o);
        assert!(!kinds(&acts).contains(&ActKind::Acquire), "{acts:?}");
    }

    /// **E8**: with the human policy's `sell_every`, the miner sells at most
    /// once per [`SELL_EVERY`]: a visit inside it goes out unsold, the first
    /// visit after it sells.
    #[test]
    fn the_miner_sells_at_most_once_per_sell_every() {
        let mut o = docked_ready();
        let mut ore = row(21, 1230, CATEGORY_ASTEROID);
        ore.singleton = false;
        ore.quantity = 500;
        o.hangar = vec![row(20, MERLIN, 6), ore];
        let mut m = fast("Miner1");
        m.policy.sell_every = SELL_EVERY;
        let start = o.now;
        let (m, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Sell]);
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Undock], "out unsold");
        // The next visits, inside the window: no sale.
        o.now = start + SELL_EVERY / 2;
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Undock]);
        o.now = start + SELL_EVERY - Duration::from_secs(1);
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Undock]);
        // The first visit after it: one sale.
        o.now = start + SELL_EVERY;
        let (m, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Sell]);
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Undock]);
        assert_eq!(
            Policy::human().sell_every,
            SELL_EVERY,
            "the person-paced bots"
        );
    }

    /// **E8**: in the closed economy, a miner in its capsule with nothing
    /// to board asks for an [`Act::Acquire`] instead of GM setup, then says
    /// `MISSING` until [`crate::acquire::ACQUIRE_EVERY`] has passed; a
    /// Merlin in the hangar is boarded from a rookie ship.
    #[test]
    fn a_closed_miner_acquires_then_says_missing_until_it_is_due_again() {
        let mut o = docked_ready();
        o.ship = Some(ShipInfo {
            ship_id: 9,
            type_id: Some(CAPSULE),
        });
        o.miner = None;
        o.hangar.clear();
        let mut m = fast("Miner1");
        m.policy.gm_setup = false;
        m.policy.closed = true;
        let (m, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![Act::Acquire {
                ship: true,
                module: true,
                ammo: false
            }]
        );
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Missing, ActKind::Wait]);
        o.now += crate::acquire::ACQUIRE_EVERY;
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Acquire]);
        // A rookie ship, and a Merlin bought: board it.
        o.ship = Some(ShipInfo {
            ship_id: 9,
            type_id: Some(990_870_001),
        });
        o.hangar = vec![row(20, MERLIN, 6)];
        let (_, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Board(20)]);
    }

    /// **FX6**: unloaded ore in the hangar is offered once a visit, then the
    /// miner goes out; the next visit offers again. A singleton (a hull) is
    /// not goods.
    #[test]
    fn hangar_goods_are_sold_once_a_visit_after_unloading() {
        let mut o = docked_ready();
        let mut ore = row(21, 1230, CATEGORY_ASTEROID);
        ore.singleton = false;
        ore.quantity = 500;
        o.hangar = vec![row(20, MERLIN, 6), ore];
        let (m, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Sell]);
        // Nobody bid: the stack is still there, and is not offered again.
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Undock]);
        assert!(!m.sold, "the next visit offers again");
        let (_, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Sell]);

        // Unloading comes first; a hull alone is nothing to sell.
        o.cargo = vec![row(11, 1230, CATEGORY_ASTEROID)];
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Unload]);
        o.cargo.clear();
        o.hangar = vec![row(20, MERLIN, 6)];
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Undock]);
    }

    #[test]
    fn home_full_with_nothing_to_unload_is_missing_not_a_loop() {
        let mut o = docked_ready();
        o.cargo = vec![row(12, 377, 7)];
        let mut m = fast("Miner1");
        m.homing = Some(Homing::HoldFull);
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Missing, ActKind::Wait]);
        // Said once; then the loop goes on.
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Undock]);
    }

    #[test]
    fn a_capsule_boards_the_merlin_in_the_hangar() {
        let mut o = docked_ready();
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(CAPSULE),
        });
        o.miner = None;
        o.hangar = vec![row(55, MERLIN, 6)];
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Board(55)]);
    }

    #[test]
    fn no_ship_and_no_gm_is_missing_never_setup() {
        let mut o = docked_ready();
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(CAPSULE),
        });
        o.miner = None;
        let mut m = fast("Miner1");
        m.policy.gm_setup = false;
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Missing, ActKind::Wait]);

        // With GM allowed: setup, once, and then Missing if it did not help.
        let (m, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(
            acts,
            vec![Act::GmSetup {
                ship: true,
                module: true,
                ammo: false
            }]
        );
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Missing, ActKind::Wait]);
    }

    #[test]
    fn a_hull_without_a_laser_fits_one_then_onlines_it() {
        let mut o = docked_ready();
        o.miner = None;
        o.hangar = vec![row(66, MINER_I, 7)];
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::FitAuto(66)]);

        let mut o = docked_ready();
        o.miner = Some(ModuleInfo {
            online: false,
            ..laser(false, None)
        });
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Online(LASER)]);
    }

    #[test]
    fn a_bot_dwells_before_undocking_when_its_policy_says_so() {
        let o = docked_ready();
        let m = Memory::new("Miner1", Policy::human());
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait]);
        let later = Observation {
            now: m.dwell_until.expect("a deadline"),
            ..o
        };
        let (_, acts) = decide(&m, &later);
        // MINER-DEFENSE: a miner at a person's pace stocks its defence drones
        // just before the undock.
        assert_eq!(kinds(&acts), vec![ActKind::Drone, ActKind::Undock]);
    }

    #[test]
    fn in_space_unbound_binds_first() {
        let mut o = in_space();
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: false,
        };
        o.park = None;
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Bind]);
    }

    #[test]
    fn a_claimed_rock_is_approached_locked_and_mined() {
        let o = in_space();
        let (m, acts) = decide(&fast("Miner1"), &o);
        let rock = m.claim.expect("claims a rock");
        assert_eq!(kinds(&acts), vec![ActKind::Follow], "20 km+: approach");

        // Beside it now.
        let mut o = in_space();
        park(&mut o).ego = ball(SHIP, if rock == ROCK_A { 18_000.0 } else { 38_000.0 });
        let (m, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Lock(rock)]);
        o.pending = vec![rock];
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait], "the lock is pending");
        o.pending.clear();
        o.locked = vec![rock];
        let (m, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![Act::Mine {
                module: LASER,
                rock
            }]
        );
        o.miner = Some(laser(true, Some(rock)));
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait], "mining");
    }

    const GEODE: i64 = 990_701;

    /// L7 BOT-ECON: a miner with a geode aboard goes home when its risk
    /// trait says the crate is worth the trip, or a human is on the grid, and
    /// otherwise carries on cutting.
    #[test]
    fn a_miner_with_a_geode_aboard_docks_by_its_risk() {
        let table = std::sync::Arc::new(crate::geode_haul::Table::with_geodes(&[(GEODE, 1)]));
        let mut m = fast("Miner1");
        m.policy.econ = table;
        let mut o = in_space();
        o.cargo = vec![ItemRow {
            quantity: 1,
            ..row(100_000_900, GEODE, 5)
        }];
        // The careful dock for the smallest geode ...
        o.hunt.temper.risk = 0.0;
        let (home, acts) = decide(&m, &o);
        assert_eq!(home.homing, Some(Homing::Geode), "{acts:?}");
        assert!(
            home.dock_reason().contains("geode"),
            "{}",
            home.dock_reason()
        );
        // ... the reckless cut on.
        o.hunt.temper.risk = 1.0;
        let (cut, _) = decide(&m, &o);
        assert_eq!(cut.homing, None);
        // A human on the grid ends it whatever the trait.
        o.humans = vec![ball(100_000_950, 5_000.0)];
        let (home, _) = decide(&m, &o);
        assert_eq!(home.homing, Some(Homing::Geode));
        // No geode, no reason.
        let mut o = in_space();
        o.hunt.temper.risk = 0.0;
        let (none, _) = decide(&m, &o);
        assert_eq!(none.homing, None);
    }

    /// L7 BOT-ECON: a docked miner flies to a motherlode it heard in Local,
    /// once, within its risk's reach; a lawful bot with nothing heard stays.
    #[test]
    fn a_docked_miner_flies_to_a_motherlode_it_heard() {
        use crate::chat::RoomKind;
        let mut map = crate::travel::Map::new();
        let (a, b) = (3_000_001, 3_000_002);
        map = map
            .with_gate(a, 50_001, b, 50_002)
            .with_gate(b, 50_002, a, 50_001)
            .with_security(a, 0.9)
            .with_security(b, 0.9)
            .with_station(STATION, a)
            .with_station(60_000_002, b);
        let mut m = fast("Miner1");
        m.policy.travel = Some(std::sync::Arc::new(map));
        let mut o = docked_ready();
        o.hunt.temper.risk = 0.5;
        assert!(to_motherlode(&mut m, &o).is_none(), "nothing heard");
        o.talk.heard = vec![crate::chatter::Line {
            seq: 1,
            room: RoomKind::Local,
            from: 90_000_777,
            text: eve_mod_econ::motherlode::local_line("Testia", b, false),
            at: o.now,
            human: false,
        }];
        m.heard.hear(&o.talk.heard, o.now);
        assert_eq!(
            to_motherlode(&mut m, &o),
            Some(vec![Act::TravelTo { system: b }])
        );
        assert!(to_motherlode(&mut m, &o).is_none(), "once");
    }

    /// L7b: a docked escort outside a fleet takes the foreman's ask from the
    /// board and flies to its system; a miner does not; in a fleet it lets go.
    #[test]
    fn a_docked_escort_takes_a_foremans_ask_and_flies_there() {
        let mut map = crate::travel::Map::new();
        let (a, b) = (3_100_001, 3_100_002);
        map = map
            .with_gate(a, 51_001, b, 51_002)
            .with_gate(b, 51_002, a, 51_001)
            .with_security(a, 0.9)
            .with_security(b, 0.3)
            .with_station(STATION, a);
        let (foreman, escort) = (8_100_001, 8_100_002);
        let mut o = docked_ready();
        o.me = escort;
        o.hunt.temper.risk = 0.5;
        let mut miner = fast("Miner2");
        miner.policy.travel = Some(std::sync::Arc::new(map.clone()));
        let mut m = Memory::new(
            "Escort2",
            Policy {
                role: Role::Escort,
                ..Policy::fast()
            },
        );
        m.policy.travel = Some(std::sync::Arc::new(map));
        // The default floor keeps an escort out of low sec: it could not fly there.
        assert!(to_escort_job(&mut m, &o).is_none(), "nothing on the board");
        m.policy.security_floor = 0.1;
        crate::escort_board::with(|b| b.post(foreman, 3_100_002, Some(5), o.now));
        o.me = 0;
        assert!(to_escort_job(&mut m, &o).is_none(), "no character yet");
        o.me = escort;
        assert!(
            to_escort_job(&mut miner, &o).is_none(),
            "a miner is no escort"
        );
        assert_eq!(
            to_escort_job(&mut m, &o),
            Some(vec![Act::TravelTo { system: b }])
        );
        assert_eq!(m.job.belt(), Some(5));
        o.fleet = Some(FleetView {
            fleet_id: 1,
            boss: None,
        });
        assert!(to_escort_job(&mut m, &o).is_none());
        assert!(m.job.job.is_none(), "in a fleet the job is released");
        crate::escort_board::with(|b| b.withdraw(foreman));
    }

    /// BOT-CAP: a miner with the capacitor out does not ask for the laser
    /// again until it has recovered; the rock stays locked.
    #[test]
    fn a_drained_miner_holds_its_laser_until_the_capacitor_is_back() {
        let mut o = in_space();
        let mut m = fast("Miner1");
        m.claim = Some(ROCK_A);
        park(&mut o).ego = ball(SHIP, 15_000.0);
        o.locked = vec![ROCK_A];
        let (m, acts) = decide(&m, &o);
        assert!(
            acts.iter().any(|a| matches!(a, Act::Mine { .. })),
            "{acts:?}"
        );
        o.cap = crate::cap::Scene {
            fraction: Some(0.01),
            drained: true,
            hold: true,
            neuts: Vec::new(),
        };
        let (m, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Wait(crate::cap::HOLD_WAIT)]);
        o.cap = crate::cap::Scene::default();
        let (_, acts) = decide(&m, &o);
        assert!(
            acts.iter().any(|a| matches!(a, Act::Mine { .. })),
            "{acts:?}"
        );
    }

    /// **The laser never starts out of range**, locked or not: with the rock
    /// already locked, a miner beyond its laser's reach approaches
    /// ([`Act::Follow`]) and does not ask to mine; and a `TargetTooFar` stop
    /// (the server's refusal, or a cycle that ended out of range) is an
    /// approach, not a failure.
    #[test]
    fn a_locked_rock_beyond_the_lasers_reach_is_approached_not_mined() {
        let mut o = in_space();
        let mut m = fast("Miner1");
        m.claim = Some(ROCK_A);
        o.locked = vec![ROCK_A];
        // 19 km from the rock's surface; the laser reaches 10 km.
        park(&mut o).ego = ball(SHIP, 1_000.0);
        let (m, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![Act::Follow {
                id: ROCK_A,
                range: APPROACH_RANGE
            }],
            "{acts:?}"
        );
        assert!(!acts.iter().any(|a| matches!(a, Act::Mine { .. })));

        o.last_stop = Some(Stop {
            seq: 1,
            reason: StopReason::TooFar,
        });
        let (m, acts) = decide(&m, &o);
        assert!(
            acts.iter()
                .any(|a| matches!(a, Act::Follow { id, .. } if *id == ROCK_A)),
            "{acts:?}"
        );
        assert!(
            m.homing.is_none(),
            "an out-of-range stop is not a reason to go home"
        );
    }

    #[test]
    fn a_far_rock_is_warped_to() {
        let mut o = in_space();
        park(&mut o).rocks = vec![ball(ROCK_A, 1_000_000.0)];
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(kinds(&acts), vec![ActKind::WarpToItem]);
    }

    #[test]
    fn a_depleted_rock_is_dropped_for_another() {
        let mut o = in_space();
        let mut m = fast("Miner1");
        m.claim = Some(ROCK_A);
        o.locked = vec![ROCK_A];
        park(&mut o).ego = ball(SHIP, 18_000.0);
        // Mined out: the stop says so and the rock left the park.
        o.last_stop = Some(Stop {
            seq: 1,
            reason: StopReason::Depleted,
        });
        park(&mut o).rocks.retain(|r| r.id != ROCK_A);
        let (m, acts) = decide(&m, &o);
        assert_eq!(m.claim, Some(ROCK_B), "a different rock");
        assert_eq!(kinds(&acts), vec![ActKind::Follow]);
    }

    #[test]
    fn a_full_hold_goes_home_warp_far_dock_near() {
        let mut o = in_space();
        o.last_stop = Some(Stop {
            seq: 1,
            reason: StopReason::HoldFull,
        });
        park(&mut o).station = Some(ball(STATION, -1_000_000.0));
        let (m, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(
            acts,
            vec![Act::WarpToItem {
                id: STATION,
                range: 0.0
            }]
        );
        assert_eq!(m.homing, Some(Homing::HoldFull));

        park(&mut o).station = Some(ball(STATION, -500.0));
        let (m, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Dock(STATION)]);

        park(&mut o).station = Some(ball(STATION, -50_000.0));
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Follow], "in between: approach");
    }

    #[test]
    fn going_home_stops_the_laser_first_and_only_once() {
        let mut o = in_space();
        o.miner = Some(laser(true, Some(ROCK_A)));
        o.last_stop = Some(Stop {
            seq: 1,
            reason: StopReason::HoldFull,
        });
        let (m, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::StopMining(LASER)]);
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait], "not sent twice");
    }

    #[test]
    fn an_empty_belt_goes_home_and_dwells_longer() {
        let mut o = in_space();
        park(&mut o).rocks.clear();
        let (m, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Dock(STATION)]);
        assert!(m.idle_next);
    }

    /// **Off a belt's grid there are no rocks, but there is the belt**
    /// (GRID-B): a miner with an empty park warps to a belt, waits there for
    /// the grid's rocks, and only then calls the belt empty and goes home.
    #[test]
    fn with_no_rock_in_the_park_a_miner_warps_to_a_belt_first() {
        const BELT: i64 = 40_000_201;
        let mut o = in_space();
        park(&mut o).rocks.clear();
        park(&mut o).belts = vec![ball(BELT, 1.0e10)];
        let (mut m, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(
            acts,
            vec![Act::WarpToItem {
                id: BELT,
                range: MINING_STANDOFF
            }]
        );
        park(&mut o).ego = ball(SHIP, 1.0e10 - 10_000.0);
        let mut waited = 0;
        loop {
            let (next, acts) = decide(&m, &o);
            m = next;
            if acts == vec![Act::Wait(SHORT_WAIT)] {
                waited += 1;
                assert!(waited <= BELT_WAITS, "waits for ever");
                continue;
            }
            assert_eq!(kinds(&acts), vec![ActKind::WarpToItem], "home: {acts:?}");
            break;
        }
        assert!(waited > 0, "gave the grid a moment");
        assert!(m.idle_next);
    }

    /// **A belt found empty is not picked again next trip** (belt-econ P2).
    #[test]
    fn a_belt_found_empty_is_not_picked_again_next_trip() {
        const A: i64 = 40_000_301;
        const B: i64 = 40_000_302;
        let mut o = in_space();
        park(&mut o).rocks.clear();
        park(&mut o).belts = vec![ball(A, 1.0e10), ball(B, 2.0e10)];
        let now = o.now;
        let mut m = fast("Miner1");
        let first = loop {
            let acts = to_a_belt(&mut m, park(&mut o), now);
            match acts {
                Some(_) => {
                    if let Some(b) = m.belt {
                        park(&mut o).ego = ball(SHIP, ball_x(park(&mut o), b) - 10_000.0);
                    }
                }
                None => break m.belt.expect("a belt"),
            }
        };
        // Home, and out again with no rock in the park.
        m.belt = None;
        m.belt_waits = 0;
        park(&mut o).ego = ball(SHIP, 0.0);
        to_a_belt(&mut m, park(&mut o), now).expect("another belt");
        assert_ne!(m.belt, Some(first), "the dry belt again");
        // Both dry: nowhere to go.
        m.dry = vec![(A, now + DRY_FOR), (B, now + DRY_FOR)];
        m.belt = None;
        assert!(to_a_belt(&mut m, park(&mut o), now).is_none());
        // Forgotten after DRY_FOR.
        assert!(to_a_belt(&mut m, park(&mut o), now + DRY_FOR).is_some());
    }

    fn ball_x(p: &ParkView, id: i64) -> f64 {
        p.belts.iter().find(|b| b.id == id).expect("belt").position[0]
    }

    /// SOC-B10: a miner with ore anomalies and drones on.
    fn ore_miner(name: &str) -> Memory {
        let mut p = Policy::fast();
        p.ore_anomalies = true;
        p.mining_drones = true;
        Memory::new(name, p)
    }

    /// **An ore anomaly before a belt, once a trip** (SOC-B10): no rock in
    /// the park asks for one site, waits for its rocks, then goes to a belt
    /// as before; docking makes the next trip try again.
    #[test]
    fn an_ore_miner_tries_an_anomaly_before_a_belt_once_a_trip() {
        const BELT: i64 = 40_000_201;
        let mut o = in_space();
        park(&mut o).rocks.clear();
        park(&mut o).belts = vec![ball(BELT, 1.0e10)];
        let (mut m, acts) = decide(&ore_miner("Miner1"), &o);
        assert_eq!(acts, vec![Act::Ore(crate::ore::OreAct::Site)]);
        for _ in 0..BELT_WAITS {
            let (next, acts) = decide(&m, &o);
            m = next;
            assert_eq!(acts, vec![Act::Wait(SHORT_WAIT)]);
        }
        let (m, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![Act::WarpToItem {
                id: BELT,
                range: MINING_STANDOFF
            }],
            "no site's rocks came: a belt"
        );
        let (m, _) = decide(&m, &docked_ready());
        assert_eq!(m.ore_site, crate::ore::SiteStep::Untried);
    }

    /// **The drones go onto the laser's rock and come home first** (SOC-B10).
    #[test]
    fn an_ore_miner_launches_drones_at_its_rock_and_recalls_them_going_home() {
        let mut o = in_space();
        park(&mut o).ego = ball(SHIP, 18_000.0);
        o.locked = vec![ROCK_A];
        o.miner = Some(laser(true, Some(ROCK_A)));
        let mut m = ore_miner("Miner1");
        m.claim = Some(ROCK_A);
        let (mut m, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Ore(crate::ore::OreAct::Mine(ROCK_A))]);
        // The arm launched one.
        m.drones = vec![100_000_777];
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait], "mining, drones and all");
        // The hold is full and the laser has stopped: drones first, then
        // the warp home.
        o.miner = Some(laser(false, None));
        o.last_stop = Some(Stop {
            seq: 1,
            reason: StopReason::HoldFull,
        });
        park(&mut o).station = Some(ball(STATION, -1_000_000.0));
        let (mut m, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Ore(crate::ore::OreAct::Recall)]);
        m.drones.clear();
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::WarpToItem]);
    }

    #[test]
    fn three_failures_of_one_kind_recover() {
        // In space, a non-travel kind: home.
        let mut o = in_space();
        o.failures.insert(ActKind::Lock, GIVE_UP_AFTER);
        let (m, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Dock(STATION)]);
        assert_eq!(m.homing, Some(Homing::Recovery));
        // Two is not three.
        o.failures.insert(ActKind::Lock, GIVE_UP_AFTER - 1);
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_ne!(acts, vec![Act::Dock(STATION)]);

        // In space, travel failing: reconnect.
        let mut o = in_space();
        o.failures.insert(ActKind::Dock, GIVE_UP_AFTER);
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Reconnect]);

        // Docked: reconnect.
        let mut o = docked_ready();
        o.failures.insert(ActKind::Undock, GIVE_UP_AFTER);
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(acts, vec![Act::Reconnect]);
    }

    #[test]
    fn too_far_resyncs_then_approaches() {
        let mut o = in_space();
        let mut m = fast("Miner1");
        m.claim = Some(ROCK_A);
        o.last_stop = Some(Stop {
            seq: 1,
            reason: StopReason::TooFar,
        });
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Resync, ActKind::Follow]);
        m.policy.resync = false;
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Follow]);
    }

    /// BOT-REDOCK: a refused warp to the boss (docked, or too close) is not
    /// a failure -- three of them sent a dev-roster miner home on Recovery,
    /// over and over -- and the bot leaves him alone for [`BOSS_RETRY`].
    #[test]
    fn a_refused_warp_to_the_boss_is_no_failure_and_backs_off() {
        assert!(ActKind::WarpToMember.is_incidental());
        let mut o = in_space();
        o.fleet = Some(FleetView {
            fleet_id: 7,
            boss: Some(90_000_009),
        });
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(
            kinds(&acts),
            vec![ActKind::WarpToMember],
            "no boss ball: away"
        );
        o.boss_unreachable_until = Some(o.now + BOSS_RETRY);
        let (m, acts) = decide(&fast("Miner1"), &o);
        assert!(m.homing.is_none(), "backing off is not homing");
        assert!(
            !acts.iter().any(|a| matches!(a, Act::WarpToMember { .. })),
            "no new warp to him while he is unreachable"
        );
        o.now += BOSS_RETRY;
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(
            kinds(&acts),
            vec![ActKind::WarpToMember],
            "then he is tried again"
        );
    }

    fn jittery(name: &str) -> Memory {
        Memory::new(name, Policy::human())
    }

    fn invited() -> Observation {
        let mut o = docked_ready();
        o.invite = Some(Invite {
            fleet_id: 7,
            inviter_id: 90_000_009,
            inviter_name: Some("Alice".to_owned()),
        });
        o
    }

    /// **REACT-JITTER**: the first look at an invite waits, within
    /// [`crate::react::LO`]..[`crate::react::HI`], then it is accepted.
    #[test]
    fn an_invite_is_accepted_after_a_think_delay_not_at_once() {
        let mut o = invited();
        let (m, acts) = decide(&jittery("Vela Orsk"), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait], "{acts:?}");
        o.now += crate::react::HI + Duration::from_millis(1);
        let (_, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::AcceptInvite { fleet: 7 }]);
    }

    #[test]
    fn the_think_delay_of_a_first_reaction_is_in_range_and_the_second_is_free() {
        use crate::react::{Class, HI, LO};
        let o = invited();
        let mut m = jittery("Vela Orsk");
        let left = m.thinking(Class::Invite, 7, &o).expect("delayed");
        assert!((LO..=HI).contains(&left), "{left:?}");
        // The same event a moment later is the same think, not a new one.
        let mut o2 = o.clone();
        o2.now += left;
        assert_eq!(m.thinking(Class::Invite, 7, &o2), None);
        o2.now += Duration::from_secs(3);
        assert_eq!(m.thinking(Class::Invite, 7, &o2), None, "second is free");
    }

    #[test]
    fn bots_think_for_different_times_and_a_fast_policy_not_at_all() {
        use crate::react::Class;
        let o = invited();
        let mut seen: Vec<Duration> = (0..20)
            .map(|i| {
                jittery(&format!("Pilot{i:02}"))
                    .thinking(Class::Invite, 7, &o)
                    .expect("delayed")
            })
            .collect();
        seen.sort();
        seen.dedup();
        assert!(seen.len() >= 15, "{seen:?}");
        assert_eq!(fast("Miner1").thinking(Class::Invite, 7, &o), None);
    }

    /// Under fire the delay is at most the low end.
    #[test]
    fn a_bot_under_fire_thinks_no_longer_than_the_low_end() {
        use crate::react::{Class, LO};
        let mut o = invited();
        o.threat.foes = vec![crate::threat::Foe {
            char_id: 90_000_001,
            cause: crate::threat::Cause::Shot,
        }];
        for i in 0..20 {
            let left = jittery(&format!("Pilot{i:02}"))
                .thinking(Class::Threat, 1, &o)
                .expect("delayed");
            assert!(left <= LO, "{left:?}");
        }
    }

    /// Following the fleet boss is never delayed.
    #[test]
    fn fleet_follow_is_not_delayed() {
        let mut o = in_space();
        o.fleet = Some(FleetView {
            fleet_id: 7,
            boss: Some(90_000_009),
        });
        park(&mut o).boss = Some(ball(100_000_009, 1_000_000.0));
        let (_, acts) = decide(&jittery("Vela Orsk"), &o);
        assert_eq!(kinds(&acts), vec![ActKind::WarpToMember]);
    }

    /// The miner's lock on a new rock waits once, then goes.
    #[test]
    fn a_new_rock_is_locked_after_a_think() {
        let mut o = in_space();
        park(&mut o).ego = ball(SHIP, 18_000.0);
        let mut m = jittery("Vela Orsk");
        m.claim = Some(ROCK_A);
        let (m1, acts1) = decide(&m, &o);
        assert!(
            !acts1.iter().any(|a| matches!(a, Act::Lock(_))),
            "{acts1:?}"
        );
        let mut o2 = o.clone();
        o2.now += crate::react::HI + Duration::from_millis(1);
        let (_, acts2) = decide(&m1, &o2);
        assert!(acts2.iter().any(|a| matches!(a, Act::Lock(_))), "{acts2:?}");
    }

    #[test]
    fn the_boss_more_than_a_warp_away_is_followed() {
        let mut o = in_space();
        o.fleet = Some(FleetView {
            fleet_id: 7,
            boss: Some(90_000_009),
        });
        park(&mut o).boss = Some(ball(100_000_009, 1_000_000.0));
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(
            kinds(&acts),
            vec![ActKind::WarpToMember],
            "the boss is {} km off",
            1_000
        );
        let Act::WarpToMember { char_id, .. } = acts[0] else {
            unreachable!()
        };
        assert_eq!(char_id, 90_000_009, "a character id, not a ship id");

        // Near the boss: the rocks nearest *him* are the candidates.
        let mut o = in_space();
        o.fleet = Some(FleetView {
            fleet_id: 7,
            boss: Some(90_000_009),
        });
        park(&mut o).boss = Some(ball(100_000_009, 41_000.0));
        park(&mut o).rocks = vec![ball(ROCK_A, 20_000.0), ball(ROCK_B, 40_000.0)];
        let pick = pick_rock(0, o.park.as_ref().expect("park"), &ball(0, 41_000.0));
        assert_eq!(pick, Some(ROCK_B), "seed 0 takes the nearest to the boss");

        // Beside a boss with no rock within a warp: wait with him, rather
        // than warp back to the old belt.
        let mut o = in_space();
        o.fleet = Some(FleetView {
            fleet_id: 7,
            boss: Some(90_000_009),
        });
        park(&mut o).ego = ball(SHIP, 1_000_000.0);
        park(&mut o).boss = Some(ball(100_000_009, 1_001_000.0));
        let (m, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait]);
        assert_eq!(m.claim, None);
    }

    #[test]
    fn two_bots_on_one_rock_the_higher_ship_yields() {
        let mut o = in_space();
        park(&mut o).ego = ball(SHIP, 18_000.0);
        o.locked = vec![ROCK_A];
        o.miner = Some(laser(true, Some(ROCK_A)));
        park(&mut o).taken.insert(ROCK_A, SHIP - 1);
        let mut m = fast("Miner1");
        m.claim = Some(ROCK_A);
        let (m2, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::StopMining(LASER)]);
        assert_eq!(m2.claim, None);

        // The lower id keeps it.
        park(&mut o).taken.insert(ROCK_A, SHIP + 1);
        let (m3, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait]);
        assert_eq!(m3.claim, Some(ROCK_A));
    }

    #[test]
    fn a_free_rock_is_preferred_and_names_spread() {
        let o = in_space();
        let mut p = o.park.clone().expect("park");
        p.taken.insert(ROCK_A, SHIP + 5);
        for seed in 0..10 {
            assert_eq!(pick_rock(seed, &p, &p.ego), Some(ROCK_B), "A is taken");
        }
        p.taken.insert(ROCK_B, SHIP + 6);
        assert_eq!(pick_rock(0, &p, &p.ego), None, "everything is taken");
        // Different seeds choose differently among free rocks.
        p.taken.clear();
        let picks: Vec<Option<i64>> = (0..2).map(|s| pick_rock(s, &p, &p.ego)).collect();
        assert_ne!(picks[0], picks[1]);
    }

    #[test]
    fn warping_is_waiting() {
        let mut o = in_space();
        park(&mut o).ego_warping = true;
        let (_, acts) = decide(&fast("Miner1"), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait]);
    }

    #[test]
    fn a_stop_is_acted_on_once() {
        let mut o = in_space();
        o.last_stop = Some(Stop {
            seq: 3,
            reason: StopReason::TooFar,
        });
        let mut m = fast("Miner1");
        m.claim = Some(ROCK_A);
        let (m, first) = decide(&m, &o);
        let (_, second) = decide(&m, &o);
        assert!(first.contains(&Act::Resync));
        assert!(!second.contains(&Act::Resync));
    }

    #[test]
    fn stop_keys_read_as_reasons() {
        assert_eq!(
            StopReason::from_key(Some("MiningItemDepleted")),
            StopReason::Depleted
        );
        assert_eq!(
            StopReason::from_key(Some("MiningDronesDeactivatedCargoHoldNowFull")),
            StopReason::HoldFull
        );
        assert_eq!(
            StopReason::from_key(Some("TargetTooFar")),
            StopReason::TooFar
        );
        assert_eq!(StopReason::from_key(None), StopReason::Stopped);
    }

    /// `in_space()` with one human ship `gap` metres (surface) past the
    /// bot, on the far side from the station.
    fn with_human(gap: f64) -> Observation {
        let mut o = in_space();
        let ego = park(&mut o).ego;
        o.humans = vec![Ball {
            id: 100_000_900,
            position: [ego.position[0] + gap + 2.0 * ego.radius, 0.0, 0.0],
            radius: ego.radius,
        }];
        o
    }

    fn playing(role: RoleRef) -> Memory {
        Memory::new(
            "Bot1",
            Policy {
                role,
                courtesy: true,
                ..Policy::fast()
            },
        )
    }

    /// Ruling 5, CP39: every civilian row goes home -- the very acts
    /// [`go_home`] gives -- when a human outside its fleet is within
    /// [`COURTESY_RANGE`], and dwells the longer dwell after; past the range
    /// it plays on as if nobody were there.
    #[test]
    fn a_civilian_role_yields_when_a_non_fleet_human_is_within_courtesy_range() {
        let civilians: Vec<RoleRef> = roles::ROLES
            .iter()
            .copied()
            .filter(|r| r.civilian)
            .collect();
        assert!(!civilians.is_empty());
        for role in civilians {
            let near = with_human(COURTESY_RANGE / 2.0);
            let (m, acts) = decide(&playing(role), &near);
            let home = go_home(&mut playing(role), &near, near.park.as_ref().unwrap());
            assert_eq!(acts, home, "{role:?}");
            assert_eq!(m.homing, Some(Homing::Courtesy), "{role:?}");
            assert!(m.idle_next && m.claim.is_none(), "{role:?}");

            let far = with_human(COURTESY_RANGE * 2.0);
            let (m_far, acts_far) = decide(&playing(role), &far);
            let (m_none, acts_none) = decide(&playing(role), &in_space());
            assert_eq!(acts_far, acts_none, "{role:?}");
            assert_ne!(m_far.homing, Some(Homing::Courtesy), "{role:?}");
            assert_eq!(m_far.homing, m_none.homing, "{role:?}");
        }
    }

    /// BOT-CHURN: a dev roster's bots never step aside -- the one human near
    /// them is the owner, watching -- so they decide as if nobody were there.
    #[test]
    fn a_dev_roster_bot_does_not_yield_to_the_human_watching() {
        let dev = std::sync::Arc::new(crate::dev::DevBot {
            group: "invented".to_owned(),
            ticker: "INV".to_owned(),
            skills: crate::dev::Skills::All5,
            skill_ids: None,
            hull: None,
            home: None,
            boss: false,
            hall: std::sync::Arc::default(),
        });
        for role in roles::ROLES.iter().copied().filter(|r| r.civilian) {
            let mut m = playing(role);
            m.policy.dev = Some(dev.clone());
            let near = with_human(COURTESY_RANGE / 4.0);
            let (m_near, acts) = decide(&m, &near);
            let (m_none, acts_none) = decide(&m, &in_space());
            assert_eq!(acts, acts_none, "{role:?}");
            assert_eq!(m_near.homing, m_none.homing, "{role:?}");
            assert_ne!(m_near.homing, Some(Homing::Courtesy), "{role:?}");
        }
    }

    /// BOT-CHURN: a dock names its reason: courtesy says how far the human
    /// is; a homing with no numbers says its name.
    #[test]
    fn the_dock_reason_names_the_homing_and_courtesys_distance() {
        let near = with_human(COURTESY_RANGE / 2.0);
        let (m, _) = decide(&playing(&roles::MINE), &near);
        let why = m.dock_reason();
        assert!(why.starts_with("courtesy: a human"), "{why}");
        assert!(why.contains("km away"), "{why}");
        let mut m = playing(&roles::MINE);
        assert_eq!(m.dock_reason(), "no homing reason set");
        m.homing = Some(Homing::HoldFull);
        assert_eq!(m.dock_reason(), "homing HoldFull");
        // A courtesy note left over does not name another reason.
        m.home_why = Some((Homing::Courtesy, "courtesy: old".to_owned()));
        assert_eq!(m.dock_reason(), "homing HoldFull");
    }

    /// Ruling 5: a row not marked civilian decides exactly as it would with
    /// no human in the park, however close one is.
    #[test]
    fn a_non_civilian_role_never_yields() {
        let fighters: Vec<RoleRef> = roles::ROLES
            .iter()
            .copied()
            .filter(|r| !r.civilian)
            .collect();
        assert!(!fighters.is_empty());
        for role in fighters {
            for gap in [0.0, COURTESY_RANGE / 2.0, COURTESY_RANGE] {
                let (m, acts) = decide(&playing(role), &with_human(gap));
                let (m_none, acts_none) = decide(&playing(role), &in_space());
                assert_eq!(acts, acts_none, "{role:?} at {gap}");
                assert_eq!(m, m_none, "{role:?} at {gap}");
            }
        }
    }

    /// CHATTER-2: the farewell kit speaks only to an audience, in space, by
    /// the governor's rules, and hands its advanced state back.
    #[test]
    fn the_farewell_kit_follows_the_governor() {
        let m = fast("Leaver");
        let mut o = in_space();
        o.talk.sociability = 1.0;
        o.talk.system_name = Some("Testville".to_owned());
        // Nobody in Local: silent.
        let mut kit = FarewellKit::of(&m, &o);
        assert_eq!(kit.line(o.now), None);
        o.talk.local_humans = 1;
        let mut said = Vec::new();
        for seed in 0..60u64 {
            let mut m = fast("Leaver");
            m.chatter = crate::chatter::State::new(seed);
            let mut kit = FarewellKit::of(&m, &o);
            if let Some(l) = kit.line(o.now) {
                assert!(!l.contains('{'), "{l}");
                // The governor remembers it: no second line at once.
                assert_eq!(kit.line(o.now), None);
                assert_ne!(kit.state, m.chatter);
                said.push(l);
            }
        }
        assert!(!said.is_empty(), "a farewell is sometimes said");
        assert!(said.len() < 60, "and not always");
    }
}
