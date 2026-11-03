//! **Fleets: who flies with whom, in which seat.**
//!
//! This module is the membership state machine behind `fleetObjectHandler`
//! (`service/fleet.rs` is the dispatch arm over it). It owns:
//!
//! - every fleet on this node, its wing/squad tree, MOTD and options;
//! - the pending invites;
//! - the reconnect window of a pilot whose client dropped;
//! - the jump beacons and bridges each pilot has open, and the fleet's
//!   target tags, because the fleet is who gets told about them.
//!
//! Every mutator is a pure function of `Fleets` that returns [`Effects`]:
//! the notifications to send (each addressed to character ids) and the
//! session changes to queue. Nothing here touches a socket, and nothing here
//! is persisted -- a fleet is a gathering of people who are online, so a
//! restart starts with none.
//!
//! # The client's model, as this module reads it
//!
//! Sources are the decompiled client under `out/research/_w/src`; line
//! numbers are of the decompiled text.
//!
//! - **Two independent axes, `role` and `job`.** `role` is the seat (fleet
//!   commander, wing commander, squad commander, member) and `job` is a bit
//!   set whose `fleetJobCreator` bit marks the boss.
//!   `fleetbroadcastexports.GetRankName` tests the job bit first and then the
//!   role, with a "...Boss" label for every role **[V]
//!   fleetbroadcastexports.py:236-250**, so the boss may sit in any seat.
//! - **Seats are positional.** A member's `squadID` is `-1` unless he sits in
//!   a squad, and his `wingID` is `-1` unless he sits in or over a wing:
//!   `fleetSvc.AddToFleet` places a member by `squadID != -1`, then
//!   `wingID != -1`, then `role == fleetRoleLeader` **[V] fleetSvc.py:469-491**.
//!   So the fleet commander is `(-1, -1)` and a wing commander `(wing, -1)`.
//! - **Squad ids are positive.** `evefleet.util.IsSuperior` tests
//!   `myself.squadID > 0` **[V] evefleet/util.py:37**.
//! - **The tree is re-read, not patched.** Every wing/squad notification
//!   handler calls `self.fleet.GetWings()` before anything else **[V]
//!   fleetSvc.py:1109-1117, 1238-1252**, so those pushes carry only ids.
//! - **`CreateWing` makes an empty wing.** The client follows a successful
//!   `CreateWing()` with its own `CreateSquad(wingID)` **[V]
//!   fleetSvc.py:561-565**, so a server that also added a squad would give
//!   every new wing two.
//! - **Empty names are the default.** The window shows a numbered default
//!   when a wing's or squad's `name` is `''` (`fleet.SquadronName`,
//!   `fleet.WingName`) **[V] ui/shared/fleet/fleet.py:55-70**, numbering by
//!   sorted id, so ids must grow in creation order.
//! - **The session carries the seat.** `IsCommanderOrBoss` reads
//!   `session.fleetrole` **[V] fleetSvc.py:1732-1733** and
//!   `IsMySubordinate` reads `session.wingid`/`squadid` **[V]
//!   fleetSvc.py:1748-1756**, so every seat change re-sends them.
//!
//! # Placement and authority (ours, from the client's own drag rules)
//!
//! `fleetSvc.CanMoveToThisEntry` lets the boss or the fleet commander place
//! anyone anywhere, a wing commander only inside his own wing, and a pilot
//! with `isFreeMove` only himself, into a squad **[V] fleetSvc.py:1974-2059**.
//! [`Fleets::move_member`] enforces the same, server-side, because the
//! client's check is advice to the window and not a guard.
//!
//! # Ids
//!
//! A fleet id is `node * NODEID_MOD + serial * FLEETID_MOD` **[A]**: the
//! moduli are the client's constants **[V] evefleet/const.py:48-49**, but no
//! client code decodes a fleet id (the only consumer is
//! `Moniker('fleetObjectHandler', fleetID)` **[V] eveMoniker.py:202-205**),
//! so all the id needs is to be unique. Falsified by a client path that
//! derives a node from `fleetID`.
//!
//! Wing and squad ids come from **one counter for the whole node**, which
//! skips every multiple of `FLEETID_MOD`. That makes them unique across all
//! fleets (`DeleteSquad(squadID)` names no wing) and disjoint from every
//! fleet id, since a fleet id is always such a multiple.

use crate::damage::BlueTime;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

mod wire;
pub use wire::{FleetEvent, Push};

#[cfg(test)]
mod tests;

/// The client's fleet constants, re-read from `evefleet/const.py` **[V]**.
pub mod consts {
    /// `fleetRoleLeader` **[V] const.py:11**: the fleet commander's seat.
    pub const ROLE_LEADER: i64 = 1;
    /// `fleetRoleWingCmdr` **[V] const.py:14**.
    pub const ROLE_WING_CMDR: i64 = 2;
    /// `fleetRoleSquadCmdr` **[V] const.py:13**.
    pub const ROLE_SQUAD_CMDR: i64 = 3;
    /// `fleetRoleMember` **[V] const.py:12**.
    pub const ROLE_MEMBER: i64 = 4;
    /// `fleetCmdrRoles` **[V] const.py:15**.
    pub const CMDR_ROLES: [i64; 3] = [ROLE_LEADER, ROLE_WING_CMDR, ROLE_SQUAD_CMDR];

    /// `fleetJobNone` **[V] const.py:8**.
    pub const JOB_NONE: i64 = 0;
    /// `fleetJobCreator` **[V] const.py:7**: the boss bit.
    pub const JOB_CREATOR: i64 = 2;

    /// `FLEET_NONEID` **[V] const.py:20**: "no wing" / "no squad".
    pub const NONE_ID: i64 = -1;

    /// `MAX_WINGS_IN_FLEET` **[V] const.py:29**.
    pub const MAX_WINGS_IN_FLEET: i64 = 25;
    /// `MAX_SQUADS_IN_WING` **[V] const.py:28**.
    pub const MAX_SQUADS_IN_WING: i64 = 25;
    /// `MAX_MEMBERS_IN_SQUAD` **[V] const.py:27**.
    pub const MAX_MEMBERS_IN_SQUAD: i64 = 256;
    /// `MAX_MEMBERS_IN_FLEET` **[V] const.py:26**.
    pub const MAX_MEMBERS_IN_FLEET: i64 = 256;
    /// `MAX_NAME_LENGTH` **[V] const.py:30**: wing and squad names.
    pub const MAX_NAME_LENGTH: usize = 10;

    /// `NODEID_MOD` **[V] const.py:48**.
    pub const NODEID_MOD: i64 = 10_000_000;
    /// `FLEETID_MOD` **[V] const.py:49**.
    pub const FLEETID_MOD: i64 = 10_000;

    /// Every tag the client's menu can put on an item: the digits `0`-`9`
    /// and the letters `ABCDEFGHIJXYZ` **[V] menusvc.py:1928-1929**. The
    /// keyboard commands use a subset of these (`eveCommands.py:612-626`).
    pub const TARGET_TAGS: [&str; 23] = [
        "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "A", "B", "C", "D", "E", "F", "G", "H",
        "I", "J", "X", "Y", "Z",
    ];
}

use consts::*;

/// The FSD dialog an invite opens on the invitee's client: `AskJoinFleet`,
/// a yes/no question taking `name` (`findings/user-errors.tsv`), passed by
/// `OnFleetInvite` to `prompt_player_fsd_dialog` **[V] fleetSvc.py:1136**.
pub const INVITE_DIALOG: &str = "AskJoinFleet";

/// How long a dropped pilot keeps his seat, in FILETIME units.
///
/// The client's own window: `AttemptReconnectLazy` only tries a stored
/// fleet whose stamp is newer than `RECONNECT_TIMEOUT * MIN` **[V]
/// fleetSvc.py:1621**, with `RECONNECT_TIMEOUT = 2` **[V] const.py:84** and
/// `MIN = 60 * 10^7` **[V] carbon/common/lib/const.py:187-188**. Holding the
/// seat for the same span is ours **[A]**; falsified by a client that tries
/// later than this and finds a retail fleet still holding it.
pub const RECONNECT_TIMEOUT: i64 = 2 * 60 * 10_000_000;

// ---------------------------------------------------------------- errors

