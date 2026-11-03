//! **The hunter** (SOC-C5, R-SOCIETY §3.6): a bot that fights the pilots it
//! is not allied with -- bots and humans alike -- in low sec, null sec and
//! wormhole space. Opt-in with `eve-bots --society-pvp`.
//!
//! **The owner's rule** (tracker §84): bots hunt *anyone not allied*,
//! humans included, where there is no law: [`Ground::is_ground`] is
//! `ZeroSec` or `LowSec` by the client's own class ([`SecurityClass`]), or
//! a wormhole or void system by its id. High sec stays safe until wars and
//! CONCORD exist (a later packet). A system whose security nothing says is
//! not hunting ground.
//!
//! **Who is allied** is C2's pure rule over the groups' books, read through
//! the society's hall ([`crate::corpmind::Hall::hostile_to`]); without a
//! hall, only the same player corp or alliance is allied
//! ([`crate::society::hostile_to`] with empty books). The driver evaluates
//! it per pilot on the grid ([`observe`]) and hands the brain
//! [`Pilot::hostile`] and the relation [`Pilot::relation`], so the brain
//! stays pure.
//!
//! **What the brain does** (the rules below are pure and tested):
//!
//! 1. *Prey* ([`choose_prey`]): a hostile pilot on the grid whose hull the
//!    bot can rate ([`Ground::class_of`]; not a capsule), no more than one
//!    size above its own and then only for the reckless
//!    ([`UP_AT`]), when the *odds* of the whole grid ([`odds`]: its side's
//!    summed [`power`] over every hostile pilot's, allies counted) reach
//!    the bar its aggression sets ([`required_odds`]). Among those, by
//!    [`score`]: the more negative the society's relation `R` toward the
//!    pilot's group, the weaker the hull, the nearer; the pilot already
//!    engaged stays prey while the rules allow it.
//! 2. *Retreat* ([`retreat`]): armour below [`armour_floor`], hull below
//!    [`hull_floor`], or odds under [`ABORT_ODDS`] of the bar -- all
//!    stricter for the timid -- set the way home for repair, and the bot
//!    warps off. The escort's own repair rule is shown no damage during a
//!    fight so that these thresholds, not its constants, decide.
//! 3. *The fight* is the escort's ([`crate::escort::in_space`]), shown the
//!    prey as the only hostile, exactly as the hostile bot does
//!    ([`crate::hostile`]); tackle modules fire first ([`tackle_first`]); a
//!    bot that commands a fleet ([`crate::fleet::Command`]) calls the prey
//!    as the primary first; [`guard`] drops every lock, orbit and shot at
//!    any other pilot.
//! 4. *The wreck* of a pilot it killed is looted by the escort's own loot
//!    step, shown pilots' wrecks too ([`Scene::wrecks`]).
//! 5. *Nobody here*: a directional scan ([`HuntAct::DScan`], [`sweep`]) --
//!    one sphere over the whole system, then a narrow cone at each belt
//!    ([`DSCAN_NARROW`]), the way a player triangulates -- then a warp to
//!    the belt that sees pilots ([`pick_belt`]). A quiet system sends the
//!    bot home ([`Homing::NoRocks`]) and, docked, to another hunting-ground
//!    system ([`HuntAct::Roam`], [`roam_target`]).
//! 6. *Every fight is the society's* ([`settle`]): the killmails the bot is
//!    on (`charMgr.GetRecentShipKillsAndLosses`, C6) become
//!    `Event::Kill`s through [`crate::corpmind::Hall::record_kill`], once
//!    per report whoever reads it first, and the allies on grid at a kill
//!    each an `Event::AllyFight`.
//!
//! **Why a role** (the packet left the choice): the `hunter` role plays the
//! `pirate` career ([`crate::career`]), blocked unless `--society-pvp`. A
//! ratter or an escort never leaves its belt on its own, while hunting is
//! a way of life with its own kit, travel and risk; keeping it apart also
//! keeps the escort's calm tests and the fleet escorts' behaviour as they
//! were. Aggressive minds draw it by their traits.
//!
//! **Every rule and number here is [A] policy**, each with its falsifier
//! at the constant. The client's own calls cite their source.

use crate::brain::{
    self, Act, Ball, GunInfo, Health, Homing, Hostile, Memory, Observation, ParkView, Place,
};
use crate::corpmind::Hall;
use crate::hostile::Class;
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::society::{self, Group};
use crate::travel::{Map, Trips};
use crate::{Conn, Error, ObjectRef, Result, escort};
use eve_marshal::PyValue;
use eve_server::celestial::{
    SecurityClass, Universe, VOID_SYSTEMS, WORMHOLE_SYSTEMS, pseudo_security,
};
use eve_server::group_contacts::Affiliation;
use eve_server::typedata;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Constants (all [A] unless a source is named)
// ---------------------------------------------------------------------------

/// A Warp Scrambler I **[V-data]** (`docs/content/R-TACKLE.md` §1.3).
pub const SCRAMBLER: i64 = 447;
/// A Warp Scrambler II **[V-data]**.
pub const SCRAMBLER_II: i64 = 448;
/// A Warp Disruptor I **[V-data]**.
pub const DISRUPTOR: i64 = 3242;
/// A Warp Disruptor II **[V-data]**.
pub const DISRUPTOR_II: i64 = 3244;
/// A Stasis Webifier I **[V-data]** (K-EWAR; effect `remoteWebifierFalloff`).
pub const STASIS_WEBIFIER: i64 = 526;
/// A Stasis Webifier II **[V-data]**.
pub const STASIS_WEBIFIER_II: i64 = 527;
/// Propulsion Jamming **[V-data]** (`skills.tsv`): the skill a scrambler
/// and a web ask for.
pub const SKILL_PROPULSION_JAMMING: i64 = 3435;

/// Prototype, Improved and Covert Ops Cloaking Device **[V-data]**
/// (`r-pvp-spec` §1.1): the cloaks a hunter fits when its stock holds one.
pub const CLOAK_TYPES: [i64; 3] = [11370, 11577, 11578];

/// Which half of the tackle a module is: it stops the warp, or it slows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TackleKind {
    Warp,
    Web,
}

/// What kind of tackle `type_id` is, `None` for anything else: a scrambler
/// or a disruptor (`tackle::PLAYER_EFFECTS`) stops the warp, a stasis
/// webifier (`ewar::WEB`) slows the prey.
pub fn tackle_kind(type_id: i64) -> Option<TackleKind> {
    match type_id {
        SCRAMBLER | SCRAMBLER_II | DISRUPTOR | DISRUPTOR_II => Some(TackleKind::Warp),
        STASIS_WEBIFIER | STASIS_WEBIFIER_II => Some(TackleKind::Web),
        _ => None,
    }
}

/// Is `type_id` a module that tackles (a scrambler, a disruptor or a web)?
pub fn is_tackle(type_id: i64) -> bool {
    tackle_kind(type_id).is_some()
}

/// What its chat calls a target. **[A]**
pub const PREY_NAME: &str = "a capsuleer";

/// A pilot this close to the bot, surface to surface, metres, counts in the
/// odds and may be prey: "on the grid". The escort's guard radius.
pub const NEAR: f64 = escort::GUARD_RADIUS;

/// Aggression at or above which a hunter takes a hull one size above its
/// own. **[A]**: falsified by hunters dying to every cruiser (too low) or
/// never leaving a frigate war (too high).
pub const UP_AT: f64 = 0.7;

/// The odds a pilot of aggression 0 asks for before it starts a fight:
/// its side's power over theirs. **[A]**: a timid hunter wants a clear
/// edge.
pub const ODDS_TIMID: f64 = 1.5;
/// ...and aggression 1 asks for. **[A]**: a reckless one takes a fight it
/// is likely to lose. Falsified by the hunters' kill-to-loss ratio in the
/// `[soc]` line staying far under (too low) or far over 1 (too high).
pub const ODDS_RECKLESS: f64 = 0.5;
/// A fight already on is broken off when the odds fall under this share of
/// the bar. **[A]**: falsified by hunters that stay while three more
/// arrive (too low) or that leave every fight a stranger joins (too high).
pub const ABORT_ODDS: f64 = 0.6;

/// Armour fraction under which a pilot of aggression 0 leaves. **[A]**
pub const ARMOUR_TIMID: f64 = 0.8;
/// ...and aggression 1. **[A]**: falsified by hunters that dock with half
/// their armour (too high) or die with a full hold of kills (too low).
pub const ARMOUR_RECKLESS: f64 = 0.3;
/// Hull fraction under which a pilot of aggression 0 leaves. **[A]**
pub const HULL_TIMID: f64 = 0.99;
/// ...and aggression 1. **[A]**
pub const HULL_RECKLESS: f64 = 0.7;

/// How much each part of [`score`] weighs. **[A]**: the relation first
/// (the society's grudge), then the easier hull, then the nearer.
pub const W_RELATION: f64 = 2.0;
pub const W_EDGE: f64 = 1.0;
pub const W_NEAR: f64 = 1.0;
/// The bonus of the pilot already engaged: it stays prey while allowed.
pub const W_STICK: f64 = 100.0;
/// BP-COUNTER: the extra score of a pilot the counters name: a tackler or
/// jammer on me, or the bubble's source while I am held
/// ([`crate::counter::W_ROLE`]).
pub fn role_bonus(scene: &Scene, doc: &Ground, p: &Pilot) -> f64 {
    if scene.priority.contains(&p.ball.id) || (scene.held && doc.is_bubbler(p.type_id)) {
        crate::counter::W_ROLE
    } else {
        0.0
    }
}

/// A pilot this far (surface, metres) scores half the nearness of one
/// beside the bot. **[A]**
pub const NEAR_SCALE: f64 = 50_000.0;

/// A directional scan at most this often per place, and again after the
/// bot has moved this far from where it scanned. **[A]**: falsified by the
/// server's `[turns]` showing the scans crowding other bots (too often).
pub const DSCAN_EVERY: Duration = Duration::from_secs(60);
/// The distance, metres, after which a scan is old. **[A]**
pub const SWEEP_MOVED: f64 = 100_000.0;
/// One AU in metres, `probescanning/const.pyc` **[V]** (the server's
/// `scan::AU`).
pub const AU: f64 = 149_597_870_700.0;
/// The sphere's reach: 14.3 AU, the client's scanner maximum. **[A]**:
/// the client's slider cap is read from its UI code, not verified.
pub const DSCAN_RANGE: f64 = 14.3 * AU;
/// A cone's whole angle, degrees. **[A]**: the client's narrowest slider
/// step is 5 degrees; falsified by a client whose slider goes lower.
pub const DSCAN_NARROW: f64 = 5.0;
/// A belt's cone reaches this far past the belt, metres. **[A]**
pub const BELT_SLACK: f64 = 100_000.0;
/// Scans that find the system empty before the bot gives it up. **[A]**
pub const EMPTY_SWEEPS_MAX: u32 = 3;

/// Quiet sorties (undock, find nobody, dock) in one system before the bot
/// roams on. **[A]**
pub const SORTIES_PER_SYSTEM: u32 = 2;
/// How far a hunter roams, in jumps, at aggression 0 and at 1. **[A]**:
/// falsified by hunters that never leave home (too small) or sit in a
/// station out of every human's reach (too large).
pub const ROAM_JUMPS_MIN: u32 = 2;
pub const ROAM_JUMPS_MAX: u32 = 6;
/// A roam picks among this many nearest candidates, by the bot's seed.
pub const ROAM_CHOICES: usize = 3;
/// Recent systems remembered, so a roam does not go back at once.
pub const ROAM_MEMORY: usize = 3;
/// Failed roams in a row before the bot hunts where it is. **[A]**
pub const ROAM_TRIES: u32 = 3;
/// Refused scans in a row before the bot stops scanning. **[A]**: falsified
/// by a server whose scanner refuses now and then (too few).
pub const SCAN_TRIES: u32 = 3;

/// A wreck within this of where the prey last was, metres, is its wreck.
/// **[A]**
pub const WRECK_NEAR: f64 = 20_000.0;
/// How long a vanished prey waits for its wreck to show, before it counts
/// as escaped. **[A]**
pub const KILL_WINDOW: Duration = Duration::from_secs(5);
/// The killmails are read at least this often when docked. **[A]**
pub const SETTLE_EVERY: Duration = Duration::from_secs(300);

/// The `MISSING` line of a hunter started without `--society-pvp`.
pub const MISSING_PVP: &str = "eve-bots --society-pvp (the hunter is opt-in)";

/// The ships a hunter flies and its kit are the escort's, and a scrambler.
pub static ROLE: RoleSpec = RoleSpec {
    name: "hunter",
    greeting: "o7 -- hunting with you",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

/// **The opt-in**: `Err` when a bot plays [`ROLE`] and `--society-pvp` was
/// not given.
pub fn check_opt_in(
    roles: impl IntoIterator<Item = brain::RoleRef>,
    opted_in: bool,
) -> std::result::Result<(), String> {
    if !opted_in && roles.into_iter().any(|r| *r == ROLE) {
        return Err(format!(
            "--loop {}: a bot that hunts pilots is opt-in; add --society-pvp",
            ROLE.name
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The doctrine: what the bot knows of the cluster
// ---------------------------------------------------------------------------

/// The logistics cruiser and logistics frigate groups **[V-data]**
/// (`typedata.tsv`: Scimitar is 832; 1527 holds the frigates).
pub const LOGI_GROUPS: [i64; 2] = [832, 1527];

/// What a hull shoots with, by its hardpoints (BP-EWAR-AIM).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm {
    Turrets,
    Missiles,
    Unknown,
}

/// What a hunter decides by, built once at startup and handed to every bot
/// behind an `Arc` ([`crate::brain::Policy::pvp`]): each system's security
/// and each ship type's class.
#[derive(Clone, Default, PartialEq)]
pub struct Ground {
    security: BTreeMap<i64, f64>,
    classes: BTreeMap<i64, Class>,
    /// Ship type -> (group, launcher slots, turret slots), for ewar aim (BP-EWAR-AIM).
    fits: BTreeMap<i64, (i64, i64, i64)>,
}

impl std::fmt::Debug for Ground {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Ground({} systems, {} hulls)",
            self.security.len(),
            self.classes.len()
        )
    }
}

impl Ground {
    /// Nothing known: no system is hunting ground.
    pub fn new() -> Ground {
        Ground::default()
    }

    /// One system's security, unrounded.
    pub fn with_security(mut self, system: i64, security: f64) -> Ground {
        self.security.insert(system, security);
        self
    }

    /// One ship type's class.
    pub fn with_class(mut self, type_id: i64, class: Class) -> Ground {
        self.classes.insert(type_id, class);
        self
    }

    /// The systems and ship hulls of a content bundle (`eve-bots --content
    /// DIR`); with no directory, an empty doctrine, in which nobody is
    /// prey and no system is ground. Errors name the directory.
    pub fn open(content: Option<&Path>) -> std::result::Result<Ground, String> {
        let mut d = Ground::new();
        let Some(dir) = content else {
            return Ok(d);
        };
        let (bundle, _) = eve_server::content::Bundle::open(dir)?;
        let at = |e: String| format!("--content {}: {e}", dir.display());
        let universe = Universe::from_bundle(&bundle).map_err(at)?;
        d.security
            .extend(universe.systems().map(|s| (s.id, s.security)));
        if let Some(p) = bundle.path(eve_server::content::TYPEDATA) {
            let td = typedata::Table::load(&p).map_err(at)?;
            d.classes.extend(
                td.types()
                    .filter_map(|t| Class::of_type(t).map(|c| (t.type_id, c))),
            );
            d.fits
                .extend(td.types().filter(|t| Class::of_type(t).is_some()).map(|t| {
                    let slots = |a| t.attribute(a).unwrap_or(0.0).round() as i64;
                    (
                        t.type_id,
                        (
                            t.group_id,
                            slots(typedata::attr::LAUNCHER_SLOTS),
                            slots(typedata::attr::TURRET_SLOTS),
                        ),
                    )
                }));
        }
        Ok(d)
    }

    /// One ship type's group and hardpoints (BP-EWAR-AIM).
    pub fn with_fit(mut self, type_id: i64, group: i64, launchers: i64, turrets: i64) -> Ground {
        self.fits.insert(type_id, (group, launchers, turrets));
        self
    }

    /// Does the hull make a warp bubble: an interdictor or a heavy interdictor
    /// ([`crate::counter::BUBBLE_GROUPS`])?
    pub fn is_bubbler(&self, type_id: Option<i64>) -> bool {
        type_id
            .and_then(|t| self.fits.get(&t))
            .is_some_and(|(g, _, _)| crate::counter::BUBBLE_GROUPS.contains(g))
    }

    /// Is the hull a logistics ship (cruiser or frigate group)?
    pub fn is_logi(&self, type_id: Option<i64>) -> bool {
        type_id
            .and_then(|t| self.fits.get(&t))
            .is_some_and(|(g, _, _)| LOGI_GROUPS.contains(g))
    }

    /// What the hull shoots with, from its hardpoints: more launchers than
    /// turrets is missiles, more turrets is turrets, a tie or an unknown
    /// hull is [`Arm::Unknown`]. **[A]** a drone boat reads Unknown.
    pub fn arm_of(&self, type_id: Option<i64>) -> Arm {
        match type_id.and_then(|t| self.fits.get(&t)) {
            Some((_, l, t)) if t > l => Arm::Turrets,
            Some((_, l, t)) if l > t => Arm::Missiles,
            _ => Arm::Unknown,
        }
    }

    /// A hull's class; a type it does not know is [`Class::Unknown`].
    pub fn class_of(&self, type_id: Option<i64>) -> Class {
        type_id
            .and_then(|t| self.classes.get(&t).copied())
            .unwrap_or(Class::Unknown)
    }

    /// Is `type_id` a ship it knows?
    pub fn is_ship(&self, type_id: i64) -> bool {
        self.classes.contains_key(&type_id)
    }

    /// A system's security as the client rounds it, if known.
    pub fn security(&self, system: i64) -> Option<f64> {
        self.security.get(&system).map(|&s| pseudo_security(s))
    }

    /// **Where a bubble may be made** (BP-CAMP): null sec and wormhole or
    /// void space. The server refuses the sphere launcher and the field in
    /// high and low sec; the bot checks first so it never asks.
    pub fn allows_bubbles(&self, system: i64) -> bool {
        if WORMHOLE_SYSTEMS.contains(&system) || VOID_SYSTEMS.contains(&system) {
            return true;
        }
        self.security(system)
            .is_some_and(|s| SecurityClass::from_level(s) == SecurityClass::Zero)
    }

    /// **Hunting ground**: low sec, null sec, or wormhole or void space
    /// (module header). High sec, and a system nothing is known of, are
    /// not.
    pub fn is_ground(&self, system: i64) -> bool {
        if WORMHOLE_SYSTEMS.contains(&system) || VOID_SYSTEMS.contains(&system) {
            return true;
        }
        self.security(system)
            .is_some_and(|s| SecurityClass::from_level(s) <= SecurityClass::Low)
    }
}

// ---------------------------------------------------------------------------
// What the driver shows the brain
// ---------------------------------------------------------------------------

/// The two traits a hunter's rules read, in `[0, 1]` (the mind's
/// `aggression` and `risk`); neutral without a mind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Temper {
    pub aggression: f64,
    pub risk: f64,
}

impl Default for Temper {
    fn default() -> Temper {
        Temper {
            aggression: 0.5,
            risk: 0.5,
        }
    }
}

/// Another pilot's ship on the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Pilot {
    pub ball: Ball,
    pub char_id: i64,
    pub type_id: Option<i64>,
    pub who: Affiliation,
    /// `Hall::hostile_to`: not allied, and not the fleet boss.
    pub hostile: bool,
    /// The society's `R[my corp][their group]`, `[-10, 10]`; 0 with no
    /// hall or no view.
    pub relation: f64,
    /// R-REL-5: the pilot's side is at war with mine (a hostile war between
    /// our corps or alliances, allies counted): legal prey in high sec, where
    /// CONCORD does not answer (`war::legal`).
    pub war: bool,
}

/// One directional scan's reading.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep {
    pub at: Instant,
    pub system: i64,
    /// Where the bot was.
    pub origin: [f64; 3],
    /// Pilots' ships in the system, not on the grid: the sphere.
    pub ships: usize,
    /// DOC-3: the sphere's pilots' ships by hull type, the grid's included
    /// and the bot's own (`ships` leaves the grid out): the d-scan the
    /// doctrine layer reads (`doctrine_sense::Sighting::scan`).
    pub hulls: BTreeMap<i64, u32>,
    /// `(belt, pilots' ships in the cone toward it)`, those with any.
    pub toward: Vec<(i64, usize)>,
}

