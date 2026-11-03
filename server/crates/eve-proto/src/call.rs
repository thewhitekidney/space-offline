//! The `CallReq` payload: how a service call and a bound-object call are
//! distinguished and carried.
//!
//! From `ServiceCallGPCS.CallUp` and `ObjectCallGPCS.CallUp`, the payload is a
//! 2-tuple `(kind, pickle)`:
//!
//! - `kind` falsy  -> service call; the pickle unmarshals to
//!   `(1, method, args, kwargs)` and the packet's destination carries the
//!   service name.
//! - `kind` truthy -> bound-object call; the pickle unmarshals to
//!   `(objectID, method, args, kwargs)` and the destination is a node address
//!   with no service set.
//!
//! The second element is a complete independent marshal stream — header and
//! all — because `ToPickle` is `blue.marshal.Save(value, GetObjectID)`. That is
//! why this module can encode and decode it end to end with no unknowns.
//!
//! **How that stream is carried** was settled by the first real client rather
//! than by reading the Python: it arrives under the substream tag `0x2B`, not
//! as a byte string. `blue.marshal.Save` produces a `str` in Python, so a
//! string tag is the reading the source suggests, and the marshal writer's
//! actual choice is the substream form. Both are decoded here — the client's
//! form because it is what happens, the string form because it is what
//! `ToPickle` literally returns and costs one match arm to keep accepting —
//! but we **emit** the substream form, so anything we send has the shape the
//! client itself produces.
//!
//! This is also what `ServiceCallGPCS.CallUp` reads first:
//!
//! ```text
//! if not packet.payload[0]:          # a service call
//!     packet.payload = packet.payload[1]   # materialise the substream
//!     return self.ForwardCallUp(packet)    # re-enter with (1, method, ...)
//! ```
//!
//! so the outer `0` and the inner leading `1` are two different discriminators
//! one level apart, which is easy to conflate and worth stating plainly.

use eve_marshal::{
    Decoder, ObjectForm, PyValue, Reader, Writer, call as callc, save, take_stream,
    write_empty_header,
};

use crate::packet::ProtoError;

/// A decoded `CallReq` payload.
#[derive(Debug, Clone, PartialEq)]
pub enum CallPayload {
    Service {
        method: String,
        args: Vec<PyValue>,
        kwargs: Vec<(PyValue, PyValue)>,
    },
    BoundObject {
        object_id: String,
        method: String,
        args: Vec<PyValue>,
        kwargs: Vec<(PyValue, PyValue)>,
    },
    /// `ClientHasReleasedTheseObjects` — the one bound-object payload whose
    /// first element is **not** an objectID.
    ///
    /// `ObjectCall.__ReleaseObjectHelper` batches the references a client has
    /// dropped and sends them every 30 s as a `Notification`, not a `CallReq`,
    /// so nothing is expected back. Its payload keeps the bound-object shape
    /// — `(what, method, args, kwargs)` with kind 1 — but `what` is the whole
    /// `[(objectID, refID), ...]` list rather than a single id, because the
    /// notification is about many objects at once.
    ///
    /// `docs/reference/bound-objects.md` §1.5 records this; it is called out as a
    /// special case there, and it is a special case here too, which is why it
    /// gets its own variant instead of a `String` that would have to lie.
    Release { objects: Vec<(String, i64)> },
}

/// Marshal a value into its own complete stream, as `ToPickle` does.
fn to_pickle(v: &PyValue) -> Vec<u8> {
    let mut w = Writer::new();
    write_empty_header(&mut w);
    save(v, &mut w);
    w.into_bytes()
}

