//! **The society's rules** (R-SOCIETY §3.2-3.3, SOC-C4): relations between
//! groups, the blue and red stances they turn into, who founds, joins and
//! leaves a corp, and when corps found and join alliances. **Everything here
//! is pure**: no socket, no clock. [`crate::corpmind`] feeds it what the bots
//! observed and acts on what it says through the CEOs' own connections.
//!
//! - **A group** ([`Group`]) is an owner id: an alliance when the pilot's
//!   corp is in one, else the player corp, else the pilot (a human in an NPC
//!   corp is a group of one) ([`group_of`]).
//! - **The relation matrix** ([`Relations`]): `R[viewer][target]` in
//!   `[-R_MAX, R_MAX]`, the viewer a bot corp, the target any group, each
//!   side holding its own view. [`Event`]s move it; time decays it toward 0
//!   with a half-life that grows with the viewer's loyalty; two derived
//!   daily rules (a common enemy, a hegemon) move it too.
//! - **Stances** ([`Stances`]): blue at `R >= ALLY_AT`, left below
//!   `ALLY_AT - BAND`; red at `R <= ENEMY_AT`, left above `ENEMY_AT + BAND`;
//!   and **no pair changes stance more than once per [`FLIP_MIN_DAYS`]**
//!   game-days, so nobody flaps blue/red within a day (gate GC).
//! - **Allied** ([`allied`], [`hostile_to`]): the server's own pure rule,
//!   `eve_server::group_contacts::allied` (same player corp, same alliance,
//!   or a positive entry in the viewer's corp or alliance book for the
//!   other's corp or alliance). SOC-C5 hunts by `hostile_to`.
//! - **Corps** (§3.2): [`wants_to_found`], [`wants_to_join`], [`fit`],
//!   [`best_corp`], [`ceo_accepts`], and the member's [`Satisfaction`].
//! - **Alliances** (§3.3): [`alliance_acts`]: two corps mutually blue for
//!   [`ALLY_DAYS`] merge (the larger or more aggressive founds, the other
//!   applies), at most [`MAX_ALLIANCES`]; a member corp whose stance
//!   toward someone is the opposite of its alliance's leaves.
//! - **The corp planner's quotas** (§3.4): [`quota`] and [`corp_need`].
//!
//! Every number below is **[A]**, each with its falsifier; no test holds one
//! (the tests name the consts).

use crate::career::Band;
use crate::mind::{Trait, Traits};
use std::collections::{BTreeMap, BTreeSet};

pub use eve_server::group_contacts::Affiliation;

/// A group: an alliance, a player corp, or a pilot's own id.
pub type Group = i64;

/// `minPlayerCorporation ..= maxPlayerCorporation`
/// (`inventorycommon/const.py`, `docs/lanes/corp.md` §1) **[V]**.
pub const PLAYER_CORPS: std::ops::RangeInclusive<i64> = 98_000_000..=98_999_999;
/// `minPlayerAlliance ..= maxPlayerAlliance` (`docs/lanes/soc-c1.md` §2)
/// **[V]**.
pub const PLAYER_ALLIANCES: std::ops::RangeInclusive<i64> = 99_000_000..=99_999_999;

/// Is `corp` a player corporation (not an NPC one)?
pub fn is_player_corp(corp: i64) -> bool {
    PLAYER_CORPS.contains(&corp)
}

/// **The group a pilot counts as**: the alliance, else the player corp,
/// else the pilot.
pub fn group_of(a: &Affiliation) -> Group {
    match a.alliance_id {
        Some(al) => al,
        None if is_player_corp(a.corporation_id) => a.corporation_id,
        None => a.character_id,
    }
}

// ---- the relation's constants (all [A]) ------------------------------------

/// The matrix's bound.
pub const R_MAX: f64 = 10.0;
/// A member of the viewer killed by a member of the target, at value factor
/// 1 **[A]**. Falsified with [`ENEMY_AT`]: one lost frigate turning a
/// neighbour red (too high), or a dozen losses leaving it neutral (too
/// low).
pub const K_KILL: f64 = 3.0;
/// The ISK lost that makes a kill count once (`value_factor` 1) **[A]**.
pub const KILL_VALUE_REF: f64 = 10_000_000.0;
/// The value factor's floor and ceiling **[A]**: a pod still hurts, a
/// battleship does not count as ten frigates.
pub const VALUE_FACTOR_MIN: f64 = 0.5;
pub const VALUE_FACTOR_MAX: f64 = 2.0;
/// The target attacked the viewer's sov structure (stage D) **[A]**.
pub const K_SIEGE: f64 = 4.0;
/// Per day per common enemy, toward each other **[A]**. Falsified by
/// `[soc]` showing two groups at war with the same third never turning
/// blue in a week.
pub const K_SHARED: f64 = 1.0;
/// Per day while two groups contest the same systems (stage D) **[A]**.
pub const K_CONTEST: f64 = 1.0;
/// Per ISK of trade between the groups, and the most one trade adds
/// **[A]**.
pub const K_TRADE: f64 = 1e-7;
pub const K_TRADE_CAP: f64 = 1.0;
/// Fought on the same side **[A]**.
pub const K_ALLY_FIGHT: f64 = 1.0;
/// **Neighbours** (this lane's addition, **[A]**): per day, toward each
/// other, between two bot corps of the same profile (kind and band). Without
/// it no blue ever forms before stage C5's fights and D's sov give the
/// other sources something to count. Falsified by the owner finding the
/// blues arbitrary, or by every same-profile corp merging into one bloc.
pub const K_NEIGHBOUR: f64 = 1.0;
/// DOC-4 (R-DOCTRINE 7.4): a blue came when asked, to both views, at value
/// factor 1 **[A]**. Falsified by blues that never learn who helps (too
/// small) or fuse after one fight (too big).
pub const K_REINFORCE: f64 = 1.5;
/// DOC-4: an ally that could help and did not come within the wait, in the
/// asker's view only **[A]**. Falsified by blues that shatter after one
/// non-appearance (too big) or never learn who stays away (too small).
pub const K_ABANDON: f64 = 2.0;
/// DOC-4: a blue shot a blue (friendly fire), in the victim's view **[A]**.
pub const K_BETRAY: f64 = 4.0;
/// The half-life of a relation, in days, at loyalty 0.5 (scaled by
/// `0.5 + loyalty`) **[A]**. Falsified by grudges that outlive a war by
/// weeks, or blues that fade between two play evenings.
pub const HALF_LIFE_DAYS: f64 = 7.0;
/// Below this a relation is forgotten.
pub const R_EPSILON: f64 = 0.01;

/// Blue at or above **[A]**.
pub const ALLY_AT: f64 = 5.0;
/// Red at or below **[A]**.
pub const ENEMY_AT: f64 = -5.0;
/// The hysteresis band: blue is left below `ALLY_AT - BAND`, red above
/// `ENEMY_AT + BAND` **[A]**.
pub const BAND: f64 = 2.0;
/// The least time between two stance changes of one pair, in game-days
/// **[A]**: gate GC's "nobody flaps blue/red more than once a day".
pub const FLIP_MIN_DAYS: f64 = 1.0;
/// The contact a blue or a red stance sets, at the real corp or alliance
/// contact calls **[A]** (the client's own steps are -10, -5, 0, 5, 10).
pub const BLUE_STANDING: f64 = 10.0;
pub const RED_STANDING: f64 = -10.0;

/// Above this share of the bot society's members, a group is a hegemon
/// **[A]**. Falsified by `[soc]` showing a permanent two-bloc stalemate
/// (tune), or one alliance holding everybody (lower).
pub const HEGEMON_SHARE: f64 = 0.5;
/// Per day toward a hegemon, from every other bot corp **[A]**.
pub const K_HEGEMON: f64 = 1.0;
/// Two corps mutually blue this many days merge into an alliance **[A]**.
pub const ALLY_DAYS: f64 = 3.0;
/// The most bot alliances **[A]** (R-SOCIETY §3.3: about 3-4 for 50 bots).
pub const MAX_ALLIANCES: usize = 4;

