//! `macho.MachoPacket` and its subclasses.
//!
//! `MachoPacket.__getstate__` returns a 14-tuple in exactly this order
//! (`carbon/common/script/net/machoNetPacket.py`):
//!
//! ```text
//! 0  command         int, the MACHONETMSG_TYPE_* id
//! 1  source          MachoAddress state
//! 2  destination     MachoAddress state
//! 3  userID
//! 4  body            tuple built from the subclass's `params`
//! 5  oob             dict, or None when empty
//! 6  contextKey
//! 7  journeyID
//! 8  trace_id
//! 9  parent_span_id
//! 10 sampled
//! 11 ingress_id
//! 12 sample_rate
//! 13 cut_parent
//! ```
//!
//! `__setstate__` unpacks in the identical order. `body` is built from the
//! subclass's `params` list; a param whose name ends in `?` is optional and,
//! when the attribute is absent, is dropped from the **end** of `body` — so an
//! optional param must be last. `oob` is `None` on the wire unless
//! `compressedPart` is set, when it becomes `{'compressedPart': ...}`.

use eve_marshal::{MachoNetMsg, PyValue, WireError, Writer, save, write_empty_header};

use crate::address::MachoAddress;

/// Everything that can go wrong building or parsing a packet.
#[derive(Debug, Clone, PartialEq)]
pub enum ProtoError {
    /// The state did not have the shape the client's `__setstate__` expects.
    Shape(&'static str),
    /// The underlying marshal stream was bad.
    Wire(WireError),
    /// A command id this crate does not model.
    UnknownCommand(i64),
}

impl core::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProtoError::Shape(s) => write!(f, "packet shape: {s}"),
            ProtoError::Wire(e) => write!(f, "marshal: {e}"),
            ProtoError::UnknownCommand(c) => write!(f, "unknown machoNet command {c}"),
        }
    }
}

impl std::error::Error for ProtoError {}

/// The number of fields in a `MachoPacket` state tuple.
pub const STATE_FIELDS: usize = 14;

/// A machoNet packet.
///
/// The tracing fields (8..13) are carried opaquely: the client reads them but
/// we have no reason to populate them, and `None` is what an absent value
/// looks like.
#[derive(Debug, Clone, PartialEq)]
pub struct MachoPacket {
    pub command: MachoNetMsg,
    pub source: MachoAddress,
    pub destination: MachoAddress,
    pub user_id: Option<i64>,
    pub body: Vec<PyValue>,
    /// `None` encodes as `None`, which is what the client expects when there
    /// is no out-of-band data. Leaving this empty matters: a stray
    /// `sessionVersion` in `oob` makes the client's `LockSession` spin.
    pub oob: Option<Vec<(PyValue, PyValue)>>,
    pub context_key: Option<PyValue>,
    pub journey_id: Option<PyValue>,
    pub trace_id: Option<PyValue>,
    pub parent_span_id: Option<PyValue>,
    pub sampled: Option<PyValue>,
    pub ingress_id: Option<PyValue>,
    pub sample_rate: Option<PyValue>,
    pub cut_parent: Option<PyValue>,
}

fn opt(v: &Option<PyValue>) -> PyValue {
    v.clone().unwrap_or(PyValue::None)
}

fn some(v: &PyValue) -> Option<PyValue> {
    match v {
        PyValue::None => None,
        other => Some(other.clone()),
    }
}

impl MachoPacket {
    /// A packet with everything optional left empty.
    pub fn new(
        command: MachoNetMsg,
        source: MachoAddress,
        destination: MachoAddress,
        body: Vec<PyValue>,
    ) -> Self {
        MachoPacket {
            command,
            source,
            destination,
            user_id: None,
            body,
            oob: None,
            context_key: None,
            journey_id: None,
            trace_id: None,
            parent_span_id: None,
            sampled: None,
            ingress_id: None,
            sample_rate: None,
            cut_parent: None,
        }
    }

