//! machoNet packet envelopes: the objects the client exchanges, on top of the
//! marshal codec in `eve-marshal`.
//!
//! Scope and its one limit. Every packet is an old-style Python instance
//! serialized by `__getstate__`/`__setstate__`, so on the wire it is
//! `(guid-object, state)` wrapped in a marshal *object* tag. The state tuples
//! are fully recovered and are what this crate builds and parses. The **outer
//! object wrapper's byte layout is not yet established** (marshal type codes
//! `0x17`/`0x22`/`0x23` — see `docs/archive/wire-format.md`), so this crate stops at
//! the state tuple and does not claim to produce a complete packet. When the
//! wrapper is settled, one function closes the gap and nothing here changes.
//!
//! Sources for the field orders: `carbon/common/script/net/machoNetPacket.py`,
//! `machoNetAddress.py`, `ObjectCallGPCS.py`, `ServiceCallGPCS.py`, and
//! `cluster/__init__.py` for the command ids. Written up in
//! `docs/archive/wire-format.md` and `docs/reference/bound-objects.md`.

#![forbid(unsafe_code)]

mod address;
mod call;
mod frame;
mod packet;

pub use address::MachoAddress;
pub use call::{
    CallPayload, object_reference, response_payload, user_error, wrapped_exception_payload,
};
pub use frame::{
    Frame, FrameError, Header as FrameHeader, MAX_FRAME, MAX_PAYLOAD, try_split_frame, write_frame,
};
pub use packet::{MachoPacket, ProtoError};
