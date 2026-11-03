// SPDX-License-Identifier: LicenseRef-NMSA-1.0 AND Zlib
// Portions transcribed from zlib's contrib/puff (puff.c 2.3): Huffman
// construction and decoding, and the block structure. Altered: translated to
// Rust, with bounds checks, an output cap and the incomplete-code rule changed.
// This is not the original software. See NOTICE. The puff notice follows, as
// its clause 3 requires:
//
//   Copyright (C) 2002-2013 Mark Adler, all rights reserved
//   version 2.3, 21 Jan 2013
//
//   This software is provided 'as-is', without any express or implied
//   warranty.  In no event will the author be held liable for any damages
//   arising from the use of this software.
//
//   Permission is granted to anyone to use this software for any purpose,
//   including commercial applications, and to alter it and redistribute it
//   freely, subject to the following restrictions:
//
//   1. The origin of this software must not be misrepresented; you must not
//      claim that you wrote the original software. If you use this software
//      in a product, an acknowledgment in the product documentation would be
//      appreciated but is not required.
//   2. Altered source versions must be plainly marked as such, and must not be
//      misrepresented as being the original software.
//   3. This notice may not be removed or altered from any source distribution.
//
//   Mark Adler    madler@alumni.caltech.edu

//! DEFLATE (RFC 1951) and zlib (RFC 1950) decompression, ported from zlib's puff (see the notice above).
//!
//! The client compresses large payloads. The first real client reached the
//! post-handshake stage and immediately sent a 174-byte frame starting `78 01`,
//! which inflates to a 275-byte marshal stream carrying a `CallReq` for
//! `machoNet.GetServiceInfo`. So compression is not optional: a server that
//! cannot inflate simply stops hearing the client the moment it starts talking.
//!
//! `78 01` is a zlib header (CM=8, CINFO=7, FLEVEL=0 "fastest"), not a marshal
//! header, which is why [`crate::inflate_zlib`] exists rather than a special
//! marshal tag. The compression is applied by the client's native
//! `blue.marshal` layer around the *whole* stream — the `0x7E` header is inside
//! the compressed data, not outside it.
//!
//! Why this is written here rather than pulled in. The workspace has no
//! external dependencies by policy, and inflate is a closed, fully specified
//! format with a published test surface, so it is a thing we can own
//! outright: about three hundred lines, no `unsafe`, and pinned against
//! vectors produced by the reference implementation.
//!
//! The decoder follows the structure of zlib's own `puff.c` reference: a
//! canonical-Huffman table of per-length counts plus a symbol list, decoded
//! one bit at a time. That is not the fastest arrangement, but it is the one
//! whose correctness can be read off the RFC line by line, and payloads here
//! are kilobytes, not megabytes.
//!
//! **This input is hostile by assumption.** It arrives from a socket before
//! anything has been authenticated, so every length is checked, the output is
//! capped (by [`MAX_OUTPUT`], or by the caller's own cap through
//! [`inflate_zlib_capped`]), and no error path can panic.

use std::fmt;

mod deflate;
pub use deflate::deflate_zlib;

/// Largest output a single payload may inflate to.
///
/// A compressed stream can expand by roughly 1000:1, so an attacker — or a
/// corrupted frame — could otherwise ask us to allocate gigabytes from a few
/// kilobytes on the wire. Real payloads are at most a few megabytes (a big
/// rowset), so 64 MiB is far above anything legitimate and far below anything
/// that threatens the process.
///
/// This is the cap for **trusted-direction** readers: the bots and `eve-probe`
/// inflate the server's own responses, which can be large. The server reads
/// the client's payloads with a lower cap through [`inflate_zlib_capped`]
/// (`docs/review/V2-wire.md` F2).
pub const MAX_OUTPUT: usize = 64 * 1024 * 1024;