/// Why a fleet call was refused. Each is one `UserError` key this build
/// ships (`findings/user-errors.tsv`); the three limits carry the `num` the
/// dialog substitutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleetError {
    NotFound,
    NotInFleet,
    NotCreator,
    NotCommanderOrBoss,
    NoPositionFound,
    TooManyMembers(i64),
    TooManyWings(i64),
    TooManySquads(i64),
    CannotDeleteNonEmptyWing,
    CannotDeleteNonEmptySquad,
    CantKickBoss,
    Generic,
}

impl FleetError {
    /// The dialog key.
    pub fn key(&self) -> &'static str {
        match self {
            Self::NotFound => "FleetNotFound",
            Self::NotInFleet => "FleetNotInFleet",
            Self::NotCreator => "FleetNotCreator",
            Self::NotCommanderOrBoss => "FleetNotCommanderOrBoss",
            Self::NoPositionFound => "FleetNoPositionFound",
            Self::TooManyMembers(_) => "FleetTooManyMembers",
            Self::TooManyWings(_) => "FleetTooManyWings",
            Self::TooManySquads(_) => "FleetTooManySquads",
            Self::CannotDeleteNonEmptyWing => "FleetCannotDeleteNonEmptyWing",
            Self::CannotDeleteNonEmptySquad => "FleetCannotDeleteNonEmptySquad",
            Self::CantKickBoss => "FleetCantKickBoss",
            Self::Generic => "FleetError",
        }
    }

    /// The dialog's arguments, as marshal dict entries.
    pub fn args(&self) -> Vec<(PyValue, PyValue)> {
        match self {
            Self::TooManyMembers(n) | Self::TooManyWings(n) | Self::TooManySquads(n) => {
                crate::pyutil::pypairs(vec![("num", PyValue::Int(*n))])
            }
            _ => Vec::new(),
        }
    }

    /// The `UserError` exception the client is sent.
    pub fn to_user_error(self) -> PyValue {
        eve_proto::user_error(self.key(), self.args())
    }
}

// ---------------------------------------------------------------- the data

/// A member's three opt-outs. All on for a new member, as the client's own
/// constructor builds them **[V] evefleet/members.py:8-9**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberOptOuts {
    pub accepts_fleet_warp: bool,
    pub accepts_conduit_jumps: bool,
    pub accepts_fleet_regroups: bool,
}

impl Default for MemberOptOuts {
    fn default() -> Self {
        Self {
            accepts_fleet_warp: true,
            accepts_conduit_jumps: true,
            accepts_fleet_regroups: true,
        }
    }
}

/// One pilot in a fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub char_id: i64,
    /// The wing he sits in or commands, else [`NONE_ID`].
    pub wing_id: i64,
    /// The squad he sits in or commands, else [`NONE_ID`].
    pub squad_id: i64,
    pub role: i64,
    pub job: i64,
    /// Fleet-bonus skill levels. Always zero here **[A]**: nothing in this
    /// server computes them yet; falsified by a client view that shows them.
    pub skills: [i64; 3],
    /// When he joined, as a FILETIME.
    pub timestamp: i64,
    pub station_id: Option<i64>,
    pub ship_type_id: Option<i64>,
    pub solar_system_id: Option<i64>,
    pub opt_outs: MemberOptOuts,
}

impl Member {
    /// Whether he holds the boss bit.
    pub fn is_boss(&self) -> bool {
        self.job & JOB_CREATOR != 0
    }

    /// Whether he sits in one of the three commander seats.
    pub fn is_commander(&self) -> bool {
        CMDR_ROLES.contains(&self.role)
    }

    /// The five fields `OnFleetMemberChanged` reports old and new.
    pub fn slot(&self) -> Slot {
        Slot {
            wing_id: self.wing_id,
            squad_id: self.squad_id,
            role: self.role,
            job: self.job,
            opt_outs: self.opt_outs,
        }
    }
}

/// A member's position and flags, before or after a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub wing_id: i64,
    pub squad_id: i64,
    pub role: i64,
    pub job: i64,
    pub opt_outs: MemberOptOuts,
}

/// A squad. Its wing is the one whose map holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Squad {
    pub squad_id: i64,
    pub name: String,
}

/// A wing and its squads, keyed by squad id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wing {
    pub wing_id: i64,
    pub name: String,
    pub squads: BTreeMap<i64, Squad>,
}

/// The fleet's options, the three attributes the client keeps
/// **[V] fleetSvc.py:153**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub is_free_move: bool,
    pub is_registered: bool,
    pub auto_join_squad_id: Option<i64>,
}

/// One fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fleet {
    pub fleet_id: i64,
    /// Who called `CreateFleet`; the only pilot `Init` will seat.
    pub creator: i64,
    pub motd: String,
    pub options: Options,
    pub members: BTreeMap<i64, Member>,
    pub wings: BTreeMap<i64, Wing>,
    pub max_size: usize,
    /// Item id to tag.
    pub target_tags: BTreeMap<i64, String>,
    /// Members whose client dropped, to the FILETIME their seat runs out.
    pub gone: BTreeMap<i64, i64>,
}

impl Fleet {
    /// Everyone who should hear about this fleet now: the members who are
    /// online, by character id.
    pub fn audience(&self) -> Vec<i64> {
        self.members
            .keys()
            .copied()
            .filter(|c| !self.gone.contains_key(c))
            .collect()
    }

    /// The member with the boss bit.
    pub fn boss(&self) -> Option<&Member> {
        self.members.values().find(|m| m.is_boss())
    }

    /// The lowest wing that has a squad and its lowest squad; else the
    /// lowest wing and no squad; else nothing.
    pub fn first_wing_and_squad(&self) -> (Option<i64>, Option<i64>) {
        self.wings
            .values()
            .find_map(|w| w.squads.keys().next().map(|s| (Some(w.wing_id), Some(*s))))
            .unwrap_or_else(|| (self.wings.keys().next().copied(), None))
    }

    /// The wing holding `squad_id`.
    fn wing_of_squad(&self, squad_id: i64) -> Option<i64> {
        self.wings
            .values()
            .find(|w| w.squads.contains_key(&squad_id))
            .map(|w| w.wing_id)
    }

    /// Where a member with no position asked for goes: the auto-join squad
    /// when it still exists, else the first squad.
    fn default_squad(&self) -> Option<i64> {
        self.options
            .auto_join_squad_id
            .filter(|s| self.wing_of_squad(*s).is_some())
            .or(self.first_wing_and_squad().1)
    }

    /// How many sit in `squad_id`, not counting `except`.
    fn headcount(&self, squad_id: i64, except: i64) -> i64 {
        self.members
            .values()
            .filter(|m| m.squad_id == squad_id && m.char_id != except)
            .count() as i64
    }

    /// Whether anyone other than `who` holds the commander seat `role`
    /// over `wing_id` / `squad_id`.
    fn seat_taken(&self, who: i64, role: i64, wing_id: i64, squad_id: i64) -> bool {
        self.members.values().any(|m| {
            m.char_id != who
                && m.role == role
                && match role {
                    ROLE_LEADER => true,
                    ROLE_WING_CMDR => m.wing_id == wing_id,
                    ROLE_SQUAD_CMDR => m.squad_id == squad_id,
                    _ => false,
                }
        })
    }