/// One thing that happened between two groups.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A member of `victim` lost a ship or pod worth `isk` to a member of
    /// `killer`. Moves the victim's view only.
    Kill {
        victim: Group,
        killer: Group,
        isk: f64,
    },
    /// `attacker` attacked `defender`'s sov structure (stage D).
    Siege { defender: Group, attacker: Group },
    /// Trade fills between the two, worth `isk`.
    Trade { a: Group, b: Group, isk: f64 },
    /// The two fought on the same side.
    AllyFight { a: Group, b: Group },
    /// The two contested the same systems for `days` (stage D).
    Contest { a: Group, b: Group, days: f64 },
    /// The two are neighbours ([`K_NEIGHBOUR`]) for `days`.
    Neighbour { a: Group, b: Group, days: f64 },
    /// DOC-4: `helper` answered `helped`'s call and came; `isk_saved` is the
    /// value of what the help kept alive (0: unknown). Moves both views.
    Reinforced {
        helper: Group,
        helped: Group,
        isk_saved: f64,
    },
    /// DOC-4: `ally` had the means and the standing and did not answer
    /// `asker`'s call within the wait. Moves the asker's view only.
    Abandoned { asker: Group, ally: Group },
    /// DOC-4: `betrayer` shot a ship of `victim`'s while blue. Moves the
    /// victim's view only.
    Betrayed { betrayer: Group, victim: Group },
}

/// `isk / KILL_VALUE_REF` inside its floor and ceiling; 1 for an unknown
/// value.
pub fn value_factor(isk: f64) -> f64 {
    if !isk.is_finite() || isk <= 0.0 {
        return 1.0;
    }
    (isk / KILL_VALUE_REF).clamp(VALUE_FACTOR_MIN, VALUE_FACTOR_MAX)
}

/// **The relation matrix**: `(viewer, target) -> R`. Absent is 0.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Relations {
    pub r: BTreeMap<(Group, Group), f64>,
    /// When [`Relations::decay`] last ran (game seconds); `None` before.
    pub at: Option<u64>,
}

impl Relations {
    /// `R[viewer][target]`.
    pub fn get(&self, viewer: Group, target: Group) -> f64 {
        self.r.get(&(viewer, target)).copied().unwrap_or(0.0)
    }

    /// Move `R[viewer][target]` by `d`, inside the bound. A group has no
    /// relation to itself.
    pub fn add(&mut self, viewer: Group, target: Group, d: f64) {
        if viewer == target || !d.is_finite() {
            return;
        }
        let v = (self.get(viewer, target) + d).clamp(-R_MAX, R_MAX);
        self.r.insert((viewer, target), v);
    }

    /// Fold one event in.
    pub fn apply(&mut self, e: &Event) {
        match *e {
            Event::Kill {
                victim,
                killer,
                isk,
            } => self.add(victim, killer, -K_KILL * value_factor(isk)),
            Event::Siege { defender, attacker } => self.add(defender, attacker, -K_SIEGE),
            Event::Trade { a, b, isk } => {
                let d = (K_TRADE * isk.max(0.0)).min(K_TRADE_CAP);
                self.add(a, b, d);
                self.add(b, a, d);
            }
            Event::AllyFight { a, b } => {
                self.add(a, b, K_ALLY_FIGHT);
                self.add(b, a, K_ALLY_FIGHT);
            }
            Event::Contest { a, b, days } => {
                self.add(a, b, -K_CONTEST * days.max(0.0));
                self.add(b, a, -K_CONTEST * days.max(0.0));
            }
            Event::Neighbour { a, b, days } => {
                self.add(a, b, K_NEIGHBOUR * days.max(0.0));
                self.add(b, a, K_NEIGHBOUR * days.max(0.0));
            }
            Event::Reinforced {
                helper,
                helped,
                isk_saved,
            } => {
                let d = K_REINFORCE * value_factor(isk_saved);
                self.add(helper, helped, d);
                self.add(helped, helper, d);
            }
            Event::Abandoned { asker, ally } => self.add(asker, ally, -K_ABANDON),
            Event::Betrayed { betrayer, victim } => self.add(victim, betrayer, -K_BETRAY),
        }
    }

    /// **Decay toward 0** from the last call to `now`: each viewer's rows by
    /// half-life `HALF_LIFE_DAYS x (0.5 + loyalty(viewer))` days of `day`
    /// seconds. Rows under [`R_EPSILON`] are forgotten. The first call only
    /// starts the clock.
    pub fn decay(&mut self, now: u64, day: u64, loyalty: impl Fn(Group) -> f64) {
        self.decay_with(now, day, |g| {
            HALF_LIFE_DAYS * (0.5 + loyalty(g).clamp(0.0, 1.0))
        });
    }

    /// [`Relations::decay`] with the half-life in days given per viewer
    /// (R-REL-1: the [`Temper`](crate::relations::Temper) scales it).
    pub fn decay_with(&mut self, now: u64, day: u64, half_days: impl Fn(Group) -> f64) {
        let Some(at) = self.at else {
            self.at = Some(now);
            return;
        };
        if now <= at {
            return;
        }
        self.at = Some(now);
        let days = (now - at) as f64 / day.max(1) as f64;
        self.r.retain(|(viewer, _), v| {
            let half = half_days(*viewer).max(f64::MIN_POSITIVE);
            *v *= 0.5f64.powf(days / half);
            v.abs() >= R_EPSILON
        });
    }

    /// **A common enemy** (daily): two viewers both at or under
    /// [`ENEMY_AT`] toward the same third group move toward each other's
    /// group by [`K_SHARED`] per day per such enemy. `group_of` maps a
    /// viewer corp to the group others see it as.
    pub fn shared_enemies(
        &mut self,
        viewers: &[Group],
        group_of: impl Fn(Group) -> Group,
        days: f64,
    ) {
        let mut moves = Vec::new();
        for (i, &g) in viewers.iter().enumerate() {
            for &h in &viewers[i + 1..] {
                let (gg, hg) = (group_of(g), group_of(h));
                if gg == hg {
                    continue;
                }
                let common = self
                    .r
                    .iter()
                    .filter(|((v, x), r)| *v == g && **r <= ENEMY_AT && *x != gg && *x != hg)
                    .filter(|((_, x), _)| self.get(h, *x) <= ENEMY_AT)
                    .count();
                if common > 0 {
                    let d = K_SHARED * days * common as f64;
                    moves.push((g, hg, d));
                    moves.push((h, gg, d));
                }
            }
        }
        for (v, t, d) in moves {
            self.add(v, t, d);
        }
    }

    /// **The balance of power** (daily): every viewer outside a group whose
    /// share of members is over [`HEGEMON_SHARE`] moves away from it by
    /// [`K_HEGEMON`] per day.
    pub fn hegemon(
        &mut self,
        viewers: &[Group],
        shares: &BTreeMap<Group, f64>,
        group_of: impl Fn(Group) -> Group,
        days: f64,
    ) {
        for (&big, &share) in shares {
            if share <= HEGEMON_SHARE {
                continue;
            }
            for &v in viewers {
                if group_of(v) != big {
                    self.add(v, big, -K_HEGEMON * days);
                }
            }
        }
    }
}

// ---- stances --------------------------------------------------------------------

/// What a relation reads as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stance {
    Neutral,
    Blue,
    Red,
}

impl Stance {
    pub fn name(self) -> &'static str {
        match self {
            Stance::Neutral => "neutral",
            Stance::Blue => "blue",
            Stance::Red => "red",
        }
    }

    /// The contact this stance sets; `None`: no contact.
    pub fn standing(self) -> Option<f64> {
        match self {
            Stance::Neutral => None,
            Stance::Blue => Some(BLUE_STANDING),
            Stance::Red => Some(RED_STANDING),
        }
    }

    /// Blue against red, either way round.
    pub fn opposes(self, other: Stance) -> bool {
        matches!(
            (self, other),
            (Stance::Blue, Stance::Red) | (Stance::Red, Stance::Blue)
        )
    }
}

/// **The threshold rule with its band** (pure): what `r` reads as for a
/// pair whose stance is `cur`.
pub fn next_stance(cur: Stance, r: f64) -> Stance {
    match cur {
        Stance::Blue if r >= ALLY_AT - BAND => Stance::Blue,
        Stance::Red if r <= ENEMY_AT + BAND => Stance::Red,
        _ if r >= ALLY_AT => Stance::Blue,
        _ if r <= ENEMY_AT => Stance::Red,
        _ => Stance::Neutral,
    }
}

// ---- five stances (R-REL-1, R-RELATIONS §3.1) ---------------------------------------

/// Friendly at or above **[A]** (half a threshold). Falsified by >40 % of
/// pairs sitting in Friendly/Wary and never reaching Allied/Hostile.
pub const FRIEND_AT: f64 = 2.5;
/// Wary at or below **[A]**.
pub const WARY_AT: f64 = -2.5;
/// Friendly is left below this, Wary above its mirror **[A]**.
pub const FRIEND_LEAVE: f64 = 1.0;
pub const WARY_LEAVE: f64 = -1.0;

/// The graded stance (the names are internal; the wire is the contact).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stance5 {
    Hostile,
    Wary,
    Neutral,
    Friendly,
    Allied,
}

