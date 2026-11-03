//! **Untargeted modules for the PvP bots** (BP-KIT, issue #120, L3 of
//! `handback/r-bot-pvp/report.md`): the smartbomb and the micro jump drive.
//! Neither is a gun (no target, no `weapon_effect`), so before this a fitted
//! one sat dark.
//!
//! - **Read** ([`Kit`], [`KitModule`]): the driver finds them among the
//!   fitted modules by default effect (38 `empWave`, 4921 `microJumpDrive`)
//!   and reads range, cycle (`duration`) and `moduleReactivationDelay` (669)
//!   off the module's own row.
//! - **Smartbomb** ([`bomb_now`], pure): fire when at least [`BOMB_MIN`]
//!   hostile ships are inside the field, **no** other pilot is, the ship is
//!   not in warp, the fight is lawful (hunting ground or a war) and the
//!   capacitor is above [`BOMB_CAP`]. One cycle (`repeat` 1), so a friend who
//!   arrives is safe by the next. `hunt::fight` asks it, and so does every
//!   gang fight that goes through it.
//! - **Micro jump** ([`jump_step`], pure but for the plan): three uses --
//!   out of a bubble ([`bubble_exit`]), a kite reset ([`kite_reset`]) and a
//!   disengage on the way home when only disruptors pin the ship
//!   ([`disengage`]; a running *scrambler* blocks the module, a disruptor does
//!   not, `tackle.rs`). The sequence: align on a ball in the chosen
//!   direction, wait until the ship moves (the server's facing is the
//!   velocity, `mjd.rs` **[A]**), activate, and block every move until the
//!   spool ends ([`spool_gate`]).
//! - **Refusals** `ModuleIsBlocked` and `MicroJumpDriveSpoolupPhase` are
//!   facts about the ship, not failures ([`ActKind::Kit`] is incidental):
//!   the driver backs the module off ([`Mem::blocked`]).
//!
//! Every number here is **[A]** policy and none is in a test by value.
//! Falsifiers: bots that bomb their own fleet in a soak (widen
//! [`BOMB_MARGIN`]); a bot that spends its MJD and then sits 100 km from the
//! fight for minutes (raise [`RESET_SHARE`]'s floor or drop the kite reset);
//! a jump that lands somewhere other than the aligned ball (the server's
//! facing is itself assumed).
//!
//! [`ActKind::Kit`]: crate::brain::ActKind::Kit

use crate::brain::{Act, Ball, Memory, Observation, Place, SHORT_WAIT};
use std::time::{Duration, Instant};

/// A smartbomb's range off its row, `empFieldRange` **[V]**.
const ATTR_RANGE: i64 = eve_server::smartbomb::ATTR_RANGE;
/// `moduleReactivationDelay` **[V-data]**, milliseconds.
const ATTR_REACTIVATION: i64 = 669;

/// Foes must be this far inside the field, metres **[A]**: the server measures
/// centre to centre and the foe moves.
pub const BOMB_MARGIN: f64 = 500.0;
/// Hostile ships inside the field before a bomb goes off **[A]**.
pub const BOMB_MIN: usize = 2;
/// A bomb needs the capacitor above this share **[A]**.
pub const BOMB_CAP: f64 = 0.3;
/// A jump needs the capacitor above this share **[A]** (a Medium drive draws
/// about a fifth of a battlecruiser's).
pub const MJD_CAP: f64 = 0.6;
/// After the align, the least wait before the jump **[A]**.
pub const ALIGN_WAIT: Duration = Duration::from_secs(6);
/// A ship faster than this (m/s) is facing its velocity **[A]**.
pub const MOVING: f64 = 50.0;
/// The speed a bubble exit assumes of a ship that is still slow **[A]**.
pub const ASSUMED_SPEED: f64 = 150.0;
/// A walk out of a bubble must take longer than the spool by this, to be
/// worth a jump **[A]**.
pub const JUMP_MARGIN: Duration = Duration::from_secs(8);
/// A drive's spool when its row does not say **[A]**; the report's 12 s.
pub const DEFAULT_SPOOL: Duration = Duration::from_secs(12);
/// A drive's reactivation when its row does not say **[A]**; the report's 180 s.
pub const DEFAULT_REACTIVATION: Duration = Duration::from_secs(180);
/// A blocked or spooling refusal puts the drive off this long **[A]**.
pub const BLOCKED_BACKOFF: Duration = Duration::from_secs(30);
/// An align that was not followed by a jump is forgotten after this **[A]**.
pub const PLAN_TTL: Duration = Duration::from_secs(45);
/// A kite reset needs a foe inside this share of the fighting reach **[A]**.
pub const RESET_SHARE: f64 = 0.35;
/// The reach (metres) from which a hull kites: Medium and Long **[A]**.
pub const KITE_REACH: f64 = 20_000.0;
/// The armour share a kite reset still needs **[A]**: below it the bot goes
/// home, the retreat rule's business.
pub const ARMOUR_FLOOR: f64 = 0.6;
/// How well a ball must point away from the foe to be a kite heading **[A]**
/// (cosine).
pub const AWAY: f64 = 0.7;