    /// A server-initiated `Notification` (command 12) with a **broadcast**
    /// destination -- the only way to reach a client-side notify event.
    ///
    /// Mirrors `CoreBroadcastStuff.ScattercastWithoutTheStars`, which is the
    /// method the real server calls:
    ///
    /// ```text
    /// def ScattercastWithoutTheStars(self, idtype, ids, method, args):
    ///     if not idtype.startswith('+'):
    ///         idtype = '*%s' % idtype
    ///     if ids:
    ///         self.ForwardNotifyDown(machoNetPacket.Notification(
    ///             destination=machoNetAddress.MachoAddress(
    ///                 broadcastID=method, idtype=idtype, narrowcast=ids),
    ///             payload=(1, args)))
    /// ```
    ///
    /// so `id_type` is passed in **with** its `*` already on it --
    /// `"*clientID"` -- because it is that method and not the transport that
    /// prepends it. `narrowcast` is a list.
    ///
    /// The payload is the client's GPCS chain written out. A packet enters at
    /// `channelHandlersUp[None]`, which `machoNet.py:826` sets to `nodes[0]` =
    /// `ExceptionWrapper`, and climbs:
    ///
    /// ```text
    /// ObjectCall.NotifyUp      payload[0] != 1 -> payload = FromPickle(payload[1])
    /// ServiceCall.NotifyUp     payload[0] == 0 -> payload = payload[1]
    /// BroadcastStuff.NotifyUp  payload[0]      -> dispatch(broadcastID, payload[1])
    /// ```
    ///
    /// Hence `(0, pickle((0, (1, args))))`, and the outer `0` is not optional:
    /// `ObjectCall.NotifyUp` branches on `payload[0] == 1` and would read the
    /// payload as an *object* notification, look the object id up in
    /// `session.machoObjectsByID`, and raise `UnMachoDestination`.
    ///
    /// The pickle is a byte string and not a substream, for the same reason as
    /// [`crate::call::pickled`]: the client hands it to `blue.marshal.Load`,
    /// and a substream tag has already materialised into an object by then.
    ///
    /// **`oob` stays empty, and a hang is what happens if it does not.**
    /// `machoNetTransport.LockSession` calls
    /// `WaitForSequenceNumber(packet.source, packet.oob.get('sn', 0))` for
    /// every even-numbered command, and that function returns early only `if
    /// not seq`. A non-zero `sn` parks the notification's tasklet in the
    /// client's `Sequencer` waiting for sequence numbers we will never send.
    /// [`MachoPacket::new`] leaves it `None`; nothing here sets it.
    ///
    /// **`source` must not be a broadcast or ANY address, and a disconnect is
    /// what happens if it is.** `machoNet.HandleMessage` closes the transport
    /// outright for any even-numbered command whose source is in `__aorb__`:
    /// *"A request must be sent from a specific address, not a forking one"* --
    /// "Connection Lost" on screen, with nothing naming us. A node address is
    /// what the real server sends, and this takes one.
    pub fn notification(
        source: MachoAddress,
        broadcast_id: &str,
        id_type: &str,
        narrowcast: Vec<PyValue>,
        args: Vec<PyValue>,
    ) -> Self {
        Self::notification_pickled(
            source,
            broadcast_id,
            id_type,
            narrowcast,
            Self::notification_body(args),
        )
    }

    /// The pickle a [`Self::notification`] carries: `pickle((0, (1,
    /// args)))`. **It names no recipient**, so a notification going to many
    /// connections is pickled once here and addressed by
    /// [`Self::notification_pickled`] for each -- the same bytes as
    /// [`Self::notification`] per recipient, without marshalling the
    /// arguments again (lane perf-fight).
    pub fn notification_body(args: Vec<PyValue>) -> PyValue {
        let inner = PyValue::Tuple(vec![
            PyValue::Int(0),
            PyValue::Tuple(vec![PyValue::Int(1), PyValue::Tuple(args)]),
        ]);
        crate::call::pickled(&inner)
    }

    /// [`Self::notification`] with its body already pickled by
    /// [`Self::notification_body`].
    pub fn notification_pickled(
        source: MachoAddress,
        broadcast_id: &str,
        id_type: &str,
        narrowcast: Vec<PyValue>,
        body: PyValue,
    ) -> Self {
        MachoPacket::new(
            MachoNetMsg::Notification,
            source,
            MachoAddress::Broadcast {
                broadcast_id: broadcast_id.to_owned(),
                narrowcast,
                id_type: id_type.to_owned(),
            },
            vec![PyValue::Tuple(vec![PyValue::Int(0), body])],
        )
    }