impl Stance5 {
    /// The name the state file keeps.
    pub fn name(self) -> &'static str {
        match self {
            Stance5::Hostile => "hostile",
            Stance5::Wary => "wary",
            Stance5::Neutral => "neutral",
            Stance5::Friendly => "friendly",
            Stance5::Allied => "allied",
        }
    }

    /// [`Stance5::name`] read back.
    pub fn parse(s: &str) -> Option<Stance5> {
        [
            Stance5::Hostile,
            Stance5::Wary,
            Stance5::Neutral,
            Stance5::Friendly,
            Stance5::Allied,
        ]
        .into_iter()
        .find(|x| x.name() == s)
    }

    /// The contact this stance sets (`None`: none); +-10 and +-5.
    pub fn standing(self) -> Option<f64> {
        match self {
            Stance5::Allied => Some(BLUE_STANDING),
            Stance5::Friendly => Some(5.0),
            Stance5::Neutral => None,
            Stance5::Wary => Some(-5.0),
            Stance5::Hostile => Some(RED_STANDING),
        }
    }

    /// Does `allied()` hold (any contact above 0)?
    pub fn is_allied(self) -> bool {
        self >= Stance5::Friendly
    }

    /// The old three-way reading.
    pub fn coarse(self) -> Stance {
        match self {
            Stance5::Allied => Stance::Blue,
            Stance5::Hostile => Stance::Red,
            _ => Stance::Neutral,
        }
    }
}

/// **The five-way threshold rule with its bands** (pure), at the normal
/// temper.
pub fn next_stance5(cur: Stance5, r: f64) -> Stance5 {
    next_stance5_with(cur, r, &crate::relations::Temper::NORMAL)
}

/// [`next_stance5`] with the temper's `hunt_bias` shifting the Wary and
/// Hostile thresholds toward 0 (and their leave points with them).
pub fn next_stance5_with(cur: Stance5, r: f64, t: &crate::relations::Temper) -> Stance5 {
    let b = t.hunt_bias;
    let (enemy, wary) = (ENEMY_AT + b, WARY_AT + b);
    let (enemy_leave, wary_leave) = (ENEMY_AT + BAND + b, WARY_LEAVE + b);
    match cur {
        Stance5::Allied if r >= ALLY_AT - BAND => return Stance5::Allied,
        Stance5::Friendly if (FRIEND_LEAVE..ALLY_AT).contains(&r) => return Stance5::Friendly,
        Stance5::Wary if r <= wary_leave && r > enemy => return Stance5::Wary,
        Stance5::Hostile if r <= enemy_leave => return Stance5::Hostile,
        _ => {}
    }
    if r >= ALLY_AT {
        Stance5::Allied
    } else if r >= FRIEND_AT {
        Stance5::Friendly
    } else if r <= enemy {
        Stance5::Hostile
    } else if r <= wary {
        Stance5::Wary
    } else {
        Stance5::Neutral
    }
}

/// One pair's stance and when it last changed (`None`: never).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub stance: Stance,
    pub since: Option<u64>,
}

/// One pair's graded stance and when it last changed (R-REL-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held5 {
    pub stance: Stance5,
    pub since: Option<u64>,
}

/// **Every owner's stance toward every target**, with the once-a-day rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stances {
    pub held: BTreeMap<(Group, Group), Held>,
    /// R-REL-3: the graded reading of the same pairs (what the contact
    /// books publish: +-10 and +-5). Where a pair has no row here, the
    /// coarse `held` reading stands.
    pub graded: BTreeMap<(Group, Group), Held5>,
    /// Pairs that were betrayed, and when (R-REL-1).
    pub betrayed: BTreeMap<(Group, Group), u64>,
}

impl Stances {
    pub fn get(&self, owner: Group, target: Group) -> Stance {
        self.held
            .get(&(owner, target))
            .map_or(Stance::Neutral, |h| h.stance)
    }

    /// When `owner` turned to its present stance toward `target`.
    pub fn since(&self, owner: Group, target: Group) -> Option<u64> {
        self.held.get(&(owner, target)).and_then(|h| h.since)
    }

    /// **Settle one pair** on relation `r` at `now`: the band rule, but no
    /// change within [`FLIP_MIN_DAYS`] of the last one. `Some((from, to))`
    /// when it changed.
    pub fn settle(
        &mut self,
        owner: Group,
        target: Group,
        r: f64,
        now: u64,
        day: u64,
    ) -> Option<(Stance, Stance)> {
        self.settle_with(owner, target, r, now, day, FLIP_MIN_DAYS)
    }

    /// [`Stances::settle`] with the flip window in game-days given (the
    /// temper's).
    pub fn settle_with(
        &mut self,
        owner: Group,
        target: Group,
        r: f64,
        now: u64,
        day: u64,
        flip_min_days: f64,
    ) -> Option<(Stance, Stance)> {
        let held = self.held.get(&(owner, target)).copied().unwrap_or(Held {
            stance: Stance::Neutral,
            since: None,
        });
        let to = next_stance(held.stance, r);
        if to == held.stance {
            return None;
        }
        let min = (flip_min_days * day as f64) as u64;
        if held.since.is_some_and(|s| now.saturating_sub(s) < min) {
            return None;
        }
        if to == Stance::Neutral && held.since.is_none() {
            return None;
        }
        self.held.insert(
            (owner, target),
            Held {
                stance: to,
                since: Some(now),
            },
        );
        Some((held.stance, to))
    }

    /// The graded stance of a pair: its graded row, else the coarse
    /// reading (blue is allied, red hostile).
    pub fn get5(&self, owner: Group, target: Group) -> Stance5 {
        match self.graded.get(&(owner, target)) {
            Some(h) => h.stance,
            None => match self.get(owner, target) {
                Stance::Blue => Stance5::Allied,
                Stance::Red => Stance5::Hostile,
                Stance::Neutral => Stance5::Neutral,
            },
        }
    }

    /// **Settle one pair's graded stance** on `r`: the five-way band rule
    /// ([`next_stance5_with`]) and the same flip window as [`Stances::settle`].
    /// `Some((from, to))` when it changed.
    #[allow(clippy::too_many_arguments)] // the read's whole context, flat for the caller
    pub fn settle5_with(
        &mut self,
        owner: Group,
        target: Group,
        r: f64,
        now: u64,
        day: u64,
        flip_min_days: f64,
        temper: &crate::relations::Temper,
    ) -> Option<(Stance5, Stance5)> {
        let held = self.graded.get(&(owner, target)).copied().unwrap_or(Held5 {
            stance: self.get5(owner, target),
            since: None,
        });
        let to = next_stance5_with(held.stance, r, temper);
        if to == held.stance {
            return None;
        }
        let min = (flip_min_days * day as f64) as u64;
        if held.since.is_some_and(|s| now.saturating_sub(s) < min) {
            return None;
        }
        self.graded.insert(
            (owner, target),
            Held5 {
                stance: to,
                since: Some(now),
            },
        );
        Some((held.stance, to))
    }

    /// **Set a stance now**, bypassing the flip window (a betrayal or a war
    /// declaration, R-RELATIONS §2.5). `Some((from, to))` when it changed.
    pub fn settle_forced(
        &mut self,
        owner: Group,
        target: Group,
        to: Stance,
        now: u64,
    ) -> Option<(Stance, Stance)> {
        let from = self.get(owner, target);
        if from == to {
            return None;
        }
        self.held.insert(
            (owner, target),
            Held {
                stance: to,
                since: Some(now),
            },
        );
        Some((from, to))
    }

    /// Mark the pair betrayed at `now`.
    pub fn mark_betrayed(&mut self, owner: Group, target: Group, now: u64) {
        self.betrayed.insert((owner, target), now);
    }

    /// When `owner` was last betrayed by `target`.
    pub fn betrayed_at(&self, owner: Group, target: Group) -> Option<u64> {
        self.betrayed.get(&(owner, target)).copied()
    }

    /// **Forget settled pairs**: drop every pair whose stance is neutral,
    /// or whose owner is gone (`live(owner)` false), once its last change
    /// is older than [`FLIP_MIN_DAYS`] -- so the once-per-window rule still
    /// holds for the pairs kept. Returns how many were dropped.
    pub fn prune(&mut self, now: u64, day: u64, live: impl Fn(Group) -> bool) -> usize {
        let min = (FLIP_MIN_DAYS * day as f64) as u64;
        let before = self.held.len();
        self.held.retain(|&(o, _), h| {
            let settled = h.since.is_none_or(|s| now.saturating_sub(s) >= min);
            !(settled && (h.stance == Stance::Neutral || !live(o)))
        });
        let held = &self.held;
        self.graded.retain(|&(o, t), h| {
            let settled = h.since.is_none_or(|s| now.saturating_sub(s) >= min);
            let coarse_none = held
                .get(&(o, t))
                .is_none_or(|c| c.stance == Stance::Neutral);
            !(settled && ((h.stance == Stance5::Neutral && coarse_none) || !live(o)))
        });
        before - self.held.len()
    }