/// Marshal a return value the way a `CallRsp` must carry it.
///
/// **A response payload is a pickle, not the value itself**, and that
/// asymmetry with the request is easy to get wrong — we got it wrong, and the
/// client's own traceback is what settled it:
///
/// ```text
/// TypeError: unhashable type
///   ObjectCallGPCS.py(239) CallDown
///   ObjectCallGPCS.py(495) FromPickle
///     thePickle = {}
/// ```
///
/// `ObjectCall.CallDown` does
///
/// ```text
/// retval = self.ForwardCallDown(packet)
/// retval.payload = self.FromPickle(retval.payload, packet.destination)
/// ```
///
/// and `FromPickle` is `blue.marshal.Load(thePickle, self.ParseObjectID)`. So
/// the client hands the response payload straight to `marshal.Load`, which
/// wants a string. We were sending the live value — an empty dict for
/// `GetServiceInfo`, `(None, None)` for `alert.BeanCount` — and `marshal.Load`
/// raised on both ("unhashable type" and "argument must be a string or
/// buffer" respectively).
///
/// A **request** is the other way round: its payload is `(0, substream)` and
/// `ServiceCallGPCS.CallUp` uses `payload[1]` as the call tuple directly,
/// without loading it. The asymmetry is real, and it has a reason: a response
/// is loaded explicitly so it can pass `ParseObjectID`, the callback that
/// turns bound-object references in a return value back into objects. A
/// request has no such references to resolve.
///
/// A plain byte string, not a substream: the substream tag materialises into
/// an object when the stream is decoded, which would land us back on
/// `marshal.Load(<an object>)`.
pub fn response_payload(v: &PyValue) -> PyValue {
    pickled(v)
}

/// A value marshalled into the byte string the client will hand to
/// `blue.marshal.Load`.
///
/// Two callers, one shape: a call response (above), and the inner pickle of a
/// `Notification` (`MachoPacket::notification`). Both land in
/// `ObjectCall.FromPickle`, which is `blue.marshal.Load(thePickle,
/// self.ParseObjectID)` -- so both want a plain byte string and neither wants
/// a substream, for the reason spelled out above.
pub fn pickled(v: &PyValue) -> PyValue {
    PyValue::Bytes(to_pickle(v))
}

/// A `UserError` the client will **raise**, as a marshalled exception.
///
/// Some calls are specified by what they throw rather than what they return,
/// and the project's "always answer with a value" default is wrong for them.
/// The one that forced this was `slash.SlashCmd('/')`: the client asks for the
/// server's command list once, and reads the answer out of the *exception*:
///
/// ```python
/// try:
///     sm.RemoteSvc('slash').SlashCmd('/')
/// except UserError as e:
///     msg = e.args[1]['reason']
///     cmds = eval(msg.split(': ')[1])
/// self.remoteCommandList = cmds          # <- not assigned on the happy path
/// ```
///
/// `cmds` is bound only inside the handler, so returning normally is an
/// `UnboundLocalError` and **every** slash command fails, including the ones
/// that never reach the server.
///
/// `0x22` is the right tag, not the `0x17` our other instances use:
/// `UserError` derives from `Exception`, which is new-style in Python 2.7, so
/// `PyInstance_NewRaw` does not apply. `0x22` reconstructs as
/// `header[0](*header[1])` — it *calls* the class — and `UserError.__init__`
/// has exactly the signature we want:
///
/// ```python
/// def __init__(self, msg=None, *args):
///     ...
///     if len(args) and type(args[0]) == type({}):
///         self.dict = args[0]
///         self.args = (self.msg, self.dict)
/// ```
///
/// so `UserError(msg, dict)` populates `.msg`, `.dict` and `.args` itself and
/// we send no state at all. `eveexceptions.UserError` is in the client's
/// `globalsWhitelist`, which the `0x02` global handler checks.
pub fn user_error(msg: &str, dict: Vec<(PyValue, PyValue)>) -> PyValue {
    PyValue::ObjectEx {
        form: ObjectForm::Call,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Global(b"eveexceptions.UserError".to_vec()),
            PyValue::Tuple(vec![
                PyValue::Bytes(msg.as_bytes().to_vec()),
                PyValue::Dict(dict),
            ]),
        ])),
        list: Vec::new(),
        dict: Vec::new(),
    }
}