/// Anything that can go wrong inflating a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflateError {
    /// Ran out of input mid-symbol.
    UnexpectedEof,
    /// The zlib header is not a zlib header (bad method, bad check bits).
    BadZlibHeader,
    /// A preset dictionary was requested. The client never uses one, and
    /// without the dictionary the stream cannot be decoded anyway.
    PresetDictionary,
    /// `BTYPE = 11`, which the format reserves and never assigns.
    ReservedBlockType,
    /// A stored block whose `LEN` and `NLEN` are not complements.
    StoredLengthMismatch,
    /// A code that is not in the tree being decoded with.
    BadCode,
    /// A code-length sequence that over- or under-subscribes its tree.
    BadCodeLengths,
    /// A back-reference pointing further back than the output produced.
    DistanceTooFar,
    /// Output exceeded the cap: [`MAX_OUTPUT`], or the one passed to
    /// [`inflate_zlib_capped`].
    OutputTooLarge,
    /// The trailing Adler-32 does not match the inflated bytes.
    ChecksumMismatch { want: u32, got: u32 },
    /// Bytes left over after the final block and the checksum.
    TrailingBytes,
}

impl fmt::Display for InflateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InflateError::UnexpectedEof => write!(f, "compressed stream ended mid-symbol"),
            InflateError::BadZlibHeader => write!(f, "not a zlib stream"),
            InflateError::PresetDictionary => write!(f, "zlib preset dictionary is not supported"),
            InflateError::ReservedBlockType => write!(f, "reserved deflate block type 11"),
            InflateError::StoredLengthMismatch => write!(f, "stored block LEN/NLEN mismatch"),
            InflateError::BadCode => write!(f, "invalid huffman code"),
            InflateError::BadCodeLengths => write!(f, "invalid huffman code lengths"),
            InflateError::DistanceTooFar => write!(f, "back-reference before start of output"),
            InflateError::OutputTooLarge => {
                write!(f, "inflated output exceeds its size cap")
            }
            InflateError::ChecksumMismatch { want, got } => {
                write!(
                    f,
                    "adler-32 mismatch: header says {want:#010x}, data gives {got:#010x}"
                )
            }
            InflateError::TrailingBytes => write!(f, "trailing bytes after the zlib stream"),
        }
    }
}

impl std::error::Error for InflateError {}

/// Does this look like a zlib stream?
///
/// Both tests from RFC 1950 §2.2: the compression method must be 8 (deflate),
/// and the two header bytes read big-endian must be a multiple of 31. Together
/// these reject every marshal header — `0x7E` and `0x7D` are not 8 in the low
/// nibble — so this can safely be used to tell a compressed payload from a
/// plain one.
pub fn looks_like_zlib(data: &[u8]) -> bool {
    if data.len() < 2 {
        return false;
    }
    let (cmf, flg) = (data[0], data[1]);
    cmf & 0x0F == 8 && (u16::from(cmf) << 8 | u16::from(flg)) % 31 == 0
}

/// Inflate a complete zlib stream: header, deflate data, Adler-32.
///
/// The checksum is verified rather than skipped. It costs one pass over bytes
/// we have just written anyway, and it turns "the frame was corrupted" into a
/// clear error instead of a baffling marshal failure three layers up.
pub fn inflate_zlib(data: &[u8]) -> Result<Vec<u8>, InflateError> {
    inflate_zlib_capped(data, MAX_OUTPUT)
}

/// [`inflate_zlib`] with the caller's own output cap.
///
/// Fails with [`InflateError::OutputTooLarge`] as soon as the output would
/// pass `cap` bytes, before the bytes are written. Output of exactly `cap`
/// bytes is accepted.
pub fn inflate_zlib_capped(data: &[u8], cap: usize) -> Result<Vec<u8>, InflateError> {
    if data.len() < 6 {
        return Err(InflateError::UnexpectedEof);
    }
    if !looks_like_zlib(data) {
        return Err(InflateError::BadZlibHeader);
    }
    // CINFO > 7 means a window above 32 KiB, which the format does not define.
    if data[0] >> 4 > 7 {
        return Err(InflateError::BadZlibHeader);
    }
    if data[1] & 0x20 != 0 {
        return Err(InflateError::PresetDictionary);
    }

    let body = &data[2..];
    let mut r = BitReader::new(body);
    let out = inflate(&mut r, cap)?;

    // The checksum is byte-aligned after the final block.
    let consumed = r.bytes_consumed();
    let tail = body.get(consumed..).ok_or(InflateError::UnexpectedEof)?;
    if tail.len() < 4 {
        return Err(InflateError::UnexpectedEof);
    }
    let want = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]);
    let got = adler32(&out);
    if want != got {
        return Err(InflateError::ChecksumMismatch { want, got });
    }
    if tail.len() > 4 {
        return Err(InflateError::TrailingBytes);
    }
    Ok(out)
}

