//! **Fleet orders** (BOT-FLEET-ORDERS, EVEJS-GAP bots Spec 2b): a bot in a
//! fleet obeys what its boss presses in the fleet window.
//!
//! The client sends a global broadcast through the bound
//! `fleet.SendBroadcast(name, scope, itemID, typeID)` (`fleetSvc.py:941`),
//! which the server relays as `OnFleetBroadcast(name, scope, charID,
//! solarSystemID, itemID, typeID)` **[V]** (`fleet_broadcast.rs`). Names are
//! `evefleet/const.py`'s **[V]**.
//!
//! - [`read`] turns one push into a [`Heard`] and the sender. Pure.
//! - **Authority [A]**: only the fleet's boss gives orders (`FleetView::boss`
//!   is the inviter -- wing and squad commanders are not known to bots;
//!   falsified by a friend who commands a squad and is ignored). The
//!   exception is a `Heal*` call, which is a request for reps and not an
//!   order, so any member may make it. `NeedBackup` and `EnemySpotted` are
//!   not orders either: they keep the relation-weighted path
//!   (`Driver::weigh_backup`, [`crate::effects::help_decision`]).
//! - [`hear`] stores an order in [`crate::brain::Memory::order`] (a newer
//!   one replaces it; leaving the fleet clears it, [`expire`]) or a heal
//!   call in `Memory::heal`.
//! - Rule 2d of `brain::choose`: [`holding`] (Hold, and Align once aligned)
//!   keeps a bot still and lets only [`HOLD_KEEPS`] acts through, so a held
//!   bot still locks, fires, repairs and flies drones; [`act`] gives the
//!   other orders as acts. Both are gated by REACT-JITTER
//!   ([`crate::react::Class::Order`]).
//! - Who obeys what **[A]**: every role obeys Travel and Hold; haulers,
//!   builders and the other passive roles ([`PASSIVE`]) obey nothing else;
//!   an unarmed bot ignores Backup.

use crate::brain::{Act, ActKind, FleetView, Memory, Observation, Place, RoleRef, SHORT_WAIT};
use crate::fleet::BROADCAST_EVENT;
use crate::logi;
use eve_marshal::PyValue;
use std::time::{Duration, Instant};

/// `evefleet.BROADCAST_*` names **[V]** (`const.py:52-66`).
pub const WARP_TO: &str = "WarpTo";
pub const ALIGN_TO: &str = "AlignTo";
pub const JUMP_TO: &str = "JumpTo";
pub const TRAVEL_TO: &str = "TravelTo";
pub const HOLD_POSITION: &str = "HoldPosition";
pub const IN_POSITION: &str = "InPosition";
pub const ENEMY_SPOTTED: &str = "EnemySpotted";
pub const NEED_BACKUP: &str = "NeedBackup";
pub const HEAL_ARMOR: &str = "HealArmor";
pub const HEAL_SHIELD: &str = "HealShield";
pub const HEAL_CAPACITOR: &str = "HealCapacitor";
pub const HEAL_TARGET: &str = "HealTarget";

/// How long each order stands **[A]**, modelled on EvEJS's verb table
/// (`orders/model.mts:47-59`, a hint). A newer order replaces it sooner.
/// Falsified by bots still holding when the owner has long moved on (too
/// long) or bots that drift off an order mid-fight (too short).
pub const WARP_FOR: Duration = Duration::from_secs(10 * 60);
pub const ALIGN_FOR: Duration = Duration::from_secs(10 * 60);
pub const JUMP_FOR: Duration = Duration::from_secs(10 * 60);
pub const TRAVEL_FOR: Duration = Duration::from_secs(20 * 60);
pub const HOLD_FOR: Duration = Duration::from_secs(10 * 60);
/// How long a call for reps stands **[A]**: a fight is short; falsified by
/// a logi that drops a ward mid-cycle (too short).
pub const HEAL_FOR: Duration = Duration::from_secs(60);
/// The most turns spent getting to a gate and through it **[A]**: a warp,
/// an approach and the jump are three; the rest is slack for a stale park.
pub const JUMP_STEPS: u32 = 10;
/// After ordering a warp the bot waits this long for the warp to begin
/// before the order counts as carried out **[A]**; a bot never re-warps for
/// the same order.
pub const WARP_SETTLE: Duration = Duration::from_secs(3);

/// The roles that obey nothing but Travel and Hold **[A]**: haulers and
/// builders (and the traders, reactors and planetary pilots, who work from
/// a station).
pub const PASSIVE: [&str; 6] = ["courier", "freighter", "builder", "trader", "reactor", "pi"];

