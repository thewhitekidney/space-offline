//! **Geodes and motherlodes, as a bot meets them** (L7 BOT-ECON, issue #34;
//! spec `handback/r-nes-econ/report.md` §2.7). Pure rules and the bot's
//! memory of what it has heard; the executor is [`crate::nes_use`].
//!
//! - **A miner values a geode by its expected contents** ([`expected_value`])
//!   and, with one aboard, lets its `risk` trait decide whether to dock now
//!   ([`should_dock`]): a cautious pilot docks for a small geode, a bold one
//!   keeps cutting until the crate is worth the trip, and any human on the
//!   grid ends the argument. The crates are opened docked
//!   ([`crate::nes_use::turn`]); nothing is peeked.
//! - **A motherlode is heard, never seen from afar.** The server tells the
//!   pilots in the system (`OnServerMessage`, [`system_notice`]); a bot that
//!   gets it says one Local line ([`announce`], `motherlode::local_line`, the
//!   machine part `[ml:<system>]`), and every bot that hears such a line
//!   ([`Heard::hear`], `motherlode::heard`) remembers the system for
//!   [`FRESH_FOR`]. A miner then flies to one within [`reach`] jumps of its
//!   `risk`; a hunter adds the system and the gates to it to the ground it
//!   roams ([`ground_around`]). There is **no cargo peeking**: a hunter
//!   learns where the ore is, not what a hold carries.
//!
//! **Every number is [A]**, falsified by the soak: miners that dock with a
//! geode every time however bold (lower [`DOCK_BASE`]), or lose ships with
//! one aboard (raise it); hunters that never find a miner at a motherlode
//! (their reach is too short).

use crate::chatter::Line;
use crate::travel::Map;
use eve_mod_econ::{motherlode, tunables};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

/// ISK one prize is worth to a bot that knows nothing of what is inside **[A]**.
/// The loot table is authored in roles (PLEX, an injector, a SKIN licence, the
/// materials of an ore), so the figure is a typical prize and not one row.
/// Falsified by a geode that pays a tenth of this on the soak's ledger.
pub const REF_PRIZE_ISK: f64 = 2_000_000.0;

/// ISK of expected contents that make a pilot of `risk` 0.5 dock **[A]**.
pub const DOCK_BASE: f64 = 4_000_000.0;

/// How long a heard motherlode is worth flying to: the event's own length.
pub const FRESH_FOR: Duration = Duration::from_secs(tunables::ML_DURATION_S as u64);

/// Systems remembered at most.
const HEARD_CAP: usize = 16;

/// The most prizes a tier pays, and their size against a tier-1 prize
/// (`tunables::PRIZES_BY_TIER`, `QTY_BY_TIER`).
fn tier_index(tier: usize) -> usize {
    tier.clamp(1, tunables::PRIZES_BY_TIER.len()) - 1
}

/// **A geode's expected contents** in ISK: its prizes times how much bigger
/// each is at that tier, at [`REF_PRIZE_ISK`] a prize.
pub fn expected_value(tier: usize) -> f64 {
    let i = tier_index(tier);
    tunables::PRIZES_BY_TIER[i] as f64 * tunables::QTY_BY_TIER[i] * REF_PRIZE_ISK
}

/// What a `risk` in `[0, 1]` makes the pilot wait for before docking.
pub fn dock_threshold(risk: f64) -> f64 {
    DOCK_BASE * (0.25 + 1.5 * risk.clamp(0.0, 1.0))
}

/// **Dock now with these geodes aboard?** `value` is their summed
/// [`expected_value`]. Nothing aboard: no. A human outside the fleet on the
/// grid: yes, whatever the risk. Else when the contents reach the pilot's
/// [`dock_threshold`].
pub fn should_dock(risk: f64, value: f64, humans_near: bool) -> bool {
    if value <= 0.0 {
        return false;
    }
    humans_near || value >= dock_threshold(risk)
}

/// Jumps a pilot of `risk` will fly to a motherlode **[A]**: one for the
/// careful, four for the reckless.
pub fn reach(risk: f64) -> u32 {
    (1.0 + 3.0 * risk.clamp(0.0, 1.0)).round() as u32
}

/// The crate types that are geodes, by tier, and the SKIN tables, from the
/// content bundle (`mining/geodes.tsv`, `skins/*.tsv`). Empty without a
/// bundle: then no geode is ever recognised and no SKIN is bought.
#[derive(Debug, Clone, Default)]
pub struct Table {
    geodes: BTreeMap<i64, usize>,
    pub skins: eve_server::skins::Skins,
}