    /// The pairs at `stance`.
    pub fn count(&self, stance: Stance) -> usize {
        self.held.values().filter(|h| h.stance == stance).count()
    }
}

// ---- allied ----------------------------------------------------------------------

/// **Whether `me` counts `other` as a friend** -- the server's own pure
/// rule (`eve_server::group_contacts::allied`, SOC-C2), over the group
/// books `standing(owner, target)` answers.
pub fn allied(
    me: Affiliation,
    other: Affiliation,
    standing: impl Fn(i64, i64) -> Option<f64>,
) -> bool {
    eve_server::group_contacts::allied(me, other, standing)
}

/// [`allied`] with the viewer's personal contacts (`personal(character id)`),
/// REL-FIX: a friend on the `--friends` list is allied.
pub fn allied_with(
    me: Affiliation,
    other: Affiliation,
    standing: impl Fn(i64, i64) -> Option<f64>,
    personal: impl Fn(i64) -> Option<f64>,
) -> bool {
    eve_server::group_contacts::allied_with(me, other, standing, personal)
}

/// [`hostile_to`] with the viewer's personal contacts.
pub fn hostile_to_with(
    me: Affiliation,
    other: Affiliation,
    standing: impl Fn(i64, i64) -> Option<f64>,
    personal: impl Fn(i64) -> Option<f64>,
) -> bool {
    !allied_with(me, other, standing, personal)
}

/// **Whom a bot may hunt** (owner, tracker §84): anyone not [`allied`],
/// humans included. Where (low, null, wormhole; high-sec safe until wars)
/// is SOC-C5's rule.
pub fn hostile_to(
    me: Affiliation,
    other: Affiliation,
    standing: impl Fn(i64, i64) -> Option<f64>,
) -> bool {
    !allied(me, other, standing)
}

// ---- corps (§3.2) ----------------------------------------------------------------

/// A career's family, for profiles and quotas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    Gather,
    Build,
    Haul,
    Rat,
    Fight,
    Explore,
}

/// The family of a career in `career::CAREERS` **[A]**; `None` for a name
/// not in the table.
pub fn family_of(career: &str) -> Option<Family> {
    Some(match career {
        "miner" | "ice_miner" | "gas_huffer" | "pi_farmer" => Family::Gather,
        "builder" | "reactor" => Family::Build,
        "trader" => Family::Haul,
        "ratter" | "site_runner" | "mission_runner" | "wormhole_raider" => Family::Rat,
        "pirate" | "fw_plexer" | "soldier" | "sov_grunt" => Family::Fight,
        "explorer" | "wormhole_diver" => Family::Explore,
        _ => return None,
    })
}

/// What kind of corp a corp is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Industry,
    Combat,
    Explore,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Industry => "industry",
            Kind::Combat => "combat",
            Kind::Explore => "explore",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        [Kind::Industry, Kind::Combat, Kind::Explore]
            .into_iter()
            .find(|k| k.name() == s)
    }
}

/// A corp's profile: the founder's career family's kind and home band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub kind: Kind,
    pub band: Band,
}

impl Profile {
    /// The profile a founder in `career` and `band` gives its corp.
    pub fn of(career: &str, band: Band) -> Profile {
        let kind = match family_of(career) {
            Some(Family::Rat | Family::Fight) => Kind::Combat,
            Some(Family::Explore) => Kind::Explore,
            _ => Kind::Industry,
        };
        Profile { kind, band }
    }

    /// `industry-high`, as `[soc]` and the state file write it.
    pub fn name(&self) -> String {
        format!("{}-{}", self.kind.name(), band_name(self.band))
    }

    pub fn parse(s: &str) -> Option<Profile> {
        let (k, b) = s.split_once('-')?;
        Some(Profile {
            kind: Kind::parse(k)?,
            band: parse_band(b)?,
        })
    }
}

pub fn band_name(b: Band) -> &'static str {
    match b {
        Band::High => "high",
        Band::Low => "low",
        Band::Null => "null",
        Band::Wormhole => "wh",
    }
}

pub fn parse_band(s: &str) -> Option<Band> {
    [Band::High, Band::Low, Band::Null, Band::Wormhole]
        .into_iter()
        .find(|b| band_name(*b) == s)
}

/// **The corp planner's quotas** (R-SOCIETY §3.4) **[A]**: the share of a
/// corp's members a profile kind wants in each family. An industrial corp:
/// half gatherers, a fifth builders, a fifth haulers, a tenth defence. A
/// combat corp: 60 % fighters, the rest ratters and builders.
pub fn quota(kind: Kind, f: Family) -> f64 {
    match (kind, f) {
        (Kind::Industry, Family::Gather) => 0.5,
        (Kind::Industry, Family::Build | Family::Haul) => 0.2,
        (Kind::Industry, Family::Rat) => 0.1,
        (Kind::Combat, Family::Fight) => 0.6,
        (Kind::Combat, Family::Rat) => 0.25,
        (Kind::Combat, Family::Build) => 0.15,
        (Kind::Explore, Family::Explore) => 0.5,
        (Kind::Explore, Family::Rat) => 0.2,
        (Kind::Explore, Family::Gather | Family::Build | Family::Haul) => 0.1,
        _ => 0.0,
    }
}

/// The share added to both sides of the need ratio, so an empty family is
/// not an infinite need **[A]**.
pub const NEED_SMOOTH: f64 = 0.1;
/// The need multiplier's bounds **[A]**: falsified by a corp whose members
/// all switch to the one career it lacks (too wide).
pub const NEED_MIN: f64 = 0.5;
pub const NEED_MAX: f64 = 2.0;

/// **`corp_need(c)`** for every career in `careers`, given the corp's
/// `kind` and its members' careers now: `(quota + s) / (share + s)` of the
/// career's family, inside `[NEED_MIN, NEED_MAX]`; 1 for a career with no
/// family.
pub fn corp_need(kind: Kind, members: &[&str], careers: &[&str]) -> BTreeMap<String, f64> {
    let n = members.len().max(1) as f64;
    let mut share: BTreeMap<Family, f64> = BTreeMap::new();
    for m in members {
        if let Some(f) = family_of(m) {
            *share.entry(f).or_default() += 1.0 / n;
        }
    }
    careers
        .iter()
        .map(|c| {
            let need = match family_of(c) {
                Some(f) => ((quota(kind, f) + NEED_SMOOTH)
                    / (share.get(&f).copied().unwrap_or(0.0) + NEED_SMOOTH))
                    .clamp(NEED_MIN, NEED_MAX),
                None => 1.0,
            };
            ((*c).to_owned(), need)
        })
        .collect()
}

/// How well bands match: the same 1, a neighbour (high-low, low-null,
/// null-wormhole) a half, else 0 **[A]**.
pub fn band_fit(a: Band, b: Band) -> f64 {
    let ix = |b: Band| match b {
        Band::High => 0i32,
        Band::Low => 1,
        Band::Null => 2,
        Band::Wormhole => 3,
    };
    match (ix(a) - ix(b)).abs() {
        0 => 1.0,
        1 => 0.5,
        _ => 0.0,
    }
}

/// The weights of family and band in [`fit`] **[A]**.
pub const FIT_FAMILY: f64 = 0.6;
pub const FIT_BAND: f64 = 0.4;

/// **`fit(profile, my career, my band)`** in `[0, 1]`: how much the
/// profile wants the career's family (its quota over its largest quota)
/// and how near the bands are.
pub fn fit(p: Profile, career: &str, band: Band) -> f64 {
    let top = [
        Family::Gather,
        Family::Build,
        Family::Haul,
        Family::Rat,
        Family::Fight,
        Family::Explore,
    ]
    .into_iter()
    .map(|f| quota(p.kind, f))
    .fold(0.0, f64::max);
    let fam = family_of(career).map_or(0.0, |f| quota(p.kind, f) / top.max(f64::EPSILON));
    FIT_FAMILY * fam + FIT_BAND * band_fit(p.band, band)
}

