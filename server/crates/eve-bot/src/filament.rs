//! **Needlejack filaments for the hunter** (master plan 3.5, lane
//! `bot-filaments`): a hunter that owns a random-jump filament may spend it
//! to reach new ground, hunt there, and fly home over the gates.
//!
//! # The server's side **[V]** (`eve_server::random_jump`)
//!
//! `randomJumpMgr.ActivateRandomJumpFilament(itemID)` spends a filament in
//! the pilot's **cargo** (group 4041, [`GROUP`]) and, after the type's spool,
//! carries the pilot to a random null-sec system of another region. The call
//! is answered at once; the departure is a `solarsystemid` session change.
//! It refuses a pilot with a PvP timer, a green safety, a capital hull, a
//! pilot in warp, or in wormhole or abyssal space.
//!
//! # What a bot does
//!
//! *Nothing from nowhere:* only a filament already in the bot's hangar or
//! hold (bought, built or looted) is ever used; none is made or bought for
//! this step.
//!
//! 1. **When** ([`want_jump`]): the hunter is bored of home -- it has had
//!    [`bored_after`] quiet sorties in a row (the more curious, the sooner)
//!    -- and is not timid ([`MIN_RISK`]), is no gang member (the gang would be
//!    left behind; the server carries only a fleet inside 6 km), flies no
//!    capital, is not away already and has not been refused [`MAX_FAILS`]
//!    times.
//! 2. **Docked**: the filament is moved to the hold ([`stock_step`], the
//!    server reads the cargo only).
//! 3. **In space, no prey**: it activates ([`jump_step`],
//!    `HuntAct::Filament`).
//! 4. **Away**: it hunts as always. [`go_home_now`] sends it home -- a
//!    `HuntAct::Roam` to the station it left, over the gates -- after a kill,
//!    when it is sent home for any reason (repair, rearm, an empty
//!    system), or when the trip has lasted [`away_budget`] (the bolder, the
//!    longer). A bot that ends up docked elsewhere flies home from there
//!    ([`return_leg`]).
//!
//! Every number is **[A]** policy; falsifier: a hunter fleet that never uses
//! its filaments, or one that strands itself in null for hours.

use crate::brain::{Act, Memory, Observation, Place};
use crate::verbs::ItemRow;
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use std::time::{Duration, Instant};

/// `randomJump` key group **[V-data]** (`eve_server::random_jump`).
pub const GROUP: i64 = eve_server::random_jump::GROUP_RANDOM_JUMP_KEYS;

/// **[A]** The Risk below which a bot never jumps into unknown null.
pub const MIN_RISK: f64 = 0.35;
/// **[A]** Quiet sorties that bore the least curious hunter, and the most.
pub const BORED_MAX: u32 = 4;
pub const BORED_MIN: u32 = 1;
/// **[A]** Refusals (or failed spools) in a row after which a bot stops.
pub const MAX_FAILS: u32 = 2;
/// **[A]** A trip lasts at least this long, whatever the nerve...
pub const AWAY_MIN: Duration = Duration::from_secs(20 * 60);
/// ...and this much longer for the most reckless.
pub const AWAY_EXTRA: Duration = Duration::from_secs(40 * 60);
/// **[A]** How long the call waits for the departure: the longest spool is
/// 60 s **[V-data]**; the rest is the departure's own delay.
pub const SPOOL_PATIENCE: Duration = Duration::from_secs(150);

/// The traits a filament step reads besides the hunter's `Temper`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mood {
    /// `homebody ... explorer`, `[0, 1]`.
    pub curiosity: f64,
    /// `content ... greedy`, `[0, 1]`.
    pub greed: f64,
}

impl Default for Mood {
    fn default() -> Mood {
        Mood {
            curiosity: 0.5,
            greed: 0.5,
        }
    }
}

/// A trip into new space: where it started, and when.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Away {
    /// The station to fly home to.
    pub home: i64,
    pub since: Instant,
    /// The hunter's kill count on departure.
    pub kills: u32,
}

/// The filament in `rows`, if any.
pub fn held(rows: &[ItemRow]) -> Option<&ItemRow> {
    rows.iter().find(|r| r.group_id == GROUP && r.quantity > 0)
}

/// Quiet sorties it takes to bore a hunter of this curiosity: from
/// [`BORED_MAX`] (a homebody) down to [`BORED_MIN`] (an explorer).
pub fn bored_after(curiosity: f64) -> u32 {
    let c = curiosity.clamp(0.0, 1.0);
    let span = f64::from(BORED_MAX - BORED_MIN);
    BORED_MAX - (span * c).round() as u32
}

