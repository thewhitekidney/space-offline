//! **Bot fleets: a director forms and flies a fleet the player way**
//! (SOC-C7, R-SOCIETY §6 Stage C row C7, §3.6).
//!
//! Lane note: `docs/lanes/soc-c7.md`. Three layers, each usable alone:
//!
//! 1. **Pure.** [`Command`] is the fleet commander's picture of his fleet
//!    and the API the society mind (C4) and the fighting packet (C5) call:
//!    [`Command::form_fleet`], [`Command::fleet_warp_to`],
//!    [`Command::call_primary`], [`Command::regroup`] and
//!    [`Command::disband`] each answer the [`Order`]s to send, and
//!    [`Command::on_push`] folds the fleet's pushes back in. [`may_invite`]
//!    is the member's "who may invite me" rule; [`engage`] is the member's
//!    turn when a primary is called; [`primary_call`] reads the call.
//! 2. **Verbs.** One [`execute`] per [`Order`], over a [`Conn`], each the
//!    client's own call (below). A driver bot sends them as
//!    [`Act::Fleet`](crate::brain::Act) (coordinator hunk).
//! 3. **[`Director`]**: a connection that is a fleet commander -- the two
//!    above glued to its socket, for a caller that holds a [`Conn`].
//!
//! **The client's calls** (`docs/lanes/fleet.md`, `fleetSvc.pyc`) **[V]**:
//!
//! - form: `fleetObjectHandler.CreateFleet()` (returns the bound fleet),
//!   then `.Init(shipTypeID, setupName)` (`fleetSvc.py:395-397`); the
//!   `fleetid` session change is the evidence;
//! - invite: `fleet.Invite(charID, wingID, squadID, role)`
//!   (`fleetSvc.Invite:428`, through `CSPAChargedAction`);
//! - wings and squads: `GetWings()`, `CreateSquad(wingID)` (`:673`);
//! - fleet warp: `bp.CmdWarpToStuff('item', id, minRange=r, fleet=True)`
//!   (`menusvc.WarpFleet:3250`), and `'char'` for "warp fleet to member";
//! - call a primary: `fleetMgr.BroadcastToBubble('Target', scope, itemID,
//!   typeID)` (`SendBroadcast_Target:1196` -> `SendBubbleBroadcast:1133`),
//!   heard as `OnFleetBroadcast(name, scope, charID, solarSystemID, itemID,
//!   typeID)` (`:1753`);
//! - disband: `fleet.DisbandFleet()` (`:698`).
//!
//! The server's fleet warp and broadcasts are this packet's
//! (`eve-server/src/fleet_warp.rs`, `fleet_broadcast.rs`).
//!
//! **Regroup** is a fleet warp to the commander himself [A]: the server
//! leaves him where he is and warps the fleet to him (`fleet_warp.rs`). The
//! client's `beyonce.CmdFleetRegroup()` is not used.
//!
//! **Members** are ordinary driver bots. They accept an invite by their
//! policy ([`crate::brain::AcceptFleet`]; `Allied` is [`may_invite`] over
//! `charMgr.GetPublicInfo`, [`inviter_is_allied`]), are moved by the
//! server's fleet warp like a player, hear a `Target` call from their boss
//! as the observation's primary ([`primary_call`]) and, while it is on
//! their grid, fight it first ([`engage`]): an armed bot runs the escort's
//! fight shown the primary alone (CP56's pattern), an unarmed one locks it.

use crate::brain::{
    self, Act, ActKind, FleetView, GIVE_UP_AFTER, Hostile, Memory, Observation, Place, RANGE_MARGIN,
};
use crate::{Conn, Error, Event, Result};
use eve_marshal::PyValue;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// `evefleet.BROADCAST_TARGET` **[V]**: the primary call's name.
pub const BROADCAST_TARGET: &str = "Target";
/// `evefleet.BROADCAST_ALL` **[V]**: the scope a director broadcasts with,
/// the whole fleet **[A]** (the client's own scope is a per-pilot setting;
/// falsified by a fleet that wants calls kept to a squad).
pub const BROADCAST_ALL: i64 = 3;
/// The push a broadcast arrives as **[V]**.
pub const BROADCAST_EVENT: &str = "OnFleetBroadcast";
/// Pilots a director seats per squad before opening the next **[A]**: a
/// squad of ten, the old fleet-boost size (falsified by owners preferring
/// one big squad). The client's own ceiling is 256.
pub const SQUAD_SIZE: usize = 10;
/// The client's own squad limit per wing (`evefleet.MAX_SQUADS_IN_WING`)
/// **[V]**: a director never asks for more.
pub const MAX_SQUADS: usize = 25;
/// How far from the target a fleet warp lands, metres **[A]**: "warp to
/// zero" leaves the standoff to the server (falsified by fleets landing
/// inside an object).
pub const FLEET_WARP_RANGE: f64 = 0.0;
/// How long a [`Director`] waits for its invitees to join **[A]**.
pub const JOIN_WAIT: Duration = Duration::from_secs(30);
/// How long a verb waits for its evidence (the session change, the warp).
const EVIDENCE: Duration = Duration::from_millis(2_500);
/// The name a called primary carries in the escort's chat lines.
pub const PRIMARY_NAME: &str = "the primary";

// ---------------------------------------------------------------------------
// Fleet hours (R-REL-2, F5)
// ---------------------------------------------------------------------------

/// **Shared fleet time** (pure): the game-hours each mate has flown with the
/// bot, from the second it joined (`joined_at`, game seconds) to `now`, for a
/// game-day of `day` seconds. The relations model weighs them
/// ([`crate::relations::FactKind::Fleet`], `K_FLEET` per hour, capped); the
/// caller owns the clock and the hall, and reports a mate when it leaves or
/// the fleet disbands. **[A]**: a game-day is 24 game-hours.
pub fn shared_hours(
    joined_at: &std::collections::BTreeMap<i64, u64>,
    now: u64,
    day: u64,
) -> Vec<(i64, f64)> {
    let hour = (day.max(1) as f64 / 24.0).max(f64::MIN_POSITIVE);
    joined_at
        .iter()
        .map(|(&mate, &at)| (mate, now.saturating_sub(at) as f64 / hour))
        .collect()
}

// ---------------------------------------------------------------------------
// Who may invite me
// ---------------------------------------------------------------------------

/// A pilot's corporation and alliance: the two things the invite rule
/// reads. Alliances arrive with SOC-C1; until then `alliance` is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Affiliation {
    pub corp: i64,
    pub alliance: Option<i64>,
}

/// **Who may invite me** (R-SOCIETY §3.6) **[A]**: my own corporation, or
/// my alliance when I am in one. A pilot in no alliance is never allied
/// with another pilot in none. Falsified by a society that wants mixed
/// fleets of friendly corps (standings-based, C2's relation).
pub fn may_invite(me: Affiliation, inviter: Affiliation) -> bool {
    me.corp == inviter.corp || (me.alliance.is_some() && me.alliance == inviter.alliance)
}

// ---------------------------------------------------------------------------
// The commander's picture: pure
// ---------------------------------------------------------------------------