    /// A reply to `request`, built exactly as `MachoPacket.Response` builds one.
    ///
    /// Read off that method's bytecode, because every field it does *not* copy
    /// is as load-bearing as the ones it does:
    ///
    /// ```text
    /// theResponse.source         = self.destination   # NOT "our node address"
    /// theResponse.destination    = self.source
    /// theResponse.userID         = self.userID
    /// theResponse.contextKey     = None
    /// theResponse.trace_id       = self.trace_id
    /// theResponse.parent_span_id = None
    /// theResponse.ingress_id     = self.ingress_id
    /// theResponse.sampled        = self.sampled
    /// theResponse.sample_rate    = self.sample_rate
    /// theResponse.cut_parent     = None
    /// ```
    ///
    /// `journeyID` is not copied either, so it stays unset.
    ///
    /// The source is the subtle one. It is the address the request was *sent
    /// to*, echoed back verbatim — not an address we invent for ourselves.
    /// `machoNetTransport.LockSession` calls
    /// `WaitForSequenceNumber(packet.source, ...)` on even-numbered commands,
    /// so an invented source is an invented sequencing peer.
    pub fn respond_to(request: &MachoPacket, command: MachoNetMsg, body: Vec<PyValue>) -> Self {
        MachoPacket {
            command,
            source: request.destination.clone(),
            destination: request.source.clone(),
            user_id: request.user_id,
            body,
            oob: None,
            context_key: None,
            journey_id: None,
            trace_id: request.trace_id.clone(),
            parent_span_id: None,
            sampled: request.sampled.clone(),
            ingress_id: request.ingress_id.clone(),
            sample_rate: request.sample_rate.clone(),
            cut_parent: None,
        }
    }

    /// `SessionInitialStateNotification`, the packet that establishes the
    /// client's session. `params` are `['sid', 'sessionType', 'initialstate']`,
    /// and `initialstate` is a **flat** `{attr: value}` map.
    ///
    /// Send this with `source` a node address and `destination` the client's.
    pub fn session_initial_state(
        source: MachoAddress,
        destination: MachoAddress,
        sid: i64,
        session_type: i32,
        initial_state: Vec<(PyValue, PyValue)>,
    ) -> Self {
        MachoPacket::new(
            MachoNetMsg::SessionInitialStateNotification,
            source,
            destination,
            vec![
                PyValue::Int(sid),
                PyValue::Int(i64::from(session_type)),
                PyValue::Dict(initial_state),
            ],
        )
    }

    /// `SessionChangeNotification`; `params` are
    /// `['sid', 'change', 'nodesOfInterest?']`.
    ///
    /// `change` is `(0, {attr: (old, new)})`. Both of those are read straight
    /// off the server's own builder, `SessionChangeGPCS.SessionChanged`:
    ///
    /// ```python
    /// packet = machoNetPacket.SessionChangeNotification(
    ///     destination=machoNetAddress.MachoAddress(clientID=clientID),
    ///     sid=sid, change=(0, change), nodesOfInterest=nodesOfInterest)
    /// ```
    ///
    /// The leading `0` is not read by the client at all — `SessionChange.NotifyUp`
    /// uses only `packet.change[1]` — but it is what the real server sends, and
    /// the proxy's own node-to-node forwarding uses `1` in the same slot.
    ///
    /// **The `old` values are discarded.** `ComputeNotifySets` rebuilds every
    /// pair as `(the client's current value, new)` before any handler sees it,
    /// and the proxy's forwarding path fills the slot with a literal `None`.
    /// So `None` there is correct rather than lazy.
    ///
    /// `nodesOfInterest` is only read in proxy mode and through
    /// `getattr(packet, 'nodesOfInterest', None)`, so `None` is right for a
    /// client-bound change.
    pub fn session_change(
        source: MachoAddress,
        destination: MachoAddress,
        sid: i64,
        change: Vec<(PyValue, PyValue)>,
    ) -> Self {
        MachoPacket::new(
            MachoNetMsg::SessionChangeNotification,
            source,
            destination,
            vec![
                PyValue::Int(sid),
                PyValue::Tuple(vec![PyValue::Int(0), PyValue::Dict(change)]),
                PyValue::None,
            ],
        )
    }

