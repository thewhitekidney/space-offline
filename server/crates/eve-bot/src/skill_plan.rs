//! **A skill plan per career** (X-SKILL): what a bot trains, in order, and
//! how it keeps the server's skill queue full.
//!
//! - **The plans** ([`CORE_PLAN`] for every pilot, then the career's own:
//!   [`MINER_PLAN`], [`FOREMAN_PLAN`], [`ICE_PLAN`], [`GAS_PLAN`], [`BUILDER_PLAN`],
//!   [`HAULER_PLAN`], [`EXPLORER_PLAN`], [`RATTER_PLAN`], [`HUNTER_PLAN`],
//!   [`PI_PLAN`]; the trader's own plan is [`crate::trader::SKILL_PLAN`],
//!   X-TRADE) are tables of **skill names** and levels, never type ids. A
//!   name is resolved against the content's skill table at run time
//!   ([`resolve`]); a name the table lacks is skipped. Every row and every
//!   level is **[A]**, falsified by a career whose pilot sits on an empty
//!   queue while its work waits on a skill a plan does not name.
//! - **Prerequisites** come from the same table ([`expand`]): a step's
//!   missing prerequisites are put in front of it, recursively, at the
//!   level the table asks for.
//! - **The queue-fill decision** ([`decide`], pure): the next steps whose
//!   books the pilot owns or a seller at this station asks for, at most
//!   [`QUEUE_TARGET`] entries in the queue in all, and the books of one
//!   run cost at most [`BOOK_SHARE`] of the liquid ISK.
//! - **[`fill`]** reads the pilot's skills, queue and wallet, decides, and
//!   hands the result to [`crate::acquire::learn_skills`], which buys the
//!   books from asks (NPC sellers at school stations or the market, never
//!   GM), injects them and queues the levels.
//!
//! The effects of a trained level are read where they already are (mining
//! yield, invention chance, trade tax); this module does no effect maths.

use crate::brain::RoleRef;
use crate::{Conn, Result};
use eve_server::content::Content;
use std::collections::BTreeMap;
use std::time::Duration;

