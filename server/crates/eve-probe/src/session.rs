//! The handshake and the packet loop — the probe acting as a client.
//!
//! `docs/reference/login-path.md` §4 is the specification, read from the client's own
//! `GPS.py`. This module is the *client half* of that table, which is the
//! half nobody had written: our server implements H1, H5, H7, H8 and H10, so
//! what was missing was something that sends H2, H3, H4, H6 and H9.
//!
//! ```text
//! H1  S->C  (secret, macho, usercount, version, build, 'codename@region', updateInfo)
//!     (V24.01: 170472, 496, 24.01, 'V24.01@ccp'; read from `eve_marshal::build`)
//! H2  C->S  (secret, macho, 0, version, build, 'codename@region')
//! H3  C->S  (None, 'VK', CryptoHash(CaseFold(user)))
//! H4  C->S  ('placebo', {})
//! H5  S->C  'OK CC'
//! H6  C->S  (clientChallenge, requestDict)
//! H7  S->C  passwordVersion (2)
//! H8  S->C  (serverChallenge, signedFunc, context, responseDict)
//! H9  C->S  (CryptoHash(serverChallenge), funcOutput, funcResult)
//! H10 S->C  authenticationResult
//! ```
//!
//! # The probe checks but does not enforce
//!
//! The real client aborts on any H1 or H8 mismatch. A probe that did the same
//! would be useless against a foreign server, because the first build
//! difference would end the run with nothing recorded. So every check the
//! client makes is made here too, and a failure is written to the transcript
//! as a **delta** and then stepped over. Reporting precisely where a foreign
//! server disagrees with our client is the deliverable; refusing to talk to
//! it is not.
//!
//! The one thing that is *not* relaxed is what we send. H2's element 3 must
//! marshal as a real and not a string, the challenge must be 64 bytes, and
//! the crypto hash must be CRC'd over the client's own byte encoding — a
//! probe that sent something the real client would not send would be
//! measuring itself.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use eve_marshal::{MachoNetMsg, PyValue, WireError, handshake as hs};
use eve_proto::{CallPayload, MachoAddress, MachoPacket, ProtoError};

use crate::crypto::{case_fold, crypto_hash, password_hash, placebo_random};
use crate::frames::{FrameReader, ReadError, encode_frame};
use crate::render::{self, Budget};
use crate::script::Step;
use crate::transcript::{Dir, Transcript};
use crate::{decode_stream, encode_stream};