/// The payload of a `MACHONETERR_WRAPPEDEXCEPTION` `ErrorResponse`.
///
/// `ExceptionWrapper.CallUp` builds it as
/// `packet.ErrorResponse(MACHONETERR_WRAPPEDEXCEPTION, (DumpsSanitized(e),))`
/// — a **1-tuple** whose single element is a nested marshal stream, which the
/// receiving side unwraps with
/// `newException = machobase.LoadsSanitized(e.payload[0]); raise newException`.
///
/// Keep it to one element. With `len(payload) > 1` the client treats elements
/// 1..6 as a server stack trace, formats a full cross-the-wire report and
/// sends an alert — none of which we have anything to put in.
pub fn wrapped_exception_payload(exception: &PyValue) -> PyValue {
    PyValue::Tuple(vec![PyValue::Bytes(to_pickle(exception))])
}

/// A **bound-object reference**, as the marshal Save callback emits one.
///
/// This is what the client turns back into a usable object. The chain, all of
/// it confirmed from `carbon/common/script/net/ObjectCallGPCS.pyc`:
///
/// - we emit tag `0x19` wrapping a byte string (`PyValue::Callback`);
/// - loading a `CallRsp` payload, the client passes `ParseObjectID` as the
///   stream's Load callback, so `0x19` calls it with that byte string;
/// - `ParseObjectID` does `blue.marshal.Load(objectID, self.ParseObjectID)`
///   on it and accepts either `(objectID, refID)` or
///   `(objectID, publicAttributes, refID)`;
/// - it then builds a `MachoObjectConnection` for the result.
///
/// Two details in `ParseObjectID` constrain `object_id` and are easy to get
/// wrong:
///
/// 1. It does `nid, oid = objectID.split(':')`, so the id must contain
///    **exactly one** colon.
/// 2. It compares `nid` against the connection's own `identifier` and
///    **raises** `RuntimeError('The specified object does not exist within
///    this session')` when they match. On a client that identifier is the
///    literal `'C=0'`, so a server-side id of the form `'N=<nodeID>:<n>'` is
///    both correct and safely distinct.
///
/// `MachoObjectConnection.__init__` then reads the node back out of the id --
/// `nid.split('=')[1]`, `MachoAddress(nodeID=long(nid))` -- and addresses
/// every later call on the object to that node. So the number in the id has
/// to be a node we actually answer as.
///
/// `ref_id` is a reference-count token; the client only echoes it back when
/// releasing the object (`OID-`). The real server uses a wallclock reading.
pub fn object_reference(object_id: &str, ref_id: i64) -> PyValue {
    debug_assert_eq!(
        object_id.matches(':').count(),
        1,
        "ParseObjectID splits on exactly one colon"
    );
    PyValue::Callback(Box::new(PyValue::Bytes(to_pickle(&PyValue::Tuple(vec![
        PyValue::Bytes(object_id.as_bytes().to_vec()),
        PyValue::Int(ref_id),
    ])))))
}

/// Unmarshal a complete stream, as `FromPickle` does.
///
/// The shared-object id table has to be consumed and handed to the decoder.
/// This function used to do neither, which was invisible until a real client
/// sent a call payload big enough to reuse a string: a 295-byte `eventLog`
/// call failed at its first `0x1B` back-reference. A nested stream has its
/// own header and its own table -- they are not inherited from the outer one.
fn from_pickle(bytes: &[u8]) -> Result<PyValue, ProtoError> {
    let stream = take_stream(bytes).map_err(ProtoError::Wire)?;
    let mut r = Reader::new(stream.body);
    // Only the server parses a `CallReq`, so this is inbound: budgeted like
    // the outer payload (`docs/review/V2-wire.md` F2), or a fan-out hidden in
    // the inner stream would build what the outer one may not.
    let v = Decoder::with_shared(stream.shared_ids)
        .with_budget(eve_marshal::value::Budget::INBOUND)
        .load(&mut r)
        .map_err(ProtoError::Wire)?;
    Ok(v)
}

