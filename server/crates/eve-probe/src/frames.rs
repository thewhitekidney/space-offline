//! Frame reassembly, from the receiving end.
//!
//! `eve_proto::try_split_frame` does the hard part. What this adds is the two
//! things a *reader* needs and a splitter cannot provide:
//!
//! 1. **A buffer.** TCP hands over arbitrary slices, so a frame arrives in
//!    pieces and several frames arrive at once. The client's own reader
//!    (`docs/reference/framing.md`, `0x1803885a0`) buffers and loops; so does this.
//! 2. **Payload-level decompression.** Under the Placebo pack the frame
//!    payload *is* the marshal blob, and the Python transport compresses that
//!    blob itself rather than setting the frame's zlib bit — so a compressed
//!    payload looks like an ordinary frame whose first byte is not `0x7D` or
//!    `0x7E`. The real client sends such frames (its first `CallReq` in
//!    `/tmp/evejs-session.pcap` begins `78 01`), so anything reading this
//!    protocol has to inflate here or it will see a corrupt stream.
//!
//! The reader is deliberately a plain state machine over a `Vec<u8>` with no
//! socket in it, so it can be tested on constructed bytes.

use eve_proto::{FrameError, try_split_frame, write_frame};

/// Why a frame could not be turned into a payload.
#[derive(Debug)]
pub enum ReadError {
    Frame(FrameError),
    /// The payload announced itself as zlib and would not inflate.
    Inflate(eve_inflate::InflateError),
}

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ReadError::Frame(e) => write!(f, "framing: {e}"),
            ReadError::Inflate(e) => write!(f, "inflate: {e}"),
        }
    }
}

impl std::error::Error for ReadError {}

/// One payload taken off the wire, with what the wire did to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    /// The marshal blob, inflated if it arrived compressed.
    pub bytes: Vec<u8>,
    /// Bytes the frame occupied, header included — what a transcript should
    /// report, because it is what was actually read.
    pub wire_len: usize,
    /// Whether [`Payload::bytes`] was inflated from a zlib stream.
    pub compressed: bool,
}

/// A buffering frame reader.
#[derive(Debug, Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    pub fn new() -> Self {
        FrameReader { buf: Vec::new() }
    }

    /// Hand over bytes just read from the socket.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Bytes held because they do not yet make a whole frame. Non-zero at
    /// the end of a session means the peer stopped mid-frame, which is worth
    /// saying out loud rather than discarding.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Take the next complete payload, or `None` if more bytes are needed.
    pub fn next_payload(&mut self) -> Result<Option<Payload>, ReadError> {
        let (bytes, wire_len, compressed) = match try_split_frame(&self.buf) {
            Err(e) => return Err(ReadError::Frame(e)),
            Ok(None) => return Ok(None),
            Ok(Some((frame, consumed))) => {
                let raw = frame.payload;
                if eve_marshal::compression::is_compressed(raw) {
                    let inflated = eve_inflate::inflate_zlib(raw).map_err(ReadError::Inflate)?;
                    (inflated, consumed, true)
                } else {
                    (raw.to_vec(), consumed, false)
                }
            }
        };
        self.buf.drain(..wire_len);
        Ok(Some(Payload {
            bytes,
            wire_len,
            compressed,
        }))
    }
}

