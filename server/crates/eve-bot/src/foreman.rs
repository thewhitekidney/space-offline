//! **The mining foreman** (PROG-MINE): an Orca (or Porpoise, or Rorqual)
//! pilot who boosts a fleet of miners and compresses their ore. Lane note
//! `docs/lanes/prog-mine.md`.
//!
//! A player who has trained into an industrial command ship flies it to the
//! belt, fits a Mining Foreman Burst and a compressor, makes a fleet of the
//! miners there and keeps the burst running, so each laser in range cuts
//! faster (K-BOOST, `eve-server/src/dbuff.rs`), and the miners compress their
//! ore at the foreman's compressor when their hold fills
//! ([`crate::ore::OreAct::Compress`], `eve-server/src/compression.rs`).
//!
//! - **Outfit** (docked): board the command hull the pilot owns; fit, online
//!   and load the burst ([`crate::burst::step`]); fit and online the hull's
//!   compressor ([`compressor_for`]). A missing part is named
//!   ([`Turn::Lacks`]) for the closed economy's own acquire path
//!   ([`KIT`]) to buy; **nothing is made from nowhere**, so a foreman exists
//!   only where a builder made the hull or a player handed it over.
//! - **The belt** (in space): warp to a belt, wait for miners on the grid,
//!   make a fleet and invite them ([`invitees`]); a fleet boss is a miner's
//!   rule b (`brain::in_space`: a boss off the grid is warped to), so the
//!   miners already in the fleet follow him.
//! - **The boost**: run the burst ([`crate::burst::step`] answers `Run`).
//!
//! One role act, [`Turn`]: the role's rules ([`docked_core`], [`in_space_core`]) only
//! pace it; the decision is [`decide`] (pure) over [`Facts`], the
//! executor [`turn`] reads the facts and does one thing.
//!
//! **[A]** (all policy, falsified by the soak's boost ledger): one belt at a
//! time, [`BELT_PATIENCE`] turns waiting for miners before trying the next;
//! invite everyone on the grid who flies a mining hull
//! ([`crate::mine_ladder::hull_of`]), up to [`INVITES_PER_TURN`] a turn; the
//! burst's range is 15 km ([V-data] attribute 54), so a miner far across a
//! belt is not boosted.

