//! The GPS low-level version check and authentication handshake, server side.
//!
//! This is `GPSTransport.HandleClientAuthentication` (+ `Step1/2/3`) from
//! `carbon/common/script/net/GPS.pyc`, reimplemented against the payload table
//! in `docs/reference/login-path.md` §4. Ten payloads, numbered H1..H10 there and here.
//!
//! **Everything here is plain marshal values** — tuples, dicts, ints, reals and
//! strings. Not one handshake payload is a Python class instance, so the
//! unresolved object-wrapper type codes (`0x17`/`0x22`/`0x23`) do not block the
//! handshake. They block the `MachoPacket` traffic that follows it.
//!
//! **Encryption.** H6 onward are marked "E" in the table, meaning they pass
//! through `EncryptedWrite`/`EncryptedRead`. Under the Placebo crypto pack —
//! which is what `/client/start.ini` selects for us — those are the identity,
//! so this module reads and writes plaintext throughout. If the pack ever
//! changes, this is the module that has to care.
//!
//! The state machine is deliberately socket-free: it takes a parsed payload and
//! returns payloads to send. All the entropy it needs (the 64-byte server
//! challenge) is handed in at construction, so a test can drive the whole
//! handshake deterministically and the only thing left in `conn` is I/O.

use crate::accounts::{Accounts, Refused, Resolved};
use crate::server::Listener;
use crate::wire::crypto_hash;
use eve_marshal::build::{self as reg, ReleaseLine};
use eve_marshal::{PyValue, handshake as hs};
use std::net::SocketAddr;
use std::sync::Arc;

/// `handShakePaddingLength`: both challenges are exactly this long, and
/// `Step3` rejects a client challenge of any other length.
pub const CHALLENGE_LEN: usize = hs::PADDING_LENGTH;

/// What the client sends as element 0 of H4, and what the real server compares
/// against its own `publicKeyVersion`. Under Placebo it is this literal.
const CRYPTO_PACK: &[u8] = b"placebo";

/// H5 is this exact string, loaded and discarded by the client.
const OK_CC: &[u8] = b"OK CC";

/// H7: the server hardcodes 2. A 1 would make the client resend H6 with a
/// cleartext password instead of a hash, which we have no use for.
const PASSWORD_VERSION: i64 = 2;

/// The logon queue position we report. The client raises
/// `AutLogonFailureTotalUsers` at **2 or more**, so this must stay below that.
const LOGON_QUEUE_POSITION: i64 = 1;

// The client raises `AutLogonFailureTotalUsers` at 2 or more. Checked here
// rather than in a test so it cannot be edited past by accident.
const _: () = assert!(LOGON_QUEUE_POSITION < 2);

/// The client builds one server answers (R-BUILD §3.1, BA-4): one release
/// line, and the accepted builds on it. H1 is sent before the client says
/// anything, so it advertises the set's **minimum**; the client accepts any
/// server build not higher than its own, so every build in the set passes
/// its check. H2 then names the client's exact build.
#[derive(Debug, Clone, PartialEq)]
pub struct Builds {
    /// The line H1 and H8 describe; H2 must match it.
    pub line: &'static ReleaseLine,
    /// Ascending and non-empty. Strict mode refuses any other build.
    pub accepted: Vec<u32>,
}

impl Builds {
    /// The default: every build `eve_marshal::build::BUILDS` accepts on the
    /// reference build's line (`pinned` and `equivalent`).
    pub fn registered() -> Builds {
        let line = reg::reference().line;
        let accepted = reg::accepted()
            .into_iter()
            .filter(|b| reg::entry(*b).is_some_and(|e| e.line == line))
            .collect();
        Builds { line, accepted }
    }

    /// `--client-build N[,M]`: every build must be registered (any status:
    /// naming an `unverified` one is the explicit opt-in), and all on one
    /// line, because one H1 cannot serve two lines.
    pub fn parse(list: &str) -> Result<Builds, String> {
        Builds::parse_in(list, reg::BUILDS)
    }

    /// [`Builds::parse`] against `table` (a fixture in tests).
    fn parse_in(list: &str, table: &'static [reg::BuildEntry]) -> Result<Builds, String> {
        let mut accepted = Vec::new();
        let mut line: Option<&'static ReleaseLine> = None;
        for part in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let b: u32 = part
                .parse()
                .map_err(|e| format!("--client-build '{part}': {e}"))?;
            let e = table.iter().find(|e| e.build == b).ok_or_else(|| {
                format!(
                    "--client-build {b}: not in the build registry (registered: {}); \
                     measure it with tools/build-delta.py and add it to tools/builds.tsv \
                     and eve_marshal::build::BUILDS",
                    table
                        .iter()
                        .map(|e| format!("{} {}", e.build, e.status.name()))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            match line {
                Some(l) if l != e.line => {
                    return Err(format!(
                        "--client-build {b} is on line {}, the others on {}: one server \
                         process speaks one release line",
                        e.line.codename, l.codename
                    ));
                }
                _ => line = Some(e.line),
            }
            if !accepted.contains(&b) {
                accepted.push(b);
            }
        }
        accepted.sort_unstable();
        let line = line.ok_or("--client-build needs at least one build")?;
        Ok(Builds { line, accepted })
    }

    /// What H1 and H8 report: the lowest accepted build.
    pub fn advertised(&self) -> u32 {
        self.accepted.first().copied().unwrap_or(reg::REFERENCE)
    }

    /// Is `build` one of the accepted set?
    pub fn accepts(&self, build: i64) -> bool {
        self.accepted.iter().any(|b| i64::from(*b) == build)
    }

    /// For the start-up banner and the logs.
    pub fn describe(&self) -> String {
        format!(
            "line {} (macho {}), client builds {} (H1 advertises {})",
            self.line.codename_at_region,
            self.line.macho,
            self.accepted
                .iter()
                .map(|b| {
                    let st = reg::entry(*b).map_or("unregistered", |e| e.status.name());
                    format!("{b} {st}")
                })
                .collect::<Vec<_>>()
                .join(", "),
            self.advertised()
        )
    }
}

/// Settings a connection needs to answer the handshake.
#[derive(Debug, Clone)]
pub struct Config {
    /// `cluster_usercount`. Cosmetic; the client only displays it.
    pub user_count: i64,
    /// `proxy_nodeid`. The client stores it as `myProxyNodeID` and addresses
    /// its first real service call to it, so it must be a node id we answer
    /// for.
    pub proxy_node_id: i64,
    /// **The catch-all account's id** (`--user-id`): `session_init['userid']`
    /// for every connection when [`Config::accounts`] is `None`.
    ///
    /// **An account id.** Never a character id (that is
    /// `session::Session::character_id`) and never a connection id (that is
    /// `session::Session::client_id`, minted per connection). This one value
    /// is copied to three places by `server::Config::with_handshake`
    /// precisely so they cannot disagree. With accounts, the id is resolved
    /// per connection in H6 ([`Handshake::resolved`]) and H10, the session
    /// and the select screen all read that one value
    /// (`docs/lanes/account-spec.md` §5).
    pub user_id: i64,
    /// `--accounts FILE`. `None` is the catch-all: one `gm` account,
    /// [`Config::user_id`], for every token. `Some` also turns on the
    /// strict build check ([`Config::strict_build`]).
    pub accounts: Option<Arc<Accounts>>,
    /// The listener this connection arrived on, for its ceiling and its
    /// `account=`. `server::serve_one` sets it per connection.
    pub listener: Listener,
    /// The connection's peer address, for the world accounts' per-peer
    /// backoff. `server::serve_one` sets it per connection.
    pub peer: Option<std::net::IpAddr>,
    /// `access_token`, required on the SSO path and indexed directly.
    pub access_token: Vec<u8>,
    /// `computer_hash`, required on the SSO path; later passed to
    /// `multiLoginBlocker.Login`.
    pub computer_hash: Vec<u8>,
    /// The marshalled code object for H8's `signedFunc`. See `crate::pycode`
    /// for what it is and why a trivial one suffices.
    pub signed_func: Vec<u8>,
    /// The release line and accepted client builds (`--client-build`).
    pub builds: Builds,
}

impl Config {
    /// Refuse every client build outside [`Config::builds`] (ruling 1,
    /// `account-spec.md` §4.5; BA-4 widened "ours" to the accepted set).
    /// Exactly when `--accounts` is given, so the two cannot disagree:
    /// derived, not a field.
    pub fn strict_build(&self) -> bool {
        self.accounts.is_some()
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            user_count: 1,
            proxy_node_id: 1,
            user_id: 1,
            accounts: None,
            listener: Listener::new(SocketAddr::from(([127, 0, 0, 1], 0))),
            peer: None,
            access_token: b"eve-offline".to_vec(),
            computer_hash: b"eve-offline".to_vec(),
            signed_func: crate::pycode::handshake_func(),
            builds: Builds::registered(),
        }
    }
}

/// Where in the ten-payload sequence a connection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// H1 not sent yet.
    Greeting,
    /// Sent H1, expecting H2.
    AwaitVersion,
    /// Expecting H3 (`VK`) or H3' (`QC`).
    AwaitUserKey,
    /// Expecting H4, the crypto-pack line.
    AwaitCryptoPack,
    /// Sent H5, expecting H6.
    AwaitAuthRequest,
    /// Sent H7 and H8, expecting H9.
    AwaitAuthResponse,
    /// Sent H10. The connection now speaks machoNet.
    Authenticated,
    /// A status probe was answered, or the handshake failed.
    Closed,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Greeting => "greeting",
            Phase::AwaitVersion => "await-version",
            Phase::AwaitUserKey => "await-user-key",
            Phase::AwaitCryptoPack => "await-crypto-pack",
            Phase::AwaitAuthRequest => "await-auth-request",
            Phase::AwaitAuthResponse => "await-auth-response",
            Phase::Authenticated => "authenticated",
            Phase::Closed => "closed",
        }
    }
}

