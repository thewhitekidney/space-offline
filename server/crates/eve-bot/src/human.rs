//! **The human side of the relations** (R-REL-4, R-RELATIONS §3.6): how the
//! owner and friends gain and lose standing with the bots, and how they are
//! told.
//!
//! What moves a human's standing is already observed and weighed by the
//! earlier lanes ([`crate::observe`] reads the kill reports, contracts, wars
//! and mail; [`crate::relations::deltas`] weighs them, a gank worse than a
//! duel). This module adds what they left open:
//!
//! - **F5, shared fleet hours** ([`presence`]): a bot in a fleet credits the
//!   humans on its grid with the game-hours that pass ([`tick`]), capped per
//!   day. The hall-wide hours feed the CEO's invitation
//!   ([`HallState::invitations`]); the bot's own regard and its corp's view
//!   rise by the F5 fact.
//! - **Tiers** ([`HallState::tier_changes`]): each bot corp holds a graded
//!   tier toward each human it has seen ([`Stance5`], with the band
//!   hysteresis), kept in the hall file; when it changes, the CEO mails the
//!   human ([`crate::mail::tier_text`]) -- the GM-free way to see where one
//!   stands, beside the contacts the bots set (R-REL-3) that the client's
//!   address book and overview show.
//! - **What a tier opens or closes** ([`unlocks`]): the §3.6 table in one
//!   place; the effects read `R_eff` at their own bars
//!   ([`crate::effects`]) and a test pins the two together.
//! - **A readable dump** ([`HallState::standing_report`]): every bot corp's
//!   tier toward one pilot, for the `[rel]` log line and the morning test.
//!
//! The `--friends` file ([`crate::friends`]) seeds and wins: each bot's
//! regard is set to the file's standing at login and `R_eff` is never below
//! it ([`HallState::r_eff_of`]); a listed friend therefore is not hunted
//! however the first hours go **[A]**, falsified by the owner wanting his
//! friends to be able to burn their standing (then make the floor decay).

use crate::corpmind::{Hall, HallState};
use crate::mind::{Mind, Trait};
use crate::relations::{self, Ctx, Fact, FactKind};
use crate::society::{self, Stance5};
use crate::{Conn, Result};
use eve_server::group_contacts::Affiliation;
use std::sync::Mutex;

/// A gap in presence longer than this many game-hours restarts a clock with
/// no credit **[A]**: a bot that left the grid and came back has not flown
/// with the pilot meanwhile. Falsified by `[rel]` hours growing for a human
/// who warped away (too long) or a fleet whose clock keeps restarting (too
/// short).
pub const PRESENCE_GAP_HOURS: f64 = 1.0;

/// The most game-hours a pair is credited per game-day: the spec's cap on
/// F5 (`K_FLEET_CAP` over `K_FLEET`).
pub const FLEET_DAY_CAP_HOURS: f64 = relations::K_FLEET_CAP / relations::K_FLEET;

/// How many tier mails one CEO turn sends **[A]**. Falsified by a friend
/// finding the mailbox flooded on the first night (too many) or a change
/// taking days to tell (too few).
pub const TIER_MAILS_PER_TURN: usize = 2;

/// A pilot this bot would not even call neutral earns no fleet credit.
pub const CREDIT_ABOVE: f64 = society::WARY_AT;

// ---- the presence clock (pure) -----------------------------------------------------------

/// One pair's clock for F5: when it last ticked, and what the current
/// game-day has credited.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    pub last: u64,
    pub day: u64,
    pub today: f64,
}

/// **Advance a presence clock** to `now` (pure): the new clock and the
/// game-hours to credit. The first tick and a tick after a gap of more than
/// [`PRESENCE_GAP_HOURS`] credit nothing; a game-day credits at most
/// [`FLEET_DAY_CAP_HOURS`]. `day` is the game-day in seconds.
pub fn tick(clock: Option<Clock>, now: u64, day: u64) -> (Clock, f64) {
    let day = day.max(1);
    let hour = (day as f64 / 24.0).max(f64::MIN_POSITIVE);
    let today = now / day;
    let Some(mut c) = clock else {
        return (
            Clock {
                last: now,
                day: today,
                today: 0.0,
            },
            0.0,
        );
    };
    if c.day != today {
        c.day = today;
        c.today = 0.0;
    }
    let gap = now.saturating_sub(c.last) as f64 / hour;
    c.last = c.last.max(now);
    if gap > PRESENCE_GAP_HOURS {
        return (c, 0.0);
    }
    let got = gap.min((FLEET_DAY_CAP_HOURS - c.today).max(0.0));
    c.today += got;
    (c, got)
}

// ---- tiers ---------------------------------------------------------------------------------