impl Sweep {
    /// Is the reading still good: young, in this system, and the bot has
    /// not moved far from where it took it.
    pub fn fresh(&self, now: Instant, system: i64, ego: &Ball) -> bool {
        self.system == system
            && now.saturating_duration_since(self.at) < DSCAN_EVERY
            && dist(self.origin, ego.position) < SWEEP_MOVED
    }
}

// ---------------------------------------------------------------------------
// The cloak (CLOAK-2): only from stock, nothing from nowhere
// ---------------------------------------------------------------------------

/// A cloaking device in the hunter's fit. It is there only if the doctrine's
/// stock or a builder supplied it ([`crate::doctrine_fit`] and
/// [`crate::fit_plan`] never fit one from nowhere); the driver reads it off
/// the fitted modules, with the two delays from the module's own row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloakKit {
    pub item_id: i64,
    pub type_id: i64,
    /// The effect to activate: `(name, id)` (607, 980 or 5945).
    pub effect: (&'static str, i64),
    /// The covert-ops cloak (980) warps cloaked.
    pub warp_safe: bool,
    /// `cloakingTargetingDelay` (560): no lock for this long after a decloak.
    pub targeting_delay: Duration,
    /// `moduleReactivationDelay` (669).
    pub reactivation: Duration,
    pub active: bool,
}

/// Clear of everything by this much, surface to surface, before the hunter
/// cloaks: the server's `minCloakingDistance` and a margin, so the cloak is
/// not broken by the next drift **[A]** (the margin).
pub const CLOAK_CLEARANCE: f64 = eve_server::cloak::MIN_CLOAKING_DISTANCE + 500.0;

/// **Rule A** (pure): idle on hunting ground with no prey, clear of
/// everything -- a safe or a perch -- the hunter cloaks and sits on its scan
/// cadence. Not while a sweep sends it to a belt, not in the tunnel, and not
/// before the module's own reactivation delay has run.
pub fn cloak_now(
    kit: &CloakKit,
    park: &ParkView,
    scene: &Scene,
    off_at: Option<Instant>,
    now: Instant,
) -> bool {
    if kit.active || park.ego_warping {
        return false;
    }
    if off_at.is_some_and(|t| now.saturating_duration_since(t) < kit.reactivation) {
        return false;
    }
    if scene.sweep.as_ref().is_some_and(|s| s.ships > 0) {
        return false;
    }
    let ego = &park.ego;
    let balls = park
        .rocks
        .iter()
        .chain(&park.belts)
        .chain(&park.station)
        .chain(&park.wrecks)
        .chain(park.hostiles.iter().map(|h| &h.ball))
        .chain(scene.pilots.iter().map(|p| &p.ball))
        .chain(scene.wrecks.iter());
    balls
        .filter(|b| b.id != ego.id)
        .all(|b| ego.surface_to(b) > CLOAK_CLEARANCE)
}

/// **Rule B** (pure): a running cloak must come off before `acts` -- prey
/// chosen, a lock, a shot, or (for a cloak that is not warp-safe) any move.
pub fn uncloak_before(kit: &CloakKit, acts: &[Act], prey: bool) -> bool {
    if !kit.active {
        return false;
    }
    if prey {
        return true;
    }
    acts.iter().any(|a| match a {
        Act::Wait(_) | Act::Say(_) | Act::Hunt(_) => false,
        Act::Lock(_) | Act::Fire { .. } => true,
        _ => !kit.warp_safe,
    })
}

/// A lock refused with `DeniedTargetAfterCloak` is held, not dropped: retry
/// when the type's own `cloakingTargetingDelay` has run (pure).
pub fn lock_hold(why: &str, kit: Option<&CloakKit>, now: Instant) -> Option<Instant> {
    why.contains("DeniedTargetAfterCloak")
        .then(|| now + kit.map_or(Duration::from_secs(1), |k| k.targeting_delay))
}

/// What the hunter sees beyond the park's NPCs, built by [`observe`] on
/// every park refresh; the sweep is kept across them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// R-REL-5: a war of mine is hostile now (the hunter then searches high
    /// sec for its targets too).
    pub war_on: bool,
    pub pilots: Vec<Pilot>,
    /// Wrecks of pilots' ships (an NPC corp's are the park's).
    pub wrecks: Vec<Ball>,
    pub temper: Temper,
    pub sweep: Option<Sweep>,
    /// The fitted cloak, if any (CLOAK-2); kept across park refreshes.
    pub cloak: Option<CloakKit>,
    /// BP-COUNTER: the ships tackling or jamming this one, killed first
    /// ([`crate::counter::priority`]); set by the driver each turn.
    pub priority: Vec<i64>,
    /// BP-COUNTER: a warp bubble holds this ship, so its interdictor or HIC
    /// is the prey.
    pub held: bool,
    /// BP-COUNTER: a guidance disruptor is on a missile ship, which then
    /// keeps the nearest prey.
    pub nearest_first: bool,
    /// The traits the filament and ESS steps read besides [`Temper`].
    pub mood: crate::filament::Mood,
}

impl Scene {
    /// A primary that is a hostile pilot's ship (C7's `engage` asks, so
    /// that another bot's ship can be a primary): never an ally's.
    pub fn primary_target(&self, id: i64) -> Option<Hostile> {
        self.pilots
            .iter()
            .find(|p| p.ball.id == id && p.hostile)
            .map(|p| Hostile {
                ball: p.ball,
                type_id: p.type_id.unwrap_or(0),
                name: PREY_NAME.to_owned(),
                shooting: None,
            })
    }
}

/// Who the bot is, for [`observe`].
pub struct Me<'a> {
    pub char_id: i64,
    pub who: Affiliation,
    /// The fleet boss, never hostile.
    pub boss: Option<i64>,
    pub ego_ball: i64,
    pub hall: Option<&'a Hall>,
    /// R-REL-3: this pilot's own regard list and loyalty, for `R_eff`
    /// (empty: the corp, alliance and coalition views alone).
    pub regard: &'a [crate::mind::Regard],
    pub loyalty: f64,
}

/// **The relation `R[corp][group of other]`** in the hall's matrix.
pub fn relation_of(hall: &Hall, my_corp: i64, other: &Affiliation) -> f64 {
    hall.lock().relations.get(my_corp, society::group_of(other))
}

/// R-REL-3: **the effective relation** a hunter holds toward a pilot
/// ([`crate::corpmind::HallState::r_eff_of`]): its own regard for the
/// pilot, its corp's view, its alliance's and its coalition's -- not the
/// corp's row alone. Without a regard list it is the layers alone.
pub fn r_eff_toward(
    hall: &Hall,
    my_corp: i64,
    regard: &[crate::mind::Regard],
    loyalty: f64,
    other: &Affiliation,
) -> f64 {
    let mut st = hall.lock();
    let now = st.relations.at.unwrap_or(0);
    st.see_human(other, now);
    st.r_eff_of(my_corp, regard, loyalty, other, now)
}

/// **Look at the park as a hunter**: every other pilot's ship with whether
/// it is hostile and how the society feels about it, and the wrecks of
/// pilots' ships. Bots of this process are pilots too (the hall's `allied`
/// is what makes some of them friends).
pub fn observe(world: &crate::park::World, me: &Me<'_>) -> (Vec<Pilot>, Vec<Ball>) {
    observe_balls(world.balls(), me)
}

/// [`observe`] over any balls (the tests' hand-built ones).
pub fn observe_balls<'a>(
    balls: impl IntoIterator<Item = &'a crate::Tracked>,
    me: &Me<'_>,
) -> (Vec<Pilot>, Vec<Ball>) {
    let ball = |t: &crate::Tracked| Ball {
        id: t.ball_id,
        position: t.motion.position,
        radius: t.radius,
    };
    let mut pilots = Vec::new();
    let mut wrecks = Vec::new();
    let ft = eve_server::pyutil::filetime_now();
    for b in balls {
        if b.ball_id == me.ego_ball {
            continue;
        }
        let Some(slim) = b.slim else { continue };
        match slim.char_id {
            Some(c) if c != me.char_id => {
                let who = Affiliation {
                    character_id: c,
                    corporation_id: slim.corp_id.unwrap_or(0),
                    alliance_id: slim.alliance_id,
                };
                let hostile = me.boss != Some(c)
                    && match me.hall {
                        Some(h) => h.hostile_to(me.who, who),
                        None => society::hostile_to(me.who, who, |_, _| None),
                    };
                let relation = me.hall.map_or(0.0, |h| {
                    r_eff_toward(h, me.who.corporation_id, me.regard, me.loyalty, &who)
                });
                let war = me.boss != Some(c)
                    && me.hall.is_some_and(|h| h.lock().at_war(&me.who, &who, ft));
                pilots.push(Pilot {
                    ball: ball(b),
                    char_id: c,
                    type_id: slim.type_id,
                    who,
                    hostile,
                    relation,
                    war,
                });
            }
            Some(_) => {}
            None => {
                if is_pilot_wreck(&slim) {
                    wrecks.push(ball(b));
                }
            }
        }
    }
    (pilots, wrecks)
}

/// A wreck that is not an NPC corporation's: a pilot's ship.
fn is_pilot_wreck(s: &crate::park::SlimInfo) -> bool {
    s.type_id
        .is_some_and(|t| eve_server::item::WRECK_TYPES.iter().any(|w| w.type_id == t))
        && !s
            .owner_id
            .is_some_and(|o| (1_000_000..2_000_000).contains(&o))
        && !crate::driver::npc_launched(s)
}

// ---------------------------------------------------------------------------
// The rules (pure)
// ---------------------------------------------------------------------------

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// A hull's size as a rank, 0 a capsule to 4 a capital; a hull it cannot
/// class counts as a medium one. **[A]**
pub fn rank(c: Class) -> i32 {
    match c {
        Class::Capsule => 0,
        Class::Small => 1,
        Class::Medium | Class::Unknown => 2,
        Class::Large => 3,
        Class::Capital => 4,
    }
}

/// A hull's fighting weight in the odds: a capsule none. **[A]**: falsified
/// by cruisers that lose to two frigates (too low) or frigates that never
/// gang a cruiser (too high).
pub fn power(c: Class) -> f64 {
    match c {
        Class::Capsule => 0.0,
        Class::Small => 1.0,
        Class::Medium | Class::Unknown => 3.0,
        Class::Large => 8.0,
        Class::Capital => 25.0,
    }
}

/// The odds the bot asks for at `aggression`.
pub fn required_odds(aggression: f64) -> f64 {
    lerp(ODDS_TIMID, ODDS_RECKLESS, aggression)
}

/// The armour fraction under which the bot leaves.
pub fn armour_floor(aggression: f64) -> f64 {
    lerp(ARMOUR_TIMID, ARMOUR_RECKLESS, aggression)
}

/// The hull fraction under which the bot leaves.
pub fn hull_floor(aggression: f64) -> f64 {
    lerp(HULL_TIMID, HULL_RECKLESS, aggression)
}

/// The hull size, rank, the bot may attack: its own, and one more at
/// [`UP_AT`].
pub fn max_rank(mine: Class, aggression: f64) -> i32 {
    rank(mine) + i32::from(aggression >= UP_AT)
}

/// **The odds of the grid**: the bot's side -- itself and every pilot that
/// is not hostile within [`NEAR`] -- over every hostile pilot within
/// [`NEAR`], by [`power`]. Infinite with no foe on the grid.
pub fn odds(mine: Class, ego: &Ball, scene: &Scene, doc: &Ground) -> f64 {
    odds_by(mine, ego, scene, doc, |p| p.hostile)
}

/// R-REL-5: **the odds of a war in high sec**: only the pilots at war with
/// the bot are foes; the others there (miners, bystanders) are neither side.
pub fn war_odds(mine: Class, ego: &Ball, scene: &Scene, doc: &Ground) -> f64 {
    let (mut side, mut foes) = (power(mine), 0.0);
    for p in &scene.pilots {
        if ego.surface_to(&p.ball) > NEAR {
            continue;
        }
        let w = power(doc.class_of(p.type_id));
        if p.war {
            foes += w;
        } else if !p.hostile {
            side += w;
        }
    }
    if foes <= 0.0 {
        f64::INFINITY
    } else {
        side / foes
    }
}

fn odds_by(
    mine: Class,
    ego: &Ball,
    scene: &Scene,
    doc: &Ground,
    foe: impl Fn(&Pilot) -> bool,
) -> f64 {
    let (mut side, mut foes) = (power(mine), 0.0);
    for p in &scene.pilots {
        if ego.surface_to(&p.ball) > NEAR {
            continue;
        }
        let w = power(doc.class_of(p.type_id));
        if foe(p) {
            foes += w;
        } else {
            side += w;
        }
    }
    if foes <= 0.0 {
        f64::INFINITY
    } else {
        side / foes
    }
}

/// The part of the prey's [`score`]: higher is a likelier choice.
pub fn score(p: &Pilot, mine: Class, ego: &Ball, doc: &Ground, engaged: bool) -> f64 {
    let (a, b) = (power(mine), power(doc.class_of(p.type_id)));
    let edge = if a.max(b) > 0.0 {
        (a - b) / a.max(b)
    } else {
        0.0
    };
    let near = 1.0 / (1.0 + ego.surface_to(&p.ball).max(0.0) / NEAR_SCALE);
    W_RELATION * (-p.relation / society::R_MAX)
        + W_EDGE * edge
        + W_NEAR * near
        + if engaged { W_STICK } else { 0.0 }
}

/// **This turn's prey** (module header, rule 1): the hostile pilot within
/// [`NEAR`] with the best [`score`] that the risk rule allows; `None` when
/// the grid's odds are under the bar.
pub fn choose_prey<'a>(
    engaged: Option<i64>,
    mine: Class,
    ego: &Ball,
    scene: &'a Scene,
    doc: &Ground,
) -> Option<&'a Pilot> {
    pick(
        engaged,
        mine,
        ego,
        scene,
        doc,
        odds(mine, ego, scene, doc),
        |p| p.hostile,
    )
}

/// R-REL-5: **a war target in high sec**: the same risk rule and score, but
/// only a pilot at war with the bot is prey, and the odds count only them.
pub fn choose_war_prey<'a>(
    engaged: Option<i64>,
    mine: Class,
    ego: &Ball,
    scene: &'a Scene,
    doc: &Ground,
) -> Option<&'a Pilot> {
    pick(
        engaged,
        mine,
        ego,
        scene,
        doc,
        war_odds(mine, ego, scene, doc),
        |p| p.war,
    )
}

fn pick<'a>(
    engaged: Option<i64>,
    mine: Class,
    ego: &Ball,
    scene: &'a Scene,
    doc: &Ground,
    odds: f64,
    foe: impl Fn(&Pilot) -> bool,
) -> Option<&'a Pilot> {
    let a = scene.temper.aggression;
    if odds < required_odds(a) {
        return None;
    }
    scene
        .pilots
        .iter()
        .filter(|p| foe(p))
        .filter(|p| ego.surface_to(&p.ball) <= NEAR)
        .filter(|p| {
            // A brawler (infinite odds, `Policy::brawl`) takes any hull: with
            // the rank cap a Rifter closed on a battleship through
            // `brawl_close`, never picked it, and re-sent its approach every
            // turn (owner, 2026-11-03).
            let c = doc.class_of(p.type_id);
            c != Class::Capsule && (odds.is_infinite() || rank(c) <= max_rank(mine, a))
        })
        .max_by(|x, y| {
            let total = |p: &Pilot| {
                score(p, mine, ego, doc, engaged == Some(p.ball.id)) + role_bonus(scene, doc, p)
            };
            // BP-COUNTER: under guidance disruption a missile ship keeps the
            // nearest prey, whatever the score says.
            let nearer = if scene.nearest_first {
                ego.surface_to(&y.ball).total_cmp(&ego.surface_to(&x.ball))
            } else {
                std::cmp::Ordering::Equal
            };
            nearer
                .then(total(x).total_cmp(&total(y)))
                // The lower id on a tie, so the same grid picks the same.
                .then(y.ball.id.cmp(&x.ball.id))
        })
}

/// Why a fight is broken off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retreat {
    Armour,
    Hull,
    /// Others joined: the odds fell under [`ABORT_ODDS`] of the bar.
    Odds,
}

impl Retreat {
    pub fn line(self) -> &'static str {
        match self {
            Retreat::Armour => "Losing -- warping off to repair",
            Retreat::Hull => "Hull going -- warping off",
            Retreat::Odds => "Too many of them -- warping off",
        }
    }
}

/// **Whether to leave** (module header, rule 2): `health` is the ship's
/// bars (`None`: untouched), `odds` this turn's [`odds`].
pub fn retreat(health: Option<Health>, t: Temper, odds: f64) -> Option<Retreat> {
    if let Some(h) = health {
        if h.hull < hull_floor(t.aggression) {
            return Some(Retreat::Hull);
        }
        if h.armour < armour_floor(t.aggression) {
            return Some(Retreat::Armour);
        }
    }
    (odds < required_odds(t.aggression) * ABORT_ODDS).then_some(Retreat::Odds)
}

/// **Tackle first** (module header, rule 3): the shots of tackle modules
/// go before the guns', otherwise in order.
pub fn tackle_first(acts: Vec<Act>, guns: &[GunInfo]) -> Vec<Act> {
    let tackles = |a: &Act| {
        matches!(a, Act::Fire { gun, .. }
            if guns.iter().any(|g| g.item_id == *gun && g.kind.is_tackle()))
    };
    let (first, rest): (Vec<Act>, Vec<Act>) = acts.into_iter().partition(tackles);
    [first, rest].concat()
}

/// **The last line of defence**: drop every lock, orbit and shot at a pilot
/// that is not `prey` (and at the fleet boss); a turn left with nothing to
/// do waits.
pub fn guard(acts: Vec<Act>, o: &Observation, prey: Option<i64>) -> Vec<Act> {
    guard_with(acts, o, prey, &[])
}

/// [`guard`], letting through the shots and locks at `allow` too: the lawful
/// foes a tackle or ewar module is aimed at (BP-EWAR-AIM, [`crate::ewar_aim`]).
pub fn guard_with(acts: Vec<Act>, o: &Observation, prey: Option<i64>, allow: &[i64]) -> Vec<Act> {
    let protected: BTreeSet<i64> = o
        .hunt
        .pilots
        .iter()
        .map(|p| p.ball.id)
        .chain(o.humans.iter().map(|b| b.id))
        .chain(o.park.as_ref().and_then(|p| p.boss).map(|b| b.id))
        .filter(|id| Some(*id) != prey && !allow.contains(id))
        .collect();
    let kept: Vec<Act> = acts
        .into_iter()
        .filter(|a| match a {
            Act::Fire { target, .. } | Act::Lock(target) | Act::Orbit { id: target, .. } => {
                !protected.contains(target)
            }
            _ => true,
        })
        .collect();
    if kept.is_empty() {
        vec![Act::Wait(brain::SHORT_WAIT)]
    } else {
        kept
    }
}

/// **Which belt to warp to** (module header, rule 5): among the sweep's
/// belts that see pilots and were not visited, the most pilots, then the
/// nearest, then the lower id.
pub fn pick_belt(sweep: &Sweep, visited: &[i64], park: &ParkView) -> Option<Ball> {
    sweep
        .toward
        .iter()
        .filter(|(id, n)| *n > 0 && !visited.contains(id))
        .filter_map(|(id, n)| park.belts.iter().find(|b| b.id == *id).map(|b| (*b, *n)))
        .min_by(|(a, na), (b, nb)| {
            nb.cmp(na)
                .then(park.ego.surface_to(a).total_cmp(&park.ego.surface_to(b)))
                .then(a.id.cmp(&b.id))
        })
        .map(|(b, _)| b)
}

/// **Where to hunt next** (pure): a station in another hunting-ground
/// system within [`roam_jumps`] of `from` by the map's gates, nearest
/// first, one of the nearest [`ROAM_CHOICES`] by `seed`, none of `avoid`.
pub fn roam_target(
    map: &Map,
    doc: &Ground,
    from: i64,
    aggression: f64,
    seed: u64,
    avoid: &[i64],
) -> Option<i64> {
    roam_target_with(map, doc, from, aggression, seed, avoid, &[])
}

/// [`roam_target`] with the motherlodes the hunter has heard of (`fresh`,
/// [`crate::geode_haul::Heard::fresh`]; L7 BOT-ECON): each such system and
/// the systems a gate from it are ground to roam to, and the motherlode
/// systems themselves come first, nearest first. This is where to roam, not
/// where the law ends: [`Ground::is_ground`] is unchanged, so a hunter in high
/// sec still engages only a war target there.
pub fn roam_target_with(
    map: &Map,
    doc: &Ground,
    from: i64,
    aggression: f64,
    seed: u64,
    avoid: &[i64],
    fresh: &[i64],
) -> Option<i64> {
    roam_target_clear(map, doc, from, aggression, seed, avoid, fresh, None)
}