impl PartialEq for Table {
    /// Same geodes and the same number of SKIN licences: the policy only
    /// compares tables to tell a changed bundle from an unchanged one.
    fn eq(&self, other: &Table) -> bool {
        self.geodes == other.geodes
            && self.skins.item_types().len() == other.skins.item_types().len()
    }
}

impl Table {
    /// A table of `(type, tier)` rows and no SKINs (tests).
    pub fn with_geodes(rows: &[(i64, usize)]) -> Table {
        Table {
            geodes: rows.iter().copied().collect(),
            skins: eve_server::skins::Skins::default(),
        }
    }

    /// A table of SKIN rows and no geodes (tests).
    pub fn with_skins(skins: eve_server::skins::Skins) -> Table {
        Table {
            geodes: BTreeMap::new(),
            skins,
        }
    }

    /// The bundle's files; no directory is an empty table. Errors name the
    /// directory.
    pub fn open(content: Option<&Path>) -> Result<Table, String> {
        let Some(dir) = content else {
            return Ok(Table::default());
        };
        let (bundle, _) = eve_server::content::Bundle::open(dir)?;
        let at = |e: String| format!("--content {}: {e}", dir.display());
        let lines = bundle
            .data_lines(
                eve_mod_econ::geode::TYPES_FILE,
                eve_mod_econ::geode::TYPES_HEADER,
            )
            .map_err(at)?;
        let mut geodes = BTreeMap::new();
        for l in &lines {
            let mut f = l.split('\t');
            if let (Some(Ok(id)), Some(Ok(tier))) = (
                f.next().map(str::parse::<i64>),
                f.next().map(str::parse::<usize>),
            ) {
                geodes.insert(id, tier);
            }
        }
        // The server's own two SKIN files (`skins.rs`'s `LICENSES`, `SKINS`).
        let lic = bundle
            .data_lines(
                "skins/licenses.tsv",
                "#type_id\tgroup_id\tskin_id\tduration\tsingle_use\tvolume\tname",
            )
            .map_err(at)?;
        let skn = bundle
            .data_lines("skins/skins.tsv", "#skin_id\tvisible\tstructure\thulls")
            .map_err(at)?;
        let skins = eve_server::skins::Skins::parse(&lic, &skn).map_err(at)?;
        Ok(Table { geodes, skins })
    }

    /// The tier of crate type `type_id`, if it is a geode.
    pub fn tier_of(&self, type_id: i64) -> Option<usize> {
        self.geodes.get(&type_id).copied()
    }

    pub fn has_geodes(&self) -> bool {
        !self.geodes.is_empty()
    }
}

/// The summed [`expected_value`] of the geodes among `(type, quantity)` rows.
pub fn hold_value(table: &Table, rows: impl IntoIterator<Item = (i64, i64)>) -> f64 {
    rows.into_iter()
        .filter_map(|(t, q)| Some(expected_value(table.tier_of(t)?) * q.max(0) as f64))
        .sum()
}

/// **The motherlodes a bot has heard of**: system to when.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Heard {
    at: BTreeMap<i64, Instant>,
    /// Lines already read (the chat queue is re-shown each turn).
    seen: u64,
    /// Systems this bot already flew to, once each.
    went: BTreeSet<i64>,
}

impl Heard {
    /// Read the lines the bot heard: every new one carrying the machine part
    /// [`motherlode::heard`] reads is a system to remember.
    pub fn hear(&mut self, lines: &[Line], now: Instant) {
        let from = self.seen;
        for l in lines.iter().filter(|l| l.seq > from) {
            self.seen = self.seen.max(l.seq);
            if let Some(system) = motherlode::heard(&l.text) {
                self.at.insert(system, l.at.min(now));
            }
        }
        self.at
            .retain(|_, t| now.saturating_duration_since(*t) < FRESH_FOR);
        while self.at.len() > HEARD_CAP {
            let Some(oldest) = self.at.iter().min_by_key(|(_, t)| **t).map(|(s, _)| *s) else {
                break;
            };
            self.at.remove(&oldest);
        }
    }

    /// The systems heard of and not yet over, newest first.
    pub fn fresh(&self, now: Instant) -> Vec<i64> {
        let mut v: Vec<(Instant, i64)> = self
            .at
            .iter()
            .filter(|(_, t)| now.saturating_duration_since(**t) < FRESH_FOR)
            .map(|(s, t)| (*t, *s))
            .collect();
        v.sort_by(|a, b| b.cmp(a));
        v.into_iter().map(|(_, s)| s).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }

    /// **Where a docked miner flies next**: the nearest heard motherlode
    /// within the pilot's [`reach`], not the one it is in, not one it already
    /// went to. Marks it as gone to.
    pub fn to_motherlode(
        &mut self,
        map: &Map,
        from: i64,
        now: Instant,
        risk: f64,
        floor: f64,
    ) -> Option<i64> {
        let fresh: Vec<i64> = self
            .fresh(now)
            .into_iter()
            .filter(|s| !self.went.contains(s))
            .collect();
        let (system, _) = nearest(map, from, &fresh, reach(risk), floor)?;
        self.went.insert(system);
        Some(system)
    }
}

/// The systems a hunter adds to its ground for the motherlodes in `fresh`:
/// each one and the systems a gate leads from (the pirates and the miners
/// come in through them). Not "hunting ground" for the law's sake: only where
/// to roam to, so a hunter still never engages a pilot in high sec.
pub fn ground_around(map: &Map, fresh: &[i64]) -> BTreeSet<i64> {
    let mut out = BTreeSet::new();
    for &s in fresh {
        out.insert(s);
        out.extend(map.neighbours(s));
    }
    out
}

/// The nearest heard motherlode within `reach` jumps of `from`, with its
/// distance, at `floor` security.
pub fn nearest(map: &Map, from: i64, fresh: &[i64], reach: u32, floor: f64) -> Option<(i64, u32)> {
    fresh
        .iter()
        .filter(|&&s| s != from)
        .filter_map(|&s| Some((map.jumps(from, s, floor)?, s)))
        .filter(|(j, _)| *j <= reach)
        .min()
        .map(|(j, s)| (s, j))
}

// ---- the server's message, and the line a foreman says --------------------------

/// The system name in the server's motherlode message
/// (`motherlode::message_text`), if `text` is one.
pub fn system_notice(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("Motherlode! One of the belts in ")?;
    let end = rest.find(" is unusually rich")?;
    Some(&rest[..end])
}

/// What a bot that got the server's message remembers to say, once per
/// system: `(system, name)`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Told {
    pub pending: Option<(i64, String)>,
    said: BTreeMap<i64, Instant>,
}

impl Told {
    /// The message arrived while the bot was in `system`.
    pub fn note(&mut self, text: &str, system: i64, now: Instant) {
        if let Some(name) = system_notice(text)
            && self
                .said
                .get(&system)
                .is_none_or(|t| now.saturating_duration_since(*t) >= FRESH_FOR)
        {
            self.pending = Some((system, name.to_owned()));
        }
    }
}