/// The most characters of a client-supplied string a refusal (and so a log
/// line) carries **[A]**. The client has no limit to cite: a legitimate
/// value here is a pack name, a version or a build, all a few characters.
/// Falsified by a real mismatch being unreadable at 64.
pub const LOGGED_TEXT_MAX: usize = 64;

/// `s` made safe to print in a log line (SEC-FIX 12): every control
/// character, quote and backslash escaped (`escape_default`, so a newline
/// cannot start a fake log line and an escape sequence cannot reach a
/// terminal), and at most [`LOGGED_TEXT_MAX`] characters of it kept.
pub fn loggable(s: &str) -> String {
    let mut out: String = s
        .chars()
        .take(LOGGED_TEXT_MAX)
        .flat_map(char::escape_default)
        .collect();
    let more = s.chars().count().saturating_sub(LOGGED_TEXT_MAX);
    if more > 0 {
        out.push_str(&format!("...(+{more})"));
    }
    out
}

/// [`loggable`] for bytes the client sent.
pub fn loggable_bytes(b: &[u8]) -> String {
    loggable(&String::from_utf8_lossy(
        &b[..b.len().min(LOGGED_TEXT_MAX * 4)],
    ))
}

/// Why a handshake was refused.
///
/// These mirror the server-side checks that produce the client's
/// `'Low-level version check failure while accepting from …'`. We refuse
/// loudly rather than tolerantly: a tolerant server here would let a framing
/// or marshal bug through and surface it much later as an unexplained
/// disconnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeError {
    /// The payload was not the shape this phase expects.
    Unexpected { phase: &'static str, why: String },
    /// A version field did not match.
    VersionMismatch {
        field: &'static str,
        expected: String,
        got: String,
    },
    /// The client's build is older than ours. Mirrors the server's
    /// `boot.build > request['boot_build']` staleness check.
    StaleClient { client_build: i64, our_build: i64 },
    /// `len(clientChallenge) != handShakePaddingLength`.
    BadChallengeLength { len: usize },
    /// H4 element 0 was not our crypto pack.
    BadCryptoPack { got: String },
    /// H9's hash of our server challenge did not match what we computed.
    ChallengeResponseMismatch { expected: String, got: String },
    /// Strict mode (`--accounts`): the client's build is not in the
    /// accepted set, older or newer (ruling 1). A newer build fails in
    /// confusing ways past the handshake, so it is refused here with the
    /// advertised build (`ours`), the client's, and the whole set.
    WrongBuild {
        ours: i64,
        theirs: i64,
        accepted: Vec<i64>,
    },
    /// H6's token, or this listener's `account=`, names no account. The
    /// connection closes; the log line names the listener and at most the
    /// first 8 hex digits of the token's hash, never the token.
    UnknownAccount { listener: SocketAddr, why: Refused },
}

impl core::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HandshakeError::Unexpected { phase, why } => {
                write!(f, "unexpected payload in {phase}: {why}")
            }
            HandshakeError::VersionMismatch {
                field,
                expected,
                got,
            } => write!(f, "{field}: expected {expected}, client sent {got}"),
            HandshakeError::StaleClient {
                client_build,
                our_build,
            } => write!(
                f,
                "client build {client_build} is older than ours ({our_build})"
            ),
            HandshakeError::BadChallengeLength { len } => {
                write!(
                    f,
                    "client challenge is {len} bytes, expected {CHALLENGE_LEN}"
                )
            }
            HandshakeError::BadCryptoPack { got } => write!(f, "crypto pack {got} is not placebo"),
            HandshakeError::ChallengeResponseMismatch { expected, got } => {
                write!(f, "challenge response {got}, expected {expected}")
            }
            HandshakeError::WrongBuild {
                ours,
                theirs,
                accepted,
            } => write!(
                f,
                "client build {theirs} is not accepted (H1 advertised {ours}); with \
                 --accounts only builds {} may log in",
                accepted
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            HandshakeError::UnknownAccount { listener, why } => {
                write!(f, "listener {listener} refused the login: {why}")
            }
        }
    }
}

impl std::error::Error for HandshakeError {}

/// What to do after feeding in a payload.
///
/// `PyValue` carries an `f64`, so this is `PartialEq` but not `Eq`.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// Payloads to send, in order, each as its own frame.
    pub replies: Vec<PyValue>,
    /// Close the connection once the replies are flushed. Set for the `QC`
    /// status probe, which is a complete exchange on its own.
    pub close: bool,
}

/// Who the client says it is. Recorded from H6, not validated: we accept any
/// login, since the point is a private server for people who already hold
/// subscriptions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientIdentity {
    pub user_name: Option<Vec<u8>>,
    pub language_id: Option<Vec<u8>>,
    pub sso_token: Option<Vec<u8>>,
    /// The client's own reported build, from H2's `boot_build` (H6 repeats
    /// it). The session carries it (`Session::client_build`).
    pub boot_build: Option<i64>,
}

/// The server half of one connection's handshake.
#[derive(Debug, Clone)]
pub struct Handshake {
    phase: Phase,
    config: Config,
    server_challenge: [u8; CHALLENGE_LEN],
    client_challenge: Option<Vec<u8>>,
    identity: ClientIdentity,
    /// The account, resolved once in H6.
    resolved: Option<Resolved>,
    /// World accounts: [`Handshake::pre_login`]'s answer, taken by H6.
    verdict: Option<Result<Resolved, Refused>>,
}

impl Handshake {
    /// `server_challenge` is handed in rather than generated here so the whole
    /// handshake is a deterministic function of its inputs. Under Placebo it
    /// carries no security weight — the client only hashes it back with a
    /// 16-bit checksum — but it must be exactly [`CHALLENGE_LEN`] bytes.
    /// The node id we answer as, which the client stores as `myProxyNodeID`.
    /// Bound-object ids have to name the same node, or a later call on the
    /// object would be addressed to a node that is not us.
    pub fn node_id(&self) -> i64 {
        self.config.proxy_node_id
    }