/// [`roam_target_with`] for a pilot whose `status` the faction police
/// attack somewhere (BOT-LAW, R-CONCORD 6.7): the walk never enters a
/// system of their net ([`Map::in_police_net`]), so no target lies in it or
/// beyond it.
#[allow(clippy::too_many_arguments)]
pub fn roam_target_clear(
    map: &Map,
    doc: &Ground,
    from: i64,
    aggression: f64,
    seed: u64,
    avoid: &[i64],
    fresh: &[i64],
    status: Option<f64>,
) -> Option<i64> {
    let extra = crate::geode_haul::ground_around(map, fresh);
    let reach = roam_jumps(aggression);
    let net = |n: i64| status.is_some_and(|st| map.in_police_net(n, st));
    let mut seen = BTreeSet::from([from]);
    let mut queue = VecDeque::from([(from, 0u32)]);
    let mut found: Vec<(u32, u32, i64)> = Vec::new();
    while let Some((s, d)) = queue.pop_front() {
        if d > 0
            && !net(s)
            && (doc.is_ground(s) || extra.contains(&s))
            && !avoid.contains(&s)
            && !map.stations_in(s).is_empty()
        {
            found.push((u32::from(!fresh.contains(&s)), d, s));
        }
        if d == reach {
            continue;
        }
        for n in map.neighbours(s) {
            // Net systems pass only while the walk is unbroken net from the
            // start; none is ever a target.
            if net(n) && !net(s) {
                continue;
            }
            if seen.insert(n) {
                queue.push_back((n, d + 1));
            }
        }
    }
    found.sort();
    // A motherlode, when one is in reach, is where the hunter goes.
    let hot = found.iter().take_while(|f| f.0 == 0).count();
    let pool = if hot > 0 { hot } else { found.len() }.min(ROAM_CHOICES);
    let (_, _, system) = *found.get(usize::try_from(seed % pool.max(1) as u64).ok()?)?;
    map.stations_in(system).first().copied()
}

/// The jumps a hunter of `aggression` roams.
pub fn roam_jumps(aggression: f64) -> u32 {
    lerp(
        f64::from(ROAM_JUMPS_MIN),
        f64::from(ROAM_JUMPS_MAX),
        aggression,
    )
    .round() as u32
}

// ---------------------------------------------------------------------------
// The memory
// ---------------------------------------------------------------------------

/// The pilot being fought, as last seen.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub id: i64,
    pub position: [f64; 3],
    /// The groups of the pilots on grid on the bot's side, at the last
    /// sight (they fought beside it).
    pub allies: Vec<Group>,
}

/// What a hunter intends and remembers. Never world state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hunt {
    pub prey: Option<Mark>,
    /// The prey vanished at this moment; its wreck decides kill or escape.
    pub gone: Option<(Mark, Instant)>,
    /// A kill is thought to have happened and the killmails have not been
    /// read since.
    pub unsettled: bool,
    pub allies: Vec<Group>,
    pub settled_at: Option<Instant>,
    /// The killmails read at least once in this run: the first reading
    /// counts nothing (old reports are not this run's kills).
    pub settled_once: bool,
    /// Kill reports already counted as kills or losses of this run.
    pub counted: BTreeSet<i64>,
    pub kills: u32,
    /// The belts already visited since the last scan's round.
    pub visited: Vec<i64>,
    pub empty_sweeps: u32,
    pub quiet_sorties: u32,
    pub roam_failed: u32,
    /// Scans refused in a row: after [`SCAN_TRIES`] the bot patrols alone.
    pub scan_failed: u32,
    /// Systems hunted lately, newest last.
    pub recent: Vec<i64>,
    /// Hangar tackle modules already tried to fit (one scrambler, one web).
    pub tackle_tried: Vec<i64>,
    /// The crimewatch safety has been set red this run ([`HuntAct::Safety`]).
    pub safety_set: bool,
    /// The security office was last asked at this moment
    /// ([`HuntAct::Office`], [`crate::tag_repair`]).
    pub office_at: Option<Instant>,
    /// The security status read at the office (BOT-LAW): roams keep out of
    /// the systems where faction police attack it.
    pub status: Option<f64>,
    /// The cloak was last taken off at this moment (its reactivation delay
    /// runs from it).
    pub cloak_off_at: Option<Instant>,
    /// No lock before this moment: a `DeniedTargetAfterCloak` was heard.
    pub lock_hold: Option<Instant>,
    /// A trip into new space on a filament ([`crate::filament`]).
    pub away: Option<crate::filament::Away>,
    /// Filament activations refused or not completed in a row.
    pub fil_failed: u32,
    /// The ESS banks it has read and the raid under way ([`crate::ess_raid`]).
    pub ess: crate::ess_raid::Mind,
}

impl Hunt {
    /// What a scan came to.
    pub fn on_scan(&mut self, ok: bool) {
        self.scan_failed = if ok { 0 } else { self.scan_failed + 1 };
    }

    /// What a roam came to: the system it ended in is remembered; a failure
    /// is counted.
    pub fn on_roam(&mut self, ok: bool, system: Option<i64>) {
        if ok {
            self.away = None;
            self.roam_failed = 0;
            self.quiet_sorties = 0;
            if let Some(s) = system {
                self.recent.push(s);
                if self.recent.len() > ROAM_MEMORY {
                    self.recent.remove(0);
                }
            }
        } else {
            self.roam_failed += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// The acts
// ---------------------------------------------------------------------------

/// What the hunter asks the driver to do beyond the shared acts.
#[derive(Debug, Clone, PartialEq)]
pub enum HuntAct {
    /// Scan: the whole system, then toward each belt `(ball, position)`.
    DScan {
        belts: Vec<(i64, [f64; 3])>,
        ego: [f64; 3],
        system: i64,
    },
    /// Docked: fly to a station in another hunting-ground system.
    Roam { station: i64 },
    /// Read the killmails into the society; `allies` fought beside it.
    Settle { allies: Vec<Group> },
    /// `crimewatch.SetSafetyLevel(0)`, once: with the safety green the
    /// server refuses an attack that would make the pilot a suspect (CW-1),
    /// which is every attack on a pilot the hunter may fight in low sec.
    Safety,
    /// An outlaw mends his name: turn in the Clone Soldier tags of the
    /// hangar, buying the ones it lacks in the closed economy
    /// ([`crate::tag_repair`]).
    Office,
    /// Activate the fitted cloak (Rule A).
    Cloak(CloakKit),
    /// Deactivate it (Rule B).
    Uncloak(CloakKit),
    /// Spend the random-jump filament `item` in the hold ([`crate::filament`]).
    Filament { item: i64 },
    /// An ESS main bank step ([`crate::ess_raid`]).
    Ess(crate::ess_raid::EssAct),
}

impl HuntAct {
    /// The act log's text and the ids it names.
    pub fn describe(&self) -> (String, Vec<i64>) {
        match self {
            HuntAct::DScan { belts, system, .. } => (
                format!("ConeScan sphere + {} cones in system {system}", belts.len()),
                belts.iter().map(|b| b.0).collect(),
            ),
            HuntAct::Roam { station } => (format!("roam to station {station}"), vec![*station]),
            HuntAct::Settle { allies } => (
                format!("settle killmails ({} allies)", allies.len()),
                vec![],
            ),
            HuntAct::Safety => ("set the ship safety to red".to_owned(), vec![]),
            HuntAct::Office => ("ask the security office for tags".to_owned(), vec![]),
            HuntAct::Filament { item } => crate::filament::describe(*item),
            HuntAct::Ess(op) => op.describe(),
            HuntAct::Cloak(k) => (format!("cloak with {}", k.item_id), vec![k.item_id]),
            HuntAct::Uncloak(k) => (format!("decloak {}", k.item_id), vec![k.item_id]),
        }
    }
}

// ---------------------------------------------------------------------------
// The role
// ---------------------------------------------------------------------------

fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(doc) = m.policy.pvp.clone() else {
        return vec![Act::Missing(MISSING_PVP), Act::Wait(brain::MISSING_WAIT)];
    };
    // Safety red, once, before anything that could lead to a shot.
    if !m.hunt.safety_set {
        m.hunt.safety_set = true;
        return vec![Act::Hunt(HuntAct::Safety)];
    }
    // Home from an empty system: one more quiet sortie.
    if m.homing == Some(Homing::NoRocks) {
        m.hunt.quiet_sorties += 1;
        m.homing = None;
    }
    m.hunt.visited.clear();
    m.hunt.empty_sweeps = 0;
    m.hunt.scan_failed = 0;
    m.hunt.prey = None;
    m.hunt.gone = None;
    // The killmails, now and then, and after a kill.
    let due = m
        .hunt
        .settled_at
        .is_none_or(|t| o.now.saturating_duration_since(t) >= SETTLE_EVERY);
    if m.hunt.unsettled || due {
        m.hunt.settled_at = Some(o.now);
        m.hunt.unsettled = false;
        let allies = std::mem::take(&mut m.hunt.allies);
        return vec![Act::Hunt(HuntAct::Settle { allies })];
    }
    // An outlaw mends his name when the office is due ([`crate::tag_repair`]).
    // Open mode only with tags in hand (it cannot buy); closed always.
    if (m.policy.closed || !crate::tag_repair::held(&o.hangar).is_empty())
        && m.hunt
            .office_at
            .is_none_or(|t| o.now.saturating_duration_since(t) >= crate::tag_repair::EVERY)
    {
        m.hunt.office_at = Some(o.now);
        return vec![Act::Hunt(HuntAct::Office)];
    }
    let acts = escort::docked(m, o);
    if !acts.contains(&Act::Undock) {
        return acts;
    }
    // The first pass is [Stock, Undock]: the prep rides with each fit, or the
    // driver undocks before the kit is fitted.
    let prep: Vec<Act> = acts
        .iter()
        .filter(|a| **a != Act::Undock)
        .cloned()
        .collect();
    // Docked away from the station a filament trip left: the way home.
    if let Some(act) = crate::filament::dock_home_step(m, o) {
        return [prep, vec![act]].concat();
    }
    // Bored of home: a filament from the hangar goes into the hold.
    if let Some(act) = crate::filament::stock_step(m, o) {
        return [prep, vec![act]].concat();
    }
    // Ready to go. A scrambler and a web in the hangar are each fitted
    // first, once, unless that kind of tackle is fitted already (K-EWAR).
    if let Some(item) = o.hangar.iter().find(|r| {
        is_tackle(r.type_id)
            && !m.hunt.tackle_tried.contains(&r.item_id)
            && !o
                .guns
                .iter()
                .any(|g| tackle_kind(g.type_id) == tackle_kind(r.type_id))
    }) {
        m.hunt.tackle_tried.push(item.item_id);
        return [prep, vec![Act::FitAuto(item.item_id)]].concat();
    }
    // BP-KIT, nothing from nowhere: a smartbomb or drive in the hangar.
    if let Some(act) = crate::pvp_kit::stock_to_fit(m, o) {
        return [prep, vec![act]].concat();
    }
    if crate::camp::kit_pending(m, o) {
        return [prep, vec![Act::Wait(brain::SHORT_WAIT)]].concat();
    }
    // CLOAK-2, nothing from nowhere: a cloak in the hangar (stock or a
    // builder's) is fitted once; none is ever made or bought for it.
    if o.hunt.cloak.is_none()
        && let Some(item) = o.hangar.iter().find(|r| {
            CLOAK_TYPES.contains(&r.type_id)
                && !m.hunt.tackle_tried.contains(&r.item_id)
                && o.ship
                    .and_then(|s| s.type_id)
                    .is_none_or(|h| crate::acquire::hull_takes(r.type_id, h))
        })
    {
        m.hunt.tackle_tried.push(item.item_id);
        return [prep, vec![Act::FitAuto(item.item_id)]].concat();
    }
    // Hunt where the law is not: roam from a station in lawful space, or
    // after a few quiet sorties in one place.
    let Place::Docked { station } = o.place else {
        return acts;
    };
    let Some(map) = m.policy.travel.clone() else {
        return acts;
    };
    let here = map.system_of(station);
    let lawful = here.is_none_or(|s| !doc.is_ground(s));
    // L7 BOT-ECON: a motherlode heard of draws the hunter from wherever it is
    // (not when it is there already).
    let fresh = m.heard.fresh(o.now);
    let at_hot = here.is_some_and(|h| fresh.contains(&h));
    let drawn = here.is_some() && !fresh.is_empty() && !at_hot;
    if ((lawful && !at_hot) || drawn || m.hunt.quiet_sorties >= SORTIES_PER_SYSTEM)
        && m.hunt.roam_failed < ROAM_TRIES
        && let Some(here) = here
    {
        // Drawn alone (on the ground, not yet restless): only to a motherlode.
        let only_hot = !lawful && drawn && m.hunt.quiet_sorties < SORTIES_PER_SYSTEM;
        let target = roam_target_clear(
            &map,
            &doc,
            here,
            o.hunt.temper.aggression,
            m.seed
                .wrapping_add(u64::from(m.hunt.kills) + m.hunt.recent.len() as u64),
            &m.hunt.recent,
            &fresh,
            m.hunt.status,
        )
        .filter(|&st| !only_hot || map.system_of(st).is_some_and(|s| fresh.contains(&s)));
        match target {
            Some(station) => return [prep, vec![Act::Hunt(HuntAct::Roam { station })]].concat(),
            None if lawful => {
                return [
                    prep,
                    vec![
                        Act::Missing("a hunting-ground station within reach"),
                        Act::Wait(brain::MISSING_WAIT),
                    ],
                ]
                .concat();
            }
            None => {}
        }
    }
    acts
}

/// In space: the module header's hunt.
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let (Some(park), Place::InSpace { system, .. }) = (&o.park, o.place) else {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    };
    let Some(doc) = m.policy.pvp.clone() else {
        return escort::in_space(m, o);
    };
    // BP-CAMP: a field left running is stopped before any early return.
    if let Some(act) = crate::camp::field_check(m, o) {
        return vec![act];
    }
    if !m.hunt.safety_set {
        m.hunt.safety_set = true;
        return vec![Act::Hunt(HuntAct::Safety)];
    }
    let ground = doc.is_ground(system);
    let mut said: Vec<Act> = Vec::new();
    book_prey(m, o, &mut said);

    let mine = my_class(o, &doc);
    let ego = park.ego;
    let t = o.hunt.temper;
    // R-REL-5: in high sec only a war target counts. BRAWL: any odds will do.
    let odds_now = if m.policy.brawl {
        f64::INFINITY
    } else if ground {
        odds(mine, &ego, &o.hunt, &doc)
    } else {
        war_odds(mine, &ego, &o.hunt, &doc)
    };

    // 2. Retreat: warp off when losing, and from NPC fire as the escort does.
    // BRAWL: never.
    if !m.policy.brawl && m.homing.is_none() && m.hunt.prey.is_some() {
        let npc = m
            .policy
            .profiles
            .as_deref()
            .is_some_and(|p| escort::incoming_dps(p, park, &ego) > escort::RETREAT_AT);
        let why = retreat(o.health, t, odds_now).or(npc.then_some(Retreat::Armour));
        if let Some(why) = why {
            m.homing = Some(Homing::Repair);
            m.target = None;
            m.hunt.prey = None;
            if o.chat {
                said.push(Act::Say(why.line().to_owned()));
            }
        }
    }

    // 1. Prey, on hunting ground -- and in high sec only a war target
    // (R-REL-5: `war::legal` holds, CONCORD does not answer).
    let prey = if m.homing.is_none() && !o.guns.is_empty() {
        let engaged = m.hunt.prey.as_ref().map(|k| k.id);
        if m.policy.brawl && ground {
            pick(engaged, mine, &ego, &o.hunt, &doc, odds_now, |p| p.hostile).cloned()
        } else if ground {
            choose_prey(engaged, mine, &ego, &o.hunt, &doc).cloned()
        } else {
            choose_war_prey(engaged, mine, &ego, &o.hunt, &doc).cloned()
        }
    } else {
        None
    };
    // BP-EWAR-AIM: with no prey on the hook, a scramble or a bubble is dealt
    // with as every other role does it (the hunter never asked, so a held
    // hunter neither burned out of a bubble nor used its drive to disengage).
    if prey.is_none()
        && let Some(acts) = crate::scram::decide(m, o).or_else(|| crate::warp_bubble::decide(m, o))
    {
        return [said, acts].concat();
    }
    // CLOAK-2 rule B: prey chosen, the cloak comes off first.
    if prey.is_some()
        && let Some(kit) = o.hunt.cloak.filter(|k| k.active)
    {
        return [said, vec![Act::Hunt(HuntAct::Uncloak(kit))]].concat();
    }
    // A lock held back by the recalibration after a decloak.
    if prey.is_some() && m.hunt.lock_hold.is_some_and(|t| o.now < t) {
        return [said, vec![Act::Wait(brain::SHORT_WAIT)]].concat();
    }
    let acts = match &prey {
        Some(p) => fight(m, o, park, p, said),
        None if m.policy.brawl && m.homing.is_none() && !o.guns.is_empty() => {
            [said, vec![brawl_close(&ego, &o.hunt, &doc)]].concat()
        }
        None => quiet(m, o, &doc, park, system, ground, said),
    };
    let allow = crate::ewar_aim::legal_ids(m, o);
    guard_with(acts, o, prey.as_ref().map(|p| p.ball.id), &allow)
}

/// BRAWL, nobody in reach: close on the nearest hostile ship on the grid
/// (out past [`NEAR`]), or hold where it is until the other side shows up.
pub fn brawl_close(ego: &Ball, scene: &Scene, doc: &Ground) -> Act {
    scene
        .pilots
        .iter()
        .filter(|p| p.hostile && doc.class_of(p.type_id) != Class::Capsule)
        .min_by(|x, y| ego.surface_to(&x.ball).total_cmp(&ego.surface_to(&y.ball)))
        .map_or(Act::Wait(brain::SHORT_WAIT), |p| {
            escort::warp_or_approach(ego, &p.ball, escort::WARP_STANDOFF)
        })
}

/// The bot's own hull class; the Rifter's when it cannot tell. **[A]**
pub(crate) fn my_class(o: &Observation, doc: &Ground) -> Class {
    match doc.class_of(o.ship.and_then(|s| s.type_id)) {
        Class::Unknown => Class::Small,
        c => c,
    }
}

/// Book the prey's going. It vanished: a wreck where it was says it died,
/// and none inside [`KILL_WINDOW`] that it left (or died in a world that
/// makes no wreck for its hull). Either way the killmails are read
/// ([`HuntAct::Settle`]): they, not the wreck, say whether the bot dealt the
/// final blow, and count it.
pub(crate) fn book_prey(m: &mut Memory, o: &Observation, said: &mut Vec<Act>) {
    let allies: Vec<Group> = o
        .hunt
        .pilots
        .iter()
        .filter(|p| !p.hostile)
        .map(|p| society::group_of(&p.who))
        .collect();
    if let Some(mark) = m.hunt.prey.clone() {
        match o.hunt.pilots.iter().find(|p| p.ball.id == mark.id) {
            Some(p) => {
                m.hunt.prey = Some(Mark {
                    id: mark.id,
                    position: p.ball.position,
                    allies,
                });
            }
            None => {
                m.hunt.prey = None;
                m.target = None;
                m.hunt.gone = Some((mark, o.now));
            }
        }
    }
    if let Some((mark, at)) = m.hunt.gone.clone() {
        let wreck = o
            .hunt
            .wrecks
            .iter()
            .any(|w| dist(w.position, mark.position) <= WRECK_NEAR);
        if wreck || o.now.saturating_duration_since(at) >= KILL_WINDOW {
            m.hunt.gone = None;
            if wreck && o.chat {
                said.push(Act::Say(format!("{PREY_NAME} down")));
            }
            m.hunt.unsettled = false;
            m.hunt.settled_at = Some(o.now);
            said.push(Act::Hunt(HuntAct::Settle {
                allies: mark.allies,
            }));
        }
    }
}

/// The fight with `prey` (module header, rule 3).
pub(crate) fn fight(
    m: &mut Memory,
    o: &Observation,
    park: &ParkView,
    prey: &Pilot,
    mut said: Vec<Act>,
) -> Vec<Act> {
    let new = m.target.as_ref().map(|(id, _)| *id) != Some(prey.ball.id);
    if new {
        m.target = Some((prey.ball.id, PREY_NAME.to_owned()));
        m.belt_waits = 0;
        if o.chat {
            said.push(Act::Say(format!("Engaging {PREY_NAME}")));
        }
    }
    let allies: Vec<Group> = o
        .hunt
        .pilots
        .iter()
        .filter(|p| !p.hostile)
        .map(|p| society::group_of(&p.who))
        .collect();
    m.hunt.prey = Some(Mark {
        id: prey.ball.id,
        position: prey.ball.position,
        allies,
    });
    // A fleet's commander calls it the primary (C7).
    let calls = if m.fleet.has_fleet() {
        crate::fleet::acts(m.fleet.call_primary(prey.ball.id, prey.type_id))
    } else {
        Vec::new()
    };
    // The escort's fight, shown the prey as the only hostile and a whole
    // ship: this module's thresholds have decided to stay.
    let mut seen = o.clone();
    seen.health = None;
    if let Some(p) = &mut seen.park {
        p.hostiles = vec![Hostile {
            ball: prey.ball,
            type_id: prey.type_id.unwrap_or(0),
            name: PREY_NAME.to_owned(),
            shooting: None,
        }];
    }
    // BP-KIT: the kite reset, then the smartbombs before the escort's acts.
    if let Some(reset) = crate::pvp_kit::kite_reset(m, o, &prey.ball, escort::fight_range(&o.guns))
    {
        return [calls, said, reset].concat();
    }
    let lawful = prey.war
        || matches!(o.place, Place::InSpace { system, .. }
            if m.policy.pvp.as_ref().is_some_and(|d| d.is_ground(system)));
    let bombs = crate::pvp_kit::bomb_now(o, lawful);
    let _ = park;
    // BP-CAMP: a dictor's probe and a heavy interdictor's field.
    let camp = crate::camp::acts(m, o, &prey.ball);
    let acts = tackle_first(escort::in_space(m, &seen), &o.guns);
    // BP-CAMP: a camper holds its spot; the fight comes to it.
    let acts = if crate::camp::camping(m, o) && m.homing.is_none() {
        crate::camp::hold_position(acts)
    } else {
        acts
    };
    [calls, said, camp, bombs, acts].concat()
}

/// Nobody to fight (module header, rule 5): the escort's rats and loot --
/// pilots' wrecks too -- then the scan and the next belt.
fn quiet(
    m: &mut Memory,
    o: &Observation,
    _doc: &Ground,
    park: &ParkView,
    system: i64,
    ground: bool,
    said: Vec<Act>,
) -> Vec<Act> {
    let mut seen = o.clone();
    if let Some(p) = &mut seen.park {
        p.wrecks.extend(o.hunt.wrecks.iter().copied());
    }
    // The way home from a filament trip, when due.
    if let Some(act) = crate::filament::home_step(m, o) {
        return [said, vec![act]].concat();
    }
    // Bored of home with a filament in the hold: new ground.
    if let Some(act) = crate::filament::jump_step(m, o, ground) {
        return [said, vec![act]].concat();
    }
    // An ESS main bank nobody defends: tried when the system's pilots are
    // none (a fresh, empty sweep), and carried on once begun.
    let empty_sweep = o
        .hunt
        .sweep
        .as_ref()
        .is_some_and(|s| s.ships == 0 && s.fresh(o.now, system, &park.ego));
    if ground
        && (empty_sweep || m.hunt.ess.stage != crate::ess_raid::Stage::Idle)
        && m.homing.is_none()
        && m.policy.gang.is_none()
        && let Some(acts) = crate::ess_raid::step(m, o, system)
    {
        return [said, acts].concat();
    }
    // BP-CAMP: refill the launcher, stop a field nobody needs.
    let camp = crate::camp::quiet(m, o);
    if !camp.is_empty() {
        return [said, camp].concat();
    }
    // BP-CAMP: a lone camper (no gang) takes the gate of its system and holds
    // it; it patrols no further.
    if crate::camp::camping(m, o)
        && m.homing.is_none()
        && m.policy.gang.is_none()
        && crate::camp::gate_of(m, system).is_some()
    {
        let post = crate::camp::post(m, o, park, system).unwrap_or(Act::Wait(brain::SHORT_WAIT));
        return [said, vec![post]].concat();
    }
    let acts = escort::in_space(m, &seen);
    // CLOAK-2 rule B: a running cloak comes off before the escort acts.
    if let Some(kit) = o.hunt.cloak.filter(|k| uncloak_before(k, &acts, false)) {
        return [said, vec![Act::Hunt(HuntAct::Uncloak(kit))]].concat();
    }
    if !(ground || o.hunt.war_on) || m.homing.is_some() || acts != [Act::Wait(escort::PATROL_WAIT)]
    {
        return [said, acts].concat();
    }
    // CLOAK-2 rule A: idle and clear of everything, sit cloaked.
    if let Some(kit) = o.hunt.cloak
        && cloak_now(&kit, park, &o.hunt, m.hunt.cloak_off_at, o.now)
    {
        return [said, vec![Act::Hunt(HuntAct::Cloak(kit))]].concat();
    }
    [said, scan_step(m, o, park, system, acts)].concat()
}

/// The patrol's end: scan, then go where the scan sees pilots; an empty
/// system sends the bot home.
fn scan_step(
    m: &mut Memory,
    o: &Observation,
    park: &ParkView,
    system: i64,
    patrol: Vec<Act>,
) -> Vec<Act> {
    let fresh = o
        .hunt
        .sweep
        .as_ref()
        .filter(|s| s.fresh(o.now, system, &park.ego));
    let Some(sweep) = fresh else {
        let belts: Vec<(i64, [f64; 3])> = park
            .belts
            .iter()
            .filter(|b| park.ego.surface_to(b) >= crate::MIN_WARP_DISTANCE)
            .map(|b| (b.id, b.position))
            .collect();
        if belts.is_empty() || m.hunt.scan_failed >= SCAN_TRIES {
            return patrol;
        }
        return vec![Act::Hunt(HuntAct::DScan {
            belts,
            ego: park.ego.position,
            system,
        })];
    };
    if sweep.ships == 0 {
        m.hunt.empty_sweeps += 1;
        if m.hunt.empty_sweeps >= EMPTY_SWEEPS_MAX {
            m.homing = Some(Homing::NoRocks);
            return brain::go_home(m, o, park);
        }
        return patrol;
    }
    m.hunt.empty_sweeps = 0;
    let picked = pick_belt(sweep, &m.hunt.visited, park);
    // CLOAK-2 rule B: a cloak that is not warp-safe comes off before the warp
    // (the server refuses `CantWarpWhileCloaked`).
    if picked.is_some()
        && let Some(kit) = o.hunt.cloak.filter(|k| k.active && !k.warp_safe)
    {
        return vec![Act::Hunt(HuntAct::Uncloak(kit))];
    }
    match picked {
        Some(belt) => {
            m.hunt.visited.push(belt.id);
            m.belt = Some(belt.id);
            vec![escort::warp_or_approach(
                &park.ego,
                &belt,
                escort::WARP_STANDOFF,
            )]
        }
        None => {
            // The pilots are somewhere the cones did not reach: start the
            // round again.
            m.hunt.visited.clear();
            patrol
        }
    }
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, kills {}, reconnects {}",
        s.name, s.phase, s.kills, s.reconnects
    )
}

