//! **Go to a character, wherever he is** (plan-fc-follow §2.2, FC-1): the one
//! pure step shared by everything that follows or answers a person -- the
//! wing that trails its FC ([`crate::wing`]) and, later, a `/bots come` that
//! sends a bot to a caller in space.
//!
//! [`step`] reads where the bot is ([`Here`]), where the character is
//! ([`Whereabouts`]) and the gate [`Map`], and returns the *one* next
//! [`Step`]; the caller turns it into an act, and asks again next turn.
//!
//! | Situation | Step |
//! |---|---|
//! | docked in his station | [`Step::Stay`] |
//! | docked elsewhere | [`Step::Undock`] |
//! | in warp or between sessions | [`Step::Wait`] |
//! | in another system, a route | [`Step::Jump`] (the first hop) |
//! | in another system, no route | [`Step::NoRoute`] |
//! | same system, he is docked | [`Step::Dock`] |
//! | same system, he is in space off this grid | [`Step::WarpToMember`] |
//! | on this grid, beyond `range + KEEP_SLACK` | [`Step::Keep`] |
//! | on this grid, close | [`Step::Arrived`] |
//!
//! Pure: no clock, no connection. The caller owns the back-off for a warp
//! the server refused.

use crate::brain::Ball;
use crate::travel::Map;

/// How far past `range` the character may drift on the grid before a bot
/// closes in again, metres **[A]**: a follow that is re-sent at every wobble
/// is act-log noise. Falsified by a wing that trails visibly farther than
/// its anchor range (shrink it).
pub const KEEP_SLACK: f64 = 1_000.0;

/// Where the bot is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Here {
    /// The system the bot is in (docked: its station's system, `0` when the
    /// map does not know it -- a docked bot's system is never compared).
    pub system: i64,
    /// The station docked in.
    pub station: Option<i64>,
    /// Out in a park (not docked, not between sessions).
    pub in_space: bool,
    /// The ship is in warp.
    pub warping: bool,
    /// The bot's own ball, when it is in a park.
    pub ego_ball: Option<Ball>,
}

/// Where the character is, as far as the step needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Whereabouts {
    pub char_id: i64,
    pub system: i64,
    /// The station he is docked in; `None`: in space.
    pub station: Option<i64>,
    /// His ship, when it is a ball in this bot's park (his grid).
    pub ship_ball_on_grid: Option<Ball>,
}

/// The next thing to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Docked where he is docked: stay.
    Stay,
    /// Docked elsewhere: leave.
    Undock,
    /// Move toward `gate` (in this system) and jump it to `to`; `partner` is
    /// the gate it arrives at.
    Jump { gate: i64, partner: i64, to: i64 },
    /// No route at the floor: say so once and hold.
    NoRoute,
    /// He is docked in this system: fly to `station` and dock.
    Dock { station: i64 },
    /// He is in space in this system but not on this grid.
    WarpToMember { char_id: i64 },
    /// On his grid, too far: close to `range` of `ball`.
    Keep { ball: i64, range: f64 },
    /// On his grid, within range.
    Arrived,
    /// Let the warp land / the session settle.
    Wait,
}

