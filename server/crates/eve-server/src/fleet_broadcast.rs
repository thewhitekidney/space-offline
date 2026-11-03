//! **Fleet broadcasts: "Target", "Warp to", "Need backup"** (SOC-C7).
//!
//! Lane note: `docs/lanes/soc-c7.md`. The client's calls **[V]**
//! (`fleetSvc.pyc`):
//!
//! - `SendBubbleBroadcast(name, itemID, typeID)` (`:1133`) calls
//!   `fleetMgr.BroadcastToBubble(name, self.broadcastScope, itemID,
//!   typeID)`; `SendSystemBroadcast` (`:1146`) the same on
//!   `BroadcastToSystem`; `SendGlobalBroadcast` (`:1119`) calls the bound
//!   fleet's `SendBroadcast` with the same four. Each first checks
//!   `name in evefleet.ALL_BROADCASTS`. `SendBroadcast_Target(itemID)` --
//!   "call a primary" -- is a **bubble** broadcast of `'Target'`.
//! - The names are strings (`evefleet/const`: `'Target'`, `'WarpTo'`,
//!   `'NeedBackup'`, ... **[V]**); the scope is `BROADCAST_DOWN/UP/ALL` =
//!   `1/2/3` **[V]**, the hierarchy: the range is in the method name **[I]**.
//! - Everyone hears it as `OnFleetBroadcast(name, scope, charID,
//!   solarSystemID, itemID, typeID)`, six positional (`:1753`, argcount 7
//!   with `self`) **[V]**. The sender is in the audience: the handler
//!   itself drops `charID == session.charid` unless the pilot asked to see
//!   his own (`WantBroadcast:1742`, `BROADCAST_SHOW_OWN`) **[V]**.
//!
//! Served here: `BroadcastToBubble` and `BroadcastToSystem` (registered by
//! hunks in `service/fleet.rs`'s `fleetMgr` arm, coordinator diff). The bound
//! `SendBroadcast` (universe) is still refused by name, as
//! `docs/lanes/fleet.md` §6 left it.
//!
//! **Who hears it** ([`audience`]): the fleet's members chosen by the scope
//! ([`hierarchy`]), cut to the range -- the bubble is the sender's grid
//! (`crate::grid::on_grid`, ships in space in his system) **[A]**, the
//! system is his solar system. Falsified by a pilot on grid who does not see
//! a broadcast, or one off grid who does.

use crate::fleet::{Fleet, Member, consts};
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The push **[V]** (`fleetSvc.OnFleetBroadcast`).
pub const EVENT: &str = "OnFleetBroadcast";
/// `evefleet.BROADCAST_DOWN` **[V]**: the sender and those under him.
pub const BROADCAST_DOWN: i64 = 1;
/// `evefleet.BROADCAST_UP` **[V]**: the sender and those over him.
pub const BROADCAST_UP: i64 = 2;
/// `evefleet.BROADCAST_ALL` **[V]**: everyone.
pub const BROADCAST_ALL: i64 = 3;

/// `evefleet.ALL_BROADCASTS` **[V]** (`evefleet/const.py`): the only names
/// `fleetSvc.SendBubbleBroadcast` lets through (`name not in
/// evefleet.ALL_BROADCASTS` raises `Illegal broadcast`).
pub const ALL_BROADCASTS: [&str; 15] = [
    "EnemySpotted",
    "NeedBackup",
    "HoldPosition",
    "InPosition",
    "TravelTo",
    "JumpBeacon",
    "Location",
    "Target",
    "HealTarget",
    "HealArmor",
    "HealShield",
    "HealCapacitor",
    "WarpTo",
    "AlignTo",
    "JumpTo",
];

/// `fleetSvc.MIN_BROADCAST_TIME = 2` seconds **[V]**: the gap the client
/// keeps between two broadcasts of the same name, and a third of it between
/// two different names (`IsTimeRestricted`). The server holds a sender to
/// the same gaps.
pub const MIN_BROADCAST_SECS: f64 = 2.0;

/// The per-sender gap state: the last broadcast's name and time.
#[derive(Debug, Clone, Default)]
pub struct Limiter {
    last: BTreeMap<i64, (String, Instant)>,
}