/// Founding: `sociability` above this **[A]**.
pub const FOUND_SOCIAL: f64 = 0.55;
/// Founding: `aggression + diligence` above this **[A]**.
pub const FOUND_DRIVE: f64 = 1.0;
/// Founding: the wallet must hold the fee and this much more **[A]**.
/// BOT-TUNE: 250,000 (was 5 M), GB-FIX's recommendation: a founder keeps a
/// few ammo and hull purchases, not five times its 1 M start. Falsified by
/// a founder that cannot fly its next ship after paying the fee.
pub const FOUND_RESERVE: f64 = 250_000.0;
/// Found anyway while fewer than this many bot corps exist **[A]**.
pub const MIN_CORPS: usize = 3;
/// Joining: `sociability` above this **[A]**. Falsified by a `[soc]` line
/// with most bots corpless after a game-week (too high), or every loner in
/// a corp (too low).
pub const JOIN_SOCIAL: f64 = 0.4;
/// The CEO accepts an applicant whose fit is at least this **[A]**.
pub const ACCEPT_FIT: f64 = 0.4;
/// No corp grows past this many members **[A]**.
pub const CORP_SIZE_CAP: usize = 15;
/// How much a grudge (in `R_MAX` units) weighs against a corp's fit when
/// choosing where to apply **[A]**.
pub const JOIN_GRUDGE_WEIGHT: f64 = 0.5;

/// **A bot founds a corp** (§3.2) when it is sociable and driven enough,
/// its wallet holds `fee` and the reserve, and no corp matches its profile
/// or fewer than [`MIN_CORPS`] exist. The skill is the caller's
/// (bought through `acquire`). **[A]**: the spec's "within `RECRUIT_RANGE`
/// jumps" is not read; any bot corp counts.
pub fn wants_to_found(
    t: &Traits,
    wallet: f64,
    fee: f64,
    matching_corp: bool,
    corps: usize,
) -> bool {
    t.get(Trait::Sociability) > FOUND_SOCIAL
        && t.get(Trait::Aggression) + t.get(Trait::Diligence) > FOUND_DRIVE
        && wallet >= fee + FOUND_RESERVE
        && (!matching_corp || corps < MIN_CORPS)
}

/// **A corpless bot looks for a corp** when sociable enough.
pub fn wants_to_join(t: &Traits) -> bool {
    t.get(Trait::Sociability) > JOIN_SOCIAL
}

/// One corp on offer to an applicant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Offer {
    pub corp: Group,
    pub fit: f64,
    /// The applicant's grudge against the corp's group, `0 ... R_MAX`.
    pub grudge: f64,
    pub size: usize,
    /// The corp counts the applicant's group an enemy.
    pub enemy: bool,
}

/// `fit - JOIN_GRUDGE_WEIGHT x grudge / R_MAX`.
pub fn join_score(o: &Offer) -> f64 {
    o.fit - JOIN_GRUDGE_WEIGHT * o.grudge.max(0.0) / R_MAX
}

/// **The corp to apply to**: the best [`join_score`] among those whose CEO
/// would accept ([`ceo_accepts`]); ties to the lower id.
pub fn best_corp(offers: &[Offer]) -> Option<Group> {
    offers
        .iter()
        .filter(|o| ceo_accepts(o.fit, o.size, o.enemy))
        .max_by(|a, b| {
            join_score(a)
                .total_cmp(&join_score(b))
                .then(b.corp.cmp(&a.corp))
        })
        .map(|o| o.corp)
}

/// **The CEO's rule**: fit at least [`ACCEPT_FIT`], room under
/// [`CORP_SIZE_CAP`], and not an enemy.
pub fn ceo_accepts(fit: f64, size: usize, enemy: bool) -> bool {
    fit >= ACCEPT_FIT && size < CORP_SIZE_CAP && !enemy
}

/// The satisfaction a new member starts at.
pub const SATISFACTION_START: f64 = 0.7;
/// The EWMA's weight on each new sample **[A]**.
pub const SATISFACTION_ALPHA: f64 = 0.2;
/// A member leaves below `LEAVE_AT x (1 - loyalty)` **[A]**...
pub const LEAVE_AT: f64 = 0.5;
/// ...held this many game-days **[A]**. Falsified by members leaving
/// after one bad evening, or never leaving a corp whose profile they left.
pub const LEAVE_CONFIRM_DAYS: f64 = 2.0;
/// The weights of one satisfaction sample **[A]**.
pub const W_INCOME: f64 = 0.5;
pub const W_FIT: f64 = 0.5;
pub const W_LOSS: f64 = 0.3;
pub const W_WAR: f64 = 0.3;
/// Aggression under this is a pacifist, unhappy in a corp at war **[A]**.
pub const PACIFIST: f64 = 0.3;

/// What a member's satisfaction is sampled from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mood {
    /// Income over the career's prior.
    pub income_ratio: f64,
    /// Ships lost since the last sample.
    pub losses: u32,
    /// The corp's fit to the member's career now.
    pub fit: f64,
    /// The corp is at war (always false until wars, §3.5).
    pub at_war: bool,
    pub aggression: f64,
}

/// One sample in `[0, 1]`.
pub fn mood_sample(m: &Mood) -> f64 {
    let income = m.income_ratio.clamp(0.0, 2.0) / 2.0;
    let losses = f64::from(m.losses.min(3)) / 3.0;
    let war = if m.at_war && m.aggression < PACIFIST {
        W_WAR
    } else {
        0.0
    };
    (W_INCOME * income + W_FIT * m.fit - W_LOSS * losses - war).clamp(0.0, 1.0)
}

/// **A member's satisfaction**: an EWMA of [`mood_sample`]s, and since
/// when it has been under the leave line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Satisfaction {
    pub value: f64,
    pub low_since: Option<u64>,
}

impl Default for Satisfaction {
    fn default() -> Self {
        Satisfaction {
            value: SATISFACTION_START,
            low_since: None,
        }
    }
}

impl Satisfaction {
    /// Fold one sample in at `now`; `true` when the member should leave:
    /// under `LEAVE_AT x (1 - loyalty)` for [`LEAVE_CONFIRM_DAYS`].
    pub fn update(&mut self, sample: f64, loyalty: f64, now: u64, day: u64) -> bool {
        self.value += SATISFACTION_ALPHA * (sample.clamp(0.0, 1.0) - self.value);
        let line = LEAVE_AT * (1.0 - loyalty.clamp(0.0, 1.0));
        if self.value >= line {
            self.low_since = None;
            return false;
        }
        let since = *self.low_since.get_or_insert(now);
        now.saturating_sub(since) as f64 >= LEAVE_CONFIRM_DAYS * day as f64
    }
}

// ---- alliances (§3.3) ------------------------------------------------------------

/// A bot corp as the alliance rule sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorpView {
    pub id: Group,
    pub members: usize,
    pub alliance: Option<Group>,
    /// The CEO's aggression.
    pub aggression: f64,
}

/// What a corp's CEO should do about alliances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllianceAct {
    Found { corp: Group },
    Apply { corp: Group, alliance: Group },
    Leave { corp: Group, alliance: Group },
}

/// The founder of a pair: more members, then more aggression, then the
/// lower id.
fn founder(a: &CorpView, b: &CorpView) -> Group {
    let key = |c: &CorpView| (c.members, c.aggression, std::cmp::Reverse(c.id));
    if key(a)
        .partial_cmp(&key(b))
        .is_some_and(|o| o == std::cmp::Ordering::Less)
    {
        b.id
    } else {
        a.id
    }
}

/// **The alliance acts** (pure):
/// - two corps mutually blue ([`Stance::Blue`] both ways, by `stance` and
///   `since`, over each other's group) for [`ALLY_DAYS`]: if neither is in
///   an alliance and fewer than [`MAX_ALLIANCES`] exist, the founder
///   ([`founder`]) founds; if one is in an alliance and the other is not,
///   the other applies to it;
/// - a member corp whose stance toward some target opposes its alliance's
///   leaves.
///
/// `alliances` is how many bot alliances exist now.
pub fn alliance_acts(
    corps: &[CorpView],
    stances: &Stances,
    alliances: usize,
    now: u64,
    day: u64,
) -> Vec<AllianceAct> {
    let group = |c: &CorpView| c.alliance.unwrap_or(c.id);
    let min = (ALLY_DAYS * day as f64) as u64;
    let long_blue = |a: &CorpView, b: &CorpView| {
        [(a.id, group(b)), (b.id, group(a))].iter().all(|&(o, t)| {
            stances.get(o, t) == Stance::Blue
                && stances
                    .since(o, t)
                    .is_some_and(|s| now.saturating_sub(s) >= min)
        })
    };
    let mut out = Vec::new();
    let mut busy: BTreeSet<Group> = BTreeSet::new();
    let mut founding = 0;
    for (i, a) in corps.iter().enumerate() {
        for b in &corps[i + 1..] {
            if busy.contains(&a.id) || busy.contains(&b.id) || !long_blue(a, b) {
                continue;
            }
            match (a.alliance, b.alliance) {
                (None, None) if alliances + founding < MAX_ALLIANCES => {
                    let f = founder(a, b);
                    out.push(AllianceAct::Found { corp: f });
                    founding += 1;
                    busy.insert(a.id);
                    busy.insert(b.id);
                }
                (Some(al), None) => {
                    out.push(AllianceAct::Apply {
                        corp: b.id,
                        alliance: al,
                    });
                    busy.insert(b.id);
                }
                (None, Some(al)) => {
                    out.push(AllianceAct::Apply {
                        corp: a.id,
                        alliance: al,
                    });
                    busy.insert(a.id);
                }
                _ => {}
            }
        }
    }
    for c in corps {
        let Some(al) = c.alliance else { continue };
        let diverges = stances
            .held
            .iter()
            .filter(|((o, t), _)| *o == c.id && *t != al)
            .any(|((_, t), h)| h.stance.opposes(stances.get(al, *t)));
        if diverges && corps.iter().filter(|o| o.alliance == Some(al)).count() > 1 {
            out.push(AllianceAct::Leave {
                corp: c.id,
                alliance: al,
            });
        }
    }
    out
}