    pub fn new(config: Config, server_challenge: [u8; CHALLENGE_LEN]) -> Self {
        Handshake {
            phase: Phase::Greeting,
            config,
            server_challenge,
            client_challenge: None,
            identity: ClientIdentity::default(),
            resolved: None,
            verdict: None,
        }
    }

    /// World accounts (ACC2): decide H6's login **before** the dispatcher
    /// takes the world lock for the turn, because the password hash is slow
    /// and `crate::account_book::login` takes the lock only briefly. Does
    /// nothing in the other modes or at any other phase.
    pub fn pre_login(&mut self, v: &PyValue, world: &std::sync::RwLock<crate::service::Registry>) {
        if self.phase != Phase::AwaitAuthRequest {
            return;
        }
        let Some(book) = self.config.accounts.as_ref().and_then(|a| a.book()) else {
            return;
        };
        let token = match v {
            PyValue::Tuple(t) if t.len() == 2 => {
                dict_get(&t[1], b"user_sso_token").and_then(as_bytes)
            }
            _ => None,
        };
        let verdict =
            crate::account_book::login(book, &self.config.listener, self.config.peer, token, world);
        if let Err(why) = &verdict {
            crate::errln!(
                "[accounts] listener {} refused a login: {why}",
                self.config.listener.addr
            );
        }
        self.verdict = Some(verdict);
    }

    /// The account this connection logged in as, from H6 on: the one value
    /// H10's `userid`, the session's `userid` and `role`, and the select
    /// screen's `userID` all come from (`account-spec.md` §5).
    pub fn resolved(&self) -> Option<Resolved> {
        self.resolved
    }