/// One row of a plan: a skill by name, and the level to train it to.
pub type Want = (&'static str, i64);

/// How often a docked bot tops up its queue **[A]**: books are bought at
/// school stations and a queue lasts hours, so a half-hour check is plenty.
/// Falsified by a skill queue that runs dry while the bot sits docked.
pub const FILL_EVERY: Duration = Duration::from_secs(20 * 60);

/// The short wait after a fill that spent free points while some remain
/// **[A]**: the books that were bought may have room for more. Falsified by
/// market-read load in a 300-bot soak.
pub const FREE_SP_FILL_EVERY: Duration = Duration::from_secs(60);

/// When the next fill runs (WS-3): soon only if the last one spent free
/// points and some are left, else [`FILL_EVERY`], so a bot with no
/// affordable books does not read the market every minute.
pub fn next_fill(spent: i64, left: i64) -> Duration {
    if spent > 0 && left > 0 {
        FREE_SP_FILL_EVERY
    } else {
        FILL_EVERY
    }
}

/// The queue length the bot keeps, in entries (one per level) **[A]**: a
/// player queues a day or two ahead. Falsified by a queue that runs empty
/// between fills (too short) or training time the wallet cannot feed
/// (too long).
pub const QUEUE_TARGET: usize = 8;

/// The share of the liquid ISK one fill may spend on books **[A]**: a book
/// is a small buy next to a hull, and a pilot with 200k does not spend 100k
/// on Hull Upgrades. Falsified by pilots that never learn a plan's
/// first rows (too low) or run short for a hull after learning (too high).
pub const BOOK_SHARE: f64 = 0.25;

/// **What every pilot wants** **[A]**: the fitting skills (CPU, power grid,
/// capacitor), the hull and the legs. Level 3 first, level 4 after.
pub const CORE_PLAN: &[Want] = &[
    ("Spaceship Command", 2),
    ("Mechanics", 3),
    ("CPU Management", 3),
    ("Power Grid Management", 3),
    ("Capacitor Management", 3),
    ("Navigation", 3),
    ("Hull Upgrades", 3),
    ("CPU Management", 4),
    ("Power Grid Management", 4),
    ("Capacitor Management", 4),
    ("Navigation", 4),
];

/// **The miner** **[A]**: yield first (Mining, Astrogeology), then the
/// upgrades that fit more laser, then (PROG-MINE) the ladder: the Venture's
/// Mining Frigate, the barge (Mining Barge needs Industry V and Astrogeology
/// III in this client's table, [`expand`] puts them in front), the Strip
/// Miner's Mining IV, the mining drones, and the exhumer (Exhumers needs
/// Mining Barge V and Astrogeology V). Levels past the hull's own ask are the
/// hull bonuses. Every row and level is **[A]** except what a hull or laser
/// asks, which is the type data's ([`crate::mine_ladder::HULLS`]). Falsified
/// by a miner that sits on an empty queue while a hull it could pay for
/// waits on a skill this does not name.
pub const MINER_PLAN: &[Want] = &[
    ("Mining", 3),
    ("Astrogeology", 3),
    ("Mining Upgrades", 3),
    ("Mining", 4),
    ("Mining Frigate", 1),
    ("Astrogeology", 4),
    ("Mining Upgrades", 4),
    ("Mining Frigate", 3),
    ("Mining Barge", 1),
    ("Mining Drone Operation", 3),
    ("Mining Barge", 3),
    ("Mining", 5),
    ("Mining Precision", 3),
    ("Exhumers", 1),
    ("Mining Barge", 5),
    ("Exhumers", 3),
    ("Mining Drone Operation", 5),
    ("Exhumers", 5),
];

/// **The foreman** (the Orca pilot, PROG-MINE) **[A]**: the command skills
/// a mining burst asks (Leadership, Mining Foreman), the hull's (Industrial
/// Command Ships needs Mining Director I and Spaceship Command V), then
/// the compressor's skill and the burst's strength. Falsified by a foreman
/// that cannot fit a burst or a compressor for want of a skill named here.
pub const FOREMAN_PLAN: &[Want] = &[
    ("Leadership", 3),
    ("Mining Foreman", 3),
    ("Shipboard Compression Technology", 1),
    ("Mining Foreman", 5),
    ("Mining Director", 1),
    ("Industrial Command Ships", 1),
    ("Command Burst Specialist", 1),
    ("Shipboard Compression Technology", 3),
    ("Industrial Command Ships", 3),
    ("Mining Director", 3),
];

/// **The ice miner** **[A]**: Ice Harvesting needs Mining (the table says
/// how far). A harvester fits only a barge or an exhumer (ICE-HULL, #130),
/// so the barge's skills (Mining Barge I, Astrogeology III) come before the
/// harvester's higher levels, and the exhumer's after them, as the miner's
/// plan has them. Falsified by an ice miner that holds a barge it cannot
/// board, or flies none for want of a skill named nowhere here.
pub const ICE_PLAN: &[Want] = &[
    ("Mining", 4),
    ("Astrogeology", 3),
    ("Mining Barge", 1),
    ("Ice Harvesting", 3),
    ("Mining Upgrades", 3),
    ("Ice Harvesting", 4),
    ("Mining Barge", 3),
    ("Mining Barge", 5),
    ("Exhumers", 1),
];

/// **The gas huffer** **[A]**.
pub const GAS_PLAN: &[Want] = &[
    ("Gas Cloud Harvesting", 3),
    ("Gas Cloud Harvesting", 4),
    ("Mining Upgrades", 3),
];

/// **The builder** (industry and invention) **[A]**: Industry (build time),
/// then the lab and the mass-production slots, then the research skills.
pub const BUILDER_PLAN: &[Want] = &[
    ("Industry", 3),
    ("Science", 3),
    ("Laboratory Operation", 3),
    ("Mass Production", 3),
    ("Industry", 4),
    ("Advanced Industry", 3),
    ("Laboratory Operation", 4),
    ("Mass Production", 4),
    ("Advanced Industry", 4),
    ("Industry", 5),
];

/// **The reactor** (MN-G) **[A]**: Reactions III is what every formula asks
/// (R-MOON §1.1, **[V-data]**), then the shorter jobs and the slot skills a
/// chain of two formulas needs.
pub const REACTOR_PLAN: &[Want] = &[
    ("Reactions", 3),
    ("Mass Reactions", 1),
    ("Reactions", 4),
    ("Mass Reactions", 2),
];

/// **The hauler and courier** **[A]**: a slow hull that must not die.
pub const HAULER_PLAN: &[Want] = &[
    ("Acceleration Control", 3),
    ("Evasive Maneuvering", 3),
    ("Warp Drive Operation", 3),
    ("Fuel Conservation", 3),
    ("Mechanics", 4),
    ("Hull Upgrades", 4),
    ("Evasive Maneuvering", 4),
];

/// **The explorer and hacker** **[A]**: the minigame skills first.
pub const EXPLORER_PLAN: &[Want] = &[
    ("Hacking", 3),
    ("Archaeology", 3),
    ("Astrometrics", 3),
    ("Hacking", 4),
    ("Archaeology", 4),
    ("Astrometrics", 4),
    ("Survey", 3),
];

/// **The ratter** (and the site runner, the mission runner) **[A]**: guns,
/// their damage skills, drones, a salvager's skill and a little tank.
pub const RATTER_PLAN: &[Want] = &[
    ("Gunnery", 4),
    ("Small Projectile Turret", 3),
    ("Rapid Firing", 3),
    ("Sharpshooter", 3),
    ("Motion Prediction", 3),
    ("Surgical Strike", 3),
    ("Drones", 3),
    ("Light Drone Operation", 3),
    ("Salvaging", 3),
    ("Repair Systems", 3),
    ("Shield Operation", 3),
    ("Small Projectile Turret", 4),
    ("Weapon Upgrades", 3),
];

/// **The hunter** (PvP) **[A]**: guns, speed and the point.
pub const HUNTER_PLAN: &[Want] = &[
    ("Gunnery", 4),
    ("Small Projectile Turret", 4),
    ("Propulsion Jamming", 3),
    ("Afterburner", 3),
    ("Evasive Maneuvering", 3),
    ("Rapid Firing", 3),
    ("Sharpshooter", 3),
    ("Motion Prediction", 3),
    ("Target Management", 3),
    ("High Speed Maneuvering", 3),
    ("Propulsion Jamming", 4),
];

/// **The planetary pilot** **[A]**.
pub const PI_PLAN: &[Want] = &[
    ("Command Center Upgrades", 3),
    ("Interplanetary Consolidation", 3),
    ("Planetology", 3),
    ("Command Center Upgrades", 4),
    ("Interplanetary Consolidation", 4),
];

/// The plans a role follows, in order: [`CORE_PLAN`] first, then its
/// career's. The trader's career plan is its own
/// ([`crate::trader::SKILL_PLAN`]).
pub fn plans_of(role: RoleRef) -> Vec<&'static [Want]> {
    let is = |r: RoleRef| std::ptr::eq(role, r);
    let career: &'static [Want] = if is(&crate::roles::MINE) {
        MINER_PLAN
    } else if is(&crate::foreman::ROLE) {
        FOREMAN_PLAN
    } else if is(&crate::ice::ROLE) {
        ICE_PLAN
    } else if is(&crate::huffer::ROLE) {
        GAS_PLAN
    } else if is(&crate::builder::ROLE) {
        BUILDER_PLAN
    } else if is(&crate::reactor::ROLE) {
        REACTOR_PLAN
    } else if is(&crate::courier::ROLE) {
        HAULER_PLAN
    } else if is(&crate::hacker::ROLE) || is(&crate::probe::ROLE) {
        EXPLORER_PLAN
    } else if is(&crate::roles::ESCORT)
        || is(&crate::wing::ROLE)
        || is(&crate::wing_fight::LOGI_ROLE)
        || is(&crate::site_runner::ROLE)
        || is(&crate::missioner::ROLE)
        || is(&crate::plexer::ROLE)
    {
        RATTER_PLAN
    } else if is(&crate::hunt::ROLE) || is(&crate::hostile::ROLE) || is(&crate::roam_gang::ROLE) {
        HUNTER_PLAN
    } else if is(&crate::pi::ROLE) {
        PI_PLAN
    } else {
        &[]
    };
    vec![CORE_PLAN, career]
}