    /// Resolve a request to seat `who` as `(wing, squad, role)`.
    ///
    /// `now` is his current slot, if he is a member. An absent id, `0` or
    /// [`NONE_ID`] means "unspecified"; any other negative id is not a
    /// position. An unspecified role is a plain member when a wing or squad
    /// is named, and his current role otherwise.
    fn place(
        &self,
        who: i64,
        now: Option<Slot>,
        wing: Option<i64>,
        squad: Option<i64>,
        role: Option<i64>,
    ) -> Result<(i64, i64, i64), FleetError> {
        let named = |id: Option<i64>| match id {
            None | Some(0) | Some(NONE_ID) => Ok(None),
            Some(n) if n < 0 => Err(FleetError::NoPositionFound),
            Some(n) => Ok(Some(n)),
        };
        let wing = named(wing)?;
        let squad = named(squad)?;
        let role = match role {
            None | Some(0) | Some(NONE_ID) => None,
            Some(r) if (ROLE_LEADER..=ROLE_MEMBER).contains(&r) => Some(r),
            Some(_) => return Err(FleetError::Generic),
        };
        let my_wing = now.map(|s| s.wing_id).filter(|w| *w > 0);
        let my_squad = now.map(|s| s.squad_id).filter(|s| *s > 0);
        let role = role.unwrap_or(if wing.is_some() || squad.is_some() {
            ROLE_MEMBER
        } else {
            now.map_or(ROLE_MEMBER, |s| s.role)
        });
        // A squad seat must have room, unless he is already in that squad.
        let room = |s: i64| {
            if my_squad != Some(s) && self.headcount(s, who) >= MAX_MEMBERS_IN_SQUAD {
                Err(FleetError::TooManyMembers(MAX_MEMBERS_IN_SQUAD))
            } else {
                Ok(())
            }
        };
        match role {
            ROLE_LEADER => {
                if self.seat_taken(who, ROLE_LEADER, NONE_ID, NONE_ID) {
                    return Err(FleetError::Generic);
                }
                Ok((NONE_ID, NONE_ID, ROLE_LEADER))
            }
            ROLE_WING_CMDR => {
                let w = wing
                    .or_else(|| squad.and_then(|s| self.wing_of_squad(s)))
                    .or(my_wing)
                    .filter(|w| self.wings.contains_key(w))
                    .ok_or(FleetError::NoPositionFound)?;
                if self.seat_taken(who, ROLE_WING_CMDR, w, NONE_ID) {
                    return Err(FleetError::Generic);
                }
                Ok((w, NONE_ID, ROLE_WING_CMDR))
            }
            ROLE_SQUAD_CMDR => {
                let s = squad.or(my_squad).ok_or(FleetError::NoPositionFound)?;
                let w = self.wing_of_squad(s).ok_or(FleetError::NoPositionFound)?;
                if wing.is_some_and(|x| x != w) {
                    return Err(FleetError::NoPositionFound);
                }
                if self.seat_taken(who, ROLE_SQUAD_CMDR, w, s) {
                    return Err(FleetError::Generic);
                }
                room(s)?;
                Ok((w, s, ROLE_SQUAD_CMDR))
            }
            _ => {
                let s = match (squad, wing) {
                    (Some(s), _) => s,
                    (None, Some(w)) => {
                        let in_wing = &self
                            .wings
                            .get(&w)
                            .ok_or(FleetError::NoPositionFound)?
                            .squads;
                        my_squad
                            .filter(|s| in_wing.contains_key(s))
                            .or_else(|| in_wing.keys().next().copied())
                            .ok_or(FleetError::NoPositionFound)?
                    }
                    (None, None) => my_squad
                        .or_else(|| self.default_squad())
                        .ok_or(FleetError::NoPositionFound)?,
                };
                let w = self.wing_of_squad(s).ok_or(FleetError::NoPositionFound)?;
                if wing.is_some_and(|x| x != w) {
                    return Err(FleetError::NoPositionFound);
                }
                room(s)?;
                Ok((w, s, ROLE_MEMBER))
            }
        }
    }

    /// Whether `me` may delete a squad in the wing `wing_id`: the boss and
    /// the fleet commander anywhere, any other commander inside his own
    /// wing. The client's squad menu offers rename and delete to every
    /// commander and the boss **[V] ui/shared/fleet/fleet.py:79-87**.
    fn may_edit_wing(me: &Member, wing_id: i64) -> bool {
        me.is_boss() || me.role == ROLE_LEADER || (me.is_commander() && me.wing_id == wing_id)
    }

    /// Tell the fleet its options moved from `old`, if they did. Deleting
    /// the default squad (or its wing) clears it too, and the client keeps
    /// the options it was last told and sends them back whole from
    /// `SetOptions` **[V] fleetSvc.py:436-440**, so a silent clear would make
    /// the boss's next free-move toggle name a squad that is gone.
    fn tell_options_changed(&self, old: Options, fx: &mut Effects) {
        let new = self.options;
        if old != new {
            fx.tell(self.audience(), FleetEvent::OptionsChanged { old, new });
        }
    }

    /// Whether `me` may shape the wing `wing_id` itself (add a squad to it,
    /// rename it): the boss, the fleet commander, or that wing's own
    /// commander. The client's wing menu, which holds both entries, is
    /// offered to the boss, the fleet commander and wing commanders only
    /// **[V] ui/shared/fleet/fleet.py:916-917**; a wing commander reaching
    /// into another wing is ours to refuse.
    fn may_shape_wing(me: &Member, wing_id: i64) -> bool {
        me.is_boss()
            || me.role == ROLE_LEADER
            || (me.role == ROLE_WING_CMDR && me.wing_id == wing_id)
    }
}

/// A pilot's open jump beacon (a cynosural field module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Beacon {
    pub solar_system_id: i64,
    pub beacon_id: i64,
    pub type_id: i64,
}

/// A pilot's open jump bridge (a portal generator).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bridge {
    pub ship_id: i64,
    pub solar_system_id: i64,
    pub beacon_id: i64,
}

/// An invite not yet answered. The position is resolved on accept, against
/// the fleet as it is then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invite {
    pub fleet_id: i64,
    pub inviter_id: i64,
    pub wing_id: Option<i64>,
    pub squad_id: Option<i64>,
    pub role: Option<i64>,
}

/// The four fleet session attributes one character should now hold.
/// `None` everywhere is "in no fleet".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionFleetState {
    pub character_id: i64,
    pub fleet_id: Option<i64>,
    pub fleet_role: Option<i64>,
    pub wing_id: Option<i64>,
    pub squad_id: Option<i64>,
}

impl SessionFleetState {
    fn cleared(character_id: i64) -> Self {
        Self {
            character_id,
            fleet_id: None,
            fleet_role: None,
            wing_id: None,
            squad_id: None,
        }
    }

    fn seated(fleet_id: i64, m: &Member) -> Self {
        Self {
            character_id: m.char_id,
            fleet_id: Some(fleet_id),
            fleet_role: Some(m.role),
            wing_id: Some(m.wing_id).filter(|w| *w > 0),
            squad_id: Some(m.squad_id).filter(|s| *s > 0),
        }
    }
}

/// What a mutator wants done: notifications and session changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Effects {
    pub pushes: Vec<Push>,
    pub sessions: Vec<SessionFleetState>,
}

impl Effects {
    /// Queue `event` for `to`; nobody to tell is nothing to send.
    fn tell(&mut self, to: Vec<i64>, event: FleetEvent) {
        if !to.is_empty() {
            self.pushes.push(Push { to, event });
        }
    }

    fn absorb(&mut self, other: Effects) {
        self.pushes.extend(other.pushes);
        self.sessions.extend(other.sessions);
    }
}

/// How a member came to leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    /// He left, or was kicked: he is online and is told.
    Present,
    /// His reconnect window ran out: there is no socket to tell.
    TimedOut,
}

fn beacon_event(char_id: i64, b: Beacon, active: bool) -> FleetEvent {
    FleetEvent::BeaconChange {
        char_id,
        solar_system_id: b.solar_system_id,
        beacon_id: b.beacon_id,
        type_id: b.type_id,
        active,
    }
}

fn bridge_event(b: Bridge, active: bool) -> FleetEvent {
    FleetEvent::BridgeChange {
        ship_id: b.ship_id,
        solar_system_id: b.solar_system_id,
        beacon_id: b.beacon_id,
        active,
    }
}

// ---------------------------------------------------------------- Fleets

/// Every fleet on this node, and everything keyed by a pilot that the
/// fleets need: seats, invites, beacons and bridges.
#[derive(Debug, Clone)]
pub struct Fleets {
    node_id: i64,
    /// The last fleet serial handed out.
    serial: i64,
    /// The last wing/squad id handed out.
    last_group_id: i64,
    fleets: BTreeMap<i64, Fleet>,
    /// Character to the fleet he sits in.
    seats: BTreeMap<i64, i64>,
    /// Invitee to his open invite.
    invites: BTreeMap<i64, Invite>,
    /// Pilot to his lit beacon, fleet or no fleet.
    beacons: BTreeMap<i64, Beacon>,
    /// Pilot to his open bridge, fleet or no fleet.
    bridges: BTreeMap<i64, Bridge>,
    /// Session changes waiting for a turn to carry them.
    pending_sessions: Vec<SessionFleetState>,
    limiter: crate::fleet_broadcast::Limiter,
}

impl Fleets {
    pub fn new(node_id: i64) -> Self {
        Self {
            node_id,
            serial: 0,
            last_group_id: 0,
            fleets: BTreeMap::new(),
            seats: BTreeMap::new(),
            invites: BTreeMap::new(),
            beacons: BTreeMap::new(),
            bridges: BTreeMap::new(),
            pending_sessions: Vec::new(),
            limiter: crate::fleet_broadcast::Limiter::default(),
        }
    }