use crate::acquire::Kit;
use crate::brain::{Act, Memory, Observation};
use crate::burst::{self, Lack};
use crate::mine_ladder::{ORCA, PORPOISE, RORQUAL};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::{Conn, Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// Mining Foreman Burst I **[V-data]**: group 1770, effect 6736, charge
/// group 1771; asks Leadership I and Mining Foreman I.
pub const BURST_MINING_I: i64 = 42_528;
/// Mining Laser Optimization Charge **[V-data]**: the charge that cuts the
/// lasers' cycle (collection 24, -15 %), the one every boosted laser
/// wants. **[A]** that it is the one a foreman loads; the field, preservation
/// and efficiency charges are the other three of group 1771.
pub const CHARGE_OPTIMIZATION: i64 = 42_830;
/// Charges the kit buys **[A]**: one a minute-long cycle, an hour of boost.
pub const KIT_CHARGES: i64 = 60;
/// Large Asteroid Ore Compressor I (the Orca's), Medium (the Porpoise's),
/// Capital (the Rorqual's) **[V-data]**: group 4174, effect 8364; each takes
/// the asteroid-ore typelist 334 and fits only its hull (`canFitShipType1`).
pub const COMPRESSOR_LARGE: i64 = 62_625;
pub const COMPRESSOR_MEDIUM: i64 = 62_622;
pub const COMPRESSOR_CAPITAL: i64 = 62_632;
/// The compressors' group **[V-data]**.
pub const GROUP_COMPRESSOR: i64 = eve_server::compression::GROUP_COMPRESSOR;

/// What a foreman needs bought: the Orca, the burst and its charges, the
/// compressor, and the two command skills the burst asks. The hull is a
/// builder's product at hundreds of millions of ISK; in a closed world no
/// bot can afford it (docs/lanes/prog-mine.md), so the ask is a standing
/// demand and a foreman exists when one is handed over or built.
pub const KIT: Kit = Kit {
    hull: Some(ORCA),
    modules: Some((BURST_MINING_I, 1)),
    ammo: Some((CHARGE_OPTIMIZATION, KIT_CHARGES)),
    skills: &[
        (crate::mine_ladder::SKILL_LEADERSHIP, 1),
        (crate::mine_ladder::SKILL_MINING_FOREMAN, 1),
    ],
    more: &[(COMPRESSOR_LARGE, 1)],
};

/// Whether `type_id` is a hull a foreman flies.
pub fn command_hull(type_id: i64) -> bool {
    [ORCA, PORPOISE, RORQUAL].contains(&type_id)
}

/// The compressor that fits `hull` **[V-data]**.
pub fn compressor_for(hull: i64) -> Option<i64> {
    match hull {
        ORCA => Some(COMPRESSOR_LARGE),
        PORPOISE => Some(COMPRESSOR_MEDIUM),
        RORQUAL => Some(COMPRESSOR_CAPITAL),
        _ => None,
    }
}

/// Turns the foreman waits at a belt with nobody to boost before it tries
/// another **[A]**.
pub const BELT_PATIENCE: u32 = 6;
/// Turns at a belt with miners but no rock left before the foreman moves on
/// **[A]**: the miners go on to another belt themselves
/// ([`crate::brain::to_a_belt`]). Falsified if a belt shows no rock for two
/// turns and has some again soon after (a late grid load).
pub const DRY_PATIENCE: u32 = 2;
/// Pilots invited in one turn **[A]**.
pub const INVITES_PER_TURN: usize = 5;
/// Between two turns **[A]**.
pub const TURN_EVERY: (Duration, Duration) = (Duration::from_secs(15), Duration::from_secs(30));
/// Between two tries at buying a missing part **[A]**.
pub const ACQUIRE_EVERY: Duration = Duration::from_secs(30 * 60);

/// What the foreman remembers between turns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Characters already invited (an invite is sent once).
    pub invited: BTreeSet<i64>,
    /// The belt this stint is at.
    pub belt: Option<i64>,
    /// Belts given up, this stint.
    pub tried: Vec<i64>,
    /// Turns at the belt with no miner on the grid.
    pub empty_turns: u32,
    /// Turns at the belt with no rock on the grid ([`DRY_PATIENCE`]).
    pub dry_turns: u32,
    /// R-REL-3: pilots the society counts friendly or better (`R_eff` at
    /// [`crate::society::FRIEND_AT`]); invited whatever they fly. Refreshed
    /// at the social turn.
    pub friends: BTreeSet<i64>,
    /// L7b: the escort that took this foreman's ask on the board
    /// ([`crate::escort_board`]); invited whatever it flies.
    pub escort: Option<i64>,
}

/// One pilot on the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pilot {
    pub char_id: i64,
    pub hull: i64,
}

/// **Whom to invite** (pure): pilots flying a mining hull ([`crate::mine_ladder::hull_of`]),
/// other than `me`, not invited before, at most [`INVITES_PER_TURN`], in
/// character-id order.
pub fn invitees(grid: &[Pilot], me: i64, invited: &BTreeSet<i64>) -> Vec<i64> {
    invitees_with(grid, me, invited, &BTreeSet::new())
}

/// [`invitees`], and also any pilot in `friends` (R-REL-3, R-RELATIONS
/// §3.2): a friendly human on the grid is asked into the fleet whatever
/// they fly.
pub fn invitees_with(
    grid: &[Pilot],
    me: i64,
    invited: &BTreeSet<i64>,
    friends: &BTreeSet<i64>,
) -> Vec<i64> {
    let mut out: Vec<i64> = grid
        .iter()
        .filter(|p| p.char_id != me && !invited.contains(&p.char_id))
        .filter(|p| friends.contains(&p.char_id) || crate::mine_ladder::hull_of(p.hull).is_some())
        .map(|p| p.char_id)
        .collect();
    out.sort_unstable();
    out.dedup();
    out.truncate(INVITES_PER_TURN);
    out
}