/// **Names to ids** (pure): each row of `plans`, in order, whose name
/// `find` knows. A name the content lacks is dropped.
pub fn resolve(plans: &[&[Want]], find: impl Fn(&str) -> Option<i64>) -> Vec<(i64, i64)> {
    plans
        .iter()
        .flat_map(|p| p.iter())
        .filter_map(|&(name, level)| find(name).map(|id| (id, level)))
        .collect()
}

/// **Prerequisites in front** (pure): every step of `steps` preceded by the
/// steps its `requires(skill)` list names, recursively, each at the table's
/// level. A skill's first mention fixes its place; a later, higher mention
/// raises that entry's level in place, so one queue trains it in one run.
/// A cycle in the table cannot loop: a skill already on the path is not
/// entered again.
pub fn expand(steps: &[(i64, i64)], requires: impl Fn(i64) -> Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    fn put(
        out: &mut Vec<(i64, i64)>,
        path: &mut Vec<i64>,
        skill: i64,
        level: i64,
        requires: &dyn Fn(i64) -> Vec<(i64, i64)>,
    ) {
        if path.contains(&skill) {
            return;
        }
        if out.iter().any(|&(s, l)| s == skill && l >= level) {
            return;
        }
        path.push(skill);
        for (r, l) in requires(skill) {
            put(out, path, r, l, requires);
        }
        path.pop();
        match out.iter_mut().find(|(s, _)| *s == skill) {
            Some(entry) => entry.1 = level,
            None => out.push((skill, level)),
        }
    }
    let mut out = Vec::new();
    for &(skill, level) in steps {
        put(&mut out, &mut Vec::new(), skill, level, &requires);
    }
    out
}

