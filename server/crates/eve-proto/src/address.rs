//! `macho.MachoAddress`. Its `__getstate__` tuple's shape depends on the
//! address type, and `__setstate__` switches on `state[0]` to pick the shape
//! (`carbon/common/script/net/machoNetAddress.py`).

use eve_marshal::{AddressType, PyValue};

use crate::packet::ProtoError;

/// A machoNet address. The variants are exactly the four `ADDRESS_TYPE_*`
/// values, and each carries the fields that type's state tuple carries.
#[derive(Debug, Clone, PartialEq)]
pub enum MachoAddress {
    /// `(1, nodeID, service, callID)`
    ///
    /// `nodeID` is an **option**, for the same reason `callID` is. The client
    /// sends exactly one node address per session with no node id at all: the
    /// `ClientHasReleasedTheseObjects` notification, whose source is a bare
    /// `MachoAddress()`. The client has no node id to put there -- it is not a
    /// node -- so the field marshals as `None` and the state tuple reads
    /// `(1, None, None, None)`. Coercing it to an integer rejected the packet.
    Node {
        node_id: Option<i64>,
        service: Option<String>,
        call_id: Option<i64>,
    },
    /// `(2, clientID, callID, service)` — note callID precedes service here,
    /// unlike every other variant.
    Client {
        client_id: i64,
        call_id: Option<i64>,
        service: Option<String>,
    },
    /// `(4, broadcastID, narrowcast, idtype)` where narrowcast is a list.
    Broadcast {
        broadcast_id: String,
        narrowcast: Vec<PyValue>,
        id_type: String,
    },
    /// `(8, service, callID)` — a 3-tuple, not 4.
    Any {
        service: Option<String>,
        call_id: Option<i64>,
    },
}

fn svc(s: &Option<String>) -> PyValue {
    match s {
        Some(v) => PyValue::Bytes(v.as_bytes().to_vec()),
        None => PyValue::None,
    }
}

fn as_svc(v: &PyValue) -> Result<Option<String>, ProtoError> {
    match v {
        PyValue::None => Ok(None),
        PyValue::Bytes(b) => Ok(Some(String::from_utf8_lossy(b).into_owned())),
        PyValue::Unicode(s) => Ok(Some(s.clone())),
        _ => Err(ProtoError::Shape(
            "address service must be a string or None",
        )),
    }
}

fn as_int(v: &PyValue, what: &'static str) -> Result<i64, ProtoError> {
    match v {
        PyValue::Int(i) => Ok(*i),
        _ => Err(ProtoError::Shape(what)),
    }
}

/// `callID` is `None` on an address that is not awaiting a reply — the first
/// real client addressed its `machoNet` node that way — so it is parsed as an
/// option rather than coerced. `None` and `0` are different callIDs, and a
/// reply has to echo back whichever one arrived.
fn as_opt_int(v: &PyValue, what: &'static str) -> Result<Option<i64>, ProtoError> {
    match v {
        PyValue::None => Ok(None),
        PyValue::Int(i) => Ok(Some(*i)),
        _ => Err(ProtoError::Shape(what)),
    }
}

fn opt_int(v: &Option<i64>) -> PyValue {
    match v {
        Some(i) => PyValue::Int(*i),
        None => PyValue::None,
    }
}

fn as_str(v: &PyValue, what: &'static str) -> Result<String, ProtoError> {
    match v {
        PyValue::Bytes(b) => Ok(String::from_utf8_lossy(b).into_owned()),
        PyValue::Unicode(s) => Ok(s.clone()),
        _ => Err(ProtoError::Shape(what)),
    }
}

impl MachoAddress {
    /// A client address with nothing bound, as used for a notification's
    /// destination.
    pub fn client(client_id: i64) -> Self {
        MachoAddress::Client {
            client_id,
            call_id: Some(0),
            service: None,
        }
    }

    /// A node address, as used for a notification's source.
    pub fn node(node_id: i64) -> Self {
        MachoAddress::Node {
            node_id: Some(node_id),
            service: None,
            call_id: Some(0),
        }
    }

