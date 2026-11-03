//! **The wing role: bots that follow a human FC everywhere** (plan-fc-follow,
//! lane FC-1; `docs/lanes/fc-1.md`).
//!
//! A `wing` bot flies the escort's fit and the escort's fight, but where the
//! escort follows whoever *invited* it and only inside one system, the wing
//! follows one named FC ([`Seat::fc`]) over gates, into docks and out again:
//!
//! - **Where the FC is** comes from the fleet composition, polled by one bot
//!   per FC and shared through the [`crate::wing_hall::WingHall`]; the driver
//!   puts it in [`Scene::fc`] each turn and sets the fleet boss to the FC
//!   (so the park's boss ball, the primary and the fleet orders all key on
//!   him).
//! - **What to do about it** is [`crate::rally::step`] (pure): undock, jump
//!   the first gate of the route, dock in his station, warp to him, close in.
//!   On his grid the escort's own turn ([`crate::escort::in_space`]) keeps
//!   the wing at its anchor range and fights what threatens him.
//! - **Docked**, the escort's kit turn runs, and its `Undock` is held back
//!   while the FC is docked in this station, or unknown ([`docked`]).
//! - **Joining**: a wing bot with no fleet applies to the FC's fleet-finder
//!   advert every [`JOIN_POLL`] ([`WingAct::Join`]); the FC's invite is then
//!   accepted by the brain's rule 2 (`AcceptFleet::Fc`). The owner may also
//!   just invite the bots.
//!
//! Every constant below is **[A]** with its falsifier in its doc comment.

use crate::brain::{self, Act, Memory, Observation, Place};
use crate::fleet_orders::{self, Heard};
use crate::rally::{self, Here, Step, Whereabouts};
use crate::roles::{RoleSpec, Summary};
use crate::wing_fight::Stance;
use crate::wing_hall::{Tally, WingHall};
use eve_server::wing_words::{Mode as WingMode, WingCmd};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a wing bot with no fleet looks at the fleet finder **[A]**.
/// Falsified by bots that wait visibly long to join (shorten), or by
/// `GetAvailableFleetAds` in the server's slow-call log (lengthen).
pub const JOIN_POLL: Duration = Duration::from_secs(20);

/// The wing routes at any security **[A]**: it follows the FC into null.
/// Falsified by a wing that stops at a gate the FC took (then the map lacks
/// the system's security, and this floor is the reason).
pub const WING_FLOOR: f64 = crate::travel::ANY_SECURITY;

/// How far off the FC a DPS wing bot sits, metres **[A]**: the escort's.
/// Falsified by a wing that drifts out of its guns' reach of the FC's rats.
pub const ANCHOR_RANGE: f64 = crate::escort::ESCORT_RANGE;

/// Commands a wing bot holds at once (the queue drops the oldest) **[A]**.
pub const CMD_QUEUE: usize = 8;

/// How long a `primary <name>` waits for that name to appear on grid **[A]**.
/// Falsified by an owner who names a target that jumps in later.
pub const PRIMARY_FOR: Duration = Duration::from_secs(60);

/// The wing's stance for a spoken mode (FC-2's [`Stance`], FC-3's words).
fn stance_of(mode: WingMode) -> Stance {
    match mode {
        WingMode::Defend => Stance::Defend,
        WingMode::Hold => Stance::Hold,
        WingMode::Free => Stance::Free,
    }
}

/// A standing duty from the owner's words: the bot does not undock after the
/// FC until `follow` clears it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duty {
    /// `dock`: in the FC's station when he is docked, else the home one.
    Dock,
    /// `go home`: the staging station.
    Home,
}

/// One bot's seat in a wing ([`crate::brain::Policy::wing`]).
#[derive(Debug, Clone)]
pub struct Seat {
    pub hall: Arc<WingHall>,
    /// The FC's character; `None`: the fleet's inviter is the FC.
    pub fc: Option<i64>,
}

impl PartialEq for Seat {
    fn eq(&self, o: &Seat) -> bool {
        self.fc == o.fc
    }
}

impl Seat {
    pub fn new(hall: Arc<WingHall>, fc: Option<i64>) -> Seat {
        Seat { hall, fc }
    }
}

