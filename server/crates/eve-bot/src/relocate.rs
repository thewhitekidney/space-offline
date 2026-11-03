//! **Ratters and site runners move to richer ground as their ship grows**
//! (PROG-PVE2): a belt's rats and a site's difficulty rise as security falls
//! (`npc_gen::BUDGET_ANCHORS`: a high-sec belt squad carries about 12 k ISK of
//! bounty at 0.9 and 60 k at 0.5; `content/population.tsv`: high-sec holds
//! difficulty 1-2 anomalies, low-sec 2-3, null 3-5), and the site runner's
//! cap rises with the hull ([`crate::site_runner::difficulty_cap`]). A frigate
//! is right in a 0.9 system; the same bot in a cruiser is under-using its ship
//! there and, as a site runner, finds nothing above difficulty 2.
//!
//! # The rule **[A]**
//!
//! Every class has a **security band** ([`band`]). A docked ratter or site
//! runner (not a fleet escort: it follows its boss) whose system is **safer**
//! than its class's band moves, at most once per [`EVERY`], to the nearest
//! system with a station inside the band within [`JUMPS`] gates, by the map
//! and the bot's own travel floor ([`crate::brain::Policy::security_floor`]:
//! the band is cut to what the floor allows, so a high-sec society keeps its
//! bots in high-sec and they move to its lowest systems). It never moves
//! **towards** safety: a bot whose system is more dangerous than its band
//! stays, as before. The move is [`crate::drone_use::DroneAct::Move`]; the
//! station it docks at becomes its base.
//!
//! Bands (pseudo-security, [`crate::travel`]'s rounding) **[A]**:
//! frigate 0.7-1.0, destroyer 0.6-0.9, cruiser 0.5-0.8, battlecruiser
//! 0.4-0.7, battleship 0.0-0.6. Falsified by a ratter whose new system's
//! belts it cannot clear (too low; its bounty falls or it loses the hull), or
//! one whose income does not rise after the move (too timid), as the `[soc]`
//! progress line shows.

use crate::hull_choice::{Class, HULLS, class_of};
use crate::travel::Map;
use eve_server::celestial::pseudo_security;
use std::collections::{BTreeSet, VecDeque};
use std::time::Duration;

/// How often a docked bot asks whether its class has outgrown its system
/// **[A]**.
pub const EVERY: Duration = Duration::from_secs(60 * 60);
/// How far a relocation reaches, gates **[A]**.
pub const JUMPS: u32 = 6;
/// The nearest candidate systems one bot chooses among, by its seed **[A]**:
/// bots of one class do not all pick the same system.
pub const CHOICES: usize = 3;

/// The security band `(lowest, highest)` of a class **[A]**.
pub fn band(class: Class) -> (f64, f64) {
    match class {
        Class::Frigate => (0.7, 1.0),
        Class::Destroyer => (0.6, 0.9),
        Class::Cruiser => (0.5, 0.8),
        Class::Battlecruiser => (0.4, 0.7),
        Class::Battleship => (0.0, 0.6),
    }
}

/// The band cut to the bot's floor: the floor lifts the low end, and the high
/// end never falls under it. [`crate::travel::ANY_SECURITY`] leaves the band
/// alone.
pub fn band_at(class: Class, floor: f64) -> (f64, f64) {
    let (lo, hi) = band(class);
    let lo = lo.max(floor);
    (lo, hi.max(lo))
}

/// Whether `security` is safer than the class's band: the bot has outgrown
/// the system.
pub fn too_safe(class: Class, security: f64) -> bool {
    pseudo_security(security) > band(class).1
}

/// **Where to move** (pure): a station in the nearest systems inside the
/// class's band ([`band_at`]) within [`JUMPS`] gates of `from`, routed at
/// `floor`, none in `avoid`; one of the nearest [`CHOICES`] by `seed`. `None`
/// when the bot's own system is not too safe, or nothing qualifies.
pub fn target(
    map: &Map,
    from: i64,
    class: Class,
    floor: f64,
    seed: u64,
    avoid: &[i64],
) -> Option<i64> {
    if !map.security(from).is_some_and(|s| too_safe(class, s)) {
        return None;
    }
    let (lo, hi) = band_at(class, floor);
    let mut seen = BTreeSet::from([from]);
    let mut queue = VecDeque::from([(from, 0u32)]);
    let mut found: Vec<(u32, i64)> = Vec::new();
    while let Some((s, d)) = queue.pop_front() {
        if d > 0
            && !avoid.contains(&s)
            && !map.stations_in(s).is_empty()
            && map
                .security(s)
                .is_some_and(|x| (lo..=hi).contains(&pseudo_security(x)))
        {
            found.push((d, s));
        }
        if d == JUMPS {
            continue;
        }
        for n in map.neighbours(s) {
            if map.allowed(n, floor) && seen.insert(n) {
                queue.push_back((n, d + 1));
            }
        }
    }
    found.sort();
    let pool = found.len().min(CHOICES);
    let (_, system) = *found.get(usize::try_from(seed % pool.max(1) as u64).ok()?)?;
    map.stations_in(system).first().copied()
}