/// The escort's kit and a scrambler with the skill for it.
fn setup(lacks: Lacks) -> Vec<String> {
    let mut lines = vec![format!("/giveskill {} 1", escort::SKILL_GUNNERY)];
    if lacks.ship {
        lines.push(format!("/createitem {}", escort::RIFTER));
    }
    if lacks.module {
        lines.push(format!("/createitem {} {}", escort::GUN, escort::WANT_GUNS));
        lines.push(format!("/giveskill {SKILL_PROPULSION_JAMMING} 1"));
        lines.push(format!("/createitem {SCRAMBLER}"));
        lines.push(format!("/createitem {STASIS_WEBIFIER}"));
    }
    if lacks.ammo {
        lines.push(format!(
            "/createitem {} {}",
            escort::AMMO,
            escort::SETUP_ROUNDS
        ));
    }
    lines
}

// ---------------------------------------------------------------------------
// The verbs
// ---------------------------------------------------------------------------

/// One ship or object the directional scanner reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanRow {
    pub id: i64,
    pub type_id: i64,
    pub group_id: i64,
}

fn attr_int(v: &PyValue, key: &str) -> Option<i64> {
    match crate::attributes(v) {
        PyValue::Dict(entries) => entries
            .iter()
            .find(|(_, k)| match k {
                PyValue::Bytes(b) => b == key.as_bytes(),
                PyValue::Unicode(s) => s == key,
                _ => false,
            })
            .and_then(|(value, _)| match value {
                PyValue::Int(n) => Some(*n),
                _ => None,
            }),
        _ => None,
    }
}

fn attr_real(v: &PyValue, key: &str) -> Option<f64> {
    match crate::attributes(v) {
        PyValue::Dict(entries) => entries
            .iter()
            .find(|(_, k)| match k {
                PyValue::Bytes(b) => b == key.as_bytes(),
                PyValue::Unicode(s) => s == key,
                _ => false,
            })
            .and_then(|(value, _)| match value {
                PyValue::Real(r) => Some(*r),
                PyValue::Int(n) => Some(*n as f64),
                _ => None,
            }),
        _ => None,
    }
}

impl Conn {
    /// **The directional scan**: `ConeScan(scanAngle, scanRange, x, y, z)`
    /// on the system scan manager (`directionalScanSvc.DirectionalScan:54`
    /// calls `GetScanMan().ConeScan(scanAngle, scanRange, *direction)`
    /// **[V]**): the angle the cone's whole width in radians, the range in
    /// metres, the direction a three-vector. The answer is a list of
    /// objects with `.id`, `.typeID` and `.groupID` (the three attributes
    /// the palette reads **[V]**); the scanner's own ship is not in it.
    pub fn directional_scan(
        &mut self,
        manager: &ObjectRef,
        angle: f64,
        range: f64,
        direction: [f64; 3],
    ) -> Result<Vec<ScanRow>> {
        let outcome = self.call_on(
            manager,
            "ConeScan",
            vec![
                PyValue::Real(angle),
                PyValue::Real(range),
                PyValue::Real(direction[0]),
                PyValue::Real(direction[1]),
                PyValue::Real(direction[2]),
            ],
            vec![],
        )?;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!("ConeScan: {key}")));
        }
        let Some(PyValue::List(rows)) = outcome.value() else {
            return Err(Error::Refused("ConeScan: not a list".to_owned()));
        };
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some(ScanRow {
                    id: attr_int(r, "id")?,
                    type_id: attr_int(r, "typeID")?,
                    group_id: attr_int(r, "groupID").unwrap_or(0),
                })
            })
            .collect())
    }
}

/// Does a scan row name a pilot's ship: a hull the doctrine knows, with an
/// id outside the NPC range.
pub fn is_pilot_ship(row: &ScanRow, doc: &Ground) -> bool {
    !(eve_server::npc::MIN_NPC_ID..eve_server::npc::END_NPC_ID).contains(&row.id)
        && doc.is_ship(row.type_id)
}

/// **Scan** (module header, rule 5): the sphere over the whole system, then
/// a [`DSCAN_NARROW`]-degree cone at each belt reaching [`BELT_SLACK`]
/// past it. Ships already on the grid (`known`) are not counted.
pub fn sweep(
    conn: &mut Conn,
    doc: &Ground,
    belts: &[(i64, [f64; 3])],
    ego: [f64; 3],
    system: i64,
    known: &BTreeSet<i64>,
) -> Result<Sweep> {
    let manager = conn.system_scan_manager()?;
    let count = |rows: Vec<ScanRow>| {
        rows.iter()
            .filter(|r| is_pilot_ship(r, doc) && !known.contains(&r.id))
            .count()
    };
    let full = std::f64::consts::TAU;
    let sphere = conn.directional_scan(&manager, full, DSCAN_RANGE, [1.0, 0.0, 0.0])?;
    let mut hulls = BTreeMap::new();
    for r in sphere.iter().filter(|r| is_pilot_ship(r, doc)) {
        *hulls.entry(r.type_id).or_insert(0u32) += 1;
    }
    let ships = count(sphere);
    let mut toward = Vec::new();
    if ships > 0 {
        for &(id, at) in belts {
            let v = [at[0] - ego[0], at[1] - ego[1], at[2] - ego[2]];
            let reach = dist(at, ego) + BELT_SLACK;
            let n = count(conn.directional_scan(&manager, DSCAN_NARROW.to_radians(), reach, v)?);
            if n > 0 {
                toward.push((id, n));
            }
        }
    }
    Ok(Sweep {
        at: Instant::now(),
        system,
        origin: ego,
        ships,
        hulls,
        toward,
    })
}

/// **Roam** (module header, rule 5): fly to `station` over the gates at any
/// security and dock; the system it ends in.
pub fn roam(conn: &mut Conn, map: &Map, station: i64) -> Result<Option<i64>> {
    crate::travel::go(
        conn,
        Trips {
            map,
            floor: crate::travel::ANY_SECURITY,
        },
        station,
    )?;
    Ok(map.system_of(station))
}

/// One killmail row about the bot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    pub id: i64,
    pub victim: Affiliation,
    pub killer: Affiliation,
    pub isk_lost: f64,
}

/// What a [`settle`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Settled {
    /// Reports of the bot's kills new to this run (counted as kills).
    pub kills: u32,
    /// Reports of the bot's losses new to this run.
    pub losses: u32,
    /// Reports newly folded into the matrix (it may have been another
    /// bot's reading that folded the rest).
    pub recorded: u32,
    /// Allies' `AllyFight`s recorded.
    pub fights: u32,
}

/// The reports about `me` among `rows` (`GetRecentShipKillsAndLosses`
/// rows, C6): kills where it dealt the final blow, losses where it was the
/// victim; NPC kills carry no final character and are skipped.
pub fn reports_of(rows: &[PyValue], me: i64) -> Vec<Report> {
    rows.iter()
        .filter_map(|r| {
            let victim = attr_int(r, "victimCharacterID")?;
            let killer = attr_int(r, "finalCharacterID")?;
            if victim == killer || (victim != me && killer != me) {
                return None;
            }
            Some(Report {
                id: attr_int(r, "killID")?,
                victim: Affiliation {
                    character_id: victim,
                    corporation_id: attr_int(r, "victimCorporationID").unwrap_or(0),
                    alliance_id: attr_int(r, "victimAllianceID"),
                },
                killer: Affiliation {
                    character_id: killer,
                    corporation_id: attr_int(r, "finalCorporationID").unwrap_or(0),
                    alliance_id: attr_int(r, "finalAllianceID"),
                },
                isk_lost: attr_real(r, "iskLost").unwrap_or(0.0),
            })
        })
        .collect()
}

/// **Fold the reports into the society** (module header, rule 6): each
/// report is counted once as a kill or a loss of this run (`counted`; none
/// on the `first` reading, whose reports are older than the run) and put
/// into the matrix once (`Hall::record_kill`, whoever reads it first) as an
/// `Event::Kill`; a kill of the bot's also an `AllyFight` with each of
/// `allies` of another group. Without a hall nothing is recorded.
pub fn settle_into(
    hall: Option<&Hall>,
    me: i64,
    reports: &[Report],
    allies: &[Group],
    counted: &mut BTreeSet<i64>,
    first: bool,
) -> Settled {
    let mut out = Settled::default();
    for r in reports {
        let mine = r.killer.character_id == me;
        if counted.insert(r.id) && !first {
            if mine {
                out.kills += 1;
            } else {
                out.losses += 1;
            }
        }
        let (victim, killer) = (society::group_of(&r.victim), society::group_of(&r.killer));
        if victim == killer {
            continue;
        }
        let e = society::Event::Kill {
            victim,
            killer,
            isk: r.isk_lost,
        };
        let Some(hall) = hall else { continue };
        if !hall.record_kill(r.id, &e) {
            continue;
        }
        out.recorded += 1;
        if mine {
            for &g in allies.iter().filter(|g| **g != killer) {
                hall.record(&society::Event::AllyFight { a: killer, b: g });
                out.fights += 1;
            }
        }
    }
    out
}

/// **Settle** (module header, rule 6): read the killmails the bot is on and
/// fold them in.
pub fn settle(
    conn: &mut Conn,
    hall: Option<&Hall>,
    allies: &[Group],
    counted: &mut BTreeSet<i64>,
    first: bool,
) -> Result<Settled> {
    settle_sensed(conn, hall, allies, counted, first, None)
}

/// [`settle`], and the rows read also go to the doctrine layer's `sensor`
/// (DOC-3): every attacker's hull and weapon, the victim's whole fit.
pub fn settle_sensed(
    conn: &mut Conn,
    hall: Option<&Hall>,
    allies: &[Group],
    counted: &mut BTreeSet<i64>,
    first: bool,
    sensor: Option<&mut crate::doctrine_sense::Sensor>,
) -> Result<Settled> {
    let Some(me) = conn.session().char_id() else {
        return Ok(Settled::default());
    };
    let outcome = conn.call(
        "charMgr",
        "GetRecentShipKillsAndLosses",
        // The second argument is the page's upper bound, a kill id
        // (`killmail::involving`'s `before`): `None` is the newest page --
        // `Int(0)` would be "before kill 0", an empty one.
        vec![PyValue::Int(crate::corpmind::KILLS_READ), PyValue::None],
    )?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!(
            "GetRecentShipKillsAndLosses: {key}"
        )));
    }
    let Some(PyValue::List(rows)) = outcome.value() else {
        return Ok(Settled::default());
    };
    if let Some(sensor) = sensor {
        sensor.on_kill_rows(&rows);
    }
    Ok(settle_into(
        hall,
        me,
        &reports_of(&rows, me),
        allies,
        counted,
        first,
    ))
}

impl Settled {
    /// The act log's note.
    pub fn summary(&self) -> String {
        format!(
            "{} kills, {} losses new; {} reports and {} ally fights into the matrix",
            self.kills, self.losses, self.recorded, self.fights
        )
    }
}

#[cfg(test)]
mod tests {
    //! The hunter's pure rules over invented values: the fixture doctrine,
    //! invented ids, and the constants by name -- no [A] number is an
    //! expected value.
    use super::*;
    use crate::brain::{Policy, ShipInfo};
    use crate::corpmind::{CorpMind, Hall, Member};
    use crate::mind::{TRAIT_COUNT, Traits};
    use crate::society::Satisfaction;
    use std::sync::Arc;

    const SYSTEM: i64 = 30_000_900;
    const HIGH: i64 = 30_000_901;
    const SMALL_HULL: i64 = 9_001;
    const MEDIUM_HULL: i64 = 9_002;
    const LARGE_HULL: i64 = 9_003;
    const POD: i64 = 9_004;
    const ME: i64 = 90_000_901;
    const CORP: i64 = 98_000_901;
    const OTHER_CORP: i64 = 98_000_902;