/// What a tier opens and closes toward a human (§3.6), one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unlocks {
    /// Prey for the hunters.
    pub hunted: bool,
    /// The CEO may invite them and the foreman fleets them.
    pub invites: bool,
    /// A `NeedBackup` of theirs is answered.
    pub backup: bool,
    /// Private contracts at a discount, staging mail.
    pub discount: bool,
    /// Contracts at a premium.
    pub premium: bool,
    /// Contracts refused outright.
    pub refused: bool,
}

/// **The §3.6 table**: what each tier opens or closes. Neutral is hunted
/// only per the odds in low and null (not in itself); Hostile is hunted at
/// relaxed odds.
pub fn unlocks(tier: Stance5) -> Unlocks {
    Unlocks {
        hunted: tier <= Stance5::Wary,
        invites: tier >= Stance5::Friendly,
        backup: tier >= Stance5::Friendly,
        discount: tier == Stance5::Allied,
        premium: tier == Stance5::Wary,
        refused: tier == Stance5::Hostile,
    }
}

/// One tier crossing, as the CEO will tell it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierChange {
    pub human: i64,
    pub from: Stance5,
    pub to: Stance5,
    pub r: f64,
}

impl HallState {
    /// The affiliation the hall last saw `id` with.
    pub fn human_affiliation(&self, id: i64) -> Option<Affiliation> {
        self.humans.get(&id).map(|s| Affiliation {
            character_id: id,
            corporation_id: s.corp,
            alliance_id: s.alliance,
        })
    }

    /// **The tier crossings of corp `corp`'s CEO** (at most `max`): every
    /// seen human whose `R_eff` (the CEO's own `regard` and `loyalty`
    /// included) puts them in another tier than the one last told, with the
    /// band hysteresis of [`society::next_stance5_with`]. A human never told
    /// before is judged from Neutral, so a first meeting at Neutral says
    /// nothing and a listed friend is told once. The new tier is stored for
    /// each change returned; the ones past `max` wait for the next turn.
    pub fn tier_changes(
        &mut self,
        corp: i64,
        regard: &[crate::mind::Regard],
        loyalty: f64,
        now: u64,
        max: usize,
    ) -> Vec<TierChange> {
        let ids: Vec<i64> = self.humans.keys().copied().collect();
        let mut out = Vec::new();
        for id in ids {
            let Some(a) = self.human_affiliation(id) else {
                continue;
            };
            let r = self.r_eff_of(corp, regard, loyalty, &a, now);
            let prev = self.tiers.get(&(corp, id)).map(|t| t.0);
            let held = prev.unwrap_or(Stance5::Neutral);
            let to = society::next_stance5_with(held, r, &self.temper);
            if to == held {
                if prev.is_none() {
                    self.tiers.insert((corp, id), (to, now));
                    self.dirty = true;
                }
                continue;
            }
            if out.len() >= max {
                continue;
            }
            self.tiers.insert((corp, id), (to, now));
            self.dirty = true;
            self.act(format!(
                "rel: {id} is {} to corp {corp} (was {}, R {r:.1})",
                to.name(),
                held.name()
            ));
            out.push(TierChange {
                human: id,
                from: held,
                to,
                r,
            });
        }
        out
    }

    /// **Every bot corp's tier toward `human`**, one line each, from the
    /// corps' own views alone (no bot's personal regard): the `[rel]` dump.
    pub fn standing_report(&self, human: i64, now: u64) -> Vec<String> {
        let a = self.human_affiliation(human).unwrap_or(Affiliation {
            character_id: human,
            corporation_id: 0,
            alliance_id: None,
        });
        self.bot_corps()
            .into_iter()
            .map(|c| {
                let r = self.r_eff_of(c, &[], 0.5, &a, now);
                let held = self.tiers.get(&(c, human)).map(|t| t.0);
                let tier =
                    society::next_stance5_with(held.unwrap_or(Stance5::Neutral), r, &self.temper);
                format!(
                    "[rel] corp {c} sees {human}: {} (R {r:.1}, {:.1} h fleet)",
                    tier.name(),
                    self.fleet_hours.get(&human).copied().unwrap_or(0.0)
                )
            })
            .collect()
    }
}

// ---- F5, the presence hook -----------------------------------------------------------------

/// REVIEW-FIX: of the pilots on the grid, only the **real fleet members**:
/// the ones heard to join the fleet (`OnFleetJoin`, [`crate::fleet::Command`])
/// and the fleet's boss. Standing on the same grid earns no fleet hours.
pub fn fleet_mates(
    on_grid: Vec<Affiliation>,
    joined: &std::collections::BTreeSet<i64>,
    boss: Option<i64>,
) -> Vec<Affiliation> {
    on_grid
        .into_iter()
        .filter(|a| joined.contains(&a.character_id) || boss == Some(a.character_id))
        .collect()
}