/// What a wing bot remembers. Never world state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    /// The fleet this bot applied to through the finder: the FC's invite
    /// for it is accepted whoever sends it.
    pub applied: Option<i64>,
    /// No [`WingAct::Join`] before this.
    pub join_next: Option<Instant>,
    /// The system a "no route" was said for.
    pub no_route_said: Option<i64>,
    /// Who this bot shoots on its own account (FC-2,
    /// [`crate::wing_fight::Stance`]); the FC changes it by command (FC-3).
    pub stance: crate::wing_fight::Stance,
    /// FC-3: commands not yet applied (from `/bots` and the FC's chat).
    pub cmds: VecDeque<WingCmd>,
    /// FC-3: the highest fleet-chat line of the FC already read.
    pub chat_seq: u64,
    /// FC-3: `dock` / `go home`, until `follow`.
    pub duty: Option<Duty>,
    /// FC-3: `primary <name>` still looking for its name, and until when.
    pub primary: Option<(String, Instant)>,
}

/// Where the FC is, as the driver read it from the hall this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fc {
    pub char_id: i64,
    pub system: i64,
    /// The station he is docked in; `None`: in space.
    pub station: Option<i64>,
}

/// The wing's part of an [`Observation`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    pub fc: Option<Fc>,
    /// FC-2: the fleet's target tags, `item -> label`
    /// (`OnFleetStateChange`, [`crate::wing_fight::parse_tags`]).
    pub tags: std::collections::BTreeMap<i64, String>,
    /// FC-2: who runs an Entosis Link on what, `ship -> target`, from
    /// `OnSpecialFX` (the driver keeps it; empty without a content bundle).
    pub links: std::collections::BTreeMap<i64, i64>,
    /// FC-2: this system is held by our alliance, so a stranger's link is
    /// a contest. Not read from anywhere yet **[A]**: see `wing_fight`.
    pub held: bool,
    /// The fleet whose tags were last read whole (`GetFleetState`), so a
    /// bot that joins late learns the tags that were set before it came.
    pub tags_of: Option<i64>,
    /// FC-3: this bot is the wing's lead (it answers for the wing).
    pub lead: bool,
    /// FC-3: where the wing's bots are, for a status line.
    pub tally: Tally,
}

/// What the wing asks of the driver beyond the shared acts.
#[derive(Debug, Clone, PartialEq)]
pub enum WingAct {
    /// Look at the fleet finder and apply to the FC's advert.
    Join,
    /// Fly to `station` in this system and dock (the FC is docked there).
    GoDock { station: i64 },
}

impl WingAct {
    pub fn describe(&self) -> (String, Vec<i64>) {
        match self {
            WingAct::Join => ("wing: apply to the FC's fleet advert".to_owned(), vec![]),
            WingAct::GoDock { station } => (
                format!("wing: dock at the FC's station {station}"),
                vec![*station],
            ),
        }
    }
}

/// **Rule 2's test** for an invite to a wing bot: from its FC, or into the
/// fleet it applied to (the finder's invite comes from the advert's boss).
pub fn accepts(m: &Memory, fc: i64, inviter: i64, fleet: i64) -> bool {
    inviter == fc || m.wing.applied == Some(fleet)
}

/// A join is due: no fleet, an FC to look for, and the poll has come round.
fn join_due(m: &Memory, o: &Observation) -> bool {
    m.policy.wing.as_ref().is_some_and(|s| s.fc.is_some())
        && o.fleet.is_none()
        && o.invite.is_none()
        && m.wing.join_next.is_none_or(|t| o.now >= t)
}

/// Where the bot is, for [`rally::step`].
fn here_of(m: &Memory, o: &Observation) -> Here {
    let map = m.policy.travel.as_deref();
    match o.place {
        Place::Docked { station } => Here {
            system: map.and_then(|mp| mp.system_of(station)).unwrap_or(0),
            station: Some(station),
            in_space: false,
            warping: false,
            ego_ball: None,
        },
        Place::InSpace { system, .. } => Here {
            system,
            station: None,
            in_space: true,
            warping: o.park.as_ref().is_some_and(|p| p.ego_warping),
            ego_ball: o.park.as_ref().map(|p| p.ego),
        },
        Place::Transit => Here {
            system: 0,
            station: None,
            in_space: false,
            warping: false,
            ego_ball: None,
        },
    }
}

/// The rally step toward the FC, or `None` when there is no map, no FC or
/// nowhere known to look for him.
pub fn rally_step(m: &Memory, o: &Observation) -> Option<Step> {
    let map = m.policy.travel.as_deref()?;
    let fc = o.wing.fc?;
    let target = Whereabouts {
        char_id: fc.char_id,
        system: fc.system,
        station: fc.station,
        ship_ball_on_grid: o.park.as_ref().and_then(|p| p.boss),
    };
    Some(rally::step(
        &here_of(m, o),
        &target,
        map,
        WING_FLOOR,
        ANCHOR_RANGE,
    ))
}

