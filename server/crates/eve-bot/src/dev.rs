//! **DEV-MODE bots** (`eve-bots --roster FILE`, against an `eve-server --dev`
//! world): bots with chosen skills and hulls, in fleets, on sides.
//!
//! The roster is a TSV, `#` comments, columns `group side role count skills
//! ship [system]`:
//!
//! - `group`: a name of letters and digits; its bots are `<group>1..N`, the
//!   first is the boss and forms the fleet the rest join ([`Fleeter`]; a
//!   `foreman` boss forms its own, as the role does).
//! - `side`: groups on one side are one corporation (allied); sides are
//!   other corporations, so [`crate::society::hostile_to`] counts them
//!   hostile and the `hunter` and `hostile` roles fight them.
//! - `role`: a `--loop` role name.
//! - `skills`: `all5`, `role` (the role's plans and the hull's skills, at V)
//!   or a plan name ([`plan_named`]).
//! - `ship`: a type **name**, resolved against the bundle at run time, or `-`.
//! - `system`: a system name or id the group's bots are seated in, else
//!   `--home`.
//!
//! What a roster bot does (`dev_turn` in the driver, once a connection, docked):
//! `/devskills`, `/devcorp`, `/devhome` (then a reconnect), and the closed
//! economy's own [`crate::acquire::run_order`] for the hull, its modules and
//! rounds, from the dev market. No GM item is created.

use crate::brain::RoleRef;
use crate::skill_plan::{self, Want};
use eve_server::content::Content;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// What a roster row asks for in skills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skills {
    All5,
    Role,
    Plan(String),
}

/// One roster row.
#[derive(Debug, Clone)]
pub struct Row {
    pub group: String,
    pub side: String,
    pub role: RoleRef,
    pub count: usize,
    pub skills: Skills,
    pub ship: Option<String>,
    pub system: Option<String>,
}

/// The skill plans by name, for a roster's `skills` column.
pub fn plan_named(name: &str) -> Option<&'static [Want]> {
    Some(match name {
        "core" => skill_plan::CORE_PLAN,
        "miner" => skill_plan::MINER_PLAN,
        "foreman" => skill_plan::FOREMAN_PLAN,
        "ice" => skill_plan::ICE_PLAN,
        "gas" => skill_plan::GAS_PLAN,
        "builder" => skill_plan::BUILDER_PLAN,
        "reactor" => skill_plan::REACTOR_PLAN,
        "hauler" => skill_plan::HAULER_PLAN,
        "explorer" => skill_plan::EXPLORER_PLAN,
        "ratter" => skill_plan::RATTER_PLAN,
        "hunter" => skill_plan::HUNTER_PLAN,
        "pi" => skill_plan::PI_PLAN,
        _ => return None,
    })
}

/// Parse a roster (pure). Errors name the line.
pub fn parse_roster(text: &str) -> Result<Vec<Row>, String> {
    let mut rows: Vec<Row> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').map(str::trim).collect();
        if f.len() < 6 || f.len() > 7 {
            return Err(format!(
                "roster line {n}: want group side role count skills ship [system], tab-separated"
            ));
        }
        let name_ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric());
        if !name_ok(f[0]) {
            return Err(format!(
                "roster line {n}: group {:?}: letters and digits only",
                f[0]
            ));
        }
        if !name_ok(f[1]) {
            return Err(format!(
                "roster line {n}: side {:?}: letters and digits only",
                f[1]
            ));
        }
        if rows.iter().any(|r| r.group == f[0]) {
            return Err(format!("roster line {n}: group {} given twice", f[0]));
        }
        let role = crate::roles::parse(f[2]).ok_or_else(|| {
            format!(
                "roster line {n}: role {:?}: {}",
                f[2],
                crate::roles::names()
            )
        })?;
        let count: usize =
            f[3].parse().ok().filter(|c| *c > 0).ok_or_else(|| {
                format!("roster line {n}: count {:?}: want a positive number", f[3])
            })?;
        let skills = match f[4] {
            "all5" => Skills::All5,
            "role" => Skills::Role,
            p if plan_named(p).is_some() => Skills::Plan(p.to_owned()),
            other => {
                return Err(format!(
                    "roster line {n}: skills {other:?}: all5, role or a plan (core miner foreman ice gas builder reactor hauler explorer ratter hunter pi)"
                ));
            }
        };
        let opt = |s: &str| (!s.is_empty() && s != "-").then(|| s.to_owned());
        rows.push(Row {
            group: f[0].to_owned(),
            side: f[1].to_owned(),
            role,
            count,
            skills,
            ship: opt(f[5]),
            system: f.get(6).and_then(|s| opt(s)),
        });
    }
    if rows.is_empty() {
        return Err("roster: no rows".to_owned());
    }
    Ok(rows)
}