/// The `[soc]` line's society fields.
pub fn summary_fields(corps: usize, alliances: usize, blue: usize, red: usize) -> String {
    format!("corps {corps} alliances {alliances} blue {blue} red {red}")
}

/// The `[soc]` summary's clock: ` day N`, the hall's game-day count, appended
/// to [`summary_fields`] so `tools/society-report.py` can time the stance
/// flips of gate GC.
pub fn day_field(game_day: u64) -> String {
    format!(" day {game_day}")
}

/// The `[soc]` line for a loss that moved (or did not move) a relation:
/// `victim`'s view of `killer` before and after. `tools/society-report.py`
/// reads it for gate GC's "a loss moves a relation".
pub fn loss_line(victim: Group, killer: Group, before: f64, after: f64) -> String {
    format!(
        "[soc] relation loss victim={victim} killer={killer} before={before:.2} after={after:.2}"
    )
}

#[cfg(test)]
mod tests {

    #[test]
    fn friendly_enters_at_friend_at_and_leaves_with_the_band() {
        use crate::relations::Temper;
        let n = Stance5::Neutral;
        assert_eq!(next_stance5(n, FRIEND_AT - 0.01), Stance5::Neutral);
        assert_eq!(next_stance5(n, FRIEND_AT), Stance5::Friendly);
        assert_eq!(next_stance5(n, ALLY_AT), Stance5::Allied);
        // Friendly holds down to FRIEND_LEAVE, then falls.
        let f = Stance5::Friendly;
        assert_eq!(next_stance5(f, FRIEND_LEAVE), Stance5::Friendly);
        assert_eq!(next_stance5(f, FRIEND_LEAVE - 0.01), Stance5::Neutral);
        // Allied holds down to ALLY_AT - BAND, then is Friendly while above FRIEND_AT.
        let a = Stance5::Allied;
        assert_eq!(next_stance5(a, ALLY_AT - BAND), Stance5::Allied);
        assert_eq!(next_stance5(a, ALLY_AT - BAND - 0.01), Stance5::Friendly);
        // The mirror side.
        assert_eq!(next_stance5(n, WARY_AT), Stance5::Wary);
        assert_eq!(next_stance5(Stance5::Wary, WARY_LEAVE), Stance5::Wary);
        assert_eq!(
            next_stance5(Stance5::Wary, WARY_LEAVE + 0.01),
            Stance5::Neutral
        );
        assert_eq!(next_stance5(n, ENEMY_AT), Stance5::Hostile);
        assert_eq!(
            next_stance5(Stance5::Hostile, ENEMY_AT + BAND),
            Stance5::Hostile
        );
        assert_eq!(
            next_stance5(Stance5::Hostile, ENEMY_AT + BAND + 0.01),
            Stance5::Wary
        );
        // Contacts and the reading of allied().
        assert_eq!(Stance5::Friendly.standing(), Some(5.0));
        assert!(Stance5::Friendly.is_allied() && !Stance5::Wary.is_allied());
        assert_eq!(Stance5::Allied.coarse(), Stance::Blue);
        // A harder temper hunts earlier: Hostile starts nearer 0.
        let hard = Temper::HARD;
        assert_eq!(
            next_stance5_with(n, ENEMY_AT + 0.5, &hard),
            Stance5::Hostile
        );
        assert_eq!(
            next_stance5_with(n, ENEMY_AT + 0.5, &Temper::NORMAL),
            Stance5::Wary
        );
    }
    use super::*;
    use crate::mind::TRAIT_COUNT;

    const DAY: u64 = 1_000;
    const A: Group = 98_000_101;
    const B: Group = 98_000_102;
    const X: Group = 90_000_103;

    fn traits(set: &[(Trait, f64)]) -> Traits {
        let mut t = Traits([0.5; TRAIT_COUNT]);
        for &(k, v) in set {
            t.0[k as usize] = v;
        }
        t
    }

    #[test]
    fn a_kill_moves_the_victims_view_down_and_only_its() {
        let mut r = Relations::default();
        r.apply(&Event::Kill {
            victim: A,
            killer: X,
            isk: KILL_VALUE_REF,
        });
        assert!((r.get(A, X) + K_KILL).abs() < 1e-9);
        assert_eq!(r.get(X, A), 0.0);
        // A bigger loss weighs more, up to the ceiling.
        let mut big = Relations::default();
        big.apply(&Event::Kill {
            victim: A,
            killer: X,
            isk: 1e3 * KILL_VALUE_REF,
        });
        assert!((big.get(A, X) + K_KILL * VALUE_FACTOR_MAX).abs() < 1e-9);
        // Never past the bound.
        for _ in 0..100 {
            big.apply(&Event::Kill {
                victim: A,
                killer: X,
                isk: 0.0,
            });
        }
        assert_eq!(big.get(A, X), -R_MAX);
    }

    #[test]
    fn a_common_enemy_moves_two_groups_toward_each_other() {
        let mut r = Relations::default();
        r.add(A, X, 2.0 * ENEMY_AT);
        r.add(B, X, 2.0 * ENEMY_AT);
        r.shared_enemies(&[A, B], |g| g, 1.0);
        assert!((r.get(A, B) - K_SHARED).abs() < 1e-9);
        assert!((r.get(B, A) - K_SHARED).abs() < 1e-9);
        // Only one of them hating X moves nothing.
        let mut one = Relations::default();
        one.add(A, X, 2.0 * ENEMY_AT);
        one.shared_enemies(&[A, B], |g| g, 1.0);
        assert_eq!(one.get(A, B), 0.0);
    }

    #[test]
    fn the_threshold_needs_the_band_to_cross_back() {
        assert_eq!(next_stance(Stance::Neutral, ALLY_AT), Stance::Blue);
        assert_eq!(
            next_stance(Stance::Neutral, ALLY_AT - BAND / 2.0),
            Stance::Neutral
        );
        assert_eq!(
            next_stance(Stance::Blue, ALLY_AT - BAND / 2.0),
            Stance::Blue
        );
        assert_eq!(
            next_stance(Stance::Blue, ALLY_AT - BAND * 1.5),
            Stance::Neutral
        );
        assert_eq!(next_stance(Stance::Neutral, ENEMY_AT), Stance::Red);
        assert_eq!(next_stance(Stance::Red, ENEMY_AT + BAND / 2.0), Stance::Red);
        assert_eq!(
            next_stance(Stance::Red, ENEMY_AT + BAND * 1.5),
            Stance::Neutral
        );
        assert_eq!(next_stance(Stance::Blue, ENEMY_AT), Stance::Red);
    }

    #[test]
    fn a_pair_changes_stance_at_most_once_per_flip_window() {
        let mut s = Stances::default();
        let t0 = 10 * DAY;
        assert_eq!(
            s.settle(A, B, ALLY_AT, t0, DAY),
            Some((Stance::Neutral, Stance::Blue))
        );
        let window = (FLIP_MIN_DAYS * DAY as f64) as u64;
        // Inside the window a red relation does not flip it.
        assert_eq!(s.settle(A, B, ENEMY_AT, t0 + window - 1, DAY), None);
        assert_eq!(s.get(A, B), Stance::Blue);
        assert_eq!(
            s.settle(A, B, ENEMY_AT, t0 + window, DAY),
            Some((Stance::Blue, Stance::Red))
        );
        // A pair never seen stays neutral without a record.
        assert_eq!(s.settle(A, X, 0.0, t0, DAY), None);
        assert!(!s.held.contains_key(&(A, X)));
    }