/// What the foreman can see.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub docked: bool,
    /// The hull flown.
    pub flown: Option<i64>,
    /// A command hull in the hangar: `(item, type)`.
    pub hangar_hull: Option<(i64, i64)>,
    /// What the burst's own planner says ([`burst::step`]).
    pub burst: burst::Step,
    /// A compressor fitted on the flown hull, and one in the hangar.
    pub compressor_fitted: bool,
    pub compressor_in_hangar: Option<i64>,
    /// In a fleet (as its commander, by [`State`]).
    pub in_fleet: bool,
    /// At a belt: its ball is within a warp's reach of the ego.
    pub at_belt: bool,
    /// Pilots on the grid worth inviting.
    pub to_invite: usize,
    /// Pilots on the grid who flew a mining hull in all.
    pub miners_here: usize,
    /// Rocks on the grid.
    pub rocks_here: usize,
}

/// One turn's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Turn {
    /// Docked: board this hangar item.
    Board(i64),
    /// No command hull flown or held: say so; the acquire path may buy it.
    NoHull,
    /// The burst step `s` (fit, online, load; or what it lacks).
    Outfit,
    /// A part is missing, and the acquire path may buy it.
    Lacks(&'static str),
    /// Docked: fit and online the compressor in the hangar.
    FitCompressor(i64),
    /// Docked and ready: go out.
    Undock,
    /// In space: warp to a belt.
    ToBelt,
    /// Make the fleet.
    Form,
    /// Invite the pilots on the grid.
    Invite,
    /// Run the burst.
    Boost,
    /// Nothing to do this turn.
    Hold,
    /// Nobody here: another belt.
    NextBelt,
}

/// **The decision** (pure).
pub fn decide(f: &Facts, empty_turns: u32, dry_turns: u32) -> Turn {
    if f.docked {
        if !f.flown.is_some_and(command_hull) {
            return match f.hangar_hull {
                Some((item, _)) => Turn::Board(item),
                None => Turn::NoHull,
            };
        }
        match f.burst {
            burst::Step::Fit { .. } | burst::Step::Online { .. } | burst::Step::Load { .. } => {
                return Turn::Outfit;
            }
            burst::Step::Lacks(Lack::Module) => return Turn::Lacks("a Mining Foreman Burst"),
            burst::Step::Lacks(Lack::Charge { .. }) => {
                return Turn::Lacks("mining burst charges");
            }
            _ => {}
        }
        if !f.compressor_fitted {
            return match f.compressor_in_hangar {
                Some(item) => Turn::FitCompressor(item),
                None => Turn::Lacks("a compressor for the hull"),
            };
        }
        return Turn::Undock;
    }
    if !f.at_belt {
        return Turn::ToBelt;
    }
    if f.miners_here == 0 {
        return if empty_turns >= BELT_PATIENCE {
            Turn::NextBelt
        } else {
            Turn::Hold
        };
    }
    if f.rocks_here == 0 && dry_turns >= DRY_PATIENCE {
        return Turn::NextBelt;
    }
    if f.to_invite > 0 {
        return if f.in_fleet { Turn::Invite } else { Turn::Form };
    }
    match f.burst {
        burst::Step::Run { .. } => Turn::Boost,
        _ => Turn::Hold,
    }
}

/// **The per-bot gate** (nothing from nowhere): `eve-bots` blocks the
/// foreman career for everybody ([`crate::career::block_capital`]); this
/// opens it for a bot that flies or holds a command hull.
pub fn lift_block(s: &mut crate::career::Signals, hangar: &[crate::ItemRow], flown: Option<i64>) {
    let owns = flown.is_some_and(command_hull)
        || hangar
            .iter()
            .any(|r| r.singleton && r.category_id == 6 && command_hull(r.type_id));
    if owns {
        s.blocked.remove(CAREER);
    }
}