    /// `CallRsp`; `params` are `['payload']`.
    pub fn call_rsp(source: MachoAddress, destination: MachoAddress, payload: PyValue) -> Self {
        MachoPacket::new(MachoNetMsg::CallRsp, source, destination, vec![payload])
    }

    /// `PingRsp`; `params` are `['times']`.
    pub fn ping_rsp(source: MachoAddress, destination: MachoAddress, times: PyValue) -> Self {
        MachoPacket::new(MachoNetMsg::PingRsp, source, destination, vec![times])
    }

    /// `ErrorResponse`; `params` are
    /// `['originalCommand', 'code', 'payload']`.
    pub fn error_response(
        source: MachoAddress,
        destination: MachoAddress,
        original_command: MachoNetMsg,
        code: i64,
        payload: PyValue,
    ) -> Self {
        MachoPacket::new(
            MachoNetMsg::ErrorResponse,
            source,
            destination,
            vec![
                PyValue::Int(i64::from(original_command as u8)),
                PyValue::Int(code),
                payload,
            ],
        )
    }

    /// The whitelisted class name this packet marshals under.
    ///
    /// A packet is not sent as `MachoPacket` — that base class is pointedly
    /// **absent** from `globalsWhitelist`, so a stream naming it is refused
    /// with `HACKER WARNING! object %s.%s is not in whitelist`. Only the
    /// eleven subclasses below are whitelisted, and the command id selects
    /// which. See `docs/reference/object-wrapper.md` for the full whitelist.
    ///
    /// Four command ids have no class: `AuthenticationReq`/`Rsp` and
    /// `ResolveReq`/`Rsp`. That is not an omission in our table — those
    /// exchanges happen inside the GPS handshake, which is not made of
    /// packets at all, so there is no class to name.
    pub fn class_name(&self) -> Result<&'static str, ProtoError> {
        use MachoNetMsg::*;
        Ok(match self.command {
            CallReq => "carbon.common.script.net.machoNetPacket.CallReq",
            CallRsp => "carbon.common.script.net.machoNetPacket.CallRsp",
            ErrorResponse => "carbon.common.script.net.machoNetPacket.ErrorResponse",
            IdentificationReq => "carbon.common.script.net.machoNetPacket.IdentificationReq",
            IdentificationRsp => "carbon.common.script.net.machoNetPacket.IdentificationRsp",
            Notification => "carbon.common.script.net.machoNetPacket.Notification",
            PingReq => "carbon.common.script.net.machoNetPacket.PingReq",
            PingRsp => "carbon.common.script.net.machoNetPacket.PingRsp",
            SessionChangeNotification => {
                "carbon.common.script.net.machoNetPacket.SessionChangeNotification"
            }
            SessionInitialStateNotification => {
                "carbon.common.script.net.machoNetPacket.SessionInitialStateNotification"
            }
            TransportClosed => "carbon.common.script.net.machoNetPacket.TransportClosed",
            AuthenticationReq | AuthenticationRsp | ResolveReq | ResolveRsp => {
                return Err(ProtoError::Shape(
                    "this command has no machoNetPacket class and cannot be sent as a packet",
                ));
            }
        })
    }

    /// The marshalled object: a `0x17` instance wrapping the state tuple.
    pub fn to_value(&self) -> Result<PyValue, ProtoError> {
        Ok(PyValue::Instance {
            class: self.class_name()?.as_bytes().to_vec(),
            state: Box::new(self.to_state()),
        })
    }

    /// Parse a `0x17`-wrapped packet, or a bare state tuple.
    pub fn from_value(v: &PyValue) -> Result<Self, ProtoError> {
        match v {
            PyValue::Instance { class, state } => {
                let packet = Self::from_state(state)?;
                // The class name and the command id are two statements of the
                // same fact. If they disagree, one of them is wrong and we
                // would rather know than pick one.
                let expected = packet.class_name()?;
                if class.as_slice() != expected.as_bytes() {
                    return Err(ProtoError::Shape(
                        "packet class name does not match its command id",
                    ));
                }
                Ok(packet)
            }
            other => Self::from_state(other),
        }
    }

    /// The complete wire payload: a marshal stream carrying this packet.
    ///
    /// This is the whole point of the crate, and until the object wrapper was
    /// recovered it could not be written. A stream header with no shared
    /// objects, then one `0x17` instance.
    pub fn to_payload(&self) -> Result<Vec<u8>, ProtoError> {
        let value = self.to_value()?;
        let mut w = Writer::new();
        write_empty_header(&mut w);
        save(&value, &mut w);
        Ok(w.into_bytes())
    }

    /// The `__getstate__` 14-tuple.
    pub fn to_state(&self) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.command as u8)),
            self.source.to_value(),
            self.destination.to_value(),
            self.user_id.map(PyValue::Int).unwrap_or(PyValue::None),
            PyValue::Tuple(self.body.clone()),
            match &self.oob {
                Some(d) if !d.is_empty() => PyValue::Dict(d.clone()),
                _ => PyValue::None,
            },
            opt(&self.context_key),
            opt(&self.journey_id),
            opt(&self.trace_id),
            opt(&self.parent_span_id),
            opt(&self.sampled),
            opt(&self.ingress_id),
            opt(&self.sample_rate),
            opt(&self.cut_parent),
        ])
    }

    /// Parse a `__getstate__` 14-tuple.
    pub fn from_state(v: &PyValue) -> Result<Self, ProtoError> {
        let t = match v {
            PyValue::Tuple(t) if t.len() == STATE_FIELDS => t,
            PyValue::Tuple(_) => return Err(ProtoError::Shape("packet state must have 14 fields")),
            _ => return Err(ProtoError::Shape("packet state must be a tuple")),
        };
        let cmd = match &t[0] {
            PyValue::Int(i) => *i,
            _ => return Err(ProtoError::Shape("command must be an integer")),
        };
        let command =
            MachoNetMsg::from_u8(u8::try_from(cmd).map_err(|_| ProtoError::UnknownCommand(cmd))?)
                .ok_or(ProtoError::UnknownCommand(cmd))?;
        let body = match &t[4] {
            PyValue::Tuple(b) => b.clone(),
            PyValue::List(b) => b.clone(),
            PyValue::None => Vec::new(),
            _ => return Err(ProtoError::Shape("body must be a tuple")),
        };
        let oob = match &t[5] {
            PyValue::Dict(d) => Some(d.clone()),
            PyValue::None => None,
            _ => return Err(ProtoError::Shape("oob must be a dict or None")),
        };
        Ok(MachoPacket {
            command,
            source: MachoAddress::from_value(&t[1])?,
            destination: MachoAddress::from_value(&t[2])?,
            user_id: match &t[3] {
                PyValue::Int(i) => Some(*i),
                PyValue::None => None,
                _ => return Err(ProtoError::Shape("userID must be an integer or None")),
            },
            body,
            oob,
            context_key: some(&t[6]),
            journey_id: some(&t[7]),
            trace_id: some(&t[8]),
            parent_span_id: some(&t[9]),
            sampled: some(&t[10]),
            ingress_id: some(&t[11]),
            sample_rate: some(&t[12]),
            cut_parent: some(&t[13]),
        })
    }
}