/// Where a fleet warp goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarpTarget {
    /// A ball: a belt, a site, a gate (`'item'`).
    Item(i64),
    /// A fleet member, by **character** id (`'char'`).
    Member(i64),
}

/// One step a fleet commander takes over the wire: one [`execute`].
#[derive(Debug, Clone, PartialEq)]
pub enum Order {
    /// `CreateFleet` and `Init(shipTypeID)`.
    Create { ship_type: i64 },
    /// `Invite(charID, wingID, squadID, None)`, into the first wing's
    /// `squad`-th squad (by id), opening squads as needed.
    Invite { char_id: i64, squad: usize },
    /// `CmdWarpToStuff(..., fleet=True)`.
    Warp { target: WarpTarget, range: f64 },
    /// `Target`: `fleetMgr.BroadcastToBubble(name, BROADCAST_ALL, itemID,
    /// typeID)`; any other name: the bound `fleet.SendBroadcast` with the
    /// same four (BOT-FLEET-ORDERS).
    Broadcast {
        name: &'static str,
        item: i64,
        type_id: Option<i64>,
    },
    /// `DisbandFleet()` on `fleet`.
    Disband { fleet: i64 },
}

impl Order {
    /// The act log's line and ids (`driver::describe`).
    pub fn describe(&self) -> (String, Vec<i64>) {
        match self {
            Order::Create { ship_type } => (format!("CreateFleet, Init({ship_type})"), vec![]),
            Order::Invite { char_id, squad } => (
                format!("fleet Invite {char_id} squad #{squad}"),
                vec![*char_id],
            ),
            Order::Warp { target, range } => {
                let (subject, id) = match target {
                    WarpTarget::Item(id) => ("item", id),
                    WarpTarget::Member(id) => ("char", id),
                };
                (
                    format!("CmdWarpToStuff {subject} {id} minRange={range:.0} fleet=True"),
                    vec![*id],
                )
            }
            Order::Broadcast { name, item, .. } => {
                let how = if *name == BROADCAST_TARGET {
                    "BroadcastToBubble"
                } else {
                    "SendBroadcast"
                };
                (format!("{how} {name} {item}"), vec![*item])
            }
            Order::Disband { fleet } => (format!("DisbandFleet {fleet}"), vec![*fleet]),
        }
    }
}

/// **The fleet commander's picture of his fleet.** Pure: every method
/// answers what to send and changes only this value. Kept in a bot's
/// [`Memory`] (coordinator hunk) or a [`Director`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Command {
    /// The fleet, once formed (the `fleetid` session change).
    pub fleet_id: Option<i64>,
    /// A `Create` is out and has not come back.
    pub creating: bool,
    /// The hull `Init` reports; `0` reads as the capsule.
    pub ship_type: i64,
    /// Who the commander means to have, in seat order.
    pub wanted: Vec<i64>,
    /// Invited, not (yet) heard to join.
    pub invited: BTreeSet<i64>,
    /// Heard to join (`OnFleetJoin`).
    pub joined: BTreeSet<i64>,
    /// The primary last called.
    pub primary: Option<i64>,
    /// The last fleet warp ordered.
    pub warp: Option<WarpTarget>,
}

impl Command {
    /// A commander flying `ship_type`.
    pub fn new(ship_type: i64) -> Command {
        Command {
            ship_type,
            ..Command::default()
        }
    }

    /// Is there a fleet, or one on its way?
    pub fn has_fleet(&self) -> bool {
        self.fleet_id.is_some() || self.creating
    }

    /// Every wanted member has joined.
    pub fn is_formed(&self) -> bool {
        self.fleet_id.is_some() && self.wanted.iter().all(|c| self.joined.contains(c))
    }

    /// **Form the fleet** with `members` (character ids): create it if there
    /// is none, then invite whoever is neither invited nor in, seated
    /// [`SQUAD_SIZE`] to a squad in the order given. Called again it invites
    /// only the newcomers, so it is also "add these".
    pub fn form_fleet(&mut self, members: &[i64]) -> Vec<Order> {
        let mut orders = Vec::new();
        if !self.has_fleet() {
            self.creating = true;
            orders.push(Order::Create {
                ship_type: if self.ship_type == 0 {
                    brain::CAPSULE
                } else {
                    self.ship_type
                },
            });
        }
        for &c in members {
            if !self.wanted.contains(&c) {
                self.wanted.push(c);
            }
        }
        for (seat, &c) in self.wanted.iter().enumerate() {
            if self.invited.contains(&c) || self.joined.contains(&c) {
                continue;
            }
            self.invited.insert(c);
            orders.push(Order::Invite {
                char_id: c,
                squad: squad_of(seat),
            });
        }
        orders
    }

    /// **Warp the fleet** to `target`. Nothing without a fleet.
    pub fn fleet_warp_to(&mut self, target: WarpTarget) -> Vec<Order> {
        if !self.has_fleet() {
            return Vec::new();
        }
        self.warp = Some(target);
        vec![Order::Warp {
            target,
            range: FLEET_WARP_RANGE,
        }]
    }

    /// **Call a primary**: one `Target` broadcast. The same primary called
    /// again is not re-broadcast. Nothing without a fleet.
    pub fn call_primary(&mut self, id: i64, type_id: Option<i64>) -> Vec<Order> {
        if !self.has_fleet() || self.primary == Some(id) {
            return Vec::new();
        }
        self.primary = Some(id);
        vec![Order::Broadcast {
            name: BROADCAST_TARGET,
            item: id,
            type_id,
        }]
    }

    /// **Send the fleet to a system**: one `TravelTo` broadcast (the item
    /// is the solar system id). Nothing without a fleet.
    pub fn travel_to(&mut self, system: i64) -> Vec<Order> {
        if !self.has_fleet() {
            return Vec::new();
        }
        vec![Order::Broadcast {
            name: BROADCAST_TRAVEL_TO,
            item: system,
            type_id: None,
        }]
    }

    /// **Mark an enemy**: one `EnemySpotted` broadcast naming the ball.
    pub fn enemy_spotted(&mut self, ball: i64, type_id: Option<i64>) -> Vec<Order> {
        if !self.has_fleet() {
            return Vec::new();
        }
        vec![Order::Broadcast {
            name: BROADCAST_ENEMY_SPOTTED,
            item: ball,
            type_id,
        }]
    }

    /// **Regroup on the commander** (`me`, his character id): a fleet warp
    /// to himself, which the server reads as "the fleet comes to me" [A].
    pub fn regroup(&mut self, me: i64) -> Vec<Order> {
        self.fleet_warp_to(WarpTarget::Member(me))
    }

    /// **Disband**: nothing without a fleet; forgets the fleet at once (a
    /// refused disband leaves a fleet nobody commands, which the next
    /// `form_fleet` replaces).
    pub fn disband(&mut self) -> Vec<Order> {
        if !self.has_fleet() {
            return Vec::new();
        }
        let orders = self
            .fleet_id
            .map(|fleet| vec![Order::Disband { fleet }])
            .unwrap_or_default();
        *self = Command::new(self.ship_type);
        orders
    }