fn text(v: &PyValue, what: &'static str) -> Result<String, ProtoError> {
    match v {
        PyValue::Bytes(b) => Ok(String::from_utf8_lossy(b).into_owned()),
        PyValue::Unicode(s) => Ok(s.clone()),
        _ => Err(ProtoError::Shape(what)),
    }
}

/// The method name on the release notification, as a literal in
/// `ObjectCall.__ReleaseObjectHelper`.
pub const RELEASE_METHOD: &str = "ClientHasReleasedTheseObjects";

/// The `[(objectID, refID), ...]` list a release notification carries.
///
/// Lenient about the container -- tuple or list -- for the same reason
/// `from_payload` is lenient about args: Python does not distinguish them
/// here, and rejecting one of the two would be our rule, not the client's.
fn released_objects(v: &PyValue) -> Result<Vec<(String, i64)>, ProtoError> {
    let items = match v {
        PyValue::List(l) | PyValue::Tuple(l) => l,
        _ => return Err(ProtoError::Shape("released objects must be a list")),
    };
    items
        .iter()
        .map(|pair| match pair {
            PyValue::Tuple(p) | PyValue::List(p) if p.len() == 2 => {
                let id = text(&p[0], "released objectID must be a string")?;
                match &p[1] {
                    PyValue::Int(r) => Ok((id, *r)),
                    _ => Err(ProtoError::Shape("released refID must be an integer")),
                }
            }
            _ => Err(ProtoError::Shape("released object must be a 2-tuple")),
        })
        .collect()
}

impl CallPayload {
    /// The inner call tuple, before it is pickled.
    fn inner(&self) -> PyValue {
        match self {
            CallPayload::Service {
                method,
                args,
                kwargs,
            } => PyValue::Tuple(vec![
                // The leading 1 is what ServiceCallGPCS skips past before
                // reading payload[1] as the method name.
                PyValue::Int(1),
                PyValue::Bytes(method.as_bytes().to_vec()),
                PyValue::Tuple(args.clone()),
                PyValue::Dict(kwargs.clone()),
            ]),
            CallPayload::BoundObject {
                object_id,
                method,
                args,
                kwargs,
            } => PyValue::Tuple(vec![
                PyValue::Bytes(object_id.as_bytes().to_vec()),
                PyValue::Bytes(method.as_bytes().to_vec()),
                PyValue::Tuple(args.clone()),
                PyValue::Dict(kwargs.clone()),
            ]),
            CallPayload::Release { objects } => PyValue::Tuple(vec![
                PyValue::List(
                    objects
                        .iter()
                        .map(|(id, ref_id)| {
                            PyValue::Tuple(vec![
                                PyValue::Bytes(id.as_bytes().to_vec()),
                                PyValue::Int(*ref_id),
                            ])
                        })
                        .collect(),
                ),
                PyValue::Bytes(RELEASE_METHOD.as_bytes().to_vec()),
                PyValue::Tuple(Vec::new()),
                PyValue::Dict(Vec::new()),
            ]),
        }
    }

    fn kind(&self) -> i64 {
        match self {
            CallPayload::Service { .. } => i64::from(callc::KIND_SERVICE),
            CallPayload::BoundObject { .. } | CallPayload::Release { .. } => {
                i64::from(callc::KIND_BOUND_OBJECT)
            }
        }
    }