#[cfg(test)]
mod tests {
    // --- the complete packet (docs/reference/object-wrapper.md) ---------------------

    #[test]
    fn a_packet_marshals_as_an_old_style_instance_named_by_its_command() {
        let p = MachoPacket::call_rsp(src(), dst(), PyValue::None);
        let PyValue::Instance { class, state } = p.to_value().unwrap() else {
            panic!("a packet must marshal as a 0x17 instance")
        };
        assert_eq!(
            String::from_utf8(class).unwrap(),
            "carbon.common.script.net.machoNetPacket.CallRsp"
        );
        let PyValue::Tuple(t) = *state else {
            panic!("state is not a tuple")
        };
        assert_eq!(t.len(), STATE_FIELDS);
    }

    /// **One pickle, many recipients, the same bytes** (lane perf-fight):
    /// a body pickled once by `notification_body` and addressed by
    /// `notification_pickled` encodes, for each client, exactly as the
    /// packet `notification` built before the split -- whose construction
    /// is spelled out here, so a change to either path shows.
    #[test]
    fn a_shared_body_encodes_byte_for_byte_as_a_notification_per_recipient() {
        let args = vec![
            PyValue::Int(7),
            PyValue::List(vec![PyValue::Real(1.5), PyValue::Bytes(b"x".to_vec())]),
            PyValue::None,
        ];
        let body = MachoPacket::notification_body(args.clone());
        for client in [11_i64, 12, 1_000_000_007] {
            let before = MachoPacket::new(
                MachoNetMsg::Notification,
                MachoAddress::node(3),
                MachoAddress::Broadcast {
                    broadcast_id: "OnSpecialFX".to_owned(),
                    narrowcast: vec![PyValue::Int(client)],
                    id_type: "*clientID".to_owned(),
                },
                vec![PyValue::Tuple(vec![
                    PyValue::Int(0),
                    crate::call::pickled(&PyValue::Tuple(vec![
                        PyValue::Int(0),
                        PyValue::Tuple(vec![PyValue::Int(1), PyValue::Tuple(args.clone())]),
                    ])),
                ])],
            )
            .to_payload()
            .unwrap();
            let shared = MachoPacket::notification_pickled(
                MachoAddress::node(3),
                "OnSpecialFX",
                "*clientID",
                vec![PyValue::Int(client)],
                body.clone(),
            )
            .to_payload()
            .unwrap();
            let whole = MachoPacket::notification(
                MachoAddress::node(3),
                "OnSpecialFX",
                "*clientID",
                vec![PyValue::Int(client)],
                args.clone(),
            )
            .to_payload()
            .unwrap();
            assert_eq!(shared, before, "client {client}");
            assert_eq!(whole, before, "client {client}");
        }
    }

