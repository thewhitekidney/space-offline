//! The bot's view of its own session, kept the way the client keeps it: by
//! applying every `SessionChangeNotification` it is sent, solicited or not.
//!
//! **Why this exists.** Until P8a a session change was a boolean: "something
//! changed". That was enough to assert an undock, and not enough to play. A
//! fleet seat (`fleetid`), a jump (`solarsystemid`), a board (`shipid`) and a
//! dock (`stationid`) all arrive this way, and two of them arrive while the
//! bot called nothing -- the fleet boss invited it, the jump timer fired. A
//! bot that drops those frames believes it is somewhere it is not.
//!
//! **The shape** is `SessionChangeNotification(..., body=[sid, (0, change),
//! None])` (`eve_proto::MachoPacket::session_change`), and `change` is a wire
//! dict of `{attribute: (old, new)}` -- value-then-key, as every dict here.
//! The *new* half is what the client applies; the *old* half is rewritten by
//! the client's `ComputeNotifySets` before anybody reads it
//! (`service.rs::undock_change`'s doc comment) **[V]**, so nothing here
//! trusts it except to tell "changed" from "absent".

use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// One session change, decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionChange {
    /// `attribute -> (old, new)`, exactly as sent.
    pub pairs: BTreeMap<String, (PyValue, PyValue)>,
}

impl SessionChange {
    /// Decode a `SessionChangeNotification` packet, or `None` for any other
    /// frame -- including one we cannot read, which is reported by the caller
    /// rather than guessed at.
    pub fn from_frame(frame: &PyValue) -> Option<SessionChange> {
        let PyValue::Instance { class, .. } = frame else {
            return None;
        };
        if !class.ends_with(b"SessionChangeNotification") {
            return None;
        }
        let packet = eve_proto::MachoPacket::from_value(frame).ok()?;
        // `[sid, (0, change), None]`.
        let PyValue::Tuple(wrapped) = packet.body.get(1)? else {
            return None;
        };
        let PyValue::Dict(entries) = wrapped.get(1)? else {
            return None;
        };
        let mut pairs = BTreeMap::new();
        for (value, key) in entries {
            let Some(key) = crate::text(key) else {
                continue;
            };
            let (old, new) = match value {
                PyValue::Tuple(p) if p.len() == 2 => (p[0].clone(), p[1].clone()),
                PyValue::List(p) if p.len() == 2 => (p[0].clone(), p[1].clone()),
                _ => continue,
            };
            pairs.insert(key, (old, new));
        }
        Some(SessionChange { pairs })
    }

    /// The new value of one attribute, if this change carries it and it is
    /// an int. `Some(None)` would be ambiguous, so "moved to `None`" is
    /// [`Self::cleared`].
    pub fn new_i64(&self, attribute: &str) -> Option<i64> {
        match self.pairs.get(attribute) {
            Some((_, PyValue::Int(v))) => Some(*v),
            _ => None,
        }
    }

    /// Whether this change sets `attribute` to `None`.
    pub fn cleared(&self, attribute: &str) -> bool {
        matches!(self.pairs.get(attribute), Some((_, PyValue::None)))
    }

    /// Whether this change mentions `attribute` at all.
    pub fn touches(&self, attribute: &str) -> bool {
        self.pairs.contains_key(attribute)
    }

    /// **Whether the client would destroy its park on this change.**
    ///
    /// `michelle.Park.Release` runs when the ship leaves the system it was
    /// bound in: on docking, and on a jump (`destiny::on_ballpark_bind`'s doc
    /// comment, "the real client binds once per undock") **[V]**. So:
    ///
    /// - `stationid` gains a value -- a dock; or
    /// - `solarsystemid` changes at all -- a jump (`A -> B`), a dock
    ///   (`A -> None`) or an undock (`None -> A`, where there is nothing
    ///   bound to release and releasing is harmless).
    ///
    /// The bound ballpark id and every park belief go with it, which is what
    /// makes `bind_ballpark` correct again after a dock or a jump without
    /// `rebind_ballpark`.
    pub fn releases_park(&self) -> bool {
        let docked = self.new_i64("stationid").is_some();
        let moved = self
            .pairs
            .get("solarsystemid")
            .is_some_and(|(old, new)| old != new);
        docked || moved
    }

    /// Whether this change moves the session to another location -- a
    /// station or a system -- which is when the client's location-bound
    /// monikers (`(stationID, groupStation)`, `(solarsystemID,
    /// groupSolarSystem)`) stop naming where it is.
    pub fn moves_location(&self) -> bool {
        self.touches("stationid") || self.touches("solarsystemid")
    }
}

/// The session as the bot currently believes it to be.
///
/// Every attribute ever set, by name, plus typed accessors for the five a
/// player-like bot reads. Built only from session changes, so it cannot hold
/// a belief the server never stated.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionView {
    /// Every attribute's latest *new* value.
    pub attributes: BTreeMap<String, PyValue>,
    /// How many changes have been applied. A cheap way for a caller to see
    /// "something moved since I last looked".
    pub changes: u64,
}

