//! The fleet's wire shapes: the notifications, and the values the bound
//! object's reads answer with.
//!
//! Argument counts and orders are the client handlers' own signatures in
//! `eve/client/script/parklife/fleetSvc.py` **[V]**, cross-checked against
//! `findings/push-events.tsv`, which records the same handlers by bytecode
//! line. Two conventions from `crate::pyutil` matter throughout: a marshal
//! dict holds `(value, key)` pairs (`dict_kv`, `pydict`), and a `KeyVal` is
//! an attribute bag the client reads with `.name`.

use super::{Fleet, Fleets, Member, MemberOptOuts, Options, SessionFleetState, Slot, Squad, Wing};
use crate::pyutil::{bstr, dict_kv, keyval, pydict};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// Something the fleet tells its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetEvent {
    /// `OnFleetJoin(member)` **[V] fleetSvc.py:1147**.
    Join { member: Member },
    /// `OnFleetLeave(charID)` **[V] fleetSvc.py:1167**.
    Leave { char_id: i64 },
    /// `OnFleetMemberChanged`, thirteen arguments: the member, the fleet,
    /// five old fields, five new ones and `isOnlyMember`
    /// **[V] fleetSvc.py:1197**. (The client-side `_Local` echo it scatters
    /// takes twelve; that one is not ours to send.)
    MemberChanged {
        char_id: i64,
        fleet_id: i64,
        old: Slot,
        new: Slot,
        is_only_member: bool,
    },
    /// `OnFleetWingAdded(wingID)` **[V] fleetSvc.py:1238**.
    WingAdded { wing_id: i64 },
    /// `OnFleetWingDeleted(wingID)` **[V] fleetSvc.py:1242**.
    WingDeleted { wing_id: i64 },
    /// `OnFleetSquadAdded(wingID, squadID)` **[V] fleetSvc.py:1246**.
    SquadAdded { wing_id: i64, squad_id: i64 },
    /// `OnFleetSquadDeleted(squadID)` **[V] fleetSvc.py:1250**.
    SquadDeleted { squad_id: i64 },
    /// `OnFleetWingNameChanged(wingID, name)` **[V] fleetSvc.py:1109**.
    WingNameChanged { wing_id: i64, name: String },
    /// `OnFleetSquadNameChanged(squadID, name)` **[V] fleetSvc.py:1114**.
    SquadNameChanged { squad_id: i64, name: String },
    /// `OnFleetInvite(fleetID, inviteID, msgName, msgDict)`
    /// **[V] fleetSvc.py:1119**. `inviteID` is the inviter's character (it
    /// goes to `evelink.character_link`), `msgName` the dialog and
    /// `msgDict` a real dict: the handler calls `.get('autoAccept')` on it
    /// and stores `name` into it.
    Invite {
        fleet_id: i64,
        inviter_id: i64,
        msg_name: String,
        auto_accept: bool,
    },
    /// `OnFleetMotdChanged(motd, reloadMotd)` **[V] fleetSvc.py:1875**.
    MotdChanged { motd: String, reload_motd: bool },
    /// `OnFleetOptionsChanged(oldOptions, options)`: two option objects,
    /// each read by attribute (`.isRegistered`, `.isFreeMove`) and the new
    /// one stored as the client's options **[V] fleetSvc.py:1254-1265**.
    OptionsChanged { old: Options, new: Options },
    /// `OnFleetDisbanded(charIDs)`: iterated, so a list even for one
    /// **[V] fleetSvc.py:1193-1195**.
    Disbanded { char_ids: Vec<i64> },
    /// `OnFleetJumpBeaconModuleChange(charID, solarsystemID, beaconID,
    /// typeID, active)` **[V] fleetSvc.py:204**.
    BeaconChange {
        char_id: i64,
        solar_system_id: i64,
        beacon_id: i64,
        type_id: i64,
        active: bool,
    },
    /// `OnBridgeModeChange(shipID, solarsystemID, itemID, active)`
    /// **[V] fleetSvc.py:196**.
    BridgeChange {
        ship_id: i64,
        solar_system_id: i64,
        beacon_id: i64,
        active: bool,
    },
    /// `OnFleetStateChange(fleetState)`: the client stores the object and
    /// reads `fleetState.targetTags.get(itemID)` off it
    /// **[V] fleetSvc.py:193-194, 222-226**.
    State { tags: BTreeMap<i64, String> },
    /// `OnFleetLootEvent(lootEvents)`: `{(charID, typeID): quantity}`, read
    /// as `k[0]`, `k[1]` and `v` **[V] fleetSvc.py:1501-1503**.
    Loot {
        char_id: i64,
        type_id: i64,
        quantity: i64,
    },
}