/// The career's name in the table.
pub const CAREER: &str = "mining_foreman";

// ---- the role ------------------------------------------------------------------------------

/// The `--loop foreman` row.
pub static ROLE: RoleSpec = RoleSpec {
    name: "foreman",
    greeting: "o7 -- boosting this belt",
    civilian: true,
    docked: crate::defend::foreman_docked,
    in_space: crate::defend::foreman_in_space,
    banner,
    setup,
};

/// What a foreman without a hull does between tries **[A]**.
pub const MISSING_HULL: &str = "an Orca, Porpoise or Rorqual it owns";

fn pace(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.failures
        .values()
        .any(|&n| n >= crate::brain::GIVE_UP_AFTER)
    {
        return vec![Act::Reconnect];
    }
    m.homing = None;
    if let Some(next) = m.market_next
        && o.now < next
    {
        return vec![Act::Wait(next - o.now)];
    }
    let next = o.now + m.jitter(TURN_EVERY);
    m.market_next = Some(next);
    vec![Act::Foreman, Act::Wait(next - o.now)]
}

pub(crate) fn docked_core(m: &mut Memory, o: &Observation) -> Vec<Act> {
    pace(m, o)
}

pub(crate) fn in_space_core(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.park.is_none() {
        return vec![Act::Wait(crate::brain::SHORT_WAIT)];
    }
    pace(m, o)
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, reconnects {}",
        s.name, s.phase, s.reconnects
    )
}

/// **Nothing from nowhere**: no slash line, whatever is lacking.
fn setup(_: Lacks) -> Vec<String> {
    Vec::new()
}

// ---- over the wire ------------------------------------------------------------------------

/// Everything in `ship_items` flagged as fitted that is a compressor.
fn fitted_compressor(conn: &mut Conn, ship: i64) -> Result<bool> {
    Ok(conn
        .ship_items(ship, None)?
        .iter()
        .any(|r| r.is_fitted() && r.group_id == GROUP_COMPRESSOR))
}

/// The pilot's trained levels, read only when the hangar holds a command hull
/// with requirements to check; `None` when there is nothing to check or the
/// read was refused (nothing is filtered then).
fn hull_levels(conn: &mut Conn, hangar: &[crate::ItemRow]) -> Result<Option<BTreeMap<i64, i64>>> {
    let checked = hangar.iter().any(|r| {
        r.singleton
            && r.category_id == 6
            && command_hull(r.type_id)
            && !crate::acquire::direct_requirements(r.type_id).is_empty()
    });
    if !checked {
        return Ok(None);
    }
    match crate::acquire::skill_levels(conn) {
        Ok(l) => Ok(Some(l)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(_) => Ok(None),
    }
}

/// The foreman's facts, read.
pub fn read(conn: &mut Conn, st: &State) -> Result<Facts> {
    let docked = conn.session().station_id().is_some();
    let me = conn.session().char_id().unwrap_or(0);
    let ship = conn
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("foreman: the session names no ship".to_owned()))?;
    let hangar = if docked {
        conn.hangar_items()?
    } else {
        Vec::new()
    };
    let flown = if docked {
        hangar.iter().find(|r| r.item_id == ship).map(|r| r.type_id)
    } else {
        conn.world_now().get(ship).and_then(|b| b.type_id())
    };
    // SKILLGATE: a command hull the pilot's skills do not board is not one
    // it holds yet ([`turn`] queues the lack).
    let levels = hull_levels(conn, &hangar)?;
    let hangar_hull = hangar
        .iter()
        .find(|r| {
            r.item_id != ship
                && r.singleton
                && r.category_id == 6
                && command_hull(r.type_id)
                && levels
                    .as_ref()
                    .is_none_or(|l| crate::acquire::flyable(l, r.type_id))
        })
        .map(|r| (r.item_id, r.type_id));
    let burst = burst::peek(conn)?;
    let compressor_fitted = fitted_compressor(conn, ship)?;
    let compressor_in_hangar = hangar
        .iter()
        .find(|r| r.group_id == GROUP_COMPRESSOR && !r.is_fitted())
        .map(|r| r.item_id);
    let world = conn.world_now();
    let ego = world.ego_ball().copied();
    let at_belt = match (ego, st.belt.and_then(|b| world.get(b))) {
        (Some(e), Some(b)) => {
            eve_server::space_state::distance(e.motion.position, b.motion.position)
                - e.radius
                - b.radius
                <= crate::MIN_WARP_DISTANCE
        }
        _ => false,
    };
    let grid: Vec<Pilot> = world
        .balls()
        .filter_map(|b| {
            let s = b.slim?;
            Some(Pilot {
                char_id: s.char_id?,
                hull: s.type_id?,
            })
        })
        .collect();
    let friends: BTreeSet<i64> = st.friends.iter().copied().chain(st.escort).collect();
    let to_invite = invitees_with(&grid, me, &st.invited, &friends).len();
    let miners_here = grid
        .iter()
        .filter(|p| p.char_id != me && crate::mine_ladder::hull_of(p.hull).is_some())
        .count();
    let rocks_here = world
        .balls()
        .filter(|b| crate::park::is_rock(b.ball_id))
        .count();
    Ok(Facts {
        docked,
        flown,
        hangar_hull,
        burst,
        compressor_fitted,
        compressor_in_hangar,
        in_fleet: conn.session().fleet_id().is_some(),
        at_belt,
        to_invite,
        miners_here,
        rocks_here,
    })
}