/// **The queue-fill decision** (pure): from the plan `steps` (prerequisites
/// already in front, [`expand`]), the pilot's `levels` (`{skill: trained
/// level}`, a key meaning the book is injected), its `queue`
/// (`(skill, to-level)` rows) and its liquid `wallet`, the `(skill, level)`
/// targets to queue now.
///
/// - Room is [`QUEUE_TARGET`] less the queue's length; a level takes one
///   entry, so the last target may be cut short to fit.
/// - A step is done when the skill is trained or queued to its level.
/// - A skill whose book the pilot lacks needs `price(skill)` (the cheapest
///   ask here; `None` is no seller and the step waits) and the run's total
///   book spend stays within [`BOOK_SHARE`] of `wallet`.
/// - A step is taken only when each prerequisite is trained, or owned and
///   queued or staged to its level: a prerequisite whose book is yet to be
///   bought holds its dependents back to the next run, so a failed purchase
///   can never leave the server a queue it refuses.
pub fn decide(
    steps: &[(i64, i64)],
    levels: &BTreeMap<i64, i64>,
    queue: &[(i64, i64)],
    wallet: f64,
    mut price: impl FnMut(i64) -> Option<f64>,
    requires: impl Fn(i64) -> Vec<(i64, i64)>,
) -> Vec<(i64, i64)> {
    let mut room = QUEUE_TARGET.saturating_sub(queue.len());
    let mut budget = wallet.max(0.0) * BOOK_SHARE;
    let mut staged: Vec<(i64, i64)> = Vec::new();
    // The level a skill will have once the queue and this run's targets
    // finish, or `None` without its book.
    let reach = |staged: &[(i64, i64)], skill: i64| -> Option<i64> {
        let owned = levels.get(&skill).copied()?;
        let queued = queue
            .iter()
            .filter(|&&(s, _)| s == skill)
            .map(|&(_, l)| l)
            .max()
            .unwrap_or(0);
        let ours = staged
            .iter()
            .find(|&&(s, _)| s == skill)
            .map_or(0, |&(_, l)| l);
        Some(owned.max(queued).max(ours))
    };
    for &(skill, level) in steps {
        if room == 0 {
            break;
        }
        let have = reach(&staged, skill);
        if have.is_some_and(|h| h >= level) {
            continue;
        }
        let ready = requires(skill).iter().all(|&(r, l)| {
            levels.get(&r).is_some_and(|&t| t >= l) || reach(&staged, r).is_some_and(|h| h >= l)
        });
        if !ready {
            continue;
        }
        let from = have.unwrap_or(0);
        if have.is_none() {
            let Some(cost) = price(skill) else { continue };
            if cost > budget {
                continue;
            }
            budget -= cost;
        }
        let to = level.min(from + room as i64);
        room -= (to - from) as usize;
        match staged.iter_mut().find(|(s, _)| *s == skill) {
            Some(entry) => entry.1 = to,
            None => staged.push((skill, to)),
        }
    }
    staged
}