    #[test]
    fn decay_is_monotone_toward_zero_and_slower_for_loyal_groups() {
        let mut fickle = Relations::default();
        fickle.add(A, X, R_MAX);
        fickle.add(A, B, -R_MAX);
        let mut loyal = fickle.clone();
        fickle.decay(0, DAY, |_| 0.0);
        loyal.decay(0, DAY, |_| 1.0);
        let mut last = (R_MAX, -R_MAX);
        for d in 1..=20 {
            fickle.decay(d * DAY, DAY, |_| 0.0);
            loyal.decay(d * DAY, DAY, |_| 1.0);
            let now = (fickle.get(A, X), fickle.get(A, B));
            assert!(now.0 <= last.0 && now.0 >= 0.0, "{now:?}");
            assert!(now.1 >= last.1 && now.1 <= 0.0, "{now:?}");
            assert!(loyal.get(A, X) >= fickle.get(A, X));
            last = now;
        }
        let at_half = HALF_LIFE_DAYS * 0.5;
        let mut one = Relations::default();
        one.add(A, X, R_MAX);
        one.decay(0, DAY, |_| 0.0);
        one.decay((at_half * DAY as f64) as u64, DAY, |_| 0.0);
        assert!((one.get(A, X) - R_MAX / 2.0).abs() < 1e-6);
    }

    #[test]
    fn the_hegemon_rule_applies_only_above_the_share() {
        let mut r = Relations::default();
        let mut shares = BTreeMap::new();
        shares.insert(X, HEGEMON_SHARE);
        r.hegemon(&[A, B], &shares, |g| g, 1.0);
        assert_eq!(r.get(A, X), 0.0);
        shares.insert(X, (HEGEMON_SHARE + 1.0) / 2.0);
        r.hegemon(&[A, B, X], &shares, |g| g, 1.0);
        assert!((r.get(A, X) + K_HEGEMON).abs() < 1e-9);
        assert!((r.get(B, X) + K_HEGEMON).abs() < 1e-9);
        assert_eq!(r.get(X, X), 0.0);
    }