    /// The service this address names, if any.
    ///
    /// Only the Node, Client and Any forms carry one; Broadcast does not.
    /// A `CallReq`'s destination is where the service name lives — it is not
    /// in the call payload — so this is how dispatch finds out what was
    /// called.
    pub fn service(&self) -> Option<&str> {
        match self {
            MachoAddress::Node { service, .. }
            | MachoAddress::Client { service, .. }
            | MachoAddress::Any { service, .. } => service.as_deref(),
            MachoAddress::Broadcast { .. } => None,
        }
    }

    /// The callID this address carries, which a reply must echo so the client
    /// can match it to its pending call. Broadcast has none.
    pub fn call_id(&self) -> Option<i64> {
        match self {
            MachoAddress::Node { call_id, .. }
            | MachoAddress::Client { call_id, .. }
            | MachoAddress::Any { call_id, .. } => *call_id,
            MachoAddress::Broadcast { .. } => None,
        }
    }

    pub fn address_type(&self) -> AddressType {
        match self {
            MachoAddress::Node { .. } => AddressType::Node,
            MachoAddress::Client { .. } => AddressType::Client,
            MachoAddress::Broadcast { .. } => AddressType::Broadcast,
            MachoAddress::Any { .. } => AddressType::Any,
        }
    }

    /// The whitelisted class name a `MachoAddress` is marshalled under.
    ///
    /// `class MachoAddress():` compiles with an empty bases tuple, so it is
    /// an old-style class and goes on the wire as marshal tag `0x17` with
    /// this name. The name is in `globalsWhitelist`; see
    /// `docs/reference/object-wrapper.md`.
    pub const CLASS: &'static str = "carbon.common.script.net.machoNetAddress.MachoAddress";

    /// The marshalled object: a `0x17` instance wrapping [`Self::to_state`].
    ///
    /// This, not the bare state tuple, is what a packet's `source` and
    /// `destination` fields hold — `MachoPacket.__getstate__` returns the
    /// address *objects*, and the marshaller wraps each one.
    pub fn to_value(&self) -> PyValue {
        PyValue::Instance {
            class: Self::CLASS.as_bytes().to_vec(),
            state: Box::new(self.to_state()),
        }
    }

    /// Parse either a `0x17`-wrapped address or a bare state tuple.
    ///
    /// Accepting the bare tuple keeps every existing caller and test working,
    /// and costs nothing: the two shapes are not confusable.
    ///
    /// **The class is matched on its final component, not its whole path.**
    /// [`Self::CLASS`] is what *we* write and what this client sends, but the
    /// module a server marshals the class from is that server's business:
    /// EvEJS sends `net.machoNetPacket.MachoAddress` -- the right class from a
    /// different module -- and an exact-path comparison rejected every packet
    /// it ever sent, so each one parsed as "not a MachoPacket". That made
    /// `evecap --calls` report every call in a foreign capture as `HANG`,
    /// which is a tool reporting a protocol failure that did not happen.
    ///
    /// Found by the `evejs-probe` lane, which worked around it in its own
    /// crate rather than reaching in here, and left the decision. The
    /// decision is that the last component is the check: it still refuses
    /// anything that is not a `MachoAddress`, and the path never carried
    /// information we acted on.
    pub fn from_value(v: &PyValue) -> Result<Self, ProtoError> {
        match v {
            PyValue::Instance { class, state } => {
                if !is_macho_address(class) {
                    return Err(ProtoError::Shape("address is not a MachoAddress instance"));
                }
                Self::from_state(state)
            }
            other => Self::from_state(other),
        }
    }