    #[test]
    fn the_base_class_is_never_named() {
        // MachoPacket is deliberately absent from globalsWhitelist; naming it
        // is "HACKER WARNING! object %s.%s is not in whitelist".
        for p in [
            MachoPacket::call_rsp(src(), dst(), PyValue::None),
            MachoPacket::ping_rsp(src(), dst(), PyValue::None),
        ] {
            let name = p.class_name().unwrap();
            assert!(name.starts_with("carbon.common.script.net.machoNetPacket."));
            assert_ne!(name, "carbon.common.script.net.machoNetPacket.MachoPacket");
        }
    }

    #[test]
    fn a_command_with_no_packet_class_is_refused_rather_than_guessed() {
        let mut p = MachoPacket::call_rsp(src(), dst(), PyValue::None);
        for cmd in [
            MachoNetMsg::AuthenticationReq,
            MachoNetMsg::AuthenticationRsp,
            MachoNetMsg::ResolveReq,
            MachoNetMsg::ResolveRsp,
        ] {
            p.command = cmd;
            assert!(p.class_name().is_err(), "{cmd:?} should have no class");
            assert!(p.to_payload().is_err());
        }
    }

    #[test]
    fn the_addresses_are_wrapped_instances_too() {
        let p = MachoPacket::call_rsp(src(), dst(), PyValue::None);
        let PyValue::Tuple(t) = p.to_state() else {
            panic!("not a tuple")
        };
        for (i, field) in [(1usize, &t[1]), (2, &t[2])] {
            let PyValue::Instance { class, .. } = field else {
                panic!("field {i} is not an instance: {field:?}")
            };
            assert_eq!(
                String::from_utf8(class.clone()).unwrap(),
                MachoAddress::CLASS
            );
        }
    }

    #[test]
    fn a_packet_round_trips_through_a_real_marshal_stream() {
        // The end-to-end claim: build a packet, marshal it to bytes, decode
        // those bytes with the codec, and get the same packet back. Before
        // the object wrapper was recovered this could not be written at all.
        let p = MachoPacket::session_initial_state(
            src(),
            dst(),
            42,
            5,
            vec![(PyValue::Int(7), PyValue::Bytes(b"userid".to_vec()))],
        );
        let bytes = p.to_payload().unwrap();

        let mut r = eve_marshal::Reader::new(&bytes);
        let header = eve_marshal::take_header(&mut r).unwrap();
        assert_eq!(header.mapcount, 0, "we emit no shared objects");
        r.bytes(header.map_table_len()).unwrap();
        let value = eve_marshal::Decoder::new().load(&mut r).unwrap();
        assert_eq!(r.remaining(), 0, "the stream must be exactly one object");

        let back = MachoPacket::from_value(&value).unwrap();
        assert_eq!(back, p);
    }