/// Small EMP Smartbomb I **[V-data]** (`typedata`): the drill's bomb.
pub const SMARTBOMB: i64 = 1563;
/// Large EMP Smartbomb I **[V-data]**: the drill's bomb. Its field (about
/// 5000 m less the margin) reaches the allies' orbit (about 3900 m), where
/// the Small one's cannot. No by-name lookup exists in this crate, so it is
/// a constant like [`SMARTBOMB`], which the tests keep. It needs 1000 MW of
/// powergrid **[V-data]**, which a Rifter (41 MW) cannot give, so the foes'
/// bomber flies [`DRILL_BOMB_HULL`]. **[A]** the field reaches the allies:
/// falsified by `fci_bot_smartbomb` finding an ally still outside it.
pub const DRILL_BOMB: i64 = 3993;
/// The Tempest **[V-data]** (battleship, group 27, eight high slots, 16000 MW
/// in the local bundle against the Large EMP's 1000 MW): the hull the foes'
/// bomber flies in the `smartbomb` drill. The allies keep their Rifters.
pub const DRILL_BOMB_HULL: i64 = 639;
/// Medium Micro Jump Drive **[V-data]**: the drill's drive (a battlecruiser's).
pub const MJD_MEDIUM: i64 = 33915;
/// The Hurricane **[V-data]** (battlecruiser, group 419, six projectile
/// turrets so the roamer undocks armed): a hull the Medium Micro Jump Drive
/// names (`canFitShipGroup`), so the drive drill flies it; a Rifter would be
/// refused (FIT-REFUSE). **[A]** the group list holds
/// battlecruisers: falsified by `fci_bot_mjd` finding the drive refused.
pub const DRILL_HULL: i64 = 24_702;
/// Energy Pulse Weapons and Micro Jump Drive Operation **[V-data]**: the
/// skills the two ask for at level 1.
pub const SKILL_BOMB: i64 = 3421;
pub const SKILL_MJD: i64 = 4385;
/// The smartbombs (T1 EMP, four sizes) and drives (Large, Medium) a hunter
/// fits when its stock holds one **[V-data]**.
pub const STOCK_TYPES: [i64; 6] = [3913, 1563, 3953, 3993, 4383, MJD_MEDIUM];
/// The environment variable that names a kit drill (CI only).
pub const DRILL_ENV: &str = "EVE_BOTS_KIT_DRILL";

/// **Nothing from nowhere**: the one stock item of the kit in a docked
/// hunter's hangar that is not fitted yet, fitted once. Nothing is made or
/// bought for it, as with the cloak.
pub fn stock_to_fit(m: &mut Memory, o: &Observation) -> Option<Act> {
    let flying = o.ship.and_then(|s| s.type_id);
    // BP-CAMP: a sphere launcher in the hangar is fitted too (a dictor's).
    let launcher = o.hangar.iter().find(|r| {
        crate::camp::is_launcher(r)
            && o.kit.launcher.is_none()
            && !m.hunt.tackle_tried.contains(&r.item_id)
    });
    let item = o.hangar.iter().find(|r| {
        STOCK_TYPES.contains(&r.type_id)
            && flying.is_none_or(|h| crate::acquire::hull_takes(r.type_id, h))
            && !m.hunt.tackle_tried.contains(&r.item_id)
            && if r.type_id == MJD_MEDIUM || r.type_id == 4383 {
                o.kit.mjd.is_none()
            } else {
                o.kit.bombs.is_empty()
            }
    });
    let item = item.or(launcher)?;
    m.hunt.tackle_tried.push(item.item_id);
    // A launcher fitted at the last dock is empty: the next turn's
    // `camp_dock` must load the probes before the undock.
    if crate::camp::is_launcher(item) {
        m.camp.dock_next = None;
    }
    Some(Act::FitAuto(item.item_id))
}

/// **The GM lines of a kit drill** (CI only; open mode only): the skill and
/// the part for `which` (`smartbomb` or `mjd`); `foe`: the hostile side, whose
/// bomber also gets its hull. Empty for anything else.
pub fn drill_lines(which: &str, foe: bool) -> Vec<String> {
    match which {
        "smartbomb" => {
            let mut lines = vec![
                format!("/giveskill {SKILL_BOMB} 1"),
                format!("/createitem {DRILL_BOMB}"),
            ];
            // Only the foes' bomber needs the grid of a battleship.
            if foe {
                lines.push(format!("/devskills hull {DRILL_BOMB_HULL}"));
                lines.push(format!("/createitem {DRILL_BOMB_HULL}"));
            }
            lines
        }
        "mjd" => vec![
            format!("/giveskill {SKILL_MJD} 1"),
            // The drive names its hulls: the Hurricane and the skills it asks.
            format!("/devskills hull {DRILL_HULL}"),
            format!("/createitem {DRILL_HULL}"),
            format!("/createitem {MJD_MEDIUM}"),
        ],
        _ => Vec::new(),
    }
}

/// The hull a drill boards after its lines: `mjd` [`DRILL_HULL`] on both sides,
/// `smartbomb` [`DRILL_BOMB_HULL`] on the foes' side alone.
pub fn drill_hull(which: &str, foe: bool) -> Option<i64> {
    match which {
        "mjd" => Some(DRILL_HULL),
        "smartbomb" if foe => Some(DRILL_BOMB_HULL),
        _ => None,
    }
}