/// Past this many remembered senders the stale ones are dropped.
const LIMITER_SWEEP_AT: usize = 4096;

impl Limiter {
    /// Whether `who` may send `name` at `now`, recording it if so. The
    /// client's own rule, line for line (`IsTimeRestricted`): the same name
    /// waits [`MIN_BROADCAST_SECS`], another a third of that.
    pub fn allow(&mut self, who: i64, name: &str, now: Instant) -> bool {
        if let Some((last_name, at)) = self.last.get(&who) {
            let gap = if last_name == name {
                MIN_BROADCAST_SECS
            } else {
                MIN_BROADCAST_SECS / 3.0
            };
            if now.saturating_duration_since(*at).as_secs_f64() < gap {
                return false;
            }
        }
        if self.last.len() >= LIMITER_SWEEP_AT {
            let keep = Duration::from_secs_f64(MIN_BROADCAST_SECS);
            self.last
                .retain(|_, (_, at)| now.saturating_duration_since(*at) < keep);
        }
        self.last.insert(who, (name.to_owned(), now));
        true
    }
}

/// How far a broadcast carries: the method it came in on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// `BroadcastToBubble`: the sender's grid.
    Bubble,
    /// `BroadcastToSystem`: the sender's solar system.
    System,
    /// The bound `fleet.SendBroadcast`: the whole hierarchy reached, no
    /// system or grid cut.
    Fleet,
}

/// One `OnFleetBroadcast`, to a list of characters.
#[derive(Debug, Clone, PartialEq)]
pub struct BroadcastPush {
    pub args: Vec<PyValue>,
    pub to: Vec<i64>,
}

impl crate::notify::Push for BroadcastPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (EVENT, self.args.clone())
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Is `m` under `me` in the fleet's tree (or `me` himself)?
fn under(me: &Member, m: &Member) -> bool {
    if m.char_id == me.char_id || me.is_boss() || me.role == consts::ROLE_LEADER {
        return true;
    }
    if me.role == consts::ROLE_WING_CMDR {
        return m.wing_id == me.wing_id;
    }
    if me.role == consts::ROLE_SQUAD_CMDR {
        return m.squad_id == me.squad_id;
    }
    false
}

/// **The members a scope reaches**, the sender always among them, sorted.
/// An unknown scope is [`BROADCAST_ALL`] **[A]** (falsified by a client
/// sending a fourth value).
pub fn hierarchy(fleet: &Fleet, sender: i64, scope: i64) -> Vec<i64> {
    let Some(me) = fleet.members.get(&sender) else {
        return Vec::new();
    };
    fleet
        .members
        .values()
        .filter(|m| match scope {
            BROADCAST_DOWN => under(me, m),
            BROADCAST_UP => under(m, me),
            _ => true,
        })
        .map(|m| m.char_id)
        .collect()
}

/// **Who hears `sender`'s broadcast**: [`hierarchy`] cut to `range`.
pub fn audience(reg: &mut Registry, sender: i64, scope: i64, range: Range) -> Vec<i64> {
    let Some(fleet) = reg.fleets().fleet_of(sender) else {
        return Vec::new();
    };
    let reached = hierarchy(fleet, sender, scope);
    if range == Range::Fleet {
        return reached;
    }
    let system = |reg: &Registry, c: i64| reg.session_system(c);
    let Some(home) = system(reg, sender) else {
        return Vec::new();
    };
    match range {
        Range::Fleet => reached,
        Range::System => reached
            .into_iter()
            .filter(|&c| system(reg, c) == Some(home))
            .collect(),
        Range::Bubble => {
            reg.advance_character(sender);
            let Some(at) = reg.roster().get(sender).map(|p| p.motion.position) else {
                // Docked: a bubble of one.
                return vec![sender];
            };
            reached
                .into_iter()
                .filter(|&c| {
                    if c == sender {
                        return true;
                    }
                    reg.advance_character(c);
                    reg.roster().get(c).is_some_and(|p| {
                        p.solar_system_id == home
                            && crate::grid::on_grid(at, p.motion.position, false)
                    })
                })
                .collect()
        }
    }
}