/// What a held bot still does (`ActKind`s): shooting, targeting, repairing,
/// drones, talking, waiting. Anything that moves the ship is dropped.
pub const HOLD_KEEPS: [ActKind; 15] = [
    ActKind::Lock,
    ActKind::Unlock,
    ActKind::Fire,
    ActKind::CeaseFire,
    ActKind::LoadAmmo,
    ActKind::Logi,
    ActKind::Drone,
    ActKind::Say,
    ActKind::Wait,
    ActKind::Stance,
    ActKind::Mine,
    ActKind::StopMining,
    ActKind::Resync,
    ActKind::Missing,
    ActKind::Reconnect,
];

/// Which bars a call for reps names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealKind {
    Armor,
    Shield,
    Capacitor,
    /// `HealTarget`: whatever the sender's ship needs.
    Target,
}

/// One broadcast, understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    /// `NeedBackup` or `EnemySpotted`: a call for help (not an order).
    Backup {
        caller: i64,
        system: i64,
        item: Option<i64>,
    },
    Warp {
        item: i64,
    },
    Align {
        item: i64,
    },
    /// `JumpTo`: a stargate ball.
    Jump {
        gate: i64,
    },
    /// `TravelTo`: a solar system id.
    Travel {
        system: i64,
    },
    Hold,
    /// A call for reps: the sender's ship.
    Heal {
        ship: i64,
        kind: HealKind,
    },
    InPosition,
}

/// **An order in force**, in `Memory::order`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub heard: Heard,
    /// Distinct per order, the REACT-JITTER key.
    pub seq: u64,
    pub until: Instant,
    /// When the bot first acted on it (once-only acts).
    pub acted: Option<Instant>,
    /// Steps taken toward a gate (a jump is warp, approach, jump).
    pub steps: u32,
}

/// **A call for reps** in force, in `Memory::heal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealCall {
    pub ship: i64,
    pub kind: HealKind,
    pub until: Instant,
}

