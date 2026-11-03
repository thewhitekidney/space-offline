//! **The bot roles, one row each** (W10.5 seam S11).
//!
//! A role is a [`RoleSpec`]: its `--loop` name, the line it says when it
//! joins a fleet, whether it is a civilian, its docked and in-space rules,
//! its driver summary banner and its GM setup lines. [`ROLES`] is the table
//! that `--loop` parsing ([`parse`]) reads, and [`crate::brain::decide`]
//! and [`crate::driver`] read a bot's own row from
//! [`crate::brain::Policy::role`], so a new role is a new row here and an
//! edit nowhere else.
//!
//! `ROLES` is coordinator-only after S11 (`docs/content/plan.md` §6.1): a
//! packet exports its `RoleSpec`, and the coordinator adds the row.

use crate::brain::{self, Act, Memory, Observation};
use crate::escort;

/// The Mining skill a Miner I needs **[V-data]**.
pub const SKILL_MINING: i64 = 3386;

/// What a role's summary banner may say: the driver's counters.
#[derive(Debug, Clone, Copy)]
pub struct Summary<'a> {
    pub name: &'a str,
    /// `docked`, `homing`, `mining`, `fighting`, `in space` or `transit`.
    pub phase: &'a str,
    pub ore_unloaded: i64,
    /// ISK the miner's sales brought in, this run (FX6, [`Act::Sell`]).
    pub isk_sold: f64,
    pub kills: u32,
    pub reconnects: u32,
}

/// What GM setup must give the bot besides its skill: the brain's verdict
/// on the hangar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lacks {
    pub ship: bool,
    pub module: bool,
    pub ammo: bool,
    /// The wallet is empty (BOTMARK): the driver asks `GetCashBalance`
    /// only when the hull is missing, and a failed ask counts as empty, as
    /// before. A character the server made with `STARTING_BALANCE`
    /// (P-START-2, world accounts and `--character NAME,bot` alike) has
    /// funds, so it is not given [`STARTING_ISK`] on top.
    pub isk: bool,
}

/// One bot role. Rows compare by `name`, which [`ROLES`] keeps unique.
pub struct RoleSpec {
    /// The `--loop` name, and the role's name in logs.
    pub name: &'static str,
    /// Said once in fleet chat on joining a fleet. **[A]**
    pub greeting: &'static str,
    /// Does this role stay out of fights? Nothing reads it yet (S11).
    pub civilian: bool,
    /// Rule 3: what to do docked.
    pub docked: fn(&mut Memory, &Observation) -> Vec<Act>,
    /// Rule 5: what to do in space, bound.
    pub in_space: fn(&mut Memory, &Observation) -> Vec<Act>,
    /// The driver's periodic summary line.
    pub banner: fn(&Summary) -> String,
    /// GM setup: the slash lines that give the role its skill, and its
    /// hull, module(s) and rounds when it [`Lacks`] them. In the closed
    /// economy (E8) the same things are bought instead, by the role's
    /// [`crate::acquire::kit_of`].
    pub setup: fn(Lacks) -> Vec<String>,
}

impl RoleSpec {
    /// `self.name`: the pre-S11 `Role::name`.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl PartialEq for RoleSpec {
    fn eq(&self, other: &RoleSpec) -> bool {
        self.name == other.name
    }
}

impl Eq for RoleSpec {}

impl std::fmt::Debug for RoleSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RoleSpec({})", self.name)
    }
}

/// Mines, and follows a fleet boss to his rocks (P8b).
pub static MINE: RoleSpec = RoleSpec {
    name: "mine",
    greeting: "o7 -- mining with you",
    civilian: true,
    docked: crate::defend::mine_docked,
    in_space: crate::defend::mine_in_space,
    banner: mine_banner,
    setup: mine_setup,
};

/// Flies a combat ship beside its fleet boss and kills the NPCs that
/// threaten the fleet; alone, it rats (P8c, [`crate::escort`]).
pub static ESCORT: RoleSpec = RoleSpec {
    name: "escort",
    greeting: "o7 -- on your wing, I'll take the rats",
    civilian: false,
    docked: escort::docked,
    in_space: escort::in_space,
    banner: escort_banner,
    setup: escort_setup,
};