/// **Credit the humans on a bot's grid while it is in a fleet** (F5), at
/// `now` (Unix seconds, as the social turn's). `mates` are the pilots no bot
/// flies that this bot shares the grid with, the fleet's human boss
/// included. REVIEW-FIX: the caller passes only real fleet members
/// ([`fleet_mates`]); grid presence alone earns nothing. A pilot the bot rates
/// at or below [`CREDIT_ABOVE`] is never credited, which keeps a hostile
/// stranger from buying standing by standing near.
///
/// The bot's own regard and its corp's view rise by the F5 fact; the
/// hall-wide hours (the CEO's invitation) are credited once per human,
/// however many bots watch. Returns the hours credited to this bot.
pub fn presence(
    hall: &Hall,
    mind: &Mutex<Mind>,
    me: &Affiliation,
    mates: &[Affiliation],
    now: u64,
) -> f64 {
    let (regard, loyalty) = {
        let m = mind.lock().unwrap_or_else(|e| e.into_inner());
        (m.regard.clone(), m.traits.get(Trait::Loyalty))
    };
    let mut credited = 0.0;
    let mut personal: Vec<relations::Delta> = Vec::new();
    let (day, temper) = {
        let mut st = hall.lock();
        let (day, temper) = (st.day, st.temper);
        for m in mates {
            if m.character_id <= 0
                || m.character_id == me.character_id
                || st.members.contains_key(&m.character_id)
            {
                continue;
            }
            st.see_human(m, now);
            if st.r_eff_of(me.corporation_id, &regard, loyalty, m, now) <= CREDIT_ABOVE {
                continue;
            }
            let pair = st
                .fleet_clock
                .get(&(me.character_id, m.character_id))
                .copied();
            let (clock, hours) = tick(pair, now, day);
            st.fleet_clock
                .insert((me.character_id, m.character_id), clock);
            let wide = st.fleet_clock.get(&(0, m.character_id)).copied();
            let (clock, wide_hours) = tick(wide, now, day);
            st.fleet_clock.insert((0, m.character_id), clock);
            if wide_hours > 0.0 {
                st.note_fleet_hours(m.character_id, wide_hours);
                // The corp's view, once per human (the matrix, spill only).
                let ds = relations::deltas(&fact(m, me, wide_hours, now), &temper);
                let corp_only: Vec<_> = ds
                    .into_iter()
                    .filter(|d| d.level == relations::Level::Corp)
                    .collect();
                st.apply_deltas(&corp_only);
            }
            if hours > 0.0 {
                credited += hours;
                personal.extend(
                    relations::deltas(&fact(m, me, hours, now), &temper)
                        .into_iter()
                        .filter(|d| d.level == relations::Level::Personal),
                );
            }
        }
        (day, temper)
    };
    if !personal.is_empty() {
        crate::observe::apply_personal(mind, me.character_id, &personal, now, day, &temper);
    }
    credited
}

/// The F5 fact for `hours` flown by `human` beside `bot`.
fn fact(human: &Affiliation, bot: &Affiliation, hours: f64, now: u64) -> Fact {
    Fact {
        kind: FactKind::Fleet,
        actor: *human,
        target: *bot,
        isk: 0.0,
        ctx: Ctx {
            hours,
            ..Ctx::default()
        },
        at: now,
    }
}

// ---- the CEO's turn ------------------------------------------------------------------------

/// **The CEO tells the humans where they stand** (called from the social
/// turn, on the CEO's own connection): for each tier crossing of the bot
/// corp's view, one mail ([`crate::mail::tier_changed`]), at most
/// [`TIER_MAILS_PER_TURN`]. A bot that is not a CEO of a bot corp does
/// nothing.
pub fn turn(conn: &mut Conn, hall: &Hall, mind: &Mutex<Mind>, now: u64) -> Result<Vec<String>> {
    let Some(me) = conn.session().char_id() else {
        return Ok(Vec::new());
    };
    let corp = conn.session().get_i64("corpid").unwrap_or(0);
    if !society::is_player_corp(corp)
        || hall.lock().corps.get(&corp).and_then(|c| c.ceo) != Some(me)
    {
        return Ok(Vec::new());
    }
    let (regard, loyalty) = {
        let m = mind.lock().unwrap_or_else(|e| e.into_inner());
        (m.regard.clone(), m.traits.get(Trait::Loyalty))
    };
    let changes = hall
        .lock()
        .tier_changes(corp, &regard, loyalty, now, TIER_MAILS_PER_TURN);
    let staging = conn.session().solar_system_id2();
    let mut out = Vec::new();
    for c in changes {
        out.push(format!(
            "rel: {} {} -> {} (R {:.1})",
            c.human,
            c.from.name(),
            c.to.name(),
            c.r
        ));
        if let Some(line) = crate::mail::tier_changed(conn, hall, corp, c.human, c.to, staging)? {
            out.push(line);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