fn int(args: &[PyValue], i: usize) -> Option<i64> {
    match args.get(i) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// **Read one push**: `OnFleetBroadcast(name, scope, charID, solarSystemID,
/// itemID, typeID)` **[V]** as `(what was heard, the sender's character)`.
/// `None` for another push, an unknown name, a sender who may not say it
/// (the authority rule above), or an order with no item. A `Backup` is
/// heard whether or not the bot knows its fleet.
pub fn read(event: &str, args: &[PyValue], fleet: Option<FleetView>) -> Option<(Heard, i64)> {
    if event != BROADCAST_EVENT {
        return None;
    }
    let name = args.first().and_then(crate::text)?;
    let sender = int(args, 2)?;
    let item = int(args, 4);
    if matches!(name.as_str(), NEED_BACKUP | ENEMY_SPOTTED) {
        return Some((
            Heard::Backup {
                caller: sender,
                system: int(args, 3)?,
                item,
            },
            sender,
        ));
    }
    let fleet = fleet?;
    // A call for reps: any member.
    let kind = match name.as_str() {
        HEAL_ARMOR => Some(HealKind::Armor),
        HEAL_SHIELD => Some(HealKind::Shield),
        HEAL_CAPACITOR => Some(HealKind::Capacitor),
        HEAL_TARGET => Some(HealKind::Target),
        _ => None,
    };
    if let Some(kind) = kind {
        // The sender's own ship (a bubble broadcast names it); `HealTarget`
        // names the item.
        return Some((Heard::Heal { ship: item?, kind }, sender));
    }
    if fleet.boss != Some(sender) {
        return None;
    }
    let heard = match name.as_str() {
        WARP_TO => Heard::Warp { item: item? },
        ALIGN_TO => Heard::Align { item: item? },
        JUMP_TO => Heard::Jump { gate: item? },
        TRAVEL_TO => Heard::Travel { system: item? },
        HOLD_POSITION => Heard::Hold,
        IN_POSITION => Heard::InPosition,
        _ => return None,
    };
    Some((heard, sender))
}

/// Does `role` work from a station and so obey only Travel and Hold?
pub fn passive(role: RoleRef) -> bool {
    PASSIVE.contains(&role.name())
}

/// Would a bot of this role follow `heard`?
fn obeys(role: RoleRef, heard: &Heard) -> bool {
    match heard {
        Heard::Travel { .. } | Heard::Hold => true,
        Heard::Warp { .. } | Heard::Align { .. } | Heard::Jump { .. } => !passive(role),
        // Not orders: the driver's backup path, the logi's, or nothing.
        Heard::Backup { .. } | Heard::Heal { .. } | Heard::InPosition => false,
    }
}

/// **Take in what was heard** (`now`: the driver's clock). An order is
/// stored in `m.order`, replacing the last; a heal call in `m.heal`; the
/// rest (a backup call, an "in position") is not stored here.
pub fn hear(m: &mut Memory, heard: Heard, now: Instant) {
    let until = |d: Duration| now + d;
    match heard {
        Heard::Heal { ship, kind } => {
            if logi::is_logi(m.policy.role) {
                m.heal = Some(HealCall {
                    ship,
                    kind,
                    until: until(HEAL_FOR),
                });
            }
        }
        h if obeys(m.policy.role, &h) => {
            let d = match h {
                Heard::Warp { .. } => WARP_FOR,
                Heard::Align { .. } => ALIGN_FOR,
                Heard::Jump { .. } => JUMP_FOR,
                Heard::Travel { .. } => TRAVEL_FOR,
                _ => HOLD_FOR,
            };
            m.order_seq += 1;
            m.order = Some(Order {
                heard: h,
                seq: m.order_seq,
                until: until(d),
                acted: None,
                steps: 0,
            });
        }
        _ => {}
    }
}

/// Drop orders and calls that have run out, and all of them once the bot
/// is out of its fleet.
pub fn expire(m: &mut Memory, o: &Observation) {
    if o.fleet.is_none() {
        m.order = None;
        m.heal = None;
        return;
    }
    if m.order.is_some_and(|x| o.now >= x.until) {
        m.order = None;
    }
    if m.heal.is_some_and(|x| o.now >= x.until) {
        m.heal = None;
    }
}

/// **Is the bot holding?** `None`: no. `Some(true)`: yes, and the `Stop`
/// has not been sent yet (this call marks it sent). Hold, and Align once
/// the bot has aligned; not while it is still thinking about the order.
pub fn holding(m: &mut Memory, o: &Observation) -> Option<bool> {
    expire(m, o);
    let order = m.order?;
    match order.heard {
        Heard::Hold => {}
        Heard::Align { .. } if order.acted.is_some() => {}
        _ => return None,
    }
    if order.acted.is_none()
        && m.thinking(crate::react::Class::Order, order.seq as i64, o)
            .is_some()
    {
        return None;
    }
    let first = order.acted.is_none();
    if first && let Some(x) = &mut m.order {
        x.acted = Some(o.now);
    }
    Some(first)
}

/// **Hold's filter**: keep only the acts that do not move the ship; when
/// nothing is left the bot waits.
pub fn hold_filter(acts: Vec<Act>) -> Vec<Act> {
    let mut kept: Vec<Act> = acts
        .into_iter()
        .filter(|a| HOLD_KEEPS.contains(&a.kind()))
        .collect();
    if kept.is_empty() {
        kept.push(Act::Wait(SHORT_WAIT));
    }
    kept
}

/// **The other orders as acts** (rule 2d, after a called primary and before
/// the role): `None` when there is nothing to do and the role decides.
pub fn act(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    expire(m, o);
    let order = m.order?;
    let docked = matches!(o.place, Place::Docked { .. });
    match order.heard {
        // Docked, only Travel can be carried out; the rest are for space.
        Heard::Warp { .. } | Heard::Align { .. } | Heard::Jump { .. } if docked => {
            m.order = None;
            return None;
        }
        Heard::Travel { .. } | Heard::Warp { .. } | Heard::Align { .. } | Heard::Jump { .. } => {}
        _ => return None,
    }
    // Fleeing or repairing comes first; the order waits.
    if m.homing.is_some() {
        return None;
    }
    // REACT-JITTER: the boss's order is read before it is carried out.
    if order.acted.is_none()
        && let Some(left) = m.thinking(crate::react::Class::Order, order.seq as i64, o)
    {
        return Some(vec![Act::Wait(left.min(SHORT_WAIT))]);
    }
    match order.heard {
        Heard::Travel { system } => {
            m.order = None;
            Some(vec![Act::TravelTo { system }])
        }
        Heard::Warp { item } => match order.acted {
            None => {
                if let Some(x) = &mut m.order {
                    x.acted = Some(o.now);
                }
                Some(vec![Act::WarpToItem {
                    id: item,
                    range: 0.0,
                }])
            }
            Some(at) if o.now.saturating_duration_since(at) < WARP_SETTLE => {
                Some(vec![Act::Wait(SHORT_WAIT)])
            }
            Some(_) => {
                m.order = None;
                None
            }
        },
        Heard::Align { item } => {
            if let Some(x) = &mut m.order {
                x.acted = Some(o.now);
            }
            Some(vec![Act::AlignTo(item)])
        }
        Heard::Jump { gate } => {
            let steps = order.steps + 1;
            if steps > JUMP_STEPS {
                m.order = None;
                return None;
            }
            if let Some(x) = &mut m.order {
                x.steps = steps;
            }
            Some(vec![Act::JumpGate { gate }])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::Role;

    const BOSS: i64 = 90_000_901;
    const MATE: i64 = 90_000_902;
    const SYSTEM: i64 = 30_000_901;
    const GATE: i64 = 1_000_901;
    const SHIP: i64 = 1_000_902;
    const FLEET: i64 = 1_000;

    fn view() -> Option<FleetView> {
        Some(FleetView {
            fleet_id: FLEET,
            boss: Some(BOSS),
        })
    }

    fn push(name: &str, sender: i64, item: Option<i64>) -> Vec<PyValue> {
        vec![
            PyValue::Bytes(name.as_bytes().to_vec()),
            PyValue::Int(3),
            PyValue::Int(sender),
            PyValue::Int(SYSTEM),
            item.map_or(PyValue::None, PyValue::Int),
            PyValue::None,
        ]
    }

    fn heard(name: &str, sender: i64, item: Option<i64>) -> Option<(Heard, i64)> {
        read(BROADCAST_EVENT, &push(name, sender, item), view())
    }

    #[test]
    fn every_name_reads_to_its_own_heard() {
        let h = |name, item| heard(name, BOSS, item).map(|x| x.0);
        assert_eq!(h(WARP_TO, Some(GATE)), Some(Heard::Warp { item: GATE }));
        assert_eq!(h(ALIGN_TO, Some(GATE)), Some(Heard::Align { item: GATE }));
        assert_eq!(h(JUMP_TO, Some(GATE)), Some(Heard::Jump { gate: GATE }));
        assert_eq!(
            h(TRAVEL_TO, Some(SYSTEM)),
            Some(Heard::Travel { system: SYSTEM })
        );
        assert_eq!(h(HOLD_POSITION, Some(SHIP)), Some(Heard::Hold));
        assert_eq!(h(IN_POSITION, Some(SHIP)), Some(Heard::InPosition));
        for name in [NEED_BACKUP, ENEMY_SPOTTED] {
            assert_eq!(
                h(name, Some(SHIP)),
                Some(Heard::Backup {
                    caller: BOSS,
                    system: SYSTEM,
                    item: Some(SHIP)
                }),
                "{name}"
            );
        }
        for (name, kind) in [
            (HEAL_ARMOR, HealKind::Armor),
            (HEAL_SHIELD, HealKind::Shield),
            (HEAL_CAPACITOR, HealKind::Capacitor),
            (HEAL_TARGET, HealKind::Target),
        ] {
            assert_eq!(h(name, Some(SHIP)), Some(Heard::Heal { ship: SHIP, kind }));
        }
        assert_eq!(h("Nonsense", Some(SHIP)), None);
        assert_eq!(h(WARP_TO, None), None, "an order needs its item");
        assert_eq!(
            read("OnFleetJoin", &push(WARP_TO, BOSS, Some(GATE)), view()),
            None
        );
    }

    #[test]
    fn only_the_boss_orders_but_any_member_calls_for_reps() {
        for name in [TRAVEL_TO, WARP_TO, ALIGN_TO, JUMP_TO, HOLD_POSITION] {
            assert_eq!(heard(name, MATE, Some(SYSTEM)), None, "{name} from a mate");
        }
        assert!(heard(HEAL_ARMOR, MATE, Some(SHIP)).is_some());
        // A backup call is heard from anyone, and without a known fleet.
        let args = push(NEED_BACKUP, MATE, Some(SHIP));
        assert!(read(BROADCAST_EVENT, &args, None).is_some());
        // Orders need a fleet and a known boss.
        assert_eq!(
            read(BROADCAST_EVENT, &push(HOLD_POSITION, BOSS, None), None),
            None
        );
        let no_boss = Some(FleetView {
            fleet_id: FLEET,
            boss: None,
        });
        assert_eq!(
            read(BROADCAST_EVENT, &push(HOLD_POSITION, BOSS, None), no_boss),
            None
        );
    }

    fn space() -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::InSpace {
            system: SYSTEM,
            bound: true,
        };
        o
    }

    fn memory(role: RoleRef) -> Memory {
        let mut m = Memory::new("Order Test", crate::brain::Policy::fast());
        m.policy.role = role;
        m
    }

    #[test]
    fn a_mates_travel_order_is_not_stored_and_the_bosss_is() {
        let mut m = memory(Role::Escort);
        let now = Instant::now();
        // The driver calls `hear` only with what `read` let through.
        assert!(heard(TRAVEL_TO, MATE, Some(SYSTEM)).is_none());
        hear(&mut m, Heard::Travel { system: SYSTEM }, now);
        let o = m.order.expect("stored");
        assert_eq!(o.until, now + TRAVEL_FOR);
        // A newer order replaces it.
        hear(&mut m, Heard::Hold, now);
        assert_eq!(m.order.map(|x| x.heard), Some(Heard::Hold));
        assert_ne!(m.order.map(|x| x.seq), Some(o.seq));
    }

    #[test]
    fn haulers_and_builders_obey_only_travel_and_hold() {
        for role in [&crate::hauler::ROLE, &crate::builder::ROLE] {
            let mut m = memory(role);
            let now = Instant::now();
            hear(&mut m, Heard::Warp { item: GATE }, now);
            hear(&mut m, Heard::Jump { gate: GATE }, now);
            hear(&mut m, Heard::Align { item: GATE }, now);
            assert_eq!(m.order, None, "{}", role.name());
            hear(&mut m, Heard::Travel { system: SYSTEM }, now);
            assert!(m.order.is_some());
            hear(&mut m, Heard::Hold, now);
            assert_eq!(m.order.map(|x| x.heard), Some(Heard::Hold));
        }
    }

    #[test]
    fn hold_drops_the_moves_and_keeps_fire() {
        let acts = vec![
            Act::Orbit {
                id: GATE,
                range: 1_000.0,
            },
            Act::Follow {
                id: GATE,
                range: 500.0,
            },
            Act::WarpToItem {
                id: GATE,
                range: 0.0,
            },
            Act::Lock(SHIP),
            Act::Fire {
                gun: 7,
                target: SHIP,
            },
        ];
        let kept = hold_filter(acts);
        assert_eq!(
            kept.iter().map(Act::kind).collect::<Vec<_>>(),
            vec![ActKind::Lock, ActKind::Fire]
        );
        assert_eq!(
            hold_filter(vec![Act::Undock]),
            vec![Act::Wait(SHORT_WAIT)],
            "nothing left: wait"
        );
    }

    #[test]
    fn a_hold_order_stops_once_then_holds_and_expires() {
        let mut m = memory(Role::Escort);
        m.policy.react = false;
        let mut o = space();
        o.fleet = view();
        let now = o.now;
        hear(&mut m, Heard::Hold, now);
        assert_eq!(holding(&mut m, &o), Some(true), "the first call sends Stop");
        assert_eq!(holding(&mut m, &o), Some(false));
        let mut late = o.clone();
        late.now = now + HOLD_FOR + Duration::from_secs(1);
        assert_eq!(holding(&mut m, &late), None, "expired");
        // Out of the fleet, the order goes.
        hear(&mut m, Heard::Hold, now);
        let mut out = o.clone();
        out.fleet = None;
        assert_eq!(holding(&mut m, &out), None);
        assert_eq!(m.order, None);
    }

    #[test]
    fn travel_becomes_one_travel_act_and_is_spent() {
        let mut m = memory(Role::Escort);
        m.policy.react = false;
        let mut o = space();
        o.fleet = view();
        hear(&mut m, Heard::Travel { system: SYSTEM }, o.now);
        assert_eq!(
            act(&mut m, &o),
            Some(vec![Act::TravelTo { system: SYSTEM }])
        );
        assert_eq!(act(&mut m, &o), None);
    }

    #[test]
    fn an_order_is_read_before_it_is_carried_out() {
        let mut m = memory(Role::Escort);
        m.policy.react = true;
        let mut o = space();
        o.fleet = view();
        hear(&mut m, Heard::Warp { item: GATE }, o.now);
        let first = act(&mut m, &o).expect("an act");
        assert!(matches!(first[..], [Act::Wait(_)]), "{first:?}");
        o.now += crate::react::HI + Duration::from_millis(1);
        let second = act(&mut m, &o).expect("an act");
        assert!(
            matches!(second[..], [Act::WarpToItem { id: GATE, .. }]),
            "{second:?}"
        );
        // Once only.
        let third = act(&mut m, &o).expect("waits for the warp to begin");
        assert!(matches!(third[..], [Act::Wait(_)]));
        o.now += WARP_SETTLE;
        assert_eq!(act(&mut m, &o), None);
        assert_eq!(m.order, None);
    }
}
