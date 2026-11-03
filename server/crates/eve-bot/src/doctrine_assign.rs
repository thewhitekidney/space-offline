//! **Members comply** (DOC-5, R-DOCTRINE 8.2): what the leader's doctrine
//! asks of one fighter.
//!
//! - [`assign`] (pure, deterministic): the doctrine's roles are filled by
//!   quota ([`quotas`]: each non-damage role's share of the fighters, the
//!   rest damage dealers); a specialist role goes to the fighters with the
//!   fewest days of training to its fit ([`gap_days`]), ties by character
//!   id, so a logistics role lands on the pilots whose repair skills are
//!   furthest along.
//! - [`HallState::plan_for`] / [`Hall::plan_for`]: the plan of one bot, its
//!   role and that role's [`RoleFit`]. `None` for a non-fighter, a corp with
//!   no leader or doctrine yet, or a hall without a catalog.
//! - [`report`]: a bot's readiness (skills, system, ship) into the hall at
//!   each social turn ([`crate::corpmind::turn`]); [`ship_of`] reads the
//!   ship state from the hangar's hulls.
//! - [`outfit`]: dock-side, fit the hangar's modules of the role fit in one
//!   `FitAuto` (what `tank::outfit` did for the old fits).
//!
//! A switch of doctrine changes only the book's `current`; a member moves
//! over by replace-on-loss ([`crate::hull_choice`]'s `DOC_BONUS` ranks the
//! doctrine hull, [`crate::hull_choice::board_choice`] keeps an old hull
//! flying until it is lost).

use crate::corpmind::{Hall, HallState};
use crate::doctrine::{Doctrine, Role, RoleFit};
use crate::doctrine_hall::{Readiness, TRAIN_DAYS_PER_LEVEL, is_fighter};
use crate::doctrine_theatre::Ship;
use crate::society::Group;
use crate::{Conn, ItemRow, Result};
use std::collections::{BTreeMap, BTreeSet};

/// One fighter as the assignment sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub char_id: i64,
    /// Skill levels; `None`: not reported yet (nothing counts as trained).
    pub levels: Option<BTreeMap<i64, i64>>,
    /// Whether the career can camp (it hunts: a gang or a hunter, not a
    /// ratter). Only a camper takes an Interdict seat.
    pub camper: bool,
}

/// What the hall tells one bot to fly.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberPlan {
    /// The doctrine's catalog key.
    pub key: String,
    pub role: Role,
    pub fit: RoleFit,
}

/// **Days of training** a pilot needs for `fit` (pure): the missing levels of
/// every skill the fit names, at [`TRAIN_DAYS_PER_LEVEL`] each. An unreported
/// pilot has none of them.
pub fn gap_days(levels: Option<&BTreeMap<i64, i64>>, fit: &RoleFit) -> f64 {
    fit.skills
        .iter()
        .map(|&(skill, need)| {
            let have = levels.and_then(|l| l.get(&skill)).copied().unwrap_or(0);
            f64::from((need - have).max(0) as i32) * TRAIN_DAYS_PER_LEVEL
        })
        .sum()
}

/// The index of the damage role (the first `Dps`, else the first role).
fn dps_index(d: &Doctrine) -> usize {
    d.roles
        .iter()
        .position(|(r, _, _)| *r == Role::Dps)
        .unwrap_or(0)
}

/// Whether a career hunts, so its bot can camp (a ratter's career never
/// launches a probe).
pub fn is_camper(career: &str) -> bool {
    matches!(
        crate::society::family_of(career),
        Some(crate::society::Family::Fight)
    )
}

/// How long a sighting of a role hull keeps its seat open, in game days **[A]**.
/// Falsified by a dictor seat that stays filled with nobody able to buy the
/// hull (shorten it), or one that flaps open and shut (lengthen it).
pub const SEEN_DAYS: f64 = 3.0;

/// Whether a role gets seats (BP-CAMP, #125). Interdict opens only while its
/// hull was seen on sale or held ("nothing from nowhere": `Makeable` reads
/// content tables, not stock, so a dictor nobody can buy would take a seat).
/// Scout stays closed: it has no behaviour (see `docs/lanes/bp-camp.md`), so
/// a seat would sit idle. A closed role's surplus falls back to damage.
fn seat_open(role: Role, hull: i64, seen: &BTreeSet<i64>) -> bool {
    match role {
        Role::Interdict => seen.contains(&hull),
        Role::Scout => false,
        _ => true,
    }
}

