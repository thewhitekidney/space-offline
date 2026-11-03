//! **The hostile bot** (W16 packet CP56, plan §4.7, ruling 5): a bot that
//! hunts the humans, opt-in per bot.
//!
//! **Opt-in, twice.** `eve-bots --loop hostile` is refused without
//! `--hostile` ([`check_opt_in`]), and only `--hostile` builds the
//! [`Rules`] that reach the brain in [`crate::brain::Policy::hostile`]. A
//! hostile-role bot without rules never undocks: it says `MISSING`
//! ([`MISSING_RULES`]). No other role reads the rules.
//!
//! **What it attacks.** A human outside its fleet (the driver's `is_human`:
//! a pilot, not the bot, not a bot of this process, not the fleet boss)
//! that is on its grid (GRID-B: a park holds its own grid's ships only),
//! when all of these hold ([`Rules::prey`]):
//!
//! 1. **the bot is in no fleet.** A bot knows one fleet member, the boss
//!    ([`crate::brain::FleetView::boss`]); so that it can never shoot any
//!    other member, a hostile in a fleet attacks no human at all. **[A]**,
//!    falsified by a friend who fleets with a hostile to have it fight
//!    beside him: that needs the fleet's member list, a driver packet.
//! 2. **the system is within [`HOSTILE_RANGE_JUMPS`] of the humans' home**
//!    (`--home` with `--content`'s gate graph; with no home, anywhere).
//! 3. **the act would be no crime** ([`Rules::flags_apply`]): CP3b's rule,
//!    `crimewatch::flags_apply`, is that suspect and criminal flags exist
//!    wherever the client's rounded security is above `0.0` [V]; the bot
//!    attacks nobody there, and nobody in a system it has no security for
//!    (no `--content`, or a system the bundle lacks), so it is never
//!    CONCORDed for anything a player could not do either. The server also
//!    calls a system zero-sec when a configured character starts in it; the
//!    bot does not know that list, which only makes it stricter.
//! 4. **the policy table allows it** ([`Table::engages`]):
//!    `content/bots/hostile.tsv`, the first row matching the target's
//!    [`Class`] and the system's rounded security.
//!
//! The last line of defence is [`guard`]: whatever the fight produced, no
//! lock, orbit or shot at a human or at the fleet boss survives unless that
//! ship is this turn's lawful prey.
//!
//! **How it fights and hunts.** It flies the escort's Rifter and fights the
//! escort's way ([`crate::escort::in_space`]: orbit at the guns' range, lock,
//! fire, home to repair or re-arm), shown a park whose only hostile is the
//! prey. With no prey it is an escort without a boss -- it shoots the rats
//! that come, at the nearest belt -- and after [`HUNT_WAITS`] quiet patrols
//! it warps on to the next belt ([`next_belt`]): the belts are global
//! (GRID-B), the ships on them are not. A d-scan would find ships farther
//! out; there is no d-scan verb yet. No act routes through gates, so the
//! hunt is the bot's own system.
//!
//! **Every rule here is [A] policy**, except the client's own crimewatch
//! rule and the rig size and capsule group, which cite their source.

use crate::brain::{self, Act, Ball, Hostile, Memory, Observation, Place, RoleRef};
use crate::escort;
use crate::roles::{Lacks, RoleSpec, Summary};
use eve_server::authored;
use eve_server::celestial::{SecurityClass, Universe, pseudo_security};
use eve_server::typedata;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// How far from the humans' home system a hostile bot hunts, in jumps
/// (plan §4.7). **[A]**: falsified by humans who never meet one (too
/// small; with no gate routing yet it is the home draw's check) or who
/// meet one on every undock (too large).
pub const HOSTILE_RANGE_JUMPS: u32 = 2;

/// Quiet patrols at one belt before hunting at the next. **[A]**:
/// falsified by a hostile that never finds a human ratting two belts away
/// (too many) or one that never stays to fight (too few).
pub const HUNT_WAITS: u32 = 6;

/// `rigSize` **[V-data]** (`dgmattribs` 1547; on a Rifter `1`, on a
/// titan `4`): the client's own hull size, `1` small to `4` capital.
pub const ATTRIBUTE_RIG_SIZE: i64 = 1547;