    /// The `__getstate__` tuple.
    pub fn to_state(&self) -> PyValue {
        let t = self.address_type() as i64;
        match self {
            MachoAddress::Node {
                node_id,
                service,
                call_id,
            } => PyValue::Tuple(vec![
                PyValue::Int(t),
                opt_int(node_id),
                svc(service),
                opt_int(call_id),
            ]),
            MachoAddress::Client {
                client_id,
                call_id,
                service,
            } => PyValue::Tuple(vec![
                PyValue::Int(t),
                PyValue::Int(*client_id),
                opt_int(call_id),
                svc(service),
            ]),
            MachoAddress::Broadcast {
                broadcast_id,
                narrowcast,
                id_type,
            } => PyValue::Tuple(vec![
                PyValue::Int(t),
                PyValue::Bytes(broadcast_id.as_bytes().to_vec()),
                PyValue::List(narrowcast.clone()),
                PyValue::Bytes(id_type.as_bytes().to_vec()),
            ]),
            MachoAddress::Any { service, call_id } => {
                PyValue::Tuple(vec![PyValue::Int(t), svc(service), opt_int(call_id)])
            }
        }
    }

    /// Parse a `__getstate__` tuple, switching on `state[0]` as the client does.
    pub fn from_state(v: &PyValue) -> Result<Self, ProtoError> {
        let items = match v {
            PyValue::Tuple(t) => t,
            _ => return Err(ProtoError::Shape("address state must be a tuple")),
        };
        let kind = as_int(
            items
                .first()
                .ok_or(ProtoError::Shape("empty address state"))?,
            "address type must be an integer",
        )?;
        match kind {
            1 => {
                if items.len() != 4 {
                    return Err(ProtoError::Shape("node address must be a 4-tuple"));
                }
                Ok(MachoAddress::Node {
                    node_id: as_opt_int(&items[1], "nodeID")?,
                    service: as_svc(&items[2])?,
                    call_id: as_opt_int(&items[3], "callID")?,
                })
            }
            2 => {
                if items.len() != 4 {
                    return Err(ProtoError::Shape("client address must be a 4-tuple"));
                }
                Ok(MachoAddress::Client {
                    client_id: as_int(&items[1], "clientID")?,
                    call_id: as_opt_int(&items[2], "callID")?,
                    service: as_svc(&items[3])?,
                })
            }
            4 => {
                if items.len() != 4 {
                    return Err(ProtoError::Shape("broadcast address must be a 4-tuple"));
                }
                let narrowcast = match &items[2] {
                    PyValue::List(l) => l.clone(),
                    PyValue::None => Vec::new(),
                    _ => return Err(ProtoError::Shape("narrowcast must be a list")),
                };
                Ok(MachoAddress::Broadcast {
                    broadcast_id: as_str(&items[1], "broadcastID")?,
                    narrowcast,
                    id_type: as_str(&items[3], "idtype")?,
                })
            }
            8 => {
                if items.len() != 3 {
                    return Err(ProtoError::Shape("any address must be a 3-tuple"));
                }
                Ok(MachoAddress::Any {
                    service: as_svc(&items[1])?,
                    call_id: as_opt_int(&items[2], "callID")?,
                })
            }
            _ => Err(ProtoError::Shape("unknown address type")),
        }
    }
}

/// Whether a marshalled class name names a `MachoAddress`, whatever module
/// it came from.
///
/// Dotted path, so the comparison is on everything after the last `.`, and a
/// name with no dot at all is compared whole.
fn is_macho_address(class: &[u8]) -> bool {
    let leaf = match class.iter().rposition(|b| *b == b'.') {
        Some(dot) => &class[dot + 1..],
        None => class,
    };
    leaf == b"MachoAddress"
}

#[cfg(test)]
mod tests {
    /// A `MachoAddress` from somebody else's module is still a
    /// `MachoAddress`.
    ///
    /// EvEJS marshals the class from `net.machoNetPacket` where this client
    /// uses `macho`, and an exact-path comparison rejected every packet that
    /// server sent -- each one parsing as "not a MachoPacket", which made
    /// `evecap --calls` report every call in a foreign capture as `HANG`.
    ///
    /// The property is "the leaf decides", asserted in both directions:
    /// another module's `MachoAddress` is accepted, and another class from
    /// our own module is not.
    #[test]
    fn the_class_is_matched_on_its_leaf_and_not_its_module() {
        assert!(super::is_macho_address(b"macho.MachoAddress"));
        assert!(super::is_macho_address(b"net.machoNetPacket.MachoAddress"));
        assert!(super::is_macho_address(b"MachoAddress"));
        assert!(!super::is_macho_address(b"macho.MachoPacket"));
        assert!(!super::is_macho_address(b"macho.machoaddress"));
        assert!(!super::is_macho_address(b""));
    }

