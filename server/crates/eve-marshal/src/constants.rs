//! Wire constants for client build V24.01 (3619879), read directly out of the
//! client (see `docs/archive/wire-format.md`). Everything here is confirmed from the
//! build; the numeric *marshal opcode* table is deliberately NOT here yet — it
//! is native in `blue.dll` and is pinned at M1 from a live capture.

/// machoNet packet command ids (`MACHONETMSG_TYPE_*`, from `cluster/__init__.py`).
/// The first field of a serialized `MachoPacket`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MachoNetMsg {
    AuthenticationReq = 0,
    AuthenticationRsp = 1,
    IdentificationReq = 2,
    IdentificationRsp = 3,
    CallReq = 6,
    CallRsp = 7,
    TransportClosed = 8,
    ResolveReq = 10,
    ResolveRsp = 11,
    Notification = 12,
    ErrorResponse = 15,
    /// **16**, confirmed from `cluster.MACHONETMSG_TYPE_SESSIONCHANGENOTIFICATION`
    /// in the client's own `cluster/__init__.pyc`, where every id in this
    /// table is defined.
    SessionChangeNotification = 16,
    SessionInitialStateNotification = 18,
    PingReq = 20,
    PingRsp = 21,
}

impl MachoNetMsg {
    /// Map a command id off the wire to its variant, or `None` if unknown.
    pub fn from_u8(v: u8) -> Option<Self> {
        use MachoNetMsg::*;
        Some(match v {
            0 => AuthenticationReq,
            1 => AuthenticationRsp,
            2 => IdentificationReq,
            3 => IdentificationRsp,
            6 => CallReq,
            7 => CallRsp,
            8 => TransportClosed,
            10 => ResolveReq,
            11 => ResolveRsp,
            12 => Notification,
            15 => ErrorResponse,
            16 => SessionChangeNotification,
            18 => SessionInitialStateNotification,
            20 => PingReq,
            21 => PingRsp,
            _ => return None,
        })
    }
}

/// `MachoAddress.addressType` (from `const.py`): selects the address tuple shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AddressType {
    Node = 1,
    Client = 2,
    Broadcast = 4,
    Any = 8,
}

/// DBRowDescriptor column type codes (`DBTYPE_*`, from `const.py`). The client
/// validates packed rows against exactly this set (`blue.dll`: "DBRowDescriptor
/// doesn't support data type %d").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum DbType {
    Empty = 0,
    I2 = 2,
    I4 = 3,
    R4 = 4,
    R5 = 5,
    Cy = 6,
    Bool = 11,
    I1 = 16,
    Ui1 = 17,
    Ui2 = 18,
    Ui4 = 19,
    I8 = 20,
    Ui8 = 21,
    FileTime = 64,
    /// Declarable but **unreadable**: the column getter's jump table stops at
    /// `0x82` (`0x1802fc119`), so reading one raises "Unexpected db column
    /// type encountered". Use [`DbType::FileTime`] instead.
    DbTimestamp = 135,
    Str = 129,
    WStr = 130,
    /// **128, not the 508 that `const.py` claims.** The native `blue.db`
    /// module exports `DBTYPE_BYTES = 0x80` (`0x1802fe989`), with `0x81`/`0x82`
    /// for STR/WSTR. 508 exceeds the descriptor's `0x87` ceiling and is
    /// rejected outright.
    Bytes = 128,
}

impl DbType {
    pub fn from_u16(v: u16) -> Option<Self> {
        use DbType::*;
        Some(match v {
            0 => Empty,
            2 => I2,
            3 => I4,
            4 => R4,
            5 => R5,
            6 => Cy,
            11 => Bool,
            16 => I1,
            17 => Ui1,
            18 => Ui2,
            19 => Ui4,
            20 => I8,
            21 => Ui8,
            64 => FileTime,
            135 => DbTimestamp,
            129 => Str,
            130 => WStr,
            128 => Bytes,
            _ => return None,
        })
    }
}

/// Transport compression parameters for this build (`machoNetTransport.pyc`).
pub mod compression {
    /// Don't compress unless the uncompressed remainder exceeds this many bytes.
    pub const MIN_BYTES_TO_COMPRESS: usize = 200;
    /// Skip compression if the already-compressed fraction is at or above this %.
    pub const MAX_PERCENT_PRECOMPRESSED: u32 = 75;
    /// Packets larger than this are dropped by the transport.
    pub const DROP_PACKET_THRESHOLD: usize = 5_000_000;
    /// zlib level used on the software (non-IOCP) machoNet path.
    pub const ZLIB_LEVEL: u32 = 1;
    /// An uncompressed marshal stream begins with one of these bytes; anything
    /// else (in practice `0x78…`) is a zlib stream to inflate first.
    pub const UNCOMPRESSED_MARKERS: [u8; 2] = [0x7D, 0x7E]; // '}' '~'