/// `const.groupCapsule` **[V-data]**: a pod.
pub const GROUP_CAPSULE: i64 = 29;

/// The policy table's path under the authored directory.
pub const REL: &str = "bots/hostile.tsv";

/// Its header, copied from `docs/content/schema.md` §4.
pub const HEADER: &str = "ship_class\tmin_sec\tmax_sec\tengage\twhy";

/// The shipped policy, compiled in; `./content/bots/hostile.tsv` wins.
const SHIPPED: &str = include_str!("../../../../content/bots/hostile.tsv");

/// The `MISSING` line of a hostile-role bot started without `--hostile`.
pub const MISSING_RULES: &str = "eve-bots --hostile (the hostile bot is opt-in)";

/// What its chat calls a target. **[A]**
pub const PREY_NAME: &str = "a capsuleer";

/// Hunts humans: see the module header. Not a civilian (ruling 5).
pub static ROLE: RoleSpec = RoleSpec {
    name: "hostile",
    greeting: "o7 -- I'm the opposition today",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

/// **The opt-in** (plan §4.7): `Err` when a bot plays [`ROLE`] and
/// `--hostile` was not given.
pub fn check_opt_in(
    roles: impl IntoIterator<Item = RoleRef>,
    opted_in: bool,
) -> Result<(), String> {
    if !opted_in && roles.into_iter().any(|r| *r == ROLE) {
        return Err(format!(
            "--loop {}: a bot that attacks humans is opt-in; add --hostile",
            ROLE.name
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The policy table
// ---------------------------------------------------------------------------

/// A target hull's class: a pod, or the client's rig size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Capsule,
    Small,
    Medium,
    Large,
    Capital,
    /// A hull the bot cannot class: only `*` rows match it.
    Unknown,
}

impl Class {
    /// A table word; `*` is not a class but "any" ([`Rule::class`]).
    pub fn parse(word: &str) -> Option<Class> {
        Some(match word {
            "capsule" => Class::Capsule,
            "small" => Class::Small,
            "medium" => Class::Medium,
            "large" => Class::Large,
            "capital" => Class::Capital,
            _ => return None,
        })
    }

    /// A ship type's class from its typedata row: its group, then its
    /// [`ATTRIBUTE_RIG_SIZE`]; `None` for a type that is not a ship.
    pub fn of_type(row: &typedata::TypeRow) -> Option<Class> {
        if row.category_id != crate::courier::CATEGORY_SHIP {
            return None;
        }
        if row.group_id == GROUP_CAPSULE {
            return Some(Class::Capsule);
        }
        let size = row.attribute(ATTRIBUTE_RIG_SIZE).unwrap_or(0.0);
        Some(match size.round() as i64 {
            1 => Class::Small,
            2 => Class::Medium,
            3 => Class::Large,
            4 => Class::Capital,
            _ => Class::Unknown,
        })
    }
}

/// One `hostile.tsv` row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    /// `None`: `*`, any class.
    pub class: Option<Class>,
    pub min_sec: f64,
    pub max_sec: f64,
    pub engage: bool,
}

impl Rule {
    fn matches(&self, class: Class, security: f64) -> bool {
        self.class.is_none_or(|c| c == class)
            && self.min_sec <= security
            && security <= self.max_sec
    }
}

/// The table, rows in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub rules: Vec<Rule>,
}

impl Table {
    /// The rows of an authored file's text; a header other than
    /// [`HEADER`] is an error, a row that does not parse is skipped and
    /// logged (`content/README.md`).
    pub fn parse(text: &str) -> Result<Table, String> {
        let (rows, _) = authored::parse(text, HEADER)?;
        Ok(Table::from_rows(&rows, REL))
    }

    /// `./content/bots/hostile.tsv` when there, else the shipped copy.
    pub fn load(dir: &authored::Dir) -> Result<Table, String> {
        let file = dir.load(REL, SHIPPED, HEADER)?;
        Ok(Table::from_rows(&file.rows, REL))
    }

    fn from_rows(rows: &[authored::Row], what: &str) -> Table {
        let mut rules = Vec::new();
        for row in rows {
            match Table::row(row) {
                Ok(r) => rules.push(r),
                Err(e) => println!("[authored] {what}: line {}: skipped: {e}", row.line),
            }
        }
        Table { rules }
    }

    fn row(r: &authored::Row) -> Result<Rule, String> {
        let class = match r.field(0) {
            "*" => None,
            w => Some(Class::parse(w).ok_or_else(|| format!("ship_class {w:?}"))?),
        };
        let (min_sec, max_sec) = (r.f64(1)?, r.f64(2)?);
        if min_sec > max_sec {
            return Err(format!("min_sec {min_sec} above max_sec {max_sec}"));
        }
        Ok(Rule {
            class,
            min_sec,
            max_sec,
            engage: r.bool(3)?,
        })
    }

    /// **The policy's verdict**: the first row matching `class` at
    /// `security` (rounded the client's way by the caller) decides; no row
    /// matching is "leave alone".
    pub fn engages(&self, class: Class, security: f64) -> bool {
        self.rules
            .iter()
            .find(|r| r.matches(class, security))
            .is_some_and(|r| r.engage)
    }
}

// ---------------------------------------------------------------------------
// The rules a hostile bot carries
// ---------------------------------------------------------------------------

/// Everything the hostile bot decides by, built once at startup and handed
/// to every bot behind an `Arc`, like [`crate::profile::Profiles`].
#[derive(Clone, Default, PartialEq)]
pub struct Rules {
    pub table: Table,
    /// System -> security as the bundle ships it (unrounded).
    security: BTreeMap<i64, f64>,
    /// Ship type -> class.
    classes: BTreeMap<i64, Class>,
    /// The systems it may hunt in; `None`: anywhere.
    range: Option<BTreeSet<i64>>,
}

/// Counts, never rows: a policy is printed whole in a few logs.
impl std::fmt::Debug for Rules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Rules({} rows, {} systems, {} hulls, range {})",
            self.table.rules.len(),
            self.security.len(),
            self.classes.len(),
            self.range
                .as_ref()
                .map_or("anywhere".to_owned(), |r| format!("{} systems", r.len()))
        )
    }
}