/// Docked: the escort's kit turn, with its `Undock` held back until the FC
/// is somewhere to follow him to.
pub fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if m.policy.wing.is_none() {
        return crate::escort::docked(m, o);
    }
    if join_due(m, o) {
        m.wing.join_next = Some(o.now + JOIN_POLL);
        return vec![Act::Wing(WingAct::Join)];
    }
    let mut acts = crate::escort::docked(m, o);
    if m.wing.duty.is_some() {
        // `dock` / `go home`: stay until the FC says follow.
        for a in &mut acts {
            if matches!(a, Act::Undock) {
                *a = Act::Wait(brain::SHORT_WAIT);
            }
        }
    }
    if acts.iter().any(|a| matches!(a, Act::Undock)) {
        // No map: nothing to route by, so the escort's own rules (undock).
        let go = match rally_step(m, o) {
            Some(step) => step == Step::Undock,
            None => m.policy.travel.is_none(),
        };
        if !go {
            for a in &mut acts {
                if matches!(a, Act::Undock) {
                    *a = Act::Wait(brain::SHORT_WAIT);
                }
            }
        }
    }
    acts
}

/// In space: follow the FC where he is; on his grid the escort's turn.
pub fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if m.policy.wing.is_none() {
        return crate::escort::in_space(m, o);
    }
    // FC-3: `go home`, `dock` (the FC in space) and `rearm` are the home
    // run of the escort's turn.
    let fc_docked = o.wing.fc.is_some_and(|f| f.station.is_some());
    if m.homing.is_none()
        && (m.wing.duty == Some(Duty::Home) || (m.wing.duty == Some(Duty::Dock) && !fc_docked))
    {
        m.homing = Some(brain::Homing::Rearm);
    }
    if m.homing.is_some() {
        return crate::escort::in_space(m, o);
    }
    let Some(step) = rally_step(m, o) else {
        // No map, no FC read yet, or no fleet: the escort's own rules.
        return crate::escort::in_space(m, o);
    };
    match step {
        Step::Jump { gate, .. } => vec![Act::JumpGate { gate }],
        Step::Dock { station } => vec![Act::Wing(WingAct::GoDock { station })],
        Step::WarpToMember { char_id } => {
            // A refused warp backs off ([`brain::BOSS_RETRY`]); wait it out.
            if o.boss_unreachable_until.is_some_and(|t| o.now < t) {
                return vec![Act::Wait(crate::escort::PATROL_WAIT)];
            }
            vec![Act::WarpToMember {
                char_id,
                range: crate::escort::WARP_STANDOFF,
            }]
        }
        Step::NoRoute => {
            let system = o.wing.fc.map_or(0, |f| f.system);
            if m.wing.no_route_said == Some(system) {
                return vec![Act::Wait(crate::escort::PATROL_WAIT)];
            }
            m.wing.no_route_said = Some(system);
            vec![
                Act::Missing("a gate route to the FC's system"),
                Act::Wait(crate::escort::PATROL_WAIT),
            ]
        }
        Step::Wait | Step::Undock | Step::Stay => vec![Act::Wait(brain::SHORT_WAIT)],
        // On his grid: the escort's turn follows him at its range, fights
        // what threatens him, loots and repairs.
        // FC-2: ... after the wing's own choice of target ([`crate::wing_fight`]).
        Step::Keep { .. } | Step::Arrived => {
            primary_on_grid(m, o);
            crate::wing_fight::on_grid(m, o)
        }
    }
}

// ---- the owner's words (FC-3) --------------------------------------------------------------------

/// Queue one command (from `/bots` through the [`crate::driver::Handle`]).
pub fn queue(m: &mut Memory, cmd: WingCmd) {
    if m.wing.cmds.len() >= CMD_QUEUE {
        m.wing.cmds.pop_front();
    }
    m.wing.cmds.push_back(cmd);
}

/// The FC's fleet-chat lines not yet read, as commands: his lines only, each
/// `seq` once, and only a line that is wholly a command ([`eve_server::wing_words::parse`]).
fn hear_chat(m: &mut Memory, o: &Observation) {
    let Some(fc) = m
        .policy
        .wing
        .as_ref()
        .and_then(|s| s.fc)
        .or(o.fleet.and_then(|f| f.boss))
    else {
        return;
    };
    let from = m.wing.chat_seq;
    let mut lines: Vec<&crate::chatter::Line> = o
        .talk
        .heard
        .iter()
        .filter(|l| l.room == crate::chat::RoomKind::Fleet && l.from == fc && l.seq > from)
        .collect();
    lines.sort_by_key(|l| l.seq);
    for l in lines {
        m.wing.chat_seq = m.wing.chat_seq.max(l.seq);
        if let Some(cmd) = eve_server::wing_words::parse(&[l.text.as_str()]) {
            queue(m, cmd);
        }
    }
}

