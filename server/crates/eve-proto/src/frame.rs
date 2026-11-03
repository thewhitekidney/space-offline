//! machoNet TCP framing: the length prefix that delimits a payload.
//!
//! Recovered by disassembly, not capture — see `docs/reference/framing.md` for the
//! addresses. The frame is:
//!
//! ```text
//! u32 word0 (little-endian)
//!   bits 0..27  plen  — bytes that FOLLOW word0 (it does not count itself)
//!   bit  28     OOB_PRESENT
//!   bit  29     ZLIB
//!   bits 30..31 unused in this build
//! [ if bit 28: u32 oobLen, then oobLen bytes ]
//! payload
//! ```
//!
//! With no flags — which is every machoNet frame, since `machoNetTransport`
//! never passes an OOB header and native compression is off on this install —
//! a frame is literally `u32 len` followed by `len` bytes.
//!
//! We never set a flag bit. The legacy stacklessio reader does not mask the
//! length word at all, so a frame with any flag set is unreadable to it; all
//! zero flags is valid for both of the client's two implementations.

/// Low 28 bits of `word0` are the length.
pub const LENGTH_MASK: u32 = 0x0FFF_FFFF;
/// Bit 28: an out-of-band section precedes the payload.
pub const FLAG_OOB: u32 = 0x1000_0000;
/// Bit 29: the payload is `u32 rawLen` followed by a zlib stream.
pub const FLAG_ZLIB: u32 = 0x2000_0000;

/// The client's own Python-side ceiling on a marshal blob
/// (`machoNet.dropPacketThreshold`). The practical payload limit.
pub const MAX_PAYLOAD: usize = 5_000_000;
/// The native per-socket cap `SocketGPS` installs, **including** the 4-byte
/// header. The hard limit.
pub const MAX_FRAME: usize = 10_485_760;

// The Python-side blob threshold must stay below the native frame cap, header
// included. Checked at compile time rather than in a test.
const _: () = assert!(MAX_PAYLOAD + 4 < MAX_FRAME);

/// Framing errors. Deliberately distinct from `ProtoError`: this layer is
/// below the marshal codec and knows nothing about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// A payload we were asked to send exceeds [`MAX_FRAME`].
    TooLong { len: usize },
    /// The peer set bit 29; the payload is zlib and we cannot inflate here.
    /// Native compression is off on this install, so this should not occur.
    Compressed,
    /// The peer set bit 30 or 31, which nothing in this build uses.
    UnknownFlags { word0: u32 },
    /// A declared length exceeds [`MAX_FRAME`] — the client would silently
    /// discard and desynchronise, so we refuse instead.
    DeclaredTooLong { plen: u32 },
    /// The OOB section does not fit inside the frame it claims to be in.
    MalformedOob { plen: u32, oob_len: u32 },
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::TooLong { len } => {
                write!(f, "payload of {len} bytes exceeds the frame cap")
            }
            FrameError::Compressed => write!(f, "peer sent a zlib-compressed frame"),
            FrameError::UnknownFlags { word0 } => write!(f, "unknown frame flags in {word0:#010x}"),
            FrameError::DeclaredTooLong { plen } => {
                write!(f, "declared frame length {plen} too long")
            }
            FrameError::MalformedOob { plen, oob_len } => {
                write!(f, "oob section of {oob_len} does not fit in {plen}")
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// A decoded `word0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Bytes following `word0`, covering any OOB section plus the payload.
    pub plen: u32,
    pub oob: bool,
    pub zlib: bool,
}

impl Header {
    pub fn parse(word0: u32) -> Self {
        Header {
            plen: word0 & LENGTH_MASK,
            oob: word0 & FLAG_OOB != 0,
            zlib: word0 & FLAG_ZLIB != 0,
        }
    }

    /// Total bytes this frame occupies on the wire, header included.
    pub fn frame_len(&self) -> usize {
        self.plen as usize + 4
    }
}