    /// **A fleet push, folded in**: `OnFleetJoin(member)` (its `charID`),
    /// `OnFleetLeave(charID)`, `OnFleetDisbanded` **[V]** (`fleet.rs`'s
    /// `FleetEvent::to_push`). Anything else is ignored.
    pub fn on_push(&mut self, event: &str, args: &[PyValue]) {
        match event {
            "OnFleetJoin" => {
                if let Some(c) = args.first().and_then(|m| attr_int(m, "charID")) {
                    self.invited.remove(&c);
                    self.joined.insert(c);
                }
            }
            "OnFleetLeave" => {
                if let Some(PyValue::Int(c)) = args.first() {
                    self.invited.remove(c);
                    self.joined.remove(c);
                }
            }
            "OnFleetDisbanded" => *self = Command::new(self.ship_type),
            _ => {}
        }
    }

    /// What an [`execute`] of `order` did: the fleet formed, or an invite
    /// that went nowhere (so the next `form_fleet` asks again).
    pub fn on_done(&mut self, order: &Order, result: &Result<Option<i64>>) {
        match (order, result) {
            (Order::Create { .. }, Ok(Some(id))) => {
                self.creating = false;
                self.fleet_id = Some(*id);
            }
            (Order::Create { .. }, _) => self.creating = false,
            (Order::Invite { char_id, .. }, Err(_)) => {
                self.invited.remove(char_id);
            }
            _ => {}
        }
    }
}

/// The squad a seat goes in: [`SQUAD_SIZE`] to a squad, at most
/// [`MAX_SQUADS`].
pub fn squad_of(seat: usize) -> usize {
    (seat / SQUAD_SIZE).min(MAX_SQUADS - 1)
}

/// The orders as the driver's acts.
pub fn acts(orders: Vec<Order>) -> Vec<Act> {
    orders.into_iter().map(Act::Fleet).collect()
}

// ---------------------------------------------------------------------------
// The member's side: pure
// ---------------------------------------------------------------------------

/// The broadcast a pilot in trouble sends **[V]** (`evefleet`'s broadcast
/// list; the server relays it, `fleet_broadcast.rs`).
pub const BROADCAST_NEED_BACKUP: &str = "NeedBackup";

/// `evefleet.BROADCAST_ENEMY_SPOTTED` **[V]**.
pub const BROADCAST_ENEMY_SPOTTED: &str = "EnemySpotted";
/// `evefleet.BROADCAST_TRAVEL_TO` **[V]**: the item is a solar system id.
pub const BROADCAST_TRAVEL_TO: &str = "TravelTo";

/// A `NeedBackup` or `EnemySpotted` heard (R-REL-3): who called, from which system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupCall {
    pub caller: i64,
    pub system: i64,
}

/// **A call for help**, from `OnFleetBroadcast(name, scope, charID,
/// solarSystemID, itemID, typeID)` **[V]**: the name must be
/// [`BROADCAST_NEED_BACKUP`]. `None` for any other broadcast.
pub fn backup_call(event: &str, args: &[PyValue]) -> Option<BackupCall> {
    if event != BROADCAST_EVENT {
        return None;
    }
    // `EnemySpotted` is a call for help too (BOT-FLEET-ORDERS): the same
    // relation-weighted answer.
    let name = args.first().and_then(crate::text)?;
    if name != BROADCAST_NEED_BACKUP && name != BROADCAST_ENEMY_SPOTTED {
        return None;
    }
    let int = |i: usize| match args.get(i) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    };
    Some(BackupCall {
        caller: int(2)?,
        system: int(3)?,
    })
}

/// **A primary call**, from `OnFleetBroadcast(name, scope, charID,
/// solarSystemID, itemID, typeID)` **[V]**: the `itemID` of a `Target`
/// broadcast by the fleet's boss -- or by anyone in it when the bot does
/// not know its boss (a commander hears his own call) **[A]**. Falsified by
/// a fleet whose wing commanders call their own primaries.
pub fn primary_call(event: &str, args: &[PyValue], fleet: Option<FleetView>) -> Option<i64> {
    if event != BROADCAST_EVENT {
        return None;
    }
    let fleet = fleet?;
    let name = args.first().and_then(crate::text)?;
    if name != BROADCAST_TARGET {
        return None;
    }
    let caller = match args.get(2) {
        Some(PyValue::Int(c)) => *c,
        _ => return None,
    };
    if fleet.boss.is_some_and(|b| b != caller) {
        return None;
    }
    match args.get(4) {
        Some(PyValue::Int(id)) => Some(*id),
        _ => None,
    }
}

/// **A fleet member's turn while a primary is called** (rule 2c, before
/// the role): `None` when there is nothing to do about it -- no fleet, no
/// primary, going home, or the primary not on this grid (an NPC or a human
/// the park shows; another bot's ship is C5's) -- and the role decides.
///
/// Armed, it is the escort's fight shown the primary as the only hostile:
/// orbit, lock, fire, reload, retreat. Unarmed, it locks the primary and
/// leaves the rest to the role.
pub fn engage(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    let primary = o.primary?;
    o.fleet?;
    if m.homing.is_some() || !matches!(o.place, Place::InSpace { bound: true, .. }) {
        return None;
    }
    let park = o.park.as_ref()?;
    let target = park
        .hostiles
        .iter()
        .find(|h| h.ball.id == primary)
        .cloned()
        .or_else(|| {
            o.humans.iter().find(|b| b.id == primary).map(|b| Hostile {
                ball: *b,
                type_id: o.human_types.get(&b.id).copied().unwrap_or(0),
                name: PRIMARY_NAME.to_owned(),
                shooting: None,
            })
        })
        // Another bot's ship, or any pilot's (SOC-C5): never an ally's.
        .or_else(|| o.hunt.primary_target(primary))?;
    if !o.guns.is_empty() {
        let mut seen = o.clone();
        if let Some(p) = &mut seen.park {
            p.hostiles = vec![target];
        }
        return Some(crate::escort::in_space(m, &seen));
    }
    if o.locked.contains(&primary)
        || o.pending.contains(&primary)
        || o.failed(ActKind::Lock) >= GIVE_UP_AFTER
        || park.ego.surface_to(&target.ball) > o.lock_range * RANGE_MARGIN
    {
        return None;
    }
    // REACT-JITTER: the boss's call is heard before the click.
    if let Some(left) = m.thinking(crate::react::Class::Target, primary, o) {
        return Some(vec![Act::Wait(left.min(crate::brain::SHORT_WAIT))]);
    }
    Some(vec![Act::Lock(primary)])
}

// ---------------------------------------------------------------------------
// The verbs
// ---------------------------------------------------------------------------

/// An attribute of a `KeyVal` (or a dict) as an integer.
fn attr_int(v: &PyValue, key: &str) -> Option<i64> {
    match attr(v, key)? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

fn attr<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    match crate::attributes(v) {
        PyValue::Dict(entries) => entries
            .iter()
            .find(|(_, k)| crate::text(k).as_deref() == Some(key))
            .map(|(value, _)| value),
        _ => None,
    }
}