    #[test]
    fn the_matrix_after_the_same_event_stream_is_deterministic() {
        let stream = [
            Event::Kill {
                victim: A,
                killer: X,
                isk: 3.0 * KILL_VALUE_REF,
            },
            Event::Trade {
                a: A,
                b: B,
                isk: 5.0 * KILL_VALUE_REF,
            },
            Event::AllyFight { a: A, b: B },
            Event::Neighbour {
                a: A,
                b: B,
                days: 0.5,
            },
            Event::Contest {
                a: B,
                b: X,
                days: 1.0,
            },
            Event::Siege {
                defender: X,
                attacker: A,
            },
        ];
        let run = || {
            let mut r = Relations::default();
            let mut s = Stances::default();
            for (i, e) in stream.iter().enumerate() {
                r.apply(e);
                r.decay(i as u64 * DAY / 3, DAY, |_| 0.5);
                for (&(o, t), &v) in r.r.clone().iter() {
                    s.settle(o, t, v, i as u64 * DAY / 3, DAY);
                }
            }
            (r, s)
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn allied_follows_the_servers_rule_and_hostile_is_its_negation() {
        let me = Affiliation {
            character_id: 90_000_001,
            corporation_id: A,
            alliance_id: None,
        };
        let mate = Affiliation {
            character_id: 90_000_002,
            ..me
        };
        let stranger = Affiliation {
            character_id: X,
            corporation_id: 1_000_044,
            alliance_id: None,
        };
        let other_corp = Affiliation {
            character_id: 90_000_004,
            corporation_id: B,
            alliance_id: None,
        };
        let none = |_: i64, _: i64| None;
        assert!(allied(me, mate, none));
        assert!(hostile_to(me, stranger, none));
        assert!(hostile_to(me, other_corp, none));
        // A coalition blue in my corp's book on their corp.
        let book = |o: i64, t: i64| (o == A && t == B).then_some(BLUE_STANDING);
        assert!(!hostile_to(me, other_corp, book));
        // One-sided: their book says nothing about me.
        assert!(hostile_to(other_corp, me, book));
        assert_eq!(group_of(&stranger), X);
        assert_eq!(group_of(&me), A);
    }

    /// **BOT-TUNE**: the reserve is what a founder keeps beyond the server's
    /// fee. A bot holding its 1 M start (the owner's figure) is still short
    /// of the fee and reserve together, so a corp takes earnings, not the
    /// start; one ISK under the line refuses, on it founds.
    #[test]
    fn a_founder_needs_the_fee_and_the_reserve_and_a_start_alone_is_short() {
        let t = traits(&[
            (Trait::Sociability, (FOUND_SOCIAL + 1.0) / 2.0),
            (Trait::Aggression, (FOUND_DRIVE / 2.0 + 1.0) / 2.0),
            (Trait::Diligence, (FOUND_DRIVE / 2.0 + 1.0) / 2.0),
        ]);
        let fee = crate::corpmind::CORP_FEE;
        let start = 1_000_000.0;
        let line = fee + FOUND_RESERVE;
        assert!(wants_to_found(&t, line, fee, false, 0));
        assert!(!wants_to_found(&t, line - 1.0, fee, false, 0));
        assert!(!wants_to_found(&t, start, fee, false, 0));
        // The reserve no longer dwarfs the start: the shortfall is the fee
        // and a quarter-million, not 5.6 M.
        assert!(line - start < 1_000_000.0);
    }

    #[test]
    fn a_sociable_driven_rich_bot_founds_and_a_matching_corp_stops_it() {
        let t = traits(&[
            (Trait::Sociability, (FOUND_SOCIAL + 1.0) / 2.0),
            (Trait::Aggression, (FOUND_DRIVE / 2.0 + 1.0) / 2.0),
            (Trait::Diligence, (FOUND_DRIVE / 2.0 + 1.0) / 2.0),
        ]);
        let fee = 1_000.0;
        let rich = fee + FOUND_RESERVE;
        assert!(wants_to_found(&t, rich, fee, false, MIN_CORPS));
        assert!(!wants_to_found(&t, rich - 1.0, fee, false, 0));
        assert!(!wants_to_found(&t, rich, fee, true, MIN_CORPS));
        assert!(wants_to_found(&t, rich, fee, true, MIN_CORPS - 1));
        let loner = traits(&[(Trait::Sociability, FOUND_SOCIAL / 2.0)]);
        assert!(!wants_to_found(&loner, rich, fee, false, 0));
        let lazy = traits(&[
            (Trait::Sociability, (FOUND_SOCIAL + 1.0) / 2.0),
            (Trait::Aggression, 0.0),
            (Trait::Diligence, FOUND_DRIVE / 2.0),
        ]);
        assert!(!wants_to_found(&lazy, rich, fee, false, 0));
        assert!(wants_to_join(&traits(&[(
            Trait::Sociability,
            (JOIN_SOCIAL + 1.0) / 2.0
        )])));
        assert!(!wants_to_join(&traits(&[(
            Trait::Sociability,
            JOIN_SOCIAL / 2.0
        )])));
    }

    #[test]
    fn a_miner_applies_to_the_industrial_corp_and_the_ceo_rule_holds() {
        let industry = Profile::of("miner", Band::High);
        let combat = Profile::of("ratter", Band::High);
        assert_eq!(industry.kind, Kind::Industry);
        assert_eq!(combat.kind, Kind::Combat);
        assert!(fit(industry, "miner", Band::High) > fit(combat, "miner", Band::High));
        assert!(fit(industry, "miner", Band::High) > fit(industry, "miner", Band::Null));
        let offers = [
            Offer {
                corp: A,
                fit: fit(industry, "miner", Band::High),
                grudge: 0.0,
                size: 1,
                enemy: false,
            },
            Offer {
                corp: B,
                fit: fit(combat, "miner", Band::High),
                grudge: 0.0,
                size: 1,
                enemy: false,
            },
        ];
        assert_eq!(best_corp(&offers), Some(A));
        // A grudge against it, a full corp, or being its enemy rules it out.
        let mut grudged = offers;
        grudged[0].grudge = R_MAX;
        grudged[1].fit = grudged[0].fit;
        assert_eq!(best_corp(&grudged), Some(B));
        let mut full = offers;
        full[0].size = CORP_SIZE_CAP;
        assert_ne!(best_corp(&full), Some(A));
        let mut enemy = offers;
        enemy[0].enemy = true;
        assert_ne!(best_corp(&enemy), Some(A));
        assert!(!ceo_accepts(ACCEPT_FIT / 2.0, 1, false));
        assert_eq!(Profile::parse(&industry.name()), Some(industry));
    }

    #[test]
    fn an_unhappy_member_leaves_only_after_the_confirm_days_and_loyalty_holds_one() {
        let bad = Mood {
            income_ratio: 0.0,
            losses: 3,
            fit: 0.0,
            at_war: false,
            aggression: 0.5,
        };
        let good = Mood {
            income_ratio: 2.0,
            losses: 0,
            fit: 1.0,
            ..bad
        };
        assert!(mood_sample(&good) > mood_sample(&bad));
        let mut s = Satisfaction::default();
        let mut left = None;
        for i in 0..200u64 {
            if s.update(mood_sample(&bad), 0.0, i * DAY / 10, DAY) {
                left = Some(i);
                break;
            }
        }
        let at = left.expect("a fickle member leaves");
        let first_low = s.low_since.expect("it went low");
        assert!(at * DAY / 10 - first_low >= (LEAVE_CONFIRM_DAYS * DAY as f64) as u64);
        // Fully loyal: the leave line is 0, never crossed.
        let mut loyal = Satisfaction::default();
        assert!((0..200u64).all(|i| !loyal.update(mood_sample(&bad), 1.0, i * DAY, DAY)));
        // One good sample under the line resets the wait.
        let mut s = Satisfaction {
            value: 0.0,
            low_since: None,
        };
        s.update(0.0, 0.0, 0, DAY);
        assert!(s.low_since.is_some());
        s.value = 1.0;
        s.update(1.0, 0.0, 1, DAY);
        assert!(s.low_since.is_none());
    }

    #[test]
    fn the_planner_wants_what_the_profile_lacks() {
        let need = corp_need(
            Kind::Industry,
            &["ratter", "ratter", "ratter", "miner"],
            &["miner", "ratter", "builder", "t_unknown"],
        );
        assert!(need["miner"] > 1.0, "{need:?}");
        assert!(need["ratter"] < 1.0, "{need:?}");
        assert!(need["builder"] > 1.0, "{need:?}");
        assert_eq!(need["t_unknown"], 1.0);
        assert!(need.values().all(|v| (NEED_MIN..=NEED_MAX).contains(v)));
    }

    fn blue_both(s: &mut Stances, a: Group, b: Group, at: u64) {
        s.settle(a, b, ALLY_AT, at, DAY);
        s.settle(b, a, ALLY_AT, at, DAY);
    }

    #[test]
    fn two_long_blue_corps_merge_and_the_larger_founds() {
        let a = CorpView {
            id: A,
            members: 3,
            alliance: None,
            aggression: 0.1,
        };
        let b = CorpView {
            id: B,
            members: 2,
            alliance: None,
            aggression: 0.9,
        };
        let mut s = Stances::default();
        blue_both(&mut s, A, B, 0);
        let soon = (ALLY_DAYS * DAY as f64) as u64 - 1;
        assert!(alliance_acts(&[a, b], &s, 0, soon, DAY).is_empty());
        let later = soon + 1;
        assert_eq!(
            alliance_acts(&[a, b], &s, 0, later, DAY),
            vec![AllianceAct::Found { corp: A }]
        );
        // Equal size: the more aggressive founds.
        let b3 = CorpView { members: 3, ..b };
        assert_eq!(
            alliance_acts(&[a, b3], &s, 0, later, DAY),
            vec![AllianceAct::Found { corp: B }]
        );
        // The cap stops a new alliance.
        assert!(alliance_acts(&[a, b], &s, MAX_ALLIANCES, later, DAY).is_empty());
        // Once A's alliance exists, B applies to it (B blue toward the
        // alliance, the alliance's corp blue toward B).
        const AL: Group = 99_000_101;
        let a_in = CorpView {
            alliance: Some(AL),
            ..a
        };
        let mut s2 = Stances::default();
        s2.settle(A, B, ALLY_AT, 0, DAY);
        s2.settle(B, AL, ALLY_AT, 0, DAY);
        assert_eq!(
            alliance_acts(&[a_in, b], &s2, 1, later, DAY),
            vec![AllianceAct::Apply {
                corp: B,
                alliance: AL
            }]
        );
    }

    #[test]
    fn a_member_corp_that_opposes_its_alliance_leaves() {
        const AL: Group = 99_000_101;
        let a = CorpView {
            id: A,
            members: 2,
            alliance: Some(AL),
            aggression: 0.5,
        };
        let b = CorpView { id: B, ..a };
        let mut s = Stances::default();
        s.settle(AL, X, ENEMY_AT, 0, DAY);
        s.settle(A, X, ENEMY_AT, 0, DAY);
        s.settle(B, X, ALLY_AT, 0, DAY);
        assert_eq!(
            alliance_acts(&[a, b], &s, 1, 0, DAY),
            vec![AllianceAct::Leave {
                corp: B,
                alliance: AL
            }]
        );
        // Alone in it, nobody leaves.
        assert!(alliance_acts(&[b], &s, 1, 0, DAY).is_empty());
    }

    /// C4-fix 3: neutral and dead-owner pairs are dropped, but not inside
    /// the flip window.
    #[test]
    fn prune_drops_settled_neutral_and_dead_pairs_only() {
        let mut s = Stances::default();
        let t0 = 10 * DAY;
        s.settle(A, X, ALLY_AT, t0, DAY);
        s.settle(A, X, 0.0, t0 + DAY, DAY);
        s.settle(B, X, ALLY_AT, t0 + DAY, DAY);
        s.settle(A, B, ALLY_AT, t0 + DAY, DAY);
        s.held.insert(
            (X, B),
            Held {
                stance: Stance::Neutral,
                since: None,
            },
        );
        // Right after A->X went neutral: kept (inside the window).
        assert_eq!(s.prune(t0 + DAY, DAY, |o| o != X), 1);
        assert!(s.held.contains_key(&(A, X)));
        assert!(!s.held.contains_key(&(X, B)));
        let later = t0 + 3 * DAY;
        // Neutral (A,X) goes; (B,X) and (A,B) stay for live owners.
        assert_eq!(s.prune(later, DAY, |_| true), 1);
        assert!(!s.held.contains_key(&(A, X)));
        assert_eq!(s.get(B, X), Stance::Blue);
        // A dead owner's blue pair goes once settled.
        assert_eq!(s.prune(later, DAY, |o| o != B), 1);
        assert_eq!(s.get(B, X), Stance::Neutral);
        assert_eq!(s.get(A, B), Stance::Blue);
    }
}

#[cfg(test)]
mod help_event_tests {
    use super::*;
    use crate::relations::{self, Temper};

    const A: Group = 98_000_401;
    const B: Group = 98_000_402;

    #[test]
    fn the_help_events_move_exactly_the_views_they_name() {
        let mut r = Relations::default();
        r.apply(&Event::Reinforced {
            helper: A,
            helped: B,
            isk_saved: 0.0,
        });
        assert_eq!(r.get(A, B), K_REINFORCE);
        assert_eq!(r.get(B, A), K_REINFORCE);
        let mut r = Relations::default();
        r.apply(&Event::Abandoned { asker: A, ally: B });
        assert_eq!(r.get(A, B), -K_ABANDON);
        assert_eq!(r.get(B, A), 0.0, "the absent ally's view does not move");
        let mut r = Relations::default();
        r.apply(&Event::Betrayed {
            betrayer: A,
            victim: B,
        });
        assert_eq!(r.get(B, A), -K_BETRAY);
        assert_eq!(r.get(A, B), 0.0);
        // A bigger rescue counts for more, inside the value band.
        let mut big = Relations::default();
        big.apply(&Event::Reinforced {
            helper: A,
            helped: B,
            isk_saved: 1.0e12,
        });
        assert_eq!(big.get(A, B), K_REINFORCE * VALUE_FACTOR_MAX);
    }

    #[test]
    fn the_model_adapter_agrees_with_apply_at_normal() {
        let events = [
            Event::Reinforced {
                helper: A,
                helped: B,
                isk_saved: 3.0e7,
            },
            Event::Abandoned { asker: A, ally: B },
            Event::Betrayed {
                betrayer: A,
                victim: B,
            },
        ];
        for e in &events {
            let mut direct = Relations::default();
            direct.apply(e);
            let mut via = Relations::default();
            for d in relations::event_deltas(e, &Temper::NORMAL) {
                via.add(d.holder, d.subject, d.d);
            }
            assert_eq!(direct.r, via.r, "{e:?}");
        }
        // The harder tempers condemn an absence more; they do not pay more
        // for a rescue.
        let hard = relations::event_deltas(&events[1], &Temper::HARD)[0].d;
        assert!(hard <= -K_ABANDON);
    }

    #[test]
    fn the_report_lines_carry_what_the_judge_reads() {
        assert_eq!(day_field(7), " day 7");
        let line = loss_line(1, 2, 3.0, 1.5);
        assert_eq!(
            line,
            "[soc] relation loss victim=1 killer=2 before=3.00 after=1.50"
        );
        let sum = format!("{}{}", summary_fields(3, 2, 1, 1), day_field(4));
        assert_eq!(sum, "corps 3 alliances 2 blue 1 red 1 day 4");
    }
}