/// **One foreman turn**: read the facts, decide ([`decide`]), do it. The
/// note says what was done, for the act log.
pub fn turn(conn: &mut Conn, m: &mut Memory) -> Result<String> {
    tend_board(conn, m);
    let facts = read(conn, &m.foreman)?;
    let step = decide(&facts, m.foreman.empty_turns, m.foreman.dry_turns);
    match step {
        Turn::Board(item) => {
            conn.board(item)?;
            Ok(format!("boarded command hull {item}"))
        }
        Turn::NoHull => {
            // SKILLGATE: a command hull held but not yet flyable: queue
            // what it asks for, ahead of the rest, and say so.
            let hangar = conn.hangar_items()?;
            let levels = hull_levels(conn, &hangar)?.unwrap_or_default();
            let lack: Vec<(i64, i64)> = hangar
                .iter()
                .filter(|r| r.singleton && r.category_id == 6 && command_hull(r.type_id))
                .flat_map(|r| crate::acquire::lacking_of(&levels, r.type_id))
                .collect();
            if lack.is_empty() {
                return Ok(format!("no command hull: wants {MISSING_HULL}"));
            }
            let missing = crate::acquire::learn_urgent_soft(conn, &lack)?;
            Ok(format!(
                "lacks skills {lack:?} for the command hull{}",
                crate::acquire::missing_note(&missing)
            ))
        }
        Turn::Lacks(what) => Ok(format!("lacks {what}")),
        Turn::Outfit => {
            let mut done = Vec::new();
            for _ in 0..6 {
                let s = burst::step(conn)?;
                done.push(format!("{s:?}"));
                if !matches!(
                    s,
                    burst::Step::Fit { .. } | burst::Step::Online { .. } | burst::Step::Load { .. }
                ) {
                    break;
                }
            }
            Ok(format!("burst: {}", done.join(", ")))
        }
        Turn::FitCompressor(item) => {
            let fitted = match crate::acquire::fit_auto_gated(conn, &[item])? {
                Ok(f) => f,
                Err(note) => return Ok(format!("compressor {item}: {note}")),
            };
            for row in fitted.iter().filter(|r| r.group_id == GROUP_COMPRESSOR) {
                if let Err(note) = crate::acquire::set_online_gated(conn, row.item_id)? {
                    return Ok(format!("compressor {item}: {note}"));
                }
            }
            Ok(format!("fitted compressor {item}"))
        }
        Turn::Undock => {
            conn.undock_repaired()?;
            m.foreman = State::default();
            Ok("undocked".to_owned())
        }
        Turn::ToBelt => to_belt(conn, m),
        Turn::NextBelt => {
            if let Some(b) = m.foreman.belt.take() {
                m.foreman.tried.push(b);
            }
            m.foreman.empty_turns = 0;
            m.foreman.dry_turns = 0;
            to_belt(conn, m)
        }
        Turn::Form => {
            let ship_type = facts.flown.unwrap_or(ORCA);
            let fleet = conn.create_fleet(ship_type)?;
            invite(conn, m, fleet)
        }
        Turn::Invite => {
            let fleet = conn
                .session()
                .fleet_id()
                .ok_or_else(|| Error::NotSent("foreman: no fleet to invite into".to_owned()))?;
            invite(conn, m, fleet)
        }
        Turn::Boost => {
            let s = burst::step(conn)?;
            Ok(format!("burst: {s:?}"))
        }
        Turn::Hold => {
            if facts.miners_here == 0 {
                m.foreman.empty_turns += 1;
            }
            if facts.at_belt && facts.rocks_here == 0 {
                m.foreman.dry_turns += 1;
            } else {
                m.foreman.dry_turns = 0;
            }
            // Keep the burst running whenever it can.
            if let burst::Step::Run { .. } = facts.burst {
                let s = burst::step(conn)?;
                return Ok(format!("burst: {s:?}"));
            }
            Ok("holding the belt".to_owned())
        }
    }
}