/// **Seats per role** for `n` fighters (pure): each other role's share of
/// `n`, rounded, never more than are left; the damage role takes the rest
/// (surplus falls back to damage).
pub fn quotas(d: &Doctrine, n: usize, seen: &BTreeSet<i64>) -> Vec<usize> {
    let mut q = vec![0usize; d.roles.len()];
    if q.is_empty() {
        return q;
    }
    let dps = dps_index(d);
    let mut left = n;
    for (i, (role, share, fit)) in d.roles.iter().enumerate() {
        if i == dps || !seat_open(*role, fit.hull, seen) {
            continue;
        }
        let want = (share * n as f64).round().max(0.0) as usize;
        let take = want.min(left);
        q[i] = take;
        left -= take;
    }
    q[dps] += left;
    q
}

/// **Who flies which role** (pure, deterministic): `char -> role index`
/// into `d.roles`. Each specialist role, in the doctrine's order, takes its
/// quota from the fighters still free, fewest [`gap_days`] first.
pub fn assign(d: &Doctrine, fighters: &[Candidate], seen: &BTreeSet<i64>) -> BTreeMap<i64, usize> {
    let mut out = BTreeMap::new();
    if d.roles.is_empty() {
        return out;
    }
    let dps = dps_index(d);
    let q = quotas(d, fighters.len(), seen);
    let mut free: Vec<&Candidate> = fighters.iter().collect();
    for (i, (_, _, fit)) in d.roles.iter().enumerate() {
        if i == dps || q[i] == 0 {
            continue;
        }
        free.sort_by(|a, b| {
            gap_days(a.levels.as_ref(), fit)
                .total_cmp(&gap_days(b.levels.as_ref(), fit))
                .then(a.char_id.cmp(&b.char_id))
        });
        // An Interdict seat goes to a career that hunts: a ratter's career
        // never launches a probe, so the seat would sit idle on it.
        let camp_only = d.roles[i].0 == Role::Interdict;
        let mut left = q[i];
        free.retain(|c| {
            if left > 0 && (c.camper || !camp_only) {
                left -= 1;
                out.insert(c.char_id, i);
                false
            } else {
                true
            }
        });
    }
    for c in free {
        out.insert(c.char_id, dps);
    }
    out
}

impl HallState {
    /// **The role hulls seated now** (BP-CAMP) for one doctrine leader's
    /// group: the hull types seen on sale or held within [`SEEN_DAYS`] of the
    /// newest report.
    pub fn open_hulls(&self, group: Group) -> BTreeSet<i64> {
        let now = self.readiness.values().map(|r| r.at).max().unwrap_or(0);
        let keep = (SEEN_DAYS * self.day as f64) as u64;
        self.hulls_seen
            .iter()
            .filter(|((g, _), at)| *g == group && now.saturating_sub(**at) <= keep)
            .map(|(&(_, h), _)| h)
            .collect()
    }

    /// The doctrine leader's group this bot belongs to.
    fn group_of(&self, char_id: i64) -> Option<Group> {
        let me = self.members.get(&char_id)?;
        self.doctrine_leaders()
            .into_iter()
            .find(|l| l.corps.contains(&me.corp))
            .map(|l| l.group)
    }

    /// **The hulls this bot's doctrine seats on a sighting** (its Interdict
    /// roles) and the group they are kept under: what the bot looks for on
    /// the market and in its hangar.
    pub fn watched_hulls(&self, char_id: i64) -> (Group, Vec<i64>) {
        let none = (0, Vec::new());
        let Some(world) = self.doctrine.as_ref() else {
            return none;
        };
        let Some(group) = self.group_of(char_id) else {
            return none;
        };
        let Some(i) = self
            .dbooks
            .get(&group)
            .and_then(|b| b.current.clone())
            .and_then(|key| world.find(&key))
        else {
            return none;
        };
        let hulls = world.catalog[i]
            .roles
            .iter()
            .filter(|(r, _, _)| *r == Role::Interdict)
            .map(|(_, _, f)| f.hull)
            .collect();
        (group, hulls)
    }