    /// Walk the client's own GPCS chain over a `Notification` we built, by
    /// hand, and recover the event name and the args at the far end.
    ///
    /// **This test is the §2.2 table of `docs/reference/push-path.md` executed.** Each
    /// step below is one `NotifyUp` in the live chain, in the order
    /// `machoNet.py:826` wires them, and the assertions are that layer's own
    /// branch condition. If any of the three nesting levels is wrong the
    /// client raises inside `ObjectCall.NotifyUp` (`UnMachoDestination`) or
    /// silently drops the event, and we would see neither.
    #[test]
    fn a_notification_climbs_the_gpcs_chain() {
        let p = MachoPacket::notification(
            MachoAddress::node(1),
            "OnAccountChange",
            "*clientID",
            vec![PyValue::Int(42)],
            vec![PyValue::Bytes(b"cash".to_vec()), PyValue::Real(7.5)],
        );

        // The destination carries the event name and the narrowcast list.
        let MachoAddress::Broadcast {
            broadcast_id,
            narrowcast,
            id_type,
        } = &p.destination
        else {
            panic!("a notification must be addressed to a broadcast")
        };
        assert_eq!(broadcast_id, "OnAccountChange");
        assert_eq!(id_type, "*clientID", "the star is literal on the wire");
        assert_eq!(narrowcast, &vec![PyValue::Int(42)]);

        // `body` is a 1-tuple: command 12's params are `['payload?']`.
        assert_eq!(p.body.len(), 1, "Notification carries one body field");
        let PyValue::Tuple(outer) = &p.body[0] else {
            panic!("the payload is a tuple")
        };

        // ObjectCall.NotifyUp: `if packet.payload[0] == 1` -> object call;
        // else `FromPickle(payload[1])`. Ours must take the else branch.
        assert_eq!(outer[0], PyValue::Int(0), "0 takes the FromPickle branch");
        let PyValue::Bytes(pickle) = &outer[1] else {
            panic!("the inner payload is a byte string, not a substream")
        };
        let mut r = eve_marshal::Reader::new(pickle);
        let header = eve_marshal::take_header(&mut r).unwrap();
        r.bytes(header.map_table_len()).unwrap();
        let inner = eve_marshal::Decoder::new().load(&mut r).unwrap();

        // ServiceCall.NotifyUp: `if packet.payload[0]` -> service call;
        // else `payload = payload[1]`.
        let PyValue::Tuple(after_object) = inner else {
            panic!("the pickle is a tuple")
        };
        assert_eq!(after_object[0], PyValue::Int(0), "0 takes the else branch");

        // BroadcastStuff.NotifyUp: `if packet.payload[0]` -> dispatch
        // `broadcastID` with `payload[1]`. Here the leading 1 is required.
        let PyValue::Tuple(after_service) = &after_object[1] else {
            panic!("payload[1] is a tuple")
        };
        assert_eq!(
            after_service[0],
            PyValue::Int(1),
            "1 is what makes BroadcastStuff dispatch at all"
        );
        assert_eq!(
            after_service[1],
            PyValue::Tuple(vec![PyValue::Bytes(b"cash".to_vec()), PyValue::Real(7.5)]),
            "the args reach the handler unchanged"
        );
    }

    /// **A non-zero `sn` is a hang, so assert there is no `oob` at all.**
    /// `machoNetTransport.LockSession` calls `WaitForSequenceNumber(source,
    /// oob.get('sn', 0))` for every even-numbered command -- 12 is even -- and
    /// that function returns early only `if not seq`. A sequence number we
    /// invent parks the notification's tasklet in the client's `Sequencer`
    /// waiting for numbers we will never send: no traceback, no error, a
    /// client that simply never sees this event or any after it.
    #[test]
    fn a_notification_carries_no_sequence_number() {
        let p =
            MachoPacket::notification(MachoAddress::node(1), "OnX", "*clientID", vec![], vec![]);
        assert!(p.oob.is_none(), "oob must be absent, not merely empty");
    }

    /// **A forking source address is a disconnect, so assert the source is a
    /// node.** `machoNet.HandleMessage` closes the transport outright for any
    /// even-numbered command whose source type is in `__aorb__` (ANY or
    /// BROADCAST): *"A request must be sent from a specific address, not a
    /// forking one"*. The player sees "Connection Lost" and nothing names us.
    #[test]
    fn a_notification_source_is_not_a_forking_address() {
        let p =
            MachoPacket::notification(MachoAddress::node(1), "OnX", "*clientID", vec![], vec![]);
        assert_eq!(
            p.source.address_type(),
            eve_marshal::AddressType::Node,
            "__aorb__ would close the transport"
        );
    }