/// **The request board** (L7b): ask for an escort from a dangerous belt with
/// miners on it, take the ask down otherwise, and note who took it so the
/// invite reaches it ([`crate::escort_board`]).
fn tend_board(conn: &mut Conn, m: &mut Memory) {
    let me = conn.session().char_id().unwrap_or(0);
    if me == 0 {
        return;
    }
    let docked = conn.session().station_id().is_some();
    let system = conn.session().solar_system_id();
    let security = system.and_then(|s| m.policy.travel.as_ref()?.security(s));
    let world = conn.world_now();
    let at_belt = !docked
        && match (world.ego_ball(), m.foreman.belt.and_then(|b| world.get(b))) {
            (Some(e), Some(b)) => {
                eve_server::space_state::distance(e.motion.position, b.motion.position)
                    - e.radius
                    - b.radius
                    <= crate::MIN_WARP_DISTANCE
            }
            _ => false,
        };
    let miners = world
        .balls()
        .filter_map(|b| b.slim?.type_id)
        .filter(|&t| crate::mine_ladder::hull_of(t).is_some())
        .count();
    let (belt, now) = (m.foreman.belt, std::time::Instant::now());
    m.foreman.escort = crate::escort_board::with(|b| {
        match system {
            Some(s) if crate::escort_board::wants_escort(at_belt, miners, security) => {
                b.post(me, s, belt, now)
            }
            _ => b.withdraw(me),
        }
        b.expire(now);
        b.taker_of(me)
    })
    .flatten();
}

/// Invite the pilots on the grid worth inviting ([`invitees`]) into
/// `fleet`'s first squad, once each; the note says whom.
pub fn invite(conn: &mut Conn, m: &mut Memory, fleet: i64) -> Result<String> {
    let me = conn.session().char_id().unwrap_or(0);
    let grid: Vec<Pilot> = conn
        .world_now()
        .balls()
        .filter_map(|b| {
            let s = b.slim?;
            Some(Pilot {
                char_id: s.char_id?,
                hull: s.type_id?,
            })
        })
        .collect();
    let friends: BTreeSet<i64> = m
        .foreman
        .friends
        .iter()
        .copied()
        .chain(m.foreman.escort)
        .collect();
    let who = invitees_with(&grid, me, &m.foreman.invited, &friends);
    let seat = conn.fleet_seat(fleet, 0)?;
    let mut sent = Vec::new();
    for c in who {
        m.foreman.invited.insert(c);
        if conn.fleet_invite(fleet, c, seat).is_ok() {
            sent.push(c);
        }
    }
    Ok(format!("fleet {fleet}: invited {sent:?}"))
}