/// One fitted smartbomb or drive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KitModule {
    pub item_id: i64,
    pub type_id: i64,
    /// `(name, id)`, what `Activate` is sent with.
    pub effect: (&'static str, i64),
    pub active: bool,
    /// Online (the fit fits the grid): an offline module is refused at `Activate`.
    pub online: bool,
    /// A bomb's field, metres; 0 for a drive.
    pub range: f64,
    /// One cycle (a drive's spool).
    pub cycle: Duration,
    /// `moduleReactivationDelay`.
    pub reactivation: Duration,
}

impl KitModule {
    /// A module as its row says. `attr` is the row's attribute lookup
    /// (served, else the bundle's): the cycle comes from the effect's own
    /// duration attribute, the cooldown from 669, never a constant.
    pub fn read(
        item_id: i64,
        type_id: i64,
        effect: &eve_server::modules::Effect,
        active: bool,
        online: bool,
        attr: impl Fn(i64) -> Option<f64>,
    ) -> KitModule {
        let ms = |a: i64, default: Duration| {
            attr(a)
                .filter(|v| *v > 0.0)
                .map_or(default, |v| Duration::from_millis(v.round() as u64))
        };
        KitModule {
            item_id,
            type_id,
            effect: (effect.name, effect.effect_id),
            active,
            online,
            // A field's reach is its radius (103); a bomb's is the bomb's.
            range: attr(if effect.effect_id == eve_server::bubble::EFFECT_FIELD {
                eve_server::bubble::ATTR_RANGE
            } else {
                ATTR_RANGE
            })
            .unwrap_or(0.0)
            .max(0.0),
            cycle: ms(effect.duration_attribute, DEFAULT_SPOOL),
            reactivation: ms(ATTR_REACTIVATION, DEFAULT_REACTIVATION),
        }
    }
}

/// What the ship carries of the two.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Kit {
    pub bombs: Vec<KitModule>,
    pub mjd: Option<KitModule>,
    /// BP-CAMP: a Heavy Interdictor's warp disruption field generator.
    pub field: Option<KitModule>,
    /// BP-CAMP: an interdiction sphere launcher, and the probes in it.
    pub launcher: Option<Launcher>,
}

/// **An interdiction sphere launcher** (group 589; BP-CAMP, #125). Its
/// default effect is the missile launcher's, so the driver finds it by group,
/// not by effect, and reads the probes loaded in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Launcher {
    pub item_id: i64,
    pub type_id: i64,
    pub flag_id: i64,
    /// `(name, id)`, what `Activate` is sent with.
    pub effect: (&'static str, i64),
    /// Probes loaded; `None` when the read failed.
    pub loaded: Option<i64>,
    /// The type of the probe loaded, else the one last seen.
    pub probe: Option<i64>,
    pub active: bool,
}

/// Which of the two an effect is, if either.
pub fn is_kit_effect(effect_id: i64) -> bool {
    eve_server::smartbomb::is_smartbomb(effect_id)
        || eve_server::mjd::is_mjd(effect_id)
        || effect_id == eve_server::bubble::EFFECT_FIELD
}

impl Kit {
    /// Add a module read from the ship (the driver's loop).
    pub fn push(&mut self, m: KitModule) {
        if m.effect.1 == eve_server::bubble::EFFECT_FIELD {
            self.field = Some(m);
        } else if eve_server::mjd::is_mjd(m.effect.1) {
            self.mjd = Some(m);
        } else {
            self.bombs.push(m);
        }
    }
}

/// One act of the kit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KitAct {
    /// One cycle of a smartbomb.
    Bomb(KitModule),
    /// Activate the drive.
    Jump(KitModule),
    /// BP-CAMP: launch one warp disrupt probe.
    Launch(Launcher),
    /// BP-CAMP: switch the warp disruption field on.
    FieldOn(KitModule),
    /// BP-CAMP: switch it off.
    FieldOff(KitModule),
    /// NW-11b: launch the mobile warp disruptor in the hold (`ship.Drop`).
    UnitLaunch(i64),
    /// NW-11b: anchor the unit in space (`dogmaIM.Activate(id, 649)`).
    UnitAnchor(i64),
    /// NW-11b: lift its anchor (`Activate(id, 650)`).
    UnitLift(i64),
    /// NW-11b: scoop it (`ship.Scoop(id)`).
    UnitScoop(i64),
}

impl KitAct {
    pub fn describe(&self) -> (String, Vec<i64>) {
        match self {
            KitAct::Bomb(m) => (format!("smartbomb {}", m.item_id), vec![m.item_id]),
            KitAct::Jump(m) => (format!("micro jump {}", m.item_id), vec![m.item_id]),
            KitAct::Launch(l) => (format!("launch probe from {}", l.item_id), vec![l.item_id]),
            KitAct::FieldOn(m) => (format!("field on {}", m.item_id), vec![m.item_id]),
            KitAct::FieldOff(m) => (format!("field off {}", m.item_id), vec![m.item_id]),
            KitAct::UnitLaunch(i) => (format!("launch unit {i}"), vec![*i]),
            KitAct::UnitAnchor(i) => (format!("anchor unit {i}"), vec![*i]),
            KitAct::UnitLift(i) => (format!("lift unit {i}"), vec![*i]),
            KitAct::UnitScoop(i) => (format!("scoop unit {i}"), vec![*i]),
        }
    }
}