impl FleetEvent {
    /// The event name and its arguments, in the handler's order.
    pub fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let int = PyValue::Int;
        match self {
            Self::Join { member } => ("OnFleetJoin", vec![member.to_pyvalue()]),
            Self::Leave { char_id } => ("OnFleetLeave", vec![int(*char_id)]),
            Self::MemberChanged {
                char_id,
                fleet_id,
                old,
                new,
                is_only_member,
            } => {
                let mut args = vec![int(*char_id), int(*fleet_id)];
                for s in [old, new] {
                    args.extend([
                        int(s.wing_id),
                        int(s.squad_id),
                        int(s.role),
                        int(s.job),
                        s.opt_outs.to_pyvalue(),
                    ]);
                }
                args.push(PyValue::Bool(*is_only_member));
                ("OnFleetMemberChanged", args)
            }
            Self::WingAdded { wing_id } => ("OnFleetWingAdded", vec![int(*wing_id)]),
            Self::WingDeleted { wing_id } => ("OnFleetWingDeleted", vec![int(*wing_id)]),
            Self::SquadAdded { wing_id, squad_id } => {
                ("OnFleetSquadAdded", vec![int(*wing_id), int(*squad_id)])
            }
            Self::SquadDeleted { squad_id } => ("OnFleetSquadDeleted", vec![int(*squad_id)]),
            Self::WingNameChanged { wing_id, name } => (
                "OnFleetWingNameChanged",
                vec![int(*wing_id), PyValue::Unicode(name.clone())],
            ),
            Self::SquadNameChanged { squad_id, name } => (
                "OnFleetSquadNameChanged",
                vec![int(*squad_id), PyValue::Unicode(name.clone())],
            ),
            Self::Invite {
                fleet_id,
                inviter_id,
                msg_name,
                auto_accept,
            } => (
                "OnFleetInvite",
                vec![
                    int(*fleet_id),
                    int(*inviter_id),
                    bstr(msg_name),
                    pydict(vec![
                        ("autoAccept", PyValue::Bool(*auto_accept)),
                        ("name", int(*inviter_id)),
                    ]),
                ],
            ),
            Self::MotdChanged { motd, reload_motd } => (
                "OnFleetMotdChanged",
                vec![PyValue::Unicode(motd.clone()), PyValue::Bool(*reload_motd)],
            ),
            Self::OptionsChanged { old, new } => (
                "OnFleetOptionsChanged",
                vec![old.to_pyvalue(), new.to_pyvalue()],
            ),
            Self::Disbanded { char_ids } => (
                "OnFleetDisbanded",
                vec![PyValue::List(char_ids.iter().copied().map(int).collect())],
            ),
            Self::BeaconChange {
                char_id,
                solar_system_id,
                beacon_id,
                type_id,
                active,
            } => (
                "OnFleetJumpBeaconModuleChange",
                vec![
                    int(*char_id),
                    int(*solar_system_id),
                    int(*beacon_id),
                    int(*type_id),
                    PyValue::Bool(*active),
                ],
            ),
            Self::BridgeChange {
                ship_id,
                solar_system_id,
                beacon_id,
                active,
            } => (
                "OnBridgeModeChange",
                vec![
                    int(*ship_id),
                    int(*solar_system_id),
                    int(*beacon_id),
                    PyValue::Bool(*active),
                ],
            ),
            Self::State { tags } => ("OnFleetStateChange", vec![state_keyval(tags)]),
            Self::Loot {
                char_id,
                type_id,
                quantity,
            } => (
                "OnFleetLootEvent",
                vec![dict_kv([(
                    PyValue::Tuple(vec![int(*char_id), int(*type_id)]),
                    int(*quantity),
                )])],
            ),
        }
    }
}

/// A fleet notification and the characters it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Push {
    pub to: Vec<i64>,
    pub event: FleetEvent,
}