/// Warp to a belt not tried this stint: the nearest, then the next.
fn to_belt(conn: &mut Conn, m: &mut Memory) -> Result<String> {
    let world = conn.world_now();
    let Some(ego) = world.ego_ball().copied() else {
        return Ok("no ego yet".to_owned());
    };
    let mut belts: Vec<(f64, i64)> = world
        .balls()
        .filter(|b| crate::driver::is_belt(b) && !m.foreman.tried.contains(&b.ball_id))
        .map(|b| {
            (
                eve_server::space_state::distance(ego.motion.position, b.motion.position),
                b.ball_id,
            )
        })
        .collect();
    belts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let Some(&(_, belt)) = belts.get((m.seed % belts.len().max(1) as u64) as usize) else {
        m.foreman.tried.clear();
        return Ok("no belt left to try".to_owned());
    };
    m.foreman.belt = Some(belt);
    conn.warp_to_item(belt, crate::brain::MINING_STANDOFF)?;
    let until = std::time::Instant::now() + crate::courier::TRAVEL_CEILING;
    while std::time::Instant::now() < until {
        conn.pump(Duration::from_millis(250))?;
        if conn
            .world_now()
            .ego_ball()
            .is_some_and(|b| b.warp_commanded())
        {
            break;
        }
    }
    while std::time::Instant::now() < until
        && conn
            .world_now()
            .ego_ball()
            .is_some_and(|b| b.warp_commanded())
    {
        conn.pump(Duration::from_millis(250))?;
    }
    Ok(format!("warped to belt {belt}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            docked: true,
            flown: Some(ORCA),
            hangar_hull: None,
            burst: burst::Step::Ready,
            compressor_fitted: true,
            compressor_in_hangar: None,
            in_fleet: false,
            at_belt: false,
            to_invite: 0,
            miners_here: 0,
            rocks_here: 5,
        }
    }

    #[test]
    fn a_pilot_without_a_command_hull_boards_one_it_holds_or_says_what_it_wants() {
        let f = Facts {
            flown: Some(crate::mine_ladder::VENTURE),
            hangar_hull: Some((7, ORCA)),
            ..facts()
        };
        assert_eq!(decide(&f, 0, 0), Turn::Board(7));
        let f = Facts {
            hangar_hull: None,
            ..f
        };
        assert_eq!(decide(&f, 0, 0), Turn::NoHull);
    }

    #[test]
    fn the_outfit_is_burst_then_compressor_then_out() {
        let fit = burst::Step::Fit { item: 1 };
        let f = Facts {
            burst: fit,
            ..facts()
        };
        assert_eq!(decide(&f, 0, 0), Turn::Outfit);
        let f = Facts {
            burst: burst::Step::Lacks(Lack::Module),
            ..facts()
        };
        assert_eq!(decide(&f, 0, 0), Turn::Lacks("a Mining Foreman Burst"));
        let f = Facts {
            compressor_fitted: false,
            compressor_in_hangar: Some(9),
            ..facts()
        };
        assert_eq!(decide(&f, 0, 0), Turn::FitCompressor(9));
        let f = Facts {
            compressor_fitted: false,
            ..facts()
        };
        assert_eq!(decide(&f, 0, 0), Turn::Lacks("a compressor for the hull"));
        assert_eq!(decide(&facts(), 0, 0), Turn::Undock);
    }

    #[test]
    fn in_space_it_goes_to_a_belt_forms_a_fleet_invites_and_boosts() {
        let out = Facts {
            docked: false,
            burst: burst::Step::Run {
                module: 1,
                effect: ("e", 6736),
            },
            ..facts()
        };
        assert_eq!(decide(&out, 0, 0), Turn::ToBelt);
        let at = Facts {
            at_belt: true,
            ..out.clone()
        };
        assert_eq!(decide(&at, 0, 0), Turn::Hold, "nobody here yet");
        assert_eq!(decide(&at, BELT_PATIENCE, 0), Turn::NextBelt);
        let miners = Facts {
            miners_here: 3,
            to_invite: 3,
            ..at.clone()
        };
        assert_eq!(decide(&miners, 0, 0), Turn::Form);
        let in_fleet = Facts {
            in_fleet: true,
            ..miners.clone()
        };
        assert_eq!(decide(&in_fleet, 0, 0), Turn::Invite);
        let all_in = Facts {
            to_invite: 0,
            in_fleet: true,
            ..miners
        };
        assert_eq!(decide(&all_in, 0, 0), Turn::Boost);
        let running = Facts {
            burst: burst::Step::Running,
            ..all_in
        };
        assert_eq!(decide(&running, 0, 0), Turn::Hold);
    }

    #[test]
    fn only_new_mining_pilots_are_invited_and_a_few_at_a_time() {
        use crate::mine_ladder::{HULK, MERLIN, VENTURE};
        let grid = [
            Pilot {
                char_id: 5,
                hull: HULK,
            },
            Pilot {
                char_id: 1,
                hull: MERLIN,
            },
            Pilot {
                char_id: 9,
                hull: 77,
            },
            Pilot {
                char_id: 3,
                hull: VENTURE,
            },
            Pilot {
                char_id: 2,
                hull: VENTURE,
            },
        ];
        let invited = BTreeSet::from([2]);
        // 9 flies no mining hull; 2 was invited; 1 is the foreman himself.
        assert_eq!(invitees(&grid, 1, &invited), vec![3, 5]);
        let many: Vec<Pilot> = (10..30)
            .map(|c| Pilot {
                char_id: c,
                hull: VENTURE,
            })
            .collect();
        assert_eq!(invitees(&many, 1, &BTreeSet::new()).len(), INVITES_PER_TURN);
    }

    #[test]
    fn each_command_hull_has_its_own_compressor() {
        assert_eq!(compressor_for(ORCA), Some(COMPRESSOR_LARGE));
        assert_eq!(compressor_for(PORPOISE), Some(COMPRESSOR_MEDIUM));
        assert_eq!(compressor_for(RORQUAL), Some(COMPRESSOR_CAPITAL));
        assert_eq!(compressor_for(crate::mine_ladder::HULK), None);
        assert!(command_hull(ORCA) && !command_hull(crate::mine_ladder::HULK));
        assert_eq!(KIT.hull, Some(ORCA));
        assert!(KIT.more.contains(&(COMPRESSOR_LARGE, 1)));
    }

    #[test]
    fn a_friendly_human_is_invited_whatever_they_fly() {
        const FRIEND: i64 = 90_000_811;
        const HULL: i64 = 990_000_001;
        let grid = [
            Pilot {
                char_id: FRIEND,
                hull: HULL,
            },
            Pilot {
                char_id: 90_000_812,
                hull: HULL,
            },
        ];
        assert!(invitees(&grid, 1, &BTreeSet::new()).is_empty());
        let friends: BTreeSet<i64> = [FRIEND].into();
        assert_eq!(
            invitees_with(&grid, 1, &BTreeSet::new(), &friends),
            vec![FRIEND]
        );
        // Still once each.
        assert!(invitees_with(&grid, 1, &friends, &friends).is_empty());
    }

    /// belt-econ P1: miners on the grid but no rock left: the foreman moves
    /// on after the patience, not before, and not while rocks remain.
    #[test]
    fn a_belt_with_miners_and_no_rocks_is_left() {
        let at = Facts {
            docked: false,
            at_belt: true,
            miners_here: 3,
            rocks_here: 0,
            ..facts()
        };
        assert_eq!(decide(&at, 0, 0), Turn::Hold);
        assert_eq!(decide(&at, 0, DRY_PATIENCE), Turn::NextBelt);
        let rocks = Facts {
            rocks_here: 4,
            ..at
        };
        assert_eq!(decide(&rocks, 0, DRY_PATIENCE), Turn::Hold);
    }
}