/// What [`want_jump`] reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brief {
    pub risk: f64,
    pub curiosity: f64,
    pub quiet_sorties: u32,
    pub gang: bool,
    pub away: bool,
    pub capital: bool,
    pub failed: u32,
}

/// Whether this bot should spend a filament it holds, now.
pub fn want_jump(b: &Brief) -> bool {
    !b.gang
        && !b.away
        && !b.capital
        && b.failed < MAX_FAILS
        && b.risk >= MIN_RISK
        && b.quiet_sorties >= bored_after(b.curiosity)
}

fn brief(m: &Memory, o: &Observation) -> Brief {
    Brief {
        risk: o.hunt.temper.risk,
        curiosity: o.hunt.mood.curiosity,
        quiet_sorties: m.hunt.quiet_sorties,
        gang: m.policy.gang.is_some(),
        away: m.hunt.away.is_some(),
        capital: o
            .ship
            .and_then(|s| s.type_id)
            .zip(m.policy.pvp.as_ref())
            .is_some_and(|(t, d)| d.class_of(Some(t)) == crate::hostile::Class::Capital),
        failed: m.hunt.fil_failed,
    }
}

/// How long a trip may last for a hunter of this nerve.
pub fn away_budget(risk: f64) -> Duration {
    AWAY_MIN + AWAY_EXTRA.mul_f64(risk.clamp(0.0, 1.0))
}

/// Whether an away hunter should fly home now: it killed something, it is
/// sent home for any reason, or the trip is long enough; and a way home is
/// still worth trying.
pub fn go_home_now(
    a: &Away,
    now: Instant,
    kills: u32,
    homing: bool,
    roam_failed: u32,
    risk: f64,
) -> bool {
    roam_failed < crate::hunt::ROAM_TRIES
        && (homing
            || kills > a.kills
            || now.saturating_duration_since(a.since) >= away_budget(risk))
}

/// Docked while away from the station it left: the way home, over the gates.
pub fn return_leg(a: &Away, here: i64, roam_failed: u32) -> Option<Act> {
    (here != a.home && roam_failed < crate::hunt::ROAM_TRIES)
        .then_some(Act::Hunt(crate::hunt::HuntAct::Roam { station: a.home }))
}

/// Docked and ready: put the filament in the hold. `None` when none is
/// wanted, held, or it is in the hold already.
pub fn stock_step(m: &Memory, o: &Observation) -> Option<Act> {
    if !want_jump(&brief(m, o)) || held(&o.cargo).is_some() {
        return None;
    }
    let f = held(&o.hangar)?;
    Some(Act::Stock {
        item: f.item_id,
        quantity: 1,
    })
}

/// In space with no prey: activate the filament in the hold.
pub fn jump_step(m: &Memory, o: &Observation, ground: bool) -> Option<Act> {
    if !ground || m.homing.is_some() || !want_jump(&brief(m, o)) {
        return None;
    }
    let f = held(&o.cargo)?;
    Some(Act::Hunt(crate::hunt::HuntAct::Filament {
        item: f.item_id,
    }))
}

/// The leg home from space, when due.
pub fn home_step(m: &Memory, o: &Observation) -> Option<Act> {
    let a = m.hunt.away?;
    go_home_now(
        &a,
        o.now,
        m.hunt.kills,
        m.homing.is_some(),
        m.hunt.roam_failed,
        o.hunt.temper.risk,
    )
    .then_some(Act::Hunt(crate::hunt::HuntAct::Roam { station: a.home }))
}

/// The leg home from a dock elsewhere.
pub fn dock_home_step(m: &Memory, o: &Observation) -> Option<Act> {
    let Place::Docked { station } = o.place else {
        return None;
    };
    return_leg(&m.hunt.away?, station, m.hunt.roam_failed)
}