    /// Whether each account is one person (NET-TAKEOVER): true for an
    /// accounts file or `--world-accounts`, false for the catch-all, which
    /// gives every token one shared user id.
    pub fn accounts_are_distinct(&self) -> bool {
        self.config
            .accounts
            .as_ref()
            .is_some_and(|a| !a.is_catch_all())
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn identity(&self) -> &ClientIdentity {
        &self.identity
    }

    /// H1, the payload the server sends unprompted on accept.
    ///
    /// A 7-tuple. Element types are checked strictly by the client's
    /// `Authenticate`, and `boot_version` in particular must be a **real**:
    /// marshalling 24.01 as anything else is a `HANDSHAKE_INCOMPATIBLEVERSION`.
    ///
    /// `build` is the accepted set's minimum ([`Builds::advertised`]).
    pub fn greeting(&mut self) -> PyValue {
        self.phase = Phase::AwaitVersion;
        let line = self.config.builds.line;
        PyValue::Tuple(vec![
            PyValue::Int(i64::from(line.secret)),
            PyValue::Int(i64::from(line.macho)),
            PyValue::Int(self.config.user_count),
            PyValue::Real(line.version),
            PyValue::Int(i64::from(self.config.builds.advertised())),
            PyValue::Bytes(line.codename_at_region.as_bytes().to_vec()),
            // update_info: stored by the client and never acted on.
            PyValue::None,
        ])
    }

    /// Feed one received payload. Returns the payloads to send back.
    pub fn on_payload(&mut self, v: &PyValue) -> Result<Outcome, HandshakeError> {
        match self.phase {
            Phase::AwaitVersion => self.on_version(v),
            Phase::AwaitUserKey => self.on_user_key(v),
            Phase::AwaitCryptoPack => self.on_crypto_pack(v),
            Phase::AwaitAuthRequest => self.on_auth_request(v),
            Phase::AwaitAuthResponse => self.on_auth_response(v),
            phase => Err(HandshakeError::Unexpected {
                phase: phase.name(),
                why: "the handshake expects nothing here".to_owned(),
            }),
        }
    }

    /// H2: the client echoes the version tuple, minus `update_info`.
    fn on_version(&mut self, v: &PyValue) -> Result<Outcome, HandshakeError> {
        let t = expect_tuple(v, self.phase)?;
        if t.len() != 6 {
            return Err(HandshakeError::Unexpected {
                phase: self.phase.name(),
                why: format!("expected a 6-tuple, got {} elements", t.len()),
            });
        }
        let builds = &self.config.builds;
        let line = builds.line;
        expect_int(&t[0], "secret_key", i64::from(line.secret))?;
        expect_int(&t[1], "macho_version", i64::from(line.macho))?;
        // t[2] is the client's user count; the real server does not act on it.
        expect_real(&t[3], "boot_version", line.version)?;
        let build = expect_build(&t[4])?;
        let our_build = i64::from(builds.advertised());
        if self.config.strict_build() && !builds.accepts(build) {
            return Err(HandshakeError::WrongBuild {
                ours: our_build,
                theirs: build,
                accepted: builds.accepted.iter().map(|b| i64::from(*b)).collect(),
            });
        }
        if our_build > build {
            return Err(HandshakeError::StaleClient {
                client_build: build,
                our_build,
            });
        }
        expect_str(
            &t[5],
            "boot_codename@region",
            line.codename_at_region.as_bytes(),
        )?;
        if !builds.accepts(build) {
            // Catch-all mode lets a newer, unmeasured build in: the client's
            // own check passed, and refusing would only hide what breaks.
            // Loudly, so the log says which build it was.
            crate::errln!(
                "[handshake] client build {build} is not in the accepted set ({}); \
                 catch-all mode lets it in. Measure it with tools/build-delta.py",
                builds.describe()
            );
        }
        self.identity.boot_build = Some(build);
        self.phase = Phase::AwaitUserKey;
        Ok(Outcome {
            replies: vec![],
            close: false,
        })
    }

    /// H3 `(None, 'VK', hash)` continues the login; H3' `(None, 'QC')` is a
    /// status probe that gets one integer and a disconnect.
    fn on_user_key(&mut self, v: &PyValue) -> Result<Outcome, HandshakeError> {
        let t = expect_tuple(v, self.phase)?;
        match t.get(1).and_then(as_bytes) {
            Some(b"QC") => {
                self.phase = Phase::Closed;
                Ok(Outcome {
                    replies: vec![PyValue::Int(0)],
                    close: true,
                })
            }
            Some(b"VK") => {
                self.phase = Phase::AwaitCryptoPack;
                Ok(Outcome {
                    replies: vec![],
                    close: false,
                })
            }
            other => Err(HandshakeError::Unexpected {
                phase: self.phase.name(),
                why: format!(
                    "element 1 should be 'VK' or 'QC', got {}",
                    other.map_or_else(|| "nothing".to_owned(), loggable_bytes)
                ),
            }),
        }
    }

    /// H4 names the crypto pack; H5 acknowledges it.
    fn on_crypto_pack(&mut self, v: &PyValue) -> Result<Outcome, HandshakeError> {
        let t = expect_tuple(v, self.phase)?;
        let pack = t.first().and_then(as_bytes).unwrap_or(b"");
        if pack != CRYPTO_PACK {
            return Err(HandshakeError::BadCryptoPack {
                got: loggable_bytes(pack),
            });
        }
        self.phase = Phase::AwaitAuthRequest;
        Ok(Outcome {
            replies: vec![PyValue::Bytes(OK_CC.to_vec())],
            close: false,
        })
    }

    /// H6 carries the client challenge and the login request; we answer with
    /// H7 then H8, as two separate payloads.
    fn on_auth_request(&mut self, v: &PyValue) -> Result<Outcome, HandshakeError> {
        let t = expect_tuple(v, self.phase)?;
        if t.len() != 2 {
            return Err(HandshakeError::Unexpected {
                phase: self.phase.name(),
                why: format!("expected a 2-tuple, got {} elements", t.len()),
            });
        }
        let challenge = as_bytes(&t[0]).ok_or_else(|| HandshakeError::Unexpected {
            phase: self.phase.name(),
            why: "element 0 should be the client challenge string".to_owned(),
        })?;
        if challenge.len() != CHALLENGE_LEN {
            return Err(HandshakeError::BadChallengeLength {
                len: challenge.len(),
            });
        }

        // Re-check the version fields, as Step3 does. The client has now sent
        // them twice, in two different shapes; a disagreement between H2 and
        // H6 means something is wrong with our marshalling, not with the
        // client, so it is worth catching.
        let line = self.config.builds.line;
        if let Some(d) = dict_get(&t[1], b"macho_version") {
            expect_int(d, "macho_version", i64::from(line.macho))?;
        }
        if let Some(d) = dict_get(&t[1], b"boot_version") {
            expect_real(d, "boot_version", line.version)?;
        }
        if let Some(d) = dict_get(&t[1], b"boot_codename") {
            expect_str(d, "boot_codename", line.codename.as_bytes())?;
        }
        if let Some(d) = dict_get(&t[1], b"boot_region") {
            expect_str(d, "boot_region", line.region.as_bytes())?;
        }

        self.identity.user_name = dict_get(&t[1], b"user_name")
            .and_then(as_bytes)
            .map(<[u8]>::to_vec);
        self.identity.language_id = dict_get(&t[1], b"user_languageid")
            .and_then(as_bytes)
            .map(<[u8]>::to_vec);
        self.identity.sso_token = dict_get(&t[1], b"user_sso_token")
            .and_then(as_bytes)
            .map(<[u8]>::to_vec);
        self.client_challenge = Some(challenge.to_vec());

        // Who this is, once, here: the token is known from H6 on and H10
        // needs the answer (`account-spec.md` §2.1, §4.1). Without
        // `--accounts` the catch-all answers `user_id` for every token.
        let catch_all;
        let accounts = match &self.config.accounts {
            Some(a) => a.as_ref(),
            None => {
                catch_all = Accounts::catch_all(self.config.user_id);
                &catch_all
            }
        };
        let in_world = accounts.book().is_some();
        let resolved = if in_world {
            self.verdict.take().unwrap_or(Err(Refused::NoToken))
        } else {
            accounts.resolve(&self.config.listener, self.identity.sso_token.as_deref())
        };
        match resolved {
            Ok(r) => self.resolved = Some(r),
            // World accounts tell the client why, in place of H7, and close
            // (`crate::account_book::refusal`); the log line was written by
            // `pre_login`.
            Err(why) if in_world => {
                self.phase = Phase::Closed;
                return Ok(Outcome {
                    replies: vec![crate::account_book::refusal(&why)],
                    close: true,
                });
            }
            Err(why) => {
                self.phase = Phase::Closed;
                return Err(HandshakeError::UnknownAccount {
                    listener: self.config.listener.addr,
                    why,
                });
            }
        }

        let response = self.h8_response_dict(challenge);
        self.phase = Phase::AwaitAuthResponse;
        Ok(Outcome {
            replies: vec![
                PyValue::Int(PASSWORD_VERSION),
                PyValue::Tuple(vec![
                    PyValue::Bytes(self.server_challenge.to_vec()),
                    // signedFunc: (marshalled code object, 0). Under Placebo
                    // `Crypto.Verify` hands element 0 straight to
                    // `marshal.loads`.
                    PyValue::Tuple(vec![
                        PyValue::Bytes(self.config.signed_func.clone()),
                        PyValue::Int(0),
                    ]),
                    // The `eval` locals dict. The real server sends it empty.
                    PyValue::Dict(vec![]),
                    response,
                ]),
            ],
            close: false,
        })
    }

    fn h8_response_dict(&self, client_challenge: &[u8]) -> PyValue {
        let line = self.config.builds.line;
        pydict(vec![
            (
                b"challenge_responsehash".as_slice(),
                PyValue::Bytes(
                    crypto_hash(&[PyValue::Bytes(client_challenge.to_vec())]).into_bytes(),
                ),
            ),
            (b"macho_version", PyValue::Int(i64::from(line.macho))),
            (b"boot_version", PyValue::Real(line.version)),
            (
                b"boot_build",
                PyValue::Int(i64::from(self.config.builds.advertised())),
            ),
            (
                b"boot_codename",
                PyValue::Bytes(line.codename.as_bytes().to_vec()),
            ),
            (
                b"boot_region",
                PyValue::Bytes(line.region.as_bytes().to_vec()),
            ),
            (b"cluster_usercount", PyValue::Int(self.config.user_count)),
            (b"proxy_nodeid", PyValue::Int(self.config.proxy_node_id)),
            (
                b"user_logonqueueposition",
                PyValue::Int(LOGON_QUEUE_POSITION),
            ),
            (b"config_vals", PyValue::Dict(vec![])),
        ])
    }

    /// H9 proves the client ran our function; H10 completes the handshake.
    fn on_auth_response(&mut self, v: &PyValue) -> Result<Outcome, HandshakeError> {
        let t = expect_tuple(v, self.phase)?;
        let got = t.first().and_then(as_bytes).unwrap_or(b"");
        let expected = crypto_hash(&[PyValue::Bytes(self.server_challenge.to_vec())]);
        if got != expected.as_bytes() {
            return Err(HandshakeError::ChallengeResponseMismatch {
                expected,
                got: loggable_bytes(got),
            });
        }
        self.phase = Phase::Authenticated;
        Ok(Outcome {
            replies: vec![self.h10_result()],
            close: false,
        })
    }

    /// The resolved account's id; the catch-all's before H6.
    fn user_id(&self) -> i64 {
        self.resolved.map_or(self.config.user_id, |r| r.user_id)
    }

    /// H10. The client merges this over H8's dict, so repeating
    /// `proxy_nodeid` is harmless and keeps each payload self-contained.
    ///
    /// `live_updates` is deliberately absent: if present, the client scatters
    /// each element's `.code` as `OnLiveClientUpdate`, which is one more thing
    /// to go wrong for no gain.
    fn h10_result(&self) -> PyValue {
        pydict(vec![
            (
                b"session_init".as_slice(),
                pydict(vec![(b"userid", PyValue::Int(self.user_id()))]),
            ),
            (b"proxy_nodeid", PyValue::Int(self.config.proxy_node_id)),
            (
                b"access_token",
                PyValue::Bytes(self.config.access_token.clone()),
            ),
            (
                b"computer_hash",
                PyValue::Bytes(self.config.computer_hash.clone()),
            ),
            (b"config_vals", PyValue::Dict(vec![])),
        ])
    }
}

// --- payload helpers -------------------------------------------------------

/// Build a marshal dict from logical `(key, value)` pairs.
///
/// **The wire order is value-then-key**, confirmed from the dict handler at
/// `0x1802dd1d0`, so `PyValue::Dict` stores `(value, key)`. Every dict this
/// module builds goes through here precisely so that inversion is written down
/// once instead of being re-derived at each call site.
fn pydict(kv: Vec<(&[u8], PyValue)>) -> PyValue {
    PyValue::Dict(
        kv.into_iter()
            .map(|(k, v)| (v, PyValue::Bytes(k.to_vec())))
            .collect(),
    )
}

/// Look a key up in a marshal dict.
///
/// Accepts a `str` or a `unicode` key. The client builds these dicts with
/// `str` keys, but a key arriving as `unicode` is a difference we would rather
/// absorb than reject, since it changes nothing about the value.
fn dict_get<'a>(v: &'a PyValue, key: &[u8]) -> Option<&'a PyValue> {
    match v {
        PyValue::Dict(entries) => entries
            .iter()
            .find(|(_, k)| as_bytes(k) == Some(key))
            .map(|(value, _)| value),
        _ => None,
    }
}

/// A Python 2 `str` or `unicode` as bytes. `unicode` is already UTF-8 here, and
/// every name the handshake compares is ASCII, so the two coincide.
fn as_bytes(v: &PyValue) -> Option<&[u8]> {
    match v {
        PyValue::Bytes(b) => Some(b),
        PyValue::Unicode(s) => Some(s.as_bytes()),
        _ => None,
    }
}