/// Every role, the default first. Coordinator-only after S11.
pub static ROLES: &[&RoleSpec] = &[
    &MINE,
    &ESCORT,
    &crate::courier::ROLE,
    &crate::builder::ROLE,
    &crate::hostile::ROLE,
    &crate::hunt::ROLE,
    &crate::roam_gang::ROLE,
    &crate::site_runner::ROLE,
    &crate::plexer::ROLE,
    &crate::trader::ROLE,
    &crate::huffer::ROLE,
    &crate::wh_dive::ROLE,
    &crate::wh_raid::ROLE,
    &crate::ice::ROLE,
    &crate::missioner::ROLE,
    &crate::probe::ROLE,
    &crate::pi::ROLE,
    &crate::hacker::ROLE,
    &crate::logi::ROLE,
    &crate::reactor::ROLE,
    &crate::capital::ROLE,
    &crate::capital::ALT_ROLE,
    &crate::hauler::ROLE,
    &crate::sov_bot::ROLE,
    // PROG-MINE: the Orca pilot who boosts and compresses for a mining fleet.
    &crate::foreman::ROLE,
    // FC-1: the escort's kit, trailing a human FC over gates and docks.
    &crate::wing::ROLE,
    // FC-2: the wing with a remote repairer for the FC.
    &crate::wing_fight::LOGI_ROLE,
];

/// The role a bot plays when nothing says otherwise.
pub fn default() -> &'static RoleSpec {
    ROLES[0]
}

/// The role named `name`, as `--loop` spells it.
pub fn parse(name: &str) -> Option<&'static RoleSpec> {
    ROLES.iter().copied().find(|r| r.name == name)
}

/// Every role's name, for an error message: `mine or escort`.
pub fn names() -> String {
    let names: Vec<&str> = ROLES.iter().map(|r| r.name).collect();
    names.join(" or ")
}

fn mine_banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, ore unloaded {} units, sold {:.2} ISK, reconnects {}",
        s.name, s.phase, s.ore_unloaded, s.isk_sold, s.reconnects
    )
}

fn escort_banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, kills {}, reconnects {}",
        s.name, s.phase, s.kills, s.reconnects
    )
}

/// ISK a fresh miner gives itself with its hull (FX6), so it can pay the
/// refinery's tax on its first load: a sandbox character used to start with
/// an empty wallet, and ore has no bid until it is refined. Since P-START-2
/// a character the server makes is funded, so only an empty wallet is
/// topped up ([`Lacks::isk`]): a character from a world saved before that,
/// or seeded without funds. **[A]** a new
/// pilot's pocket money; falsified by `sell` lines that still say
/// `NotEnoughMoney`, or by miners whose wallets are mostly this.
pub const STARTING_ISK: f64 = 100_000.0;

fn mine_setup(lacks: Lacks) -> Vec<String> {
    let mut lines = vec![format!("/giveskill {SKILL_MINING} 1")];
    if lacks.ship {
        lines.push("/createitem merlin".to_owned());
        if lacks.isk {
            lines.push(format!("/giveisk {STARTING_ISK:.0}"));
        }
    }
    if lacks.module {
        lines.push(format!("/createitem {}", brain::MINER_I));
    }
    lines
}

fn escort_setup(lacks: Lacks) -> Vec<String> {
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
    use super::*;

    /// FX6: a fresh miner (no hull) with an empty wallet also gets a
    /// wallet, once, with its hull; a miner that only lacks a laser does
    /// not, and neither does one the server already funded (BOTMARK).
    #[test]
    fn a_fresh_miner_gives_itself_pocket_money_with_its_hull() {
        let lacks = |ship, isk| Lacks {
            ship,
            module: true,
            ammo: false,
            isk,
        };
        let isk = |l: &[String]| l.iter().filter(|s| s.starts_with("/giveisk ")).count();
        assert_eq!(isk(&mine_setup(lacks(true, true))), 1);
        assert_eq!(isk(&mine_setup(lacks(false, true))), 0);
        assert_eq!(isk(&mine_setup(lacks(true, false))), 0);
        assert!(mine_setup(lacks(true, false)).contains(&"/createitem merlin".to_owned()));
    }

    #[test]
    fn names_are_unique_and_parse_round_trips() {
        for (i, r) in ROLES.iter().enumerate() {
            assert!(
                ROLES[..i].iter().all(|q| q.name != r.name),
                "{} twice",
                r.name
            );
            assert!(!r.name.is_empty());
            let back = parse(r.name).expect("every row parses by its name");
            assert!(std::ptr::eq(back, *r), "{} parses to another row", r.name);
        }
        assert_eq!(parse(""), None);
        assert_eq!(
            parse("MINE"),
            None,
            "names are exact, as --loop spells them"
        );
        assert!(std::ptr::eq(default(), ROLES[0]));
    }

    #[test]
    fn mine_is_civilian_and_escort_is_not() {
        assert!(MINE.civilian);
        assert!(!ESCORT.civilian);
    }
}