    fn doctrine() -> Ground {
        Ground::new()
            .with_security(SYSTEM, -0.4)
            .with_security(HIGH, 0.9)
            .with_class(SMALL_HULL, Class::Small)
            .with_class(MEDIUM_HULL, Class::Medium)
            .with_class(LARGE_HULL, Class::Large)
            .with_class(POD, Class::Capsule)
    }

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 20.0,
        }
    }

    fn aff(character_id: i64, corporation_id: i64) -> Affiliation {
        Affiliation {
            character_id,
            corporation_id,
            alliance_id: None,
        }
    }

    fn pilot(id: i64, hull: i64, x: f64, hostile: bool, relation: f64) -> Pilot {
        Pilot {
            ball: ball(id, x),
            char_id: id + 1_000,
            type_id: Some(hull),
            who: aff(id + 1_000, if hostile { OTHER_CORP } else { CORP }),
            hostile,
            relation,
            war: false,
        }
    }

    fn scene(pilots: Vec<Pilot>, aggression: f64) -> Scene {
        Scene {
            pilots,
            temper: Temper {
                aggression,
                risk: 0.5,
            },
            ..Scene::default()
        }
    }

    fn ego() -> Ball {
        ball(1, 0.0)
    }

    // ---- hunting ground ----

    /// The first act of a hunter, docked or in space, is the safety set to
    /// red, once (CW-1 refuses a suspect-making attack on green).
    #[test]
    fn a_hunter_sets_its_safety_red_before_anything_else_and_once() {
        let policy = hunter().policy;
        let mut m = Memory::new("Invented Hunter", policy);
        assert!(!m.hunt.safety_set);
        let o = ready_docked(10);
        let first = docked(&mut m, &o);
        assert_eq!(first, vec![Act::Hunt(HuntAct::Safety)]);
        assert!(m.hunt.safety_set);
        assert_ne!(docked(&mut m, &o), first);
    }

    /// Low and null are ground, high sec and an unknown system are not,
    /// and wormhole and void ids are ground whatever the table says.
    #[test]
    fn only_low_null_and_wormhole_space_is_hunting_ground() {
        let d = doctrine()
            .with_security(30_000_902, 0.3)
            .with_security(30_000_903, 0.0)
            .with_security(30_000_904, 0.04);
        assert!(d.is_ground(SYSTEM));
        assert!(d.is_ground(30_000_902));
        assert!(d.is_ground(30_000_903));
        assert!(!d.is_ground(HIGH));
        // 0.04 is lifted to 0.05 by the client's rounding: still low sec.
        assert!(d.is_ground(30_000_904));
        assert!(!d.is_ground(30_000_999), "a system nothing is known of");
        assert!(d.is_ground(WORMHOLE_SYSTEMS.start + 5));
        assert!(d.is_ground(VOID_SYSTEMS.start + 5));
        assert!(!Ground::new().is_ground(SYSTEM), "an empty doctrine");
    }

    // ---- prey: the counters (BP-COUNTER) ----

    /// A tackler on me is the prey before a nearer, hated one; the bubble's
    /// source is, while I am held; a missile ship under a guidance
    /// disruptor takes the nearest.
    #[test]
    fn the_counters_name_the_prey() {
        const DICTOR_HULL: i64 = 970_800_001;
        let d = doctrine().with_fit(DICTOR_HULL, crate::counter::BUBBLE_GROUPS[0], 0, 0);
        let d = d.with_class(DICTOR_HULL, Class::Small);
        let near = pilot(11, SMALL_HULL, 1_000.0, true, -9.0);
        let tackler = pilot(12, SMALL_HULL, 30_000.0, true, 0.0);
        let bubbler = pilot(13, DICTOR_HULL, 30_000.0, true, 0.0);
        let mut s = scene(vec![near.clone(), tackler, bubbler], 1.0);
        let pick = |s: &Scene| choose_prey(None, Class::Large, &ego(), s, &d).map(|p| p.ball.id);
        assert_eq!(pick(&s), Some(11), "no counter: the score as before");
        s.priority = vec![12];
        assert_eq!(pick(&s), Some(12));
        s.priority.clear();
        assert_eq!(pick(&s), Some(11), "not held: a dictor is just a pilot");
        s.held = true;
        assert_eq!(pick(&s), Some(13));
        s.held = false;
        s.nearest_first = true;
        assert_eq!(pick(&s), Some(11));
    }

    // ---- prey ----

    /// The relation first: the same hulls at the same distance, the pilot
    /// the society hates most is the prey; a friend's pilot, not hostile,
    /// never is.
    #[test]
    fn prey_is_the_most_hated_hostile_and_never_an_ally() {
        let d = doctrine();
        let s = scene(
            vec![
                pilot(11, SMALL_HULL, 5_000.0, true, 0.0),
                pilot(12, SMALL_HULL, 5_000.0, true, -4.0),
                pilot(13, SMALL_HULL, 1_000.0, false, -9.0),
            ],
            1.0,
        );
        let p = choose_prey(None, Class::Small, &ego(), &s, &d).expect("prey");
        assert_eq!(p.ball.id, 12);
        let only_ally = scene(vec![pilot(13, SMALL_HULL, 1_000.0, false, -9.0)], 1.0);
        assert!(choose_prey(None, Class::Small, &ego(), &only_ally, &d).is_none());
    }

    /// An easier hull and a nearer one score higher; the one engaged stays
    /// prey over a better one.
    #[test]
    fn the_weaker_the_nearer_and_the_engaged_one_stays() {
        let d = doctrine();
        let weak = pilot(21, SMALL_HULL, 30_000.0, true, 0.0);
        let strong = pilot(22, MEDIUM_HULL, 30_000.0, true, 0.0);
        let s = scene(vec![strong.clone(), weak.clone()], 1.0);
        assert_eq!(
            choose_prey(None, Class::Medium, &ego(), &s, &d)
                .expect("prey")
                .ball
                .id,
            21
        );
        // The same hull: the nearer.
        let near = pilot(23, SMALL_HULL, 2_000.0, true, 0.0);
        let far = pilot(24, SMALL_HULL, 40_000.0, true, 0.0);
        let s = scene(vec![far.clone(), near.clone()], 1.0);
        assert_eq!(
            choose_prey(None, Class::Small, &ego(), &s, &d)
                .expect("prey")
                .ball
                .id,
            23
        );
        // Engaged stays.
        assert_eq!(
            choose_prey(Some(24), Class::Small, &ego(), &s, &d)
                .expect("prey")
                .ball
                .id,
            24
        );
    }

    /// The risk rule: nothing above the bot's size unless reckless, never a
    /// pod, never past [`NEAR`], and a hull it cannot class counts medium.
    #[test]
    fn the_risk_rule_bounds_the_size_the_range_and_pods() {
        let d = doctrine();
        let medium = scene(
            vec![pilot(31, MEDIUM_HULL, 5_000.0, true, 0.0)],
            UP_AT - 0.01,
        );
        assert!(choose_prey(None, Class::Small, &ego(), &medium, &d).is_none());
        // A reckless frigate takes a cruiser only with a cruiser beside it
        // (the odds), and a timid-enough one never.
        let backed = |a| {
            scene(
                vec![
                    pilot(31, MEDIUM_HULL, 5_000.0, true, 0.0),
                    pilot(36, MEDIUM_HULL, 5_000.0, false, 0.0),
                ],
                a,
            )
        };
        assert!(choose_prey(None, Class::Small, &ego(), &backed(1.0), &d).is_some());
        assert!(choose_prey(None, Class::Small, &ego(), &backed(UP_AT - 0.01), &d).is_none());
        let alone = scene(vec![pilot(31, MEDIUM_HULL, 5_000.0, true, 0.0)], 1.0);
        assert!(
            choose_prey(None, Class::Small, &ego(), &alone, &d).is_none(),
            "alone, the odds say no"
        );
        let large = scene(vec![pilot(32, LARGE_HULL, 5_000.0, true, 0.0)], 1.0);
        assert!(
            choose_prey(None, Class::Small, &ego(), &large, &d).is_none(),
            "two sizes up is never prey"
        );
        let pod = scene(vec![pilot(33, POD, 5_000.0, true, 0.0)], 1.0);
        assert!(choose_prey(None, Class::Small, &ego(), &pod, &d).is_none());
        let far = scene(
            vec![pilot(34, SMALL_HULL, NEAR + 100_000.0, true, 0.0)],
            1.0,
        );
        assert!(choose_prey(None, Class::Small, &ego(), &far, &d).is_none());
        let unknown = scene(
            vec![
                pilot(35, 424_242, 5_000.0, true, 0.0),
                pilot(37, MEDIUM_HULL, 5_000.0, false, 0.0),
            ],
            1.0,
        );
        assert!(
            choose_prey(None, Class::Small, &ego(), &unknown, &d).is_some(),
            "an unclassed hull counts as medium: a reckless frigate with a cruiser takes it"
        );
    }

    /// The odds: allies on the grid help, other hostile pilots count against
    /// it, and the bar falls with aggression.
    #[test]
    fn the_odds_count_allies_and_every_hostile_on_the_grid() {
        let d = doctrine();
        let lone = scene(vec![pilot(41, SMALL_HULL, 5_000.0, true, 0.0)], 0.5);
        assert!((odds(Class::Small, &ego(), &lone, &d) - 1.0).abs() < 1e-9);
        let pair = scene(
            vec![
                pilot(41, SMALL_HULL, 5_000.0, true, 0.0),
                pilot(42, SMALL_HULL, 5_000.0, true, 0.0),
            ],
            0.5,
        );
        assert!((odds(Class::Small, &ego(), &pair, &d) - 0.5).abs() < 1e-9);
        let backed = scene(
            vec![
                pilot(41, SMALL_HULL, 5_000.0, true, 0.0),
                pilot(42, SMALL_HULL, 5_000.0, true, 0.0),
                pilot(43, SMALL_HULL, 5_000.0, false, 0.0),
                pilot(44, SMALL_HULL, 5_000.0, false, 0.0),
            ],
            0.5,
        );
        assert!((odds(Class::Small, &ego(), &backed, &d) - 1.5).abs() < 1e-9);
        let empty = scene(vec![], 0.5);
        assert!(odds(Class::Small, &ego(), &empty, &d).is_infinite());
        assert!(required_odds(0.0) > required_odds(0.5));
        assert!(required_odds(0.5) > required_odds(1.0));
    }

    /// A timid hunter declines a fight a reckless one takes: the same grid.
    #[test]
    fn aggression_sets_which_fights_are_taken() {
        let d = doctrine();
        let grid = |a| {
            scene(
                vec![
                    pilot(51, SMALL_HULL, 5_000.0, true, 0.0),
                    pilot(52, SMALL_HULL, 5_000.0, true, 0.0),
                ],
                a,
            )
        };
        assert!(choose_prey(None, Class::Small, &ego(), &grid(0.0), &d).is_none());
        // A reckless frigate takes two frigates only if its bar allows.
        let bar = required_odds(1.0);
        let taken = choose_prey(None, Class::Small, &ego(), &grid(1.0), &d).is_some();
        assert_eq!(taken, 0.5 >= bar, "two on one is odds 0.5");
    }

    // ---- retreat ----

    fn health(armour: f64, hull: f64) -> Option<Health> {
        Some(Health {
            shield: 0.0,
            armour,
            hull,
        })
    }

    /// Retreat thresholds follow aggression: the timid leave sooner; a whole
    /// ship at good odds never leaves; losing odds do.
    #[test]
    fn the_retreat_thresholds_follow_aggression() {
        let timid = Temper {
            aggression: 0.0,
            risk: 0.5,
        };
        let reckless = Temper {
            aggression: 1.0,
            risk: 0.5,
        };
        assert!(armour_floor(0.0) > armour_floor(1.0));
        assert!(hull_floor(0.0) > hull_floor(1.0));
        // Between the two floors: the timid go, the reckless stay.
        let between = (armour_floor(0.0) + armour_floor(1.0)) / 2.0;
        assert_eq!(
            retreat(health(between, 1.0), timid, f64::INFINITY),
            Some(Retreat::Armour)
        );
        assert_eq!(retreat(health(between, 1.0), reckless, f64::INFINITY), None);
        // Under the reckless floor everyone goes.
        let low = armour_floor(1.0) / 2.0;
        assert_eq!(
            retreat(health(low, 1.0), reckless, f64::INFINITY),
            Some(Retreat::Armour)
        );
        // The hull first.
        assert_eq!(
            retreat(health(1.0, 0.01), reckless, f64::INFINITY),
            Some(Retreat::Hull)
        );
        assert_eq!(retreat(None, timid, f64::INFINITY), None);
        // Odds collapsing.
        let bad = required_odds(1.0) * ABORT_ODDS / 2.0;
        assert_eq!(retreat(None, reckless, bad), Some(Retreat::Odds));
        let fine = required_odds(1.0);
        assert_eq!(retreat(None, reckless, fine), None);
    }

    // ---- the fight ----

    fn gun(id: i64, type_id: i64) -> GunInfo {
        GunInfo {
            item_id: id,
            type_id,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: None,
            capacity: 0,
            range: 2_000.0,
            reload: Duration::ZERO,
            kind: match tackle_kind(type_id) {
                Some(TackleKind::Warp) => crate::brain::GunKind::Scram,
                Some(TackleKind::Web) => crate::brain::GunKind::Web,
                None => crate::brain::GunKind::Weapon,
            },
            reach: 0.0,
        }
    }

    /// Tackle first, guns after, order kept inside each group; other acts
    /// keep their place after the shots.
    #[test]
    fn tackle_modules_fire_before_guns() {
        let guns = [gun(1, escort::GUN), gun(2, escort::GUN), gun(3, SCRAMBLER)];
        let acts = vec![
            Act::Fire { gun: 1, target: 9 },
            Act::Fire { gun: 2, target: 9 },
            Act::Fire { gun: 3, target: 9 },
            Act::Wait(Duration::ZERO),
        ];
        let got = tackle_first(acts, &guns);
        assert_eq!(
            got,
            vec![
                Act::Fire { gun: 3, target: 9 },
                Act::Fire { gun: 1, target: 9 },
                Act::Fire { gun: 2, target: 9 },
                Act::Wait(Duration::ZERO),
            ]
        );
        assert!(is_tackle(SCRAMBLER) && is_tackle(DISRUPTOR) && !is_tackle(escort::GUN));
    }

    fn observation(pilots: Vec<Pilot>, aggression: f64) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::InSpace {
            system: SYSTEM,
            bound: true,
        };
        o.ship = Some(ShipInfo {
            ship_id: 1,
            type_id: Some(SMALL_HULL),
        });
        o.park = Some(ParkView {
            ego: ego(),
            ego_warping: false,
            rocks: vec![],
            belts: vec![
                ball(70, 20_000.0),
                ball(71, 900_000.0),
                ball(72, 1_200_000.0),
            ],
            station: Some(ball(80, 3_000_000.0)),
            boss: None,
            taken: BTreeMap::new(),
            hostiles: vec![],
            wrecks: vec![],
            ego_tether: None,
        });
        o.lock_range = 50_000.0;
        o.guns = vec![gun(1, escort::GUN), gun(2, escort::GUN)];
        o.hunt = scene(pilots, aggression);
        o
    }

    fn hunter() -> Memory {
        let mut policy = Policy::fast();
        policy.role = &ROLE;
        policy.pvp = Some(Arc::new(doctrine()));
        let mut m = Memory::new("Invented Hunter", policy);
        m.hunt.safety_set = true;
        m
    }

    /// **The guard**: whatever the fight produced, no lock, orbit or shot
    /// at a pilot that is not the prey survives -- an ally least of all.
    #[test]
    fn the_guard_drops_every_act_at_a_pilot_that_is_not_prey() {
        let o = observation(
            vec![
                pilot(61, SMALL_HULL, 3_000.0, true, 0.0),
                pilot(62, SMALL_HULL, 3_000.0, false, 0.0),
            ],
            1.0,
        );
        let acts = vec![
            Act::Lock(62),
            Act::Orbit { id: 62, range: 1.0 },
            Act::Fire { gun: 1, target: 62 },
            Act::Lock(61),
        ];
        assert_eq!(guard(acts.clone(), &o, Some(61)), vec![Act::Lock(61)]);
        assert_eq!(
            guard(acts, &o, None),
            vec![Act::Wait(brain::SHORT_WAIT)],
            "with no prey, a turn of nothing but pilots waits"
        );
    }

    /// BP-EWAR-AIM: the guard lets through what an ewar module is aimed at.
    #[test]
    fn the_guard_with_an_allow_list_keeps_the_shots_at_a_lawful_foe() {
        let o = observation(
            vec![
                pilot(61, SMALL_HULL, 3_000.0, true, 0.0),
                pilot(62, SMALL_HULL, 3_000.0, true, 0.0),
                pilot(63, SMALL_HULL, 3_000.0, false, 0.0),
            ],
            1.0,
        );
        let acts = vec![
            Act::Lock(62),
            Act::Fire { gun: 1, target: 62 },
            Act::Fire { gun: 1, target: 63 },
        ];
        assert_eq!(
            guard_with(acts, &o, Some(61), &[62]),
            vec![Act::Lock(62), Act::Fire { gun: 1, target: 62 }]
        );
    }

    /// BP-EWAR-AIM, the gap BP-KIT found: a hunter with no prey that is
    /// held by a warp bubble burns out like every other role.
    #[test]
    fn a_hunter_with_no_prey_in_a_bubble_burns_out() {
        let mut m = hunter();
        let mut o = observation(vec![], 1.0);
        o.warp_bubble = crate::warp_bubble::View {
            held: true,
            field: false,
            zones: vec![([-1_000.0, 0.0, 0.0], 10_000.0)],
            gates: Vec::new(),
        };
        let acts = in_space(&mut m, &o);
        assert!(
            acts.iter().any(|a| matches!(a, Act::AlignTo(_))),
            "{acts:?}"
        );
        assert!(m.warp_bubble.is_some());
    }

    /// And one that is scrambled aligns home and waits instead of roaming.
    #[test]
    fn a_scrambled_hunter_with_no_prey_aligns_home_and_waits() {
        let mut m = hunter();
        let mut o = observation(vec![], 1.0);
        o.home = Some(80);
        o.scram = crate::scram::View {
            scrambled: true,
            sources: vec![900],
            hard: true,
            ewar: Vec::new(),
        };
        let acts = in_space(&mut m, &o);
        assert!(acts.contains(&Act::AlignTo(80)), "{acts:?}");
        assert!(m.scram.is_some());
    }

    /// In space on hunting ground a hostile pilot is engaged, an allied one
    /// is spared, and the acts never name the ally.
    #[test]
    fn the_hunter_engages_the_hostile_and_spares_the_ally() {
        let mut m = hunter();
        let o = observation(
            vec![
                pilot(61, SMALL_HULL, 3_000.0, true, -2.0),
                pilot(62, SMALL_HULL, 1_000.0, false, 0.0),
            ],
            1.0,
        );
        let acts = in_space(&mut m, &o);
        assert!(!acts.is_empty());
        let names = |acts: &[Act]| -> Vec<i64> {
            acts.iter()
                .filter_map(|a| match a {
                    Act::Lock(t) | Act::Orbit { id: t, .. } | Act::Fire { target: t, .. } => {
                        Some(*t)
                    }
                    Act::WarpToItem { id, .. } => Some(*id),
                    _ => None,
                })
                .collect()
        };
        assert!(names(&acts).contains(&61), "{acts:?}");
        assert!(!names(&acts).contains(&62), "{acts:?}");
        assert_eq!(m.target.as_ref().map(|t| t.0), Some(61));
        assert_eq!(m.hunt.prey.as_ref().map(|k| k.id), Some(61));
    }

    /// High sec: nobody is prey, whatever the relation.
    #[test]
    fn high_sec_stays_safe() {
        let mut m = hunter();
        let mut o = observation(vec![pilot(61, SMALL_HULL, 3_000.0, true, -9.0)], 1.0);
        o.place = Place::InSpace {
            system: HIGH,
            bound: true,
        };
        let acts = in_space(&mut m, &o);
        assert!(m.target.is_none(), "{acts:?}");
        assert!(
            !acts.iter().any(|a| matches!(
                a,
                Act::Lock(61) | Act::Fire { target: 61, .. } | Act::Orbit { id: 61, .. }
            )),
            "{acts:?}"
        );
    }

    /// R-REL-5: in high sec a pilot at war with the bot is prey (CONCORD does
    /// not answer, `war::legal`); a hostile stranger there still is not, and
    /// the guard keeps every shot off him.
    #[test]
    fn a_war_target_in_high_sec_is_prey_and_a_stranger_is_not() {
        let mut m = hunter();
        let mut target = pilot(61, SMALL_HULL, 3_000.0, true, -9.0);
        target.war = true;
        let stranger = pilot(62, SMALL_HULL, 3_500.0, true, -9.0);
        let mut o = observation(vec![target, stranger], 1.0);
        o.place = Place::InSpace {
            system: HIGH,
            bound: true,
        };
        let acts = in_space(&mut m, &o);
        assert_eq!(m.hunt.prey.as_ref().map(|k| k.id), Some(61), "{acts:?}");
        assert!(
            !acts.iter().any(|a| matches!(
                a,
                Act::Lock(62) | Act::Fire { target: 62, .. } | Act::Orbit { id: 62, .. }
            )),
            "{acts:?}"
        );
        // Without the war, nobody is prey in high sec.
        let mut m = hunter();
        o.hunt.pilots[0].war = false;
        let acts = in_space(&mut m, &o);
        assert!(m.hunt.prey.is_none() && m.target.is_none(), "{acts:?}");
    }

    /// R-REL-5: only the war's pilots are foes in the high-sec odds; a
    /// bystander is not a foe and not an ally.
    #[test]
    fn the_high_sec_odds_count_only_the_war() {
        let mut target = pilot(61, SMALL_HULL, 3_000.0, true, 0.0);
        target.war = true;
        let by = pilot(62, LARGE_HULL, 3_500.0, true, 0.0);
        let sc = scene(vec![target.clone(), by], 0.5);
        let d = doctrine();
        let mine = Class::Small;
        assert_eq!(
            war_odds(mine, &ego(), &sc, &d),
            power(mine) / power(Class::Small)
        );
        assert!(
            odds(mine, &ego(), &sc, &d) < war_odds(mine, &ego(), &sc, &d),
            "the stranger counts against the plain odds only"
        );
        let none = scene(vec![pilot(63, SMALL_HULL, 3_000.0, true, 0.0)], 0.5);
        assert!(war_odds(mine, &ego(), &none, &d).is_infinite());
    }

    /// Losing sends the bot home: armour under its floor sets the way home
    /// and the prey is let go.
    #[test]
    fn a_fight_going_badly_ends_in_the_way_home() {
        let mut m = hunter();
        let mut o = observation(vec![pilot(61, SMALL_HULL, 3_000.0, true, 0.0)], 0.0);
        // Engage first (a reckless-enough odds: lone small vs small is 1.0).
        o.hunt.temper.aggression = 0.6;
        let _ = in_space(&mut m, &o);
        assert!(m.hunt.prey.is_some());
        o.health = health(armour_floor(0.6) / 2.0, 1.0);
        let acts = in_space(&mut m, &o);
        assert_eq!(m.homing, Some(Homing::Repair), "{acts:?}");
        assert!(m.hunt.prey.is_none());
        assert!(
            !acts
                .iter()
                .any(|a| matches!(a, Act::Lock(61) | Act::Fire { .. })),
            "{acts:?}"
        );
    }

    /// BRAWL: outnumbered three to one and with its armour nearly gone, a
    /// brawler still takes the prey and stays on it; a plain hunter does not
    /// start.
    #[test]
    fn a_brawler_takes_any_odds_and_never_breaks_off() {
        let foes = vec![
            pilot(61, SMALL_HULL, 3_000.0, true, 0.0),
            pilot(62, SMALL_HULL, 3_100.0, true, 0.0),
            pilot(63, SMALL_HULL, 3_200.0, true, 0.0),
        ];
        let mut o = observation(foes, 0.0);
        let mut plain = hunter();
        let _ = in_space(&mut plain, &o);
        assert!(
            plain.hunt.prey.is_none(),
            "a timid hunter outnumbered waits"
        );
        let mut m = hunter();
        m.policy.brawl = true;
        let _ = in_space(&mut m, &o);
        assert!(m.hunt.prey.is_some());
        o.health = health(0.01, 0.1);
        let acts = in_space(&mut m, &o);
        assert_eq!(m.homing, None, "{acts:?}");
        assert!(m.hunt.prey.is_some(), "{acts:?}");
    }

    /// BRAWL, nobody in reach: close on the nearest hostile ship (not a
    /// friend, not a capsule); nobody at all, hold.
    #[test]
    fn a_brawler_takes_a_hull_above_its_rank() {
        let sc = scene(vec![pilot(71, LARGE_HULL, NEAR / 6.0, true, 0.0)], 1.0);
        let mine = Class::Small;
        assert!(pick(None, mine, &ego(), &sc, &doctrine(), 1.0, |p| p.hostile).is_none());
        assert_eq!(
            pick(None, mine, &ego(), &sc, &doctrine(), f64::INFINITY, |p| p
                .hostile)
            .map(|p| p.ball.id),
            Some(71)
        );
    }

    #[test]
    fn a_brawler_with_nobody_in_reach_closes_on_the_nearest_foe() {
        let far = NEAR * 4.0;
        let sc = scene(
            vec![
                pilot(61, SMALL_HULL, far * 2.0, true, 0.0),
                pilot(62, MEDIUM_HULL, far, true, 0.0),
                pilot(63, POD, NEAR * 2.0, true, 0.0),
                pilot(64, SMALL_HULL, NEAR * 1.5, false, 0.0),
            ],
            1.0,
        );
        assert_eq!(
            brawl_close(&ego(), &sc, &doctrine()),
            escort::warp_or_approach(&ego(), &ball(62, far), escort::WARP_STANDOFF)
        );
        assert_eq!(
            brawl_close(&ego(), &scene(Vec::new(), 1.0), &doctrine()),
            Act::Wait(brain::SHORT_WAIT)
        );
    }

    /// A commander calls its prey as the primary, once, before the fight.
    #[test]
    fn a_fleet_commander_calls_the_prey_as_the_primary_once() {
        let mut m = hunter();
        m.fleet = crate::fleet::Command::new(0);
        m.fleet.fleet_id = Some(5);
        let o = observation(vec![pilot(61, SMALL_HULL, 3_000.0, true, 0.0)], 1.0);
        let first = in_space(&mut m, &o);
        let calls = |acts: &[Act]| {
            acts.iter()
                .filter(|a| {
                    matches!(
                        a,
                        Act::Fleet(crate::fleet::Order::Broadcast { item: 61, .. })
                    )
                })
                .count()
        };
        assert_eq!(calls(&first), 1, "{first:?}");
        let again = in_space(&mut m, &o);
        assert_eq!(calls(&again), 0, "the same primary is not called again");
    }

    /// C7's member rule finds a primary that is a hostile pilot's ship --
    /// another bot's, which is not a "human" -- and never an ally's.
    #[test]
    fn a_fleet_member_engages_a_pilot_ship_as_primary_and_never_an_allys() {
        let mut m = hunter();
        let mut o = observation(
            vec![
                pilot(61, SMALL_HULL, 3_000.0, true, 0.0),
                pilot(62, SMALL_HULL, 3_000.0, false, 0.0),
            ],
            1.0,
        );
        o.fleet = Some(brain::FleetView {
            fleet_id: 5,
            boss: None,
        });
        o.primary = Some(61);
        assert!(o.humans.is_empty(), "a bot's ship is not in the humans");
        let acts = crate::fleet::engage(&mut m, &o).expect("the primary is engaged");
        assert!(
            acts.iter().any(|a| matches!(
                a,
                Act::Lock(61) | Act::Orbit { id: 61, .. } | Act::WarpToItem { id: 61, .. }
            )),
            "{acts:?}"
        );
        o.primary = Some(62);
        assert!(
            crate::fleet::engage(&mut m, &o).is_none(),
            "an ally is no primary"
        );
        assert_eq!(
            o.hunt.primary_target(61).map(|h| h.ball.id),
            Some(61),
            "and the scene says so"
        );
        assert!(o.hunt.primary_target(62).is_none());
    }

    /// A pilot who vanishes with a wreck where it was is a kill: the
    /// killmails are read at once, with the allies on grid; one who
    /// vanishes with none is waited for through [`KILL_WINDOW`], and the
    /// killmails are read then too -- they, not the wreck, say who killed
    /// whom (a world makes no wreck for some hulls). The brain counts
    /// nothing itself.
    #[test]
    fn a_vanished_prey_has_the_killmails_read_with_or_without_a_wreck() {
        let mut m = hunter();
        let mut o = observation(
            vec![
                pilot(61, SMALL_HULL, 3_000.0, true, 0.0),
                pilot(62, SMALL_HULL, 1_000.0, false, 0.0),
            ],
            1.0,
        );
        let _ = in_space(&mut m, &o);
        assert_eq!(m.hunt.prey.as_ref().map(|k| k.id), Some(61));
        // 61 is gone; a wreck lies where it was.
        o.hunt.pilots.retain(|p| p.ball.id != 61);
        o.hunt.wrecks = vec![ball(900, 3_000.0)];
        let acts = in_space(&mut m, &o);
        let ally = society::group_of(&aff(62 + 1_000, CORP));
        assert!(
            acts.contains(&Act::Hunt(HuntAct::Settle { allies: vec![ally] })),
            "{acts:?}"
        );
        assert_eq!(m.kills, 0, "counting is the settle's");
        assert!(m.hunt.gone.is_none() && m.hunt.prey.is_none());
        // No wreck: waited for, then read all the same.
        let mut m = hunter();
        let mut o = observation(vec![pilot(61, SMALL_HULL, 3_000.0, true, 0.0)], 1.0);
        let _ = in_space(&mut m, &o);
        o.hunt.pilots.clear();
        let early = in_space(&mut m, &o);
        assert!(m.hunt.gone.is_some(), "{early:?}");
        assert!(
            !early
                .iter()
                .any(|a| matches!(a, Act::Hunt(HuntAct::Settle { .. })))
        );
        o.now += KILL_WINDOW;
        let late = in_space(&mut m, &o);
        assert!(m.hunt.gone.is_none());
        assert!(
            late.iter()
                .any(|a| matches!(a, Act::Hunt(HuntAct::Settle { .. }))),
            "{late:?}"
        );
    }

    // ---- the scan ----

    fn sweep_of(o: &Observation, ships: usize, toward: Vec<(i64, usize)>, age: Duration) -> Sweep {
        Sweep {
            at: o.now - age.min(Duration::from_millis(1)),
            system: SYSTEM,
            origin: [0.0; 3],
            ships,
            hulls: BTreeMap::new(),
            toward,
        }
    }

    /// With nobody on the grid and no fresh scan, the hunter scans; the
    /// belts it names are the park's, far enough to need a warp.
    #[test]
    fn a_quiet_grid_asks_for_a_scan_of_the_belts() {
        let mut m = hunter();
        let o = observation(vec![], 1.0);
        let acts = in_space(&mut m, &o);
        let Some(Act::Hunt(HuntAct::DScan { belts, system, .. })) = acts.first() else {
            panic!("a scan, got {acts:?}");
        };
        assert_eq!(*system, SYSTEM);
        let ids: Vec<i64> = belts.iter().map(|b| b.0).collect();
        assert_eq!(ids, vec![71, 72], "the belt it sits at needs no cone");
    }

    /// A fresh scan that sees pilots toward a belt warps there once; the
    /// belt is then visited and not warped to again; an empty scan counts
    /// toward going home.
    #[test]
    fn a_scan_that_sees_pilots_sends_the_hunter_to_the_belt_once() {
        let mut m = hunter();
        let mut o = observation(vec![], 1.0);
        o.hunt.sweep = Some(sweep_of(&o, 3, vec![(71, 1), (72, 2)], Duration::ZERO));
        let acts = in_space(&mut m, &o);
        assert!(
            matches!(acts.first(), Some(Act::WarpToItem { id: 72, .. })),
            "most pilots first: {acts:?}"
        );
        let acts = in_space(&mut m, &o);
        assert!(
            matches!(acts.first(), Some(Act::WarpToItem { id: 71, .. })),
            "then the other: {acts:?}"
        );
        // Both visited: the round starts again (a patrol wait).
        let acts = in_space(&mut m, &o);
        assert!(m.hunt.visited.is_empty(), "{acts:?}");
        // An empty system, EMPTY_SWEEPS_MAX times, is the way home.
        let mut m = hunter();
        let mut o = observation(vec![], 1.0);
        o.hunt.sweep = Some(sweep_of(&o, 0, vec![], Duration::ZERO));
        // (The first empty sweep also reads the system's ESS, once.)
        for _ in 0..=EMPTY_SWEEPS_MAX {
            let _ = in_space(&mut m, &o);
        }
        assert_eq!(m.homing, Some(Homing::NoRocks));
    }

    /// An old scan, or one taken far from here, is not trusted.
    #[test]
    fn a_scan_goes_stale_with_time_and_with_distance() {
        let o = observation(vec![], 1.0);
        let s = sweep_of(&o, 1, vec![], Duration::ZERO);
        let e = ego();
        assert!(s.fresh(o.now, SYSTEM, &e));
        assert!(!s.fresh(o.now + DSCAN_EVERY, SYSTEM, &e));
        assert!(!s.fresh(o.now, SYSTEM + 1, &e));
        let far = ball(1, SWEEP_MOVED + 1.0);
        assert!(!s.fresh(o.now, SYSTEM, &far));
    }

    /// Only a known hull outside the NPC id range is a pilot's ship.
    #[test]
    fn a_scan_row_is_a_pilot_only_for_a_known_hull_outside_the_npc_range() {
        let d = doctrine();
        let row = |id, type_id| ScanRow {
            id,
            type_id,
            group_id: 0,
        };
        assert!(is_pilot_ship(&row(100_000_001, SMALL_HULL), &d));
        assert!(!is_pilot_ship(&row(100_000_001, 777), &d), "not a hull");
        assert!(!is_pilot_ship(
            &row(eve_server::npc::MIN_NPC_ID, SMALL_HULL),
            &d
        ));
    }

    /// The belt pick: most pilots, then nearest, skipping the visited and
    /// belts the park does not show.
    #[test]
    fn the_belt_pick_prefers_pilots_then_distance_and_skips_visited() {
        let o = observation(vec![], 1.0);
        let park = o.park.as_ref().unwrap();
        let s = sweep_of(&o, 4, vec![(70, 1), (71, 3), (73, 9)], Duration::ZERO);
        assert_eq!(pick_belt(&s, &[], park).map(|b| b.id), Some(71));
        assert_eq!(pick_belt(&s, &[71], park).map(|b| b.id), Some(70));
        assert_eq!(pick_belt(&s, &[71, 70], park).map(|b| b.id), None);
        let tie = sweep_of(&o, 2, vec![(71, 1), (70, 1)], Duration::ZERO);
        assert_eq!(pick_belt(&tie, &[], park).map(|b| b.id), Some(70));
    }

    // ---- roaming ----

    fn map() -> Map {
        // 1 -- 2 -- 3 -- 4; stations in 1, 3 and 4; 3 and 4 are ground.
        Map::new()
            .with_gate(1, 101, 2, 201)
            .with_gate(2, 202, 1, 102)
            .with_gate(2, 203, 3, 301)
            .with_gate(3, 302, 2, 204)
            .with_gate(3, 303, 4, 401)
            .with_gate(4, 402, 3, 304)
            .with_station(10, 1)
            .with_station(30, 3)
            .with_station(40, 4)
    }

    fn roam_doc() -> Ground {
        Ground::new()
            .with_security(1, 0.9)
            .with_security(2, 0.8)
            .with_security(3, 0.2)
            .with_security(4, -0.5)
    }

    /// BOT-LAW: a hunter the police would attack roams only to ground it
    /// reaches without entering their net.
    #[test]
    fn a_roam_keeps_out_of_the_police_net() {
        let d = roam_doc();
        let m = map()
            .with_security(1, 0.9)
            .with_security(2, 0.8)
            .with_security(3, 0.2)
            .with_security(4, -0.5);
        let go = |from, st| roam_target_clear(&m, &d, from, 1.0, 0, &[], &[], st);
        // At -3.0 the police attack from 0.8: systems 1 and 2 are the net.
        assert_eq!(go(2, Some(-3.0)), Some(30), "out of the net is fine");
        // Inside the net, the way out is through it, however deep.
        assert_eq!(go(1, Some(-3.0)), Some(30), "two jumps deep, still out");
        // A clean start does not enter the net, and a net target never is one.
        assert_eq!(go(3, Some(-3.0)), Some(40));
        // A status nobody polices, or none read, roams as ever.
        assert_eq!(go(1, None), roam_target(&m, &d, 1, 1.0, 0, &[]));
        assert_eq!(go(1, Some(-1.0)), roam_target(&m, &d, 1, 1.0, 0, &[]));
    }

    /// A roam picks a station in a ground system within the trait's reach,
    /// nearest first, never one to avoid, and nothing when none is in
    /// reach.
    #[test]
    fn a_roam_picks_a_ground_station_in_reach_and_not_a_recent_one() {
        let (m, d) = (map(), roam_doc());
        // Reach at aggression 1 covers both; the nearest is system 3.
        assert_eq!(roam_target(&m, &d, 1, 1.0, 0, &[]), Some(30));
        assert_eq!(roam_target(&m, &d, 1, 1.0, 1, &[]), Some(40));
        assert_eq!(roam_target(&m, &d, 1, 1.0, 0, &[3]), Some(40));
        assert_eq!(roam_target(&m, &d, 1, 1.0, 0, &[3, 4]), None);
        // The seed only chooses among the nearest few.
        assert_eq!(roam_target(&m, &d, 1, 1.0, 2, &[]), Some(30));
        // Not into the system it is in, and not out of reach.
        assert_eq!(roam_target(&m, &d, 3, 1.0, 0, &[]), Some(40));
        assert_eq!(
            roam_target(&m, &d, 1, 0.0, 0, &[]),
            Some(30),
            "even the timid go two jumps"
        );
        assert_eq!(roam_target(&m, &Ground::new(), 1, 1.0, 0, &[]), None);
        assert!(roam_jumps(0.0) <= roam_jumps(1.0));
    }

    /// L7 BOT-ECON: a motherlode heard of is where a hunter roams first, and
    /// it and the systems a gate from it are ground to roam to; the law's
    /// idea of hunting ground is untouched.
    #[test]
    fn hunter_ground_includes_the_motherlodes_it_heard() {
        let (m, d) = (map(), roam_doc());
        // Without the line, seed 0 goes to the nearer system 3.
        assert_eq!(roam_target_with(&m, &d, 1, 1.0, 0, &[], &[]), Some(30));
        // With a motherlode in 4, that is where it goes, whatever the seed.
        for seed in 0..4 {
            assert_eq!(roam_target_with(&m, &d, 1, 1.0, seed, &[], &[4]), Some(40));
        }
        // An empty doctrine has no ground, but a motherlode is somewhere to go,
        // and so is the system a gate from it.
        assert_eq!(
            roam_target_with(&m, &Ground::new(), 1, 1.0, 0, &[], &[]),
            None
        );
        assert_eq!(
            roam_target_with(&m, &Ground::new(), 1, 1.0, 0, &[], &[3]),
            Some(30)
        );
        assert_eq!(
            roam_target_with(&m, &Ground::new(), 1, 1.0, 0, &[3], &[3]),
            Some(40),
            "the gate's far side"
        );
        // Not the law's ground: a heard motherlode in high sec is no licence
        // to engage there.
        assert!(!d.is_ground(2));
    }

    /// Docked in lawful space the hunter roams, once its kit is ready;
    /// docked in a hunting-ground station it undocks and hunts; after the
    /// quiet sorties it roams on.
    #[test]
    fn a_hunter_docked_in_lawful_space_roams_and_one_on_the_ground_undocks() {
        let mut policy = Policy::fast();
        policy.role = &ROLE;
        policy.pvp = Some(Arc::new(roam_doc()));
        policy.travel = Some(Arc::new(map()));
        let mut m = Memory::new("Invented Hunter", policy);
        m.hunt.safety_set = true;
        m.hunt.settled_at = Some(Instant::now());
        let mut o = ready_docked(10);
        let acts = docked(&mut m, &o);
        assert!(
            matches!(
                acts.first(),
                Some(Act::Hunt(HuntAct::Roam { station: 30 | 40 }))
            ),
            "{acts:?}"
        );
        // On the ground: undock.
        o.place = Place::Docked { station: 30 };
        let acts = docked(&mut m, &o);
        assert_eq!(acts, vec![Act::Undock]);
        // After the quiet sorties: roam on, not back.
        m.hunt.quiet_sorties = SORTIES_PER_SYSTEM;
        let acts = docked(&mut m, &o);
        assert!(
            matches!(acts.first(), Some(Act::Hunt(HuntAct::Roam { station: 40 }))),
            "{acts:?}"
        );
        // Failed roams in a row: it hunts where it is.
        m.hunt.roam_failed = ROAM_TRIES;
        assert_eq!(docked(&mut m, &o), vec![Act::Undock]);
    }

    /// A roam's result is remembered; a failure is counted and a success
    /// resets it.
    #[test]
    fn a_roam_is_remembered_and_a_failure_counted() {
        let mut h = Hunt::default();
        h.on_roam(false, None);
        h.on_roam(false, None);
        assert_eq!(h.roam_failed, 2);
        h.quiet_sorties = 5;
        h.on_roam(true, Some(3));
        assert_eq!((h.roam_failed, h.quiet_sorties), (0, 0));
        for s in 10..20 {
            h.on_roam(true, Some(s));
        }
        assert_eq!(h.recent.len(), ROAM_MEMORY);
        assert_eq!(h.recent.last(), Some(&19));
    }

    fn row(item_id: i64, type_id: i64, quantity: i64) -> crate::ItemRow {
        crate::ItemRow {
            item_id,
            type_id,
            owner_id: ME,
            location_id: 60_000_001,
            flag_id: crate::FLAG_HANGAR,
            quantity,
            group_id: 0,
            category_id: if type_id == escort::AMMO {
                crate::CATEGORY_CHARGE
            } else {
                7
            },
            singleton: quantity == 1,
        }
    }

    // ---- CLOAK-2: the cloak, only from stock ----

    fn kit(effect: (&'static str, i64), warp_safe: bool, active: bool) -> CloakKit {
        CloakKit {
            item_id: 4_100,
            type_id: CLOAK_TYPES[0],
            effect,
            warp_safe,
            targeting_delay: Duration::from_millis(560),
            reactivation: Duration::from_secs(30),
            active,
        }
    }

    const CLOAK_607: (&str, i64) = ("cloaking", 607);
    const CLOAK_980: (&str, i64) = ("cloakingWarpSafe", 980);

    /// Rule A: idle, no prey, clear of everything: sit cloaked. Anything
    /// within reach, a sweep that sees pilots or a fresh decloak says no.
    #[test]
    fn an_idle_hunter_with_a_cloak_in_the_clear_cloaks_and_not_beside_things() {
        let mut m = hunter();
        let mut o = observation(vec![], 1.0);
        o.hunt.cloak = Some(kit(CLOAK_607, false, false));
        let acts = in_space(&mut m, &o);
        assert_eq!(
            acts,
            vec![Act::Hunt(HuntAct::Cloak(kit(CLOAK_607, false, false)))]
        );
        // A rock beside it: the scan goes on as without a cloak.
        let mut near = o.clone();
        near.park.as_mut().expect("a park").rocks = vec![ball(90, 1_500.0)];
        let acts = in_space(&mut m, &near);
        assert!(
            !acts
                .iter()
                .any(|a| matches!(a, Act::Hunt(HuntAct::Cloak(_)))),
            "{acts:?}"
        );
        // Not before the module's own reactivation delay has run.
        m.hunt.cloak_off_at = Some(o.now);
        assert!(!cloak_now(
            &kit(CLOAK_607, false, false),
            o.park.as_ref().expect("a park"),
            &o.hunt,
            m.hunt.cloak_off_at,
            o.now + Duration::from_secs(5)
        ));
        // Without a cloak in the fit: nothing changes.
        let mut m = hunter();
        let plain = observation(vec![], 1.0);
        let acts = in_space(&mut m, &plain);
        assert!(
            !acts
                .iter()
                .any(|a| matches!(a, Act::Hunt(HuntAct::Cloak(_)))),
            "{acts:?}"
        );
    }

    /// Rule B: prey chosen, the cloak comes off before anything else.
    #[test]
    fn a_hunter_decloaks_before_it_engages_prey() {
        let mut m = hunter();
        let mut o = observation(vec![pilot(61, SMALL_HULL, 3_000.0, true, -2.0)], 1.0);
        let k = kit(CLOAK_980, true, true);
        o.hunt.cloak = Some(k);
        assert_eq!(in_space(&mut m, &o), vec![Act::Hunt(HuntAct::Uncloak(k))]);
        assert!(m.hunt.prey.is_none(), "the fight starts after the decloak");
    }

    /// A cloak that is not warp-safe comes off for any move; a warp-safe one
    /// only for a lock or a shot (pure).
    #[test]
    fn only_the_covert_cloak_stays_on_for_a_warp() {
        let warp = vec![Act::WarpToItem {
            id: 70,
            range: 1_000.0,
        }];
        assert!(uncloak_before(&kit(CLOAK_607, false, true), &warp, false));
        assert!(!uncloak_before(&kit(CLOAK_980, true, true), &warp, false));
        let lock = vec![Act::Lock(61)];
        assert!(uncloak_before(&kit(CLOAK_980, true, true), &lock, false));
        let wait = vec![Act::Wait(escort::PATROL_WAIT)];
        assert!(!uncloak_before(&kit(CLOAK_607, false, true), &wait, false));
        assert!(!uncloak_before(&kit(CLOAK_607, false, false), &warp, true));
    }

    /// A lock refused with `DeniedTargetAfterCloak` is retried at the type's
    /// own delay; no other refusal is held.
    #[test]
    fn a_lock_refused_after_a_cloak_is_held_for_the_modules_delay() {
        let now = Instant::now();
        let k = kit(CLOAK_607, false, false);
        assert_eq!(
            lock_hold("AddTarget: DeniedTargetAfterCloak", Some(&k), now),
            Some(now + Duration::from_millis(560))
        );
        assert_eq!(lock_hold("DeniedTargetSelf", Some(&k), now), None);
    }

    /// Nothing from nowhere: a cloak in the hangar is fitted once; with none
    /// there, the hunter fits none.
    #[test]
    fn a_hunter_fits_a_cloak_only_from_its_hangar_and_only_once() {
        let mut m = hunter();
        m.hunt.settled_at = Some(Instant::now());
        let mut o = ready_docked(10);
        o.hangar = vec![row(4_200, CLOAK_TYPES[1], 1)];
        let acts = docked(&mut m, &o);
        assert_eq!(acts, vec![Act::FitAuto(4_200)]);
        let again = docked(&mut m, &o);
        assert_ne!(again, vec![Act::FitAuto(4_200)]);
        let mut m = hunter();
        m.hunt.settled_at = Some(Instant::now());
        let acts = docked(&mut m, &ready_docked(10));
        assert!(
            !acts.iter().any(|a| matches!(a, Act::FitAuto(_))),
            "{acts:?}"
        );
    }

    /// A docked observation of a fitted, loaded Rifter, ready to undock.
    fn ready_docked(station: i64) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station };
        o.ship = Some(ShipInfo {
            ship_id: 1,
            type_id: Some(escort::RIFTER),
        });
        let mut g = gun(1, escort::GUN);
        g.rounds = Some(10);
        g.capacity = 10;
        o.guns = vec![g, {
            let mut g = gun(2, escort::GUN);
            g.rounds = Some(10);
            g.capacity = 10;
            g
        }];
        o.cargo = vec![row(900, escort::AMMO, escort::SPARE_ROUNDS)];
        o
    }

    /// A hunter without the opt-in never undocks and says so; the
    /// opt-in check names the flag.
    #[test]
    fn a_hunter_without_the_doctrine_says_missing_and_the_opt_in_is_checked() {
        let mut policy = Policy::fast();
        policy.role = &ROLE;
        let mut m = Memory::new("Invented Hunter", policy);
        m.hunt.safety_set = true;
        let acts = docked(&mut m, &ready_docked(10));
        assert!(matches!(acts.first(), Some(Act::Missing(_))), "{acts:?}");
        assert!(check_opt_in([&ROLE], false).is_err());
        assert!(check_opt_in([&ROLE], true).is_ok());
        assert!(check_opt_in([&crate::roles::ESCORT], false).is_ok());
        assert!(crate::roles::parse("hunter").is_some());
    }

    /// fix-w8b B1: with the drone bay on, the first pass is [Stock, Undock];
    /// the tackle and the kit are still fitted before the undock, one per
    /// turn, and the Stock is not lost.
    #[test]
    fn the_first_undock_keeps_the_stock_and_still_fits_the_kit() {
        let mut policy = Policy::fast();
        policy.role = &ROLE;
        policy.combat_drones = true;
        policy.pvp = Some(Arc::new(doctrine()));
        let mut m = Memory::new("Invented Hunter", policy);
        m.hunt.safety_set = true;
        m.hunt.settled_at = Some(Instant::now());
        let mut o = ready_docked(10);
        o.hangar = vec![
            row(777, SCRAMBLER, 1),
            row(778, crate::pvp_kit::STOCK_TYPES[0], 1),
        ];
        let stock = Act::Drone(crate::drone_use::DroneAct::Stock);
        let first = docked(&mut m, &o);
        assert_eq!(first, vec![stock, Act::FitAuto(777)]);
        assert_eq!(docked(&mut m, &o), vec![Act::FitAuto(778)]);
        assert_eq!(docked(&mut m, &o), vec![Act::Undock]);
    }

    /// A scrambler in the hangar is fitted once before the hunt.
    #[test]
    fn a_scrambler_in_the_hangar_is_fitted_once() {
        let mut policy = Policy::fast();
        policy.role = &ROLE;
        policy.pvp = Some(Arc::new(doctrine()));
        let mut m = Memory::new("Invented Hunter", policy);
        m.hunt.safety_set = true;
        m.hunt.settled_at = Some(Instant::now());
        let mut o = ready_docked(10);
        o.hangar = vec![row(777, SCRAMBLER, 1)];
        assert_eq!(docked(&mut m, &o), vec![Act::FitAuto(777)]);
        assert_eq!(docked(&mut m, &o), vec![Act::Undock], "not twice");
    }

    /// SEC-1c: a hunter holding Clone Soldier tags asks the security office
    /// once, then not again before [`crate::tag_repair::EVERY`]; one with no
    /// tags (open mode, which cannot buy) never does.
    #[test]
    fn a_hunter_with_clone_tags_asks_the_office_once_per_interval() {
        let mut policy = Policy::fast();
        policy.role = &ROLE;
        policy.pvp = Some(Arc::new(doctrine()));
        let mut m = Memory::new("Invented Hunter", policy);
        m.hunt.safety_set = true;
        m.hunt.settled_at = Some(Instant::now());
        let mut o = ready_docked(10);
        let tag = eve_server::security_office::BANDS[1].type_id;
        assert_eq!(docked(&mut m, &o), vec![Act::Undock], "no tags, no ask");
        // A commodity (category 17), which the ratter's sale never lists.
        o.hangar = vec![crate::ItemRow {
            category_id: 17,
            ..row(778, tag, 2)
        }];
        assert_eq!(docked(&mut m, &o), vec![Act::Hunt(HuntAct::Office)]);
        assert_eq!(docked(&mut m, &o), vec![Act::Undock], "not again at once");
        o.now += crate::tag_repair::EVERY;
        m.hunt.settled_at = Some(o.now);
        assert_eq!(docked(&mut m, &o), vec![Act::Hunt(HuntAct::Office)]);
    }

    /// A web in the hangar is fitted beside the scrambler already fitted, once;
    /// a second scrambler is not, as the kind is covered (K-EWAR).
    #[test]
    fn a_web_in_the_hangar_is_fitted_beside_the_scrambler() {
        let mut policy = Policy::fast();
        policy.role = &ROLE;
        policy.pvp = Some(Arc::new(doctrine()));
        let mut m = Memory::new("Invented Hunter", policy);
        m.hunt.safety_set = true;
        m.hunt.settled_at = Some(Instant::now());
        let mut o = ready_docked(10);
        o.guns.push(gun(5, SCRAMBLER));
        o.hangar = vec![row(778, SCRAMBLER_II, 1), row(777, STASIS_WEBIFIER, 1)];
        assert_eq!(docked(&mut m, &o), vec![Act::FitAuto(777)]);
        assert_eq!(docked(&mut m, &o), vec![Act::Undock], "not twice");
        assert_eq!(tackle_kind(STASIS_WEBIFIER_II), Some(TackleKind::Web));
        assert_eq!(tackle_kind(escort::GUN), None);
    }

    /// The killmails are read when docked after a kill and then every
    /// [`SETTLE_EVERY`].
    #[test]
    fn the_killmails_are_read_after_a_kill_and_now_and_then() {
        let mut policy = Policy::fast();
        policy.role = &ROLE;
        policy.pvp = Some(Arc::new(doctrine()));
        let mut m = Memory::new("Invented Hunter", policy);
        m.hunt.safety_set = true;
        let o = ready_docked(10);
        let first = docked(&mut m, &o);
        assert!(
            matches!(first.first(), Some(Act::Hunt(HuntAct::Settle { .. }))),
            "{first:?}"
        );
        assert_eq!(docked(&mut m, &o), vec![Act::Undock]);
        m.hunt.unsettled = true;
        m.hunt.allies = vec![5];
        assert_eq!(
            docked(&mut m, &o),
            vec![Act::Hunt(HuntAct::Settle { allies: vec![5] })]
        );
        let mut later = o.clone();
        later.now += SETTLE_EVERY;
        assert!(matches!(
            docked(&mut m, &later).first(),
            Some(Act::Hunt(HuntAct::Settle { .. }))
        ));
    }

    /// The setup lines are the escort's and a scrambler with its skill.
    #[test]
    fn the_setup_gives_the_escorts_kit_and_a_scrambler() {
        let lines = setup(Lacks {
            ship: true,
            module: true,
            ammo: true,
            isk: false,
        });
        for want in [
            format!("/createitem {}", escort::RIFTER),
            format!("/createitem {SCRAMBLER}"),
            format!("/createitem {STASIS_WEBIFIER}"),
            format!("/giveskill {SKILL_PROPULSION_JAMMING} 1"),
            format!("/createitem {} {}", escort::AMMO, escort::SETUP_ROUNDS),
        ] {
            assert!(lines.contains(&want), "{want} in {lines:?}");
        }
        let none = setup(Lacks {
            ship: false,
            module: false,
            ammo: false,
            isk: false,
        });
        assert_eq!(none.len(), 1, "only the skill: {none:?}");
    }

    // ---- the scan verb's answer ----

    fn kv(pairs: &[(&str, PyValue)]) -> PyValue {
        PyValue::Dict(
            pairs
                .iter()
                .map(|(k, v)| (v.clone(), PyValue::Bytes(k.as_bytes().to_vec())))
                .collect(),
        )
    }

    // ---- the society ----

    fn report(id: i64, victim: Affiliation, killer: Affiliation) -> Report {
        Report {
            id,
            victim,
            killer,
            isk_lost: 5_000_000.0,
        }
    }

    fn member(corp: i64) -> Member {
        Member {
            name: "Invented".to_owned(),
            traits: Traits([0.5; TRAIT_COUNT]),
            career: "pirate".to_owned(),
            band: crate::career::Band::Low,
            corp,
            alliance: None,
            last_turn: None,
            satisfaction: Satisfaction::default(),
            applied: None,
            salt: 0,
        }
    }

    fn hall_with_bot_corps(ceos: &[(i64, i64)]) -> Hall {
        let hall = Hall::new(60);
        {
            let mut st = hall.lock();
            for &(corp, ceo) in ceos {
                st.members.insert(ceo, member(corp));
                st.corps.insert(
                    corp,
                    CorpMind {
                        id: corp,
                        ceo: Some(ceo),
                        profile: crate::society::Profile::of("pirate", crate::career::Band::Low),
                        founded: 0,
                        alliance: None,
                        name: String::new(),
                        applied_alliance: None,
                        salt: 0,
                    },
                );
            }
        }
        hall
    }

    /// **The fight lands in the matrix**: my kill moves the victim bot
    /// corp's view of mine down, my loss moves mine down toward the
    /// killer, each once however many times (or by whom) it is read; an
    /// ally beside me at a kill gains an `AllyFight`; and a report is
    /// counted as a kill or loss of the run once, none on the first
    /// reading.
    #[test]
    fn a_kill_and_a_loss_land_in_the_matrix_once_and_allies_gain_a_fight() {
        const BOB: i64 = 90_000_902;
        const BOB_CORP: i64 = 98_000_903;
        const CAS: i64 = 90_000_903;
        const CAS_CORP: i64 = 98_000_904;
        let hall = hall_with_bot_corps(&[(CORP, ME), (BOB_CORP, BOB), (CAS_CORP, CAS)]);
        let me = aff(ME, CORP);
        let bob = aff(BOB, BOB_CORP);
        let view = |h: &Hall, v: i64, t: i64| h.lock().relations.get(v, t);
        let mut counted = BTreeSet::new();
        // A kill of mine.
        let kill = report(1, bob, me);
        let got = settle_into(Some(&hall), ME, &[kill], &[CAS_CORP], &mut counted, false);
        assert_eq!(
            (got.kills, got.losses, got.recorded, got.fights),
            (1, 0, 1, 1)
        );
        assert!(view(&hall, BOB_CORP, CORP) < 0.0, "Bob's view of me");
        assert_eq!(view(&hall, CORP, BOB_CORP), 0.0, "mine of him: unmoved");
        assert!(
            view(&hall, CAS_CORP, CORP) > 0.0 && view(&hall, CORP, CAS_CORP) > 0.0,
            "the ally's view and mine, both up"
        );
        // Read again: nothing new, nothing moves.
        let snapshot = hall.snapshot().relations;
        assert_eq!(
            settle_into(Some(&hall), ME, &[kill], &[CAS_CORP], &mut counted, false),
            Settled::default()
        );
        // Bob's own turn reading the same report records nothing either.
        assert!(!hall.record_kill(
            1,
            &crate::society::Event::Kill {
                victim: BOB_CORP,
                killer: CORP,
                isk: 1.0
            }
        ));
        assert_eq!(hall.snapshot().relations, snapshot);
        // A loss of mine to Bob.
        let loss = report(2, me, bob);
        let got = settle_into(Some(&hall), ME, &[loss], &[], &mut counted, false);
        assert_eq!((got.kills, got.losses, got.recorded), (0, 1, 1));
        assert!(view(&hall, CORP, BOB_CORP) < 0.0, "my view of Bob");
        assert!(hall.snapshot().recorded.contains(&1) && hall.snapshot().recorded.contains(&2));
        // A report another bot's turn already folded in is still counted.
        let hall2 = hall_with_bot_corps(&[(CORP, ME), (BOB_CORP, BOB)]);
        assert!(hall2.record_kill(
            3,
            &crate::society::Event::Kill {
                victim: BOB_CORP,
                killer: CORP,
                isk: 1.0
            }
        ));
        let before = hall2.snapshot().relations;
        let got = settle_into(
            Some(&hall2),
            ME,
            &[report(3, bob, me)],
            &[],
            &mut BTreeSet::new(),
            false,
        );
        assert_eq!(
            (got.kills, got.recorded),
            (1, 0),
            "counted, not recorded twice"
        );
        assert_eq!(hall2.snapshot().relations, before);
        // The first reading counts nothing: those reports are older than
        // the run; they are still put in the matrix.
        let hall3 = hall_with_bot_corps(&[(CORP, ME), (BOB_CORP, BOB)]);
        let mut seen = BTreeSet::new();
        let got = settle_into(
            Some(&hall3),
            ME,
            &[report(4, bob, me)],
            &[],
            &mut seen,
            true,
        );
        assert_eq!((got.kills, got.losses, got.recorded), (0, 0, 1));
        assert!(seen.contains(&4));
        // No hall: counted, nothing recorded.
        let got = settle_into(None, ME, &[report(5, bob, me)], &[], &mut seen, false);
        assert_eq!((got.kills, got.recorded), (1, 0));
    }

    /// The killmail rows of the client's shape are read as reports: kills and
    /// losses of mine only, NPC kills (no final character) skipped.
    #[test]
    fn killmail_rows_become_reports_of_my_kills_and_losses_only() {
        let row = |id: i64, victim: i64, killer: Option<i64>| {
            let mut pairs = vec![
                ("killID", PyValue::Int(id)),
                ("victimCharacterID", PyValue::Int(victim)),
                ("victimCorporationID", PyValue::Int(CORP)),
                ("iskLost", PyValue::Real(2.5)),
            ];
            if let Some(k) = killer {
                pairs.push(("finalCharacterID", PyValue::Int(k)));
                pairs.push(("finalCorporationID", PyValue::Int(OTHER_CORP)));
                pairs.push(("finalAllianceID", PyValue::Int(99_000_001)));
            }
            kv(&pairs)
        };
        let rows = vec![
            row(1, 7, Some(ME)),
            row(2, ME, Some(8)),
            row(3, 7, Some(8)),
            row(4, ME, None),
            row(5, ME, Some(ME)),
        ];
        let got = reports_of(&rows, ME);
        let ids: Vec<i64> = got.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(got[0].killer.character_id, ME);
        assert_eq!(got[1].killer.alliance_id, Some(99_000_001));
        assert!((got[0].isk_lost - 2.5).abs() < 1e-9);
    }

    /// The hall's state file keeps what the matrix has already counted.
    #[test]
    fn recorded_reports_survive_the_state_file() {
        let hall = hall_with_bot_corps(&[(CORP, ME)]);
        assert!(hall.record_kill(
            77,
            &crate::society::Event::Kill {
                victim: CORP,
                killer: OTHER_CORP,
                isk: 1.0
            }
        ));
        let text = hall.lock().render();
        let back = crate::corpmind::HallState::parse(&text, 60).expect("reads back");
        assert!(back.recorded.contains(&77));
        assert_eq!(back.relations, hall.snapshot().relations);
    }

    fn tracked(id: i64, x: f64, slim: crate::park::SlimInfo) -> crate::Tracked {
        crate::Tracked {
            ball_id: id,
            motion: eve_server::space_state::Motion {
                position: [x, 0.0, 0.0],
                velocity: [0.0; 3],
                mode: eve_server::destiny::Mode::Stop,
                speed_fraction: 0.0,
            },
            radius: 20.0,
            since: 0.0,
            slim: Some(slim),
            own_hull: None,
        }
    }

    fn ship_slim(char_id: i64, corp: i64, alliance: Option<i64>) -> crate::park::SlimInfo {
        crate::park::SlimInfo {
            type_id: Some(SMALL_HULL),
            owner_id: Some(char_id),
            launcher_id: None,
            loot_rights: None,
            char_id: Some(char_id),
            corp_id: Some(corp),
            alliance_id: alliance,
            hacking_state: None,
            gate_range: None,
            security_status: None,
            wormhole_class: None,
        }
    }

    /// NW-7: a ship owned by CONCORD or by a police faction's corporation is
    /// the law (the escort leaves it alone unless it shoots); a pirate's is not.
    #[test]
    fn the_law_is_known_by_its_owner() {
        use crate::driver::is_law;
        let content = eve_server::content::Content::fixture();
        let owned = |owner: i64| {
            tracked(
                eve_server::npc::MIN_NPC_ID + 1,
                0.0,
                crate::park::SlimInfo {
                    type_id: Some(990_330_011),
                    owner_id: Some(owner),
                    ..crate::park::SlimInfo::default()
                },
            )
        };
        assert!(is_law(
            Some(&content),
            &owned(eve_server::concord::CONCORD_CORP)
        ));
        assert!(is_law(
            Some(&content),
            &owned(eve_server::concord::FIXTURE_POLICE_CORP)
        ));
        assert!(!is_law(Some(&content), &owned(1_999_011)));
        assert!(
            is_law(None, &owned(eve_server::concord::CONCORD_CORP)),
            "CONCORD needs no content"
        );
        assert!(!is_law(
            None,
            &owned(eve_server::concord::FIXTURE_POLICE_CORP)
        ));
    }

    /// GAP-LOOT: a rat's wreck owned by a player is still a rat's (the
    /// hunt does not list it), and the escort may take it only by the
    /// client's `HaveLootRight` -- its own, its fleet's, abandoned, NPC-owned.
    #[test]
    fn a_players_rat_wreck_is_not_a_pilot_wreck_and_loot_follows_the_rights() {
        use crate::driver::{is_npc_wreck, may_loot, npc_launched};
        let wreck_type = eve_server::item::WRECK_TYPES[0].type_id;
        let (me, other, fleet) = (90_000_901, 90_000_902, 7);
        let rat = eve_server::npc::MIN_NPC_ID + 5;
        let slim = |owner: i64, fleet: Option<i64>, abandoned: bool, launcher: i64| {
            crate::park::SlimInfo {
                type_id: Some(wreck_type),
                owner_id: Some(owner),
                launcher_id: Some(launcher),
                loot_rights: Some((owner, 98_000_001, fleet, abandoned)),
                ..crate::park::SlimInfo::default()
            }
        };
        let w = |s| tracked(1, 0.0, s);
        let mine = slim(me, None, false, rat);
        assert!(npc_launched(&mine));
        assert!(!is_pilot_wreck(&mine), "a rat's wreck");
        assert!(is_npc_wreck(&w(mine)));
        assert!(is_pilot_wreck(&slim(other, None, false, 100_000_001)));
        assert!(!is_npc_wreck(&w(slim(other, None, false, 100_000_001))));
        let check = |s, fleet| may_loot(&w(s), Some(me), Some(1_000_009), fleet);
        assert!(check(slim(me, None, false, rat), None), "mine");
        assert!(!check(slim(other, None, false, rat), None), "a stranger's");
        assert!(check(slim(other, Some(fleet), false, rat), Some(fleet)));
        assert!(!check(
            slim(other, Some(fleet), false, rat),
            Some(fleet + 1)
        ));
        assert!(!check(slim(other, Some(fleet), false, rat), None));
        assert!(check(slim(other, None, true, rat), None), "abandoned");
        assert!(check(slim(1_000_124, None, false, rat), None), "NPC-owned");
    }

    /// `observe` reads the slim: a different player corp is hostile, the
    /// same corp is not, the boss never; the relation comes from the hall;
    /// a wreck of a pilot's ship is listed, an NPC corp's is not.
    #[test]
    fn observe_marks_hostile_pilots_by_the_alliance_rule_and_lists_pilot_wrecks() {
        const BOB: i64 = 90_000_902;
        const BOB_CORP: i64 = 98_000_903;
        let hall = hall_with_bot_corps(&[(CORP, ME)]);
        hall.record(&crate::society::Event::Kill {
            victim: CORP,
            killer: BOB_CORP,
            isk: 1.0,
        });
        let wreck_type = eve_server::item::WRECK_TYPES[0].type_id;
        let wreck = |owner| crate::park::SlimInfo {
            type_id: Some(wreck_type),
            owner_id: Some(owner),
            ..crate::park::SlimInfo::default()
        };
        let balls = vec![
            tracked(1, 0.0, ship_slim(ME, CORP, None)),
            tracked(2, 1_000.0, ship_slim(90_000_910, CORP, None)),
            tracked(3, 2_000.0, ship_slim(BOB, BOB_CORP, None)),
            tracked(4, 3_000.0, ship_slim(90_000_911, 1_000_009, None)),
            tracked(5, 4_000.0, ship_slim(90_000_912, OTHER_CORP, None)),
            tracked(6, 5_000.0, wreck(BOB)),
            tracked(7, 6_000.0, wreck(1_000_009)),
        ];
        let me = Me {
            char_id: ME,
            who: aff(ME, CORP),
            boss: Some(90_000_912),
            ego_ball: 1,
            hall: Some(&hall),
            regard: &[],
            loyalty: 0.5,
        };
        let (pilots, wrecks) = observe_balls(&balls, &me);
        let by = |id: i64| pilots.iter().find(|p| p.ball.id == id).expect("a pilot");
        assert_eq!(pilots.len(), 4, "everyone but me and the wrecks");
        assert!(!by(2).hostile, "my own corp");
        assert!(by(3).hostile);
        assert!(by(4).hostile, "an NPC corp is nobody's friend");
        assert!(!by(5).hostile, "the boss is never hostile");
        assert!(by(3).relation < 0.0, "the hall's grudge against Bob's corp");
        assert_eq!(by(2).relation, 0.0);
        assert_eq!(
            wrecks.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![6],
            "a pilot's wreck, not an NPC corp's"
        );
        // Without a hall: only the same corp is allied.
        let me = Me { hall: None, ..me };
        let (pilots, _) = observe_balls(&balls, &me);
        assert!(!pilots.iter().find(|p| p.ball.id == 2).unwrap().hostile);
        assert!(pilots.iter().find(|p| p.ball.id == 3).unwrap().hostile);
        assert_eq!(pilots.iter().map(|p| p.relation).sum::<f64>(), 0.0);
    }

    /// R-REL-5: a pilot whose side is at war with mine is flagged `war`,
    /// a bystander and the fleet boss are not.
    #[test]
    fn the_scene_flags_the_pilots_at_war_with_me() {
        const BOB: i64 = 90_000_902;
        const BOB_CORP: i64 = 98_000_903;
        const CAS: i64 = 90_000_903;
        const CAS_CORP: i64 = 98_000_904;
        let hall = hall_with_bot_corps(&[(CORP, ME)]);
        let ft = eve_server::pyutil::filetime_now();
        hall.lock().note_war(crate::coalition::WarRow {
            id: 1,
            by: CORP,
            against: BOB_CORP,
            started: ft - 10,
            finished: None,
            mutual: false,
            open: false,
            allies: vec![],
            seen: 0,
        });
        let balls = vec![
            tracked(1, 0.0, ship_slim(ME, CORP, None)),
            tracked(3, 2_000.0, ship_slim(BOB, BOB_CORP, None)),
            tracked(4, 3_000.0, ship_slim(CAS, CAS_CORP, None)),
        ];
        let me = Me {
            char_id: ME,
            who: aff(ME, CORP),
            boss: None,
            ego_ball: 1,
            hall: Some(&hall),
            regard: &[],
            loyalty: 0.5,
        };
        let (pilots, _) = observe_balls(&balls, &me);
        let by = |id: i64| pilots.iter().find(|p| p.ball.id == id).expect("a pilot");
        assert!(by(3).war, "the corp I am at war with");
        assert!(!by(4).war, "a stranger");
        let me = Me {
            boss: Some(BOB),
            ..me
        };
        let (pilots, _) = observe_balls(&balls, &me);
        assert!(
            !pilots.iter().find(|p| p.ball.id == 3).unwrap().war,
            "the boss is never a target"
        );
    }

    /// R-REL-3: the relation a hunter holds is `R_eff` -- its own regard for
    /// the pilot and the layers -- not the corp row alone.
    #[test]
    fn hunting_uses_r_eff_not_the_corp_only_view() {
        const BOB: i64 = 90_000_902;
        const BOB_CORP: i64 = 98_000_903;
        let hall = hall_with_bot_corps(&[(CORP, ME)]);
        let balls = vec![
            tracked(1, 0.0, ship_slim(ME, CORP, None)),
            tracked(3, 2_000.0, ship_slim(BOB, BOB_CORP, None)),
        ];
        let me = |regard: &'static [crate::mind::Regard]| Me {
            char_id: ME,
            who: aff(ME, CORP),
            boss: None,
            ego_ball: 1,
            hall: Some(&hall),
            regard,
            loyalty: 0.5,
        };
        let rel = |m: &Me| observe_balls(&balls, m).0[0].relation;
        let plain = rel(&me(&[]));
        assert_eq!(plain, 0.0, "the corp row says nothing");
        static DISLIKED: [crate::mind::Regard; 1] = [crate::mind::Regard {
            who: BOB,
            score: -4.0,
            at: 0,
        }];
        assert!(
            rel(&me(&DISLIKED)) < plain,
            "the pilot's own grudge shows in the prey score"
        );
        // A listed friend is never below the file, whatever the grudge.
        hall.befriend(BOB, 3.0);
        assert!(rel(&me(&DISLIKED)) >= 3.0);
        // The pilot is remembered as a human the bots have seen.
        assert!(hall.lock().humans.contains_key(&BOB));
    }
}