/// **Apply the queued commands** (and the FC's fleet-chat ones) and return
/// the lead's replies: only the wing's lead speaks, once per command.
pub fn commands(m: &mut Memory, o: &Observation) -> Vec<Act> {
    hear_chat(m, o);
    let mut said = Vec::new();
    while let Some(cmd) = m.wing.cmds.pop_front() {
        if let Some(line) = apply(m, o, cmd)
            && o.wing.lead
        {
            said.push(Act::Say(line));
        }
    }
    said
}

/// A hold ends when the FC gives any other order that moves the wing.
fn clear_hold(m: &mut Memory) {
    if m.order.is_some_and(|x| x.heard == Heard::Hold) {
        m.order = None;
    }
}

/// Carry out one command; the reply the lead says, if any.
fn apply(m: &mut Memory, o: &Observation, cmd: WingCmd) -> Option<String> {
    let in_space = matches!(o.place, Place::InSpace { .. });
    let reply = format!("copy: {}", cmd.describe());
    match cmd {
        WingCmd::Follow => {
            clear_hold(m);
            m.wing.duty = None;
            m.wing.stance = Stance::default();
            m.wing.primary = None;
            if m.homing == Some(brain::Homing::Rearm) {
                m.homing = None;
            }
        }
        WingCmd::Hold => fleet_orders::hear(m, Heard::Hold, o.now),
        WingCmd::Dock => {
            clear_hold(m);
            m.wing.duty = Some(Duty::Dock);
        }
        WingCmd::GoHome => {
            clear_hold(m);
            m.wing.duty = Some(Duty::Home);
        }
        WingCmd::Rearm => {
            clear_hold(m);
            m.wing.duty = None;
            if in_space && m.homing.is_none() {
                m.homing = Some(brain::Homing::Rearm);
            }
        }
        WingCmd::Stance(mode) => m.wing.stance = stance_of(mode),
        WingCmd::Primary(name) => m.wing.primary = Some((name, o.now + PRIMARY_FOR)),
        WingCmd::Status => return Some(status_line(m, o)),
        WingCmd::Join => {
            m.wing.join_next = None;
            return None;
        }
        // FC-4 acts on these; until then the lead says so.
        c if c.is_sov() => {
            return Some(format!(
                "not yet: {} (the sov wing is not built)",
                c.describe()
            ));
        }
        _ => {}
    }
    Some(reply)
}

/// The wing's one-line report, from the hall's tally (the lead says it).
fn status_line(m: &Memory, o: &Observation) -> String {
    let t = o.wing.tally;
    let fc = match o.wing.fc {
        Some(f) if f.station.is_some() => "FC docked".to_owned(),
        Some(f) => {
            let name = m
                .policy
                .travel
                .as_deref()
                .and_then(|mp| mp.name_of(f.system))
                .map_or_else(|| format!("system {}", f.system), str::to_owned);
            format!("FC in {name}")
        }
        None => "FC not seen".to_owned(),
    };
    format!(
        "wing: {} on grid, {} docked, {} elsewhere; {:?}; {fc}",
        t.grid, t.docked, t.away, m.wing.stance
    )
}

/// A `primary <name>` becomes the bot's target when a hostile of that name
/// is on grid; it waits [`PRIMARY_FOR`] for one **[A]** (names only: the
/// park carries no ship-type names).
fn primary_on_grid(m: &mut Memory, o: &Observation) {
    let Some((name, until)) = m.wing.primary.clone() else {
        return;
    };
    if o.now >= until {
        m.wing.primary = None;
        return;
    }
    let Some(park) = o.park.as_ref() else {
        return;
    };
    if let Some(h) = park
        .hostiles
        .iter()
        .find(|h| h.name.to_ascii_lowercase().contains(&name))
    {
        m.target = Some((h.ball.id, h.name.clone()));
        m.wing.primary = None;
    }
}

// ---- the role -----------------------------------------------------------------------------------

/// A fighter that trails a human FC over the gates (the escort's kit).
pub static ROLE: RoleSpec = RoleSpec {
    name: "wing",
    greeting: "o7 -- on your wing, FC",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

fn banner(s: &Summary) -> String {
    (crate::roles::ESCORT.banner)(s)
}

fn setup(lacks: crate::roles::Lacks) -> Vec<String> {
    (crate::roles::ESCORT.setup)(lacks)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod word_tests;