/// **The foreman's announcement** (`motherlode::local_line`): once per
/// motherlode, and only from a foreman, who is the pilot a fleet listens to.
/// The line is returned and the system marked as said.
pub fn announce(told: &mut Told, foreman: bool, now: Instant) -> Option<String> {
    if !foreman {
        return None;
    }
    let (system, name) = told.pending.take()?;
    told.said.insert(system, now);
    Some(motherlode::local_line(&name, system, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::RoomKind;

    fn line(seq: u64, text: &str, at: Instant) -> Line {
        Line {
            seq,
            room: RoomKind::Local,
            from: 7,
            text: text.to_owned(),
            at,
            human: false,
        }
    }

    #[test]
    fn a_bigger_geode_is_worth_more_and_an_unknown_tier_is_clamped() {
        assert!(expected_value(2) > expected_value(1));
        assert!(expected_value(3) > expected_value(2));
        assert_eq!(expected_value(9), expected_value(3));
        assert_eq!(expected_value(0), expected_value(1));
    }

    #[test]
    fn the_risk_trait_decides_when_a_geode_is_worth_the_trip() {
        let small = expected_value(1);
        assert!(
            should_dock(0.0, small, false),
            "the careful dock for the smallest"
        );
        assert!(
            !should_dock(1.0, small, false),
            "the reckless cut on for a small one"
        );
        assert!(should_dock(1.0, expected_value(3), false));
        assert!(
            !should_dock(0.5, 0.0, true),
            "nothing aboard, nothing to protect"
        );
        assert!(should_dock(1.0, small, true), "a human on the grid ends it");
        assert!(dock_threshold(0.0) < dock_threshold(0.5));
        assert!(dock_threshold(0.5) < dock_threshold(1.0));
    }

    #[test]
    fn a_bolder_pilot_flies_farther_to_a_motherlode() {
        assert_eq!(reach(0.0), 1);
        assert_eq!(reach(1.0), 4);
        assert!(reach(0.2) <= reach(0.8));
        assert_eq!(reach(7.0), 4, "clamped");
    }

    #[test]
    fn the_hold_is_valued_by_the_geodes_in_it_and_nothing_else() {
        let t = Table::with_geodes(&[(900, 1), (901, 3)]);
        let rows = [(900, 2), (901, 1), (5, 40)];
        let want = 2.0 * expected_value(1) + expected_value(3);
        assert!((hold_value(&t, rows) - want).abs() < 1e-6);
        assert_eq!(hold_value(&Table::default(), rows), 0.0);
    }

    #[test]
    fn a_motherlode_line_is_remembered_once_and_forgotten_when_it_is_over() {
        let now = Instant::now();
        let said = motherlode::local_line("Testia", 31, false);
        let echo = motherlode::local_line("Echoia", 32, true);
        let mut h = Heard::default();
        h.hear(
            &[line(1, "o7", now), line(2, &said, now), line(3, &echo, now)],
            now,
        );
        assert_eq!(h.fresh(now).len(), 2);
        assert!(h.fresh(now).contains(&31) && h.fresh(now).contains(&32));
        // The same lines are not read twice.
        h.hear(&[line(2, &said, now)], now);
        assert_eq!(h.fresh(now).len(), 2);
        // Over after the event's length.
        let later = now + FRESH_FOR + Duration::from_secs(1);
        assert!(h.fresh(later).is_empty());
        h.hear(&[], later);
        assert!(h.is_empty());
    }

    /// 1 - 2 - 3 - 4 - 5 in a line, all high sec, all with a station.
    fn chain() -> Map {
        let mut m = Map::new();
        for i in 1..5i64 {
            let (a, b) = (3_000_000 + i, 3_000_000 + i + 1);
            m = m
                .with_gate(a, 50_000 + a, b, 50_000 + b)
                .with_gate(b, 50_000 + b, a, 50_000 + a);
        }
        for i in 1..=5i64 {
            m = m
                .with_security(3_000_000 + i, 0.9)
                .with_station(60_000 + i, 3_000_000 + i);
        }
        m
    }

    #[test]
    fn a_hunter_roams_to_the_motherlode_and_the_systems_a_gate_away() {
        let m = chain();
        let g = ground_around(&m, &[3_000_003]);
        assert_eq!(
            g.into_iter().collect::<Vec<_>>(),
            vec![3_000_002, 3_000_003, 3_000_004]
        );
        assert!(ground_around(&m, &[]).is_empty());
    }

    #[test]
    fn a_miner_flies_to_a_motherlode_only_within_its_reach_and_only_once() {
        let m = chain();
        let now = Instant::now();
        let said = motherlode::local_line("Far", 3_000_005, false);
        let mut h = Heard::default();
        h.hear(&[line(1, &said, now)], now);
        let home = 3_000_001;
        // Four jumps away: only the reckless go.
        assert_eq!(h.to_motherlode(&m, home, now, 0.1, 0.5), None);
        assert_eq!(h.to_motherlode(&m, home, now, 1.0, 0.5), Some(3_000_005));
        assert_eq!(
            h.to_motherlode(&m, home, now, 1.0, 0.5),
            None,
            "it has been"
        );
        // Not to the system it is already in.
        let mut h = Heard::default();
        h.hear(&[line(1, &said, now)], now);
        assert_eq!(h.to_motherlode(&m, 3_000_005, now, 1.0, 0.5), None);
    }

    #[test]
    fn a_line_that_is_not_ours_is_not_a_motherlode() {
        let now = Instant::now();
        let mut h = Heard::default();
        h.hear(&[line(1, "mining in [ml:oops]", now)], now);
        assert!(h.is_empty());
    }

    #[test]
    fn the_server_message_gives_the_system_name() {
        let text = motherlode::message_text("Quietia Prime");
        assert_eq!(system_notice(&text), Some("Quietia Prime"));
        assert_eq!(system_notice("Welcome to the cluster"), None);
    }

    #[test]
    fn only_a_foreman_announces_and_only_once_per_motherlode() {
        let now = Instant::now();
        let text = motherlode::message_text("Testia");
        let mut t = Told::default();
        t.note(&text, 31, now);
        assert_eq!(announce(&mut t, false, now), None, "a miner stays quiet");
        let line = announce(&mut t, true, now).expect("the foreman speaks");
        assert_eq!(motherlode::heard(&line), Some(31));
        assert_eq!(announce(&mut t, true, now), None);
        // The same event does not make a second announcement.
        t.note(&text, 31, now + Duration::from_secs(60));
        assert_eq!(announce(&mut t, true, now), None);
        // A later one does.
        t.note(&text, 31, now + FRESH_FOR + Duration::from_secs(1));
        assert!(announce(&mut t, true, now + FRESH_FOR).is_some());
    }

    #[test]
    fn a_foreman_who_was_not_told_says_nothing() {
        assert_eq!(announce(&mut Told::default(), true, Instant::now()), None);
    }
}