/// `u32_le(len) + payload`, which is the whole of machoNet framing on the
/// send side. Never sets a flag bit — `docs/reference/framing.md` is explicit that the
/// legacy reader cannot read one.
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    write_frame(payload, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(payload: &[u8]) -> Vec<u8> {
        encode_frame(payload).expect("short payload")
    }

    /// One frame, delivered one byte at a time. The splitter must not see a
    /// frame until the last byte, and must see exactly one then.
    #[test]
    fn a_frame_split_across_every_boundary() {
        let wire = framed(b"\x7e\x00\x00\x00\x00\x01");
        let mut r = FrameReader::new();
        for (i, b) in wire.iter().enumerate() {
            r.push(&[*b]);
            if i + 1 < wire.len() {
                assert_eq!(
                    r.next_payload().unwrap(),
                    None,
                    "byte {i} ended a frame early"
                );
            }
        }
        let p = r.next_payload().unwrap().expect("complete frame");
        assert_eq!(p.bytes, b"\x7e\x00\x00\x00\x00\x01");
        assert_eq!(p.wire_len, 10);
        assert!(!p.compressed);
        assert_eq!(r.next_payload().unwrap(), None);
        assert_eq!(r.buffered(), 0);
    }

    /// Three frames in one read, which is what the client's own reader loops
    /// for and what a server pipelining a handshake actually does: EvEJS
    /// writes its password-version and server-handshake packets back to back.
    #[test]
    fn pipelined_frames_all_come_out() {
        let mut wire = Vec::new();
        for n in 1..=3u8 {
            wire.extend_from_slice(&framed(&[0x7E, 0, 0, 0, 0, n]));
        }
        let mut r = FrameReader::new();
        r.push(&wire);
        let mut seen = Vec::new();
        while let Some(p) = r.next_payload().unwrap() {
            seen.push(p.bytes[5]);
        }
        assert_eq!(seen, vec![1, 2, 3]);
        assert_eq!(r.buffered(), 0);
    }

    /// A payload whose first byte is neither `0x7D` nor `0x7E` is a zlib
    /// stream and must be inflated. Round-tripped through our own deflate so
    /// the test needs no stored bytes.
    #[test]
    fn a_compressed_payload_is_inflated() {
        // A stored zlib stream of `7e 00 00 00 00 01`, produced by zlib at
        // level 1 -- the level the client's transport uses. Checked here by
        // inflating it, which is the only property the reader depends on.
        let blob: &[u8] = &[
            0x78, 0x01, 0xab, 0x63, 0x00, 0x02, 0x46, 0x00, 0x02, 0xfb, 0x00, 0x80,
        ];
        assert_eq!(
            eve_inflate::inflate_zlib(blob).unwrap(),
            vec![0x7E, 0, 0, 0, 0, 0x01],
            "the stored zlib stream must inflate to the marshal blob"
        );
        let mut r = FrameReader::new();
        r.push(&framed(blob));
        let p = r.next_payload().unwrap().unwrap();
        assert!(p.compressed);
        assert_eq!(p.bytes, vec![0x7E, 0, 0, 0, 0, 0x01]);
        assert_eq!(p.wire_len, blob.len() + 4);
    }

    /// An empty payload is a legal frame. It carries no marshal stream, so
    /// `is_compressed` must not call it zlib -- a `first()` of `None`.
    #[test]
    fn an_empty_payload_is_not_compressed() {
        let mut r = FrameReader::new();
        r.push(&framed(b""));
        let p = r.next_payload().unwrap().unwrap();
        assert!(p.bytes.is_empty());
        assert!(!p.compressed);
    }

    /// A frame header is four bytes and nothing else. Pinning the writer
    /// against the format rather than against itself.
    #[test]
    fn the_writer_emits_a_bare_little_endian_length() {
        assert_eq!(framed(b"ab"), vec![0x02, 0x00, 0x00, 0x00, b'a', b'b']);
        assert_eq!(&framed(&vec![0u8; 300])[..4], &[0x2C, 0x01, 0x00, 0x00]);
    }

    /// A truncated tail is held, not consumed: the session end needs to be
    /// able to say "the peer stopped mid-frame".
    #[test]
    fn a_partial_tail_stays_buffered() {
        let mut r = FrameReader::new();
        // A payload must start 0x7D or 0x7E to be read as marshal rather than
        // zlib -- `b"xy"` would not do, because `x` is 0x78.
        r.push(&framed(b"\x7e\x00\x00\x00\x00\x01"));
        r.push(&[0x10, 0x00]);
        assert!(r.next_payload().unwrap().is_some());
        assert_eq!(r.next_payload().unwrap(), None);
        assert_eq!(r.buffered(), 2);
    }
}
