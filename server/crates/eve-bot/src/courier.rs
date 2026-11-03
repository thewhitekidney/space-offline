//! **The courier** (W15 packet CP34): a bot that takes a courier mission
//! from an agent, hauls the cargo and turns the mission in.
//!
//! [`run`] is the whole job on one [`Conn`], every step a player verb
//! (`verbs.rs`): talk to the agent and press Accept; read the transport leg
//! from the objective document; go to the pickup station, move the cargo
//! from the hangar into the hold; go to the drop-off station, move it into
//! the hangar; talk to the agent again and press Complete. The way between
//! two stations is [`next_move`], pure, so its rules are tested as a table.
//!
//! [`ROLE`] is the `--loop courier` row of [`crate::roles::ROLES`]. Docked
//! in a hull, it asks for [`Act::Courier`] on the policy's agent
//! (`eve-bots --agent ID`), whose driver arm (CP39) is one [`run`], then
//! dwells; with no agent it says `MISSING` ([`MISSING_AGENT_ACT`]).
//!
//! **Every rule here is [A] policy** -- what a player would do -- except the
//! client's own gates, which the verbs enforce and cite.

use crate::brain::{self, Act, Memory, Observation};
use crate::roles::{Lacks, RoleSpec, Summary};
pub use crate::verbs::{
    AgentTurn, BUTTON_ACCEPT, BUTTON_COMPLETE, BUTTON_DECLINE, BUTTON_QUIT, BUTTON_REQUEST_MISSION,
    BUTTON_VIEW_MISSION, CourierLeg,
};
use crate::{Conn, DOCK_RANGE, Error, MIN_WARP_DISTANCE, Result};
use std::time::{Duration, Instant};

/// `const.categoryShip` **[V-data]**: a hangar row that can be boarded.
pub const CATEGORY_SHIP: i64 = 6;

/// The `MISSING` line of the `--loop courier` row with no agent to haul for.
pub const MISSING_AGENT_ACT: &str = "an agent to haul for (eve-bots --agent ID)";

/// The most moves [`run`] makes to reach one station before it gives up.
/// **[A]**: a same-system trip is at most undock, bind, warp, approach, dock.
pub const MAX_MOVES: usize = 12;

/// How long a warp or an approach may take before it counts as stuck.
/// **[A]**, a ceiling, not a duration.
pub const TRAVEL_CEILING: Duration = Duration::from_secs(120);

/// How long [`run`] waits for each burst while a warp or approach runs.
const POLL: Duration = Duration::from_millis(250);

/// Hauls, and stays out of fights.
pub static ROLE: RoleSpec = RoleSpec {
    name: "courier",
    greeting: "o7 -- hauling, not fighting",
    civilian: true,
    docked,
    in_space,
    banner,
    setup,
};

/// Docked: a hull first (a capsule has no hold), then a run for the
/// policy's agent and a dwell -- see the module header.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    let active = o.ship.map(|s| s.ship_id);
    let in_capsule = o
        .ship
        .is_none_or(|s| s.type_id.is_none_or(|t| t == brain::CAPSULE));
    if in_capsule {
        if let Some(hull) = o.hangar.iter().find(|r| {
            is_hull(r.category_id, r.type_id)
                && Some(r.item_id) != active
                && !m.combat.untrained.contains(&r.type_id)
        }) {
            return vec![Act::Board(hull.item_id)];
        }
        return brain::gm_or_missing(m, true, false, false, "a hull in the hangar");
    }
    let Some(agent) = m.policy.agent else {
        return vec![
            Act::Missing(MISSING_AGENT_ACT),
            Act::Wait(brain::MISSING_WAIT),
        ];
    };
    let rest = m.jitter(m.policy.dwell);
    vec![Act::Courier { agent }, Act::Wait(rest)]
}