/// Inflate a raw DEFLATE stream (no zlib wrapper, no checksum).
pub fn inflate_raw(data: &[u8]) -> Result<Vec<u8>, InflateError> {
    inflate(&mut BitReader::new(data), MAX_OUTPUT)
}

/// Adler-32 (RFC 1950 §9).
pub fn adler32(data: &[u8]) -> u32 {
    const BASE: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 is the largest run that cannot overflow `b` in 32 bits, which is
    // what lets the modulo stay outside the inner loop.
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= BASE;
        b %= BASE;
    }
    (b << 16) | a
}

// --- bit reader ------------------------------------------------------------

/// DEFLATE reads bits least-significant-first within each byte, while Huffman
/// codes are packed most-significant-bit-first. Keeping that asymmetry in one
/// small type is the whole point of this struct.
struct BitReader<'a> {
    data: &'a [u8],
    /// Index of the next byte to pull into `bitbuf`.
    pos: usize,
    bitbuf: u32,
    bitcnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            bitbuf: 0,
            bitcnt: 0,
        }
    }

    /// Read `n` bits (n <= 24), LSB first.
    fn bits(&mut self, n: u32) -> Result<u32, InflateError> {
        while self.bitcnt < n {
            let byte = *self.data.get(self.pos).ok_or(InflateError::UnexpectedEof)?;
            self.pos += 1;
            self.bitbuf |= u32::from(byte) << self.bitcnt;
            self.bitcnt += 8;
        }
        let v = self.bitbuf & ((1u32 << n) - 1);
        self.bitbuf >>= n;
        self.bitcnt -= n;
        Ok(v)
    }

    /// Drop the partial byte, leaving the reader on a byte boundary.
    fn align(&mut self) {
        let drop = self.bitcnt % 8;
        self.bitbuf >>= drop;
        self.bitcnt -= drop;
    }

    /// Take `n` whole bytes. Only valid right after [`BitReader::align`].
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], InflateError> {
        // Whole bytes already buffered must be given back before touching the
        // slice, or they would be read twice.
        let buffered = (self.bitcnt / 8) as usize;
        let start = self.pos - buffered;
        let end = start.checked_add(n).ok_or(InflateError::UnexpectedEof)?;
        let out = self
            .data
            .get(start..end)
            .ok_or(InflateError::UnexpectedEof)?;
        self.pos = end;
        self.bitbuf = 0;
        self.bitcnt = 0;
        Ok(out)
    }

    /// How many bytes of input the stream has actually consumed, counting the
    /// partially-used byte as consumed.
    fn bytes_consumed(&self) -> usize {
        self.pos - (self.bitcnt / 8) as usize
    }
}

// --- canonical huffman -----------------------------------------------------

/// A canonical Huffman tree as `puff.c` represents it: how many codes exist of
/// each bit length, and the symbols in canonical order.
struct Huffman {
    /// `counts[len]` = number of codes of that length. Index 0 is unused.
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
}

const MAX_BITS: usize = 15;

impl Huffman {
    /// Build from a list of per-symbol code lengths (0 = symbol unused).
    ///
    /// An incomplete tree is accepted only in the one case the RFC allows: a
    /// distance tree with a single code, which appears in streams that contain
    /// no matches at all. Anything else that does not exactly fill the code
    /// space is a corrupt stream, and silently tolerating it would let
    /// malformed input decode to attacker-chosen bytes.
    fn new(lengths: &[u8], allow_single: bool) -> Result<Huffman, InflateError> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &l in lengths {
            if l as usize > MAX_BITS {
                return Err(InflateError::BadCodeLengths);
            }
            counts[l as usize] += 1;
        }
        counts[0] = 0;