/// `{id: KeyVal}` (wire order `(value, key)`) as `(id, value)`, by id.
fn by_id(v: &PyValue) -> Vec<(i64, &PyValue)> {
    let mut out: Vec<(i64, &PyValue)> = match crate::attributes(v) {
        PyValue::Dict(entries) => entries
            .iter()
            .filter_map(|(value, key)| match key {
                PyValue::Int(id) => Some((*id, value)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    out.sort_by_key(|(id, _)| *id);
    out
}

fn bstr(s: &str) -> PyValue {
    PyValue::Bytes(s.as_bytes().to_vec())
}

fn refused(what: &str, outcome: &crate::CallOutcome) -> Error {
    Error::Refused(format!(
        "{what}: {}",
        outcome
            .refusal()
            .unwrap_or_else(|| "no UserError key".to_owned())
    ))
}

impl Conn {
    /// **Form a fleet**: `CreateFleet()`, then `Init(shipTypeID, None)` on
    /// the object it returned **[V]**. Evidence: the `fleetid` session
    /// change. Returns the fleet id.
    pub fn create_fleet(&mut self, ship_type: i64) -> Result<i64> {
        let created = self.call("fleetObjectHandler", "CreateFleet", vec![])?;
        let Some(fleet) = created
            .value()
            .and_then(|v| crate::object_refs(&v).into_iter().next())
        else {
            return Err(refused("CreateFleet", &created));
        };
        let mut init = self.call_on(
            &fleet,
            "Init",
            vec![PyValue::Int(ship_type), PyValue::None],
            vec![],
        )?;
        if init.refusal().is_some() {
            return Err(refused("Init", &init));
        }
        self.await_session_change(&mut init, EVIDENCE)?;
        self.session()
            .fleet_id()
            .ok_or_else(|| Error::Refused("Init: no fleetid session change".to_owned()))
    }

    /// `(wingID, [squadID, ...])` of the fleet's first wing, by id
    /// (`GetWings()` **[V]**).
    fn first_wing(&mut self, fleet_id: i64) -> Result<(i64, Vec<i64>)> {
        let answer = self.moniker_call(
            "fleetObjectHandler",
            PyValue::Int(fleet_id),
            "GetWings",
            vec![],
            vec![],
        )?;
        let wings = answer.value.clone().unwrap_or(PyValue::None);
        let Some((wing, w)) = by_id(&wings).into_iter().next() else {
            return Err(refused("GetWings: no wing", &answer.outcome));
        };
        let squads = attr(w, "squads")
            .map(|s| by_id(s).into_iter().map(|(id, _)| id).collect())
            .unwrap_or_default();
        Ok((wing, squads))
    }

    /// **The seat for the `squad`-th squad** of the first wing, opening
    /// squads with `CreateSquad(wingID)` **[V]** until it exists.
    pub fn fleet_seat(&mut self, fleet_id: i64, squad: usize) -> Result<(i64, i64)> {
        let (wing, mut squads) = self.first_wing(fleet_id)?;
        let squad = squad.min(MAX_SQUADS - 1);
        while squads.len() <= squad {
            let answer = self.moniker_call(
                "fleetObjectHandler",
                PyValue::Int(fleet_id),
                "CreateSquad",
                vec![PyValue::Int(wing)],
                vec![],
            )?;
            match answer.value {
                Some(PyValue::Int(id)) => squads.push(id),
                _ => return Err(refused("CreateSquad", &answer.outcome)),
            }
        }
        Ok((wing, squads[squad]))
    }

    /// **Invite** `char_id` into a seat: `Invite(charID, wingID, squadID,
    /// None)` on the fleet's moniker **[V]**. The server answers `True`; the
    /// join is heard later as `OnFleetJoin`.
    pub fn fleet_invite(&mut self, fleet_id: i64, char_id: i64, seat: (i64, i64)) -> Result<()> {
        let answer = self.moniker_call(
            "fleetObjectHandler",
            PyValue::Int(fleet_id),
            "Invite",
            vec![
                PyValue::Int(char_id),
                PyValue::Int(seat.0),
                PyValue::Int(seat.1),
                PyValue::None,
            ],
            vec![],
        )?;
        match answer.value {
            Some(PyValue::Bool(true)) => Ok(()),
            _ => Err(refused(&format!("Invite({char_id})"), &answer.outcome)),
        }
    }

    /// **Fleet warp**: `CmdWarpToStuff(subject, id, minRange=range,
    /// fleet=True)` **[V]**. Evidence, for an item: this ship's `WarpTo`; a
    /// warp to oneself (the regroup) moves only the others, so a raise is
    /// the only refusal there.
    pub fn fleet_warp(&mut self, target: WarpTarget, range: f64) -> Result<()> {
        let (subject, id) = match target {
            WarpTarget::Item(id) => ("item", id),
            WarpTarget::Member(id) => ("char", id),
        };
        let mut outcome = self.bound_call_kw(
            "CmdWarpToStuff",
            vec![bstr(subject), PyValue::Int(id)],
            vec![
                (PyValue::Real(range), bstr("minRange")),
                (PyValue::Bool(true), bstr("fleet")),
            ],
        )?;
        if outcome.refusal().is_some() {
            return Err(refused("CmdWarpToStuff(fleet)", &outcome));
        }
        if target == WarpTarget::Member(self.session().char_id().unwrap_or(0)) {
            return Ok(());
        }
        let Some(ship) = self.session().ship_id() else {
            return Ok(());
        };
        let warps = |name: &str, args: &[PyValue]| {
            name == "DoDestinyUpdate"
                && crate::read_action_list(args)
                    .iter()
                    .any(|(_, f, ops)| f == "WarpTo" && ops.first() == Some(&PyValue::Int(ship)))
        };
        let already = outcome
            .notifications
            .iter()
            .filter_map(crate::notification)
            .any(|(n, a)| warps(&n, &a));
        if already
            || self
                .await_push(EVIDENCE, &mut outcome.notifications, warps)?
                .is_some()
        {
            return Ok(());
        }
        Err(Error::Refused(format!(
            "CmdWarpToStuff('{subject}', {id}, fleet=True): no WarpTo for this ship"
        )))
    }

    /// **A fleet broadcast**, the client's way. `Target` is a bubble
    /// broadcast, `fleetMgr.BroadcastToBubble(name, BROADCAST_ALL, itemID,
    /// typeID)`; every other name is global, the bound
    /// `fleet.SendBroadcast(name, scope, itemID, typeID)` on the fleet the
    /// session names (`fleetSvc.py:941`) **[V]**. A bot with no fleet in its
    /// session falls back to the bubble call. Answered `None`.
    pub fn fleet_broadcast(&mut self, name: &str, item: i64, type_id: Option<i64>) -> Result<()> {
        if name != BROADCAST_TARGET
            && let Some(fleet) = self.session().fleet_id()
        {
            let answer = self.moniker_call(
                "fleetObjectHandler",
                PyValue::Int(fleet),
                "SendBroadcast",
                vec![
                    bstr(name),
                    PyValue::Int(BROADCAST_ALL),
                    PyValue::Int(item),
                    type_id.map_or(PyValue::None, PyValue::Int),
                ],
                vec![],
            )?;
            return match answer.refusal() {
                None => Ok(()),
                Some(_) => Err(refused(&format!("SendBroadcast({name})"), &answer.outcome)),
            };
        }
        let outcome = self.call(
            "fleetMgr",
            "BroadcastToBubble",
            vec![
                bstr(name),
                PyValue::Int(BROADCAST_ALL),
                PyValue::Int(item),
                type_id.map_or(PyValue::None, PyValue::Int),
            ],
        )?;
        match outcome.refusal() {
            None => Ok(()),
            Some(_) => Err(refused(&format!("BroadcastToBubble({name})"), &outcome)),
        }
    }

    /// **Disband**: `DisbandFleet()` on the fleet's moniker **[V]**.
    /// Evidence: the `fleetid` session change clearing it.
    pub fn disband_fleet(&mut self, fleet_id: i64) -> Result<()> {
        let mut answer = self.moniker_call(
            "fleetObjectHandler",
            PyValue::Int(fleet_id),
            "DisbandFleet",
            vec![],
            vec![],
        )?;
        if answer.refusal().is_some() {
            return Err(refused("DisbandFleet", &answer.outcome));
        }
        self.await_session_change(&mut answer.outcome, EVIDENCE)?;
        match self.session().fleet_id() {
            None => Ok(()),
            Some(_) => Err(Error::Refused(
                "DisbandFleet: the session still names the fleet".to_owned(),
            )),
        }
    }

    /// **A pilot's corporation and alliance**: `charMgr.GetPublicInfo(charID)`,
    /// a `KeyVal` with `corporationID` and `allianceID` **[V]**
    /// (`crimewatch` reads both unguarded).
    pub fn affiliation_of(&mut self, char_id: i64) -> Result<Affiliation> {
        let outcome = self.call("charMgr", "GetPublicInfo", vec![PyValue::Int(char_id)])?;
        let info = outcome.value().unwrap_or(PyValue::None);
        let Some(corp) = attr_int(&info, "corporationID") else {
            return Err(refused(&format!("GetPublicInfo({char_id})"), &outcome));
        };
        Ok(Affiliation {
            corp,
            alliance: attr_int(&info, "allianceID"),
        })
    }

    /// This pilot's own, from the session's `corpid` and `allianceid`.
    pub fn my_affiliation(&self) -> Option<Affiliation> {
        Some(Affiliation {
            corp: self.session().get_i64("corpid")?,
            alliance: self.session().get_i64("allianceid"),
        })
    }
}

impl Conn {
    /// **The fleet composition** (FC-1): `GetFleetComposition()` on the
    /// fleet the session names (`fleetSvc.py:1024`), as rows. The server
    /// answers every member's *live* system and station (FC-0).
    pub fn fleet_composition(&mut self) -> Result<Vec<crate::wing_hall::Row>> {
        let Some(fleet) = self.session().fleet_id() else {
            return Err(Error::NotSent(
                "GetFleetComposition: not in a fleet".to_owned(),
            ));
        };
        let answer = self.moniker_call(
            "fleetObjectHandler",
            PyValue::Int(fleet),
            "GetFleetComposition",
            vec![],
            vec![],
        )?;
        match &answer.value {
            Some(v @ PyValue::List(_)) => Ok(crate::wing_hall::parse_composition(v)),
            _ => Err(refused("GetFleetComposition", &answer.outcome)),
        }
    }

    /// **The fleets on offer** (FC-1): `fleetProxy.GetAvailableFleetAds()`
    /// is `{fleetID: advert}` (`fleetSvc.py:2131`); the rows are
    /// `(fleetID, the advert leader's charID)`.
    pub fn available_fleet_ads(&mut self) -> Result<Vec<(i64, i64)>> {
        let outcome = self.call("fleetProxy", "GetAvailableFleetAds", vec![])?;
        if outcome.refusal().is_some() {
            return Err(refused("GetAvailableFleetAds", &outcome));
        }
        let ads = outcome.value().unwrap_or(PyValue::None);
        Ok(by_id(&ads)
            .into_iter()
            .filter_map(|(fleet, ad)| {
                let leader = attr(ad, "leader")?;
                Some((fleet, attr_int(leader, "charID")?))
            })
            .collect())
    }

    /// **Apply to join** a listed fleet: `fleetProxy.ApplyToJoinFleet(fleetID,
    /// True)` (`fleetSvc.py:2149`). An advert that needs no approval is
    /// answered with an invite from its boss, heard as the usual push.
    pub fn apply_to_join_fleet(&mut self, fleet_id: i64) -> Result<()> {
        let outcome = self.call(
            "fleetProxy",
            "ApplyToJoinFleet",
            vec![PyValue::Int(fleet_id), PyValue::Bool(true)],
        )?;
        match outcome.refusal() {
            None => Ok(()),
            Some(_) => Err(refused("ApplyToJoinFleet", &outcome)),
        }
    }

    /// **The fleet's target tags** (FC-2): the bound `beyonce.GetFleetState()`
    /// (`michelle.py:283-291`), as `item -> label`. A bot learns later
    /// changes from `OnFleetStateChange`; this reads the ones set before it
    /// came. Empty without a fleet.
    pub fn fleet_target_tags(&mut self) -> Result<std::collections::BTreeMap<i64, String>> {
        let out = self.bound_call("GetFleetState", vec![])?;
        if out.refusal().is_some() {
            return Err(refused("GetFleetState", &out));
        }
        Ok(out
            .value()
            .as_ref()
            .and_then(crate::wing_fight::parse_tags)
            .unwrap_or_default())
    }

    /// **Tag a ship** (a commander's `beyonce.CmdFleetTagTarget(itemID,
    /// tag)`, `None` to untag); the server then pushes the whole map to the
    /// fleet. The answer is unused by the client, so there is no refusal.
    pub fn tag_target(&mut self, item: i64, tag: Option<&str>) -> Result<()> {
        let tag = tag.map_or(PyValue::None, |t| PyValue::Unicode(t.to_owned()));
        self.bound_call("CmdFleetTagTarget", vec![PyValue::Int(item), tag])
            .map(|_| ())
    }
}

/// **May this inviter's invite be accepted** under `AcceptFleet::Allied`:
/// [`may_invite`] over the session and `GetPublicInfo`. A bot that cannot
/// tell its own corporation says no.
pub fn inviter_is_allied(conn: &mut Conn, inviter: i64) -> Result<bool> {
    let Some(me) = conn.my_affiliation() else {
        return Ok(false);
    };
    Ok(may_invite(me, conn.affiliation_of(inviter)?))
}

/// **Send one order.** `Ok(Some(fleet))` for a `Create`, else `Ok(None)`.
/// Fold the answer into the [`Command`] with [`Command::on_done`].
pub fn execute(conn: &mut Conn, cmd: &Command, order: &Order) -> Result<Option<i64>> {
    let fleet = || {
        cmd.fleet_id
            .ok_or_else(|| Error::NotSent("no fleet yet".to_owned()))
    };
    match order {
        Order::Create { ship_type } => conn.create_fleet(*ship_type).map(Some),
        Order::Invite { char_id, squad } => {
            let fleet = fleet()?;
            let seat = conn.fleet_seat(fleet, *squad)?;
            conn.fleet_invite(fleet, *char_id, seat).map(|()| None)
        }
        Order::Warp { target, range } => conn.fleet_warp(*target, *range).map(|()| None),
        Order::Broadcast {
            name,
            item,
            type_id,
        } => conn.fleet_broadcast(name, *item, *type_id).map(|()| None),
        Order::Disband { fleet } => conn.disband_fleet(*fleet).map(|()| None),
    }
}

// ---------------------------------------------------------------------------
// The director: a connection that commands a fleet
// ---------------------------------------------------------------------------

/// **A fleet commander on its own connection.** Each method plans with the
/// [`Command`] and sends what it planned; pushes read on the way are folded
/// in. For a caller that holds a [`Conn`] (a test, a society thread); a
/// driver bot sends the same orders as [`Act::Fleet`].
pub struct Director {
    pub conn: Conn,
    pub command: Command,
}

impl Director {
    /// `conn` is logged in and selected; its pushes are kept from now on.
    pub fn new(mut conn: Conn, ship_type: i64) -> Director {
        conn.keep_pushes();
        Director {
            conn,
            command: Command::new(ship_type),
        }
    }

    /// Read the backlog of pushes (a fight's damage, a fleet's moves) now
    /// rather than inside the next call, whose reply read is bounded.
    fn drain(&mut self) -> Result<()> {
        while !self.conn.pump(Duration::from_millis(1))?.is_empty() {}
        self.absorb();
        Ok(())
    }

    fn send(&mut self, orders: Vec<Order>) -> Result<()> {
        self.drain()?;
        let mut first_error = None;
        for order in orders {
            let result = execute(&mut self.conn, &self.command, &order);
            self.command.on_done(&order, &result);
            self.absorb();
            if let Err(e) = result {
                // The transport gone is the end; a refusal is one order.
                if matches!(e, Error::Io(_) | Error::Protocol(_)) {
                    return Err(e);
                }
                first_error.get_or_insert(e);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn absorb(&mut self) {
        for (event, args) in self.conn.take_pushes() {
            self.command.on_push(&event, &args);
        }
    }

    /// Read the socket for `within`, folding what arrives.
    pub fn pump(&mut self, within: Duration) -> Result<Vec<Event>> {
        let events = self.conn.pump(within)?;
        self.absorb();
        Ok(events)
    }

    /// **Form the fleet** and wait up to `within` for every member to join.
    /// `Ok(true)` once they have.
    pub fn form_fleet(&mut self, members: &[i64], within: Duration) -> Result<bool> {
        let orders = self.command.form_fleet(members);
        self.send(orders)?;
        let deadline = Instant::now() + within;
        while !self.command.is_formed() && Instant::now() < deadline {
            self.pump(Duration::from_millis(250))?;
        }
        Ok(self.command.is_formed())
    }

    /// **Warp the fleet** to `target`.
    pub fn fleet_warp_to(&mut self, target: WarpTarget) -> Result<()> {
        let orders = self.command.fleet_warp_to(target);
        self.send(orders)
    }

    /// **Call a primary**: lock it (`AddTarget`), then broadcast it.
    pub fn call_primary(&mut self, id: i64, type_id: Option<i64>) -> Result<()> {
        let orders = self.command.call_primary(id, type_id);
        if orders.is_empty() {
            return Ok(());
        }
        self.drain()?;
        let locked = self.conn.lock_target(id).map(|_| ());
        self.absorb();
        self.send(orders)?;
        locked
    }

    /// **Regroup** the fleet on the director.
    pub fn regroup(&mut self) -> Result<()> {
        let me = self
            .conn
            .session()
            .char_id()
            .ok_or_else(|| Error::NotSent("regroup: no character".to_owned()))?;
        let orders = self.command.regroup(me);
        self.send(orders)
    }

    /// **Disband** the fleet.
    pub fn disband(&mut self) -> Result<()> {
        let orders = self.command.disband();
        self.send(orders)
    }
}

// ---------------------------------------------------------------------------
// The theatre's orders (DOC-4)
// ---------------------------------------------------------------------------

/// `evefleet.BROADCAST_HOLD_POSITION` **[V]**: "hold position".
pub const BROADCAST_HOLD: &str = "HoldPosition";

/// **A theatre action as fleet orders** (R-DOCTRINE 7.3): the commander
/// invites whoever the machine commits (`Commit`, `Surge`), asks the bubble
/// for backup, holds, or disbands on a disengage. `ship` is the
/// commander's own ship ball (the item a broadcast names); `me` its
/// character. Pure: the caller sends the orders ([`execute`], or
/// [`Director::run`]).
///
/// Once every invitee has joined, the next call also warps the fleet onto
/// the commander (`regroup`), which is how the committed fleet arrives; a
/// fleet that is still forming warps nothing.
pub fn orders_for(
    cmd: &mut Command,
    action: &crate::doctrine_theatre::Action,
    ship: i64,
    me: i64,
) -> Vec<Order> {
    use crate::doctrine_theatre::Action;
    match action {
        Action::Commit { who } | Action::Surge { who } => {
            let mut v = cmd.form_fleet(who);
            if cmd.is_formed() && cmd.warp != Some(WarpTarget::Member(me)) {
                v.extend(cmd.regroup(me));
            }
            v
        }
        Action::NeedBackup { .. } => vec![Order::Broadcast {
            name: BROADCAST_NEED_BACKUP,
            item: ship,
            type_id: None,
        }],
        Action::HoldPosition => vec![Order::Broadcast {
            name: BROADCAST_HOLD,
            item: ship,
            type_id: None,
        }],
        Action::Disengage { .. } => cmd.disband(),
        Action::Scout { .. } | Action::Escalate | Action::Rebuild { .. } => Vec::new(),
    }
}

impl Director {
    /// **Run the theatre's actions** on this commander's connection: each
    /// through [`orders_for`], then sent. The commander's own ship must be
    /// in space (a broadcast names it).
    pub fn run(&mut self, actions: &[crate::doctrine_theatre::Action]) -> Result<()> {
        let me = self
            .conn
            .session()
            .char_id()
            .ok_or_else(|| Error::NotSent("run: no character".to_owned()))?;
        let ship = self
            .conn
            .session()
            .ship_id()
            .ok_or_else(|| Error::NotSent("run: no ship".to_owned()))?;
        for a in actions {
            let orders = orders_for(&mut self.command, a, ship, me);
            self.send(orders)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests: pure state only (the socket test is `tests/bots/fleet.rs`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Ball, GunInfo, ParkView, Policy};

    const ME: i64 = 90_000_001;
    const ANN: i64 = 90_000_002;
    const BEN: i64 = 90_000_003;
    const FLEET: i64 = 7;
    const ROCK_SHIP: i64 = 601;
    const SHIP: i64 = 100_000_009;
    const RAT: i64 = 900_000_001;

    fn joined(c: i64) -> Vec<PyValue> {
        vec![PyValue::Instance {
            class: b"util.KeyVal".to_vec(),
            state: Box::new(PyValue::Dict(vec![(
                PyValue::Int(c),
                PyValue::Bytes(b"charID".to_vec()),
            )])),
        }]
    }

    fn formed() -> Command {
        let mut c = Command::new(ROCK_SHIP);
        let orders = c.form_fleet(&[ANN, BEN]);
        c.on_done(&orders[0], &Ok(Some(FLEET)));
        c
    }

    #[test]
    fn same_corp_or_same_alliance_may_invite_and_no_alliance_is_no_bond() {
        let me = Affiliation {
            corp: 1,
            alliance: None,
        };
        assert!(may_invite(me, me));
        assert!(!may_invite(
            me,
            Affiliation {
                corp: 2,
                alliance: None
            }
        ));
        let in_a = |corp| Affiliation {
            corp,
            alliance: Some(10),
        };
        assert!(may_invite(in_a(1), in_a(2)), "allied corps");
        assert!(!may_invite(
            in_a(1),
            Affiliation {
                corp: 2,
                alliance: Some(11)
            }
        ));
    }

    #[test]
    fn forming_creates_then_invites_each_member_once_in_squads() {
        let mut c = Command::new(ROCK_SHIP);
        let orders = c.form_fleet(&[ANN, BEN]);
        assert_eq!(
            orders,
            vec![
                Order::Create {
                    ship_type: ROCK_SHIP
                },
                Order::Invite {
                    char_id: ANN,
                    squad: 0
                },
                Order::Invite {
                    char_id: BEN,
                    squad: 0
                },
            ]
        );
        assert!(c.has_fleet() && !c.is_formed());
        assert!(c.form_fleet(&[ANN, BEN]).is_empty(), "nothing twice");
        assert_eq!(squad_of(0), 0);
        assert_eq!(squad_of(SQUAD_SIZE), 1, "the eleventh opens squad two");
        assert_eq!(squad_of(usize::MAX / 2), MAX_SQUADS - 1);
    }

    #[test]
    fn a_capsule_is_reported_when_the_hull_is_unknown() {
        let mut c = Command::default();
        assert_eq!(
            c.form_fleet(&[]),
            vec![Order::Create {
                ship_type: brain::CAPSULE
            }]
        );
    }

    #[test]
    fn joins_leaves_and_a_failed_invite_are_folded_in() {
        let mut c = formed();
        assert_eq!(c.fleet_id, Some(FLEET));
        c.on_push("OnFleetJoin", &joined(ANN));
        assert!(!c.is_formed());
        c.on_push("OnFleetJoin", &joined(BEN));
        assert!(c.is_formed());
        c.on_push("OnFleetLeave", &[PyValue::Int(BEN)]);
        assert!(!c.is_formed());
        assert_eq!(
            c.form_fleet(&[]),
            vec![Order::Invite {
                char_id: BEN,
                squad: 0
            }],
            "the leaver is asked back"
        );
        let invite = Order::Invite {
            char_id: BEN,
            squad: 0,
        };
        c.on_done(&invite, &Err(Error::Refused("FleetError".to_owned())));
        assert_eq!(c.form_fleet(&[]), vec![invite], "and again after a refusal");
        c.on_push("OnFleetDisbanded", &[]);
        assert_eq!(c, Command::new(ROCK_SHIP));
    }

    #[test]
    fn warp_primary_regroup_and_disband_need_a_fleet() {
        let mut c = Command::new(ROCK_SHIP);
        assert!(c.fleet_warp_to(WarpTarget::Item(5)).is_empty());
        assert!(c.call_primary(RAT, None).is_empty());
        assert!(c.disband().is_empty());
        let mut c = formed();
        assert_eq!(
            c.fleet_warp_to(WarpTarget::Item(5)),
            vec![Order::Warp {
                target: WarpTarget::Item(5),
                range: FLEET_WARP_RANGE
            }]
        );
        assert_eq!(
            c.regroup(ME),
            vec![Order::Warp {
                target: WarpTarget::Member(ME),
                range: FLEET_WARP_RANGE
            }]
        );
        assert_eq!(
            c.call_primary(RAT, Some(11)),
            vec![Order::Broadcast {
                name: BROADCAST_TARGET,
                item: RAT,
                type_id: Some(11)
            }]
        );
        assert!(c.call_primary(RAT, Some(11)).is_empty(), "called once");
        assert_eq!(c.call_primary(RAT + 1, None).len(), 1, "a new primary is");
        assert_eq!(c.disband(), vec![Order::Disband { fleet: FLEET }]);
        assert!(!c.has_fleet(), "forgotten at once");
        assert_eq!(
            acts(vec![Order::Disband { fleet: FLEET }]),
            vec![Act::Fleet(Order::Disband { fleet: FLEET })]
        );
    }

    fn broadcast(name: &str, caller: i64, item: i64) -> Vec<PyValue> {
        vec![
            PyValue::Bytes(name.as_bytes().to_vec()),
            PyValue::Int(BROADCAST_ALL),
            PyValue::Int(caller),
            PyValue::Int(30_000_001),
            PyValue::Int(item),
            PyValue::None,
        ]
    }

    #[test]
    fn a_needbackup_is_read_whoever_sends_it_and_nothing_else_is() {
        let call = broadcast(BROADCAST_NEED_BACKUP, ANN, RAT);
        assert_eq!(
            backup_call(BROADCAST_EVENT, &call),
            Some(BackupCall {
                caller: ANN,
                system: 30_000_001
            })
        );
        assert_eq!(
            backup_call(BROADCAST_EVENT, &broadcast("Target", ANN, RAT)),
            None
        );
        assert_eq!(backup_call("OnFleetJoin", &call), None);
        assert_eq!(backup_call(BROADCAST_EVENT, &[]), None);
    }

    #[test]
    fn only_the_boss_calls_the_primary_and_only_a_target_is_one() {
        let led = Some(FleetView {
            fleet_id: FLEET,
            boss: Some(ME),
        });
        let call = broadcast("Target", ME, RAT);
        assert_eq!(primary_call(BROADCAST_EVENT, &call, led), Some(RAT));
        assert_eq!(primary_call(BROADCAST_EVENT, &call, None), None, "no fleet");
        assert_eq!(
            primary_call(BROADCAST_EVENT, &broadcast("Target", ANN, RAT), led),
            None,
            "not the boss"
        );
        assert_eq!(
            primary_call(BROADCAST_EVENT, &broadcast("WarpTo", ME, RAT), led),
            None
        );
        assert_eq!(primary_call("OnFleetJoin", &call, led), None);
        let own = Some(FleetView {
            fleet_id: FLEET,
            boss: None,
        });
        assert_eq!(
            primary_call(BROADCAST_EVENT, &broadcast("Target", ANN, RAT), own),
            Some(RAT),
            "a boss-less member (the commander) hears any call"
        );
    }

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    /// In space, in a fleet, a rat called primary 10 km off.
    fn seen(armed: bool) -> (Memory, Observation) {
        let m = Memory::new("Member1", Policy::fast());
        let mut o = Observation::empty(Instant::now());
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: true,
        };
        o.ship = Some(brain::ShipInfo {
            ship_id: SHIP,
            type_id: Some(ROCK_SHIP),
        });
        o.fleet = Some(FleetView {
            fleet_id: FLEET,
            boss: Some(ME),
        });
        o.primary = Some(RAT);
        o.lock_range = 50_000.0;
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: None,
            taken: Default::default(),
            hostiles: vec![
                Hostile {
                    ball: ball(RAT + 1, 2_000.0),
                    type_id: 1,
                    name: "Nearer".to_owned(),
                    shooting: None,
                },
                Hostile {
                    ball: ball(RAT, 10_000.0),
                    type_id: 1,
                    name: "Rat".to_owned(),
                    shooting: None,
                },
            ],
            wrecks: Vec::new(),
            ego_tether: None,
        });
        if armed {
            o.guns = vec![GunInfo {
                item_id: 1,
                type_id: 2,
                flag_id: 27,
                online: true,
                active: false,
                target: None,
                rounds: Some(100),
                capacity: 100,
                range: 5_000.0,
                reload: Duration::ZERO,
                kind: crate::brain::GunKind::Weapon,
                reach: 0.0,
            }];
        }
        (m, o)
    }

    #[test]
    fn an_unarmed_member_locks_the_primary_and_then_leaves_it_to_its_role() {
        let (mut m, mut o) = seen(false);
        assert_eq!(engage(&mut m, &o), Some(vec![Act::Lock(RAT)]));
        o.locked.push(RAT);
        assert_eq!(engage(&mut m, &o), None);
    }

    #[test]
    fn an_armed_member_fights_the_primary_not_the_nearer_rat() {
        let (mut m, mut o) = seen(true);
        let acts = engage(&mut m, &o).expect("engages");
        assert!(
            acts.iter()
                .any(|a| matches!(a, Act::Orbit { id, .. } if *id == RAT)),
            "{acts:?}"
        );
        if let Some(p) = &mut o.park {
            p.ego_tether = Some(RAT);
        }
        let acts = engage(&mut m, &o).expect("engages");
        assert!(acts.contains(&Act::Lock(RAT)), "{acts:?}");
        o.locked.push(RAT);
        let acts = engage(&mut m, &o).expect("engages");
        assert!(
            acts.contains(&Act::Fire {
                gun: 1,
                target: RAT
            }),
            "{acts:?}"
        );
        assert!(
            !acts.iter().any(
                |a| matches!(a, Act::Lock(id) | Act::Fire { target: id, .. } if *id == RAT + 1)
            ),
            "{acts:?}"
        );
    }

    #[test]
    fn no_fleet_no_primary_off_grid_or_homeward_is_the_roles_turn() {
        let (mut m, mut o) = seen(false);
        o.fleet = None;
        assert_eq!(engage(&mut m, &o), None);
        let (mut m, mut o) = seen(false);
        o.primary = None;
        assert_eq!(engage(&mut m, &o), None);
        let (mut m, mut o) = seen(false);
        o.primary = Some(RAT + 99);
        assert_eq!(engage(&mut m, &o), None, "not on this grid");
        let (mut m, o) = seen(false);
        m.homing = Some(brain::Homing::Repair);
        assert_eq!(engage(&mut m, &o), None);
    }

    #[test]
    fn a_human_called_primary_is_locked_too() {
        let (mut m, mut o) = seen(false);
        const HUMAN_SHIP: i64 = 100_000_050;
        o.humans = vec![ball(HUMAN_SHIP, 3_000.0)];
        o.primary = Some(HUMAN_SHIP);
        assert_eq!(engage(&mut m, &o), Some(vec![Act::Lock(HUMAN_SHIP)]));
    }

    #[test]
    fn a_theatre_action_becomes_fleet_orders() {
        use crate::doctrine_theatre::Action;
        let mut cmd = Command::new(587);
        // Nothing to invite, no fleet: a NeedBackup is still just a broadcast.
        let nb = orders_for(&mut cmd, &Action::NeedBackup { system: 5 }, 700, ME);
        assert_eq!(
            nb,
            vec![Order::Broadcast {
                name: BROADCAST_NEED_BACKUP,
                item: 700,
                type_id: None
            }]
        );
        let hold = orders_for(&mut cmd, &Action::HoldPosition, 700, ME);
        assert!(matches!(&hold[0], Order::Broadcast { name, .. } if *name == BROADCAST_HOLD));
        // A commit forms the fleet and invites; the warp waits for the joins.
        let go = orders_for(
            &mut cmd,
            &Action::Commit {
                who: vec![ANN, BEN],
            },
            700,
            ME,
        );
        assert!(matches!(go[0], Order::Create { .. }));
        assert_eq!(
            go.iter()
                .filter(|o| matches!(o, Order::Invite { .. }))
                .count(),
            2
        );
        assert!(!go.iter().any(|o| matches!(o, Order::Warp { .. })));
        cmd.on_done(&go[0], &Ok(Some(FLEET)));
        for c in [ANN, BEN] {
            cmd.on_push("OnFleetJoin", &[joined(c)[0].clone()]);
        }
        // A surge of one more: invited, and the formed fleet regroups on the commander.
        let more = orders_for(&mut cmd, &Action::Surge { who: vec![ANN] }, 700, ME);
        assert!(
            more.iter().all(|o| !matches!(o, Order::Invite { .. })),
            "already in"
        );
        assert_eq!(
            more,
            vec![Order::Warp {
                target: WarpTarget::Member(ME),
                range: FLEET_WARP_RANGE
            }]
        );
        // Said once; a disengage disbands, and the scout and rebuild are not fleet business.
        assert!(orders_for(&mut cmd, &Action::Surge { who: vec![] }, 700, ME).is_empty());
        assert!(orders_for(&mut cmd, &Action::Scout { system: 1 }, 700, ME).is_empty());
        assert!(orders_for(&mut cmd, &Action::Rebuild { counter: None }, 700, ME).is_empty());
        let off = orders_for(
            &mut cmd,
            &Action::Disengage {
                abandon_if_losing: true,
            },
            700,
            ME,
        );
        assert_eq!(off, vec![Order::Disband { fleet: FLEET }]);
    }

    /// BOT-FLEET-ORDERS: the global broadcasts are `SendBroadcast`, the
    /// primary call stays a bubble broadcast, and `EnemySpotted` is a call
    /// for help like `NeedBackup`.
    #[test]
    fn global_broadcasts_are_send_broadcast_and_target_is_bubble() {
        let mut c = Command::new(0);
        assert!(c.travel_to(30_000_777).is_empty(), "no fleet, no order");
        c.fleet_id = Some(5);
        let travel = c.travel_to(30_000_777);
        assert!(matches!(
            &travel[..],
            [Order::Broadcast { name, item: 30_000_777, .. }] if *name == BROADCAST_TRAVEL_TO
        ));
        assert!(travel[0].describe().0.starts_with("SendBroadcast TravelTo"));
        let spotted = c.enemy_spotted(1_000_777, Some(587));
        assert!(
            spotted[0]
                .describe()
                .0
                .starts_with("SendBroadcast EnemySpotted")
        );
        let primary = c.call_primary(1_000_778, None);
        assert!(
            primary[0]
                .describe()
                .0
                .starts_with("BroadcastToBubble Target")
        );
        let args = vec![
            PyValue::Bytes(BROADCAST_ENEMY_SPOTTED.as_bytes().to_vec()),
            PyValue::Int(3),
            PyValue::Int(90_000_777),
            PyValue::Int(30_000_777),
            PyValue::Int(1_000_777),
            PyValue::None,
        ];
        assert_eq!(
            backup_call(BROADCAST_EVENT, &args),
            Some(BackupCall {
                caller: 90_000_777,
                system: 30_000_777
            })
        );
    }
}