/// Append a frame carrying `payload`, with no flags set.
pub fn write_frame(payload: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
    if payload.len() + 4 > MAX_FRAME {
        return Err(FrameError::TooLong { len: payload.len() });
    }
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

/// One frame taken off a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame<'a> {
    pub header: Header,
    /// The out-of-band section, present only when bit 28 was set. machoNet
    /// never sets it.
    pub oob: Option<&'a [u8]>,
    pub payload: &'a [u8],
}

/// Try to take one frame from the front of `buf`.
///
/// Returns `Ok(None)` when more bytes are needed — the client's reader behaves
/// the same way, so a caller should buffer and retry. On success, returns the
/// frame and how many bytes to consume; call again to handle frames that were
/// pipelined into one read, as the client does.
pub fn try_split_frame(buf: &[u8]) -> Result<Option<(Frame<'_>, usize)>, FrameError> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let word0 = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if word0 & !(LENGTH_MASK | FLAG_OOB | FLAG_ZLIB) != 0 {
        return Err(FrameError::UnknownFlags { word0 });
    }
    let h = Header::parse(word0);
    if h.frame_len() > MAX_FRAME {
        return Err(FrameError::DeclaredTooLong { plen: h.plen });
    }
    // A flag this server cannot honour is refused from the header alone,
    // before any of the frame is waited for (SEC-FIX 11): otherwise a peer
    // could declare a 10 MiB zlib frame and make us buffer all of it first.
    if h.zlib {
        return Err(FrameError::Compressed);
    }
    if buf.len() < h.frame_len() {
        return Ok(None);
    }
    let body = &buf[4..h.frame_len()];
    let (oob, payload) = if h.oob {
        if body.len() < 4 {
            return Err(FrameError::MalformedOob {
                plen: h.plen,
                oob_len: 0,
            });
        }
        let oob_len = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
        if 4 + oob_len > body.len() {
            return Err(FrameError::MalformedOob {
                plen: h.plen,
                oob_len: oob_len as u32,
            });
        }
        (Some(&body[4..4 + oob_len]), &body[4 + oob_len..])
    } else {
        (None, body)
    };
    Ok(Some((
        Frame {
            header: h,
            oob,
            payload,
        },
        h.frame_len(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_a_bare_length_then_the_payload() {
        let mut out = Vec::new();
        write_frame(b"hello", &mut out).unwrap();
        assert_eq!(out, vec![5, 0, 0, 0, b'h', b'e', b'l', b'l', b'o']);
    }

    #[test]
    fn the_prefix_does_not_count_itself() {
        let mut out = Vec::new();
        write_frame(&[0u8; 100], &mut out).unwrap();
        let word0 = u32::from_le_bytes([out[0], out[1], out[2], out[3]]);
        assert_eq!(
            word0, 100,
            "plen is the payload length, not the frame length"
        );
        assert_eq!(out.len(), 104);
        let (f, used) = try_split_frame(&out).unwrap().unwrap();
        assert_eq!(f.header.frame_len(), 104);
        assert_eq!(used, 104);
        assert_eq!(f.payload.len(), 100);
    }

    #[test]
    fn round_trips_and_sets_no_flags() {
        let mut out = Vec::new();
        write_frame(b"\x7e\x00\x00\x00\x00", &mut out).unwrap();
        let word0 = u32::from_le_bytes([out[0], out[1], out[2], out[3]]);
        assert_eq!(word0 & !LENGTH_MASK, 0, "we must never set a flag bit");
        let (f, _) = try_split_frame(&out).unwrap().unwrap();
        assert!(!f.header.oob && !f.header.zlib);
        assert_eq!(f.payload, b"\x7e\x00\x00\x00\x00");
        assert_eq!(f.oob, None);
    }

    /// SEC-FIX 11: the zlib flag is refused at the 4-byte header, not after
    /// the whole declared frame has been buffered.
    #[test]
    fn a_zlib_frame_is_refused_from_its_header_alone() {
        let word0 = FLAG_ZLIB | 1_000_000;
        let buf = word0.to_le_bytes();
        assert_eq!(try_split_frame(&buf), Err(FrameError::Compressed));
    }

    #[test]
    fn partial_reads_return_none_until_complete() {
        let mut out = Vec::new();
        write_frame(b"abcdefgh", &mut out).unwrap();
        for n in 0..out.len() {
            assert_eq!(
                try_split_frame(&out[..n]),
                Ok(None),
                "{n} bytes should be short"
            );
        }
        assert!(try_split_frame(&out).unwrap().is_some());
    }

    #[test]
    fn pipelined_frames_are_taken_one_at_a_time() {
        let mut out = Vec::new();
        write_frame(b"one", &mut out).unwrap();
        write_frame(b"two", &mut out).unwrap();
        write_frame(b"three", &mut out).unwrap();
        let mut rest: &[u8] = &out;
        let mut got = Vec::new();
        while let Some((f, used)) = try_split_frame(rest).unwrap() {
            got.push(f.payload.to_vec());
            rest = &rest[used..];
        }
        assert_eq!(
            got,
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );
        assert!(rest.is_empty());
    }

    #[test]
    fn an_oob_section_is_split_off() {
        // Hand-built, since we never emit one: plen covers oobLen + oob + payload.
        let oob = b"OID+";
        let payload = b"body";
        let plen = (4 + oob.len() + payload.len()) as u32;
        let mut buf = (plen | FLAG_OOB).to_le_bytes().to_vec();
        buf.extend_from_slice(&(oob.len() as u32).to_le_bytes());
        buf.extend_from_slice(oob);
        buf.extend_from_slice(payload);
        let (f, used) = try_split_frame(&buf).unwrap().unwrap();
        assert_eq!(f.oob, Some(&oob[..]));
        assert_eq!(f.payload, payload);
        assert_eq!(used, buf.len());
    }

    #[test]
    fn a_compressed_frame_is_reported_not_mis_parsed() {
        let mut buf = (3u32 | FLAG_ZLIB).to_le_bytes().to_vec();
        buf.extend_from_slice(&[1, 2, 3]);
        assert_eq!(try_split_frame(&buf), Err(FrameError::Compressed));
    }

    #[test]
    fn unused_flag_bits_are_rejected() {
        let mut buf = (1u32 | 0x4000_0000).to_le_bytes().to_vec();
        buf.push(0);
        assert!(matches!(
            try_split_frame(&buf),
            Err(FrameError::UnknownFlags { .. })
        ));
    }

    #[test]
    fn oversize_is_refused_in_both_directions() {
        let mut out = Vec::new();
        let huge = vec![0u8; MAX_FRAME];
        assert_eq!(
            write_frame(&huge, &mut out),
            Err(FrameError::TooLong { len: MAX_FRAME })
        );

        // A declared length past the cap must be refused rather than trusted:
        // the client silently discards such a frame and desynchronises.
        let buf = (MAX_FRAME as u32).to_le_bytes().to_vec();
        assert!(matches!(
            try_split_frame(&buf),
            Err(FrameError::DeclaredTooLong { .. })
        ));
    }

    #[test]
    fn a_payload_at_the_python_threshold_still_frames() {
        let mut out = Vec::new();
        assert!(write_frame(&vec![0u8; MAX_PAYLOAD], &mut out).is_ok());
        let (f, used) = try_split_frame(&out).unwrap().unwrap();
        assert_eq!(f.payload.len(), MAX_PAYLOAD);
        assert_eq!(used, MAX_PAYLOAD + 4);
    }

    #[test]
    fn a_zero_length_payload_is_a_valid_frame() {
        let mut out = Vec::new();
        write_frame(b"", &mut out).unwrap();
        assert_eq!(out, vec![0, 0, 0, 0]);
        let (f, used) = try_split_frame(&out).unwrap().unwrap();
        assert_eq!(f.payload.len(), 0);
        assert_eq!(used, 4);
    }
}