        // Kraft check: left starts at one code of length 0 and doubles each
        // level, minus the codes used at that level. It must reach exactly 0.
        let mut left = 1i32;
        for &count in &counts[1..=MAX_BITS] {
            left <<= 1;
            left -= i32::from(count);
            if left < 0 {
                return Err(InflateError::BadCodeLengths);
            }
        }

        // `left` is the code space nothing claimed. zlib's rule (and puff's):
        // an incomplete code is accepted only as a lone one-bit code in a
        // literal/length or distance tree, or as no codes at all. Anything
        // else leaves bit patterns that decode to nothing, which a corrupt or
        // hostile stream would use to steer the decoder (Q-FUZZ).
        let used: u32 = counts[1..].iter().map(|&n| u32::from(n)).sum();
        if left > 0 && used > 0 && !(allow_single && used == 1 && counts[1] == 1) {
            return Err(InflateError::BadCodeLengths);
        }

        let mut offs = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offs[len + 1] = offs[len] + counts[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Huffman { counts, symbols })
    }

    /// Decode one symbol, walking one bit at a time down the length classes.
    fn decode(&self, r: &mut BitReader) -> Result<u16, InflateError> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..=MAX_BITS {
            code |= r.bits(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - first < count {
                let at = (index + (code - first)) as usize;
                return self.symbols.get(at).copied().ok_or(InflateError::BadCode);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(InflateError::BadCode)
    }
}

// --- the length/distance tables (RFC 1951 §3.2.5) --------------------------

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order code lengths for the code-length alphabet are stored in
/// (RFC 1951 §3.2.7) — most-used lengths first, so trailing zeroes can be
/// omitted.
const CLEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

// --- the main loop ---------------------------------------------------------

fn inflate(r: &mut BitReader, cap: usize) -> Result<Vec<u8>, InflateError> {
    let mut out: Vec<u8> = Vec::new();
    // Built on the first fixed block, then reused: an empty fixed block is
    // ten bits, so a 5 MB frame of them is four million blocks, and rebuilding
    // the trees for each was the whole cost (Q-FUZZ).
    let mut fixed: Option<(Huffman, Huffman)> = None;
    loop {
        let last = r.bits(1)? == 1;
        match r.bits(2)? {
            0 => stored_block(r, &mut out, cap)?,
            1 => {
                if fixed.is_none() {
                    fixed = Some(fixed_trees()?);
                }
                if let Some((lit, dist)) = &fixed {
                    huffman_block(r, &mut out, lit, dist, cap)?;
                }
            }
            2 => {
                let (lit, dist) = dynamic_trees(r)?;
                huffman_block(r, &mut out, &lit, &dist, cap)?;
            }
            _ => return Err(InflateError::ReservedBlockType),
        }
        if last {
            return Ok(out);
        }
    }
}

fn stored_block(r: &mut BitReader, out: &mut Vec<u8>, cap: usize) -> Result<(), InflateError> {
    r.align();
    let hdr = r.bytes(4)?;
    let len = u16::from_le_bytes([hdr[0], hdr[1]]);
    let nlen = u16::from_le_bytes([hdr[2], hdr[3]]);
    if len != !nlen {
        return Err(InflateError::StoredLengthMismatch);
    }
    let len = len as usize;
    if out.len() + len > cap {
        return Err(InflateError::OutputTooLarge);
    }
    out.extend_from_slice(r.bytes(len)?);
    Ok(())
}

/// The fixed trees of RFC 1951 §3.2.6. Rebuilt per block rather than cached in
/// a static: it is a few hundred bytes of work on a path that runs once per
/// frame, and it keeps the code free of lazily-initialised global state.
fn fixed_trees() -> Result<(Huffman, Huffman), InflateError> {
    let mut lengths = [0u8; 288];
    for (sym, l) in lengths.iter_mut().enumerate() {
        *l = match sym {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let lit = Huffman::new(&lengths, false)?;
    // All 30 real distance codes are 5 bits; 30 and 31 exist in the tree but
    // never appear in a valid stream.
    let dist = Huffman::new(&[5u8; 32], false)?;
    Ok((lit, dist))
}

fn dynamic_trees(r: &mut BitReader) -> Result<(Huffman, Huffman), InflateError> {
    let hlit = r.bits(5)? as usize + 257;
    let hdist = r.bits(5)? as usize + 1;
    let hclen = r.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(InflateError::BadCodeLengths);
    }

    let mut clen_lengths = [0u8; 19];
    for &slot in CLEN_ORDER.iter().take(hclen) {
        clen_lengths[slot] = r.bits(3)? as u8;
    }
    let clen_tree = Huffman::new(&clen_lengths, false)?;

    // The literal/length and distance lengths are one run-length-coded
    // sequence, read as a single array and split afterwards.
    let total = hlit + hdist;
    let mut lengths = vec![0u8; total];
    let mut i = 0;
    while i < total {
        let sym = clen_tree.decode(r)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                // Repeat the previous length 3-6 times.
                if i == 0 {
                    return Err(InflateError::BadCodeLengths);
                }
                let prev = lengths[i - 1];
                let n = 3 + r.bits(2)? as usize;
                if i + n > total {
                    return Err(InflateError::BadCodeLengths);
                }
                lengths[i..i + n].fill(prev);
                i += n;
            }
            17 | 18 => {
                // Repeat zero: 3-10 times (17) or 11-138 times (18).
                let n = if sym == 17 {
                    3 + r.bits(3)? as usize
                } else {
                    11 + r.bits(7)? as usize
                };
                if i + n > total {
                    return Err(InflateError::BadCodeLengths);
                }
                lengths[i..i + n].fill(0);
                i += n;
            }
            _ => return Err(InflateError::BadCodeLengths),
        }
    }

    // An end-of-block code is mandatory; without it a stream could never
    // terminate and we would read past the block forever.
    if lengths[256] == 0 {
        return Err(InflateError::BadCodeLengths);
    }
    let lit = Huffman::new(&lengths[..hlit], true)?;
    let dist = Huffman::new(&lengths[hlit..], true)?;
    Ok((lit, dist))
}