/// Anything that can stop a probe run.
#[derive(Debug)]
pub enum ProbeError {
    Io(io::Error),
    Read(ReadError),
    Wire(WireError),
    Proto(ProtoError),
    /// The peer closed, or stopped talking, before the exchange finished.
    Closed(&'static str),
    /// The peer said something this protocol does not allow at this point.
    /// The string is for a human; the transcript holds the bytes.
    Protocol(String),
}

impl core::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProbeError::Io(e) => write!(f, "io: {e}"),
            ProbeError::Read(e) => write!(f, "{e}"),
            ProbeError::Wire(e) => write!(f, "marshal: {e}"),
            ProbeError::Proto(e) => write!(f, "packet: {e}"),
            ProbeError::Closed(what) => write!(f, "connection closed while {what}"),
            ProbeError::Protocol(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ProbeError {}

impl From<io::Error> for ProbeError {
    fn from(e: io::Error) -> Self {
        ProbeError::Io(e)
    }
}
impl From<ReadError> for ProbeError {
    fn from(e: ReadError) -> Self {
        ProbeError::Read(e)
    }
}
impl From<WireError> for ProbeError {
    fn from(e: WireError) -> Self {
        ProbeError::Wire(e)
    }
}
impl From<ProtoError> for ProbeError {
    fn from(e: ProtoError) -> Self {
        ProbeError::Proto(e)
    }
}

/// What to log in and as.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub user: String,
    pub password: String,
    /// `None` takes the username/password path; `Some` takes the SSO path,
    /// which sends `user_name = 'EVE-SSO-CONNECTION'` and the token.
    pub sso_token: Option<String>,
    pub language: String,
}

impl Credentials {
    /// The `user_name` that goes on the wire, which is not the account name
    /// on the SSO path.
    pub fn wire_user(&self) -> &str {
        if self.sso_token.is_some() {
            "EVE-SSO-CONNECTION"
        } else {
            &self.user
        }
    }
}

/// H1, as the server sent it.
#[derive(Debug, Clone)]
pub struct VersionExchange {
    pub secret_key: i64,
    pub macho_version: i64,
    pub user_count: i64,
    pub boot_version: f64,
    pub boot_build: i64,
    pub codename: String,
    pub region: String,
    pub update_info: PyValue,
}

/// H8's `serverChallenge` and its `responseDict`, which is what the next two
/// messages both need.
type ServerHandshake = (Vec<u8>, Vec<(PyValue, PyValue)>);

/// What H8 and H10 together establish.
#[derive(Debug, Clone)]
pub struct Login {
    pub user_id: i64,
    pub client_id: i64,
    pub proxy_node_id: i64,
    pub queue_position: i64,
    pub session_init: Vec<(PyValue, PyValue)>,
    /// Keys of H8's `config_vals`, which is a server-authored blob and
    /// therefore streamed data in its own right.
    pub config_val_keys: Vec<String>,
    pub ack_keys: Vec<String>,
}

/// A live probe session.
pub struct Session {
    stream: TcpStream,
    reader: FrameReader,
    tr: Transcript,
    poll: Duration,
    client_id: i64,
    /// The account id the handshake granted. The real client puts it in
    /// every packet's `userID` field (field 3 of the 14-tuple), so we do too.
    user_id: Option<i64>,
    proxy_node_id: i64,
    next_call_id: i64,
    /// label -> objectID, filled in by `bind`.
    objects: BTreeMap<String, String>,
    /// callID -> (service, method, sent at ms)
    pending: BTreeMap<i64, (String, String, f64)>,
    /// service.method -> the shape of its reply, for the schema summary.
    shapes: BTreeMap<String, Vec<String>>,
    /// Unsolicited packets, counted by what they were.
    pushes: BTreeMap<String, usize>,
    /// Foreign class names already reported, so a delta is noted once.
    rewrites: BTreeSet<String>,
    pub deltas: Vec<String>,
}

impl Session {
    /// Connect, read H1, and report what the server claims to be.
    pub fn connect<A: ToSocketAddrs>(
        addr: A,
        mut tr: Transcript,
        poll: Duration,
    ) -> Result<(Self, VersionExchange), ProbeError> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(poll))?;
        tr.heading("stage 1: connect and frame")?;
        tr.line(&format!("connected to {}", stream.peer_addr()?))?;
        let mut s = Session {
            stream,
            reader: FrameReader::new(),
            tr,
            poll,
            client_id: 0,
            user_id: None,
            proxy_node_id: 0,
            next_call_id: 1,
            objects: BTreeMap::new(),
            pending: BTreeMap::new(),
            shapes: BTreeMap::new(),
            pushes: BTreeMap::new(),
            rewrites: BTreeSet::new(),
            deltas: Vec::new(),
        };
        let h1 = s.read_value("waiting for H1 version exchange", Duration::from_secs(10))?;
        s.tr.detail(&render::value(&h1, Budget::sample()))?;
        let v = s.parse_h1(&h1)?;
        Ok((s, v))
    }

    pub fn transcript(&mut self) -> &mut Transcript {
        &mut self.tr
    }

    pub fn client_id(&self) -> i64 {
        self.client_id
    }

    pub fn proxy_node_id(&self) -> i64 {
        self.proxy_node_id
    }

    /// Objects a `bind` step produced, as `label -> objectID`.
    pub fn objects(&self) -> &BTreeMap<String, String> {
        &self.objects
    }

    /// Calls that were issued and never answered. An unanswered `CallReq` is
    /// a hang on a real client, so naming them is the point.
    pub fn unanswered(&self) -> Vec<(String, String)> {
        self.pending
            .values()
            .map(|(s, m, _)| (s.clone(), m.clone()))
            .collect()
    }

    fn note_delta(&mut self, text: &str) -> Result<(), ProbeError> {
        self.deltas.push(text.to_string());
        self.tr.line(&format!("DELTA  {text}"))?;
        Ok(())
    }

    // ── framing ──────────────────────────────────────────────────────────

    fn send_value(&mut self, v: &PyValue, note: &str) -> Result<(), ProbeError> {
        let payload = encode_stream(v);
        let frame = encode_frame(&payload).map_err(ReadError::Frame)?;
        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        self.tr.frame(Dir::Out, &payload, note)?;
        self.tr.detail(&render::value(v, Budget::default()))?;
        Ok(())
    }

    /// Read one payload, blocking up to `budget`. `None` on timeout.
    fn next_payload(
        &mut self,
        budget: Duration,
    ) -> Result<Option<crate::frames::Payload>, ProbeError> {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(p) = self.reader.next_payload()? {
                return Ok(Some(p));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut buf = [0u8; 64 * 1024];
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    return if self.reader.buffered() == 0 {
                        Ok(None)
                    } else {
                        Err(ProbeError::Closed("a frame was incomplete"))
                    };
                }
                Ok(n) => self.reader.push(&buf[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(ProbeError::Io(e)),
            }
        }
    }

    fn read_value(&mut self, what: &'static str, budget: Duration) -> Result<PyValue, ProbeError> {
        match self.next_payload(budget)? {
            None => Err(ProbeError::Closed(what)),
            Some(p) => {
                let note = if p.compressed {
                    format!("{what} (zlib payload, {} bytes inflated)", p.bytes.len())
                } else {
                    what.to_string()
                };
                self.tr.frame(Dir::In, &p.bytes, &note)?;
                Ok(decode_stream(&p.bytes)?)
            }
        }
    }

    // ── handshake ────────────────────────────────────────────────────────

    fn parse_h1(&mut self, v: &PyValue) -> Result<VersionExchange, ProbeError> {
        let PyValue::Tuple(t) = v else {
            return Err(ProbeError::Protocol(format!(
                "H1 must be a tuple, got {}",
                render::shape(v)
            )));
        };
        if t.len() < 6 {
            return Err(ProbeError::Protocol(format!(
                "H1 must have at least 6 fields, got {}",
                t.len()
            )));
        }
        let int = |v: &PyValue| match v {
            PyValue::Int(i) => Some(*i),
            _ => None,
        };
        let combined = match &t[5] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            PyValue::Unicode(s) => s.clone(),
            other => {
                return Err(ProbeError::Protocol(format!(
                    "H1 codename must be a string, got {}",
                    render::shape(other)
                )));
            }
        };
        let (codename, region) = match combined.split_once('@') {
            Some((c, r)) => (c.to_string(), r.to_string()),
            None => (combined.clone(), String::new()),
        };
        let v = VersionExchange {
            secret_key: int(&t[0]).unwrap_or(-1),
            macho_version: int(&t[1]).unwrap_or(-1),
            user_count: int(&t[2]).unwrap_or(-1),
            boot_version: match &t[3] {
                PyValue::Real(f) => *f,
                PyValue::Int(i) => *i as f64,
                _ => f64::NAN,
            },
            boot_build: int(&t[4]).unwrap_or(-1),
            codename,
            region,
            update_info: t.get(6).cloned().unwrap_or(PyValue::None),
        };

        // Every check the client makes, in the client's own order. A failure
        // is recorded and stepped over; see the module header.
        if v.region != hs::REGION {
            self.note_delta(&format!(
                "HANDSHAKE_INCOMPATIBLEREGION: server region {:?}, client {:?}",
                v.region,
                hs::REGION
            ))?;
        }
        if v.codename != hs::CODENAME {
            self.note_delta(&format!(
                "HANDSHAKE_INCOMPATIBLERELEASE: server codename {:?}, client {:?}",
                v.codename,
                hs::CODENAME
            ))?;
        }
        if (v.boot_version - hs::BOOT_VERSION).abs() > f64::EPSILON {
            self.note_delta(&format!(
                "HANDSHAKE_INCOMPATIBLEVERSION: server {}, client {}",
                v.boot_version,
                hs::BOOT_VERSION
            ))?;
        }
        if v.macho_version != i64::from(hs::MACHO_VERSION) {
            self.note_delta(&format!(
                "HANDSHAKE_INCOMPATIBLEPROTOCOL: server macho {}, client {}",
                v.macho_version,
                hs::MACHO_VERSION
            ))?;
        }
        if v.boot_build > i64::from(hs::BOOT_BUILD) {
            self.note_delta(&format!(
                "HANDSHAKE_INCOMPATIBLEBUILD: server build {} is newer than the client's {}",
                v.boot_build,
                hs::BOOT_BUILD
            ))?;
        } else if v.boot_build != i64::from(hs::BOOT_BUILD) {
            // Not a client-side rejection -- the client accepts an older
            // server build -- but it is exactly the delta that makes service
            // lists and shapes differ, so it belongs in the record.
            self.note_delta(&format!(
                "build delta: server {} vs our client {} (accepted: the client \
                 only refuses a NEWER server). Framing and opcodes carry over; \
                 service names and reply shapes may not.",
                v.boot_build,
                hs::BOOT_BUILD
            ))?;
        }
        if v.secret_key != i64::from(hs::SECRET_KEY) {
            self.note_delta(&format!(
                "handshake secret key is {} not {} (the client does not check it)",
                v.secret_key,
                hs::SECRET_KEY
            ))?;
        }
        Ok(v)
    }

    /// H2 through H10. Returns what the server granted us.
    pub fn login(&mut self, creds: &Credentials) -> Result<Login, ProbeError> {
        self.tr.heading("stage 2: handshake")?;

        // H2. boot_version must be a real: the server-side check is
        // type-strict (`docs/reference/login-path.md` §4).
        self.send_value(
            &PyValue::Tuple(vec![
                PyValue::Int(i64::from(hs::SECRET_KEY)),
                PyValue::Int(i64::from(hs::MACHO_VERSION)),
                PyValue::Int(0),
                PyValue::Real(hs::BOOT_VERSION),
                PyValue::Int(i64::from(hs::BOOT_BUILD)),
                PyValue::Bytes(hs::CODENAME_AT_REGION.as_bytes().to_vec()),
            ]),
            "H2 version exchange",
        )?;

        // H3. The VIP key is CryptoHash(CaseFold(user_name)) -- hashed over
        // the client's own marshalling, see crate::crypto.
        let wire_user = creds.wire_user().to_string();
        let vip = crypto_hash(&[PyValue::Bytes(case_fold(&wire_user).into_bytes())]);
        self.send_value(
            &PyValue::Tuple(vec![
                PyValue::None,
                PyValue::Bytes(b"VK".to_vec()),
                PyValue::Bytes(vip.into_bytes()),
            ]),
            "H3 VK command",
        )?;

        // H4. `publicKeyVersion` is the literal 'placebo' and the Placebo
        // context's Initialize() returns an empty dict.
        self.send_value(
            &PyValue::Tuple(vec![
                PyValue::Bytes(b"placebo".to_vec()),
                PyValue::Dict(Vec::new()),
            ]),
            "H4 crypto request",
        )?;

        let h5 = self.read_value("waiting for H5 'OK CC'", Duration::from_secs(10))?;
        self.tr.detail(&render::value(&h5, Budget::default()))?;
        match &h5 {
            PyValue::Bytes(b) if b == b"OK CC" => {}
            other => {
                self.note_delta(&format!(
                    "H5 was {} rather than 'OK CC'",
                    render::value(other, Budget::default())
                ))?;
            }
        }

        // H6. 64 NUL bytes, because Placebo's get_random_bytes returns zeros.
        let challenge = placebo_random(hs::PADDING_LENGTH);
        let request = self.request_dict(creds);
        self.send_value(
            &PyValue::Tuple(vec![
                PyValue::Bytes(challenge.clone()),
                PyValue::Dict(request),
            ]),
            "H6 crypto challenge (login)",
        )?;

        // H7 and H8 arrive as two frames, and a server may pipeline them.
        let h7 = self.read_value("waiting for H7 password version", Duration::from_secs(15))?;
        self.tr.detail(&render::value(&h7, Budget::default()))?;
        match &h7 {
            PyValue::Int(2) => {}
            PyValue::Int(1) => {
                self.note_delta(
                    "H7 passwordVersion is 1: the real client resends H6 with a \
                     plaintext password. Not implemented -- no server we target \
                     asks for it.",
                )?;
            }
            other => {
                self.note_delta(&format!(
                    "H7 was {} rather than the expected int 2",
                    render::value(other, Budget::default())
                ))?;
            }
        }

        let h8 = self.read_value("waiting for H8 server handshake", Duration::from_secs(20))?;
        let (server_challenge, response) = self.parse_h8(&h8, &challenge)?;

        // H9. We cannot run `signedFunc` -- we have no Python -- and we do
        // not need to: the client's own __Execute swallows every exception
        // the expression raises and reports `funcResult = {}`. That is the
        // state we report, honestly.
        self.send_value(
            &PyValue::Tuple(vec![
                PyValue::Bytes(crypto_hash(&[PyValue::Bytes(server_challenge)]).into_bytes()),
                PyValue::Bytes(b"".to_vec()),
                PyValue::Dict(Vec::new()),
            ]),
            "H9 handshake result (no interpreter: funcResult = {})",
        )?;

        let h10 = self.read_value(
            "waiting for H10 authentication result",
            Duration::from_secs(20),
        )?;
        self.tr.detail(&render::value(&h10, Budget::sample()))?;
        self.finish_login(&h10, &response)
    }

    fn request_dict(&self, creds: &Credentials) -> Vec<(PyValue, PyValue)> {
        let key = |k: &str| PyValue::Bytes(k.as_bytes().to_vec());
        let hash = password_hash(creds.wire_user(), &creds.password);
        let mut entries = vec![
            (
                PyValue::Int(i64::from(hs::MACHO_VERSION)),
                key("macho_version"),
            ),
            (PyValue::Real(hs::BOOT_VERSION), key("boot_version")),
            (PyValue::Int(i64::from(hs::BOOT_BUILD)), key("boot_build")),
            (
                PyValue::Bytes(hs::CODENAME.as_bytes().to_vec()),
                key("boot_codename"),
            ),
            (
                PyValue::Bytes(hs::REGION.as_bytes().to_vec()),
                key("boot_region"),
            ),
            (
                PyValue::Bytes(creds.wire_user().as_bytes().to_vec()),
                key("user_name"),
            ),
            (PyValue::None, key("user_password")),
            (PyValue::Bytes(hash.to_vec()), key("user_password_hash")),
            (
                PyValue::Unicode(creds.language.clone()),
                key("user_languageid"),
            ),
            (PyValue::Int(0), key("user_affiliateid")),
            (
                match &creds.sso_token {
                    Some(t) => PyValue::Unicode(t.clone()),
                    None => PyValue::None,
                },
                key("user_sso_token"),
            ),
        ];
        entries.sort_by_key(|(_, k)| match k {
            PyValue::Bytes(b) => b.clone(),
            _ => Vec::new(),
        });
        entries
    }

    fn parse_h8(
        &mut self,
        v: &PyValue,
        client_challenge: &[u8],
    ) -> Result<ServerHandshake, ProbeError> {
        let PyValue::Tuple(t) = v else {
            return Err(ProbeError::Protocol(format!(
                "H8 must be a 4-tuple, got {}",
                render::shape(v)
            )));
        };
        if t.len() < 4 {
            return Err(ProbeError::Protocol(format!(
                "H8 must have 4 fields, got {}",
                t.len()
            )));
        }
        let server_challenge = match &t[0] {
            PyValue::Bytes(b) => b.clone(),
            PyValue::Unicode(s) => s.as_bytes().to_vec(),
            other => {
                self.note_delta(&format!(
                    "H8 serverChallenge is {} rather than a string",
                    render::shape(other)
                ))?;
                Vec::new()
            }
        };
        if server_challenge.len() != hs::PADDING_LENGTH {
            self.note_delta(&format!(
                "H8 serverChallenge is {} bytes, not the handShakePaddingLength of {}. \
                 The client does not check this (only the SERVER checks the client's), \
                 so it is legal and we hash whatever arrived.",
                server_challenge.len(),
                hs::PADDING_LENGTH
            ))?;
        }

        // signedFunc: `(blob, flag)`, and the flag is a required zero-equal
        // VALUE, not a disabled check -- `docs/re/evejs-crosscheck.md` §4.
        // We never run the blob, but we report what we were handed, because
        // a server sending executable code to every client is a finding.
        match &t[1] {
            PyValue::Tuple(f) if f.len() == 2 => {
                let verified = matches!(&f[1], PyValue::Int(0) | PyValue::Bool(false));
                let size = match &f[0] {
                    PyValue::Bytes(b) | PyValue::SubStream(b) => b.len(),
                    _ => 0,
                };
                self.tr.line(&format!(
                    "H8 signedFunc: {size}-byte CPython-marshal blob, flag {} ({})",
                    render::value(&f[1], Budget::default()),
                    if verified {
                        "zero-equal, so the client would run it"
                    } else {
                        "NOT zero-equal: the real client would raise \
                         'Failed to verify initial function blobs'"
                    }
                ))?;
                if size > 0 {
                    self.tr.line(
                        "NOTE  the server handed us code to execute. The probe has no \
                         interpreter and runs nothing.",
                    )?;
                }
            }
            other => {
                self.note_delta(&format!(
                    "H8 signedFunc is {} rather than a 2-tuple",
                    render::shape(other)
                ))?;
            }
        }

        let response = match &t[3] {
            PyValue::Dict(d) => d.clone(),
            other => {
                return Err(ProbeError::Protocol(format!(
                    "H8 responseDict must be a dict, got {}",
                    render::shape(other)
                )));
            }
        };

        let expect = crypto_hash(&[PyValue::Bytes(client_challenge.to_vec())]);
        match dict_str(&response, "challenge_responsehash") {
            Some(got) if got == expect => {
                self.tr
                    .line(&format!("H8 challenge_responsehash {got} matches"))?;
            }
            Some(got) => {
                self.note_delta(&format!(
                    "H8 challenge_responsehash is {got}, expected {expect}. The real \
                     client closes here with 'Server response signature incorrect'.",
                ))?;
            }
            None => {
                self.note_delta(
                    "H8 responseDict has no challenge_responsehash; the real client \
                     would compare None and close.",
                )?;
            }
        }
        Ok((server_challenge, response))
    }

    fn finish_login(
        &mut self,
        h10: &PyValue,
        h8_response: &[(PyValue, PyValue)],
    ) -> Result<Login, ProbeError> {
        let ack = match h10 {
            PyValue::Dict(d) => d.clone(),
            other => {
                return Err(ProbeError::Protocol(format!(
                    "H10 must be a dict, got {}",
                    render::shape(other)
                )));
            }
        };

        // `response.update(crAck)`: H10 is merged over H8, so a key may come
        // from either. Look in the merged view, as the client does.
        let mut merged = h8_response.to_vec();
        for (v, k) in &ack {
            merged.retain(|(_, ek)| ek != k);
            merged.push((v.clone(), k.clone()));
        }

        let session_init = match dict_get(&merged, "session_init") {
            Some(PyValue::Dict(d)) => d.clone(),
            Some(other) => {
                return Err(ProbeError::Protocol(format!(
                    "session_init must be a dict, got {}",
                    render::shape(other)
                )));
            }
            None => {
                return Err(ProbeError::Protocol(
                    "H10 has no session_init; the real client raises KeyError here".into(),
                ));
            }
        };
        let user_id = match dict_get(&session_init, "userid") {
            Some(PyValue::Int(i)) => *i,
            _ => {
                return Err(ProbeError::Protocol(
                    "session_init has no integer userid; the real client raises \
                     KeyError: 'userid'"
                        .into(),
                ));
            }
        };
        let proxy_node_id = match dict_get(&merged, "proxy_nodeid") {
            Some(PyValue::Int(i)) => *i,
            _ => {
                return Err(ProbeError::Protocol(
                    "neither H8 nor H10 carries proxy_nodeid; the real client raises \
                     KeyError and every later call would be addressed to node 0"
                        .into(),
                ));
            }
        };
        let client_id = match dict_get(&merged, "user_clientid") {
            Some(PyValue::Int(i)) => *i,
            _ => 0,
        };
        let queue_position = match dict_get(&merged, "user_logonqueueposition") {
            Some(PyValue::Int(i)) => *i,
            _ => 0,
        };
        if queue_position >= 2 {
            self.note_delta(&format!(
                "user_logonqueueposition is {queue_position}; the real client raises \
                 UserError('AutLogonFailureTotalUsers') at 2 or more"
            ))?;
        }

        // The two keys build 3619879 added, and whose absence killed the real
        // client against EvEJS right after character selection
        // (`docs/reference/login-path.md` §4 H10). Checked here so a probe run reports
        // it instead of a human finding out at the character screen.
        for required in ["access_token", "computer_hash"] {
            if dict_get(&merged, required).is_none() {
                self.note_delta(&format!(
                    "H10 has no {required}: connectionService.LoginSso indexes it \
                     directly, so the real client dies with KeyError: '{required}'"
                ))?;
            }
        }

        let config_val_keys = match dict_get(&merged, "config_vals") {
            Some(PyValue::Dict(d)) => dict_keys(d),
            _ => Vec::new(),
        };

        self.client_id = client_id;
        self.user_id = Some(user_id);
        self.proxy_node_id = proxy_node_id;
        self.tr.line(&format!(
            "logged in: userid={user_id} clientID={client_id} proxy_nodeid={proxy_node_id}"
        ))?;
        self.tr.line(&format!(
            "session_init keys: {}",
            dict_keys(&session_init).join(", ")
        ))?;
        if !config_val_keys.is_empty() {
            self.tr.line(&format!(
                "config_vals: {} keys — {}",
                config_val_keys.len(),
                config_val_keys.join(", ")
            ))?;
        }
        Ok(Login {
            user_id,
            client_id,
            proxy_node_id,
            queue_position,
            session_init,
            config_val_keys,
            ack_keys: dict_keys(&ack),
        })
    }

    // ── calls ────────────────────────────────────────────────────────────

    fn take_call_id(&mut self) -> i64 {
        let id = self.next_call_id;
        self.next_call_id += 1;
        id
    }

    /// The source address of a call: `Client(clientID, callID)`. The reply's
    /// destination echoes it, which is how requests and responses pair —
    /// never by order, because several calls are outstanding at once.
    ///
    /// **The clientID here is 0, not the `user_clientid` the handshake
    /// granted.** `MachoTransport._AssociateWithSession` installs
    /// `session.__dict__['clientID'] = 0` on the client branch, and the real
    /// client's own frames in `/tmp/evejs-session.pcap` are
    /// `MachoAddress(2, 0, <callID>, None)` throughout — while the same
    /// server's pushes are addressed to `(2, 20000065450, ...)`. A probe that
    /// put its granted id in the source would be the one thing on the wire
    /// that no client does.
    fn me(&self, call_id: i64) -> MachoAddress {
        MachoAddress::Client {
            client_id: 0,
            call_id: Some(call_id),
            service: None,
        }
    }

    /// Where a service call is addressed.
    ///
    /// The client has two forms and uses both: `sm.RemoteSvc(n)` addresses
    /// `MachoAddress(service=n)` — the ANY form, address type 8 — and
    /// `sm.ProxySvc(n)` addresses the proxy node by id. `machoNet` and
    /// `pingService` are the ProxySvc ones. Confirmed from the capture: the
    /// real client's `machoNet.GetServiceInfo` goes to `(1, 65450,
    /// 'machoNet', None)` and its `config.GetOldStationData` to
    /// `(8, 'config', None)`.
    fn destination(&self, service: &str) -> MachoAddress {
        if matches!(service, "machoNet" | "pingService") {
            MachoAddress::Node {
                node_id: Some(self.proxy_node_id),
                service: Some(service.to_string()),
                call_id: None,
            }
        } else {
            MachoAddress::Any {
                service: Some(service.to_string()),
                call_id: None,
            }
        }
    }

    /// `kw2` as `ServiceCallGPCS` builds it: the keyword dict with
    /// `machoVersion` forced to 1 when absent.
    fn kwargs() -> Vec<(PyValue, PyValue)> {
        vec![(PyValue::Int(1), PyValue::Bytes(b"machoVersion".to_vec()))]
    }

    /// Issue a plain service call. Returns its callID.
    pub fn call(
        &mut self,
        service: &str,
        method: &str,
        args: Vec<PyValue>,
    ) -> Result<i64, ProbeError> {
        let call_id = self.take_call_id();
        let payload = CallPayload::Service {
            method: method.to_string(),
            args,
            kwargs: Self::kwargs(),
        };
        let mut packet = MachoPacket::new(
            MachoNetMsg::CallReq,
            self.me(call_id),
            self.destination(service),
            vec![payload.to_payload()],
        );
        packet.user_id = self.user_id;
        self.send_packet(&packet, &format!("CallReq #{call_id} {service}.{method}"))?;
        self.pending.insert(
            call_id,
            (service.to_string(), method.to_string(), self.tr.millis()),
        );
        Ok(call_id)
    }

    /// Issue a call on an object a `bind` returned.
    ///
    /// The destination is the **node named in the objectID** with no service
    /// and no callID — `MachoObjectConnection` fixes it there, which is why
    /// an objectID whose node is not a node the server answers as is a hang
    /// with no missing method behind it (`docs/reference/bound-objects.md`).
    /// `label` is what the transcript and the schema summary call this
    /// object — its service name, not its objectID. The id contains a
    /// per-session counter, so keying the summary on it would give a
    /// different set of headings every run and make two runs
    /// incomparable, which is the opposite of what a regression tool needs.
    pub fn call_object(
        &mut self,
        label: &str,
        object_id: &str,
        method: &str,
        args: Vec<PyValue>,
    ) -> Result<i64, ProbeError> {
        let node = object_node(object_id).ok_or_else(|| {
            ProbeError::Protocol(format!("{object_id:?} is not an N=<node>:<n> object id"))
        })?;
        let call_id = self.take_call_id();
        let payload = CallPayload::BoundObject {
            object_id: object_id.to_string(),
            method: method.to_string(),
            args,
            kwargs: Self::kwargs(),
        };
        let mut packet = MachoPacket::new(
            MachoNetMsg::CallReq,
            self.me(call_id),
            MachoAddress::Node {
                node_id: Some(node),
                service: None,
                call_id: None,
            },
            vec![payload.to_payload()],
        );
        packet.user_id = self.user_id;
        self.send_packet(
            &packet,
            &format!("CallReq #{call_id} {label}[{object_id}].{method}"),
        )?;
        self.pending.insert(
            call_id,
            (label.to_string(), method.to_string(), self.tr.millis()),
        );
        Ok(call_id)
    }

    fn send_packet(&mut self, packet: &MachoPacket, note: &str) -> Result<(), ProbeError> {
        let payload = packet.to_payload()?;
        let frame = encode_frame(&payload).map_err(ReadError::Frame)?;
        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        self.tr.frame(Dir::Out, &payload, note)?;
        Ok(())
    }

    /// Read and record whatever arrives, until `budget` elapses with nothing
    /// outstanding or `budget` elapses outright.
    pub fn drain(&mut self, budget: Duration) -> Result<(), ProbeError> {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            let Some(p) = self.next_payload(self.poll)? else {
                continue;
            };
            let note = if p.compressed {
                format!("({} bytes inflated)", p.bytes.len())
            } else {
                String::new()
            };
            let value = match decode_stream(&p.bytes) {
                Ok(v) => v,
                Err(e) => {
                    self.tr
                        .frame(Dir::In, &p.bytes, &format!("UNDECODABLE {e}"))?;
                    self.note_delta(&format!("a payload would not decode: {e}"))?;
                    continue;
                }
            };
            let (value, rewritten) = crate::normalise_foreign(&value);
            for class in rewritten {
                if self.rewrites.insert(class.clone()) {
                    self.note_delta(&format!(
                        "the server marshals addresses as {class:?}; the class lives in \
                         machoNetAddress, and eve-proto compares the name exactly. The \
                         probe rewrites it -- see eve_probe::normalise_foreign."
                    ))?;
                }
            }
            match MachoPacket::from_value(&value) {
                Ok(packet) => self.handle_packet(&packet, &p.bytes, &note)?,
                Err(e) => {
                    self.tr
                        .frame(Dir::In, &p.bytes, &format!("not a MachoPacket: {e} {note}"))?;
                    self.tr.detail(&render::value(&value, Budget::default()))?;
                }
            }
        }
        Ok(())
    }

    fn handle_packet(
        &mut self,
        packet: &MachoPacket,
        raw: &[u8],
        note: &str,
    ) -> Result<(), ProbeError> {
        let call_id = packet.destination.call_id();
        match packet.command {
            MachoNetMsg::CallRsp => {
                let paired = call_id.and_then(|id| self.pending.remove(&id));
                let payload = packet.body.first().cloned().unwrap_or(PyValue::None);
                let value = unpickle(&payload);
                let (service, method, sent) = paired.unwrap_or_else(|| {
                    ("?".into(), format!("callID {call_id:?}"), self.tr.millis())
                });
                let took = self.tr.millis() - sent;
                self.tr.frame(
                    Dir::In,
                    raw,
                    &format!("CallRsp {service}.{method} ({took:.1}ms) {note}"),
                )?;
                self.tr.detail(&render::value(&value, Budget::sample()))?;
                self.record_shape(&service, &method, &value)?;
                self.learn_object(&service, &value)?;
            }
            MachoNetMsg::ErrorResponse => {
                let paired = call_id.and_then(|id| self.pending.remove(&id));
                let (service, method) = paired
                    .map(|(s, m, _)| (s, m))
                    .unwrap_or_else(|| ("?".into(), format!("callID {call_id:?}")));
                self.tr.frame(
                    Dir::In,
                    raw,
                    &format!("ErrorResponse {service}.{method} {note}"),
                )?;
                self.tr.detail(&render::value(
                    &PyValue::Tuple(packet.body.clone()),
                    Budget::sample(),
                ))?;
                self.record_shape(
                    &service,
                    &format!("{method} [raised]"),
                    &PyValue::Tuple(packet.body.clone()),
                )?;
            }
            MachoNetMsg::PingReq => {
                // Answer it. A probe that leaves pings unanswered is a probe
                // the server will eventually drop.
                let rsp = MachoPacket::respond_to(
                    packet,
                    MachoNetMsg::PingRsp,
                    vec![packet.body.first().cloned().unwrap_or(PyValue::None)],
                );
                self.tr.frame(Dir::In, raw, &format!("PingReq {note}"))?;
                self.send_packet(&rsp, "PingRsp")?;
            }
            other => {
                // Everything else is a push: something the server sent that
                // we did not ask for. These are the prize.
                let name = format!("{other:?}");
                *self.pushes.entry(name.clone()).or_insert(0) += 1;
                let body = deep_unpickle(&PyValue::Tuple(packet.body.clone()));
                let detail = push_name(packet, &body);
                self.tr
                    .frame(Dir::In, raw, &format!("PUSH {name} {detail} {note}"))?;
                self.tr.detail(&render::value(&body, Budget::sample()))?;
                self.record_shape("PUSH", &format!("{name} {detail}"), &body)?;
                if packet.command == MachoNetMsg::CallReq {
                    // The server is calling US. Nothing we can usefully run,
                    // but leaving it unanswered hangs the caller's tasklet on
                    // the far side, so reply None and say so.
                    let rsp = MachoPacket::respond_to(
                        packet,
                        MachoNetMsg::CallRsp,
                        vec![eve_proto::response_payload(&PyValue::None)],
                    );
                    self.send_packet(
                        &rsp,
                        "CallRsp None (the probe implements no client service)",
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Note the shape of a reply once per `service.method`, with one sample.
    fn record_shape(
        &mut self,
        service: &str,
        method: &str,
        value: &PyValue,
    ) -> Result<(), ProbeError> {
        let key = format!("{service}.{method}");
        if self.shapes.contains_key(&key) {
            return Ok(());
        }
        let mut lines = vec![format!("### `{key}`"), String::new()];
        lines.push(format!("- shape: `{}`", render::shape(value)));
        if let Some(cols) = render::columns_of(value) {
            lines.push(format!("- columns: `{}`", cols.join("`, `")));
        }
        if let Some(n) = render::row_count(value) {
            lines.push(format!("- rows: {n}"));
        }
        if let Some(row) = render::first_row(value) {
            if let Some(cols) = render::columns_of(row) {
                lines.push(format!("- row columns: `{}`", cols.join("`, `")));
            }
            lines.push(format!("- sample row: `{}`", clip(row)));
        } else {
            lines.push(format!("- value: `{}`", clip(value)));
        }
        lines.push(String::new());
        for l in &lines {
            self.tr.schema(l)?;
        }
        self.shapes.insert(key, lines);
        Ok(())
    }

    /// Learn an objectID out of a `MachoBindObject` reply, so a later `on`
    /// step can name it.
    fn learn_object(&mut self, service: &str, value: &PyValue) -> Result<(), ProbeError> {
        let PyValue::Tuple(t) = value else {
            return Ok(());
        };
        if t.len() != 2 {
            return Ok(());
        }
        let PyValue::Callback(child) = &t[0] else {
            return Ok(());
        };
        if let Some((id, refid)) = render::bound_object(child) {
            self.tr
                .line(&format!("bound {service} -> {id} (ref {refid})"))?;
            self.objects.insert(service.to_string(), id);
        }
        // The fused call's return value can itself be a bound object -- an
        // `invbroker.GetInventoryFromId` hands back an inventory to call
        // `List` on -- so label that too, as `<service>.retval`.
        if let PyValue::Callback(inner) = &t[1]
            && let Some((id, refid)) = render::bound_object(inner)
        {
            self.tr
                .line(&format!("bound {service}.retval -> {id} (ref {refid})"))?;
            self.objects.insert(format!("{service}.retval"), id);
        }
        Ok(())
    }

    /// Run a parsed script.
    pub fn run(&mut self, steps: &[Step], settle: Duration) -> Result<(), ProbeError> {
        for step in steps {
            match step {
                Step::Note(text) => self.tr.heading(text)?,
                Step::Drain { millis } => {
                    self.tr.line(&format!("drain {millis}ms"))?;
                    self.drain(Duration::from_millis(*millis))?;
                }
                Step::Call {
                    service,
                    method,
                    args,
                } => {
                    self.call(service, method, args.clone())?;
                    self.drain(settle)?;
                }
                Step::Bind {
                    service,
                    bind_params,
                    method,
                    args,
                } => {
                    // Resolve then bind, exactly as `Moniker.__ResolveImpl`
                    // and `Moniker.Bind` do: two round trips, with the first
                    // real call fused into the second.
                    self.call(service, "MachoResolveObject", vec![bind_params.clone()])?;
                    self.drain(settle)?;
                    let call = PyValue::Tuple(vec![
                        PyValue::Bytes(method.as_bytes().to_vec()),
                        PyValue::Tuple(args.clone()),
                        PyValue::Dict(Vec::new()),
                    ]);
                    self.call(service, "MachoBindObject", vec![bind_params.clone(), call])?;
                    self.drain(settle)?;
                }
                Step::On {
                    label,
                    method,
                    args,
                } => match self.objects.get(label).cloned() {
                    Some(id) => {
                        self.call_object(label, &id, method, args.clone())?;
                        self.drain(settle)?;
                    }
                    None => {
                        self.tr.line(&format!(
                            "SKIP  on {label}.{method}: no bound object with that label \
                             (a bind must have succeeded first)"
                        ))?;
                    }
                },
            }
        }
        Ok(())
    }

    /// Close out the transcript with the things only the end of a run knows.
    pub fn finish(&mut self) -> Result<(), ProbeError> {
        self.tr.heading("summary")?;
        let unanswered = self.unanswered();
        self.tr.line(&format!(
            "{} replies with a distinct shape, {} unanswered call(s), {} delta(s)",
            self.shapes.len(),
            unanswered.len(),
            self.deltas.len()
        ))?;
        for (service, method) in &unanswered {
            self.tr.line(&format!(
                "UNANSWERED  {service}.{method} — a hang on a real client"
            ))?;
        }
        let pushes: Vec<String> = self
            .pushes
            .iter()
            .map(|(k, n)| format!("{k} x{n}"))
            .collect();
        if !pushes.is_empty() {
            self.tr
                .line(&format!("unsolicited: {}", pushes.join(", ")))?;
        }
        for d in self.deltas.clone() {
            self.tr.line(&format!("DELTA  {d}"))?;
        }
        if self.reader.buffered() != 0 {
            self.tr.line(&format!(
                "{} byte(s) left in the read buffer: the peer stopped mid-frame",
                self.reader.buffered()
            ))?;
        }
        self.tr.flush()?;
        Ok(())
    }
}

/// A `CallRsp` payload is a **pickle**, not the value: the client hands it
/// straight to `blue.marshal.Load`. So is a `Notification`'s inner payload.
/// Unwrap one level when we see one.
pub fn unpickle(v: &PyValue) -> PyValue {
    match v {
        PyValue::Bytes(b) | PyValue::SubStream(b) => match decode_stream(b) {
            Ok(inner) => inner,
            Err(_) => v.clone(),
        },
        other => other.clone(),
    }
}

/// A rendered value, clipped so one 19,352-entry dict does not become one
/// 2 MB line in a file a person is meant to read.
fn clip(v: &PyValue) -> String {
    const MAX: usize = 1200;
    let mut s = render::value(v, Budget::sample());
    if s.len() > MAX {
        let cut = (0..=MAX)
            .rev()
            .find(|i| s.is_char_boundary(*i))
            .unwrap_or(0);
        s.truncate(cut);
        s.push_str(" ...");
    }
    s
}

/// Unwrap every nested pickle in a value.
///
/// A `Notification`'s payload is `(x, (0, <pickle>))` — `ObjectCall.FromPickle`
/// is what the client runs on it — so a push rendered without this is a page
/// of `'~\x00\x00\x00\x00...'` and tells a reader nothing. That is the one
/// shape this crate exists to reveal, so it is worth a heuristic.
///
/// **It is a heuristic**, and the shape of the risk is worth stating: any
/// byte string whose first byte is `0x7D` or `0x7E` and that decodes cleanly
/// as a complete marshal stream is replaced by what it decodes to. A genuine
/// payload of bytes could in principle satisfy both and be shown as
/// something it is not. The `.evecap` beside the log holds the original
/// bytes, which is the answer to every heuristic in a transcript.
pub fn deep_unpickle(v: &PyValue) -> PyValue {
    match v {
        PyValue::Bytes(b) | PyValue::SubStream(b) => {
            if eve_marshal::compression::is_compressed(b) {
                // Not a marshal stream: leave the bytes alone.
                return v.clone();
            }
            match decode_stream(b) {
                Ok(inner) => deep_unpickle(&inner),
                Err(_) => v.clone(),
            }
        }
        PyValue::Tuple(items) => PyValue::Tuple(items.iter().map(deep_unpickle).collect()),
        PyValue::List(items) => PyValue::List(items.iter().map(deep_unpickle).collect()),
        PyValue::Dict(entries) => PyValue::Dict(
            entries
                .iter()
                .map(|(val, key)| (deep_unpickle(val), key.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Find the first dict at any depth, for naming a push by its keys.
fn first_dict(v: &PyValue) -> Option<&Vec<(PyValue, PyValue)>> {
    match v {
        PyValue::Dict(d) => Some(d),
        PyValue::Tuple(items) | PyValue::List(items) => items.iter().find_map(first_dict),
        _ => None,
    }
}

/// A one-line name for a push, so the transcript can be skimmed for them.
fn push_name(packet: &MachoPacket, body: &PyValue) -> String {
    if let MachoAddress::Broadcast {
        broadcast_id,
        id_type,
        ..
    } = &packet.destination
    {
        return format!("{id_type}:{broadcast_id}");
    }
    match packet.command {
        MachoNetMsg::SessionInitialStateNotification | MachoNetMsg::SessionChangeNotification => {
            first_dict(body)
                .map(|d| format!("[{}]", dict_keys(d).join(", ")))
                .unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// The node id out of an `N=<nodeID>:<n>` object id.
pub fn object_node(object_id: &str) -> Option<i64> {
    let (nid, _) = object_id.split_once(':')?;
    let (prefix, value) = nid.split_once('=')?;
    if prefix != "N" {
        return None;
    }
    value.parse().ok()
}

/// Look a key up in a wire-order dict (entries are `(value, key)`).
pub fn dict_get<'a>(entries: &'a [(PyValue, PyValue)], key: &str) -> Option<&'a PyValue> {
    entries.iter().find_map(|(v, k)| match k {
        PyValue::Bytes(b) if b == key.as_bytes() => Some(v),
        PyValue::Unicode(s) if s == key => Some(v),
        _ => None,
    })
}

fn dict_str(entries: &[(PyValue, PyValue)], key: &str) -> Option<String> {
    match dict_get(entries, key)? {
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        PyValue::Unicode(s) => Some(s.clone()),
        PyValue::Int(i) => Some(i.to_string()),
        _ => None,
    }
}

/// A dict's keys, in wire order, for a transcript line.
pub fn dict_keys(entries: &[(PyValue, PyValue)]) -> Vec<String> {
    entries
        .iter()
        .map(|(_, k)| match k {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            PyValue::Unicode(s) => s.clone(),
            other => render::shape(other),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entries are `(value, key)`, so a lookup that reads them the other way
    /// round finds nothing — and would make every handshake field read as
    /// absent, which is a confusing way to fail.
    #[test]
    fn dict_lookup_uses_wire_order() {
        let d = vec![
            (PyValue::Int(42), PyValue::Bytes(b"userid".to_vec())),
            (
                PyValue::Unicode("EN".into()),
                PyValue::Unicode("languageID".into()),
            ),
        ];
        assert_eq!(dict_get(&d, "userid"), Some(&PyValue::Int(42)));
        assert_eq!(
            dict_get(&d, "languageID"),
            Some(&PyValue::Unicode("EN".into()))
        );
        assert_eq!(dict_get(&d, "42"), None);
        assert_eq!(dict_keys(&d), vec!["userid", "languageID"]);
    }

    #[test]
    fn object_ids_name_their_node() {
        assert_eq!(object_node("N=1000003120:7"), Some(1_000_003_120));
        assert_eq!(
            object_node("C=0:7"),
            None,
            "a client-hosted object is not ours to call"
        );
        assert_eq!(object_node("N=1000003120"), None, "no colon");
        assert_eq!(object_node("N=x:7"), None);
        assert_eq!(object_node(""), None);
    }

    /// A response payload is a pickle. Unwrapping it is not optional: the
    /// value inside is what the service returned, and the bytes outside are
    /// what `blue.marshal.Load` is handed.
    #[test]
    fn a_pickled_response_is_unwrapped() {
        let inner = PyValue::Tuple(vec![PyValue::Int(1), PyValue::Bytes(b"ok".to_vec())]);
        let pickled = PyValue::Bytes(crate::encode_stream(&inner));
        assert_eq!(unpickle(&pickled), inner);
        // Not a pickle: hand it back untouched rather than guessing.
        assert_eq!(unpickle(&PyValue::Int(7)), PyValue::Int(7));
        let garbage = PyValue::Bytes(b"xyz".to_vec());
        assert_eq!(unpickle(&garbage), garbage);
    }

    /// A pickle nested two levels down -- which is where a `Notification`
    /// keeps its payload -- must come out, or every push in the transcript
    /// reads as a wall of escaped bytes.
    #[test]
    fn nested_pickles_are_unwrapped_at_every_depth() {
        let inner = PyValue::Dict(vec![(PyValue::Int(5), PyValue::Bytes(b"charid".to_vec()))]);
        let notification = PyValue::Tuple(vec![PyValue::Tuple(vec![
            PyValue::Int(0),
            PyValue::Bytes(crate::encode_stream(&inner)),
        ])]);
        assert_eq!(
            deep_unpickle(&notification),
            PyValue::Tuple(vec![PyValue::Tuple(vec![PyValue::Int(0), inner])])
        );
    }

    /// Bytes that are not a marshal stream stay bytes. A portrait upload and
    /// a settings blob both travel as `str`, and turning either into a
    /// half-decoded tree would be worse than leaving it alone.
    #[test]
    fn ordinary_bytes_survive_unpickling() {
        for raw in [
            b"\xff\xd8\xff\xe0JFIF".to_vec(), // a JPEG
            b"\x78\x01\xab\x63".to_vec(),     // a zlib stream
            b"hello".to_vec(),
            Vec::new(),
        ] {
            let v = PyValue::Bytes(raw);
            assert_eq!(deep_unpickle(&v), v, "{v:?}");
        }
    }

    /// A push is named by the keys of the first dict in it, wherever that
    /// dict is: a session change keeps it inside a tuple inside a tuple.
    #[test]
    fn a_nested_dict_still_names_its_push() {
        let body = PyValue::Tuple(vec![
            PyValue::Int(1),
            PyValue::Tuple(vec![
                PyValue::Int(0),
                PyValue::Dict(vec![(PyValue::Int(9), PyValue::Bytes(b"charid".to_vec()))]),
            ]),
        ]);
        assert_eq!(
            first_dict(&body).map(|d| dict_keys(d)),
            Some(vec!["charid".to_string()])
        );
        assert_eq!(first_dict(&PyValue::Int(1)), None);
    }

    /// The SSO path puts a literal on the wire in place of the account name,
    /// and the VIP key is hashed over *that*, not over the account.
    #[test]
    fn the_sso_path_renames_the_user_on_the_wire() {
        let plain = Credentials {
            user: "probe".into(),
            password: "pw".into(),
            sso_token: None,
            language: "EN".into(),
        };
        assert_eq!(plain.wire_user(), "probe");
        let sso = Credentials {
            sso_token: Some("offline".into()),
            ..plain
        };
        assert_eq!(sso.wire_user(), "EVE-SSO-CONNECTION");
    }
}