    /// Group `group`'s bots looked for these role hulls at `now`, and found
    /// the ones in `seen` (a look that found nothing is kept too, so it is
    /// not repeated within a game day).
    pub fn see_hulls(&mut self, group: Group, looked: &[i64], seen: &[i64], now: u64) {
        for &h in looked {
            self.hulls_looked.insert((group, h), now);
        }
        for &h in seen {
            self.hulls_seen.insert((group, h), now);
        }
    }

    /// **The plan of one bot** (module header).
    pub fn plan_for(&self, char_id: i64) -> Option<MemberPlan> {
        let world = self.doctrine.as_ref()?;
        let me = self.members.get(&char_id)?;
        if !is_fighter(&me.career) {
            return None;
        }
        let leader = self
            .doctrine_leaders()
            .into_iter()
            .find(|l| l.corps.contains(&me.corp))?;
        let key = self.dbooks.get(&leader.group)?.current.clone()?;
        let d = &world.catalog[world.find(&key)?];
        let fighters: Vec<Candidate> = self
            .members
            .iter()
            .filter(|(_, m)| leader.corps.contains(&m.corp) && is_fighter(&m.career))
            .map(|(c, m)| Candidate {
                char_id: *c,
                levels: self.readiness.get(c).and_then(|r| r.skills.clone()),
                camper: is_camper(&m.career),
            })
            .collect();
        let i = *assign(d, &fighters, &self.open_hulls(leader.group)).get(&char_id)?;
        let (role, _, fit) = &d.roles[i];
        // Variety: a line bot draws a real fit of the doctrine's hull
        // (same family and layer) by its own seed; else the greedy fit.
        let real = (*role == Role::Dps)
            .then(|| {
                world.seeds.pick(
                    fit.hull,
                    d.family,
                    d.layer,
                    roll_of(self.doctrine_seed, char_id),
                )
            })
            .flatten();
        Some(MemberPlan {
            key,
            role: *role,
            fit: real.unwrap_or(fit).clone(),
        })
    }
}