fn expect_tuple(v: &PyValue, phase: Phase) -> Result<&[PyValue], HandshakeError> {
    match v {
        PyValue::Tuple(t) => Ok(t),
        other => Err(HandshakeError::Unexpected {
            phase: phase.name(),
            why: format!("expected a tuple, got {}", type_name(other)),
        }),
    }
}

fn expect_int(v: &PyValue, field: &'static str, want: i64) -> Result<(), HandshakeError> {
    match v {
        PyValue::Int(got) if *got == want => Ok(()),
        other => Err(HandshakeError::VersionMismatch {
            field,
            expected: want.to_string(),
            got: describe(other),
        }),
    }
}

/// The client's check is `boot.version != r[3]` on a Python float, so this is
/// an exact comparison and not a tolerance. Marshalling 24.01 as an integer or
/// a string is the mistake this catches.
fn expect_real(v: &PyValue, field: &'static str, want: f64) -> Result<(), HandshakeError> {
    match v {
        PyValue::Real(got) if *got == want => Ok(()),
        other => Err(HandshakeError::VersionMismatch {
            field,
            expected: want.to_string(),
            got: describe(other),
        }),
    }
}

fn expect_str(v: &PyValue, field: &'static str, want: &[u8]) -> Result<(), HandshakeError> {
    match as_bytes(v) {
        Some(got) if got == want => Ok(()),
        _ => Err(HandshakeError::VersionMismatch {
            field,
            expected: String::from_utf8_lossy(want).into_owned(),
            got: describe(v),
        }),
    }
}

/// `boot_build` is typed `int | unicode | str` by the server's own check, so a
/// build arriving as a string is legal and must be parsed, not refused.
fn expect_build(v: &PyValue) -> Result<i64, HandshakeError> {
    match v {
        PyValue::Int(n) => Ok(*n),
        _ => match as_bytes(v).and_then(|b| std::str::from_utf8(b).ok()) {
            Some(s) => s
                .trim()
                .parse::<i64>()
                .map_err(|_| HandshakeError::VersionMismatch {
                    field: "boot_build",
                    expected: "an integer".to_owned(),
                    got: loggable(s),
                }),
            None => Err(HandshakeError::VersionMismatch {
                field: "boot_build",
                expected: "an integer".to_owned(),
                got: describe(v),
            }),
        },
    }
}

fn type_name(v: &PyValue) -> &'static str {
    match v {
        PyValue::None => "None",
        PyValue::Bool(_) => "bool",
        PyValue::Int(_) => "int",
        PyValue::Long(_) => "long",
        PyValue::Real(_) => "real",
        PyValue::Bytes(_) => "str",
        PyValue::Unicode(_) => "unicode",
        PyValue::Tuple(_) => "tuple",
        PyValue::List(_) => "list",
        PyValue::Dict(_) => "dict",
        PyValue::Instance { .. } => "instance",
        PyValue::ObjectEx { .. } => "object",
        PyValue::Global(_) => "global",
        PyValue::DbRow(_) => "dbrow",
        PyValue::SubStream(_) => "substream",
        PyValue::Callback(_) => "callback",
    }
}