impl Rules {
    /// The table alone: no system's security known, so nobody is prey yet.
    pub fn new(table: Table) -> Rules {
        Rules {
            table,
            ..Rules::default()
        }
    }

    /// One system's security, unrounded.
    pub fn with_security(mut self, system: i64, security: f64) -> Rules {
        self.security.insert(system, security);
        self
    }

    /// Every system of a universe table (CP3b).
    pub fn with_universe(mut self, u: &Universe) -> Rules {
        self.security
            .extend(u.systems().map(|s| (s.id, s.security)));
        self
    }

    /// One ship type's class.
    pub fn with_class(mut self, type_id: i64, class: Class) -> Rules {
        self.classes.insert(type_id, class);
        self
    }

    /// Every ship type of a typedata table.
    pub fn with_types(mut self, td: &typedata::Table) -> Rules {
        self.classes.extend(
            td.types()
                .filter_map(|t| Class::of_type(t).map(|c| (t.type_id, c))),
        );
        self
    }

    /// The systems it may hunt in.
    pub fn with_range(mut self, systems: impl IntoIterator<Item = i64>) -> Rules {
        self.range = Some(systems.into_iter().collect());
        self
    }

    /// `eve-bots --hostile`: the table from `./content` (or compiled in);
    /// with `--content DIR`, the bundle's systems, hull classes and -- with
    /// `--home` -- the systems within [`HOSTILE_RANGE_JUMPS`] of it by the
    /// gate graph. Errors name the file.
    pub fn open(content: Option<&Path>, home: Option<i64>) -> Result<Rules, String> {
        let mut rules = Rules::new(Table::load(&authored::Dir::default())?);
        let Some(dir) = content else {
            return Ok(rules);
        };
        let (bundle, _) = eve_server::content::Bundle::open(dir)?;
        let at = |e: String| format!("--content {}: {e}", dir.display());
        rules = rules.with_universe(&Universe::from_bundle(&bundle).map_err(at)?);
        if let Some(p) = bundle.path(eve_server::content::TYPEDATA) {
            rules = rules.with_types(&typedata::Table::load(&p).map_err(at)?);
        }
        if let (Some(home), Some(p)) = (home, bundle.path(eve_server::content::CELESTIALS)) {
            let graph = crate::driver::gate_graph(&eve_server::load_celestials(&p)?);
            rules = rules.with_range(crate::driver::within(&graph, home, HOSTILE_RANGE_JUMPS));
        }
        Ok(rules)
    }