impl SessionView {
    pub fn apply(&mut self, change: &SessionChange) {
        for (key, (_, new)) in &change.pairs {
            self.attributes.insert(key.clone(), new.clone());
        }
        self.changes += 1;
    }

    /// One attribute as an int; `None` when absent or `None`.
    pub fn get_i64(&self, attribute: &str) -> Option<i64> {
        match self.attributes.get(attribute) {
            Some(PyValue::Int(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn char_id(&self) -> Option<i64> {
        self.get_i64("charid")
    }

    pub fn ship_id(&self) -> Option<i64> {
        self.get_i64("shipid")
    }

    /// Docked here, or `None` in space.
    pub fn station_id(&self) -> Option<i64> {
        self.get_i64("stationid")
    }

    /// **The system this ship is in space in**, or `None` while docked:
    /// docked sessions carry `solarsystemid2` and no `solarsystemid`
    /// (`service.rs::character_change`) **[V]**.
    pub fn solar_system_id(&self) -> Option<i64> {
        self.get_i64("solarsystemid")
    }

    /// The system, docked or not.
    pub fn solar_system_id2(&self) -> Option<i64> {
        self.get_i64("solarsystemid2")
            .or_else(|| self.solar_system_id())
    }

    pub fn fleet_id(&self) -> Option<i64> {
        self.get_i64("fleetid")
    }

    /// The location-bound moniker's params, as the client's `eveMoniker`
    /// builds them: `(stationID, groupStation=15)` docked,
    /// `(solarsystemID, groupSolarSystem=5)` in space (`fitting.md` §1.1,
    /// `service/inventory.rs`'s `dogmaIM` arm) **[V]**.
    pub fn location_moniker(&self) -> Option<PyValue> {
        let (id, group) = match (self.station_id(), self.solar_system_id()) {
            (Some(station), _) => (station, GROUP_STATION),
            (None, Some(system)) => (system, GROUP_SOLAR_SYSTEM),
            (None, None) => return None,
        };
        Some(PyValue::Tuple(vec![PyValue::Int(id), PyValue::Int(group)]))
    }
}

/// `const.groupStation` **[V]**.
pub const GROUP_STATION: i64 = 15;
/// `const.groupSolarSystem` **[V]**.
pub const GROUP_SOLAR_SYSTEM: i64 = 5;

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(change: Vec<(PyValue, PyValue)>) -> PyValue {
        eve_proto::MachoPacket::session_change(
            eve_proto::MachoAddress::node(1),
            eve_proto::MachoAddress::client(1),
            7,
            change,
        )
        .to_value()
        .expect("encodes")
    }

    fn pair(key: &str, old: PyValue, new: PyValue) -> (PyValue, PyValue) {
        (
            PyValue::Tuple(vec![old, new]),
            PyValue::Bytes(key.as_bytes().to_vec()),
        )
    }

    #[test]
    fn a_dock_is_decoded_and_releases_the_park() {
        // The server's own `dock_change` shape: two keys, both halves.
        let f = frame(vec![
            pair("stationid", PyValue::None, PyValue::Int(60_012_526)),
            pair("solarsystemid", PyValue::Int(30_000_001), PyValue::None),
        ]);
        let c = SessionChange::from_frame(&f).expect("decodes");
        assert_eq!(c.new_i64("stationid"), Some(60_012_526));
        assert!(c.cleared("solarsystemid"));
        assert!(c.releases_park());
        assert!(c.moves_location());

        let mut view = SessionView::default();
        view.apply(&c);
        assert_eq!(view.station_id(), Some(60_012_526));
        assert_eq!(view.solar_system_id(), None);
        assert_eq!(
            view.location_moniker(),
            Some(PyValue::Tuple(vec![
                PyValue::Int(60_012_526),
                PyValue::Int(GROUP_STATION)
            ]))
        );
    }

    #[test]
    fn a_jump_releases_the_park_and_a_board_does_not() {
        let jump = SessionChange::from_frame(&frame(vec![pair(
            "solarsystemid",
            PyValue::Int(30_000_001),
            PyValue::Int(30_000_003),
        )]))
        .expect("decodes");
        assert!(jump.releases_park());

        let board = SessionChange::from_frame(&frame(vec![pair(
            "shipid",
            PyValue::Int(1),
            PyValue::Int(2),
        )]))
        .expect("decodes");
        assert!(!board.releases_park());
        assert!(!board.moves_location());
        assert_eq!(board.new_i64("shipid"), Some(2));
    }

    #[test]
    fn a_notification_is_not_a_session_change() {
        let n = eve_proto::MachoPacket::new(
            eve_marshal::MachoNetMsg::CallRsp,
            eve_proto::MachoAddress::node(1),
            eve_proto::MachoAddress::client(1),
            vec![PyValue::None],
        )
        .to_value()
        .expect("encodes");
        assert_eq!(SessionChange::from_frame(&n), None);
    }
}