#[cfg(test)]
mod filament_tests {
    //! The filament and ESS steps through the hunter's own `docked` and
    //! `in_space`, on invented scenes.
    use super::*;
    use crate::brain::{Policy, ShipInfo};
    use crate::ess_raid::{EssAct, EssView};
    use crate::filament::{self, Away, Mood};
    use std::sync::Arc;

    const SYSTEM: i64 = 30_000_900;
    const HULL: i64 = 9_001;
    const HOME: i64 = 60_000_010;
    const ELSEWHERE: i64 = 60_000_077;
    const ME: i64 = 90_000_901;
    const FILAMENT_ITEM: i64 = 7_001;

    fn hunter() -> Memory {
        let mut policy = Policy::fast();
        policy.role = &ROLE;
        policy.pvp = Some(Arc::new(
            Ground::new()
                .with_security(SYSTEM, -0.4)
                .with_class(HULL, Class::Small),
        ));
        let mut m = Memory::new("Invented Hunter", policy);
        m.hunt.safety_set = true;
        m.hunt.settled_at = Some(Instant::now());
        m.hunt.quiet_sorties = filament::BORED_MAX;
        m
    }

    fn filament_row(flag: i64) -> crate::ItemRow {
        crate::ItemRow {
            item_id: FILAMENT_ITEM,
            type_id: 900_001,
            owner_id: ME,
            location_id: HOME,
            flag_id: flag,
            quantity: 1,
            group_id: filament::GROUP,
            category_id: 5,
            singleton: false,
        }
    }