/// Whether a hull type is one the rule applies to (a table hull).
pub fn class_flown(flying: Option<i64>) -> Option<Class> {
    flying.and_then(|t| class_of(HULLS, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::travel::ANY_SECURITY;

    // An invented chain of systems, gates both ways: 1 (0.95) - 2 (0.8) -
    // 3 (0.65) - 4 (0.5) - 5 (0.35) - 6 (0.1).
    const SYSTEMS: [(i64, f64); 6] = [
        (1, 0.95),
        (2, 0.8),
        (3, 0.65),
        (4, 0.5),
        (5, 0.35),
        (6, 0.1),
    ];

    fn map() -> Map {
        let mut m = Map::new();
        for (id, sec) in SYSTEMS {
            m = m.with_security(id, sec).with_station(60_000_000 + id, id);
        }
        for pair in SYSTEMS.windows(2) {
            let (a, b) = (pair[0].0, pair[1].0);
            m = m
                .with_gate(a, 9_000 + a * 10 + b, b, 9_000 + b * 10 + a)
                .with_gate(b, 9_000 + b * 10 + a, a, 9_000 + a * 10 + b);
        }
        m
    }

    #[test]
    fn the_bands_step_down_with_the_class() {
        let lows: Vec<f64> = Class::ALL.iter().map(|c| band(*c).0).collect();
        let highs: Vec<f64> = Class::ALL.iter().map(|c| band(*c).1).collect();
        assert!(lows.windows(2).all(|w| w[0] >= w[1]), "{lows:?}");
        assert!(highs.windows(2).all(|w| w[0] >= w[1]), "{highs:?}");
        for c in Class::ALL {
            let (lo, hi) = band(c);
            assert!(lo < hi);
        }
    }

    #[test]
    fn a_frigate_is_at_home_everywhere_and_a_cruiser_leaves_the_safest_systems() {
        assert!(!too_safe(Class::Frigate, 1.0));
        assert!(!too_safe(Class::Cruiser, 0.8));
        assert!(too_safe(Class::Cruiser, 0.95));
        assert!(too_safe(Class::Battleship, 0.9));
        // Never towards safety: a system under the band is not "too safe".
        assert!(!too_safe(Class::Frigate, 0.45));
    }

    #[test]
    fn the_floor_cuts_the_band() {
        let (lo, hi) = band_at(Class::Battleship, 0.45);
        assert!(lo >= 0.45 && hi >= lo);
        assert_eq!(
            band_at(Class::Battleship, ANY_SECURITY),
            band(Class::Battleship)
        );
        // A floor above the whole band still leaves a place to be.
        let (lo, hi) = band_at(Class::Battlecruiser, 0.9);
        assert!(lo == 0.9 && hi >= lo);
    }

    #[test]
    fn a_cruiser_in_the_safest_system_moves_to_the_nearest_system_in_its_band() {
        let m = map();
        let got = target(&m, 1, Class::Cruiser, ANY_SECURITY, 0, &[]).expect("a target");
        // 0.8 is two gates away and inside the cruiser's 0.5-0.8.
        assert_eq!(m.system_of(got), Some(2));
        // Another seed chooses among the nearest few.
        let seen: BTreeSet<i64> = (0..9)
            .filter_map(|s| target(&m, 1, Class::Cruiser, ANY_SECURITY, s, &[]))
            .filter_map(|s| m.system_of(s))
            .collect();
        assert!(seen.len() > 1 && seen.len() <= CHOICES, "{seen:?}");
        assert!(seen.iter().all(|s| (2..=4).contains(s)));
    }

    #[test]
    fn a_bot_already_inside_or_below_its_band_stays() {
        let m = map();
        assert_eq!(target(&m, 3, Class::Cruiser, ANY_SECURITY, 0, &[]), None);
        assert_eq!(target(&m, 5, Class::Cruiser, ANY_SECURITY, 0, &[]), None);
        assert_eq!(target(&m, 1, Class::Frigate, ANY_SECURITY, 0, &[]), None);
    }

    #[test]
    fn the_floor_keeps_a_high_sec_bot_in_high_sec_and_the_gates_bound_the_reach() {
        let m = map();
        // A battleship wants 0.0-0.6, but a 0.45 floor keeps it from the
        // systems below 0.45: it takes the 0.5 one, three gates from 2.
        let got = target(&m, 2, Class::Battleship, 0.45, 0, &[]).expect("a target");
        assert_eq!(m.system_of(got), Some(4));
        // Without a floor the nearest system in 0.0-0.6 is the same one.
        // Too far is none: a chain longer than the reach.
        let mut long = Map::new();
        for i in 0..=JUMPS as i64 + 2 {
            let sec = if i == 0 { 0.95 } else { 0.5 };
            long = long
                .with_security(i + 1, sec)
                .with_station(7_000 + i, i + 1);
            if i > 0 {
                long = long.with_gate(i, 100 + i, i + 1, 200 + i).with_gate(
                    i + 1,
                    200 + i,
                    i,
                    100 + i,
                );
            }
        }
        // Everything past system 1 is in the cruiser's band and the nearest is
        // one gate away.
        assert!(target(&long, 1, Class::Cruiser, ANY_SECURITY, 0, &[]).is_some());
        // A map where the only match is past the reach has no target.
        let mut far = Map::new();
        for i in 1..=JUMPS as i64 + 2 {
            let sec = if i == JUMPS as i64 + 2 { 0.6 } else { 0.95 };
            far = far.with_security(i, sec).with_station(8_000 + i, i);
            if i > 1 {
                far =
                    far.with_gate(i - 1, 300 + i, i, 400 + i)
                        .with_gate(i, 400 + i, i - 1, 300 + i);
            }
        }
        assert_eq!(target(&far, 1, Class::Cruiser, ANY_SECURITY, 0, &[]), None);
    }

    #[test]
    fn a_system_to_avoid_is_not_chosen() {
        let m = map();
        let got = target(&m, 1, Class::Cruiser, ANY_SECURITY, 0, &[2, 3]).expect("a target");
        assert_eq!(m.system_of(got), Some(4));
    }
}