/// Why a jump is planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Bubble,
    Kite,
    Home,
}

#[derive(Debug, Clone, PartialEq)]
struct Plan {
    ball: i64,
    since: Instant,
    why: Why,
}

/// The bot's memory of its drive.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mem {
    /// Nothing moves before this: the drive is spooling.
    pub spool_until: Option<Instant>,
    /// The drive is not tried before this (its cooldown, or a back-off).
    pub ready_at: Option<Instant>,
    plan: Option<Plan>,
    fix: Option<([f64; 3], Instant)>,
    /// The ship's speed, m/s, from two positions.
    pub speed: f64,
}

impl Mem {
    /// A refusal (`ModuleIsBlocked`, `MicroJumpDriveSpoolupPhase`) puts the
    /// drive off for [`BLOCKED_BACKOFF`].
    pub fn blocked(&mut self, now: Instant) {
        self.ready_at = Some(now + BLOCKED_BACKOFF);
        self.plan = None;
    }

    pub fn spooling(&self, now: Instant) -> bool {
        self.spool_until.is_some_and(|t| now < t)
    }
}

/// Whether a refusal text is one the drive makes of a ship it will not move.
pub fn is_drive_refusal(why: &str) -> bool {
    why.contains("ModuleIsBlocked")
        || why.contains(eve_server::mjd::SPOOLING)
        || why.contains("CantActivateMJDBecauseOfZeroVelocity")
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// **The speed sense** (every turn): the ship's speed from its last two
/// positions, kept at least half a second apart.
pub fn observe(m: &mut Memory, o: &Observation) {
    // No drive, no need to know how fast the ship goes.
    let (Some(park), Some(_)) = (o.park.as_ref(), o.kit.mjd) else {
        return;
    };
    if !matches!(o.place, Place::InSpace { .. }) {
        m.kit.fix = None;
        m.kit.speed = 0.0;
        return;
    }
    let here = park.ego.position;
    match m.kit.fix {
        Some((p, at)) => {
            let dt = o.now.saturating_duration_since(at).as_secs_f64();
            if dt >= 0.5 {
                m.kit.speed = dist(p, here) / dt;
                m.kit.fix = Some((here, o.now));
            }
        }
        None => m.kit.fix = Some((here, o.now)),
    }
}

// ---------------------------------------------------------------------------
// The smartbomb
// ---------------------------------------------------------------------------

/// **Rule `bomb_now`** (pure): the bombs to fire this turn. `lawful`: the
/// fight is on hunting ground or a war (the caller knows). Empty when any
/// condition fails.
pub fn bomb_now(o: &Observation, lawful: bool) -> Vec<Act> {
    let Some(park) = o.park.as_ref() else {
        return Vec::new();
    };
    let idle: Vec<&KitModule> = o
        .kit
        .bombs
        .iter()
        .filter(|b| b.online && !b.active && b.range > 0.0)
        .collect();
    if idle.is_empty() || !lawful || park.ego_warping {
        return Vec::new();
    }
    if o.cap.hold || o.cap.fraction.is_some_and(|f| f <= BOMB_CAP) {
        return Vec::new();
    }
    let reach = idle.iter().map(|b| b.range).fold(f64::MAX, f64::min) - BOMB_MARGIN;
    let widest = idle.iter().map(|b| b.range).fold(0.0, f64::max) + BOMB_MARGIN;
    let here = park.ego.position;
    let hostile_ships = o
        .hunt
        .pilots
        .iter()
        .filter(|p| p.hostile)
        .map(|p| p.ball)
        .chain(park.hostiles.iter().map(|h| h.ball))
        .filter(|b| b.id != park.ego.id && dist(here, b.position) <= reach)
        .count();
    if hostile_ships < BOMB_MIN {
        return Vec::new();
    }
    // Anyone who is not a foe and could be in the field holds the bomb.
    let foe_ids: Vec<i64> = o
        .hunt
        .pilots
        .iter()
        .filter(|p| p.hostile)
        .map(|p| p.ball.id)
        .collect();
    let others = o
        .hunt
        .pilots
        .iter()
        .filter(|p| !p.hostile)
        .map(|p| p.ball)
        .chain(
            o.humans
                .iter()
                .filter(|b| !foe_ids.contains(&b.id))
                .copied(),
        )
        .chain(park.boss);
    if others
        .filter(|b| b.id != park.ego.id)
        .any(|b| dist(here, b.position) <= widest)
    {
        return Vec::new();
    }
    idle.into_iter()
        .map(|b| Act::Kit(KitAct::Bomb(*b)))
        .collect()
}

// ---------------------------------------------------------------------------
// The micro jump drive
// ---------------------------------------------------------------------------

/// **The drive, if it may be tried now** (pure): fitted, idle, past its
/// cooldown, not in warp, not under a running scrambler, with the capacitor.
pub fn jump_ready(m: &Memory, o: &Observation) -> Option<KitModule> {
    let mjd = o.kit.mjd.filter(|d| !d.active)?;
    let park = o.park.as_ref()?;
    if !matches!(o.place, Place::InSpace { bound: true, .. }) || park.ego_warping {
        return None;
    }
    if o.scram.hard || m.kit.spooling(o.now) || m.kit.ready_at.is_some_and(|t| o.now < t) {
        return None;
    }
    if o.cap.hold || o.cap.fraction.is_some_and(|f| f <= MJD_CAP) {
        return None;
    }
    Some(mjd)
}

/// **The jump sequence** (pure but for the plan): align on `toward`, wait
/// until the ship moves, activate. `None` when the drive may not be tried.
pub fn jump_step(m: &mut Memory, o: &Observation, why: Why, toward: i64) -> Option<Vec<Act>> {
    let drive = jump_ready(m, o)?;
    let now = o.now;
    let plan = m
        .kit
        .plan
        .clone()
        .filter(|p| p.ball == toward && now.saturating_duration_since(p.since) < PLAN_TTL);
    let Some(plan) = plan else {
        m.kit.plan = Some(Plan {
            ball: toward,
            since: now,
            why,
        });
        return Some(vec![Act::AlignTo(toward), Act::Wait(SHORT_WAIT)]);
    };
    if now.saturating_duration_since(plan.since) < ALIGN_WAIT || m.kit.speed < MOVING {
        return Some(vec![Act::Wait(SHORT_WAIT)]);
    }
    m.kit.plan = None;
    m.kit.spool_until = Some(now + drive.cycle);
    m.kit.ready_at = Some(now + drive.cycle + drive.reactivation);
    eprintln!(
        "[pvp_kit] {}: micro jump ({:?}) toward {toward}",
        m.name, plan.why
    );
    Some(vec![Act::Kit(KitAct::Jump(drive))])
}

/// **Use 1: out of a bubble.** `exit` is how far the ship is from the edge of
/// the sphere that holds it (`None`: a moving field, so the edge is
/// unknown). A walk that is shorter than the spool and a margin is walked.
pub fn bubble_exit(
    m: &mut Memory,
    o: &Observation,
    exit: Option<f64>,
    toward: i64,
) -> Option<Vec<Act>> {
    let drive = o.kit.mjd?;
    if let Some(d) = exit {
        let speed = if m.kit.speed >= MOVING {
            m.kit.speed
        } else {
            ASSUMED_SPEED
        };
        if d / speed <= (drive.cycle + JUMP_MARGIN).as_secs_f64() {
            return None;
        }
    }
    jump_step(m, o, Why::Bubble, toward)
}

/// **Use 3: disengage.** Going home and pinned, and only by disruptors: the
/// drive is not blocked by them. Jump toward the home ball.
pub fn disengage(m: &mut Memory, o: &Observation, home: i64) -> Option<Vec<Act>> {
    if m.homing.is_none() || !o.scram.scrambled || o.scram.hard {
        return None;
    }
    jump_step(m, o, Why::Home, home)
}

/// **Use 2: the kite reset.** A hull that fights from reach whose prey has
/// closed inside [`RESET_SHARE`] of it, with armour to spare, jumps away
/// along the ball that points most directly from the prey. `reach` is the
/// fighting range ([`crate::escort::fight_range`]).
pub fn kite_reset(m: &mut Memory, o: &Observation, prey: &Ball, reach: f64) -> Option<Vec<Act>> {
    let park = o.park.as_ref()?;
    if reach < KITE_REACH || o.health.is_some_and(|h| h.armour <= ARMOUR_FLOOR) {
        return None;
    }
    if park.ego.surface_to(prey) >= RESET_SHARE * reach {
        return None;
    }
    let ball = away_ball(o, prey)?;
    jump_step(m, o, Why::Kite, ball)
}

/// The known ball that points most directly away from `prey`, if one points
/// away well enough ([`AWAY`]).
fn away_ball(o: &Observation, prey: &Ball) -> Option<i64> {
    let park = o.park.as_ref()?;
    let here = park.ego.position;
    let away = unit(sub(here, prey.position))?;
    park.belts
        .iter()
        .chain(park.station.iter())
        .chain(park.boss.iter())
        .filter(|b| b.id != prey.id && b.id != park.ego.id)
        .filter_map(|b| unit(sub(b.position, here)).map(|d| (dot(d, away), b.id)))
        .filter(|(c, _)| *c >= AWAY)
        .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, id)| id)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let l = dot(v, v).sqrt();
    (l > 1.0).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