    /// The `(kind, pickle)` payload tuple, in the form the client emits.
    pub fn to_payload(&self) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(self.kind()),
            PyValue::SubStream(to_pickle(&self.inner())),
        ])
    }

    /// Parse a `(kind, pickle)` payload tuple.
    pub fn from_payload(v: &PyValue) -> Result<Self, ProtoError> {
        let items = match v {
            PyValue::Tuple(t) if t.len() == 2 => t,
            _ => return Err(ProtoError::Shape("call payload must be a 2-tuple")),
        };
        let kind = match &items[0] {
            PyValue::Int(i) => *i,
            PyValue::None => 0,
            _ => return Err(ProtoError::Shape("call payload kind must be an integer")),
        };
        // The substream form is what the real client sends; the byte-string
        // form is what `ToPickle` returns in Python. Either way the bytes are
        // a complete marshal stream, so both take the same path from here.
        let blob = match &items[1] {
            PyValue::SubStream(b) | PyValue::Bytes(b) => b,
            _ => {
                return Err(ProtoError::Shape(
                    "call payload pickle must be a substream or a byte string",
                ));
            }
        };
        let inner = from_pickle(blob)?;
        let t = match &inner {
            PyValue::Tuple(t) if t.len() >= 3 => t,
            _ => {
                return Err(ProtoError::Shape(
                    "inner call must be a tuple of at least 3",
                ));
            }
        };
        let args = match &t[2] {
            PyValue::Tuple(a) => a.clone(),
            PyValue::List(a) => a.clone(),
            PyValue::None => Vec::new(),
            _ => return Err(ProtoError::Shape("call args must be a tuple")),
        };
        let kwargs = match t.get(3) {
            Some(PyValue::Dict(d)) => d.clone(),
            Some(PyValue::None) | None => Vec::new(),
            Some(_) => return Err(ProtoError::Shape("call kwargs must be a dict")),
        };
        let method = text(&t[1], "call method must be a string")?;

        // The client tests truthiness, not equality with 1.
        if callc::is_bound_object(kind as i32) {
            if method == RELEASE_METHOD {
                return Ok(CallPayload::Release {
                    objects: released_objects(&t[0])?,
                });
            }
            Ok(CallPayload::BoundObject {
                object_id: text(&t[0], "bound objectID must be a string")?,
                method,
                args,
                kwargs,
            })
        } else {
            Ok(CallPayload::Service {
                method,
                args,
                kwargs,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_call_round_trips() {
        let c = CallPayload::Service {
            method: "GetCharacterSelectionData".into(),
            args: vec![],
            kwargs: vec![],
        };
        assert_eq!(CallPayload::from_payload(&c.to_payload()).unwrap(), c);
    }

    #[test]
    fn service_call_with_args_round_trips() {
        let c = CallPayload::Service {
            method: "SelectCharacterID".into(),
            args: vec![PyValue::Int(90_000_001), PyValue::Bool(false)],
            kwargs: vec![(PyValue::Int(1), PyValue::Bytes(b"loginAs".to_vec()))],
        };
        assert_eq!(CallPayload::from_payload(&c.to_payload()).unwrap(), c);
    }

    /// The release notification, in the exact shape the client sends it.
    ///
    /// The object ids and refIDs below are copied from a capture's `OID-`
    /// out-of-band dict, so this is the real thing rather than a shape we
    /// invented and then agreed with ourselves about.
    #[test]
    fn a_release_notification_carries_a_list_where_an_object_id_belongs() {
        let inner = PyValue::Tuple(vec![
            PyValue::List(vec![
                PyValue::Tuple(vec![
                    PyValue::Bytes(b"N=1:6".to_vec()),
                    PyValue::Int(134_368_993_569_987_287),
                ]),
                PyValue::Tuple(vec![
                    PyValue::Bytes(b"N=1:14".to_vec()),
                    PyValue::Int(134_368_993_646_172_739),
                ]),
            ]),
            PyValue::Bytes(RELEASE_METHOD.as_bytes().to_vec()),
            PyValue::Tuple(Vec::new()),
            PyValue::Dict(Vec::new()),
        ]);
        let payload = PyValue::Tuple(vec![PyValue::Int(1), PyValue::SubStream(to_pickle(&inner))]);

        let parsed = CallPayload::from_payload(&payload).unwrap();
        assert_eq!(
            parsed,
            CallPayload::Release {
                objects: vec![
                    ("N=1:6".into(), 134_368_993_569_987_287),
                    ("N=1:14".into(), 134_368_993_646_172_739),
                ],
            }
        );
        // And it survives a trip back out, so the variant is a faithful model
        // of the payload and not just somewhere to put it.
        assert_eq!(
            CallPayload::from_payload(&parsed.to_payload()).unwrap(),
            parsed
        );
    }

    #[test]
    fn bound_object_call_round_trips() {
        let c = CallPayload::BoundObject {
            object_id: "N=1234:7".into(),
            method: "GetAllInfo".into(),
            args: vec![PyValue::Bool(true), PyValue::Bool(true), PyValue::None],
            kwargs: vec![],
        };
        assert_eq!(CallPayload::from_payload(&c.to_payload()).unwrap(), c);
    }

    #[test]
    fn the_kind_flag_selects_the_variant() {
        let svc = CallPayload::Service {
            method: "x".into(),
            args: vec![],
            kwargs: vec![],
        };
        let PyValue::Tuple(t) = svc.to_payload() else {
            panic!()
        };
        assert_eq!(t[0], PyValue::Int(0));

        let bound = CallPayload::BoundObject {
            object_id: "N=1:1".into(),
            method: "x".into(),
            args: vec![],
            kwargs: vec![],
        };
        let PyValue::Tuple(t) = bound.to_payload() else {
            panic!()
        };
        assert_eq!(t[0], PyValue::Int(1));
    }

    #[test]
    fn the_pickle_is_a_complete_independent_stream() {
        // It must carry its own marshal header, because FromPickle calls
        // blue.marshal.Load on it directly.
        let c = CallPayload::Service {
            method: "GetTime".into(),
            args: vec![],
            kwargs: vec![],
        };
        let PyValue::Tuple(t) = c.to_payload() else {
            panic!()
        };
        // Carried under the substream tag, which is the form the real client
        // sends and therefore the form we emit.
        let PyValue::SubStream(blob) = &t[1] else {
            panic!("pickle must be a substream, got {:?}", t[1])
        };
        assert_eq!(blob[0], 0x7E, "pickle must start with a stream marker");
        // and it must parse standalone
        assert!(from_pickle(blob).is_ok());
    }

    #[test]
    fn both_pickle_carriers_decode_to_the_same_call() {
        let c = CallPayload::Service {
            method: "GetServiceInfo".into(),
            args: vec![],
            kwargs: vec![],
        };
        let PyValue::Tuple(t) = c.to_payload() else {
            panic!()
        };
        let PyValue::SubStream(blob) = &t[1] else {
            panic!()
        };
        // The byte-string spelling must decode identically: it is what
        // ToPickle literally returns, so a server or capture that uses it is
        // not wrong, just differently carried.
        let as_bytes = PyValue::Tuple(vec![t[0].clone(), PyValue::Bytes(blob.clone())]);
        assert_eq!(CallPayload::from_payload(&as_bytes).unwrap(), c);
        assert_eq!(CallPayload::from_payload(&c.to_payload()).unwrap(), c);
    }

    #[test]
    fn a_nonzero_kind_other_than_one_is_still_a_bound_call() {
        let bound = CallPayload::BoundObject {
            object_id: "C=5:2".into(),
            method: "OnNotify".into(),
            args: vec![],
            kwargs: vec![],
        };
        let PyValue::Tuple(mut t) = bound.to_payload() else {
            panic!()
        };
        t[0] = PyValue::Int(42); // truthy, not 1
        let parsed = CallPayload::from_payload(&PyValue::Tuple(t)).unwrap();
        assert!(matches!(parsed, CallPayload::BoundObject { .. }));
    }

    #[test]
    fn malformed_payloads_are_rejected() {
        assert!(CallPayload::from_payload(&PyValue::None).is_err());
        assert!(CallPayload::from_payload(&PyValue::Tuple(vec![PyValue::Int(0)])).is_err());
        // pickle that is not bytes
        let bad = PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(5)]);
        assert!(CallPayload::from_payload(&bad).is_err());
        // bytes that are not a valid stream
        let bad2 = PyValue::Tuple(vec![PyValue::Int(0), PyValue::Bytes(vec![0x01, 0x02])]);
        assert!(CallPayload::from_payload(&bad2).is_err());
    }
}