/// In space: home, the miner's way ([`brain::go_home`]).
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    match &o.park {
        Some(park) => brain::go_home(m, o, park),
        None => vec![Act::Wait(brain::SHORT_WAIT)],
    }
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, reconnects {}",
        s.name, s.phase, s.reconnects
    )
}

/// A Merlin when the hangar has no hull: 150 m3 of hold **[A]** (the hull
/// the other roles' setup already makes).
fn setup(lacks: Lacks) -> Vec<String> {
    if lacks.ship {
        vec!["/createitem merlin".to_owned()]
    } else {
        Vec::new()
    }
}

fn is_hull(category_id: i64, type_id: i64) -> bool {
    category_id == CATEGORY_SHIP && type_id != brain::CAPSULE
}

/// Where the courier is, as far as the next move needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Here {
    /// The station docked in.
    pub docked_at: Option<i64>,
    /// In space with the ballpark bound.
    pub bound: bool,
    /// The ship's surface distance to the target station, when both are in
    /// the park.
    pub gap: Option<f64>,
    /// The ship is in warp.
    pub warping: bool,
}

/// One step toward a station.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Arrived,
    Undock,
    Bind,
    /// Still in warp: let it land.
    Wait,
    Warp,
    Approach,
    Dock,
    /// The station is not in this park: another system. Gates are not
    /// routed yet.
    NotHere,
}

/// **The way to `target`**, one move at a time. Docked there: arrived;
/// docked elsewhere: undock; in space: bind, then warp from beyond
/// [`MIN_WARP_DISTANCE`] (the client's own floor), dock inside
/// [`DOCK_RANGE`] (the client's own gate) and approach in between.
pub fn next_move(here: &Here, target: i64) -> Move {
    match here.docked_at {
        Some(s) if s == target => return Move::Arrived,
        Some(_) => return Move::Undock,
        None => {}
    }
    if !here.bound {
        return Move::Bind;
    }
    if here.warping {
        return Move::Wait;
    }
    match here.gap {
        None => Move::NotHere,
        Some(g) if g >= MIN_WARP_DISTANCE => Move::Warp,
        Some(g) if g < DOCK_RANGE => Move::Dock,
        Some(_) => Move::Approach,
    }
}

/// What a finished [`run`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub leg: CourierLeg,
    /// Units moved hangar -> hold at the pickup.
    pub loaded: i64,
    /// Units moved hold -> hangar at the drop-off.
    pub unloaded: i64,
    /// Every move made on the way, in order.
    pub moves: Vec<Move>,
    /// `lastActionInfo['missionCompleted']` of the Complete.
    pub completed: bool,
}

/// **Take a courier mission from `agent`, haul it and turn it in.**
/// Docked, with a hull in the hangar or already boarded. An agent whose
/// conversation already shows Quit has a mission accepted, and [`run`]
/// carries on with it.
pub fn run(c: &mut Conn, agent: i64) -> Result<Delivery> {
    if c.session().station_id().is_none() {
        return Err(Error::NotSent("courier: not docked".to_owned()));
    }
    board_a_hull(c)?;

    let turn = c.talk_to_agent(agent)?;
    if turn.action_for(BUTTON_ACCEPT).is_some() {
        c.agent_button(agent, &turn, BUTTON_ACCEPT)?;
    } else if turn.action_for(BUTTON_QUIT).is_none() {
        return Err(Error::Refused(format!(
            "courier: agent {agent} offers no mission ({:?})",
            turn.actions
        )));
    }
    let objective = c.mission_objective(agent)?;
    let leg = CourierLeg::from_objective(&objective).ok_or_else(|| {
        Error::Refused(format!(
            "courier: agent {agent}'s mission has no transport leg"
        ))
    })?;

    let mut moves = travel(c, leg.pickup_station)?;
    let loaded = load(c, &leg)?;
    moves.extend(travel(c, leg.dropoff_station)?);
    let mut unloaded = 0;
    for row in c.cargo()? {
        if row.type_id == leg.cargo_type {
            c.unload(row.item_id, None)?;
            unloaded += row.quantity;
        }
    }

    let turn = c.talk_to_agent(agent)?;
    let done = c.agent_button(agent, &turn, BUTTON_COMPLETE)?;
    Ok(Delivery {
        leg,
        loaded,
        unloaded,
        moves,
        completed: done.completed,
    })
}