/// The ticker of the hostile side in the kit drills (`/devcorp FOES`).
pub const FOES_TICKER: &str = "FOES";

impl DevBot {
    /// Whether this bot is on the drill's hostile side.
    pub fn is_foe(&self) -> bool {
        self.ticker == FOES_TICKER
    }
}

/// The corporation ticker of a side: its first five characters, upper case.
/// Two sides that collide are refused by [`expand`].
pub fn ticker_of(side: &str) -> String {
    side.chars()
        .take(5)
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Which characters each group has, shared by the process: a boss invites
/// the ones that logged in.
#[derive(Debug, Default)]
pub struct DevHall {
    groups: Mutex<BTreeMap<String, Vec<i64>>>,
}

impl DevHall {
    /// A bot of `group` is in the game as `char_id`.
    pub fn register(&self, group: &str, char_id: i64) {
        if let Ok(mut g) = self.groups.lock() {
            let v = g.entry(group.to_owned()).or_default();
            if !v.contains(&char_id) {
                v.push(char_id);
            }
        }
    }

    /// The group's characters.
    pub fn members(&self, group: &str) -> Vec<i64> {
        self.groups
            .lock()
            .ok()
            .and_then(|g| g.get(group).cloned())
            .unwrap_or_default()
    }
}

/// One roster bot's dev settings, in its [`crate::brain::Policy`].
#[derive(Debug, Clone)]
pub struct DevBot {
    pub group: String,
    pub ticker: String,
    pub skills: Skills,
    /// The skill ids `Skills::Role` or `Skills::Plan` come to, resolved from
    /// the bundle; `None` falls back to `all5` (no bundle).
    pub skill_ids: Option<Vec<i64>>,
    pub hull: Option<i64>,
    pub home: Option<String>,
    pub boss: bool,
    pub hall: std::sync::Arc<DevHall>,
}

impl PartialEq for DevBot {
    fn eq(&self, other: &DevBot) -> bool {
        self.group == other.group && self.ticker == other.ticker && self.hull == other.hull
    }
}

impl DevBot {
    /// The `/devskills` lines this bot's skills come to (pure).
    pub fn skill_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        match (&self.skills, &self.skill_ids) {
            (Skills::All5, _) | (_, None) => out.push("/devskills all5".to_owned()),
            (_, Some(ids)) => {
                let list: Vec<String> = ids.iter().map(i64::to_string).collect();
                out.push(format!("/devskills ids {}", list.join(",")));
                if let Some(h) = self.hull {
                    out.push(format!("/devskills hull {h}"));
                }
            }
        }
        out
    }
}

/// The skills the role kit's types ask for directly (SKILLGATE, pure over
/// `content`); none without a typedata table.
fn kit_skills(content: &Content, role: RoleRef) -> Vec<i64> {
    let Some(t) = content.try_get::<eve_server::typedata::Installed>() else {
        return Vec::new();
    };
    let kit = crate::acquire::kit_of(role);
    kit.hull
        .into_iter()
        .chain(kit.modules.map(|m| m.0))
        .chain(kit.ammo.map(|m| m.0))
        .chain(kit.more.iter().map(|m| m.0))
        .flat_map(|ty| eve_server::drone_skills::required_skills(t.table(), ty))
        .map(|(s, _)| s)
        .collect()
}

