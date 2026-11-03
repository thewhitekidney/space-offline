//! **eve-probe: our own non-graphical EVE client.**
//!
//! It connects to a machoNet server, performs the V24.01 login handshake as
//! the client performs it, drives a scripted session, and writes down
//! everything the server streams back.
//!
//! # Why a client
//!
//! Most of what a running EVE server knows reaches a client *only* as a
//! stream: market orders, agent dialogue, corporation rows, station contents,
//! mission offers, standings. None of it is in a shipped file or a public
//! static dump, because a server has to author it. We had never seen those
//! shapes on the wire, and the cheapest way to see them is to point our own
//! codec at a server that already implements them.
//!
//! Second reason, and the longer-lived one: this is how our own server gets
//! regression-tested with no GUI and no human, which is the unsolved half of
//! `docs/archive/plan-client-ci.md`.
//!
//! # What it is not
//!
//! It is not a game client. It does not render, it does not run Python, and
//! it does not pretend to be either. In particular it cannot execute the
//! `signedFunc` a server sends at H8 — and it does not have to, because the
//! client's own `GPS.__Execute` swallows every exception the expression
//! raises and the server never inspects the result
//! (`docs/re/evejs-crosscheck.md` §4). The probe reports `funcResult = {}`
//! and moves on, which is behaviour the real client also produces whenever
//! the expression fails.
//!
//! # Layering
//!
//! ```text
//! bin/probe            CLI: run a script against a server, or replay a capture
//!  └─ session          the handshake and the packet loop
//!      ├─ script       the session as data rather than code
//!      ├─ transcript   .log / .evecap / .schemas.md
//!      ├─ render       PyValue -> readable, and PyValue -> schema
//!      ├─ crypto       the Placebo pack, client side
//!      └─ frames       frame reassembly and payload inflation
//! ```
//!
//! Everything below `session` is pure and tested on constructed bytes; there
//! is no test in this crate that needs a server, because the gate runs with
//! no network at all.

#![forbid(unsafe_code)]

pub mod crypto;
pub mod frames;
pub mod render;
pub mod script;
pub mod session;
pub mod sha1;
pub mod transcript;

use eve_marshal::{Decoder, PyValue, Reader, WireError, take_stream};

/// Decode one complete marshal stream — header, shared-id table, body — into
/// a value.
///
/// Every payload on this protocol is one of these, and so is every nested
/// pickle inside one, which is why this is the crate's single entry point to
/// the codec rather than something each module does for itself.
///
/// The shared-object id table lives at the **end** of the stream, not after
/// the header; `eve_marshal::take_stream` is where that is written down, and
/// it is worth not rediscovering.
pub fn decode_stream(bytes: &[u8]) -> Result<PyValue, WireError> {
    let stream = take_stream(bytes)?;
    let mut decoder = Decoder::with_shared(stream.shared_ids.clone());
    decoder.load(&mut Reader::new(stream.body))
}

/// The class name `eve_proto::MachoAddress` insists on.
const ADDRESS_CLASS: &[u8] = b"carbon.common.script.net.machoNetAddress.MachoAddress";

/// Rewrite a foreign server's near-miss class names into the ones
/// `eve_proto` requires, and report each rewrite.
///
/// **This exists because of a real, observed delta.** EvEJS marshals a
/// packet's `source` and `destination` as instances of
/// `carbon.common.script.net.machoNetPacket.MachoAddress` — the right class
/// name in the *wrong module*; the address class lives in `machoNetAddress`,
/// and that is what the real client emits in the other direction (both
/// forms are in `/tmp/evejs-session.pcap`). The client accepts it, because
/// its unmarshaller only needs the name to be in `globalsWhitelist` and then
/// calls `__setstate__` on whatever it built. `eve_proto::MachoAddress::from_value`
/// compares the class name exactly, so without this every packet from that
/// server parses as "not a MachoPacket".
///
/// Normalising in the probe rather than relaxing `eve-proto` is deliberate:
/// strictness is right for a *server*, which should reject a packet it does
/// not understand, and wrong for a *probe*, whose job is to read what a
/// foreign server actually sent. The lane note carries this to the
/// coordinator as a question about `eve-proto` rather than an answer.
///
/// Returns the normalised value and the distinct class names it rewrote.
pub fn normalise_foreign(v: &PyValue) -> (PyValue, Vec<String>) {
    let mut seen = Vec::new();
    let out = rewrite(v, &mut seen);
    (out, seen)
}

fn rewrite(v: &PyValue, seen: &mut Vec<String>) -> PyValue {
    match v {
        PyValue::Instance { class, state } => {
            let mut class = class.clone();
            if class != ADDRESS_CLASS && class.ends_with(b".MachoAddress") {
                let name = String::from_utf8_lossy(&class).into_owned();
                if !seen.contains(&name) {
                    seen.push(name);
                }
                class = ADDRESS_CLASS.to_vec();
            }
            PyValue::Instance {
                class,
                state: Box::new(rewrite(state, seen)),
            }
        }
        PyValue::Tuple(items) => PyValue::Tuple(items.iter().map(|i| rewrite(i, seen)).collect()),
        PyValue::List(items) => PyValue::List(items.iter().map(|i| rewrite(i, seen)).collect()),
        other => other.clone(),
    }
}