impl Conn {
    /// **Spend a filament**: `randomJumpMgr.ActivateRandomJumpFilament(item)`
    /// (`ActivateRandomJumpFilament` **[V]**, the answer is discarded by the
    /// client). The departure is a `solarsystemid` session change after the
    /// spool; the park is bound again in the new system.
    pub fn activate_filament(&mut self, item: i64) -> Result<i64> {
        let from = self.session().solar_system_id();
        let mut outcome = self.call(
            "randomJumpMgr",
            "ActivateRandomJumpFilament",
            vec![PyValue::Int(item)],
        )?;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!(
                "ActivateRandomJumpFilament({item}): {key}"
            )));
        }
        self.await_session_change(&mut outcome, SPOOL_PATIENCE)?;
        let now = self.session().solar_system_id();
        match now {
            Some(system) if outcome.session_changed && now != from => {
                self.bind_ballpark(system)?;
                Ok(system)
            }
            _ => Err(Error::Refused(format!(
                "ActivateRandomJumpFilament({item}): no solarsystemid session change"
            ))),
        }
    }
}

/// The text the act log shows.
pub fn describe(item: i64) -> (String, Vec<i64>) {
    (
        format!("randomJumpMgr.ActivateRandomJumpFilament {item}"),
        vec![item],
    )
}

#[cfg(test)]
mod tests {
    //! Invented ids and scenes; the constants by name.
    use super::*;

    fn bored() -> Brief {
        Brief {
            risk: 0.8,
            curiosity: 0.9,
            quiet_sorties: BORED_MAX,
            gang: false,
            away: false,
            capital: false,
            failed: 0,
        }
    }

    #[test]
    fn a_bored_bold_lone_hunter_jumps() {
        assert!(want_jump(&bored()));
    }

    #[test]
    fn the_timid_the_ganged_the_away_and_the_refused_stay() {
        for b in [
            Brief {
                risk: MIN_RISK / 2.0,
                ..bored()
            },
            Brief {
                gang: true,
                ..bored()
            },
            Brief {
                away: true,
                ..bored()
            },
            Brief {
                capital: true,
                ..bored()
            },
            Brief {
                failed: MAX_FAILS,
                ..bored()
            },
        ] {
            assert!(!want_jump(&b), "{b:?}");
        }
    }

    #[test]
    fn the_more_curious_tire_of_home_sooner() {
        assert!(bored_after(1.0) < bored_after(0.0));
        assert_eq!(bored_after(0.0), BORED_MAX);
        assert_eq!(bored_after(1.0), BORED_MIN);
        let b = Brief {
            quiet_sorties: BORED_MIN,
            ..bored()
        };
        assert!(want_jump(&Brief {
            curiosity: 1.0,
            ..b
        }));
        assert!(!want_jump(&Brief {
            curiosity: 0.0,
            ..b
        }));
    }

    #[test]
    fn the_bolder_stay_away_longer() {
        assert!(away_budget(0.9) > away_budget(0.1));
    }

    fn away(now: Instant) -> Away {
        Away {
            home: 60_000_001,
            since: now,
            kills: 2,
        }
    }

    #[test]
    fn an_away_hunter_goes_home_after_a_kill_a_retreat_or_its_budget() {
        let t0 = Instant::now();
        let a = away(t0);
        let risk = 0.5;
        assert!(!go_home_now(&a, t0, 2, false, 0, risk));
        assert!(go_home_now(&a, t0, 3, false, 0, risk));
        assert!(go_home_now(&a, t0, 2, true, 0, risk));
        assert!(!go_home_now(
            &a,
            t0 + away_budget(risk) / 2,
            2,
            false,
            0,
            risk
        ));
        assert!(go_home_now(&a, t0 + away_budget(risk), 2, false, 0, risk));
        // The way home failed too often: it stays where it is.
        assert!(!go_home_now(&a, t0, 3, true, crate::hunt::ROAM_TRIES, risk));
    }

    #[test]
    fn docked_away_the_way_home_is_the_roam_to_the_old_station() {
        let a = away(Instant::now());
        assert_eq!(
            return_leg(&a, 60_000_777, 0),
            Some(Act::Hunt(crate::hunt::HuntAct::Roam { station: a.home }))
        );
        assert_eq!(return_leg(&a, a.home, 0), None);
    }

    fn row(item: i64, group: i64, flag: i64) -> ItemRow {
        ItemRow {
            item_id: item,
            type_id: 900_001,
            owner_id: 1,
            location_id: 60_000_001,
            flag_id: flag,
            quantity: 1,
            group_id: group,
            category_id: 5,
            singleton: false,
        }
    }

    #[test]
    fn only_a_key_group_item_is_a_filament() {
        let rows = [row(1, GROUP + 1, 4), row(2, GROUP, 4)];
        assert_eq!(held(&rows).map(|r| r.item_id), Some(2));
        assert_eq!(held(&rows[..1]), None);
    }
}