/// One expanded bot.
pub struct Expanded {
    pub name: String,
    pub role: RoleRef,
    pub dev: std::sync::Arc<DevBot>,
    /// ROAM-GANG: the seat of a `roamgang` bot in its group's gang (the
    /// group is the gang id, the roster order is the rank).
    pub gang: Option<crate::roam_gang::Seat>,
    /// FC-1: the seat of a `wing` bot (no FC yet: the caller sets `fc` and
    /// the process's hall -- a spawn's `fc=`).
    pub wing: Option<crate::wing::Seat>,
}

/// Expand a roster into bots, resolving names against `content` (pure over
/// it). A ship name the bundle lacks is an error naming the line.
pub fn expand(rows: &[Row], content: Option<&Content>) -> Result<Vec<Expanded>, String> {
    expand_with(rows, content, crate::roam_gang::GangKnobs::NORMAL)
}

/// [`expand`], the gangs' tunables at the run's difficulty.
pub fn expand_with(
    rows: &[Row],
    content: Option<&Content>,
    knobs: crate::roam_gang::GangKnobs,
) -> Result<Vec<Expanded>, String> {
    let mut tickers: BTreeMap<String, &str> = BTreeMap::new();
    for r in rows {
        let t = ticker_of(&r.side);
        if let Some(other) = tickers.insert(t.clone(), &r.side)
            && other != r.side
        {
            return Err(format!("sides {other} and {} share the ticker {t}", r.side));
        }
    }
    let hall = std::sync::Arc::new(DevHall::default());
    let gangs = std::sync::Arc::new(crate::roam_gang::GangHall::new());
    let wings = std::sync::Arc::new(crate::wing_hall::WingHall::new());
    let mut out = Vec::new();
    for r in rows {
        let hull = match (&r.ship, content) {
            (Some(name), Some(c)) => Some(
                eve_server::item::find_type_in(c, name)
                    .map(|t| t.type_id)
                    .ok_or_else(|| format!("group {}: no type named {name:?}", r.group))?,
            ),
            _ => None,
        };
        let skill_ids = content.and_then(|c| {
            let plans: Vec<&[Want]> = match &r.skills {
                Skills::All5 => return None,
                Skills::Role => skill_plan::plans_of(r.role),
                Skills::Plan(p) => vec![plan_named(p)?],
            };
            let ids = skill_plan::resolve(&plans, |n| {
                eve_server::skill::find_any_in(c, n).map(|s| s.type_id)
            });
            // A fighter's guns ask for turret skills its plans name only
            // for one weapon and size; the grant takes them all (the
            // roamers were refused the queue for want of Gunnery).
            let fights = plans
                .iter()
                .any(|p| [skill_plan::HUNTER_PLAN, skill_plan::RATTER_PLAN].contains(p));
            let guns = if fights {
                crate::fit_plan::gun_skills()
            } else {
                Vec::new()
            };
            // SKILLGATE: the role kit's own hull, modules and rounds ask for
            // skills the plans need not name (the grant takes the skill at
            // level 5, which any requirement is within).
            let kit = kit_skills(c, r.role);
            Some(
                ids.into_iter()
                    .map(|(id, _)| id)
                    .chain(guns)
                    .chain(kit)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            )
        });
        for i in 1..=r.count {
            let gang = (*r.role == crate::roam_gang::ROLE).then(|| crate::roam_gang::Seat {
                hall: std::sync::Arc::clone(&gangs),
                gang: r.group.clone(),
                rank: i - 1,
                size: r.count,
                knobs,
            });
            // FC-1: a wing's FC is a person; no wing bot forms a fleet of its own.
            let wing = crate::wing_fight::is_wing(r.role)
                .then(|| crate::wing::Seat::new(std::sync::Arc::clone(&wings), None));
            out.push(Expanded {
                name: format!("{}{i}", r.group),
                role: r.role,
                gang,
                wing,
                dev: std::sync::Arc::new(DevBot {
                    group: r.group.clone(),
                    ticker: ticker_of(&r.side),
                    skills: r.skills.clone(),
                    skill_ids: skill_ids.clone(),
                    hull,
                    home: r.system.clone(),
                    boss: i == 1 && !crate::wing_fight::is_wing(r.role),
                    hall: std::sync::Arc::clone(&hall),
                }),
            });
        }
    }
    let mut seen = BTreeSet::new();
    for e in &out {
        if !seen.insert(&e.name) {
            return Err(format!("two bots named {}", e.name));
        }
    }
    Ok(out)
}