/// Out of the capsule: board the first hull in the hangar.
fn board_a_hull(c: &mut Conn) -> Result<()> {
    let hangar = c.hangar_items()?;
    let active = c.session().ship_id();
    if hangar
        .iter()
        .any(|r| Some(r.item_id) == active && is_hull(r.category_id, r.type_id))
    {
        return Ok(());
    }
    // SKILLGATE: a hull the pilot's skills do not board is no hull yet.
    let levels = crate::acquire::gate_levels(c)?;
    let Some(hull) = hangar.iter().find(|r| {
        is_hull(r.category_id, r.type_id) && crate::acquire::flyable_in(&levels, r.type_id)
    }) else {
        if let (Some(r), Some(l)) = (
            hangar.iter().find(|r| is_hull(r.category_id, r.type_id)),
            levels.as_ref(),
        ) {
            let missing =
                crate::acquire::learn_urgent_soft(c, &crate::acquire::lacking_of(l, r.type_id))?;
            return Err(Error::NotSent(format!(
                "courier: no hull in the hangar it can fly{}",
                crate::acquire::missing_note(&missing)
            )));
        }
        return Err(Error::NotSent(
            "courier: no hull in the hangar it can fly".to_owned(),
        ));
    };
    c.board(hull.item_id)?;
    Ok(())
}

/// The leg's cargo, hangar -> hold, stack by stack, up to its quantity.
fn load(c: &mut Conn, leg: &CourierLeg) -> Result<i64> {
    let mut need = leg.quantity;
    for row in c.hangar_items()? {
        if need == 0 {
            break;
        }
        if row.type_id != leg.cargo_type {
            continue;
        }
        let take = row.quantity.min(need);
        c.load_cargo(row.item_id, (take < row.quantity).then_some(take))?;
        need -= take;
    }
    if need > 0 {
        return Err(Error::Refused(format!(
            "courier: {need} of type {} missing from the hangar",
            leg.cargo_type
        )));
    }
    Ok(leg.quantity)
}

/// Move until docked at `target`, through [`next_move`].
fn travel(c: &mut Conn, target: i64) -> Result<Vec<Move>> {
    let mut moves = Vec::new();
    for _ in 0..MAX_MOVES {
        let here = here(c, target);
        let step = next_move(&here, target);
        moves.push(step);
        match step {
            Move::Arrived => return Ok(moves),
            Move::Undock => {
                c.undock_repaired()?;
            }
            Move::Bind => {
                let system = c
                    .session()
                    .solar_system_id()
                    .ok_or_else(|| Error::NotSent("courier: in space with no system".into()))?;
                c.bind_ballpark(system)?;
                settle(c, |c| c.world().has_state())?;
            }
            Move::Wait => settle(c, |c| !warping(c))?,
            Move::Warp => {
                c.warp_to_item(target, 0.0)?;
                settle(c, warping)?;
                settle(c, |c| !warping(c))?;
            }
            Move::Approach => {
                c.follow(target, 0.0)?;
                settle(c, |c| gap(c, target).is_some_and(|g| g < DOCK_RANGE))?;
            }
            Move::Dock => {
                c.dock(target)?;
                if let Some(other) = c.session().station_id().filter(|s| *s != target) {
                    return Err(Error::Refused(format!(
                        "courier: CmdDock({target}) docked the ship at {other}"
                    )));
                }
            }
            Move::NotHere => {
                return Err(Error::NotSent(format!(
                    "courier: station {target} is not in this system's park"
                )));
            }
        }
    }
    Err(Error::Refused(format!(
        "courier: not docked at {target} after {MAX_MOVES} moves ({moves:?})"
    )))
}