/// **The next step toward `target`** (pure), routing over `map` at `floor`
/// and keeping `range` metres (surface to surface) off him on his grid.
pub fn step(here: &Here, target: &Whereabouts, map: &Map, floor: f64, range: f64) -> Step {
    if let Some(station) = here.station {
        return if target.station == Some(station) {
            Step::Stay
        } else {
            Step::Undock
        };
    }
    if !here.in_space || here.warping {
        return Step::Wait;
    }
    if here.system != target.system {
        let hop = map
            .route(here.system, target.system, floor)
            .filter(|r| r.len() >= 2)
            .and_then(|r| {
                map.gate(here.system, r[1])
                    .map(|(gate, partner)| (gate, partner, r[1]))
            });
        return match hop {
            Some((gate, partner, to)) => Step::Jump { gate, partner, to },
            None => Step::NoRoute,
        };
    }
    if let Some(station) = target.station {
        return Step::Dock { station };
    }
    match (&target.ship_ball_on_grid, &here.ego_ball) {
        (None, _) => Step::WarpToMember {
            char_id: target.char_id,
        },
        (Some(ball), Some(ego)) if ego.surface_to(ball) > range + KEEP_SLACK => Step::Keep {
            ball: ball.id,
            range,
        },
        (Some(_), _) => Step::Arrived,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // An invented line of three systems: 1 -- 2 -- 3, and an island 9.
    const S1: i64 = 1;
    const S2: i64 = 2;
    const S3: i64 = 3;
    const ISLAND: i64 = 9;
    const ST1: i64 = 61;
    const ST1B: i64 = 62;
    const ST3: i64 = 63;
    const WHO: i64 = 7;

    fn map() -> Map {
        Map::new()
            .with_gate(S1, 101, S2, 102)
            .with_gate(S2, 102, S1, 101)
            .with_gate(S2, 103, S3, 104)
            .with_gate(S3, 104, S2, 103)
            .with_station(ST1, S1)
            .with_station(ST1B, S1)
            .with_station(ST3, S3)
            .with_security(S1, 0.9)
            .with_security(S2, -0.5)
            .with_security(S3, -0.8)
    }

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    fn docked(station: i64) -> Here {
        Here {
            system: S1,
            station: Some(station),
            in_space: false,
            warping: false,
            ego_ball: None,
        }
    }

    fn flying(system: i64, x: f64) -> Here {
        Here {
            system,
            station: None,
            in_space: true,
            warping: false,
            ego_ball: Some(ball(1, x)),
        }
    }

    fn who(system: i64, station: Option<i64>, ship: Option<Ball>) -> Whereabouts {
        Whereabouts {
            char_id: WHO,
            system,
            station,
            ship_ball_on_grid: ship,
        }
    }

    const ANY: f64 = crate::travel::ANY_SECURITY;
    const RANGE: f64 = 2_000.0;

    #[test]
    fn docked_in_his_station_stays_and_docked_anywhere_else_undocks() {
        let m = map();
        assert_eq!(
            step(&docked(ST1), &who(S1, Some(ST1), None), &m, ANY, RANGE),
            Step::Stay
        );
        // He is in space, or docked in another station, or in another system.
        for t in [
            who(S1, None, None),
            who(S1, Some(ST1B), None),
            who(S3, Some(ST3), None),
        ] {
            assert_eq!(
                step(&docked(ST1), &t, &m, ANY, RANGE),
                Step::Undock,
                "{t:?}"
            );
        }
    }

    #[test]
    fn a_warp_or_a_session_change_waits() {
        let m = map();
        let mut h = flying(S1, 0.0);
        h.warping = true;
        assert_eq!(step(&h, &who(S1, None, None), &m, ANY, RANGE), Step::Wait);
        h.warping = false;
        h.in_space = false;
        assert_eq!(step(&h, &who(S1, None, None), &m, ANY, RANGE), Step::Wait);
    }

    #[test]
    fn another_system_is_the_first_hop_of_the_route() {
        let m = map();
        let t = who(S3, None, None);
        assert_eq!(
            step(&flying(S1, 0.0), &t, &m, ANY, RANGE),
            Step::Jump {
                gate: 101,
                partner: 102,
                to: S2
            }
        );
        assert_eq!(
            step(&flying(S2, 0.0), &t, &m, ANY, RANGE),
            Step::Jump {
                gate: 103,
                partner: 104,
                to: S3
            }
        );
    }

    #[test]
    fn no_route_and_a_floor_that_forbids_it_are_no_route() {
        let m = map();
        assert_eq!(
            step(&flying(S1, 0.0), &who(ISLAND, None, None), &m, ANY, RANGE),
            Step::NoRoute
        );
        // The route to system 3 passes systems below a high-sec floor.
        assert_eq!(
            step(&flying(S1, 0.0), &who(S3, None, None), &m, 0.45, RANGE),
            Step::NoRoute
        );
    }

    #[test]
    fn he_docked_in_this_system_is_a_dock() {
        let m = map();
        assert_eq!(
            step(&flying(S1, 0.0), &who(S1, Some(ST1B), None), &m, ANY, RANGE),
            Step::Dock { station: ST1B }
        );
    }

    #[test]
    fn off_grid_is_a_warp_and_on_grid_is_keep_or_arrived() {
        let m = map();
        let h = flying(S1, 0.0);
        assert_eq!(
            step(&h, &who(S1, None, None), &m, ANY, RANGE),
            Step::WarpToMember { char_id: WHO }
        );
        // Far: close in on his ball.
        let far = who(S1, None, Some(ball(5, RANGE + KEEP_SLACK + 500.0)));
        assert_eq!(
            step(&h, &far, &m, ANY, RANGE),
            Step::Keep {
                ball: 5,
                range: RANGE
            }
        );
        // Close: arrived.
        let near = who(S1, None, Some(ball(5, 600.0)));
        assert_eq!(step(&h, &near, &m, ANY, RANGE), Step::Arrived);
    }
}