    /// A hull's class; a type it does not know is [`Class::Unknown`].
    pub fn class_of(&self, type_id: Option<i64>) -> Class {
        type_id
            .and_then(|t| self.classes.get(&t).copied())
            .unwrap_or(Class::Unknown)
    }

    /// The system's security as the client rounds it, if known.
    pub fn security(&self, system: i64) -> Option<f64> {
        self.security.get(&system).map(|&s| pseudo_security(s))
    }

    /// **Would attacking a capsuleer in `system` be a crime?** CP3b's
    /// `crimewatch::flags_apply` rule over the bot's own table: flags apply
    /// where the client's class is low sec or higher [V]; and, stricter
    /// than the server, in any system the bot has no security for.
    pub fn flags_apply(&self, system: i64) -> bool {
        self.security(system)
            .is_none_or(|s| SecurityClass::from_level(s).is_low_sec_or_higher())
    }

    /// Inside the hunting range.
    pub fn in_range(&self, system: i64) -> bool {
        self.range.as_ref().is_none_or(|r| r.contains(&system))
    }

    /// **This turn's prey** (module header, rules 1-4): the human already
    /// engaged while still allowed, else the nearest allowed one.
    pub fn prey(&self, m: &Memory, o: &Observation) -> Option<(Ball, Option<i64>)> {
        if o.fleet.is_some() || m.homing.is_some() {
            return None;
        }
        let Place::InSpace { system, .. } = o.place else {
            return None;
        };
        let park = o.park.as_ref()?;
        if !self.in_range(system) || self.flags_apply(system) {
            return None;
        }
        let security = self.security(system)?;
        let allowed: Vec<(Ball, Option<i64>)> = o
            .humans
            .iter()
            .map(|h| (*h, o.human_types.get(&h.id).copied()))
            .filter(|(_, t)| self.table.engages(self.class_of(*t), security))
            .collect();
        let engaged = m.target.as_ref().map(|(id, _)| *id);
        if let Some(p) = allowed.iter().find(|(b, _)| Some(b.id) == engaged) {
            return Some(*p);
        }
        allowed.into_iter().min_by(|a, b| {
            park.ego
                .surface_to(&a.0)
                .total_cmp(&park.ego.surface_to(&b.0))
                .then(a.0.id.cmp(&b.0.id))
        })
    }
}

// ---------------------------------------------------------------------------
// The role
// ---------------------------------------------------------------------------

/// Docked: the escort's preparation (a Rifter, guns, rounds), once opted
/// in; without rules, `MISSING` and stay docked.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if m.policy.hostile.is_none() {
        return vec![Act::Missing(MISSING_RULES), Act::Wait(brain::MISSING_WAIT)];
    }
    m.belt_waits = 0;
    escort::docked(m, o)
}

/// In space: the module header's hunt.
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(park) = &o.park else {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    };
    let prey = m.policy.hostile.clone().and_then(|rules| rules.prey(m, o));
    let acts = match prey {
        Some((ball, type_id)) => {
            let mut said = Vec::new();
            if m.target.as_ref().map(|(id, _)| *id) != Some(ball.id) {
                m.target = Some((ball.id, PREY_NAME.to_owned()));
                m.belt_waits = 0;
                if o.chat {
                    said.push(Act::Say(format!("Engaging {PREY_NAME}")));
                }
            }
            // The escort's fight, shown the prey as the only hostile.
            let mut seen = o.clone();
            if let Some(p) = &mut seen.park {
                p.hostiles = vec![Hostile {
                    ball,
                    type_id: type_id.unwrap_or(0),
                    name: PREY_NAME.to_owned(),
                    shooting: None,
                }];
            }
            [said, escort::in_space(m, &seen)].concat()
        }
        None => {
            // A human let go -- gone, out of reach of the rules, or the bot
            // heading home -- is no kill and no "down" line.
            if let Some((id, _)) = &m.target
                && !park.hostiles.iter().any(|h| h.ball.id == *id)
            {
                m.target = None;
            }
            let acts = escort::in_space(m, o);
            if acts == [Act::Wait(escort::PATROL_WAIT)] && o.fleet.is_none() {
                hunt(m, park).unwrap_or(acts)
            } else {
                acts
            }
        }
    };
    guard(acts, o, prey.map(|(b, _)| b.id))
}