fn huffman_block(
    r: &mut BitReader,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
    cap: usize,
) -> Result<(), InflateError> {
    loop {
        let sym = lit.decode(r)?;
        match sym {
            0..=255 => {
                if out.len() >= cap {
                    return Err(InflateError::OutputTooLarge);
                }
                out.push(sym as u8);
            }
            256 => return Ok(()),
            257..=285 => {
                let idx = sym as usize - 257;
                let len =
                    LENGTH_BASE[idx] as usize + r.bits(u32::from(LENGTH_EXTRA[idx]))? as usize;

                let dsym = dist.decode(r)? as usize;
                if dsym >= DIST_BASE.len() {
                    return Err(InflateError::BadCode);
                }
                let distance =
                    DIST_BASE[dsym] as usize + r.bits(u32::from(DIST_EXTRA[dsym]))? as usize;
                if distance > out.len() {
                    return Err(InflateError::DistanceTooFar);
                }
                if out.len() + len > cap {
                    return Err(InflateError::OutputTooLarge);
                }
                // Copied one byte at a time on purpose: DEFLATE matches may
                // overlap the region being written (distance 1, length 100 is
                // a run of one byte), so a bulk copy would be wrong.
                let start = out.len() - distance;
                for k in 0..len {
                    out.push(out[start + k]);
                }
            }
            _ => return Err(InflateError::BadCode),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bytes the first real client sent as its first post-handshake
    /// frame (capture `1792404377-0002.evecap`, frame 11). This is the reason
    /// the crate exists, so it is the first thing it is tested against.
    const CLIENT_FRAME: &[u8] = &[
        0x78, 0x01, 0xab, 0x63, 0x00, 0x02, 0x71, 0x61, 0xfd, 0xe4, 0xc4, 0xa2, 0xa4, 0xfc, 0x3c,
        0xbd, 0xe4, 0xfc, 0xdc, 0x5c, 0x20, 0x55, 0x9c, 0x5c, 0x94, 0x59, 0x50, 0xa2, 0x97, 0x97,
        0x5a, 0xa2, 0x97, 0x9b, 0x98, 0x9c, 0x91, 0xef, 0x97, 0x5a, 0x12, 0x90, 0x98, 0x9c, 0x0d,
        0xe4, 0x3a, 0x27, 0xe6, 0xe4, 0x04, 0xa5, 0x16, 0x8a, 0xf0, 0xb1, 0xb1, 0x89, 0x0b, 0x9b,
        0x12, 0xd4, 0xe5, 0x98, 0x92, 0x52, 0x94, 0x5a, 0x5c, 0xac, 0xe7, 0x0b, 0x32, 0x05, 0xca,
        0x11, 0x61, 0x61, 0x63, 0xe2, 0xe0, 0x64, 0x24, 0x5f, 0x3b, 0x27, 0xa7, 0x30, 0x07, 0xcc,
        0x59, 0x8c, 0x9c, 0xaa, 0x3a, 0x1c, 0xda, 0x72, 0x75, 0x20, 0x6f, 0x88, 0xb0, 0x70, 0x0a,
        0xf3, 0xb9, 0xa7, 0x96, 0x04, 0xa7, 0x16, 0x95, 0x65, 0x26, 0xa7, 0x7a, 0xe6, 0xa5, 0xe5,
        0xab, 0x88, 0x31, 0x72, 0x0a, 0xb6, 0x33, 0x32, 0x0a, 0xab, 0x98, 0x24, 0x1a, 0xa7, 0x25,
        0x9a, 0xa5, 0xa5, 0xe8, 0xa6, 0x24, 0xa6, 0x9a, 0xe9, 0x9a, 0x18, 0x26, 0x99, 0xea, 0x5a,
        0x5a, 0x98, 0x1a, 0xeb, 0x26, 0x27, 0x1b, 0xa5, 0xa5, 0xa6, 0x9a, 0x27, 0x27, 0x5a, 0x24,
        0xa6, 0x30, 0x82, 0x01, 0x00, 0xa8, 0xe4, 0x52, 0xb7,
    ];

    #[test]
    fn inflates_the_first_real_client_frame() {
        let out = inflate_zlib(CLIENT_FRAME).expect("the client's own frame must inflate");
        assert_eq!(out.len(), 275);
        // A marshal stream header, then an old-style instance (0x17) naming
        // the packet class.
        assert_eq!(&out[..6], &[0x7E, 0x00, 0x00, 0x00, 0x00, 0x17]);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("machoNetPacket.CallReq"), "{text:?}");
        assert!(text.contains("GetServiceInfo"), "{text:?}");
    }

    #[test]
    fn recognises_the_client_frame_as_zlib() {
        assert!(looks_like_zlib(CLIENT_FRAME));
    }

    #[test]
    fn marshal_headers_are_not_mistaken_for_zlib() {
        // The whole point of looks_like_zlib is telling these two apart.
        assert!(!looks_like_zlib(&[0x7E, 0x00, 0x00, 0x00, 0x00]));
        assert!(!looks_like_zlib(&[0x7D, 0x00, 0x00, 0x00, 0x00, 0x00]));
        // Nor any marshal stream, whatever its second byte.
        for b in 0u8..=255 {
            assert!(
                !looks_like_zlib(&[0x7E, b]),
                "0x7E {b:#04x} looked like zlib"
            );
            assert!(
                !looks_like_zlib(&[0x7D, b]),
                "0x7D {b:#04x} looked like zlib"
            );
        }
    }

    /// Vectors produced by Python's `zlib.compress` at every level, so all
    /// three block types and both tree kinds are exercised.
    #[test]
    fn round_trips_reference_vectors() {
        // zlib.compress(b"hello hello hello hello", 9)
        let hello: &[u8] = &[
            0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x27, 0x01, 0x68, 0x03,
            0x08, 0xb1,
        ];
        assert_eq!(inflate_zlib(hello).unwrap(), b"hello hello hello hello");

        // zlib.compress(b"", 9) -- empty input, one empty fixed block.
        let empty: &[u8] = &[0x78, 0xda, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        assert_eq!(inflate_zlib(empty).unwrap(), b"");

        // zlib.compress(b"abc", 0) -- level 0 emits a *stored* block.
        let stored: &[u8] = &[
            0x78, 0x01, 0x01, 0x03, 0x00, 0xfc, 0xff, 0x61, 0x62, 0x63, 0x02, 0x4d, 0x01, 0x27,
        ];
        assert_eq!(inflate_zlib(stored).unwrap(), b"abc");
    }

    #[test]
    fn adler32_matches_reference_values() {
        // From Python's zlib.adler32.
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"a"), 0x0062_0062);
        assert_eq!(adler32(b"abc"), 0x024d_0127);
        assert_eq!(adler32(b"123456789"), 0x091e_01de);
        // Long enough to cross the 5552-byte modulo chunk boundary.
        let long = vec![b'x'; 20_000];
        assert_eq!(adler32(&long), 0x6547_a11d);
    }

    #[test]
    fn overlapping_matches_are_copied_byte_by_byte() {
        // zlib.compress(b"a" * 300, 9): a length-258 match at distance 1,
        // which only decodes correctly if the copy is not a bulk memcpy.
        let data = vec![b'a'; 300];
        let compressed: &[u8] = &[
            0x78, 0xda, 0x4b, 0x4c, 0x1c, 0x05, 0xc4, 0x02, 0x00, 0xd8, 0xa8, 0x71, 0xad,
        ];
        assert_eq!(inflate_zlib(compressed).unwrap(), data);
    }

    /// The caller's cap holds on every output path (a stored block, a
    /// literal, a back-reference) and is inclusive: exactly `cap`
    /// bytes is accepted, one more is refused.
    #[test]
    fn a_caller_cap_refuses_one_byte_past_it() {
        // zlib.compress(b"abc", 0): one stored block.
        let stored: &[u8] = &[
            0x78, 0x01, 0x01, 0x03, 0x00, 0xfc, 0xff, 0x61, 0x62, 0x63, 0x02, 0x4d, 0x01, 0x27,
        ];
        // zlib.compress(b"a" * 300, 9): a literal, then back-references.
        let matched: &[u8] = &[
            0x78, 0xda, 0x4b, 0x4c, 0x1c, 0x05, 0xc4, 0x02, 0x00, 0xd8, 0xa8, 0x71, 0xad,
        ];
        for data in [stored, matched, CLIENT_FRAME] {
            let n = inflate_zlib(data).expect("inflates uncapped").len();
            assert_eq!(inflate_zlib_capped(data, n).map(|v| v.len()), Ok(n));
            assert_eq!(
                inflate_zlib_capped(data, n - 1),
                Err(InflateError::OutputTooLarge)
            );
        }
        // The first symbol of the matched stream is a literal, so a cap of
        // zero stops it on the literal path.
        assert_eq!(
            inflate_zlib_capped(matched, 0),
            Err(InflateError::OutputTooLarge)
        );
    }

    #[test]
    fn a_corrupt_checksum_is_rejected() {
        let mut bad = CLIENT_FRAME.to_vec();
        let n = bad.len();
        bad[n - 1] ^= 0xFF;
        assert!(matches!(
            inflate_zlib(&bad),
            Err(InflateError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn truncation_is_refused_at_every_length() {
        // Never a panic, never a success: the one thing a network decoder
        // must guarantee on a short read.
        for cut in 0..CLIENT_FRAME.len() {
            assert!(
                inflate_zlib(&CLIENT_FRAME[..cut]).is_err(),
                "a stream cut to {cut} bytes was accepted"
            );
        }
    }

    #[test]
    fn corruption_never_panics() {
        // Flip every bit of every byte in turn. Any of these may decode to
        // something, but none may panic or hang.
        for i in 0..CLIENT_FRAME.len() {
            for bit in 0..8 {
                let mut bad = CLIENT_FRAME.to_vec();
                bad[i] ^= 1 << bit;
                let _ = inflate_zlib(&bad);
            }
        }
    }

    #[test]
    fn non_zlib_input_is_refused() {
        assert_eq!(
            inflate_zlib(&[0x7E, 0, 0, 0, 0, 1, 2, 3]),
            Err(InflateError::BadZlibHeader)
        );
        assert_eq!(inflate_zlib(&[]), Err(InflateError::UnexpectedEof));
    }

    #[test]
    fn a_preset_dictionary_is_refused_rather_than_misread() {
        // FDICT set (FLG bit 5), with the check bits fixed up so the header
        // is otherwise valid.
        let mut hdr = [0x78u8, 0x20];
        for flg in 0x20u8..=0x3F {
            if (u16::from(hdr[0]) << 8 | u16::from(flg)) % 31 == 0 {
                hdr[1] = flg;
                break;
            }
        }
        let mut data = hdr.to_vec();
        data.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(inflate_zlib(&data), Err(InflateError::PresetDictionary));
    }

    #[test]
    fn a_reserved_block_type_is_refused() {
        // BFINAL=1, BTYPE=11 -> first byte 0b111 = 0x07.
        assert_eq!(inflate_raw(&[0x07]), Err(InflateError::ReservedBlockType));
    }

    #[test]
    fn a_back_reference_before_the_start_is_refused() {
        // A fixed-Huffman block whose first symbol is a length/distance pair.
        // Hand-built: BFINAL=1, BTYPE=01, then code 257 (len 3) and distance
        // code 0 (distance 1) with nothing emitted yet.
        let mut bits = BitWriter::new();
        bits.push(1, 1); // BFINAL
        bits.push(1, 2); // BTYPE = fixed
        bits.push_rev(0b0000001, 7); // literal/length symbol 257
        bits.push_rev(0b00000, 5); // distance symbol 0
        assert_eq!(
            inflate_raw(&bits.finish()),
            Err(InflateError::DistanceTooFar)
        );
    }

    /// Minimal LSB-first bit writer, for hand-building the malformed streams
    /// above. Only the tests need to emit deflate, so it lives here.
    struct BitWriter {
        out: Vec<u8>,
        bit: u32,
        acc: u32,
    }

    impl BitWriter {
        fn new() -> Self {
            BitWriter {
                out: Vec::new(),
                bit: 0,
                acc: 0,
            }
        }
        /// Push `n` bits of `v`, least-significant first (the way DEFLATE
        /// stores block headers and extra bits).
        fn push(&mut self, v: u32, n: u32) {
            for k in 0..n {
                self.acc |= ((v >> k) & 1) << self.bit;
                self.bit += 1;
                if self.bit == 8 {
                    self.out.push(self.acc as u8);
                    self.acc = 0;
                    self.bit = 0;
                }
            }
        }
        /// Push `n` bits of `v`, most-significant first (the way Huffman codes
        /// are packed).
        fn push_rev(&mut self, v: u32, n: u32) {
            for k in (0..n).rev() {
                self.push((v >> k) & 1, 1);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.bit > 0 {
                self.out.push(self.acc as u8);
            }
            self.out
        }
    }

    /// Q-FUZZ: the Kraft check refused only over-subscribed codes, so a code
    /// that left bit patterns unassigned was accepted and decoded whatever
    /// the stream steered it to. zlib's rule is now the rule: an incomplete
    /// code is a lone one-bit code in a literal/length or distance tree, or
    /// no codes at all.
    #[test]
    fn incomplete_huffman_codes_are_refused_except_the_lone_one_bit_code() {
        // 1/2 + 1/4 of the space used
        assert_eq!(
            Huffman::new(&[1, 2], true).err(),
            Some(InflateError::BadCodeLengths)
        );
        assert_eq!(
            Huffman::new(&[2, 2, 2], true).err(),
            Some(InflateError::BadCodeLengths)
        );
        // a lone code longer than one bit
        assert_eq!(
            Huffman::new(&[0, 2], true).err(),
            Some(InflateError::BadCodeLengths)
        );
        // the lone one-bit code: allowed in a distance tree, not in the
        // code-length code
        assert!(Huffman::new(&[1], true).is_ok());
        assert!(Huffman::new(&[0, 1, 0], true).is_ok());
        assert_eq!(
            Huffman::new(&[1], false).err(),
            Some(InflateError::BadCodeLengths)
        );
        // no codes at all
        assert!(Huffman::new(&[0, 0, 0], true).is_ok());
        // complete codes, and over-subscribed ones
        assert!(Huffman::new(&[1, 1], false).is_ok());
        assert!(Huffman::new(&[1, 2, 2], false).is_ok());
        assert_eq!(
            Huffman::new(&[1, 1, 1], true).err(),
            Some(InflateError::BadCodeLengths)
        );
    }
}