/// A bot's own draw in `[0, 1)` (splitmix64 of the run's seed and the bot).
pub fn roll_of(seed: u64, char_id: i64) -> f64 {
    let mut z = seed ^ (char_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

impl Hall {
    /// The plan of one bot ([`HallState::plan_for`]).
    pub fn plan_for(&self, char_id: i64) -> Option<MemberPlan> {
        self.lock().plan_for(char_id)
    }
}

/// **The ship state** the machine counts (pure): a doctrine hull in hand is
/// `Staged`; none, and the fit must be bought (`Market`).
pub fn ship_of(hull: i64, hangar: &[ItemRow]) -> Ship {
    if hangar
        .iter()
        .any(|r| r.type_id == hull && r.category_id == crate::acquire::CATEGORY_SHIP)
    {
        Ship::Staged
    } else {
        Ship::Market
    }
}

/// **A bot's report** (pure part): its readiness from what it read.
pub fn readiness(
    levels: Option<BTreeMap<i64, i64>>,
    system: Option<i64>,
    ship: Option<Ship>,
    now: u64,
) -> Readiness {
    Readiness {
        skills: levels,
        system,
        online: true,
        wake_in_s: None,
        ship,
        jumps_to: BTreeMap::new(),
        at: now,
    }
}

/// **Report to the hall** (each social turn, docked): skills, system and,
/// with a doctrine, whether the bot holds the hull. A failed read is a
/// report without it, never an error; only a dead connection is one.
pub fn report(conn: &mut Conn, hall: &Hall, me: i64, now: u64) -> Result<()> {
    if hall.lock().doctrine.is_none() {
        return Ok(());
    }
    let levels = match crate::acquire::skill_levels(conn) {
        Ok(l) => Some(l),
        Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
        Err(_) => None,
    };
    let system = conn.session().solar_system_id();
    let ship = match hall.plan_for(me) {
        Some(p) => match conn.hangar_items() {
            Ok(h) => Some(ship_of(p.fit.hull, &h)),
            Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
            Err(_) => None,
        },
        None => None,
    };
    hall.report_readiness(me, readiness(levels, system, ship, now));
    look_for_role_hulls(conn, hall, me, now)
}

/// **Look for the role hulls** (BP-CAMP, #125): a docked bot of a doctrine with
/// an Interdict role posts the hull as seen when its hangar holds one or the
/// market lists one for sale. A sighting still fresh is not looked up again
/// (one `GetOrders` per hull per game day at most). Soft but for a dead
/// connection.
fn look_for_role_hulls(conn: &mut Conn, hall: &Hall, me: i64, now: u64) -> Result<()> {
    let (group, watched): (Group, Vec<i64>) = {
        let st = hall.lock();
        let day = st.day;
        // Any look counts, the one that found nothing too.
        let fresh = |g: Group, h: &i64| {
            st.hulls_looked
                .get(&(g, *h))
                .is_some_and(|at| now.saturating_sub(*at) < day)
        };
        let (g, hulls) = st.watched_hulls(me);
        (g, hulls.into_iter().filter(|h| !fresh(g, h)).collect())
    };
    if watched.is_empty() {
        return Ok(());
    }
    let hangar = match conn.hangar_items() {
        Ok(h) => h,
        Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
        Err(_) => Vec::new(),
    };
    let mut seen = Vec::new();
    let looked = watched.clone();
    for h in watched {
        let mut found = held(&hangar, h);
        if !found {
            found = match conn.market_orders(h) {
                Ok(book) => !book.asks.is_empty(),
                Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
                Err(_) => false,
            };
        }
        if found {
            seen.push(h);
        }
    }
    hall.lock().see_hulls(group, &looked, &seen, now);
    Ok(())
}

/// Whether the hangar holds a hull of `hull` (assembled or packaged).
pub fn held(hangar: &[ItemRow], hull: i64) -> bool {
    hangar
        .iter()
        .any(|r| r.type_id == hull && r.category_id == crate::acquire::CATEGORY_SHIP)
}

/// **Fit the role fit** (docked, the doctrine hull boarded): the hangar's
/// modules the fit names and the ship lacks, in one `FitAuto`
/// ([`crate::fit_plan::fit_ids`]). The note says what happened; empty when
/// there was nothing to fit.
pub fn outfit(conn: &mut Conn, fit: &RoleFit) -> Result<String> {
    let hangar = conn.hangar_items()?;
    let fitted = conn.fitted()?;
    let ids = crate::fit_plan::fit_ids(fit, &hangar, &fitted);
    if ids.is_empty() {
        return Ok(String::new());
    }
    match conn.fit_auto(&ids) {
        Ok(rows) => {
            // The server onlines what the budget allows; ask for the rest,
            // softly (a refused online is not a fault).
            for r in &rows {
                if let Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) =
                    conn.set_online(r.item_id)
                {
                    return Err(e);
                }
            }
            Ok(format!("doctrine fit: {} modules fitted", rows.len()))
        }
        Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => Err(e),
        Err(e) => Ok(format!("doctrine fit: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctrine::testkit::{brawler, flat_tank};
    use crate::doctrine::{Family, FleetSize, Layer, Range, Stance};
    use crate::hull_choice::Class;

    const HULL: i64 = 970_500_001;
    const LOGI_SKILL: i64 = 970_500_101;
    const TACKLE_SKILL: i64 = 970_500_102;

    fn fit(skills: &[(i64, i64)]) -> RoleFit {
        RoleFit {
            hull: HULL,
            modules: vec![],
            charges: vec![],
            skills: skills.to_vec(),
            profile: brawler(flat_tank(1.0, 1.0, 1.0)),
            cost: 1.0,
        }
    }

    fn doctrine() -> Doctrine {
        Doctrine {
            key: "invented".to_owned(),
            class: Class::Cruiser,
            family: Family::Projectile,
            layer: Layer::Armour,
            active: false,
            range: Range::Short,
            stance: Stance::Brawl,
            roles: vec![
                (Role::Dps, 0.6, fit(&[])),
                (Role::Logi, 0.2, fit(&[(LOGI_SKILL, 4)])),
                (Role::Tackle, 0.2, fit(&[(TACKLE_SKILL, 3)])),
            ],
            fleet: FleetSize {
                min: 2,
                target: 10,
                max_on_grid: 30,
            },
        }
    }

    const DICTOR: i64 = 9_011;

    /// A doctrine that carries both T2 roles, the dictor with a hull of its own.
    fn t2_doctrine() -> Doctrine {
        let mut d = doctrine();
        let mut dictor = fit(&[]);
        dictor.hull = DICTOR;
        d.roles.push((Role::Interdict, 0.05, dictor));
        d.roles.push((Role::Scout, 0.05, fit(&[])));
        d
    }

    #[test]
    fn a_t2_role_gets_no_seat_until_its_hull_is_seen() {
        // Makeable reads content tables, not stock: a dictor "makeable" at
        // boot must not take a seat in a fleet of ten or more.
        let d = t2_doctrine();
        let none = BTreeSet::new();
        for n in [0, 1, 10, 20, 40] {
            let q = quotas(&d, n, &none);
            assert_eq!(q[3], 0, "interdict seat at {n}");
            assert_eq!(q[4], 0, "scout seat at {n}");
            assert_eq!(q.iter().sum::<usize>(), n);
        }
        let crew: Vec<Candidate> = (1..=20).map(|i| pilot(i, &[])).collect();
        let a = assign(&d, &crew, &none);
        assert!(a.values().all(|&i| i < 3));
    }

    #[test]
    fn a_dictor_hull_seen_opens_the_interdict_seat_and_never_the_scout_seat() {
        let d = t2_doctrine();
        let seen: BTreeSet<i64> = [DICTOR, HULL].into_iter().collect();
        let q = quotas(&d, 20, &seen);
        assert_eq!(q[3], 1, "one dictor in twenty");
        assert_eq!(q[4], 0, "a scout has no behaviour, so no seat");
        assert_eq!(q.iter().sum::<usize>(), 20);
        // Too few fighters: the seat rounds to nothing.
        assert_eq!(quotas(&d, 4, &seen)[3], 0);
        let crew: Vec<Candidate> = (1..=20).map(|i| pilot(i, &[])).collect();
        let a = assign(&d, &crew, &seen);
        assert_eq!(a.values().filter(|&&i| i == 3).count(), 1);
        // A hull that is not the dictor's opens nothing.
        let other: BTreeSet<i64> = [HULL].into_iter().collect();
        assert_eq!(quotas(&d, 20, &other)[3], 0);
    }

    #[test]
    fn the_hall_opens_a_seat_on_a_fresh_sighting_and_closes_it_when_stale() {
        let mut st = crate::corpmind::HallState::new(100);
        st.readiness.insert(1, readiness(None, None, None, 1_000));
        assert!(st.open_hulls(7).is_empty());
        st.see_hulls(7, &[DICTOR], &[DICTOR], 900);
        assert_eq!(st.open_hulls(7), [DICTOR].into_iter().collect());
        // Another leader's group does not see it.
        assert!(st.open_hulls(8).is_empty());
        // The newest report is more than SEEN_DAYS later.
        st.readiness.insert(
            2,
            readiness(None, None, None, 900 + (SEEN_DAYS * 100.0) as u64 + 1),
        );
        assert!(st.open_hulls(7).is_empty());
        st.see_hulls(7, &[DICTOR], &[DICTOR], 900 + (SEEN_DAYS * 100.0) as u64);
        assert!(!st.open_hulls(7).is_empty());
        // A look that found nothing is kept, and does not open a seat.
        st.see_hulls(9, &[DICTOR], &[], 900);
        assert_eq!(st.hulls_looked.get(&(9, DICTOR)), Some(&900));
        assert!(st.open_hulls(9).is_empty());
    }

    #[test]
    fn only_a_hunting_career_takes_the_interdict_seat() {
        let d = t2_doctrine();
        let seen: BTreeSet<i64> = [DICTOR, HULL].into_iter().collect();
        let mut crew: Vec<Candidate> = (1..=20).map(|i| pilot(i, &[])).collect();
        for c in &mut crew[..19] {
            c.camper = false;
        }
        let a = assign(&d, &crew, &seen);
        let dictors: Vec<i64> = a
            .iter()
            .filter(|(_, r)| **r == 3)
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(dictors, vec![20], "the one camper in the crew");
        // No camper at all: the seat falls back to damage.
        crew[19].camper = false;
        let a = assign(&d, &crew, &seen);
        assert!(a.values().all(|&r| r != 3));
        assert!(is_camper("pirate") && !is_camper("ratter") && !is_camper("miner"));
    }

    #[test]
    fn a_hull_in_the_hangar_is_held() {
        let row = |t, c| ItemRow {
            item_id: 1,
            type_id: t,
            owner_id: 0,
            location_id: 0,
            flag_id: 4,
            quantity: 1,
            group_id: 0,
            category_id: c,
            singleton: false,
        };
        assert!(held(&[row(DICTOR, crate::acquire::CATEGORY_SHIP)], DICTOR));
        assert!(!held(&[row(DICTOR, 7)], DICTOR));
        assert!(!held(&[], DICTOR));
    }

    fn pilot(id: i64, skills: &[(i64, i64)]) -> Candidate {
        Candidate {
            char_id: id,
            levels: Some(skills.iter().copied().collect()),
            camper: true,
        }
    }

    #[test]
    fn roles_follow_quotas() {
        let d = doctrine();
        assert_eq!(quotas(&d, 10, &BTreeSet::new()), vec![6, 2, 2]);
        assert_eq!(quotas(&d, 1, &BTreeSet::new()), vec![1, 0, 0]);
        assert_eq!(quotas(&d, 0, &BTreeSet::new()), vec![0, 0, 0]);
        // Ten pilots; ids 3 and 7 repair best, 5 and 9 have the tackle skill.
        let mut crew: Vec<Candidate> = (1..=10).map(|i| pilot(i, &[])).collect();
        crew[2] = pilot(3, &[(LOGI_SKILL, 4)]);
        crew[6] = pilot(7, &[(LOGI_SKILL, 3)]);
        crew[4] = pilot(5, &[(TACKLE_SKILL, 3)]);
        crew[8] = pilot(9, &[(TACKLE_SKILL, 2)]);
        let a = assign(&d, &crew, &BTreeSet::new());
        assert_eq!(a.len(), 10);
        let of = |role: usize| -> Vec<i64> {
            a.iter()
                .filter(|(_, r)| **r == role)
                .map(|(c, _)| *c)
                .collect()
        };
        assert_eq!(of(1), vec![3, 7]);
        assert_eq!(of(2), vec![5, 9]);
        assert_eq!(of(0).len(), 6);
        // Deterministic, and independent of the order the hall lists them.
        let mut rev = crew.clone();
        rev.reverse();
        assert_eq!(assign(&d, &rev, &BTreeSet::new()), a);
        // Few pilots: the surplus (everyone) flies damage.
        let two = assign(&d, &crew[..2], &BTreeSet::new());
        assert!(two.values().all(|r| *r == 0));
    }

    #[test]
    fn an_unreported_pilot_is_the_last_choice_for_a_specialist_seat() {
        let d = doctrine();
        let mut crew: Vec<Candidate> = (1..=5)
            .map(|i| Candidate {
                char_id: i,
                levels: None,
                camper: true,
            })
            .collect();
        crew[4] = pilot(5, &[(LOGI_SKILL, 1)]);
        let a = assign(&d, &crew, &BTreeSet::new());
        assert_eq!(a[&5], 1, "the only pilot with any repair skill");
        assert!(gap_days(None, &d.roles[1].2) > gap_days(crew[4].levels.as_ref(), &d.roles[1].2));
    }

    #[test]
    fn the_ship_state_follows_the_hangar() {
        assert_eq!(ship_of(HULL, &[]), Ship::Market);
        let row = |t, c| ItemRow {
            item_id: 1,
            type_id: t,
            owner_id: 0,
            location_id: 0,
            flag_id: 4,
            quantity: 1,
            group_id: 0,
            category_id: c,
            singleton: true,
        };
        assert_eq!(
            ship_of(HULL, &[row(HULL, crate::acquire::CATEGORY_SHIP)]),
            Ship::Staged
        );
        assert_eq!(
            ship_of(HULL, &[row(HULL, 7)]),
            Ship::Market,
            "a module of that type is no hull"
        );
    }
}