    #[test]
    fn a_class_name_disagreeing_with_the_command_is_refused() {
        let p = MachoPacket::call_rsp(src(), dst(), PyValue::None);
        let wrong = PyValue::Instance {
            class: b"carbon.common.script.net.machoNetPacket.PingRsp".to_vec(),
            state: Box::new(p.to_state()),
        };
        assert!(MachoPacket::from_value(&wrong).is_err());
    }

    #[test]
    fn the_payload_is_a_header_then_one_instance_tag() {
        let p = MachoPacket::ping_rsp(src(), dst(), PyValue::None);
        let bytes = p.to_payload().unwrap();
        assert_eq!(&bytes[..5], &[0x7E, 0, 0, 0, 0], "empty stream header");
        assert_eq!(bytes[5], 0x17, "then the old-style-instance tag");
    }

    use super::*;

    fn src() -> MachoAddress {
        MachoAddress::node(42)
    }
    fn dst() -> MachoAddress {
        MachoAddress::client(7)
    }

    #[test]
    fn state_has_exactly_fourteen_fields_in_order() {
        let p = MachoPacket::ping_rsp(src(), dst(), PyValue::Tuple(vec![]));
        let PyValue::Tuple(t) = p.to_state() else {
            panic!()
        };
        assert_eq!(t.len(), STATE_FIELDS);
        // field 0 is the command id, 4 is the body, 5 is oob
        assert_eq!(t[0], PyValue::Int(21)); // PingRsp
        assert_eq!(t[4], PyValue::Tuple(vec![PyValue::Tuple(vec![])]));
        assert_eq!(t[5], PyValue::None);
    }

    #[test]
    fn packets_round_trip() {
        let p = MachoPacket::session_initial_state(
            src(),
            dst(),
            12345,
            5,
            vec![
                (PyValue::Bytes(b"userid".to_vec()), PyValue::Int(99)),
                (PyValue::Bytes(b"role".to_vec()), PyValue::Int(1)),
            ],
        );
        let got = MachoPacket::from_state(&p.to_state()).unwrap();
        assert_eq!(got, p);
    }

    #[test]
    fn empty_oob_encodes_as_none_not_an_empty_dict() {
        // A stray oob entry makes the client's LockSession spin, so an empty
        // oob must serialise as None.
        let mut p = MachoPacket::call_rsp(src(), dst(), PyValue::None);
        p.oob = Some(vec![]);
        let PyValue::Tuple(t) = p.to_state() else {
            panic!()
        };
        assert_eq!(t[5], PyValue::None);
    }

    #[test]
    fn session_initial_state_carries_session_type_five() {
        let p = MachoPacket::session_initial_state(src(), dst(), 1, 5, vec![]);
        assert_eq!(p.command, MachoNetMsg::SessionInitialStateNotification);
        assert_eq!(p.body[1], PyValue::Int(5));
        assert_eq!(p.body.len(), 3, "params are sid, sessionType, initialstate");
    }

    #[test]
    fn wrong_field_count_is_rejected() {
        let short = PyValue::Tuple(vec![PyValue::Int(21); 13]);
        assert_eq!(
            MachoPacket::from_state(&short),
            Err(ProtoError::Shape("packet state must have 14 fields"))
        );
    }

    #[test]
    fn unknown_command_is_rejected() {
        let mut t = vec![PyValue::None; STATE_FIELDS];
        t[0] = PyValue::Int(99);
        t[1] = src().to_state();
        t[2] = dst().to_state();
        t[4] = PyValue::Tuple(vec![]);
        assert_eq!(
            MachoPacket::from_state(&PyValue::Tuple(t)),
            Err(ProtoError::UnknownCommand(99))
        );
    }

    #[test]
    fn error_response_body_is_command_code_payload() {
        let p = MachoPacket::error_response(
            src(),
            dst(),
            MachoNetMsg::CallReq,
            7,
            PyValue::Bytes(b"nope".to_vec()),
        );
        assert_eq!(p.body.len(), 3);
        assert_eq!(p.body[0], PyValue::Int(6)); // CallReq
        assert_eq!(p.body[1], PyValue::Int(7));
    }
}