fn here(c: &mut Conn, target: i64) -> Here {
    Here {
        docked_at: c.session().station_id(),
        bound: c.ballpark().is_some() && c.world().has_state(),
        gap: gap(c, target),
        warping: warping(c),
    }
}

fn gap(c: &mut Conn, target: i64) -> Option<f64> {
    let world = c.world_now();
    world.surface_distance(world.ego?, target)
}

fn warping(c: &mut Conn) -> bool {
    c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
}

/// Read pushes until `done` holds or [`TRAVEL_CEILING`] passes.
fn settle(c: &mut Conn, mut done: impl FnMut(&mut Conn) -> bool) -> Result<()> {
    let deadline = Instant::now() + TRAVEL_CEILING;
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(Error::Refused(
                "courier: still moving after the travel ceiling".into(),
            ));
        }
        c.pump(POLL)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: i64 = 61_000_001;
    const AWAY: i64 = 61_000_002;

    fn space(gap: Option<f64>) -> Here {
        Here {
            docked_at: None,
            bound: true,
            gap,
            warping: false,
        }
    }

    /// The way between two stations, as a table: each row is one state and
    /// the move it calls for. The distances are the client's own gates.
    #[test]
    fn the_way_to_a_station_is_undock_bind_warp_approach_dock() {
        let docked = |s| Here {
            docked_at: Some(s),
            bound: false,
            gap: None,
            warping: false,
        };
        let unbound = Here {
            bound: false,
            ..space(None)
        };
        let in_warp = Here {
            warping: true,
            ..space(Some(MIN_WARP_DISTANCE * 2.0))
        };
        let between = (DOCK_RANGE + MIN_WARP_DISTANCE) / 2.0;
        let rows = [
            (docked(AWAY), Move::Arrived),
            (docked(HOME), Move::Undock),
            (unbound, Move::Bind),
            (in_warp, Move::Wait),
            (space(None), Move::NotHere),
            (space(Some(MIN_WARP_DISTANCE)), Move::Warp),
            (space(Some(between)), Move::Approach),
            (space(Some(DOCK_RANGE)), Move::Approach),
            (space(Some(0.0)), Move::Dock),
        ];
        for (here, want) in rows {
            assert_eq!(next_move(&here, AWAY), want, "{here:?}");
        }
    }

    #[test]
    fn the_courier_is_a_civilian_row() {
        assert!(ROLE.civilian);
        assert_eq!(ROLE.name(), "courier");
    }

    /// Docked in a hull, the row hauls for its agent, or says what it lacks
    /// rather than undocking with nothing to do; in a capsule it boards a
    /// hull first.
    #[test]
    fn docked_it_boards_a_hull_then_says_missing() {
        use crate::brain::{Place, Policy, ShipInfo};
        let mut m = Memory::new("Courier1", Policy::fast());
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: HOME };
        o.ship = Some(ShipInfo {
            ship_id: 1,
            type_id: Some(brain::CAPSULE),
        });
        let hull = crate::verbs::ItemRow {
            item_id: 2,
            type_id: brain::MERLIN,
            owner_id: 0,
            location_id: HOME,
            flag_id: crate::FLAG_HANGAR,
            quantity: 1,
            group_id: 0,
            category_id: CATEGORY_SHIP,
            singleton: true,
        };
        o.hangar = vec![hull];
        assert_eq!(docked(&mut m, &o), vec![Act::Board(2)]);
        o.ship = Some(ShipInfo {
            ship_id: 2,
            type_id: Some(brain::MERLIN),
        });
        assert_eq!(
            docked(&mut m, &o)[0],
            Act::Missing(MISSING_AGENT_ACT),
            "no agent to haul for"
        );
        m.policy.agent = Some(3_000_001);
        assert_eq!(docked(&mut m, &o)[0], Act::Courier { agent: 3_000_001 });
    }
}