    fn bold(o: &mut Observation) {
        o.hunt.temper = Temper {
            aggression: 0.7,
            risk: 0.9,
        };
        o.hunt.mood = Mood {
            curiosity: 1.0,
            greed: 1.0,
        };
    }

    fn docked_obs(station: i64) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station };
        o.ship = Some(ShipInfo {
            ship_id: 1,
            type_id: Some(escort::RIFTER),
        });
        let mut g = GunInfo {
            item_id: 1,
            type_id: escort::GUN,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(10),
            capacity: 10,
            range: 2_000.0,
            reload: Duration::ZERO,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        };
        o.guns = vec![g, {
            g.item_id = 2;
            g
        }];
        o.cargo = vec![crate::ItemRow {
            item_id: 900,
            type_id: escort::AMMO,
            owner_id: ME,
            location_id: HOME,
            flag_id: crate::FLAG_CARGO,
            quantity: escort::SPARE_ROUNDS,
            group_id: 0,
            category_id: crate::CATEGORY_CHARGE,
            singleton: false,
        }];
        bold(&mut o);
        o
    }

    fn space_obs() -> Observation {
        let mut o = docked_obs(HOME);
        o.place = Place::InSpace {
            system: SYSTEM,
            bound: true,
        };
        let ball = |id: i64, x: f64| Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 20.0,
        };
        o.park = Some(ParkView {
            ego: ball(1, 0.0),
            ego_warping: false,
            rocks: vec![],
            belts: vec![ball(70, 20_000.0)],
            station: Some(ball(80, 3_000_000.0)),
            boss: None,
            taken: BTreeMap::new(),
            hostiles: vec![],
            wrecks: vec![],
            ego_tether: None,
        });
        o.lock_range = 50_000.0;
        o.me = ME;
        o
    }

    /// Bored of home, a hunter stocks the filament from the hangar into
    /// the hold before it undocks; a timid one does not.
    #[test]
    fn a_bored_hunter_docked_stocks_its_filament_and_a_timid_one_does_not() {
        let mut m = hunter();
        let mut o = docked_obs(HOME);
        o.hangar = vec![filament_row(crate::FLAG_HANGAR)];
        let acts = docked(&mut m, &o);
        assert!(
            acts.contains(&Act::Stock {
                item: FILAMENT_ITEM,
                quantity: 1
            }),
            "{acts:?}"
        );
        o.hunt.temper.risk = 0.0;
        assert!(!docked(&mut m, &o).iter().any(|a| matches!(
            a,
            Act::Stock {
                item: FILAMENT_ITEM,
                ..
            }
        )));
        // Nothing from nowhere: no filament in the hangar, none is asked for.
        let mut bare = docked_obs(HOME);
        bare.hangar.clear();
        assert!(!docked(&mut m, &bare).iter().any(|a| matches!(
            a,
            Act::Stock {
                item: FILAMENT_ITEM,
                ..
            }
        )));
    }

    /// In space with the filament in the hold and no prey: spend it. A hunter
    /// in a gang does not.
    #[test]
    fn a_bored_hunter_in_space_spends_the_filament_in_its_hold() {
        let mut m = hunter();
        let mut o = space_obs();
        o.cargo.push(filament_row(crate::FLAG_CARGO));
        let acts = in_space(&mut m, &o);
        assert_eq!(
            acts,
            vec![Act::Hunt(HuntAct::Filament {
                item: FILAMENT_ITEM
            })]
        );
        let mut calm = hunter();
        calm.hunt.quiet_sorties = 0;
        assert_ne!(
            in_space(&mut calm, &o),
            vec![Act::Hunt(HuntAct::Filament {
                item: FILAMENT_ITEM
            })]
        );
    }

    fn away_at(m: &mut Memory, o: &Observation) {
        m.hunt.away = Some(Away {
            home: HOME,
            since: o.now,
            kills: m.hunt.kills,
        });
        m.hunt.quiet_sorties = 0;
    }

    /// Away, it hunts; sent home for repair it flies the gates home, not to
    /// a station that is not there.
    #[test]
    fn an_away_hunter_flies_home_over_the_gates_when_sent_home_or_after_a_kill() {
        let mut m = hunter();
        let o = space_obs();
        away_at(&mut m, &o);
        let home = Act::Hunt(HuntAct::Roam { station: HOME });
        assert_ne!(in_space(&mut m, &o).first(), Some(&home));
        m.hunt.kills += 1;
        assert_eq!(in_space(&mut m, &o).first(), Some(&home));
        let mut m = hunter();
        away_at(&mut m, &o);
        m.homing = Some(Homing::Repair);
        assert_eq!(in_space(&mut m, &o).first(), Some(&home));
        // And the roam's success ends the trip.
        m.hunt.on_roam(true, Some(30_000_001));
        assert!(m.hunt.away.is_none());
    }

    #[test]
    fn docked_at_a_foreign_station_the_away_hunter_goes_home() {
        let mut m = hunter();
        let o = docked_obs(ELSEWHERE);
        away_at(&mut m, &o);
        let acts = docked(&mut m, &o);
        assert!(
            acts.contains(&Act::Hunt(HuntAct::Roam { station: HOME })),
            "{acts:?}"
        );
    }

    /// The sweep found no pilot ship in the system.
    fn empty_sweep(o: &mut Observation) {
        o.hunt.sweep = Some(Sweep {
            at: o.now,
            system: SYSTEM,
            origin: [0.0, 0.0, 0.0],
            ships: 0,
            hulls: BTreeMap::new(),
            toward: Vec::new(),
        });
    }

    fn rich_view(linked_by: Option<i64>) -> EssView {
        EssView {
            system: SYSTEM,
            beacon: 3_000_000_000_001,
            ess: 3_100_000_000_001,
            value: crate::ess_raid::FLOOR_CONTENT * 2.0,
            linked_by,
            read_at: Instant::now(),
            gap: None,
        }
    }

    /// An undefended, rich ESS: read it, warp to the beacon, link. A crowded
    /// Local, or a timid pilot, leaves it alone.
    #[test]
    fn a_hunter_reads_the_ess_then_warps_to_its_beacon_and_links() {
        let mut m = hunter();
        m.hunt.quiet_sorties = 0;
        let mut o = space_obs();
        // Before any sweep the bank is not on its mind.
        assert_ne!(
            in_space(&mut m, &o).first(),
            Some(&Act::Hunt(HuntAct::Ess(EssAct::Read { system: SYSTEM })))
        );
        empty_sweep(&mut o);
        assert_eq!(
            in_space(&mut m, &o).first(),
            Some(&Act::Hunt(HuntAct::Ess(EssAct::Read { system: SYSTEM })))
        );
        m.hunt.ess.on_read(SYSTEM, Some(rich_view(None)));
        let beacon = Act::WarpToItem {
            id: 3_000_000_000_001,
            range: 0.0,
        };
        assert_eq!(in_space(&mut m, &o).first(), Some(&beacon));
        // Landed and settled: the link is asked for.
        m.hunt.ess.stage_at = Some(o.now - crate::ess_raid::SETTLE);
        assert_eq!(
            in_space(&mut m, &o).first(),
            Some(&Act::Hunt(HuntAct::Ess(EssAct::Link)))
        );
    }

    #[test]
    fn a_defended_ess_or_a_timid_hunter_does_not_raid() {
        let mut crowded = hunter();
        crowded.hunt.quiet_sorties = 0;
        crowded.hunt.ess.on_read(SYSTEM, Some(rich_view(None)));
        let mut o = space_obs();
        empty_sweep(&mut o);
        o.threat.local = Some((1..=9).chain([ME]).collect());
        let first = in_space(&mut crowded, &o);
        assert!(
            !first.iter().any(|a| matches!(
                a,
                Act::WarpToItem {
                    id: 3_000_000_000_001,
                    ..
                }
            )),
            "{first:?}"
        );
        let mut timid = hunter();
        timid.hunt.quiet_sorties = 0;
        timid.hunt.ess.on_read(SYSTEM, Some(rich_view(None)));
        let mut o = space_obs();
        empty_sweep(&mut o);
        o.hunt.temper.risk = 0.0;
        let first = in_space(&mut timid, &o);
        assert!(
            !matches!(
                first.first(),
                Some(Act::WarpToItem {
                    id: 3_000_000_000_001,
                    ..
                })
            ),
            "{first:?}"
        );
    }
}