    // ------------------------------------------------------------ lookups

    pub fn fleet(&self, fleet_id: i64) -> Option<&Fleet> {
        self.fleets.get(&fleet_id)
    }

    pub fn len(&self) -> usize {
        self.fleets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fleets.is_empty()
    }

    /// The fleet `char_id` sits in, his reconnect window included.
    pub fn fleet_of(&self, char_id: i64) -> Option<&Fleet> {
        self.seats.get(&char_id).and_then(|id| self.fleets.get(id))
    }

    /// The fleet a call from `char_id` is about when it names none: his
    /// own, else the one he created and has not yet been seated in.
    pub fn resolve_for(&self, char_id: i64) -> Option<i64> {
        self.fleet_of(char_id).map(|f| f.fleet_id).or_else(|| {
            self.fleets
                .values()
                .find(|f| f.creator == char_id && f.members.is_empty())
                .map(|f| f.fleet_id)
        })
    }

    pub fn invite_of(&self, char_id: i64) -> Option<&Invite> {
        self.invites.get(&char_id)
    }

    /// The fleet and the acting member, or why not.
    fn seat_of(&self, actor: i64, fleet_id: i64) -> Result<(&Fleet, &Member), FleetError> {
        let fleet = self.fleets.get(&fleet_id).ok_or(FleetError::NotFound)?;
        let me = fleet.members.get(&actor).ok_or(FleetError::NotInFleet)?;
        Ok((fleet, me))
    }

    /// The next wing or squad id: see the module docs.
    fn next_group_id(&mut self) -> i64 {
        loop {
            self.last_group_id += 1;
            if self.last_group_id % FLEETID_MOD != 0 {
                return self.last_group_id;
            }
        }
    }

    // ------------------------------------------------------ session queue

    pub fn queue_session_changes(&mut self, states: Vec<SessionFleetState>) {
        self.pending_sessions.extend(states);
    }

    pub fn take_session_changes(&mut self) -> Vec<SessionFleetState> {
        std::mem::take(&mut self.pending_sessions)
    }

    pub fn has_session_changes(&self) -> bool {
        !self.pending_sessions.is_empty()
    }

    pub fn broadcast_limiter_mut(&mut self) -> &mut crate::fleet_broadcast::Limiter {
        &mut self.limiter
    }

    // ---------------------------------------------------------- lifecycle

    /// `CreateFleet()`: a new fleet with one wing holding one squad, and
    /// nobody in it. The creator is seated by `Init`, which the client calls
    /// on the object it was just handed **[V] fleetSvc.py:329-331**.
    ///
    /// A pilot who sits in a fleet is refused, as the client refuses
    /// itself **[V] fleetSvc.py:327-328**. A fleet he created earlier and
    /// never entered is dropped: nobody can reach it any more.
    pub fn create(&mut self, creator: i64) -> Result<i64, FleetError> {
        if self.seats.contains_key(&creator) {
            return Err(FleetError::Generic);
        }
        self.fleets
            .retain(|_, f| !(f.creator == creator && f.members.is_empty()));
        self.serial += 1;
        let fleet_id = self.node_id * NODEID_MOD + self.serial * FLEETID_MOD;
        let wing_id = self.next_group_id();
        let squad_id = self.next_group_id();
        let squad = Squad {
            squad_id,
            name: String::new(),
        };
        let wing = Wing {
            wing_id,
            name: String::new(),
            squads: BTreeMap::from([(squad_id, squad)]),
        };
        self.fleets.insert(
            fleet_id,
            Fleet {
                fleet_id,
                creator,
                motd: String::new(),
                options: Options::default(),
                members: BTreeMap::new(),
                wings: BTreeMap::from([(wing_id, wing)]),
                max_size: MAX_MEMBERS_IN_FLEET as usize,
                target_tags: BTreeMap::new(),
                gone: BTreeMap::new(),
            },
        );
        Ok(fleet_id)
    }

    /// `Init(shipTypeID, ...)`: seat the creator in the fleet commander's
    /// seat with the boss bit. A member calling it again changes nothing;
    /// anyone else is refused.
    pub fn init(
        &mut self,
        actor: i64,
        fleet_id: i64,
        ship_type_id: Option<i64>,
        station_id: Option<i64>,
        solar_system_id: Option<i64>,
    ) -> Result<Effects, FleetError> {
        let fleet = self.fleets.get(&fleet_id).ok_or(FleetError::NotFound)?;
        if fleet.members.contains_key(&actor) {
            return Ok(Effects::default());
        }
        if fleet.creator != actor || !fleet.members.is_empty() || self.seats.contains_key(&actor) {
            return Err(FleetError::Generic);
        }
        let member = Member {
            char_id: actor,
            wing_id: NONE_ID,
            squad_id: NONE_ID,
            role: ROLE_LEADER,
            job: JOB_CREATOR,
            skills: [0; 3],
            timestamp: crate::pyutil::filetime_now(),
            station_id,
            ship_type_id,
            solar_system_id,
            opt_outs: MemberOptOuts::default(),
        };
        Ok(self.seat(fleet_id, member))
    }