/// What a boss's fleet tick remembers.
#[derive(Debug, Default)]
pub struct Fleeter {
    pub invited: BTreeSet<i64>,
    pub next: Option<std::time::Instant>,
    /// When the boss's dev kit was done (the start of [`FLEET_DEFER_S`]).
    pub since: Option<std::time::Instant>,
}

/// Seconds between a boss's looks for members to invite.
pub const FLEET_EVERY_S: u64 = 20;

/// FAST-KIT: a boss still docked this long after its dev kit invites anyway
/// (a role that never undocks must still form its fleet). **[A]**: 30 s,
/// about one quick prep; falsified by a boss invited-while-docked on a
/// normal quick spawn (then raise it).
pub const FLEET_DEFER_S: u64 = 30;

/// The members a boss still has to invite (pure): the group's characters
/// but `me`, not invited before.
pub fn to_invite(members: &[i64], me: i64, invited: &BTreeSet<i64>) -> Vec<i64> {
    members
        .iter()
        .copied()
        .filter(|c| *c != me && !invited.contains(c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "# group side role count skills ship system\n\
        mine1\tred\tmine\t5\trole\tVenture\t\n\
        orca1\tred\tforeman\t1\tall5\t-\t\n\
        pvpa\tblue\thunter\t3\thunter\tRifter\tInvented\n";

    #[test]
    fn only_the_foes_ticker_is_a_foe() {
        let mut d = DevBot {
            group: "g".into(),
            ticker: FOES_TICKER.into(),
            skills: Skills::Role,
            skill_ids: None,
            hull: None,
            home: None,
            boss: false,
            hall: std::sync::Arc::new(DevHall::default()),
        };
        assert!(d.is_foe());
        d.ticker = "ALLY".into();
        assert!(!d.is_foe());
    }

    #[test]
    fn a_roster_parses_and_expands_with_a_boss_per_group() {
        let rows = parse_roster(GOOD).expect("parses");
        assert_eq!(rows.len(), 3);
        let bots = expand(&rows, None).expect("expands");
        assert_eq!(bots.len(), 9);
        assert_eq!(bots[0].name, "mine11");
        assert!(bots[0].dev.boss && !bots[1].dev.boss);
        assert_eq!(bots[0].dev.ticker, "RED");
        assert_eq!(bots[8].dev.ticker, "BLUE");
        assert_eq!(bots[8].dev.home.as_deref(), Some("Invented"));
        // No bundle: role skills fall back to all5.
        assert_eq!(bots[0].dev.skill_lines(), vec!["/devskills all5"]);
    }

    /// FC-1: a wing's first bot is no boss (its FC is a person, and no wing
    /// forms a fleet of its own), every wing bot has a seat with no FC yet,
    /// and other roles have none.
    #[test]
    fn wing_rows_are_not_bosses_and_have_a_seat() {
        let rows = parse_roster("wg\tred\twing\t3\tall5\t-\t\nmn\tred\tmine\t2\tall5\t-\t\n")
            .expect("parses");
        let bots = expand(&rows, None).expect("expands");
        assert_eq!(bots.len(), 5);
        for b in &bots[..3] {
            assert!(!b.dev.boss, "{}", b.name);
            let seat = b.wing.as_ref().expect("a seat");
            assert_eq!(seat.fc, None);
        }
        assert!(bots[3].dev.boss && bots[3].wing.is_none());
        // One hall for the group's bots.
        let (a, b) = (bots[0].wing.as_ref(), bots[1].wing.as_ref());
        assert!(std::sync::Arc::ptr_eq(
            &a.expect("seat").hall,
            &b.expect("seat").hall
        ));
    }

    fn shipped(text: &str) -> Vec<Row> {
        parse_roster(text).expect("a shipped roster parses")
    }

    /// The rosters under `content/bots/dev/`: they parse, name types only
    /// (no ids), and have the shapes the owner asked for.
    #[test]
    fn the_shipped_rosters_have_their_shapes() {
        let mining = shipped(include_str!("../../../../content/bots/dev/mining.tsv"));
        let count = |rows: &[Row], role: &str| -> usize {
            rows.iter()
                .filter(|r| r.role.name() == role)
                .map(|r| r.count)
                .sum()
        };
        assert_eq!((count(&mining, "foreman"), count(&mining, "mine")), (2, 10));
        let pvp = shipped(include_str!("../../../../content/bots/dev/pvp.tsv"));
        assert_eq!(pvp.len(), 3);
        assert!(
            pvp[..2]
                .iter()
                .all(|r| r.count == 10 && r.role.name() == "hunter")
        );
        assert_eq!(pvp[2].role.name(), "roamgang", "the roaming gang");
        assert!(pvp[2].count >= 4 && pvp[2].side != pvp[0].side && pvp[2].side != pvp[1].side);
        assert_ne!(pvp[0].side, pvp[1].side, "opposite sides");
        assert_ne!(pvp[0].ship, pvp[1].ship, "different doctrines");
        let pve = shipped(include_str!("../../../../content/bots/dev/pve.tsv"));
        assert!(count(&pve, "sites") > 0);
        let world = shipped(include_str!("../../../../content/bots/dev/world50.tsv"));
        assert_eq!(world.iter().map(|r| r.count).sum::<usize>(), 50);
        for rows in [&mining, &pvp, &pve, &world] {
            assert!(expand(rows, None).is_ok());
            assert!(
                rows.iter()
                    .all(|r| r.ship.as_deref().is_none_or(|s| s.parse::<i64>().is_err())),
                "names, never ids"
            );
        }
    }

    #[test]
    fn the_servers_spawn_names_and_tickers_are_expands() {
        // BOT-ACCOUNTS: `eve_server::bot_control` makes accounts for
        // `<group><i>` in the corporation `ticker_of(side)`; see its
        // `provision`. If either rule changes, change both.
        let rows = parse_roster("grp\tredteam\tmine\t2\tall5\t-\n").unwrap();
        let bots = expand(&rows, None).unwrap();
        assert_eq!(bots[1].name, "grp2");
        assert_eq!(bots[1].dev.ticker, "REDTE");
    }

    #[test]
    fn a_bad_row_is_refused_with_its_line() {
        for bad in [
            "a\tb\tmine\t1\tall5",
            "a b\tred\tmine\t1\tall5\t-",
            "a\tred\tnorole\t1\tall5\t-",
            "a\tred\tmine\t0\tall5\t-",
            "a\tred\tmine\t1\tsomeplan\t-",
            "a\tred\tmine\t1\tall5\t-\na\tred\tmine\t1\tall5\t-",
        ] {
            assert!(parse_roster(bad).is_err(), "{bad:?}");
        }
        assert!(parse_roster("# nothing\n").is_err());
    }

    #[test]
    fn two_sides_may_not_share_a_ticker() {
        let rows = parse_roster("a\tredteam1\tmine\t1\tall5\t-\nb\tredteam2\tmine\t1\tall5\t-\n")
            .expect("parses");
        assert!(expand(&rows, None).is_err());
    }

    #[test]
    fn the_boss_invites_each_member_once() {
        let hall = DevHall::default();
        for c in [11, 12, 13] {
            hall.register("g", c);
        }
        hall.register("g", 12);
        let m = hall.members("g");
        assert_eq!(m, vec![11, 12, 13]);
        let mut invited = BTreeSet::new();
        assert_eq!(to_invite(&m, 11, &invited), vec![12, 13]);
        invited.insert(12);
        assert_eq!(to_invite(&m, 11, &invited), vec![13]);
    }

    #[test]
    fn role_skills_with_ids_say_ids_and_the_hull() {
        let hall = std::sync::Arc::new(DevHall::default());
        let d = DevBot {
            group: "g".into(),
            ticker: "G".into(),
            skills: Skills::Role,
            skill_ids: Some(vec![5, 6]),
            hull: Some(77),
            home: None,
            boss: true,
            hall,
        };
        assert_eq!(
            d.skill_lines(),
            vec!["/devskills ids 5,6", "/devskills hull 77"]
        );
    }
}