#[cfg(test)]
mod fit_tests {
    //! The pirate career's share, which `career.rs`'s base was fitted to.
    use super::*;

    fn share(blocked: bool) -> f64 {
        let mut s = crate::career::Signals::default();
        s.blocked.insert("mission_runner".to_owned());
        crate::career::block_capital(&mut s);
        if blocked {
            s.blocked.insert("pirate".to_owned());
        }
        let (bots, seeds) = (50, 40u64);
        let mut pirates = 0.0;
        for seed in 0..seeds {
            for i in 0..bots {
                let m = crate::mind::Mind::seeded(
                    &format!("Invented Pilot {i}"),
                    seed,
                    crate::career::CAREERS,
                    &s,
                    0,
                );
                if m.career == "pirate" {
                    pirates += 1.0;
                }
            }
        }
        pirates / (bots as u64 * seeds) as f64
    }

    /// Blocked (no `--society-pvp`), nobody is a pirate; unblocked, a few
    /// in a hundred are, and the career is served by the hunter role.
    #[test]
    fn the_pirate_career_is_a_few_percent_when_unblocked() {
        assert_eq!(share(true), 0.0);
        let s = share(false);
        assert!((0.02..=0.10).contains(&s), "{s}");
        let c = crate::career::find(crate::career::CAREERS, "pirate").expect("a row");
        assert!(c.served());
        assert!(std::ptr::eq(c.role.expect("a role"), &ROLE));
    }
}