/// **The doctrine's skills, once** (DOC-5, pure): `rows` (the role fit's
/// skills) put after the career plan `steps`, the cheapest unlock first
/// (fewest missing levels against `levels`, ties in the fit's order),
/// prerequisites in front ([`expand`]). A skill the plan already holds at
/// that level is not added again; one held at a lower level is raised in
/// place. Merging the same rows twice changes nothing.
pub fn merge_doctrine(
    steps: &[(i64, i64)],
    rows: &[(i64, i64)],
    levels: &BTreeMap<i64, i64>,
    requires: impl Fn(i64) -> Vec<(i64, i64)>,
) -> Vec<(i64, i64)> {
    let mut ordered: Vec<(i64, i64)> = rows.to_vec();
    // Stable: equal gaps keep the fit's order.
    ordered.sort_by_key(|&(s, l)| (l - levels.get(&s).copied().unwrap_or(0)).max(0));
    let mut out = steps.to_vec();
    for (skill, level) in expand(&ordered, requires) {
        match out.iter_mut().find(|(s, _)| *s == skill) {
            Some(entry) => entry.1 = entry.1.max(level),
            None => out.push((skill, level)),
        }
    }
    out
}

/// What one [`fill`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filled {
    /// The targets decided, `(skill, level)`.
    pub targets: Vec<(i64, i64)>,
    /// What [`crate::acquire::learn_skills`] queued, and could not.
    pub learned: Vec<i64>,
    pub missing: Vec<String>,
}

/// **Keep the queue filled**, docked: read the pilot's skills, queue and
/// wallet, [`decide`] the next steps of `role`'s plans, and buy, inject and
/// queue them ([`crate::acquire::learn_skills`]). Books are bought at this
/// station only **[A]** (falsified by a career whose home sells none of its
/// books and who never learns). No GM.
pub fn fill(conn: &mut Conn, content: &Content, role: RoleRef) -> Result<Filled> {
    fill_for(conn, content, role, None)
}

/// [`fill`] for a pilot whose place on the combat ladder is known
/// (PROG-PVE): a fighter's plan ends with [`crate::progression::plan`], the
/// racial hull skills and the gun skills of the class after the one it
/// flies, for the weapon its race favours ([`crate::progression::Rung::weapon`]).
pub fn fill_for(
    conn: &mut Conn,
    content: &Content,
    role: RoleRef,
    rung: Option<crate::progression::Rung>,
) -> Result<Filled> {
    fill_with(conn, content, role, rung, None)
}

/// [`fill_for`] for a pilot with a doctrine (DOC-5): the career plan, the
/// ladder, then the role fit's skills ([`merge_doctrine`]). With a doctrine
/// the ladder's next-class steps are dropped: the doctrine's class replaces
/// them ([`crate::progression::wanted_with`]).
pub fn fill_with(
    conn: &mut Conn,
    content: &Content,
    role: RoleRef,
    rung: Option<crate::progression::Rung>,
    doctrine: Option<&crate::doctrine::RoleFit>,
) -> Result<Filled> {
    let Some(station) = conn.session().station_id() else {
        return Ok(Filled::default());
    };
    let ladder: Vec<Want> = rung
        .filter(|_| crate::progression::climbs(role) && doctrine.is_none())
        .map(|r| crate::progression::plan(r, r.weapon))
        .unwrap_or_default();
    let mut names: Vec<&[Want]> = plans_of(role);
    names.push(&ladder);
    let requires = |s: i64| eve_server::skill::required_in(content, s).to_vec();
    let steps = expand(
        &resolve(&names, |n| {
            eve_server::skill::find_any_in(content, n).map(|s| s.type_id)
        }),
        requires,
    );
    if steps.is_empty() && doctrine.is_none() {
        return Ok(Filled::default());
    }
    let levels = crate::acquire::skill_levels(conn)?;
    // SKILLGATE: a climbing role whose skills board no table hull starts with
    // the drawn race's Frigate 1 and its small turret 1 -- the first hull it
    // may buy; until then it flies the rookie ship's civilian gun.
    let steps = match rung.filter(|r| {
        crate::progression::climbs(role)
            && doctrine.is_none()
            && !crate::progression::has_usable_hull(r, &levels)
    }) {
        Some(r) => {
            let head = [
                (
                    r.race.hull_skill(crate::hull_choice::Class::Frigate),
                    crate::progression::FLY_LEVEL,
                ),
                (
                    crate::fit_plan::fit_for(crate::hull_choice::Class::Frigate, r.weapon).skill,
                    crate::progression::FLY_LEVEL,
                ),
            ];
            let mut out = expand(&head, requires);
            for st in steps {
                if !out.contains(&st) {
                    out.push(st);
                }
            }
            out
        }
        None => steps,
    };
    let steps = match doctrine {
        Some(fit) => merge_doctrine(&steps, &fit.skills, &levels, requires),
        None => steps,
    };
    let queue = crate::acquire::skill_queue(conn)?;
    let wallet = conn.cash_balance()?;
    let mut failed = false;
    let targets = decide(
        &steps,
        &levels,
        &queue,
        wallet,
        |skill| match conn.market_orders(skill) {
            Ok(book) => cheapest_ask(&book, station),
            Err(_) => {
                failed = true;
                None
            }
        },
        requires,
    );
    let mut out = Filled {
        targets,
        ..Filled::default()
    };
    if failed {
        out.missing
            .push("skill plan: a market read failed".to_owned());
    }
    if !out.targets.is_empty() {
        let got = crate::acquire::learn_skills(conn, &out.targets, None)?;
        out.learned = got.learned;
        out.missing.extend(got.missing);
    }
    Ok(out)
}