impl crate::notify::Push for Push {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        self.event.to_push()
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `{targetTags: {itemID: tag}}`.
fn state_keyval(tags: &BTreeMap<i64, String>) -> PyValue {
    keyval(vec![(
        "targetTags",
        dict_kv(
            tags.iter()
                .map(|(item, tag)| (PyValue::Int(*item), PyValue::Unicode(tag.clone()))),
        ),
    )])
}

fn opt_int(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

impl MemberOptOuts {
    /// The three keys `GetNewMemberOptOutsKeyval` builds
    /// **[V] evefleet/members.py:8-9**.
    pub fn to_pyvalue(self) -> PyValue {
        keyval(vec![
            ("acceptsFleetWarp", PyValue::Bool(self.accepts_fleet_warp)),
            (
                "acceptsConduitJumps",
                PyValue::Bool(self.accepts_conduit_jumps),
            ),
            (
                "acceptsFleetRegroups",
                PyValue::Bool(self.accepts_fleet_regroups),
            ),
        ])
    }
}

impl Member {
    /// A member record. `timestamp` is a FILETIME, so it marshals as a
    /// long; `memberOptOuts` is read off it by `TakesFleetWarp` and its
    /// two siblings **[V] fleetSvc.py:424-431**.
    pub fn to_pyvalue(&self) -> PyValue {
        keyval(vec![
            ("charID", PyValue::Int(self.char_id)),
            ("wingID", PyValue::Int(self.wing_id)),
            ("squadID", PyValue::Int(self.squad_id)),
            ("role", PyValue::Int(self.role)),
            ("job", PyValue::Int(self.job)),
            (
                "skills",
                PyValue::List(self.skills.iter().copied().map(PyValue::Int).collect()),
            ),
            ("timestamp", PyValue::Int(self.timestamp)),
            ("stationID", opt_int(self.station_id)),
            ("shipTypeID", opt_int(self.ship_type_id)),
            ("solarSystemID", opt_int(self.solar_system_id)),
            ("memberOptOuts", self.opt_outs.to_pyvalue()),
        ])
    }
}

impl Squad {
    pub fn to_pyvalue(&self) -> PyValue {
        keyval(vec![
            ("squadID", PyValue::Int(self.squad_id)),
            ("name", PyValue::Unicode(self.name.clone())),
        ])
    }
}

impl Wing {
    /// A wing with `name` and `squads`, the mapping `GetFleetHierarchy`
    /// walks with `.iteritems()` **[V] fleetSvc.py:457-460**.
    pub fn to_pyvalue(&self) -> PyValue {
        keyval(vec![
            ("wingID", PyValue::Int(self.wing_id)),
            ("name", PyValue::Unicode(self.name.clone())),
            (
                "squads",
                dict_kv(
                    self.squads
                        .values()
                        .map(|s| (PyValue::Int(s.squad_id), s.to_pyvalue())),
                ),
            ),
        ])
    }
}

impl Options {
    pub fn to_pyvalue(self) -> PyValue {
        keyval(vec![
            ("isFreeMove", PyValue::Bool(self.is_free_move)),
            ("isRegistered", PyValue::Bool(self.is_registered)),
            ("autoJoinSquadID", opt_int(self.auto_join_squad_id)),
        ])
    }
}

impl Fleet {
    /// `GetInitState()`: the five attributes `InitFleet` reads, and no
    /// others **[V] fleetSvc.py:254-259**. `members` is keyed by character
    /// id, because the client primes owner names from its keys
    /// (`cfg.eveowners.Prime(self.members.keys())`, `:260`).
    pub fn init_state(&self) -> PyValue {
        keyval(vec![
            ("fleetID", PyValue::Int(self.fleet_id)),
            (
                "members",
                dict_kv(
                    self.members
                        .values()
                        .map(|m| (PyValue::Int(m.char_id), m.to_pyvalue())),
                ),
            ),
            ("wings", self.wings_pyvalue()),
            ("options", self.options.to_pyvalue()),
            ("motd", PyValue::Unicode(self.motd.clone())),
        ])
    }

    /// `GetWings()`: wing id to wing.
    pub fn wings_pyvalue(&self) -> PyValue {
        dict_kv(
            self.wings
                .values()
                .map(|w| (PyValue::Int(w.wing_id), w.to_pyvalue())),
        )
    }

    /// The fleet-state object: today only the target tags.
    pub fn state_pyvalue(&self) -> PyValue {
        state_keyval(&self.target_tags)
    }
}

impl Fleets {
    /// A session change carrying `state`'s four attributes.
    ///
    /// Each entry is `(old, new)`, but the client compares `new` against its
    /// own current value and never reads `old` **[V]
    /// carbon/common/script/sys/basesession.py:1095-1100**, so `old` is sent
    /// as `None`.
    pub fn session_change(state: SessionFleetState) -> Vec<(PyValue, PyValue)> {
        let change = |v: Option<i64>| PyValue::Tuple(vec![PyValue::None, opt_int(v)]);
        crate::pyutil::pypairs(vec![
            ("fleetid", change(state.fleet_id)),
            ("fleetrole", change(state.fleet_role)),
            ("wingid", change(state.wing_id)),
            ("squadid", change(state.squad_id)),
        ])
    }
}