    use super::*;

    fn rt(a: MachoAddress) {
        let s = a.to_state();
        assert_eq!(MachoAddress::from_state(&s).unwrap(), a);
    }

    #[test]
    fn every_variant_round_trips() {
        rt(MachoAddress::Node {
            node_id: Some(12),
            service: Some("machoNet".into()),
            call_id: Some(3),
        });
        rt(MachoAddress::Node {
            node_id: Some(12),
            service: None,
            call_id: Some(0),
        });
        // callID None, which is what the first real client sent as the
        // destination of its machoNet call.
        rt(MachoAddress::Node {
            node_id: Some(1),
            service: Some("machoNet".into()),
            call_id: None,
        });
        rt(MachoAddress::Client {
            client_id: 7,
            call_id: Some(9),
            service: None,
        });
        rt(MachoAddress::Any {
            service: Some("slash".into()),
            call_id: Some(1),
        });
        rt(MachoAddress::Broadcast {
            broadcast_id: "clientID".into(),
            narrowcast: vec![PyValue::Int(1)],
            id_type: "*corpid".into(),
        });
    }

    /// The client sends exactly one of these per session and we rejected it
    /// for four months of captures.
    ///
    /// `(1, None, None, None)` is the source address on the
    /// `ClientHasReleasedTheseObjects` notification: a node address from
    /// something that is not a node. Coercing nodeID to an integer turned the
    /// whole packet into `unparsed packet: packet shape: nodeID`, which reads
    /// like a marshal problem and is nothing of the sort.
    #[test]
    fn a_node_address_can_have_no_node_id() {
        let state = PyValue::Tuple(vec![
            PyValue::Int(1),
            PyValue::None,
            PyValue::None,
            PyValue::None,
        ]);
        assert_eq!(
            MachoAddress::from_state(&state).unwrap(),
            MachoAddress::Node {
                node_id: None,
                service: None,
                call_id: None,
            }
        );
        rt(MachoAddress::Node {
            node_id: None,
            service: None,
            call_id: None,
        });
    }

    #[test]
    fn client_puts_call_id_before_service() {
        // The one variant whose field order differs; getting this wrong would
        // silently swap two fields.
        let a = MachoAddress::Client {
            client_id: 7,
            call_id: Some(9),
            service: Some("x".into()),
        };
        let PyValue::Tuple(t) = a.to_state() else {
            panic!("not a tuple")
        };
        assert_eq!(t[0], PyValue::Int(2));
        assert_eq!(t[1], PyValue::Int(7)); // clientID
        assert_eq!(t[2], PyValue::Int(9)); // callID
        assert_eq!(t[3], PyValue::Bytes(b"x".to_vec())); // service last
    }

    #[test]
    fn any_is_a_three_tuple() {
        let a = MachoAddress::Any {
            service: None,
            call_id: Some(4),
        };
        let PyValue::Tuple(t) = a.to_state() else {
            panic!("not a tuple")
        };
        assert_eq!(t.len(), 3, "ANY addresses are 3-tuples, not 4");
    }

    #[test]
    fn wrong_arity_is_rejected() {
        let bad = PyValue::Tuple(vec![PyValue::Int(8), PyValue::None]);
        assert!(MachoAddress::from_state(&bad).is_err());
        let bad2 = PyValue::Tuple(vec![PyValue::Int(99), PyValue::None, PyValue::Int(0)]);
        assert!(MachoAddress::from_state(&bad2).is_err());
    }
}