    /// Mirror of the client's read-path test: is this blob zlib-compressed?
    pub fn is_compressed(blob: &[u8]) -> bool {
        match blob.first() {
            Some(b) => !UNCOMPRESSED_MARKERS.contains(b),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_round_trip() {
        for v in [0u8, 1, 2, 3, 6, 7, 8, 10, 11, 12, 15, 16, 18, 20, 21] {
            assert_eq!(MachoNetMsg::from_u8(v).map(|m| m as u8), Some(v), "cmd {v}");
        }
        assert_eq!(MachoNetMsg::from_u8(99), None);
        // the gaps in the numbering are genuinely unused commands
        assert_eq!(MachoNetMsg::from_u8(4), None);
    }

    #[test]
    fn dbtypes_match_the_native_module_not_const_py() {
        // The native blue.db init exports 0x80/0x81/0x82 as BYTES/STR/WSTR.
        // const.py's 508 for BYTES is stale and the client rejects it.
        assert_eq!(DbType::Bytes as u16, 128);
        assert_eq!(DbType::Str as u16, 129);
        assert_eq!(DbType::WStr as u16, 130);
        assert_eq!(DbType::from_u16(508), None, "508 must not be accepted");
        assert_eq!(DbType::from_u16(11), Some(DbType::Bool));
        assert_eq!(DbType::from_u16(7), None);
        // every declarable type is within the descriptor's 0x87 ceiling
        for v in [
            0u16, 2, 3, 4, 5, 6, 11, 16, 17, 18, 19, 20, 21, 64, 128, 129, 130, 135,
        ] {
            let t = DbType::from_u16(v).unwrap_or_else(|| panic!("0x{v:x}"));
            assert!(t as u16 <= 0x87, "0x{v:x} exceeds the descriptor ceiling");
        }
    }

    #[test]
    fn compression_discriminator_matches_client() {
        // '}' and '~' lead an uncompressed marshal stream; 0x78 is zlib.
        assert!(!compression::is_compressed(b"}abc"));
        assert!(!compression::is_compressed(&[0x7E, 0x00]));
        assert!(compression::is_compressed(&[0x78, 0x9C, 0x01]));
        assert!(!compression::is_compressed(&[]));
    }
}

/// Low-level handshake ("GPS") constants for this build. The handshake is
/// implemented in Python (`carbon/common/script/net/GPS.py`), not natively, so
/// these were read directly from the client. See `docs/reference/login-path.md`.
///
/// **Aliases of [`crate::build`]'s reference line and build** (BA-4), kept
/// so nothing that names them breaks; new code reads `crate::build`.
pub mod handshake {
    use crate::build::{REFERENCE, REFERENCE_LINE};

    /// `eveMachoNetVersion.machoVersion`. The client refuses a mismatch.
    pub const MACHO_VERSION: u32 = REFERENCE_LINE.macho;
    /// `GPSTransport.__HANDSHAKE_SECRET_KEY`, validated field 0 of the
    /// handshake tuple.
    pub const SECRET_KEY: u32 = REFERENCE_LINE.secret;
    /// `handShakePaddingLength` default.
    pub const PADDING_LENGTH: usize = REFERENCE_LINE.padding;
    /// `boot.version` — must marshal as a **real**, not an integer; the
    /// server-side check is type-strict.
    pub const BOOT_VERSION: f64 = REFERENCE_LINE.version;
    /// `boot.build` of the pinned build ([`crate::build::REFERENCE`]). The
    /// client rejects a server build *higher* than its own, so a server may
    /// report this or lower; the server advertises
    /// [`crate::build::line_min_build`].
    pub const BOOT_BUILD: u32 = REFERENCE;
    /// `boot.codename`.
    pub const CODENAME: &str = REFERENCE_LINE.codename;
    /// `boot.region`.
    pub const REGION: &str = REFERENCE_LINE.region;
    /// The combined field the handshake sends: `codename + "@" + region`.
    pub const CODENAME_AT_REGION: &str = REFERENCE_LINE.codename_at_region;
    /// `sessionType` in a SessionInitialStateNotification.
    pub const SESSION_TYPE: i32 = 5;
}

/// How a `CallReq`'s payload distinguishes a plain service call from a call on
/// a bound object. Verified in `ObjectCallGPCS.CallUp` and
/// `ServiceCallGPCS.CallUp`; see `docs/reference/bound-objects.md`.
///
/// `CallReq.payload` is a 2-tuple `(kind, substream)`. The substream
/// unmarshals to the inner call tuple:
///
/// - `kind` falsy  -> **service call**; inner `(1, method, args, kwargs)`,
///   and the packet's destination carries the `service` name.
/// - `kind` truthy -> **bound-object call**; inner
///   `(objectID, method, args, kwargs?)`, and the destination is a node
///   address with no service or callID set.
pub mod call {
    /// `payload[0]` for a plain service call.
    pub const KIND_SERVICE: i32 = 0;
    /// `payload[0]` for a call on a bound object.
    pub const KIND_BOUND_OBJECT: i32 = 1;

    /// A bound object hosted by a server node: `"N=<nodeID>:<counter>"`.
    pub const OBJECT_ID_NODE_PREFIX: char = 'N';
    /// A bound object hosted by the client: `"C=<clientID>:<counter>"`.
    /// These are notification-only (non-blocking).
    pub const OBJECT_ID_CLIENT_PREFIX: char = 'C';

    /// `oob` key announcing newly held object references.
    pub const OOB_REF_ADDED: &str = "OID+";
    /// `oob` key releasing object references.
    pub const OOB_REF_RELEASED: &str = "OID-";
    /// Notification the client sends to release bound objects.
    pub const RELEASE_NOTIFICATION: &str = "ClientHasReleasedTheseObjects";

    /// Is this payload kind a bound-object call? Mirrors the client's
    /// `if packet.payload[0]:` truthiness test.
    pub fn is_bound_object(kind: i32) -> bool {
        kind != KIND_SERVICE
    }

    /// Which side hosts the object named by this id, if it is well formed.
    pub fn object_host(object_id: &str) -> Option<char> {
        let c = object_id.chars().next()?;
        if object_id.as_bytes().get(1) != Some(&b'=') {
            return None;
        }
        match c {
            OBJECT_ID_NODE_PREFIX | OBJECT_ID_CLIENT_PREFIX => Some(c),
            _ => None,
        }
    }
}

#[cfg(test)]
mod call_tests {
    use super::call::*;

    #[test]
    fn payload_kind_matches_the_client_truthiness_test() {
        assert!(!is_bound_object(KIND_SERVICE));
        assert!(is_bound_object(KIND_BOUND_OBJECT));
        // the client tests truthiness, not equality with 1
        assert!(is_bound_object(7));
    }

    #[test]
    fn object_ids_name_their_host() {
        assert_eq!(object_host("N=12345:17"), Some('N'));
        assert_eq!(object_host("C=99:1"), Some('C'));
        assert_eq!(object_host("X=1:1"), None);
        assert_eq!(object_host("N12345:17"), None); // missing '='
        assert_eq!(object_host(""), None);
    }
}

#[cfg(test)]
mod handshake_tests {
    use super::handshake::*;

    #[test]
    fn handshake_constants_match_the_client() {
        // eveMachoNetVersion.machoVersion
        let l = &crate::build::REFERENCE_LINE;
        assert_eq!(MACHO_VERSION, l.macho);
        // GPSTransport.__HANDSHAKE_SECRET_KEY
        assert_eq!(SECRET_KEY, l.secret);
        assert_eq!(PADDING_LENGTH, l.padding);
        assert_eq!(BOOT_BUILD, crate::build::REFERENCE);
        assert_eq!(SESSION_TYPE, 5);
        // the handshake sends codename and region joined by '@'
        assert_eq!(CODENAME_AT_REGION, format!("{CODENAME}@{REGION}"));
    }

    #[test]
    fn boot_version_is_a_real_not_an_integer() {
        // The server-side Step1 check is type-strict: boot.version must
        // marshal as a real. Guard against it being retyped to an integer.
        assert!(
            BOOT_VERSION.fract() != 0.0,
            "BOOT_VERSION must not be integral"
        );
        assert_eq!(BOOT_VERSION, crate::build::REFERENCE_LINE.version);
    }
}