/// Marshal a value into a complete stream with no shared objects.
pub fn encode_stream(v: &PyValue) -> Vec<u8> {
    let mut w = eve_marshal::Writer::new();
    eve_marshal::write_empty_header(&mut w);
    eve_marshal::save(v, &mut w);
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codec round-trips through a real stream header, which is the one
    /// property everything else in this crate rests on.
    #[test]
    fn streams_round_trip() {
        let cases = [
            PyValue::None,
            PyValue::Int(0),
            PyValue::Int(-1),
            PyValue::Int(1),
            PyValue::Int(i64::MIN),
            PyValue::Real(eve_marshal::build::reference().line.version),
            PyValue::Bytes(
                eve_marshal::build::reference()
                    .line
                    .codename_at_region
                    .as_bytes()
                    .to_vec(),
            ),
            PyValue::Unicode("offline".into()),
            PyValue::Tuple(Vec::new()),
            PyValue::Tuple(vec![PyValue::Int(i64::from(
                eve_marshal::build::reference().line.macho,
            ))]),
            PyValue::List(vec![PyValue::Bool(true), PyValue::Bool(false)]),
            PyValue::Dict(vec![(PyValue::Int(5), PyValue::Bytes(b"userid".to_vec()))]),
            PyValue::Tuple(vec![
                PyValue::Int(i64::from(eve_marshal::build::reference().line.secret)),
                PyValue::Int(i64::from(eve_marshal::build::reference().line.macho)),
                PyValue::Int(0),
                PyValue::Real(eve_marshal::build::reference().line.version),
                PyValue::Int(i64::from(eve_marshal::build::REFERENCE)),
                PyValue::Bytes(
                    eve_marshal::build::reference()
                        .line
                        .codename_at_region
                        .as_bytes()
                        .to_vec(),
                ),
            ]),
        ];
        for v in cases {
            let encoded = encode_stream(&v);
            assert_eq!(decode_stream(&encoded).expect("decodes"), v, "{v:?}");
        }
    }

    /// A stream whose bytes are not a stream is an error, not a panic. Every
    /// byte the probe reads came from a foreign server, so this is the
    /// ordinary case and not the exotic one.
    #[test]
    fn a_malformed_stream_is_an_error() {
        assert!(decode_stream(&[]).is_err());
        assert!(decode_stream(b"nope").is_err());
        // a valid header followed by nothing
        assert!(decode_stream(&[0x7E, 0, 0, 0, 0]).is_err());
        // a declared map table that is not there
        assert!(decode_stream(&[0x7E, 0x10, 0, 0, 0, 0x01]).is_err());
    }

    /// A packet whose addresses name the wrong module must still parse, and
    /// the rewrite must be reported rather than silent — a server getting a
    /// class name wrong is a finding, not a detail.
    #[test]
    fn a_foreign_address_class_is_normalised_and_reported() {
        let address = PyValue::Instance {
            class: b"carbon.common.script.net.machoNetPacket.MachoAddress".to_vec(),
            state: Box::new(PyValue::Tuple(vec![
                PyValue::Int(1),
                PyValue::Int(65450),
                PyValue::None,
                PyValue::None,
            ])),
        };
        let packet = PyValue::Tuple(vec![PyValue::Int(7), address]);
        let (fixed, rewritten) = normalise_foreign(&packet);
        assert_eq!(
            rewritten,
            vec!["carbon.common.script.net.machoNetPacket.MachoAddress"]
        );
        let PyValue::Tuple(t) = &fixed else {
            panic!("shape changed")
        };
        let PyValue::Instance { class, .. } = &t[1] else {
            panic!("not an instance")
        };
        assert_eq!(class.as_slice(), ADDRESS_CLASS);
        assert!(eve_proto::MachoAddress::from_value(&t[1]).is_ok());

        // An already-correct name is left alone and reported as nothing, so
        // the delta list means what it says.
        let (again, more) = normalise_foreign(&fixed);
        assert_eq!(again, fixed);
        assert!(more.is_empty());
    }

    /// Only the address class is rewritten. A packet class, a rowset class or
    /// anything else keeps its name: this is a targeted fix for one observed
    /// mismatch, not a general "accept anything".
    #[test]
    fn nothing_else_is_rewritten() {
        let v = PyValue::Instance {
            class: b"utillib.KeyVal".to_vec(),
            state: Box::new(PyValue::Dict(Vec::new())),
        };
        let (out, rewritten) = normalise_foreign(&v);
        assert_eq!(out, v);
        assert!(rewritten.is_empty());
    }

    /// `0x7D` is marshal **version 1** and carries no mapcount word, so its
    /// body starts one byte after the marker. Our own server never writes
    /// this form and the real client never does either -- but a third-party
    /// server might, which is the whole reason the probe exists. A reader
    /// that took the mapcount unconditionally would swallow four bytes of
    /// the body here and fail somewhere unrelated.
    #[test]
    fn a_version_one_header_decodes() {
        assert_eq!(
            decode_stream(&[0x7D, 0x01, 0x01]).expect("version 1 stream"),
            PyValue::None
        );
        // version 0, spelled the long way, does carry the mapcount.
        assert_eq!(
            decode_stream(&[0x7D, 0x00, 0, 0, 0, 0, 0x01]).expect("version 0 stream"),
            PyValue::None
        );
    }
}