/// `fleetMgr.BroadcastToBubble` / `BroadcastToSystem(name, scope, itemID,
/// typeID)`: push [`EVENT`] to the [`audience`]. Answers `None` (the client
/// discards it, `fleetSvc.py:1143`, `:1156` [V]); a caller in no fleet is
/// `FleetNotInFleet`, the dialog the rest of the subsystem raises.
pub fn dispatch(
    reg: &mut Registry,
    req: &Request<'_>,
    range: Range,
) -> Result<PyValue, ServiceError> {
    let not_in_fleet = || ServiceError::Raise(crate::fleet::FleetError::NotInFleet.to_user_error());
    let Some(actor) = req.actor else {
        return Err(not_in_fleet());
    };
    if reg.fleets().fleet_of(actor).is_none() {
        return Err(not_in_fleet());
    }
    // SEC-FIX 5: the client's own checks, held on the server. The name is
    // one of `ALL_BROADCASTS`, the scope one of the four `BROADCAST_*`
    // values, the item and type ids integers or `None`; nothing else is
    // relayed to the fleet.
    let bad = |why: &str| ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    };
    if req.args.len() != 4 {
        return Err(bad("a broadcast is (name, scope, itemID, typeID)"));
    }
    let name_text = match crate::pyutil::as_bytes(&req.args[0]).map(std::str::from_utf8) {
        Some(Ok(n)) if ALL_BROADCASTS.contains(&n) => n.to_owned(),
        _ => return Err(bad("not a broadcast name")),
    };
    let scope = match &req.args[1] {
        PyValue::Int(n) if (0..=BROADCAST_ALL).contains(n) => *n,
        _ => return Err(bad("not a broadcast scope")),
    };
    for id in &req.args[2..] {
        if !matches!(id, PyValue::None | PyValue::Int(_)) {
            return Err(bad("itemID and typeID are integers or None"));
        }
    }
    let name = req.args[0].clone();
    if !reg
        .fleets_mut()
        .broadcast_limiter_mut()
        .allow(actor, &name_text, Instant::now())
    {
        // The client drops these too (`BroadcastTimeRestriction`).
        return Ok(PyValue::None);
    }
    let solar_system = reg
        .session_system(actor)
        .map_or(PyValue::None, PyValue::Int);
    let args = vec![
        name,
        PyValue::Int(scope),
        PyValue::Int(actor),
        solar_system,
        req.args[2].clone(),
        req.args[3].clone(),
    ];
    let to = audience(reg, actor, scope, range);
    if !to.is_empty() {
        reg.schedule(Duration::ZERO, BroadcastPush { args, to });
    }
    Ok(PyValue::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::Fleets;

    const BOSS: i64 = 1_000_001;
    const ANN: i64 = 1_000_002;
    const BEN: i64 = 1_000_003;
    const SHIP: i64 = 670;

    fn fleet() -> (Fleets, i64) {
        let mut f = Fleets::new(1);
        let id = f.create(BOSS).expect("created");
        f.init(BOSS, id, Some(SHIP), None, None).expect("init");
        for who in [ANN, BEN] {
            f.invite(BOSS, id, who, None, None, None).expect("invited");
            f.accept_invite(who, id, Some(SHIP), None, None)
                .expect("accepted");
        }
        (f, id)
    }

    #[test]
    fn all_reaches_everyone_and_the_sender_hears_himself() {
        let (f, id) = fleet();
        let fleet = f.fleet(id).expect("a fleet");
        assert_eq!(hierarchy(fleet, ANN, BROADCAST_ALL), vec![BOSS, ANN, BEN]);
        assert_eq!(
            hierarchy(fleet, ANN, 99),
            vec![BOSS, ANN, BEN],
            "unknown is all"
        );
        assert!(
            hierarchy(fleet, 9, BROADCAST_ALL).is_empty(),
            "a stranger reaches nobody"
        );
    }

    #[test]
    fn down_from_the_boss_is_everyone_and_down_from_a_member_is_himself() {
        let (f, id) = fleet();
        let fleet = f.fleet(id).expect("a fleet");
        assert_eq!(hierarchy(fleet, BOSS, BROADCAST_DOWN), vec![BOSS, ANN, BEN]);
        assert_eq!(hierarchy(fleet, ANN, BROADCAST_DOWN), vec![ANN]);
    }

    #[test]
    fn up_from_a_member_reaches_the_boss_and_not_his_squadmate() {
        let (f, id) = fleet();
        let fleet = f.fleet(id).expect("a fleet");
        let up = hierarchy(fleet, ANN, BROADCAST_UP);
        assert!(up.contains(&BOSS) && up.contains(&ANN), "{up:?}");
        assert!(!up.contains(&BEN), "{up:?}");
    }

    #[test]
    fn the_push_has_six_arguments() {
        let p = BroadcastPush {
            args: vec![PyValue::None; 6],
            to: vec![ANN],
        };
        let (event, args) = crate::notify::Push::to_push(&p);
        assert_eq!(event, "OnFleetBroadcast");
        assert_eq!(args.len(), 6);
        assert_eq!(crate::notify::Push::audience(&p), Some(vec![ANN]));
    }

    // ---- SEC-FIX 5 ------------------------------------------------------

    fn registry_in_fleet() -> Registry {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = [BOSS, ANN]
                .iter()
                .map(|&id| crate::model::Character {
                    character_id: id,
                    name: format!("Pilot {id}"),
                    ship_id: 100_000_000 + id,
                    ..crate::model::Character::default()
                })
                .collect();
        });
        let id = reg.fleets_mut().create(BOSS).expect("created");
        reg.fleets_mut()
            .init(BOSS, id, Some(SHIP), None, None)
            .expect("init");
        reg.fleets_mut()
            .invite(BOSS, id, ANN, None, None, None)
            .expect("invited");
        reg.fleets_mut()
            .accept_invite(ANN, id, Some(SHIP), None, None)
            .expect("accepted");
        for who in [BOSS, ANN] {
            reg.character_mut(who).expect("a pilot").solar_system_id = Some(30_000_999);
        }
        reg
    }

    fn send(reg: &mut Registry, args: Vec<PyValue>) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "fleetMgr",
                method: "BroadcastToSystem",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(BOSS),
            },
            Range::System,
        )
    }

    fn bcast(name: &str) -> Vec<PyValue> {
        vec![
            PyValue::Bytes(name.as_bytes().to_vec()),
            PyValue::Int(BROADCAST_ALL),
            PyValue::Int(5),
            PyValue::None,
        ]
    }

    #[test]
    fn only_the_clients_broadcast_names_scopes_and_id_types_are_relayed() {
        let mut reg = registry_in_fleet();
        for args in [
            // A name the client never sends, and one that is not a string.
            bcast("Spam"),
            vec![
                PyValue::Int(7),
                PyValue::Int(3),
                PyValue::None,
                PyValue::None,
            ],
            // A scope outside the four values.
            vec![
                bcast("Target")[0].clone(),
                PyValue::Int(99),
                PyValue::None,
                PyValue::None,
            ],
            // Ids that are not integers: a megabyte string, a list.
            vec![
                bcast("Target")[0].clone(),
                PyValue::Int(3),
                PyValue::Unicode("x".repeat(1_000_000)),
                PyValue::None,
            ],
            vec![
                bcast("Target")[0].clone(),
                PyValue::Int(3),
                PyValue::None,
                PyValue::List(vec![]),
            ],
            // The wrong arity.
            vec![bcast("Target")[0].clone()],
        ] {
            assert!(
                matches!(
                    send(&mut reg, args.clone()),
                    Err(ServiceError::BadArguments { .. })
                ),
                "{args:?}"
            );
        }
        assert_eq!(reg.scheduled_count(), 0, "nothing was relayed");
        send(&mut reg, bcast("Target")).expect("a real one");
        assert_eq!(reg.scheduled_count(), 1);
    }

    fn fleet_send(
        reg: &mut Registry,
        actor: i64,
        args: Vec<PyValue>,
    ) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "fleet",
                method: "SendBroadcast",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(actor),
            },
            Range::Fleet,
        )
    }

    fn travel_to(scope: i64) -> Vec<PyValue> {
        vec![
            PyValue::Bytes(b"TravelTo".to_vec()),
            PyValue::Int(scope),
            PyValue::Int(30_000_002),
            PyValue::None,
        ]
    }

    #[test]
    fn the_fleet_range_reaches_members_in_other_systems() {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = [BOSS, ANN, BEN]
                .iter()
                .map(|&id| crate::model::Character {
                    character_id: id,
                    name: format!("Pilot {id}"),
                    ship_id: 100_000_000 + id,
                    ..crate::model::Character::default()
                })
                .collect();
        });
        let id = reg.fleets_mut().create(BOSS).expect("created");
        reg.fleets_mut()
            .init(BOSS, id, Some(SHIP), None, None)
            .expect("init");
        for who in [ANN, BEN] {
            reg.fleets_mut()
                .invite(BOSS, id, who, None, None, None)
                .expect("invited");
            reg.fleets_mut()
                .accept_invite(who, id, Some(SHIP), None, None)
                .expect("accepted");
        }
        reg.character_mut(BOSS).expect("a pilot").solar_system_id = Some(30_000_999);
        reg.character_mut(ANN).expect("a pilot").solar_system_id = Some(30_000_001);
        reg.character_mut(BEN).expect("a pilot").solar_system_id = Some(30_000_002);
        let all = audience(&mut reg, BOSS, BROADCAST_ALL, Range::Fleet);
        assert_eq!(all.len(), 3, "{all:?}");
        for who in [BOSS, ANN, BEN] {
            assert!(all.contains(&who), "{who} in {all:?}");
        }
        // The system range would have cut it to the sender's own system.
        assert_eq!(
            audience(&mut reg, BOSS, BROADCAST_ALL, Range::System),
            vec![BOSS]
        );
        // DOWN from a plain member reaches only him.
        assert_eq!(
            audience(&mut reg, ANN, BROADCAST_DOWN, Range::Fleet),
            vec![ANN]
        );
        fleet_send(&mut reg, BOSS, travel_to(BROADCAST_ALL)).expect("relayed");
        assert_eq!(reg.scheduled_count(), 1);
    }

    #[test]
    fn a_bad_name_on_the_fleet_range_is_bad_arguments() {
        let mut reg = registry_in_fleet();
        let mut args = travel_to(BROADCAST_ALL);
        args[0] = PyValue::Bytes(b"NoSuchButton".to_vec());
        assert!(matches!(
            fleet_send(&mut reg, BOSS, args),
            Err(ServiceError::BadArguments { .. })
        ));
        assert_eq!(reg.scheduled_count(), 0);
    }

    #[test]
    fn a_repeat_within_two_seconds_on_the_fleet_range_is_dropped() {
        let mut reg = registry_in_fleet();
        fleet_send(&mut reg, BOSS, travel_to(BROADCAST_ALL)).expect("first");
        fleet_send(&mut reg, BOSS, travel_to(BROADCAST_ALL)).expect("repeat answers None");
        assert_eq!(reg.scheduled_count(), 1);
    }

    #[test]
    fn a_flood_is_held_to_the_clients_own_gap() {
        let mut reg = registry_in_fleet();
        for _ in 0..50 {
            send(&mut reg, bcast("Target")).expect("answers");
        }
        assert_eq!(reg.scheduled_count(), 1, "one in the first two seconds");
    }

    #[test]
    fn the_gap_is_the_clients_two_seconds_for_a_name_and_a_third_for_another() {
        let mut l = Limiter::default();
        let t = Instant::now();
        let secs = Duration::from_secs_f64;
        assert!(l.allow(BOSS, "Target", t));
        assert!(!l.allow(BOSS, "Target", t + secs(MIN_BROADCAST_SECS - 0.1)));
        assert!(l.allow(BOSS, "Target", t + secs(MIN_BROADCAST_SECS)));
        let t2 = t + secs(MIN_BROADCAST_SECS);
        assert!(!l.allow(BOSS, "WarpTo", t2 + secs(MIN_BROADCAST_SECS / 3.0 - 0.1)));
        assert!(l.allow(BOSS, "WarpTo", t2 + secs(MIN_BROADCAST_SECS / 3.0)));
        // Another sender has a gap of his own.
        assert!(l.allow(ANN, "Target", t));
    }
}