fn describe(v: &PyValue) -> String {
    match v {
        PyValue::Int(n) => n.to_string(),
        PyValue::Real(x) => x.to_string(),
        PyValue::Bytes(b) => loggable_bytes(b),
        PyValue::Unicode(s) => loggable(s),
        other => type_name(other).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::crypto_hash;

    const CHALLENGE: [u8; CHALLENGE_LEN] = [0xAB; CHALLENGE_LEN];

    fn new_handshake() -> Handshake {
        Handshake::new(Config::default(), CHALLENGE)
    }

    fn bstr(s: &str) -> PyValue {
        PyValue::Bytes(s.as_bytes().to_vec())
    }

    /// H2 as the real client builds it.
    fn h2() -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(i64::from(hs::SECRET_KEY)),
            PyValue::Int(i64::from(hs::MACHO_VERSION)),
            PyValue::Int(0),
            PyValue::Real(hs::BOOT_VERSION),
            PyValue::Int(i64::from(hs::BOOT_BUILD)),
            bstr(hs::CODENAME_AT_REGION),
        ])
    }

    fn h3_vk() -> PyValue {
        PyValue::Tuple(vec![PyValue::None, bstr("VK"), bstr("hashed-name")])
    }

    fn h4() -> PyValue {
        PyValue::Tuple(vec![bstr("placebo"), PyValue::Dict(vec![])])
    }

    fn request_dict() -> PyValue {
        request_dict_with("a-token")
    }

    fn request_dict_with(token: &str) -> PyValue {
        pydict(vec![
            (
                b"macho_version".as_slice(),
                PyValue::Int(i64::from(hs::MACHO_VERSION)),
            ),
            (
                b"boot_version",
                PyValue::Real(reg::reference().line.version),
            ),
            (b"boot_build", PyValue::Int(i64::from(reg::REFERENCE))),
            (b"boot_codename", bstr(reg::reference().line.codename)),
            (b"boot_region", bstr("ccp")),
            (b"user_name", bstr("EVE-SSO-CONNECTION")),
            (b"user_password", PyValue::None),
            (b"user_languageid", bstr("EN")),
            (b"user_affiliateid", PyValue::Int(0)),
            (b"user_sso_token", bstr(token)),
        ])
    }

    fn h6(challenge: &[u8]) -> PyValue {
        PyValue::Tuple(vec![PyValue::Bytes(challenge.to_vec()), request_dict()])
    }

    /// Drive a handshake to `Authenticated`, returning it and every payload
    /// the server sent, H1 first.
    fn walk() -> (Handshake, Vec<PyValue>) {
        let mut h = new_handshake();
        let mut sent = vec![h.greeting()];
        for payload in [h2(), h3_vk(), h4()] {
            sent.extend(h.on_payload(&payload).unwrap().replies);
        }
        let client_challenge = [0x11u8; CHALLENGE_LEN];
        sent.extend(h.on_payload(&h6(&client_challenge)).unwrap().replies);
        let h9 = PyValue::Tuple(vec![
            PyValue::Bytes(crypto_hash(&[PyValue::Bytes(CHALLENGE.to_vec())]).into_bytes()),
            bstr(""),
            PyValue::Dict(vec![]),
        ]);
        sent.extend(h.on_payload(&h9).unwrap().replies);
        (h, sent)
    }

    #[test]
    fn the_full_handshake_authenticates() {
        let (h, sent) = walk();
        assert_eq!(h.phase(), Phase::Authenticated);
        // H1, H5, H7, H8, H10 — the five payloads the server originates.
        assert_eq!(sent.len(), 5, "sent {sent:?}");
        assert_eq!(
            h.identity().user_name.as_deref(),
            Some(b"EVE-SSO-CONNECTION".as_slice())
        );
        assert_eq!(
            h.identity().sso_token.as_deref(),
            Some(b"a-token".as_slice())
        );
        assert_eq!(h.identity().boot_build, Some(i64::from(reg::REFERENCE)));
    }

    #[test]
    fn h1_is_a_seven_tuple_with_boot_version_as_a_real() {
        let mut h = new_handshake();
        let PyValue::Tuple(t) = h.greeting() else {
            panic!("H1 is not a tuple")
        };
        assert_eq!(t.len(), 7);
        assert_eq!(t[0], PyValue::Int(i64::from(reg::reference().line.secret)));
        assert_eq!(t[1], PyValue::Int(i64::from(reg::reference().line.macho)));
        // This is the assertion that matters: an Int here is
        // HANDSHAKE_INCOMPATIBLEVERSION, and nothing else in the tuple would
        // tell us which field was wrong.
        assert!(
            matches!(t[3], PyValue::Real(_)),
            "boot_version must be a real"
        );
        // The registry's line minimum, so every accepted client passes.
        assert_eq!(
            t[4],
            PyValue::Int(i64::from(reg::line_min_build(reg::reference().line)))
        );
        assert_eq!(t[5], bstr(reg::reference().line.codename_at_region));
        assert_eq!(t[6], PyValue::None);
    }

    #[test]
    fn h5_is_the_literal_ok_cc() {
        let mut h = new_handshake();
        h.greeting();
        h.on_payload(&h2()).unwrap();
        h.on_payload(&h3_vk()).unwrap();
        let out = h.on_payload(&h4()).unwrap();
        assert_eq!(out.replies, vec![bstr("OK CC")]);
        assert!(!out.close);
    }

    #[test]
    fn h8_hashes_the_client_challenge_as_a_one_tuple() {
        let (_, sent) = walk();
        let PyValue::Tuple(h8) = &sent[3] else {
            panic!("H8 is not a tuple: {:?}", sent[3])
        };
        assert_eq!(h8.len(), 4);
        assert_eq!(h8[0], PyValue::Bytes(CHALLENGE.to_vec()));
        assert_eq!(
            h8[2],
            PyValue::Dict(vec![]),
            "the eval locals must be empty"
        );

        let client_challenge = vec![0x11u8; CHALLENGE_LEN];
        let want = crypto_hash(&[PyValue::Bytes(client_challenge.clone())]);
        let got = dict_get(&h8[3], b"challenge_responsehash").unwrap();
        assert_eq!(got, &PyValue::Bytes(want.into_bytes()));

        // Falsification: hashing the bare challenge instead of the 1-tuple
        // containing it must give a different answer, or this test proves
        // nothing about the tuple.
        let bare = crate::wire::crc_hqx(&crate::wire::dumps(&PyValue::Bytes(client_challenge)), 0)
            .to_string();
        assert_ne!(got, &PyValue::Bytes(bare.into_bytes()));
    }

    #[test]
    fn h8_keeps_the_logon_queue_below_the_clients_refusal_threshold() {
        let (_, sent) = walk();
        let PyValue::Tuple(h8) = &sent[3] else {
            panic!("H8 is not a tuple")
        };
        let pos = dict_get(&h8[3], b"user_logonqueueposition").unwrap();
        // The client raises AutLogonFailureTotalUsers at >= 2; the bound
        // itself is asserted at compile time beside the constant.
        assert_eq!(pos, &PyValue::Int(LOGON_QUEUE_POSITION));
    }

    #[test]
    fn h10_carries_the_keys_the_client_indexes_directly() {
        let (_, sent) = walk();
        let h10 = sent.last().unwrap();
        let session_init = dict_get(h10, b"session_init").expect("session_init");
        assert_eq!(dict_get(session_init, b"userid"), Some(&PyValue::Int(1)));
        assert!(dict_get(h10, b"proxy_nodeid").is_some());
        assert!(dict_get(h10, b"access_token").is_some());
        assert!(dict_get(h10, b"computer_hash").is_some());
        // live_updates must be absent: present, it is scattered as
        // OnLiveClientUpdate for no benefit to us.
        assert!(dict_get(h10, b"live_updates").is_none());
    }

    #[test]
    fn the_qc_status_probe_answers_once_and_closes() {
        let mut h = new_handshake();
        h.greeting();
        h.on_payload(&h2()).unwrap();
        let out = h
            .on_payload(&PyValue::Tuple(vec![PyValue::None, bstr("QC")]))
            .unwrap();
        assert_eq!(out.replies, vec![PyValue::Int(0)]);
        assert!(out.close);
        assert_eq!(h.phase(), Phase::Closed);
    }

    #[test]
    fn a_wrong_macho_version_is_refused() {
        let mut h = new_handshake();
        h.greeting();
        let PyValue::Tuple(mut t) = h2() else {
            unreachable!()
        };
        t[1] = PyValue::Int(495);
        let err = h.on_payload(&PyValue::Tuple(t)).unwrap_err();
        assert!(
            matches!(
                err,
                HandshakeError::VersionMismatch {
                    field: "macho_version",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn boot_version_sent_as_an_int_is_refused() {
        let mut h = new_handshake();
        h.greeting();
        let PyValue::Tuple(mut t) = h2() else {
            unreachable!()
        };
        t[3] = PyValue::Int(24);
        let err = h.on_payload(&PyValue::Tuple(t)).unwrap_err();
        assert!(
            matches!(
                err,
                HandshakeError::VersionMismatch {
                    field: "boot_version",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn a_build_as_a_string_is_accepted_because_the_server_types_it_loosely() {
        let mut h = new_handshake();
        h.greeting();
        let PyValue::Tuple(mut t) = h2() else {
            unreachable!()
        };
        t[4] = bstr(&reg::REFERENCE.to_string());
        h.on_payload(&PyValue::Tuple(t)).unwrap();
        assert_eq!(h.phase(), Phase::AwaitUserKey);
        assert_eq!(h.identity().boot_build, Some(i64::from(reg::REFERENCE)));
    }

    #[test]
    fn an_older_client_build_is_refused_but_a_newer_one_is_not() {
        let min = i64::from(Builds::registered().advertised());
        for (build, should_fail) in [(min - 1, true), (min, false), (min + 1, false)] {
            let mut h = new_handshake();
            h.greeting();
            let PyValue::Tuple(mut t) = h2() else {
                unreachable!()
            };
            t[4] = PyValue::Int(build);
            let result = h.on_payload(&PyValue::Tuple(t));
            assert_eq!(
                result.is_err(),
                should_fail,
                "build {build} should {} have been refused",
                if should_fail { "" } else { "not" }
            );
        }
    }

    #[test]
    fn a_non_placebo_crypto_pack_is_refused() {
        let mut h = new_handshake();
        h.greeting();
        h.on_payload(&h2()).unwrap();
        h.on_payload(&h3_vk()).unwrap();
        let err = h
            .on_payload(&PyValue::Tuple(vec![bstr("crypto"), PyValue::Dict(vec![])]))
            .unwrap_err();
        assert!(
            matches!(err, HandshakeError::BadCryptoPack { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn a_client_challenge_of_the_wrong_length_is_refused() {
        for len in [0usize, 1, 63, 65, 128] {
            let mut h = new_handshake();
            h.greeting();
            h.on_payload(&h2()).unwrap();
            h.on_payload(&h3_vk()).unwrap();
            h.on_payload(&h4()).unwrap();
            let err = h.on_payload(&h6(&vec![0x22u8; len])).unwrap_err();
            assert!(
                matches!(err, HandshakeError::BadChallengeLength { len: got } if got == len),
                "len {len} gave {err:?}"
            );
        }
    }

    #[test]
    fn a_wrong_challenge_response_is_refused() {
        let mut h = new_handshake();
        h.greeting();
        h.on_payload(&h2()).unwrap();
        h.on_payload(&h3_vk()).unwrap();
        h.on_payload(&h4()).unwrap();
        h.on_payload(&h6(&[0x11u8; CHALLENGE_LEN])).unwrap();
        let err = h
            .on_payload(&PyValue::Tuple(vec![
                bstr("0"),
                bstr(""),
                PyValue::Dict(vec![]),
            ]))
            .unwrap_err();
        assert!(
            matches!(err, HandshakeError::ChallengeResponseMismatch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn payloads_out_of_order_are_refused() {
        let mut h = new_handshake();
        h.greeting();
        // H4 where H2 belongs.
        assert!(h.on_payload(&h4()).is_err());
    }

    #[test]
    fn nothing_is_accepted_after_authentication() {
        let (mut h, _) = walk();
        let err = h.on_payload(&h2()).unwrap_err();
        assert!(
            matches!(
                err,
                HandshakeError::Unexpected {
                    phase: "authenticated",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    /// An accounts file of one invented token.
    fn strict_config(token: &str, user_id: i64) -> Config {
        use crate::accounts::{hex, sha256};
        let line = format!(
            "{}\t{user_id}\tgm\tinvented\n",
            hex(&sha256(token.as_bytes()))
        );
        Config {
            accounts: Some(Arc::new(Accounts::parse(&line).unwrap())),
            ..Config::default()
        }
    }

    /// H1 to H6 with `token`, returning H6's result.
    fn up_to_h6(h: &mut Handshake, token: &str) -> Result<Outcome, HandshakeError> {
        h.greeting();
        for payload in [h2(), h3_vk(), h4()] {
            h.on_payload(&payload).unwrap();
        }
        h.on_payload(&PyValue::Tuple(vec![
            PyValue::Bytes(vec![0x11; CHALLENGE_LEN]),
            request_dict_with(token),
        ]))
    }

    /// World accounts (ACC2): H6 is decided by `pre_login`; a sign-up
    /// reaches H7 and resolves, a wrong password is answered with the
    /// refusal object instead of H7 and closes.
    #[test]
    fn world_accounts_decide_h6_before_the_lock() {
        let world =
            std::sync::RwLock::new(crate::service::Registry::new(crate::model::SandboxConfig {
                characters: Vec::new(),
                catch_all: None,
                ..crate::model::SandboxConfig::default()
            }));
        let config = || Config {
            accounts: Some(Arc::new(Accounts::in_world(
                crate::account_book::Book::with_iterations(2),
            ))),
            listener: Listener {
                signup: true,
                ..Config::default().listener
            },
            ..Config::default()
        };
        let h6 = |token: &str| {
            PyValue::Tuple(vec![
                PyValue::Bytes(vec![0x11; CHALLENGE_LEN]),
                request_dict_with(token),
            ])
        };
        let drive = |token: &str| {
            let mut h = Handshake::new(config(), CHALLENGE);
            h.greeting();
            for payload in [h2(), h3_vk(), h4()] {
                h.on_payload(&payload).unwrap();
            }
            h.pre_login(&h6(token), &world);
            let out = h.on_payload(&h6(token)).unwrap();
            (h, out)
        };
        let (h, out) = drive("Invented Cid:acc2-invented-pass-3");
        assert!(!out.close);
        assert_eq!(out.replies[0], PyValue::Int(PASSWORD_VERSION));
        assert!(
            h.resolved()
                .is_some_and(|r| r.user_id >= crate::accounts::FIRST_USER_ID)
        );

        let (h, out) = drive("Invented Cid:acc2-not-the-password");
        assert!(out.close, "a refused login closes");
        assert_eq!(h.phase(), Phase::Closed);
        assert_eq!(h.resolved(), None);
        assert!(matches!(out.replies[0], PyValue::ObjectEx { .. }));
    }

    #[test]
    fn an_unknown_token_is_refused_when_accounts_are_set() {
        let mut h = Handshake::new(strict_config("invented-token", 1234), CHALLENGE);
        let err = up_to_h6(&mut h, "a-token").unwrap_err();
        assert!(
            matches!(
                err,
                HandshakeError::UnknownAccount {
                    why: Refused::UnknownToken { .. },
                    ..
                }
            ),
            "got {err:?}"
        );
        assert!(
            !err.to_string().contains("a-token"),
            "logged the token: {err}"
        );
        assert_eq!(h.phase(), Phase::Closed);
        assert_eq!(h.resolved(), None);

        // Its own token logs in, and H10 carries the resolved account, not
        // the catch-all's `user_id`.
        let mut h = Handshake::new(strict_config("invented-token", 1234), CHALLENGE);
        up_to_h6(&mut h, "invented-token").unwrap();
        assert_eq!(h.resolved().map(|r| r.user_id), Some(1234));
        let h9 = PyValue::Tuple(vec![
            PyValue::Bytes(crypto_hash(&[PyValue::Bytes(CHALLENGE.to_vec())]).into_bytes()),
            bstr(""),
            PyValue::Dict(vec![]),
        ]);
        let h10 = h.on_payload(&h9).unwrap().replies.pop().unwrap();
        let init = dict_get(&h10, b"session_init").unwrap();
        assert_eq!(dict_get(init, b"userid"), Some(&PyValue::Int(1234)));

        // No token at all is refused too.
        let mut h = Handshake::new(strict_config("invented-token", 1234), CHALLENGE);
        let err = up_to_h6(&mut h, "").unwrap_err();
        assert!(
            matches!(
                err,
                HandshakeError::UnknownAccount {
                    why: Refused::NoToken,
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    /// H2 carrying `build`, on a handshake made from `config`.
    fn h2_with(config: Config, build: i64) -> (Handshake, Result<Outcome, HandshakeError>) {
        let mut h = Handshake::new(config, CHALLENGE);
        h.greeting();
        let PyValue::Tuple(mut t) = h2() else {
            unreachable!()
        };
        t[4] = PyValue::Int(build);
        let r = h.on_payload(&PyValue::Tuple(t));
        (h, r)
    }

    #[test]
    fn strict_mode_accepts_exactly_the_registered_set() {
        let builds = Builds::registered();
        let ours = i64::from(builds.advertised());
        let accepted: Vec<i64> = builds.accepted.iter().map(|b| i64::from(*b)).collect();
        // Every registered pinned/equivalent build logs in, and H1 advertises
        // the line minimum to each.
        for e in reg::BUILDS.iter().filter(|e| e.status.accepted()) {
            let mut h = Handshake::new(strict_config("invented-token", 1234), CHALLENGE);
            let PyValue::Tuple(t) = h.greeting() else {
                unreachable!()
            };
            assert_eq!(t[4], PyValue::Int(i64::from(reg::line_min_build(e.line))));
            let (h, r) = h2_with(strict_config("invented-token", 1234), i64::from(e.build));
            r.unwrap_or_else(|err| panic!("build {} refused: {err}", e.build));
            assert_eq!(h.identity().boot_build, Some(i64::from(e.build)));
        }
        // Below the minimum, and an unregistered newer build, are refused
        // with the whole set named.
        let newest = *accepted.last().unwrap();
        for build in [ours - 1, newest + 1] {
            let (_, r) = h2_with(strict_config("invented-token", 1234), build);
            assert_eq!(
                r.unwrap_err(),
                HandshakeError::WrongBuild {
                    ours,
                    theirs: build,
                    accepted: accepted.clone(),
                }
            );
        }
        assert!(strict_config("t", 1).strict_build());
        assert!(!Config::default().strict_build());
    }

    #[test]
    fn catch_all_refuses_below_the_minimum_as_stale_and_lets_a_newer_build_in() {
        let builds = Builds::registered();
        let min = i64::from(builds.advertised());
        let newest = i64::from(*builds.accepted.last().unwrap());
        let (_, r) = h2_with(Config::default(), min - 1);
        assert_eq!(
            r.unwrap_err(),
            HandshakeError::StaleClient {
                client_build: min - 1,
                our_build: min
            }
        );
        let (h, r) = h2_with(Config::default(), newest + 1);
        r.unwrap();
        assert_eq!(h.identity().boot_build, Some(newest + 1));
    }

    /// Invented build numbers: the accepted set, not the registry, decides.
    const INVENTED_LOW: u32 = 100;
    const INVENTED_HIGH: u32 = 105;

    #[test]
    fn a_chosen_set_advertises_its_own_minimum_and_strict_mode_follows_it() {
        let builds = Builds {
            line: reg::reference().line,
            accepted: vec![INVENTED_LOW, INVENTED_HIGH],
        };
        let config = || Config {
            builds: builds.clone(),
            ..strict_config("invented-token", 1234)
        };
        let mut h = Handshake::new(config(), CHALLENGE);
        let PyValue::Tuple(t) = h.greeting() else {
            unreachable!()
        };
        assert_eq!(t[4], PyValue::Int(i64::from(INVENTED_LOW)));
        for b in [INVENTED_LOW, INVENTED_HIGH] {
            h2_with(config(), i64::from(b)).1.unwrap();
        }
        // In between the two: newer than the advertised build, but not in
        // the set, so strict mode refuses it.
        assert!(matches!(
            h2_with(config(), i64::from(INVENTED_LOW) + 1).1,
            Err(HandshakeError::WrongBuild { .. })
        ));
        assert!(builds.describe().contains("H1 advertises 100"));
    }

    #[test]
    fn client_build_parses_registered_builds_on_one_line_only() {
        // Only the builds on the reference line: the registry may hold
        // several lines, and one process speaks one.
        let line = reg::reference().line;
        let all: Vec<String> = reg::BUILDS
            .iter()
            .filter(|e| e.line == line)
            .map(|e| e.build.to_string())
            .collect();
        let b = Builds::parse(&all.join(",")).unwrap();
        assert_eq!(b.line, line);
        assert_eq!(b.accepted.len(), all.len());
        assert!(b.accepted.windows(2).all(|w| w[0] < w[1]), "sorted");
        let one = Builds::parse(&reg::REFERENCE.to_string()).unwrap();
        assert_eq!(one.accepted, vec![reg::REFERENCE]);
        assert!(Builds::parse("").is_err());
        assert!(Builds::parse("x").is_err());
        let unregistered = reg::BUILDS.iter().map(|e| e.build).max().unwrap() + 1;
        let e = Builds::parse(&unregistered.to_string()).unwrap_err();
        assert!(e.contains("not in the build registry"), "{e}");
        let on_line: Vec<u32> = reg::accepted()
            .into_iter()
            .filter(|b| reg::entry(*b).is_some_and(|e| e.line == line))
            .collect();
        assert_eq!(Builds::registered().accepted, on_line);
        assert_eq!(reg::reference().line, &reg::REFERENCE_LINE);
    }

    /// A second release line, invented (a future V99.01): a registry of two
    /// lines to exercise what the one-line registry cannot.
    static NEXT_LINE: reg::ReleaseLine = reg::ReleaseLine {
        macho: 497,
        version: 99.0,
        codename: "V99.01",
        region: "ccp",
        secret: 170_473,
        padding: 64,
        codename_at_region: "V99.01@ccp",
    };
    static TWO_LINES: [reg::BuildEntry; 3] = [
        reg::BuildEntry {
            build: INVENTED_LOW,
            line: &reg::REFERENCE_LINE,
            status: reg::Status::Pinned,
            data_hash: None,
        },
        reg::BuildEntry {
            build: INVENTED_HIGH,
            line: &reg::REFERENCE_LINE,
            status: reg::Status::Equivalent,
            data_hash: None,
        },
        reg::BuildEntry {
            build: INVENTED_HIGH + 10,
            line: &NEXT_LINE,
            status: reg::Status::Unverified,
            data_hash: None,
        },
    ];

    #[test]
    fn a_two_line_registry_parses_each_line_alone_and_refuses_a_mix() {
        let a = Builds::parse_in("100,105", &TWO_LINES).unwrap();
        assert_eq!(a.line, &reg::REFERENCE_LINE);
        assert_eq!(a.accepted, vec![100, 105]);
        let b = Builds::parse_in("115", &TWO_LINES).unwrap();
        assert_eq!(b.line, &NEXT_LINE);
        assert_eq!(b.accepted, vec![115]);
        let e = Builds::parse_in("105,115", &TWO_LINES).unwrap_err();
        assert!(
            e.contains("one server process speaks one release line"),
            "{e}"
        );
    }

    #[test]
    fn h2_with_the_old_lines_codename_is_refused_by_the_new_line() {
        // The server speaks the new line; a client still announcing the
        // reference line's codename fails on the codename, not by luck.
        let config = Config {
            builds: Builds {
                line: &NEXT_LINE,
                accepted: vec![115],
            },
            ..Config::default()
        };
        let mut h = Handshake::new(config, CHALLENGE);
        h.greeting();
        let PyValue::Tuple(mut t) = h2() else {
            unreachable!()
        };
        // The new line's secret, macho and version, so only the codename
        // is the old line's.
        t[0] = PyValue::Int(i64::from(NEXT_LINE.secret));
        t[1] = PyValue::Int(i64::from(NEXT_LINE.macho));
        t[3] = PyValue::Real(NEXT_LINE.version);
        t[4] = PyValue::Int(115);
        let err = h.on_payload(&PyValue::Tuple(t)).unwrap_err();
        match err {
            HandshakeError::VersionMismatch {
                field, expected, ..
            } => {
                assert_eq!(field, "boot_codename@region");
                assert_eq!(expected, "V99.01@ccp");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn without_accounts_every_token_is_the_catch_all() {
        let config = Config {
            user_id: 7,
            ..Config::default()
        };
        for token in ["a-token", ""] {
            let mut h = Handshake::new(config.clone(), CHALLENGE);
            up_to_h6(&mut h, token).unwrap();
            let r = h.resolved().unwrap();
            assert_eq!((r.user_id, r.role), (7, crate::accounts::Role::Gm));
        }
    }

    #[test]
    fn pydict_stores_value_then_key() {
        // The wire order is value-first. If this inverts, every dict the
        // handshake sends has its keys and values swapped, and the client
        // rejects the handshake with no useful diagnostic — so pin it.
        let d = pydict(vec![(b"k".as_slice(), PyValue::Int(7))]);
        let PyValue::Dict(entries) = &d else {
            panic!("not a dict")
        };
        assert_eq!(entries[0].0, PyValue::Int(7), "slot 0 must be the value");
        assert_eq!(entries[0].1, bstr("k"), "slot 1 must be the key");
        assert_eq!(dict_get(&d, b"k"), Some(&PyValue::Int(7)));
    }

    #[test]
    fn dict_lookup_accepts_a_unicode_key() {
        let d = PyValue::Dict(vec![(PyValue::Int(3), PyValue::Unicode("k".to_owned()))]);
        assert_eq!(dict_get(&d, b"k"), Some(&PyValue::Int(3)));
    }

    /// SEC-FIX 12: what the client sent never reaches a refusal raw.
    #[test]
    fn refusal_text_escapes_and_caps_what_the_client_sent() {
        let hostile = "evil\n2026-01-01 INFO fake line\x1b[31m\"\\".to_owned() + &"x".repeat(5_000);
        let mut h = new_handshake();
        h.greeting();
        h.on_payload(&h2()).unwrap();
        h.on_payload(&h3_vk()).unwrap();
        let err = h
            .on_payload(&PyValue::Tuple(vec![bstr(&hostile), PyValue::Dict(vec![])]))
            .unwrap_err();
        let text = err.to_string();
        assert!(!text.contains('\n') && !text.contains('\x1b'), "{text:?}");
        assert!(
            text.contains("\\n2026-01-01 INFO fake line"),
            "escaped, not dropped: {text}"
        );
        assert!(text.len() < 300, "capped, got {} bytes", text.len());
        // A version field as a hostile string, and the unmatched-element text.
        let mut h = new_handshake();
        h.greeting();
        let PyValue::Tuple(mut t) = h2() else {
            unreachable!()
        };
        t[1] = PyValue::Unicode(hostile.clone());
        let text = h.on_payload(&PyValue::Tuple(t)).unwrap_err().to_string();
        assert!(
            !text.contains('\n') && !text.contains('\x1b') && text.len() < 300,
            "{text:?}"
        );
        let mut h = new_handshake();
        h.greeting();
        h.on_payload(&h2()).unwrap();
        let bad = PyValue::Tuple(vec![PyValue::None, bstr(&hostile)]);
        let text = h.on_payload(&bad).unwrap_err().to_string();
        assert!(
            !text.contains('\n') && !text.contains('\x1b') && text.len() < 300,
            "{text:?}"
        );
    }

    #[test]
    fn loggable_keeps_a_short_plain_value_and_marks_the_cut() {
        assert_eq!(loggable("placebo"), "placebo");
        let cut = loggable(&"a".repeat(LOGGED_TEXT_MAX + 3));
        assert!(cut.ends_with("...(+3)"), "{cut}");
    }
}