/// **No move while the drive spools** (pure): the server refuses a warp, a
/// gate and a dock with `MicroJumpDriveSpoolupPhase`, so a turn that would
/// move waits instead.
pub fn spool_gate(m: &Memory, o: &Observation, acts: Vec<Act>) -> Vec<Act> {
    if !m.kit.spooling(o.now) {
        return acts;
    }
    let moves = acts.iter().any(|a| {
        matches!(
            a,
            Act::WarpToItem { .. }
                | Act::WarpToMember { .. }
                | Act::Dock(_)
                | Act::JumpGate { .. }
                | Act::TravelTo { .. }
                | Act::Gang(crate::roam_gang::GangAct::Fly { .. })
                | Act::Hunt(crate::hunt::HuntAct::Roam { .. })
        )
    });
    if moves {
        vec![Act::Wait(SHORT_WAIT)]
    } else {
        acts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Hostile, ParkView, Policy};
    use crate::hunt::Pilot;
    use eve_server::group_contacts::Affiliation;

    const SHIP: i64 = 1_000_101;
    const STATION: i64 = 1_000_301;
    const GATE: i64 = 1_000_501;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    fn module(item: i64, effect_id: i64, range: f64) -> KitModule {
        KitModule {
            item_id: item,
            type_id: 1_000_900 + item,
            effect: ("someEffect", effect_id),
            active: false,
            online: true,
            range,
            cycle: Duration::from_secs(10),
            reactivation: Duration::from_secs(100),
        }
    }

    fn bomb() -> KitModule {
        module(1, eve_server::smartbomb::EFFECT, 4_000.0)
    }

    fn drive() -> KitModule {
        module(2, eve_server::mjd::EFFECT, 0.0)
    }

    fn pilot(id: i64, x: f64, hostile: bool) -> Pilot {
        Pilot {
            ball: ball(id, x),
            char_id: id + 5,
            type_id: None,
            who: Affiliation {
                character_id: id + 5,
                corporation_id: 1_000_700,
                alliance_id: None,
            },
            hostile,
            relation: 0.0,
            war: false,
        }
    }

    fn npc(id: i64, x: f64) -> Hostile {
        Hostile {
            ball: ball(id, x),
            type_id: 0,
            name: "rat".to_owned(),
            shooting: None,
        }
    }

    fn obs(now: Instant) -> Observation {
        let mut o = Observation::empty(now);
        o.place = Place::InSpace {
            system: 1_000_001,
            bound: true,
        };
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: Some(ball(STATION, 5_000_000.0)),
            boss: None,
            taken: Default::default(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o.kit.bombs = vec![bomb()];
        o.kit.mjd = Some(drive());
        o
    }

    fn mem() -> Memory {
        Memory::new("Hunter1", Policy::fast())
    }

    fn with_foes(o: &mut Observation, n: usize) {
        for i in 0..n {
            o.hunt
                .pilots
                .push(pilot(1_002_000 + i as i64, 1_000.0, true));
        }
    }

    #[test]
    fn a_bomb_needs_two_foes() {
        let mut o = obs(Instant::now());
        with_foes(&mut o, 1);
        assert!(bomb_now(&o, true).is_empty());
        // An NPC in the field is a foe too.
        o.park
            .as_mut()
            .unwrap()
            .hostiles
            .push(npc(1_003_000, 900.0));
        assert_eq!(bomb_now(&o, true), vec![Act::Kit(KitAct::Bomb(bomb()))]);
        // Two foes at the edge of the field are not inside it.
        let mut far = obs(Instant::now());
        for i in 0..2 {
            far.hunt
                .pilots
                .push(pilot(1_002_000 + i, bomb().range - 10.0, true));
        }
        assert!(bomb_now(&far, true).is_empty());
    }

    #[test]
    fn a_bomb_holds_for_a_friend_in_range() {
        let mut o = obs(Instant::now());
        with_foes(&mut o, 3);
        assert!(!bomb_now(&o, true).is_empty());
        o.hunt.pilots.push(pilot(1_004_000, 2_000.0, false));
        assert!(bomb_now(&o, true).is_empty());
        // Far enough away, he is no reason to hold.
        o.hunt.pilots.last_mut().unwrap().ball.position[0] = 90_000.0;
        assert!(!bomb_now(&o, true).is_empty());
        // A human the hunt does not classify holds it too.
        o.humans.push(ball(1_005_000, 1_500.0));
        assert!(bomb_now(&o, true).is_empty());
    }

    #[test]
    fn a_bomb_waits_for_war_warp_and_capacitor() {
        let mut o = obs(Instant::now());
        with_foes(&mut o, 2);
        assert!(bomb_now(&o, false).is_empty());
        o.park.as_mut().unwrap().ego_warping = true;
        assert!(bomb_now(&o, true).is_empty());
        o.park.as_mut().unwrap().ego_warping = false;
        o.cap.fraction = Some(BOMB_CAP / 2.0);
        assert!(bomb_now(&o, true).is_empty());
        o.cap.fraction = Some(1.0);
        o.kit.bombs[0].active = true;
        assert!(bomb_now(&o, true).is_empty());
        // An offline bomb (over the grid) is never sent.
        o.kit.bombs[0].active = false;
        o.kit.bombs[0].online = false;
        assert!(bomb_now(&o, true).is_empty());
        o.kit.bombs[0].online = true;
        assert!(!bomb_now(&o, true).is_empty());
    }

    /// Run `jump_step` the way turns go: align, wait, then the jump.
    fn run_to_jump(m: &mut Memory, o: &mut Observation) -> Vec<Act> {
        let first = jump_step(m, o, Why::Home, STATION).unwrap();
        assert_eq!(first[0], Act::AlignTo(STATION));
        o.now += ALIGN_WAIT + Duration::from_secs(1);
        m.kit.speed = MOVING * 4.0;
        jump_step(m, o, Why::Home, STATION).unwrap()
    }

    #[test]
    fn a_jump_aligns_waits_for_motion_then_goes() {
        let mut m = mem();
        let mut o = obs(Instant::now());
        let first = jump_step(&mut m, &o, Why::Bubble, STATION).unwrap();
        assert_eq!(first[0], Act::AlignTo(STATION));
        // Too soon, and too slow.
        o.now += Duration::from_secs(1);
        let wait = jump_step(&mut m, &o, Why::Bubble, STATION).unwrap();
        assert!(matches!(wait[0], Act::Wait(_)));
        o.now += ALIGN_WAIT;
        assert!(matches!(
            jump_step(&mut m, &o, Why::Bubble, STATION).unwrap()[0],
            Act::Wait(_)
        ));
        m.kit.speed = MOVING * 2.0;
        assert_eq!(
            jump_step(&mut m, &o, Why::Bubble, STATION).unwrap(),
            vec![Act::Kit(KitAct::Jump(drive()))]
        );
        // The cooldown is on, and so is the spool.
        assert!(m.kit.spooling(o.now));
        assert!(jump_ready(&m, &o).is_none());
    }

    #[test]
    fn a_jump_is_not_tried_while_scrambled() {
        let mut m = mem();
        let mut o = obs(Instant::now());
        o.scram.scrambled = true;
        o.scram.hard = true;
        m.homing = Some(crate::brain::Homing::Repair);
        assert!(jump_step(&mut m, &o, Why::Home, STATION).is_none());
        assert!(disengage(&mut m, &o, STATION).is_none());
        // Pinned by a disruptor alone, the drive works.
        o.scram.hard = false;
        assert!(disengage(&mut m, &o, STATION).is_some());
        // And a ship that is not going home does not disengage.
        let mut stay = mem();
        assert!(disengage(&mut stay, &o, STATION).is_none());
    }

    #[test]
    fn no_warp_during_the_spool() {
        let mut m = mem();
        let mut o = obs(Instant::now());
        let go = run_to_jump(&mut m, &mut o);
        assert!(matches!(go[0], Act::Kit(KitAct::Jump(_))));
        let warp = vec![Act::WarpToItem {
            id: STATION,
            range: 0.0,
        }];
        assert!(matches!(spool_gate(&m, &o, warp.clone())[0], Act::Wait(_)));
        let gate = vec![Act::JumpGate { gate: GATE }];
        assert!(matches!(spool_gate(&m, &o, gate)[0], Act::Wait(_)));
        // A shot is not a move; and once the spool is over the warp goes.
        let shot = vec![Act::CeaseFire(1)];
        assert_eq!(spool_gate(&m, &o, shot.clone()), shot);
        o.now += drive().cycle + Duration::from_secs(1);
        assert_eq!(spool_gate(&m, &o, warp.clone()), warp);
    }

    #[test]
    fn the_cooldown_is_read_from_the_module() {
        let effect = eve_server::modules::Effect {
            effect_id: eve_server::mjd::EFFECT,
            name: "microJumpDrive",
            category: 1,
            guid: "",
            is_offensive: false,
            duration_attribute: 73,
            warp_safe: None,
        };
        let row = |a: i64| match a {
            73 => Some(7_000.0),
            ATTR_REACTIVATION => Some(41_000.0),
            _ => None,
        };
        let read = KitModule::read(5, 6, &effect, false, true, row);
        assert_eq!(read.cycle, Duration::from_secs(7));
        assert_eq!(read.reactivation, Duration::from_secs(41));
        // A row that says nothing falls back to the defaults.
        let blank = KitModule::read(5, 6, &effect, false, true, |_| None);
        assert_eq!(blank.cycle, DEFAULT_SPOOL);
        // The jump holds the drive off by what the module says.
        let mut m = mem();
        let mut o = obs(Instant::now());
        o.kit.mjd = Some(read);
        let t0 = o.now;
        run_to_jump(&mut m, &mut o);
        let at = m.kit.ready_at.unwrap();
        assert_eq!(
            at.saturating_duration_since(o.now),
            read.cycle + read.reactivation
        );
        assert!(at > t0);
    }

    #[test]
    fn the_stock_is_fitted_once_and_never_made() {
        let mut m = mem();
        let mut o = obs(Instant::now());
        o.kit = Kit::default();
        assert_eq!(stock_to_fit(&mut m, &o), None);
        o.hangar = vec![crate::ItemRow {
            item_id: 77,
            type_id: SMARTBOMB,
            owner_id: 1_000_002,
            location_id: 1_000_003,
            flag_id: crate::FLAG_HANGAR,
            quantity: 1,
            group_id: 0,
            category_id: 0,
            singleton: true,
        }];
        assert_eq!(stock_to_fit(&mut m, &o), Some(Act::FitAuto(77)));
        // Tried once.
        assert_eq!(stock_to_fit(&mut m, &o), None);
        // A kit already fitted is not fitted twice.
        let mut again = mem();
        o.kit.bombs = vec![bomb()];
        assert_eq!(stock_to_fit(&mut again, &o), None);
    }

    /// fix-w8b B6: fitting a launcher makes the next turn's `camp_dock` due,
    /// so the probes are loaded before the undock.
    #[test]
    fn fitting_a_launcher_clears_the_dock_timer() {
        let mut m = mem();
        let mut o = obs(Instant::now());
        o.kit = Kit::default();
        m.camp.dock_next = Some(o.now + Duration::from_secs(60));
        o.hangar = vec![crate::ItemRow {
            item_id: 88,
            type_id: 9_911,
            owner_id: 1_000_002,
            location_id: 1_000_003,
            flag_id: crate::FLAG_HANGAR,
            quantity: 1,
            group_id: crate::camp::GROUP_LAUNCHER,
            category_id: 0,
            singleton: true,
        }];
        assert_eq!(stock_to_fit(&mut m, &o), Some(Act::FitAuto(88)));
        assert_eq!(m.camp.dock_next, None);
        // A bomb leaves the timer alone.
        let mut m = mem();
        m.camp.dock_next = Some(o.now);
        o.hangar[0].group_id = 0;
        o.hangar[0].type_id = SMARTBOMB;
        o.hangar[0].item_id = 89;
        assert_eq!(stock_to_fit(&mut m, &o), Some(Act::FitAuto(89)));
        assert!(m.camp.dock_next.is_some());
    }

    #[test]
    fn a_drill_names_its_part_and_unknown_drills_name_none() {
        assert_eq!(drill_lines("smartbomb", false).len(), 2);
        assert!(drill_lines("smartbomb", false).contains(&format!("/createitem {DRILL_BOMB}")));
        // The foes' bomber gets a battleship; the allies stay on their Rifters.
        let foe = drill_lines("smartbomb", true);
        assert!(foe.contains(&format!("/createitem {DRILL_BOMB_HULL}")));
        assert!(
            !drill_lines("smartbomb", false).contains(&format!("/createitem {DRILL_BOMB_HULL}"))
        );
        assert_eq!(drill_hull("smartbomb", true), Some(DRILL_BOMB_HULL));
        assert_eq!(drill_hull("smartbomb", false), None);
        assert!(STOCK_TYPES.contains(&DRILL_BOMB));
        let mjd = drill_lines("mjd", false);
        assert_eq!(mjd.len(), 4);
        assert!(mjd.contains(&format!("/createitem {DRILL_HULL}")));
        assert_eq!(drill_hull("mjd", false), Some(DRILL_HULL));
        assert_eq!(drill_hull("mjd", true), Some(DRILL_HULL));
        assert!(drill_lines("", true).is_empty());
        assert!(drill_lines("anything else", true).is_empty());
    }

    #[test]
    fn a_refused_drive_is_put_off_not_failed() {
        assert!(is_drive_refusal("Activate(2, \"x\"): ModuleIsBlocked"));
        assert!(is_drive_refusal("Activate: MicroJumpDriveSpoolupPhase"));
        assert!(!is_drive_refusal("Activate: NotEnoughEnergy"));
        assert!(crate::brain::ActKind::Kit.is_incidental());
        let mut m = mem();
        let o = obs(Instant::now());
        m.kit.blocked(o.now);
        assert!(jump_ready(&m, &o).is_none());
    }

    #[test]
    fn a_short_walk_out_of_a_bubble_is_walked() {
        let mut m = mem();
        let o = obs(Instant::now());
        // 200 m at the assumed speed is a couple of seconds.
        assert!(bubble_exit(&mut m, &o, Some(200.0), STATION).is_none());
        // 15 km is not.
        assert!(bubble_exit(&mut m, &o, Some(15_000.0), STATION).is_some());
        // A field that moves has no edge to walk to.
        assert!(bubble_exit(&mut mem(), &o, None, STATION).is_some());
    }

    #[test]
    fn a_kite_reset_needs_reach_armour_and_a_ball_away_from_the_prey() {
        let mut m = mem();
        let mut o = obs(Instant::now());
        let prey = ball(1_002_000, 3_000.0);
        o.park.as_mut().unwrap().belts = vec![ball(1_000_401, -400_000.0)];
        o.park.as_mut().unwrap().station = None;
        let reach = KITE_REACH * 2.0;
        assert!(kite_reset(&mut m, &o, &prey, reach).is_some());
        // A short-reach hull does not kite.
        assert!(kite_reset(&mut mem(), &o, &prey, KITE_REACH / 4.0).is_none());
        // A prey still far off is not a reason.
        let far = ball(1_002_000, reach);
        assert!(kite_reset(&mut mem(), &o, &far, reach).is_none());
        // Armour below the floor leaves the retreat to the other rule.
        o.health = Some(crate::brain::Health {
            shield: 0.0,
            armour: ARMOUR_FLOOR / 2.0,
            hull: 1.0,
        });
        assert!(kite_reset(&mut mem(), &o, &prey, reach).is_none());
        // And no ball behind the bot, no jump.
        o.health = None;
        o.park.as_mut().unwrap().belts = vec![ball(1_000_401, 400_000.0)];
        assert!(kite_reset(&mut mem(), &o, &prey, reach).is_none());
    }
}