/// The cheapest ask at `station` (pure).
pub fn cheapest_ask(book: &crate::verbs::Book, station: i64) -> Option<f64> {
    book.asks
        .iter()
        .filter(|a| !a.bid && a.station_id == station && a.vol_remaining > 0 && a.price > 0.0)
        .map(|a| a.price)
        .min_by(f64::total_cmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids.
    const A: i64 = 900_001;
    const B: i64 = 900_002;
    const C: i64 = 900_003;
    const D: i64 = 900_004;

    /// `B` needs `A` at 3, `C` needs `B` at 2.
    fn requires(s: i64) -> Vec<(i64, i64)> {
        match s {
            B => vec![(A, 3)],
            C => vec![(B, 2)],
            _ => vec![],
        }
    }

    fn cheap(_: i64) -> Option<f64> {
        Some(100.0)
    }

    #[test]
    fn the_next_fill_is_soon_only_while_points_were_spent_and_remain() {
        assert_eq!(next_fill(10, 5), FREE_SP_FILL_EVERY);
        assert_eq!(next_fill(0, 5), FILL_EVERY);
        assert_eq!(next_fill(10, 0), FILL_EVERY);
        assert_eq!(next_fill(0, 0), FILL_EVERY);
    }

    #[test]
    fn the_skill_plan_gains_the_doctrines_skills_once() {
        let career = [(A, 2), (D, 1)];
        // The fit needs C (which pulls B and A in front), D at 1 (the plan
        // has it) and A at 4 (the plan has it lower).
        let rows: [(i64, i64); 3] = [(C, 1), (D, 1), (A, 4)];
        let levels: BTreeMap<i64, i64> = BTreeMap::from([(A, 3)]);
        let once = merge_doctrine(&career, &rows, &levels, requires);
        // A is raised in place, D is not repeated, B and C follow.
        assert_eq!(once[0], (A, 4));
        assert_eq!(once.iter().filter(|(s, _)| *s == D).count(), 1);
        assert_eq!(once.iter().filter(|(s, _)| *s == A).count(), 1);
        let pos = |s: i64| once.iter().position(|(x, _)| *x == s).expect("listed");
        assert!(pos(B) < pos(C), "prerequisites in front: {once:?}");
        // Once: merging the result again changes nothing.
        assert_eq!(merge_doctrine(&once, &rows, &levels, requires), once);
        // The cheapest unlock first: D (one level short) before C (three).
        let fresh = merge_doctrine(&[], &[(C, 3), (D, 1)], &BTreeMap::new(), requires);
        assert_eq!(fresh.last(), Some(&(C, 3)), "{fresh:?}");
        assert_eq!(fresh.first(), Some(&(D, 1)), "{fresh:?}");
        // Skills the plan holds already: no change.
        let none = merge_doctrine(&career, &[(A, 2)], &levels, requires);
        assert_eq!(none, career);
    }

    #[test]
    fn resolve_keeps_order_and_drops_unknown_names() {
        let plan: &[Want] = &[("x", 3), ("gone", 1), ("y", 2)];
        let got = resolve(&[plan, &[("x", 4)]], |n| match n {
            "x" => Some(A),
            "y" => Some(B),
            _ => None,
        });
        assert_eq!(got, [(A, 3), (B, 2), (A, 4)]);
    }

    #[test]
    fn a_roaming_gang_member_follows_the_hunters_plan() {
        let plans = plans_of(&crate::roam_gang::ROLE);
        assert!(plans.contains(&HUNTER_PLAN));
    }

    #[test]
    fn expand_puts_prerequisites_first_recursively() {
        assert_eq!(expand(&[(C, 1)], requires), [(A, 3), (B, 2), (C, 1)]);
    }

    #[test]
    fn expand_raises_a_skill_in_place_and_survives_a_cycle() {
        let got = expand(&[(A, 1), (D, 1), (A, 4)], |_| vec![]);
        assert_eq!(got, [(A, 4), (D, 1)]);
        let cyc = |s: i64| if s == A { vec![(B, 1)] } else { vec![(A, 1)] };
        let got = expand(&[(A, 2)], cyc);
        assert!(got.contains(&(A, 2)) && got.len() <= 2, "{got:?}");
    }

    #[test]
    fn every_plan_name_is_unique_per_level_and_levels_are_one_to_five() {
        let all = [
            CORE_PLAN,
            MINER_PLAN,
            ICE_PLAN,
            GAS_PLAN,
            BUILDER_PLAN,
            HAULER_PLAN,
            EXPLORER_PLAN,
            RATTER_PLAN,
            HUNTER_PLAN,
            PI_PLAN,
        ];
        for plan in all {
            let mut seen = std::collections::BTreeSet::new();
            for &(name, level) in plan {
                assert!((1..=5).contains(&level), "{name}");
                assert!(seen.insert((name, level)), "{name} {level} twice");
            }
            let mut best: BTreeMap<&str, i64> = BTreeMap::new();
            for &(name, level) in plan {
                let top = best.entry(name).or_insert(0);
                assert!(level > *top, "{name} must rise down the table");
                *top = level;
            }
        }
    }

    #[test]
    fn a_fresh_pilot_stages_what_the_room_allows_in_plan_order() {
        let steps = [(A, 3), (B, 2), (D, 1)];
        let got = decide(&steps, &BTreeMap::new(), &[], 1_000_000.0, cheap, requires);
        // B waits for A's book to be injected: next run.
        assert_eq!(got, [(A, 3), (D, 1)]);
    }

    #[test]
    fn an_owned_prerequisite_trained_or_queued_releases_its_dependents() {
        let steps = [(A, 3), (B, 2)];
        let levels: BTreeMap<i64, i64> = [(A, 0)].into();
        let got = decide(&steps, &levels, &[], 1e6, cheap, requires);
        assert_eq!(got, [(A, 3), (B, 2)]);
        // Trained to 3: B alone.
        let levels: BTreeMap<i64, i64> = [(A, 3)].into();
        assert_eq!(decide(&steps, &levels, &[], 1e6, cheap, requires), [(B, 2)]);
    }

    #[test]
    fn what_the_queue_holds_is_not_queued_again_and_fills_the_rest() {
        let steps = [(A, 3), (D, 2)];
        let levels: BTreeMap<i64, i64> = [(A, 1), (D, 0)].into();
        let queue = [(A, 2), (A, 3)];
        let got = decide(&steps, &levels, &queue, 1e6, cheap, requires);
        assert_eq!(got, [(D, 2)]);
    }

    #[test]
    fn a_full_queue_stages_nothing_and_the_last_target_is_cut_to_the_room() {
        let steps = [(A, 5)];
        let levels: BTreeMap<i64, i64> = [(A, 0)].into();
        let full: Vec<(i64, i64)> = (0..QUEUE_TARGET as i64).map(|l| (D, l + 1)).collect();
        assert!(decide(&steps, &levels, &full, 1e6, cheap, requires).is_empty());
        let almost = &full[..QUEUE_TARGET - 2];
        assert_eq!(
            decide(&steps, &levels, almost, 1e6, cheap, requires),
            [(A, 2)]
        );
    }

    #[test]
    fn books_cost_at_most_the_share_of_the_wallet_and_need_a_seller() {
        let steps = [(A, 1), (D, 1), (C, 1)];
        // The budget is BOOK_SHARE of 1000: it buys the first book only.
        let wallet = 1_000.0;
        let price = |_| Some(BOOK_SHARE * wallet * 0.6);
        let got = decide(&steps, &BTreeMap::new(), &[], wallet, price, |_| vec![]);
        assert_eq!(got, [(A, 1)]);
        // No seller for D: C is next.
        let no_d = |s| if s == D { None } else { Some(1.0) };
        let got = decide(&steps, &BTreeMap::new(), &[], 1e6, no_d, |_| vec![]);
        assert_eq!(got, [(A, 1), (C, 1)]);
        // A book already owned costs nothing: a poor pilot still queues it.
        let owned: BTreeMap<i64, i64> = [(A, 0)].into();
        let got = decide(&[(A, 2)], &owned, &[], 0.0, |_| Some(1e9), |_| vec![]);
        assert_eq!(got, [(A, 2)]);
    }

    #[test]
    fn the_cheapest_ask_is_here_only() {
        use crate::verbs::{Book, BookOrder};
        let o = |station, price| BookOrder {
            order_id: 1,
            type_id: A,
            station_id: station,
            price,
            vol_remaining: 1,
            bid: false,
        };
        let book = Book {
            asks: vec![o(1, 30.0), o(2, 5.0), o(1, 20.0)],
            bids: vec![],
        };
        assert_eq!(cheapest_ask(&book, 1), Some(20.0));
        assert_eq!(cheapest_ask(&book, 3), None);
    }

    #[test]
    fn every_role_has_the_core_plan_first() {
        for r in crate::roles::ROLES {
            let p = plans_of(r);
            assert_eq!(p[0], CORE_PLAN, "{}", r.name);
        }
        assert_eq!(plans_of(&crate::roles::MINE)[1], MINER_PLAN);
    }

    /// PROG-MINE: the miner's plan climbs the ladder in order (Mining
    /// Frigate, Mining Barge, Exhumers), the barge's prerequisites come
    /// from the table in front of it, and the foreman has a plan of its own.
    #[test]
    fn the_miner_plan_climbs_frigate_barge_exhumer_and_the_foreman_has_its_own() {
        let at = |name: &str, level: i64| {
            MINER_PLAN
                .iter()
                .position(|&(n, l)| n == name && l == level)
                .unwrap_or_else(|| panic!("{name} {level} is in the plan"))
        };
        assert!(at("Mining Frigate", 1) < at("Mining Barge", 1));
        assert!(at("Mining Barge", 1) < at("Exhumers", 1));
        assert!(at("Mining Barge", 5) < at("Exhumers", 5));
        // Invented ids and a table shaped like the client's: Mining Barge
        // asks Industry V, Astrogeology III and Mining Frigate III.
        let id = |name: &str| match name {
            "Mining Barge" => Some(2),
            "Industry" => Some(3),
            "Astrogeology" => Some(4),
            "Mining Frigate" => Some(5),
            _ => None,
        };
        let steps = resolve(&[&[("Mining Barge", 1)]], id);
        assert_eq!(steps, vec![(2, 1)]);
        let requires = |s: i64| match s {
            2 => vec![(3, 5), (4, 3), (5, 3)],
            _ => vec![],
        };
        assert_eq!(
            expand(&steps, requires),
            vec![(3, 5), (4, 3), (5, 3), (2, 1)],
            "the barge's prerequisites first, at the table's levels"
        );
        let foreman = plans_of(&crate::foreman::ROLE);
        assert_eq!(foreman[0], CORE_PLAN);
        assert_eq!(foreman[1], FOREMAN_PLAN);
        assert!(FOREMAN_PLAN.iter().any(|&(n, _)| n == "Mining Foreman"));
        assert!(
            FOREMAN_PLAN
                .iter()
                .any(|&(n, _)| n == "Industrial Command Ships")
        );
    }
}