/// A quiet patrol: wait [`HUNT_WAITS`] of them, then warp to the
/// [`next_belt`].
fn hunt(m: &mut Memory, park: &brain::ParkView) -> Option<Vec<Act>> {
    if m.belt_waits < HUNT_WAITS {
        m.belt_waits += 1;
        return None;
    }
    let next = next_belt(park)?;
    m.belt_waits = 0;
    m.belt = Some(next.id);
    Some(vec![escort::warp_or_approach(
        &park.ego,
        &next,
        escort::WARP_STANDOFF,
    )])
}

/// **The belt after the nearest one**, by id, wrapping: the bot walks
/// every belt of the system in turn. `None` with fewer than two belts.
pub fn next_belt(park: &brain::ParkView) -> Option<Ball> {
    if park.belts.len() < 2 {
        return None;
    }
    let here = park
        .belts
        .iter()
        .min_by(|a, b| {
            park.ego
                .surface_to(a)
                .total_cmp(&park.ego.surface_to(b))
                .then(a.id.cmp(&b.id))
        })?
        .id;
    let mut ids: Vec<&Ball> = park.belts.iter().collect();
    ids.sort_by_key(|b| b.id);
    let at = ids.iter().position(|b| b.id == here)?;
    Some(*ids[(at + 1) % ids.len()])
}

/// **The last line of defence** (module header): drop every lock, orbit
/// and shot whose target is a human or the fleet boss, unless it is
/// `prey`; a turn left with nothing to do waits.
pub fn guard(acts: Vec<Act>, o: &Observation, prey: Option<i64>) -> Vec<Act> {
    let protected: BTreeSet<i64> = o
        .humans
        .iter()
        .map(|b| b.id)
        .chain(o.park.as_ref().and_then(|p| p.boss).map(|b| b.id))
        .filter(|id| Some(*id) != prey)
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

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, kills {}, reconnects {}",
        s.name, s.phase, s.kills, s.reconnects
    )
}