    /// Put `member` into the fleet and tell everyone what they need: the
    /// others hear `OnFleetJoin`, the newcomer gets his session, and each
    /// side learns the other's open beacons, bridges and the tags.
    ///
    /// `OnFleetJoin` skips the joiner: his own client re-reads the whole
    /// fleet when the member is himself **[V] fleetSvc.py:1147-1150**.
    fn seat(&mut self, fleet_id: i64, member: Member) -> Effects {
        let who = member.char_id;
        let mut fx = Effects::default();
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return fx;
        };
        let others: Vec<i64> = fleet.audience();
        fx.sessions
            .push(SessionFleetState::seated(fleet_id, &member));
        fleet.members.insert(who, member.clone());
        self.seats.insert(who, fleet_id);
        self.invites.remove(&who);
        fx.tell(others.clone(), FleetEvent::Join { member });
        if let Some(b) = self.beacons.get(&who) {
            fx.tell(others.clone(), beacon_event(who, *b, true));
        }
        if let Some(b) = self.bridges.get(&who) {
            fx.tell(others, bridge_event(*b, true));
        }
        fx.absorb(self.replay_to(fleet_id, who));
        fx
    }

    /// The fleet's open beacons, bridges and tags, told to `who` alone.
    fn replay_to(&self, fleet_id: i64, who: i64) -> Effects {
        let mut fx = Effects::default();
        let Some(fleet) = self.fleets.get(&fleet_id) else {
            return fx;
        };
        for c in fleet.members.keys().filter(|c| **c != who) {
            if let Some(b) = self.beacons.get(c) {
                fx.tell(vec![who], beacon_event(*c, *b, true));
            }
        }
        for c in fleet.members.keys().filter(|c| **c != who) {
            if let Some(b) = self.bridges.get(c) {
                fx.tell(vec![who], bridge_event(*b, true));
            }
        }
        if !fleet.target_tags.is_empty() {
            fx.tell(
                vec![who],
                FleetEvent::State {
                    tags: fleet.target_tags.clone(),
                },
            );
        }
        fx
    }

    /// `Invite(charID, wingID, squadID, role)`: offer `invitee` a seat.
    ///
    /// The push goes to the invitee alone, who is by definition in no
    /// fleet. A commander or the boss may invite; a pilot who already sits
    /// in a fleet cannot be invited. A position, when named, must be inside
    /// the inviter's command, and is checked now and again on accept.
    pub fn invite(
        &mut self,
        actor: i64,
        fleet_id: i64,
        invitee: i64,
        wing_id: Option<i64>,
        squad_id: Option<i64>,
        role: Option<i64>,
    ) -> Result<(bool, Effects), FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        if !(me.is_boss() || me.is_commander()) {
            return Err(FleetError::NotCommanderOrBoss);
        }
        if invitee == actor || self.seats.contains_key(&invitee) {
            return Err(FleetError::Generic);
        }
        if fleet.members.len() >= fleet.max_size {
            return Err(FleetError::TooManyMembers(fleet.max_size as i64));
        }
        let (w, s, r) = fleet.place(invitee, None, wing_id, squad_id, role)?;
        // A named seat must be inside the inviter's command, as for a move:
        // `CanMoveToSquad` lets a wing commander drop a pilot from outside
        // the fleet only into his own wing, and a squad commander only into
        // his own squad **[V] fleetSvc.py:2004-2011**, and the fleet and wing
        // seats need the boss or the fleet commander (`CanMoveToThisEntry`
        // **[V] fleetSvc.py:2043-2056**). No seat named is the plain "invite
        // to fleet" any commander may send.
        let named = [wing_id, squad_id, role]
            .iter()
            .any(|v| matches!(v, Some(n) if *n != 0 && *n != NONE_ID));
        if named && !(me.is_boss() || me.role == ROLE_LEADER) {
            let inside = match me.role {
                ROLE_WING_CMDR => w == me.wing_id && (r == ROLE_MEMBER || r == ROLE_SQUAD_CMDR),
                ROLE_SQUAD_CMDR => s == me.squad_id && r == ROLE_MEMBER,
                _ => false,
            };
            if !inside {
                return Err(FleetError::NotCommanderOrBoss);
            }
        }
        self.invites.insert(
            invitee,
            Invite {
                fleet_id,
                inviter_id: actor,
                wing_id,
                squad_id,
                role,
            },
        );
        let mut fx = Effects::default();
        fx.tell(
            vec![invitee],
            FleetEvent::Invite {
                fleet_id,
                inviter_id: actor,
                msg_name: INVITE_DIALOG.to_owned(),
                auto_accept: false,
            },
        );
        Ok((true, fx))
    }

    /// `AcceptInvite(shipTypeID)`: take the seat the invite offered.
    ///
    /// The prompt is not modal **[V] fleetSvc.py:1136** (`modal=False`), so
    /// the invitee may have joined another fleet meanwhile: that is refused
    /// and the stale invite dropped.
    pub fn accept_invite(
        &mut self,
        actor: i64,
        fleet_id: i64,
        ship_type_id: Option<i64>,
        station_id: Option<i64>,
        solar_system_id: Option<i64>,
    ) -> Result<(bool, Effects), FleetError> {
        if self.seats.contains_key(&actor) {
            self.invites.remove(&actor);
            return Err(FleetError::Generic);
        }
        let invite = match self.invites.get(&actor) {
            Some(i) if i.fleet_id == fleet_id => *i,
            _ => return Err(FleetError::NotFound),
        };
        let Some(fleet) = self.fleets.get(&fleet_id) else {
            self.invites.remove(&actor);
            return Err(FleetError::NotFound);
        };
        let placed = if fleet.members.len() >= fleet.max_size {
            Err(FleetError::TooManyMembers(fleet.max_size as i64))
        } else {
            fleet.place(actor, None, invite.wing_id, invite.squad_id, invite.role)
        };
        let (wing_id, squad_id, role) = match placed {
            Ok(p) => p,
            Err(e) => {
                self.invites.remove(&actor);
                return Err(e);
            }
        };
        let member = Member {
            char_id: actor,
            wing_id,
            squad_id,
            role,
            job: JOB_NONE,
            skills: [0; 3],
            timestamp: crate::pyutil::filetime_now(),
            station_id,
            ship_type_id,
            solar_system_id,
            opt_outs: MemberOptOuts::default(),
        };
        Ok((true, self.seat(fleet_id, member)))
    }

    /// `RejectInvite()`: drop the invite, answering who sent it.
    pub fn reject_invite(&mut self, actor: i64, fleet_id: i64) -> Option<i64> {
        match self.invites.get(&actor) {
            Some(i) if i.fleet_id == fleet_id => self.invites.remove(&actor).map(|i| i.inviter_id),
            _ => None,
        }
    }

    /// `LeaveFleet()`.
    ///
    /// `OnFleetLeave` goes to the whole fleet, the leaver too: his own
    /// client clears its fleet state when the id is his
    /// **[V] fleetSvc.py:1170-1174**. The last member out closes the fleet.
    pub fn leave(&mut self, actor: i64, fleet_id: i64) -> Result<Effects, FleetError> {
        let fleet = self.fleets.get(&fleet_id).ok_or(FleetError::NotFound)?;
        if !fleet.members.contains_key(&actor) {
            if fleet.creator == actor && fleet.members.is_empty() {
                self.fleets.remove(&fleet_id);
                return Ok(Effects::default());
            }
            return Err(FleetError::NotInFleet);
        }
        Ok(self.depart(fleet_id, actor, Exit::Present))
    }

    /// `KickMember(charID)`.
    ///
    /// Kicking oneself is leaving, which is what the client's own menu does
    /// **[V] fleetSvc.py:587-591**. Otherwise the actor needs a commander
    /// seat or the boss bit, the target must be under him, and nobody but
    /// the boss himself removes the boss. A target who is not a member is
    /// nothing to do.
    pub fn kick(
        &mut self,
        actor: i64,
        fleet_id: i64,
        target: i64,
    ) -> Result<(bool, Effects), FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        if target == actor {
            return Ok((true, self.depart(fleet_id, actor, Exit::Present)));
        }
        if !(me.is_boss() || me.is_commander()) {
            return Err(FleetError::NotCommanderOrBoss);
        }
        let Some(them) = fleet.members.get(&target) else {
            return Ok((false, Effects::default()));
        };
        if them.is_boss() {
            return Err(FleetError::CantKickBoss);
        }
        let reaches = me.is_boss()
            || me.role == ROLE_LEADER
            || (me.role == ROLE_WING_CMDR && them.wing_id == me.wing_id)
            || (me.role == ROLE_SQUAD_CMDR
                && them.squad_id == me.squad_id
                && them.role == ROLE_MEMBER);
        if !reaches {
            return Err(FleetError::NotCommanderOrBoss);
        }
        Ok((true, self.depart(fleet_id, target, Exit::Present)))
    }

    /// Take `who` out of `fleet_id` and say so.
    ///
    /// The others hear `OnFleetLeave` and lose sight of his beacon and
    /// bridge. If he held the boss bit, it passes on. If nobody is left the
    /// fleet closes; a pilot who is present then gets `OnFleetDisbanded`
    /// naming himself.
    fn depart(&mut self, fleet_id: i64, who: i64, exit: Exit) -> Effects {
        let mut fx = Effects::default();
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return fx;
        };
        let Some(was) = fleet.members.remove(&who) else {
            return fx;
        };
        fleet.gone.remove(&who);
        let emptied = fleet.members.is_empty();
        let rest = fleet.audience();
        self.seats.remove(&who);
        if exit == Exit::Present {
            fx.sessions.push(SessionFleetState::cleared(who));
        }
        if emptied {
            self.close(fleet_id);
            if exit == Exit::Present {
                fx.tell(
                    vec![who],
                    FleetEvent::Disbanded {
                        char_ids: vec![who],
                    },
                );
            }
            return fx;
        }
        let mut told = rest.clone();
        if exit == Exit::Present {
            told.push(who);
            told.sort_unstable();
        }
        fx.tell(told, FleetEvent::Leave { char_id: who });
        if let Some(b) = self.beacons.get(&who) {
            fx.tell(rest.clone(), beacon_event(who, *b, false));
        }
        if let Some(b) = self.bridges.get(&who) {
            fx.tell(rest, bridge_event(*b, false));
        }
        if was.is_boss() {
            fx.absorb(self.hand_on_boss(fleet_id));
        }
        fx
    }

    /// Give the boss bit to the lowest character id still online, else to
    /// the lowest of those whose window is still open **[A]**. Falsified by
    /// a retail fleet that hands the job to someone else (the fleet
    /// commander, the longest-serving member).
    fn hand_on_boss(&mut self, fleet_id: i64) -> Effects {
        let mut fx = Effects::default();
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return fx;
        };
        let audience = fleet.audience();
        let Some(heir) = audience
            .first()
            .copied()
            .or_else(|| fleet.members.keys().next().copied())
        else {
            return fx;
        };
        let alone = fleet.members.len() == 1;
        let Some(m) = fleet.members.get_mut(&heir) else {
            return fx;
        };
        let old = m.slot();
        m.job |= JOB_CREATOR;
        let new = m.slot();
        fx.tell(
            audience,
            FleetEvent::MemberChanged {
                char_id: heir,
                fleet_id,
                old,
                new,
                is_only_member: alone,
            },
        );
        fx
    }

    /// Forget a fleet and everything pointing into it.
    fn close(&mut self, fleet_id: i64) {
        if let Some(fleet) = self.fleets.remove(&fleet_id) {
            for c in fleet.members.keys() {
                self.seats.remove(c);
            }
        }
        self.invites.retain(|_, i| i.fleet_id != fleet_id);
    }

    /// `DisbandFleet()`: the boss ends the fleet.
    ///
    /// `OnFleetDisbanded(charIDs)` lists everyone, the boss included: the
    /// client runs its leave handler for each id, and the one that is its
    /// own clears the window **[V] fleetSvc.py:1193-1195**.
    pub fn disband(&mut self, actor: i64, fleet_id: i64) -> Result<Effects, FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        if !me.is_boss() {
            return Err(FleetError::NotCreator);
        }
        let char_ids: Vec<i64> = fleet.members.keys().copied().collect();
        let audience = fleet.audience();
        let mut fx = Effects::default();
        fx.tell(
            audience,
            FleetEvent::Disbanded {
                char_ids: char_ids.clone(),
            },
        );
        fx.sessions
            .extend(char_ids.iter().map(|c| SessionFleetState::cleared(*c)));
        self.close(fleet_id);
        Ok(fx)
    }

    /// `MakeLeader(charID)`: the boss hands the boss bit to `target`.
    ///
    /// Only the job moves; both members keep their seats. Each member's
    /// change is its own `OnFleetMemberChanged`, which is how the client's
    /// history records "is now / is no longer the boss"
    /// **[V] fleetSvc.py:1209-1214**.
    pub fn make_leader(
        &mut self,
        actor: i64,
        fleet_id: i64,
        target: i64,
    ) -> Result<(bool, Effects), FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        if !me.is_boss() {
            return Err(FleetError::NotCreator);
        }
        if !fleet.members.contains_key(&target) {
            return Err(FleetError::NotInFleet);
        }
        if target == actor {
            return Ok((true, Effects::default()));
        }
        let mut fx = Effects::default();
        for (who, on) in [(actor, false), (target, true)] {
            fx.absorb(self.update_member(fleet_id, who, |m| {
                if on {
                    m.job |= JOB_CREATOR;
                } else {
                    m.job &= !JOB_CREATOR;
                }
            }));
        }
        Ok((true, fx))
    }

    /// Change one member with `edit` and report it: `OnFleetMemberChanged`
    /// to the fleet, and his session when his seat moved. A change that
    /// changes nothing reports nothing.
    fn update_member(
        &mut self,
        fleet_id: i64,
        who: i64,
        edit: impl FnOnce(&mut Member),
    ) -> Effects {
        let mut fx = Effects::default();
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return fx;
        };
        let alone = fleet.members.len() == 1;
        let Some(m) = fleet.members.get_mut(&who) else {
            return fx;
        };
        let old = m.slot();
        edit(m);
        let new = m.slot();
        if old == new {
            return fx;
        }
        let moved = (old.wing_id, old.squad_id, old.role) != (new.wing_id, new.squad_id, new.role);
        let session = SessionFleetState::seated(fleet_id, m);
        fx.tell(
            fleet.audience(),
            FleetEvent::MemberChanged {
                char_id: who,
                fleet_id,
                old,
                new,
                is_only_member: alone,
            },
        );
        if moved && !fleet.gone.contains_key(&who) {
            fx.sessions.push(session);
        }
        fx
    }

    /// `MoveMember(charID, wingID, squadID, role)`.
    ///
    /// Who may move whom is in the module docs. A seat that is already
    /// taken (a second fleet commander, a second commander of one wing or
    /// squad) is `FleetError`; a position that does not exist is
    /// `FleetNoPositionFound`; a move to where he already is answers
    /// `false` and sends nothing.
    pub fn move_member(
        &mut self,
        actor: i64,
        fleet_id: i64,
        target: i64,
        wing_id: Option<i64>,
        squad_id: Option<i64>,
        role: Option<i64>,
    ) -> Result<(bool, Effects), FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        let free_self = target == actor && fleet.options.is_free_move;
        let anywhere = me.is_boss() || me.role == ROLE_LEADER;
        if !(anywhere || me.is_commander() || free_self) {
            return Err(FleetError::NotCommanderOrBoss);
        }
        let them = fleet.members.get(&target).ok_or(FleetError::NotInFleet)?;
        let (w, s, r) = fleet.place(target, Some(them.slot()), wing_id, squad_id, role)?;
        if !anywhere {
            let squad_seat = r == ROLE_MEMBER || r == ROLE_SQUAD_CMDR;
            let in_my_wing = me.role == ROLE_WING_CMDR
                && them.wing_id == me.wing_id
                && w == me.wing_id
                && squad_seat;
            if !((free_self && squad_seat) || in_my_wing) {
                return Err(FleetError::NotCommanderOrBoss);
            }
        }
        if (w, s, r) == (them.wing_id, them.squad_id, them.role) {
            return Ok((false, Effects::default()));
        }
        let fx = self.update_member(fleet_id, target, |m| {
            m.wing_id = w;
            m.squad_id = s;
            m.role = r;
        });
        Ok((true, fx))
    }

    // ------------------------------------------------- wings and squads

    /// `CreateWing()`: the boss adds an empty wing (see the module docs for
    /// why it has no squad).
    pub fn create_wing(&mut self, actor: i64, fleet_id: i64) -> Result<(i64, Effects), FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        if !me.is_boss() {
            return Err(FleetError::NotCreator);
        }
        if fleet.wings.len() as i64 >= MAX_WINGS_IN_FLEET {
            return Err(FleetError::TooManyWings(MAX_WINGS_IN_FLEET));
        }
        let wing_id = self.next_group_id();
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return Err(FleetError::NotFound);
        };
        fleet.wings.insert(
            wing_id,
            Wing {
                wing_id,
                name: String::new(),
                squads: BTreeMap::new(),
            },
        );
        let mut fx = Effects::default();
        fx.tell(fleet.audience(), FleetEvent::WingAdded { wing_id });
        Ok((wing_id, fx))
    }

    /// `DeleteWing(wingID)`: the boss removes a wing nobody sits in. An
    /// auto-join squad inside it is cleared with it.
    pub fn delete_wing(
        &mut self,
        actor: i64,
        fleet_id: i64,
        wing_id: i64,
    ) -> Result<Effects, FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        if !me.is_boss() {
            return Err(FleetError::NotCreator);
        }
        if !fleet.wings.contains_key(&wing_id) {
            return Err(FleetError::NoPositionFound);
        }
        if fleet.members.values().any(|m| m.wing_id == wing_id) {
            return Err(FleetError::CannotDeleteNonEmptyWing);
        }
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return Err(FleetError::NotFound);
        };
        let old = fleet.options;
        if let Some(wing) = fleet.wings.remove(&wing_id)
            && fleet
                .options
                .auto_join_squad_id
                .is_some_and(|s| wing.squads.contains_key(&s))
        {
            fleet.options.auto_join_squad_id = None;
        }
        let mut fx = Effects::default();
        fx.tell(fleet.audience(), FleetEvent::WingDeleted { wing_id });
        fleet.tell_options_changed(old, &mut fx);
        Ok(fx)
    }

    /// `ChangeWingName(wingID, name)`, cut to `MAX_NAME_LENGTH` characters
    /// as the client cuts it **[V] fleetSvc.py:400-401**.
    pub fn change_wing_name(
        &mut self,
        actor: i64,
        fleet_id: i64,
        wing_id: i64,
        name: &str,
    ) -> Result<Effects, FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        if !fleet.wings.contains_key(&wing_id) {
            return Err(FleetError::NoPositionFound);
        }
        if !Fleet::may_shape_wing(me, wing_id) {
            return Err(FleetError::NotCommanderOrBoss);
        }
        let name: String = name.chars().take(MAX_NAME_LENGTH).collect();
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return Err(FleetError::NotFound);
        };
        if let Some(w) = fleet.wings.get_mut(&wing_id) {
            w.name.clone_from(&name);
        }
        let mut fx = Effects::default();
        fx.tell(
            fleet.audience(),
            FleetEvent::WingNameChanged { wing_id, name },
        );
        Ok(fx)
    }

    /// `CreateSquad(wingID)`: a wing commander adds a squad to his own
    /// wing; the boss and the fleet commander to any.
    pub fn create_squad(
        &mut self,
        actor: i64,
        fleet_id: i64,
        wing_id: i64,
    ) -> Result<(i64, Effects), FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        if !(me.is_boss() || me.is_commander()) {
            return Err(FleetError::NotCommanderOrBoss);
        }
        let wing = fleet
            .wings
            .get(&wing_id)
            .ok_or(FleetError::NoPositionFound)?;
        if !Fleet::may_shape_wing(me, wing_id) {
            return Err(FleetError::NotCommanderOrBoss);
        }
        if wing.squads.len() as i64 >= MAX_SQUADS_IN_WING {
            return Err(FleetError::TooManySquads(MAX_SQUADS_IN_WING));
        }
        let squad_id = self.next_group_id();
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return Err(FleetError::NotFound);
        };
        if let Some(w) = fleet.wings.get_mut(&wing_id) {
            w.squads.insert(
                squad_id,
                Squad {
                    squad_id,
                    name: String::new(),
                },
            );
        }
        let mut fx = Effects::default();
        fx.tell(
            fleet.audience(),
            FleetEvent::SquadAdded { wing_id, squad_id },
        );
        Ok((squad_id, fx))
    }

    /// `DeleteSquad(squadID)`: remove a squad nobody sits in.
    pub fn delete_squad(
        &mut self,
        actor: i64,
        fleet_id: i64,
        squad_id: i64,
    ) -> Result<Effects, FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        if !(me.is_boss() || me.is_commander()) {
            return Err(FleetError::NotCommanderOrBoss);
        }
        let wing_id = fleet
            .wing_of_squad(squad_id)
            .ok_or(FleetError::NoPositionFound)?;
        if !Fleet::may_edit_wing(me, wing_id) {
            return Err(FleetError::NotCommanderOrBoss);
        }
        if fleet.members.values().any(|m| m.squad_id == squad_id) {
            return Err(FleetError::CannotDeleteNonEmptySquad);
        }
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return Err(FleetError::NotFound);
        };
        if let Some(w) = fleet.wings.get_mut(&wing_id) {
            w.squads.remove(&squad_id);
        }
        let old = fleet.options;
        if fleet.options.auto_join_squad_id == Some(squad_id) {
            fleet.options.auto_join_squad_id = None;
        }
        let mut fx = Effects::default();
        fx.tell(fleet.audience(), FleetEvent::SquadDeleted { squad_id });
        fleet.tell_options_changed(old, &mut fx);
        Ok(fx)
    }

    /// `ChangeSquadName(squadID, name)`, cut like a wing's.
    pub fn change_squad_name(
        &mut self,
        actor: i64,
        fleet_id: i64,
        squad_id: i64,
        name: &str,
    ) -> Result<Effects, FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        let wing_id = fleet
            .wing_of_squad(squad_id)
            .ok_or(FleetError::NoPositionFound)?;
        let own_squad = me.role == ROLE_SQUAD_CMDR && me.squad_id == squad_id;
        let over_it = me.is_boss()
            || me.role == ROLE_LEADER
            || (me.role == ROLE_WING_CMDR && me.wing_id == wing_id);
        if !(own_squad || over_it) {
            return Err(FleetError::NotCommanderOrBoss);
        }
        let name: String = name.chars().take(MAX_NAME_LENGTH).collect();
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return Err(FleetError::NotFound);
        };
        if let Some(s) = fleet
            .wings
            .get_mut(&wing_id)
            .and_then(|w| w.squads.get_mut(&squad_id))
        {
            s.name.clone_from(&name);
        }
        let mut fx = Effects::default();
        fx.tell(
            fleet.audience(),
            FleetEvent::SquadNameChanged { squad_id, name },
        );
        Ok(fx)
    }

    // ------------------------------------------------------------ settings

    /// `SetMotdEx(motd)`: the boss's message, with `reloadMotd` set so the
    /// fleet chat window shows it **[V] fleetSvc.py:1875-1882**.
    pub fn set_motd(
        &mut self,
        actor: i64,
        fleet_id: i64,
        motd: &str,
    ) -> Result<Effects, FleetError> {
        let (_, me) = self.seat_of(actor, fleet_id)?;
        if !me.is_boss() {
            return Err(FleetError::NotCreator);
        }
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return Err(FleetError::NotFound);
        };
        fleet.motd = motd.to_owned();
        let mut fx = Effects::default();
        fx.tell(
            fleet.audience(),
            FleetEvent::MotdChanged {
                motd: motd.to_owned(),
                reload_motd: true,
            },
        );
        Ok(fx)
    }

    /// `SetOptions` / `SetAutoJoinSquadID`: the boss changes free move or
    /// the default squad. `None` leaves a field alone; `Some(None)` clears
    /// the default squad. Answers the options as they now stand.
    ///
    /// The client takes its options only from `InitFleet` and from
    /// `OnFleetOptionsChanged(oldOptions, options)` **[V]
    /// fleetSvc.py:1254-1265**, so a real change is told to the fleet with
    /// both. A call that changes nothing tells nobody **[A]**: falsified by
    /// a retail no-op `SetOptions` that still sends the notification.
    pub fn set_options(
        &mut self,
        actor: i64,
        fleet_id: i64,
        is_free_move: Option<bool>,
        auto_join_squad_id: Option<Option<i64>>,
    ) -> Result<(Options, Effects), FleetError> {
        let (fleet, me) = self.seat_of(actor, fleet_id)?;
        if !me.is_boss() {
            return Err(FleetError::NotCreator);
        }
        if let Some(Some(s)) = auto_join_squad_id
            && fleet.wing_of_squad(s).is_none()
        {
            return Err(FleetError::NoPositionFound);
        }
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return Err(FleetError::NotFound);
        };
        let old = fleet.options;
        if let Some(on) = is_free_move {
            fleet.options.is_free_move = on;
        }
        if let Some(s) = auto_join_squad_id {
            fleet.options.auto_join_squad_id = s;
        }
        let mut fx = Effects::default();
        fleet.tell_options_changed(old, &mut fx);
        Ok((fleet.options, fx))
    }

    /// Mark a fleet as advertised in the fleet finder, or not. Answers
    /// whether the fleet exists.
    pub fn set_registered(&mut self, fleet_id: i64, on: bool) -> bool {
        match self.fleets.get_mut(&fleet_id) {
            Some(f) => {
                f.options.is_registered = on;
                true
            }
            None => false,
        }
    }

    /// `SetFleetMaxSize(size)`: the boss caps the fleet, within the range
    /// the client's own prompt offers, 1 to `MAX_MEMBERS_IN_FLEET`
    /// **[V] fleetSvc.py:2196-2201**. The client's `MIN_MEMBERS_IN_FLEET`
    /// (`const.py:25`) is read by no client code, so it is not the floor.
    pub fn set_max_size(
        &mut self,
        actor: i64,
        fleet_id: i64,
        size: i64,
    ) -> Result<usize, FleetError> {
        let (_, me) = self.seat_of(actor, fleet_id)?;
        if !me.is_boss() {
            return Err(FleetError::NotCreator);
        }
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return Err(FleetError::NotFound);
        };
        fleet.max_size = size.clamp(1, MAX_MEMBERS_IN_FLEET) as usize;
        Ok(fleet.max_size)
    }

    /// One of the three opt-out setters. `which` is the remote method's
    /// name (`SetTakesFleetWarp`, `SetAcceptsRegroupValue`,
    /// `SetAcceptsConduitJumpsValue` **[V] fleetSvc.py:415-422**) or the
    /// attribute it sets.
    pub fn set_opt_out(
        &mut self,
        actor: i64,
        fleet_id: i64,
        which: &str,
        on: bool,
    ) -> Result<Effects, FleetError> {
        self.seat_of(actor, fleet_id)?;
        let flag: fn(&mut MemberOptOuts) -> &mut bool = match which {
            "SetTakesFleetWarp" | "acceptsFleetWarp" => |o| &mut o.accepts_fleet_warp,
            "SetAcceptsRegroupValue" | "acceptsFleetRegroups" => |o| &mut o.accepts_fleet_regroups,
            "SetAcceptsConduitJumpsValue" | "acceptsConduitJumps" => {
                |o| &mut o.accepts_conduit_jumps
            }
            _ => return Err(FleetError::Generic),
        };
        Ok(self.update_member(fleet_id, actor, |m| *flag(&mut m.opt_outs) = on))
    }

    // ------------------------------------------------- dropping and coming back

    /// A pilot's last client went away. His seat is held until
    /// `now + RECONNECT_TIMEOUT` and nobody is told yet; an open invite is
    /// dropped, since nobody is there to answer it.
    pub fn character_disconnected(&mut self, char_id: i64, now: i64) -> Effects {
        self.invites.remove(&char_id);
        if let Some(fleet_id) = self.seats.get(&char_id).copied()
            && let Some(fleet) = self.fleets.get_mut(&fleet_id)
        {
            fleet.gone.insert(char_id, now + RECONNECT_TIMEOUT);
        }
        Effects::default()
    }

    /// Whether any seat is being held for a dropped pilot.
    pub fn has_gone(&self) -> bool {
        self.fleets.values().any(|f| !f.gone.is_empty())
    }

    /// Remove every dropped pilot whose window has closed by `now`, oldest
    /// deadline first, as if each had left -- except that he is not told,
    /// having no client to tell.
    pub fn expire(&mut self, now: i64) -> Effects {
        let mut due: Vec<(i64, i64, i64)> = self
            .fleets
            .values()
            .flat_map(|f| {
                f.gone
                    .iter()
                    .filter(|(_, at)| **at <= now)
                    .map(|(c, at)| (*at, f.fleet_id, *c))
            })
            .collect();
        due.sort_unstable();
        let mut fx = Effects::default();
        for (_, fleet_id, who) in due {
            fx.absorb(self.depart(fleet_id, who, Exit::TimedOut));
        }
        fx
    }

    /// `Reconnect()`: a member is back. His seat is his again, his session
    /// is re-sent, and he is told the fleet's beacons, bridges and tags,
    /// which his client forgot. `None` when he holds no seat in that fleet.
    pub fn reconnect(&mut self, char_id: i64, fleet_id: i64) -> Option<Effects> {
        let fleet = self.fleets.get_mut(&fleet_id)?;
        let member = fleet.members.get(&char_id)?;
        let session = SessionFleetState::seated(fleet_id, member);
        fleet.gone.remove(&char_id);
        let mut fx = Effects::default();
        fx.sessions.push(session);
        fx.absorb(self.replay_to(fleet_id, char_id));
        Some(fx)
    }

    // ------------------------------------------------ beacons and bridges

    pub fn beacon_of(&self, char_id: i64) -> Option<Beacon> {
        self.beacons.get(&char_id).copied()
    }

    pub fn bridge_of(&self, char_id: i64) -> Option<Bridge> {
        self.bridges.get(&char_id).copied()
    }

    /// The beacons `char_id`'s fleet has lit, his own included, by pilot.
    /// Nothing outside a fleet.
    pub fn beacons_visible_to(&self, char_id: i64) -> Vec<(i64, Beacon)> {
        self.fleet_of(char_id).map_or_else(Vec::new, |f| {
            f.members
                .keys()
                .filter_map(|c| self.beacons.get(c).map(|b| (*c, *b)))
                .collect()
        })
    }

    /// The bridges `char_id`'s fleet has open, by pilot, as
    /// [`Self::beacons_visible_to`].
    pub fn bridges_visible_to(&self, char_id: i64) -> Vec<(i64, Bridge)> {
        self.fleet_of(char_id).map_or_else(Vec::new, |f| {
            f.members
                .keys()
                .filter_map(|c| self.bridges.get(c).map(|b| (*c, *b)))
                .collect()
        })
    }

    /// The online members of `char_id`'s fleet, or nobody.
    fn fleetmates(&self, char_id: i64) -> Vec<i64> {
        self.fleet_of(char_id)
            .map_or_else(Vec::new, Fleet::audience)
    }

    /// A pilot lights a beacon. It is remembered even outside a fleet (so
    /// joining one announces it); inside one, the fleet is told.
    pub fn light_beacon(&mut self, char_id: i64, beacon: Beacon) -> Vec<Push> {
        self.beacons.insert(char_id, beacon);
        let mut fx = Effects::default();
        fx.tell(
            self.fleetmates(char_id),
            beacon_event(char_id, beacon, true),
        );
        fx.pushes
    }

    /// A pilot's beacon goes out.
    pub fn douse_beacon(&mut self, char_id: i64) -> Vec<Push> {
        let mut fx = Effects::default();
        if let Some(b) = self.beacons.remove(&char_id) {
            fx.tell(self.fleetmates(char_id), beacon_event(char_id, b, false));
        }
        fx.pushes
    }

    /// A pilot opens a bridge, as [`Self::light_beacon`].
    pub fn open_bridge(&mut self, char_id: i64, bridge: Bridge) -> Vec<Push> {
        self.bridges.insert(char_id, bridge);
        let mut fx = Effects::default();
        fx.tell(self.fleetmates(char_id), bridge_event(bridge, true));
        fx.pushes
    }

    /// A pilot's bridge closes; a second close tells nobody.
    pub fn close_bridge(&mut self, char_id: i64) -> Vec<Push> {
        let mut fx = Effects::default();
        if let Some(b) = self.bridges.remove(&char_id) {
            fx.tell(self.fleetmates(char_id), bridge_event(b, false));
        }
        fx.pushes
    }

    /// Whether `char_id` takes conduit jumps: his opt-out in a fleet, and
    /// yes outside one **[A]** (nothing there to opt out of; falsified by a
    /// conduit that refuses a fleetless passenger).
    pub fn accepts_conduit_jumps(&self, char_id: i64) -> bool {
        self.fleet_of(char_id)
            .and_then(|f| f.members.get(&char_id))
            .is_none_or(|m| m.opt_outs.accepts_conduit_jumps)
    }

    // --------------------------------------------------- tags and loot

    /// `CmdFleetTagTarget(itemID, tag)`: a commander or the boss tags an
    /// item, or with `None` untags it. A tag sits on one item at a time, so
    /// tagging a second item with it moves it. The whole tag map goes to the
    /// fleet, which is what `OnFleetStateChange` replaces
    /// **[V] fleetSvc.py:193-194, targetMgr.py:627-633**.
    pub fn tag(
        &mut self,
        actor: i64,
        item_id: i64,
        tag: Option<&str>,
    ) -> Result<Effects, FleetError> {
        let fleet_id = self
            .seats
            .get(&actor)
            .copied()
            .ok_or(FleetError::NotInFleet)?;
        let (_, me) = self.seat_of(actor, fleet_id)?;
        if !(me.is_boss() || me.is_commander()) {
            return Err(FleetError::NotCommanderOrBoss);
        }
        if tag.is_some_and(|t| !TARGET_TAGS.contains(&t)) {
            return Err(FleetError::Generic);
        }
        let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
            return Err(FleetError::NotFound);
        };
        match tag {
            Some(t) => {
                fleet.target_tags.retain(|_, held| held != t);
                fleet.target_tags.insert(item_id, t.to_owned());
            }
            None => {
                fleet.target_tags.remove(&item_id);
            }
        }
        let mut fx = Effects::default();
        fx.tell(
            fleet.audience(),
            FleetEvent::State {
                tags: fleet.target_tags.clone(),
            },
        );
        Ok(fx)
    }

    /// An item died: its tag leaves every fleet that had it.
    pub fn untag_item(&mut self, item_id: i64) -> Effects {
        let mut fx = Effects::default();
        for fleet in self.fleets.values_mut() {
            if fleet.target_tags.remove(&item_id).is_some() {
                fx.tell(
                    fleet.audience(),
                    FleetEvent::State {
                        tags: fleet.target_tags.clone(),
                    },
                );
            }
        }
        fx
    }

    /// `GetFleetState()`'s answer for `char_id`, `None` outside a fleet.
    pub fn state_of(&self, char_id: i64) -> Option<PyValue> {
        self.fleet_of(char_id).map(Fleet::state_pyvalue)
    }

    /// `OnFleetLootEvent` for `looter` taking `quantity` of `type_id`: to
    /// the looter and the online members for whom `here` holds.
    pub fn loot_event(
        &self,
        looter: i64,
        type_id: i64,
        quantity: i64,
        here: impl Fn(i64) -> bool,
    ) -> Vec<Push> {
        let Some(fleet) = self.fleet_of(looter) else {
            return Vec::new();
        };
        let to: Vec<i64> = fleet
            .audience()
            .into_iter()
            .filter(|c| *c == looter || here(*c))
            .collect();
        let mut fx = Effects::default();
        fx.tell(
            to,
            FleetEvent::Loot {
                char_id: looter,
                type_id,
                quantity,
            },
        );
        fx.pushes
    }
}

/// The clock stage: when a seat is being held, close the windows that ran
/// out by `now`. Pushes leave on this tick; session changes wait on the
/// fleet queue for the next turn.
pub fn tick(reg: &mut Registry, _at: Instant, now: BlueTime) {
    if !reg.fleets().has_gone() {
        return;
    }
    let fx = reg.fleets_mut().expire(now);
    for push in fx.pushes {
        reg.schedule(Duration::ZERO, push);
    }
    reg.fleets_mut().queue_session_changes(fx.sessions);
}