/// The escort's kit: the gunnery skill, a Rifter, its guns and rounds.
fn setup(lacks: Lacks) -> Vec<String> {
    let mut lines = vec![format!("/giveskill {} 1", escort::SKILL_GUNNERY)];
    if lacks.ship {
        lines.push(format!("/createitem {}", escort::RIFTER));
    }
    if lacks.module {
        lines.push(format!("/createitem {} {}", escort::GUN, escort::WANT_GUNS));
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

#[cfg(test)]
mod tests {
    //! The hostile's rules over invented fixtures: a fixture policy table,
    //! CP3b's fixture universe, invented ids. Asserts which ship is
    //! engaged and which acts name it, never a range or a delay.
    use super::*;
    use crate::ItemRow;
    use crate::brain::{ActKind, FleetView, GunInfo, ParkView, Policy, ShipInfo, decide};
    use crate::roles;
    use std::sync::Arc;
    use std::time::Instant;

    const STATION: i64 = 61_000_001;
    const SHIP: i64 = 100_000_101;
    const GUN_A: i64 = 100_000_102;
    const HUMAN: i64 = 100_000_201;
    const OTHER: i64 = 100_000_202;
    const BOSS_SHIP: i64 = 100_000_209;
    const BOSS: i64 = 90_000_209;
    /// Invented hull types and systems.
    const SMALL_HULL: i64 = 990_001;
    const MEDIUM_HULL: i64 = 990_002;
    const NULL: i64 = 39_100_001;
    const DEEP_NULL: i64 = 39_100_002;

    /// An invented policy: small hulls in deep null and in shallow null
    /// are decided by different rows; medium hulls by two rows, the first
    /// of which says no; anything else, no.
    fn fixture_table() -> Table {
        Table::parse(
            "# invented\n\
             ship_class\tmin_sec\tmax_sec\tengage\twhy\n\
             small\t-1.0\t-0.5\t1\t[A] invented\n\
             small\t-0.5\t0.0\t0\t[A] invented\n\
             medium\t-1.0\t0.0\t0\t[A] invented\n\
             medium\t-1.0\t0.0\t1\t[A] invented\n\
             *\t-1.0\t1.0\t0\t[A] invented\n",
        )
        .expect("the fixture parses")
    }

    fn rules(table: Table) -> Rules {
        Rules::new(table)
            .with_security(NULL, -0.2)
            .with_security(DEEP_NULL, -0.8)
            .with_class(SMALL_HULL, Class::Small)
            .with_class(MEDIUM_HULL, Class::Medium)
    }

    fn hostile(rules: Option<Rules>) -> Memory {
        Memory::new(
            "Hostile1",
            Policy {
                role: &ROLE,
                hostile: rules.map(Arc::new),
                ..Policy::fast()
            },
        )
    }

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    /// In space in `system`, armed and loaded, with `human` in a `hull`
    /// beside it, already orbited and locked: an engaged turn is a shot.
    fn beside(system: i64, human: i64, hull: Option<i64>) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::InSpace {
            system,
            bound: true,
        };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(escort::RIFTER),
        });
        o.guns = vec![GunInfo {
            item_id: GUN_A,
            type_id: escort::GUN,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(200),
            capacity: 200,
            range: 3_000.0,
            reload: std::time::Duration::ZERO,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o.cargo = vec![ItemRow {
            item_id: 1,
            type_id: escort::AMMO,
            owner_id: 0,
            location_id: SHIP,
            flag_id: crate::FLAG_CARGO,
            quantity: escort::SPARE_ROUNDS,
            group_id: 0,
            category_id: crate::CATEGORY_CHARGE,
            singleton: false,
        }];
        o.home = Some(STATION);
        o.lock_range = 20_000.0;
        o.locked = vec![human];
        o.humans = vec![ball(human, 1_000.0)];
        if let Some(t) = hull {
            o.human_types.insert(human, t);
        }
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: Some(ball(STATION, -1_000.0)),
            boss: None,
            taken: BTreeMap::new(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: Some(human),
        });
        o
    }

    /// Does any act lock, orbit or shoot `ship`?
    fn engages(acts: &[Act], ship: i64) -> bool {
        acts.iter().any(|a| {
            matches!(a, Act::Fire { target, .. } | Act::Lock(target) | Act::Orbit { id: target, .. }
                if *target == ship)
        })
    }

    fn fires_on(acts: &[Act], ship: i64) -> bool {
        acts.iter()
            .any(|a| matches!(a, Act::Fire { target, .. } if *target == ship))
    }

    /// **Opt-in**: without `--hostile` the role cannot be chosen, with it
    /// it can, and no other role needs it. A hostile-role bot that got past
    /// anyway (no rules in its policy) stays docked saying `MISSING`, and
    /// in space engages nobody.
    #[test]
    fn without_the_opt_in_the_role_cannot_be_chosen() {
        assert!(check_opt_in([&ROLE as RoleRef], false).is_err());
        assert!(check_opt_in([roles::default(), &ROLE], false).is_err());
        assert!(check_opt_in([&ROLE as RoleRef], true).is_ok());
        assert!(check_opt_in(roles::ROLES.iter().copied().filter(|r| **r != ROLE), false).is_ok());
        assert!(check_opt_in(std::iter::empty(), false).is_ok());

        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: STATION };
        let (_, acts) = decide(&hostile(None), &o);
        assert_eq!(acts[0], Act::Missing(MISSING_RULES));

        let o = beside(DEEP_NULL, HUMAN, Some(SMALL_HULL));
        let (_, acts) = decide(&hostile(None), &o);
        assert!(!engages(&acts, HUMAN), "{acts:?}");
        // The same turn with rules is a shot: the fixture is not inert.
        let (_, acts) = decide(&hostile(Some(rules(fixture_table()))), &o);
        assert!(fires_on(&acts, HUMAN), "{acts:?}");
    }

    /// **The table decides** (rule over a fixture table): the first row
    /// matching the hull's class and the system's security; a class the bot
    /// does not know matches only `*`.
    #[test]
    fn it_engages_only_what_the_policy_table_allows() {
        let r = rules(fixture_table());
        // (system, hull, the fixture's first matching row says engage)
        let cases = [
            (DEEP_NULL, Some(SMALL_HULL), true),
            (NULL, Some(SMALL_HULL), false),
            (DEEP_NULL, Some(MEDIUM_HULL), false),
            (NULL, Some(MEDIUM_HULL), false),
            (DEEP_NULL, None, false),
            (DEEP_NULL, Some(990_999), false),
        ];
        for (system, hull, want) in cases {
            let o = beside(system, HUMAN, hull);
            let (_, acts) = decide(&hostile(Some(r.clone())), &o);
            assert_eq!(engages(&acts, HUMAN), want, "{system} {hull:?}: {acts:?}");
            assert_eq!(fires_on(&acts, HUMAN), want, "{system} {hull:?}");
        }
        // Order: the same rows with `*` first engage nothing.
        let mut t = fixture_table();
        t.rules.rotate_right(1);
        let o = beside(DEEP_NULL, HUMAN, Some(SMALL_HULL));
        let (_, acts) = decide(&hostile(Some(rules(t))), &o);
        assert!(!engages(&acts, HUMAN), "{acts:?}");
        // Two humans the table allows: the nearer one.
        let mut o = beside(DEEP_NULL, OTHER, Some(SMALL_HULL));
        o.humans.push(ball(HUMAN, 5_000.0));
        o.human_types.insert(HUMAN, SMALL_HULL);
        let prey = r.prey(&hostile(None), &o).map(|(b, _)| b.id);
        assert_eq!(prey, Some(OTHER));
    }

    /// **Never a crime** (CP3b): over every system of the fixture universe,
    /// the server's home system and one nobody knows, with a table that
    /// says "engage" everywhere, the bot shoots the human only where the
    /// server's own `crimewatch::flags_apply` says flags do not apply --
    /// and it does shoot somewhere, so the rule is not vacuous.
    #[test]
    fn it_never_fires_where_cp3b_flags_apply() {
        let u = Universe::fixture();
        let reg = eve_server::Registry::new(eve_server::model::SandboxConfig::default());
        let everything =
            Table::parse(&format!("{HEADER}\n*\t-1.0\t1.0\t1\t[A] invented\n")).expect("parses");
        let r = Rules::new(everything)
            .with_universe(&u)
            .with_class(SMALL_HULL, Class::Small);
        let home = eve_server::model::Character::default()
            .solar_system_id
            .expect("the sandbox's home system");
        let systems: Vec<i64> = u
            .systems()
            .map(|s| s.id)
            .chain([home, 39_999_999])
            .collect();
        let mut fired = 0;
        for system in systems {
            let o = beside(system, HUMAN, Some(SMALL_HULL));
            let (_, acts) = decide(&hostile(Some(r.clone())), &o);
            if eve_server::crimewatch::flags_apply(&reg, system) {
                assert!(!engages(&acts, HUMAN), "{system}: {acts:?}");
            }
            if fires_on(&acts, HUMAN) {
                assert!(!r.flags_apply(system));
                fired += 1;
            }
        }
        assert!(fired > 0, "the fixture has a lawful system");
    }

    /// **Never a fleet member**: in a fleet the bot attacks no human -- not
    /// the boss, not anyone the driver counts as a human -- even with a
    /// stale target on one; out of the fleet the same turn is a shot.
    #[test]
    fn it_never_engages_a_fleet_member() {
        let r = rules(fixture_table());
        let mut o = beside(DEEP_NULL, HUMAN, Some(SMALL_HULL));
        let (_, acts) = decide(&hostile(Some(r.clone())), &o);
        assert!(fires_on(&acts, HUMAN), "out of a fleet: {acts:?}");

        o.fleet = Some(FleetView {
            fleet_id: 7,
            boss: Some(BOSS),
        });
        o.park.as_mut().expect("a park").boss = Some(ball(BOSS_SHIP, 2_000.0));
        let mut m = hostile(Some(r.clone()));
        m.target = Some((HUMAN, PREY_NAME.to_owned()));
        let (_, acts) = decide(&m, &o);
        assert!(!engages(&acts, HUMAN), "{acts:?}");
        assert!(!engages(&acts, BOSS_SHIP), "{acts:?}");
        assert_eq!(r.prey(&m, &o), None);

        // The guard alone: a shot at a human or the boss that is not this
        // turn's prey never leaves the role.
        let shots = vec![
            Act::Fire {
                gun: GUN_A,
                target: HUMAN,
            },
            Act::Lock(BOSS_SHIP),
        ];
        assert_eq!(
            guard(shots.clone(), &o, None),
            vec![Act::Wait(brain::SHORT_WAIT)]
        );
        assert_eq!(guard(shots, &o, Some(HUMAN))[0].kind(), ActKind::Fire);
    }

    /// Quiet at a belt: after [`HUNT_WAITS`] patrols it warps to the next
    /// belt by id, wrapping, and with one belt it stays.
    #[test]
    fn quiet_it_moves_on_to_the_next_belt() {
        let mut o = beside(DEEP_NULL, HUMAN, Some(SMALL_HULL));
        o.humans.clear();
        o.locked.clear();
        let park = o.park.as_mut().expect("a park");
        park.ego_tether = None;
        park.belts = vec![ball(40_000_003, 900.0), ball(40_000_001, 50_000_000.0)];
        let mut m = hostile(Some(rules(fixture_table())));
        let mut warped = None;
        for _ in 0..=HUNT_WAITS + 1 {
            let (next, acts) = decide(&m, &o);
            m = next;
            if let Some(Act::WarpToItem { id, .. }) = acts.first() {
                warped = Some(*id);
                break;
            }
            assert_eq!(acts, vec![Act::Wait(escort::PATROL_WAIT)]);
        }
        assert_eq!(warped, Some(40_000_001), "wraps to the lowest id");
        let park = o.park.as_mut().expect("a park");
        park.belts.truncate(1);
        assert_eq!(next_belt(park), None);
    }

    /// WARP-MIN: the next belt 20 km off is approached, never warped to.
    #[test]
    fn a_next_belt_inside_the_minimum_warp_is_approached() {
        let mut o = beside(DEEP_NULL, HUMAN, Some(SMALL_HULL));
        o.humans.clear();
        o.locked.clear();
        let park = o.park.as_mut().expect("a park");
        park.ego_tether = None;
        park.belts = vec![ball(40_000_003, 900.0), ball(40_000_001, 20_000.0)];
        let mut m = hostile(Some(rules(fixture_table())));
        for _ in 0..=HUNT_WAITS + 1 {
            let (next, acts) = decide(&m, &o);
            m = next;
            assert!(
                !acts.iter().any(|a| matches!(a, Act::WarpToItem { .. })),
                "{acts:?}"
            );
            if matches!(acts.first(), Some(Act::Follow { id: 40_000_001, .. })) {
                return;
            }
        }
        panic!("never approached the next belt");
    }

    /// The shipped table is well formed (never its values), and a wrong
    /// header is refused.
    #[test]
    fn the_shipped_table_parses_and_a_bad_header_is_refused() {
        let (rows, skipped) = authored::parse(SHIPPED, HEADER).expect("the header");
        assert!(skipped.is_empty(), "{skipped:?}");
        let t = Table::parse(SHIPPED).expect("parses");
        assert_eq!(t.rules.len(), rows.len(), "every row is a rule");
        assert!(Table::parse("class\tmin_sec\tmax_sec\tengage\twhy\n").is_err());
        let t = Table::parse(&format!(
            "{HEADER}\nfrigates\t-1\t0\t1\tx\nsmall\t0\t-1\t1\tx\n"
        ))
        .expect("parses");
        assert!(
            t.rules.is_empty(),
            "a bad class and an empty range are skipped"
        );
    }

    #[test]
    fn the_hostile_is_not_a_civilian() {
        assert!(!ROLE.civilian);
        assert_eq!(ROLE.name(), "hostile");
        assert_eq!(Class::parse("*"), None);
    }
}
