//! The public gateway, as far as the client needs it to stop waiting.
//!
//! **Why this exists.** The client sends gRPC requests to a "public gateway"
//! beside machoNet (`publicGateway/`, `eve_public_grpc_client.pyd`). Nothing
//! answered, so each request ran into its *local* deadline (3 s for a ship
//! SKIN, 10 s for most), and two things the owner uses all the time sit behind
//! such a wait: the fitting window (`cosmeticsSvc.GetAppliedSkinState`, 3 s a
//! time, three times) and the right-click menu in space (the mercenary-den
//! activities request). Evidence: `docs/lanes/ui-lag.md`.
//!
//! **What it is.** A std-only HTTP/2 (h2c, no TLS: the client takes
//! `EVE_PUBLIC_GATEWAY_ADDRESS` as insecure, `tools/eve_patch.py` launch env)
//! server for the three gRPC services in the client's own `gateway.proto`
//! **[V]** (read from the `.pyd`): `eve_public.gateway.Requests/Send`
//! (`Request` -> `Response`, bidirectional), `Notices/Consume` (we hold the
//! stream open and send nothing), `Events/Publish` (an empty message back per
//! event), and the three `Ping`s. The `Request`/`Response` field numbers are
//! the client's `public_pb2` **[V]**.
//!
//! **What it answers.** Instantly, from the pilot the request names
//! (`authoritative_context.identity.character`): see [`answer`]. Most classes
//! get the empty answer a pilot with none of the thing would get; personal
//! skill plans are real and persisted in the world file
//! ([`crate::gateway_plans`]). It is still not a gateway implementation;
//! each request class that matters gets its own arm.
//!
//! **Notices.** `Notices/Consume` is answered with response headers and then
//! held open and never ended. Since GW-NOTICE it is written to on demand:
//! see [`NoticeHub`] and [`publish`]. The client's consumer
//! (`publicGateway/grpc/noticeConsumer.py`) polls its *local* queue with a
//! 1 ms sleep, about 15 times a second in the logs, whatever the server does;
//! with the stream held it logs `consumer_state Active`, `channel_state
//! Ready`, no status breadcrumbs, and `Consumed no notices` every 5 s **[V]**
//! (client log 21-300). Ending the stream would make it reconnect in a loop.
//!
//! HPACK (RFC 7541) is decoded in full, dynamic table included, because the
//! only header we need, `:path`, is usually an index into it.

use crate::delta::Deltas;
use crate::service::Registry;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// The port the client dials (`EVE_PUBLIC_GATEWAY_ADDRESS=<host>:8443`).
pub const PORT: u16 = 8443;
/// Connections at once. The client opens one.
const CAP: usize = 8;
const IDLE: Duration = Duration::from_secs(3600);
const MAX_FRAME: usize = 1 << 20;
const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

// ---- protobuf, just enough ------------------------------------------------

fn varint(b: &[u8], p: &mut usize) -> Option<u64> {
    let (mut r, mut s) = (0u64, 0u32);
    loop {
        let x = *b.get(*p)?;
        *p += 1;
        r |= u64::from(x & 0x7f).checked_shl(s)?;
        if x < 0x80 {
            return Some(r);
        }
        s += 7;
        if s > 63 {
            return None;
        }
    }
}

/// `(field, bytes)` for every length-delimited field, and `(field, value)`
/// as 8 little-endian bytes for varints; other wire types end the walk.
fn fields(b: &[u8]) -> Vec<(u64, Vec<u8>)> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let Some(k) = varint(b, &mut p) else { break };
        match k & 7 {
            0 => {
                let Some(v) = varint(b, &mut p) else { break };
                out.push((k >> 3, v.to_le_bytes().to_vec()));
            }
            2 => {
                let Some(l) = varint(b, &mut p) else { break };
                let Some(end) = p.checked_add(l as usize).filter(|e| *e <= b.len()) else {
                    break;
                };
                out.push((k >> 3, b[p..end].to_vec()));
                p = end;
            }
            _ => break,
        }
    }
    out
}

pub(crate) fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub(crate) fn put_bytes(out: &mut Vec<u8>, field: u64, v: &[u8]) {
    put_varint(out, field << 3 | 2);
    put_varint(out, v.len() as u64);
    out.extend_from_slice(v);
}

/// A decoded field: a varint, or a length-delimited value.
enum Val {
    Int(u64),
    Bytes(Vec<u8>),
}

fn walk(b: &[u8]) -> Vec<(u64, Val)> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let Some(k) = varint(b, &mut p) else { break };
        match k & 7 {
            0 => {
                let Some(v) = varint(b, &mut p) else { break };
                out.push((k >> 3, Val::Int(v)));
            }
            2 => {
                let Some(l) = varint(b, &mut p) else { break };
                let Some(end) = p.checked_add(l as usize).filter(|e| *e <= b.len()) else {
                    break;
                };
                out.push((k >> 3, Val::Bytes(b[p..end].to_vec())));
                p = end;
            }
            _ => break,
        }
    }
    out
}

/// The first length-delimited value of field `n`.
pub(crate) fn get_bytes(b: &[u8], n: u64) -> Option<Vec<u8>> {
    walk(b).into_iter().find_map(|(k, v)| match v {
        Val::Bytes(x) if k == n => Some(x),
        _ => None,
    })
}

/// Every length-delimited value of field `n` (a repeated message).
pub(crate) fn field(b: &[u8], n: u64) -> Vec<Vec<u8>> {
    walk(b)
        .into_iter()
        .filter_map(|(k, v)| match v {
            Val::Bytes(x) if k == n => Some(x),
            _ => None,
        })
        .collect()
}

/// The first varint of field `n`.
pub(crate) fn get_uint(b: &[u8], n: u64) -> Option<u64> {
    walk(b).into_iter().find_map(|(k, v)| match v {
        Val::Int(x) if k == n => Some(x),
        _ => None,
    })
}

/// The pilot a request is for: `Request.authoritative_context.identity
/// .character.sequential` (fields 6, 10, 1, 1), which the client fills in
/// from the session (`publicGateway/grpc/requestBroker.py`) **[V]**. 0 when
/// absent.
fn request_character(request: &[u8]) -> i64 {
    get_bytes(request, 6)
        .and_then(|c| get_bytes(&c, 10))
        .and_then(|i| get_bytes(&i, 1))
        .and_then(|c| get_uint(&c, 1))
        .map_or(0, |n| n as i64)
}

/// Where the gateway's stored state lives: the server's world (its delta
/// rows, under the world lock), or a bare set of rows for a test.
#[derive(Clone)]
pub enum Ground {
    Server(Arc<RwLock<Registry>>),
    Local(Arc<Mutex<Deltas>>),
}

impl Ground {
    /// A ground that owns its rows (tests, tools).
    pub fn local() -> Ground {
        Ground::Local(Arc::new(Mutex::new(Deltas::default())))
    }

    pub(crate) fn read<R>(&self, f: impl FnOnce(&Deltas) -> R) -> R {
        match self {
            Ground::Server(w) => f(crate::service::world_read_timed(w, "gateway_read").deltas()),
            Ground::Local(d) => f(&d.lock().unwrap_or_else(|e| e.into_inner())),
        }
    }

    pub(crate) fn write<R>(&self, f: impl FnOnce(&mut Deltas) -> R) -> R {
        match self {
            Ground::Server(w) => {
                f(crate::service::world_write_timed(w, "gateway_write").deltas_mut())
            }
            Ground::Local(d) => f(&mut d.lock().unwrap_or_else(|e| e.into_inner())),
        }
    }
}

/// What the gateway says to a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u32,
    /// `(full type name, encoded message)` for the `Any` payload; `None`
    /// leaves it unset.
    pub payload: Option<(String, Vec<u8>)>,
}

/// The type name inside an `Any`'s `type_url`, as the proto package spells it.
fn type_name(url: &[u8]) -> String {
    let s = String::from_utf8_lossy(url);
    let name = s.rsplit('/').next().unwrap_or("");
    name.replace(".requests_pb2.", ".")
        .replace(".api_pb2.", ".")
}

fn put_uint(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

/// Seconds since the epoch.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `google.protobuf.Timestamp { seconds }`.
fn timestamp(secs: u64) -> Vec<u8> {
    let mut ts = Vec::new();
    put_uint(&mut ts, 1, secs);
    ts
}

/// The omega licence's expiry, from now. Nine years, not 2100: the client
/// turns it into a `datetime` with `fromtimestamp`, and a 32-bit `time_t`
/// ends in 2038 **[A]** (falsifier: `ValueError` from `user_license.py`
/// with a later date; the machoNet side, `GetSubscriptionTime`, uses 2100
/// because only blue formats that one).
const LICENSE_YEARS: u64 = 9;
/// `plex.vault` balance in cents of PLEX **[A]**: this server has no PLEX
/// store, so the vault is empty (falsifier: a window that wants to spend it).
pub const PLEX_BALANCE_CENTS: u64 = 0;
/// A career path goal's target (progress is always 0 of it) **[A]**.
const PATH_GOAL_TARGET: u64 = 100;

/// Requests the client reads as "none", with a `404` and no payload: a
/// missing single record where an empty message would read as a real one
/// with default fields (a black corporation palette, a lock on gate 0, a
/// licence that expires in 1970). Each is `[V]` from the messenger that sends
/// it.
fn is_not_found(ty: &str) -> bool {
    matches!(
        ty,
        // the ship SKIN: `shipSkinStateRequestMessenger` returns `(response, None)`
        "eve_public.cosmetic.ship.api.GetRequest"
            // `corpColorsRequestMessenger`: NotFoundException -> no palette
            | "eve_public.cosmetic.corporation.palette.api.GetRequest"
            | "eve_public.cosmetic.corporation.palette.api.GetOwnRequest"
            // `structure ... requestMessenger.get_request`: `(None, None)`
            | "eve_public.cosmetic.structure.paintwork.api.GetRequest"
            // `gateLockMessenger.get_request`: NOT_FOUND -> no lock
            | "eve_public.stargate.lock.api.GetRequest"
            // `request_messenger` (mercenary den): NOT_FOUND -> UnknownMercenaryDen
            | "eve_public.sovereignty.mercenaryden.api.GetAsOwnerRequest"
            | "eve_public.sovereignty.mercenaryden.activity.api.GetForMercenaryDenRequest"
            | "eve_public.sovereignty.mercenaryden.activity.api.StartRequest"
            | "eve_public.career.goal.api.ClaimRewardRequest"
    ) || is_content_checkpoint(ty)
}

/// The flatbuffer content classes (`content/client/flatbuffer/provider.py`):
/// the reply is a CDN checkpoint (an https url) and there is no CDN, so there
/// is nothing right to say. The provider then raises whatever we answer
/// (`IndexError` for an empty checkpoint, `GenericNotFoundError` for a 404);
/// both end that repository's update task, which keeps its default (empty)
/// data. A `404` is the cleaner of the two and names the cause in the log.
fn is_content_checkpoint(ty: &str) -> bool {
    ty.ends_with(".cdn.GetLatestRequest")
        || ty.ends_with(".cdn.GetUpdateRequest")
        || ty.ends_with(".definition.api.GetLatestRequest")
}

/// Careers, as `career.Path`, with the 16-byte id each path goal gets.
/// The ids are invented; the client only compares them.
fn career_path_goals() -> Vec<u8> {
    let mut out = Vec::new();
    for path in 1u64..=4 {
        let mut id = Vec::new();
        put_bytes(
            &mut id,
            1,
            &[
                0xca, 0x4e, 0xe4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, path as u8,
            ],
        );
        let mut attrs = Vec::new();
        put_uint(&mut attrs, 4, PATH_GOAL_TARGET);
        put_uint(&mut attrs, 9, path);
        let mut goal = Vec::new();
        put_bytes(&mut goal, 1, &id);
        put_bytes(&mut goal, 2, &attrs);
        put_bytes(&mut out, 1, &goal);
    }
    out
}

/// How to answer a request class. **Everything here is a choice made to
/// keep the UI moving, and every arm not marked `[V]` is [A]** until a
/// client run shows what it does with it; the falsifier is the same for all:
/// a new traceback from the messenger that sent it (`evecap --alerts`).
///
/// - Skill plans and their milestones are real and per pilot:
///   [`crate::gateway_plans`].
/// - [`is_not_found`] classes: **404**, no payload.
/// - `user.license.api.GetRequest`, the omega licence: this server's pilots
///   are omega (`subscriptionMgr.GetCloneGrade`), so the licence exists and
///   runs [`LICENSE_YEARS`]. Without one the client's neocom upsell button
///   is built from `has_license`; with an empty `Attributes` it raised
///   `can't compare datetime to NoneType` (`user_license.is_expired`).
/// - `cosmetic.character.title.api.GetEquippedForCharacterRequest`: the
///   `no_title` branch of the oneof.
/// - `plex.vault.api.BalanceRequest`: [`PLEX_BALANCE_CENTS`].
/// - `faction.activity.GetSolarSystemScoresRequest`: the asked system, no
///   scores (the client builds `SolarSystemScores` from the echoed id).
/// - `career.goal.api.GetDefinitionsRequest`: one goal per career path, no
///   progress rows. `jobboard.relevance_profile` reads each path's goal
///   (`goal_data_controller.get_career_path_goal`) and raised
///   `AttributeError: 'NoneType' ... goal_id` with none.
/// - `pirate.{corruption,suppression}.api.GetSystemInfoRequest`: progress 0/1.
/// - `sovereignty.hub.workforce.api.{GetConfigurationRequest,GetStateRequest}`:
///   the `inactive` mode set. Left empty, `create_from_proto` returns None and
///   `SovereigntyHub.UpdateWorkforceTransportState` raises on `.get_mode()`.
/// - `chat.local.api.*`: untouched. The client sends none of them while the
///   local-chat feature flag is off, and it is off (XMPP carries local);
///   if it came on, the empty member list is the honest answer.
/// - every other `*Request`: 200 with an *empty typed* `*Response` (no dens,
///   no thresholds, no rewards, no daily goals: what a pilot with none would
///   get). Payload unset reads as `None` in the client
///   (`ResponseHandler.handle` unpacks only when the `type_url` is set,
///   `docs/guides/debugging-the-client.md` §3c) and ~15 messengers raised on
///   it (sysinfo lane, client log 20:23).
pub fn answer(ty: &str, body: &[u8], character: i64, g: &Ground) -> Reply {
    let reply = |status, payload| Reply { status, payload };
    if let Some(r) = crate::gateway_ext::handle(ty, body, character, g) {
        return r;
    }
    if is_not_found(ty) {
        return reply(404, None);
    }
    let base = ty.strip_suffix("Request");
    let typed = |value: Vec<u8>| reply(200, base.map(|b| (format!("{b}Response"), value)));
    match ty {
        "eve_public.user.license.api.GetRequest" => {
            // `Identifier.license_type` 1; only the omega licence exists.
            let kind = get_bytes(body, 1)
                .and_then(|l| get_bytes(&l, 1))
                .unwrap_or_default();
            if kind != b"eve_clonestate_omega" {
                return reply(404, None);
            }
            let now = now_secs();
            let mut attrs = Vec::new();
            put_bytes(
                &mut attrs,
                2,
                &timestamp(now + LICENSE_YEARS * 365 * 86_400),
            );
            put_bytes(&mut attrs, 3, &timestamp(now));
            let mut out = Vec::new();
            put_bytes(&mut out, 1, &attrs);
            typed(out)
        }
        "eve_public.cosmetic.character.title.api.GetEquippedForCharacterRequest" => {
            let mut out = Vec::new();
            put_uint(&mut out, 2, 1); // no_title
            typed(out)
        }
        "eve_public.plex.vault.api.BalanceRequest" => {
            let mut cur = Vec::new();
            put_uint(&mut cur, 1, PLEX_BALANCE_CENTS);
            let mut out = Vec::new();
            put_bytes(&mut out, 1, &cur);
            typed(out)
        }
        "eve_public.faction.activity.GetSolarSystemScoresRequest" => {
            let mut out = Vec::new();
            if let Some(system) = get_bytes(body, 1) {
                put_bytes(&mut out, 1, &system);
            }
            typed(out)
        }
        "eve_public.career.goal.api.GetDefinitionsRequest" => typed(career_path_goals()),
        // A system with no corruption or suppression: progress 0/1 (the
        // insurgency panel divides numerator by denominator, so 0/0 would
        // raise), stage 0. Corruption and suppression `GetSystemInfoResponse`
        // share one layout (`total_progress` = field 1, a `Fraction` of
        // numerator 1, denominator 2; `[V]` from both `requests_pb2`
        // descriptors).
        "eve_public.pirate.corruption.api.GetSystemInfoRequest"
        | "eve_public.pirate.suppression.api.GetSystemInfoRequest" => {
            let mut fraction = Vec::new();
            put_uint(&mut fraction, 2, 1);
            let mut value = Vec::new();
            put_bytes(&mut value, 1, &fraction);
            typed(value)
        }
        // A hub with no workforce set up is idle [A: a fresh hub reads
        // inactive; falsifier: a hub panel showing another mode with nothing
        // configured]. The `mode` oneof must be set: unset, `create_from_proto`
        // returns None for the configuration and the state alike, and the
        // client catches only `SovereigntyWorkforceError`. `inactive` = 1 in
        // both, the message is response field 1 (`[V]` `workforce_pb2`).
        "eve_public.sovereignty.hub.workforce.api.GetConfigurationRequest"
        | "eve_public.sovereignty.hub.workforce.api.GetStateRequest" => {
            let mut mode = Vec::new();
            put_uint(&mut mode, 1, 1);
            let mut out = Vec::new();
            put_bytes(&mut out, 1, &mode);
            typed(out)
        }
        // Writes nothing here can honour: refused, so the client shows its
        // own "could not" instead of believing a save that never happened.
        "eve_public.cosmetic.corporation.palette.api.SetRequest" => reply(403, None),
        // Everything else: an empty typed response. The client reads a
        // missing payload as `None` and the messengers then raise
        // `AttributeError` on it (the location panel,
        // `infoPanelLocationInfo.UpdateMainLocationInfo` -> `GetCorruption`
        // -> `.total_progress`, only catches `TimeoutException`, so the whole
        // panel failed to build). An empty message reads as defaults.
        _ => typed(Vec::new()),
    }
}

/// Turn one `eve_public.Request` into its `eve_public.Response` bytes, the
/// request's type name and the status it got.
pub fn respond(request: &[u8], instance: &[u8; 16], g: &Ground) -> (Vec<u8>, String, u32) {
    let f = fields(request);
    let get = |n: u64| f.iter().find(|(k, _)| *k == n).map(|(_, v)| v.as_slice());
    let any = get(3).unwrap_or(&[]);
    let ty = get_bytes(any, 1).map(|v| type_name(&v)).unwrap_or_default();
    let value = get_bytes(any, 2).unwrap_or_default();
    let r = answer(&ty, &value, request_character(request), g);
    let mut out = Vec::new();
    // dispatched: Timestamp { seconds }
    put_bytes(&mut out, 1, &timestamp(now_secs()));
    put_bytes(&mut out, 2, get(2).unwrap_or(&[]));
    put_varint(&mut out, 3 << 3);
    put_varint(&mut out, u64::from(r.status));
    if let Some((name, value)) = &r.payload {
        let mut any = Vec::new();
        put_bytes(
            &mut any,
            1,
            format!("type.googleapis.com/{name}").as_bytes(),
        );
        put_bytes(&mut any, 2, value);
        put_bytes(&mut out, 5, &any);
    }
    put_bytes(&mut out, 6, b"eve-offline gateway stub");
    put_bytes(&mut out, 7, get(5).unwrap_or(&[]));
    put_bytes(&mut out, 8, instance);
    (out, ty, r.status)
}

// ---- HPACK ------------------------------------------------------------------

/// RFC 7541 appendix B: (bits, code), symbol 256 is EOS.
const HUFFMAN: [(u8, u32); 257] = [
    (13, 0x1ff8),
    (23, 0x7fffd8),
    (28, 0xfffffe2),
    (28, 0xfffffe3),
    (28, 0xfffffe4),
    (28, 0xfffffe5),
    (28, 0xfffffe6),
    (28, 0xfffffe7),
    (28, 0xfffffe8),
    (24, 0xffffea),
    (30, 0x3ffffffc),
    (28, 0xfffffe9),
    (28, 0xfffffea),
    (30, 0x3ffffffd),
    (28, 0xfffffeb),
    (28, 0xfffffec),
    (28, 0xfffffed),
    (28, 0xfffffee),
    (28, 0xfffffef),
    (28, 0xffffff0),
    (28, 0xffffff1),
    (28, 0xffffff2),
    (30, 0x3ffffffe),
    (28, 0xffffff3),
    (28, 0xffffff4),
    (28, 0xffffff5),
    (28, 0xffffff6),
    (28, 0xffffff7),
    (28, 0xffffff8),
    (28, 0xffffff9),
    (28, 0xffffffa),
    (28, 0xffffffb),
    (6, 0x14),
    (10, 0x3f8),
    (10, 0x3f9),
    (12, 0xffa),
    (13, 0x1ff9),
    (6, 0x15),
    (8, 0xf8),
    (11, 0x7fa),
    (10, 0x3fa),
    (10, 0x3fb),
    (8, 0xf9),
    (11, 0x7fb),
    (8, 0xfa),
    (6, 0x16),
    (6, 0x17),
    (6, 0x18),
    (5, 0x0),
    (5, 0x1),
    (5, 0x2),
    (6, 0x19),
    (6, 0x1a),
    (6, 0x1b),
    (6, 0x1c),
    (6, 0x1d),
    (6, 0x1e),
    (6, 0x1f),
    (7, 0x5c),
    (8, 0xfb),
    (15, 0x7ffc),
    (6, 0x20),
    (12, 0xffb),
    (10, 0x3fc),
    (13, 0x1ffa),
    (6, 0x21),
    (7, 0x5d),
    (7, 0x5e),
    (7, 0x5f),
    (7, 0x60),
    (7, 0x61),
    (7, 0x62),
    (7, 0x63),
    (7, 0x64),
    (7, 0x65),
    (7, 0x66),
    (7, 0x67),
    (7, 0x68),
    (7, 0x69),
    (7, 0x6a),
    (7, 0x6b),
    (7, 0x6c),
    (7, 0x6d),
    (7, 0x6e),
    (7, 0x6f),
    (7, 0x70),
    (7, 0x71),
    (7, 0x72),
    (8, 0xfc),
    (7, 0x73),
    (8, 0xfd),
    (13, 0x1ffb),
    (19, 0x7fff0),
    (13, 0x1ffc),
    (14, 0x3ffc),
    (6, 0x22),
    (15, 0x7ffd),
    (5, 0x3),
    (6, 0x23),
    (5, 0x4),
    (6, 0x24),
    (5, 0x5),
    (6, 0x25),
    (6, 0x26),
    (6, 0x27),
    (5, 0x6),
    (7, 0x74),
    (7, 0x75),
    (6, 0x28),
    (6, 0x29),
    (6, 0x2a),
    (5, 0x7),
    (6, 0x2b),
    (7, 0x76),
    (6, 0x2c),
    (5, 0x8),
    (5, 0x9),
    (6, 0x2d),
    (7, 0x77),
    (7, 0x78),
    (7, 0x79),
    (7, 0x7a),
    (7, 0x7b),
    (15, 0x7ffe),
    (11, 0x7fc),
    (14, 0x3ffd),
    (13, 0x1ffd),
    (28, 0xffffffc),
    (20, 0xfffe6),
    (22, 0x3fffd2),
    (20, 0xfffe7),
    (20, 0xfffe8),
    (22, 0x3fffd3),
    (22, 0x3fffd4),
    (22, 0x3fffd5),
    (23, 0x7fffd9),
    (22, 0x3fffd6),
    (23, 0x7fffda),
    (23, 0x7fffdb),
    (23, 0x7fffdc),
    (23, 0x7fffdd),
    (23, 0x7fffde),
    (24, 0xffffeb),
    (23, 0x7fffdf),
    (24, 0xffffec),
    (24, 0xffffed),
    (22, 0x3fffd7),
    (23, 0x7fffe0),
    (24, 0xffffee),
    (23, 0x7fffe1),
    (23, 0x7fffe2),
    (23, 0x7fffe3),
    (23, 0x7fffe4),
    (21, 0x1fffdc),
    (22, 0x3fffd8),
    (23, 0x7fffe5),
    (22, 0x3fffd9),
    (23, 0x7fffe6),
    (23, 0x7fffe7),
    (24, 0xffffef),
    (22, 0x3fffda),
    (21, 0x1fffdd),
    (20, 0xfffe9),
    (22, 0x3fffdb),
    (22, 0x3fffdc),
    (23, 0x7fffe8),
    (23, 0x7fffe9),
    (21, 0x1fffde),
    (23, 0x7fffea),
    (22, 0x3fffdd),
    (22, 0x3fffde),
    (24, 0xfffff0),
    (21, 0x1fffdf),
    (22, 0x3fffdf),
    (23, 0x7fffeb),
    (23, 0x7fffec),
    (21, 0x1fffe0),
    (21, 0x1fffe1),
    (22, 0x3fffe0),
    (21, 0x1fffe2),
    (23, 0x7fffed),
    (22, 0x3fffe1),
    (23, 0x7fffee),
    (23, 0x7fffef),
    (20, 0xfffea),
    (22, 0x3fffe2),
    (22, 0x3fffe3),
    (22, 0x3fffe4),
    (23, 0x7ffff0),
    (22, 0x3fffe5),
    (22, 0x3fffe6),
    (23, 0x7ffff1),
    (26, 0x3ffffe0),
    (26, 0x3ffffe1),
    (20, 0xfffeb),
    (19, 0x7fff1),
    (22, 0x3fffe7),
    (23, 0x7ffff2),
    (22, 0x3fffe8),
    (25, 0x1ffffec),
    (26, 0x3ffffe2),
    (26, 0x3ffffe3),
    (26, 0x3ffffe4),
    (27, 0x7ffffde),
    (27, 0x7ffffdf),
    (26, 0x3ffffe5),
    (24, 0xfffff1),
    (25, 0x1ffffed),
    (19, 0x7fff2),
    (21, 0x1fffe3),
    (26, 0x3ffffe6),
    (27, 0x7ffffe0),
    (27, 0x7ffffe1),
    (26, 0x3ffffe7),
    (27, 0x7ffffe2),
    (24, 0xfffff2),
    (21, 0x1fffe4),
    (21, 0x1fffe5),
    (26, 0x3ffffe8),
    (26, 0x3ffffe9),
    (28, 0xffffffd),
    (27, 0x7ffffe3),
    (27, 0x7ffffe4),
    (27, 0x7ffffe5),
    (20, 0xfffec),
    (24, 0xfffff3),
    (20, 0xfffed),
    (21, 0x1fffe6),
    (22, 0x3fffe9),
    (21, 0x1fffe7),
    (21, 0x1fffe8),
    (23, 0x7ffff3),
    (22, 0x3fffea),
    (22, 0x3fffeb),
    (25, 0x1ffffee),
    (25, 0x1ffffef),
    (24, 0xfffff4),
    (24, 0xfffff5),
    (26, 0x3ffffea),
    (23, 0x7ffff4),
    (26, 0x3ffffeb),
    (27, 0x7ffffe6),
    (26, 0x3ffffec),
    (26, 0x3ffffed),
    (27, 0x7ffffe7),
    (27, 0x7ffffe8),
    (27, 0x7ffffe9),
    (27, 0x7ffffea),
    (27, 0x7ffffeb),
    (28, 0xffffffe),
    (27, 0x7ffffec),
    (27, 0x7ffffed),
    (27, 0x7ffffee),
    (27, 0x7ffffef),
    (27, 0x7fffff0),
    (26, 0x3ffffee),
    (30, 0x3fffffff),
];

fn huffman_decode(b: &[u8]) -> Option<Vec<u8>> {
    static MAP: OnceLock<HashMap<(u8, u32), u16>> = OnceLock::new();
    let map = MAP.get_or_init(|| {
        HUFFMAN
            .iter()
            .enumerate()
            .map(|(i, &(n, c))| ((n, c), i as u16))
            .collect()
    });
    let (mut code, mut len) = (0u32, 0u8);
    let mut out = Vec::new();
    for &byte in b {
        for i in (0..8).rev() {
            code = code << 1 | u32::from(byte >> i & 1);
            len += 1;
            if let Some(&s) = map.get(&(len, code)) {
                if s == 256 {
                    return None;
                }
                out.push(s as u8);
                code = 0;
                len = 0;
            } else if len > 30 {
                return None;
            }
        }
    }
    // Padding: at most 7 bits, all ones.
    (len <= 7 && code == (1u32 << len) - 1).then_some(out)
}

/// The static table's names (RFC 7541 appendix A); only `:path`'s values
/// are ever read.
const STATIC_NAMES: [&str; 61] = [
    ":authority",
    ":method",
    ":method",
    ":path",
    ":path",
    ":scheme",
    ":scheme",
    ":status",
    ":status",
    ":status",
    ":status",
    ":status",
    ":status",
    ":status",
    "accept-charset",
    "accept-encoding",
    "accept-language",
    "accept-ranges",
    "accept",
    "access-control-allow-origin",
    "age",
    "allow",
    "authorization",
    "cache-control",
    "content-disposition",
    "content-encoding",
    "content-language",
    "content-length",
    "content-location",
    "content-range",
    "content-type",
    "cookie",
    "date",
    "etag",
    "expect",
    "expires",
    "from",
    "host",
    "if-match",
    "if-modified-since",
    "if-none-match",
    "if-range",
    "if-unmodified-since",
    "last-modified",
    "link",
    "location",
    "max-forwards",
    "proxy-authenticate",
    "proxy-authorization",
    "range",
    "referer",
    "refresh",
    "retry-after",
    "server",
    "set-cookie",
    "strict-transport-security",
    "transfer-encoding",
    "user-agent",
    "vary",
    "via",
    "www-authenticate",
];

#[derive(Default)]
struct Hpack {
    /// Newest first.
    dynamic: Vec<(Vec<u8>, Vec<u8>)>,
    size: usize,
    max: usize,
}

impl Hpack {
    fn new() -> Self {
        Hpack {
            dynamic: Vec::new(),
            size: 0,
            max: 4096,
        }
    }

    fn insert(&mut self, name: Vec<u8>, value: Vec<u8>) {
        self.size += name.len() + value.len() + 32;
        self.dynamic.insert(0, (name, value));
        self.evict();
    }

    fn evict(&mut self) {
        while self.size > self.max {
            match self.dynamic.pop() {
                Some((n, v)) => self.size -= n.len() + v.len() + 32,
                None => {
                    self.size = 0;
                    break;
                }
            }
        }
    }

    fn entry(&self, i: usize) -> Option<(Vec<u8>, Vec<u8>)> {
        match i {
            0 => None,
            1..=61 => {
                let v: &[u8] = match i {
                    4 => b"/",
                    5 => b"/index.html",
                    _ => b"",
                };
                Some((STATIC_NAMES[i - 1].as_bytes().to_vec(), v.to_vec()))
            }
            _ => self.dynamic.get(i - 62).cloned(),
        }
    }

    fn int(b: &[u8], p: &mut usize, bits: u32) -> Option<usize> {
        let mask = (1usize << bits) - 1;
        let mut v = usize::from(*b.get(*p)?) & mask;
        *p += 1;
        if v < mask {
            return Some(v);
        }
        let mut shift = 0u32;
        loop {
            let x = *b.get(*p)?;
            *p += 1;
            v = v.checked_add(usize::from(x & 0x7f).checked_shl(shift)?)?;
            if x < 0x80 {
                return Some(v);
            }
            shift += 7;
            if shift > 28 {
                return None;
            }
        }
    }

    fn string(b: &[u8], p: &mut usize) -> Option<Vec<u8>> {
        let huff = *b.get(*p)? & 0x80 != 0;
        let n = Self::int(b, p, 7)?;
        let end = p.checked_add(n).filter(|e| *e <= b.len())?;
        let raw = &b[*p..end];
        *p = end;
        if huff {
            huffman_decode(raw)
        } else {
            Some(raw.to_vec())
        }
    }

    /// Decode one header block; the `:path` value if it carried one.
    fn decode(&mut self, b: &[u8]) -> Option<Option<Vec<u8>>> {
        let mut p = 0;
        let mut path = None;
        while p < b.len() {
            let first = b[p];
            let (name, value) = if first & 0x80 != 0 {
                let i = Self::int(b, &mut p, 7)?;
                self.entry(i)?
            } else if first & 0xe0 == 0x20 {
                // Dynamic table size update.
                self.max = Self::int(b, &mut p, 5)?;
                self.evict();
                continue;
            } else {
                let (bits, index) = if first & 0x40 != 0 {
                    (6, true)
                } else {
                    (4, false)
                };
                let i = Self::int(b, &mut p, bits)?;
                let name = if i == 0 {
                    Self::string(b, &mut p)?
                } else {
                    self.entry(i)?.0
                };
                let value = Self::string(b, &mut p)?;
                if index {
                    self.insert(name.clone(), value.clone());
                }
                (name, value)
            };
            if name == b":path" {
                path = Some(value);
            }
        }
        Some(path)
    }
}

// ---- HTTP/2 ------------------------------------------------------------------

const DATA: u8 = 0;
const HEADERS: u8 = 1;
const RST_STREAM: u8 = 3;
const SETTINGS: u8 = 4;
const PING: u8 = 6;
const GOAWAY: u8 = 7;
const WINDOW_UPDATE: u8 = 8;
const CONTINUATION: u8 = 9;
const END_STREAM: u8 = 1;
const ACK: u8 = 1;
const END_HEADERS: u8 = 4;
const PADDED: u8 = 8;
const PRIORITY: u8 = 0x20;

fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let n = payload.len();
    let mut f = vec![(n >> 16) as u8, (n >> 8) as u8, n as u8, kind, flags];
    f.extend_from_slice(&(stream & 0x7fff_ffff).to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// A literal header with no indexing and a new name, nothing Huffman coded:
/// our side of HPACK needs no state.
fn literal(out: &mut Vec<u8>, name: &str, value: &str) {
    out.push(0);
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
}

fn response_headers() -> Vec<u8> {
    let mut h = vec![0x88]; // :status 200, indexed
    literal(&mut h, "content-type", "application/grpc");
    h
}

fn trailers(status: u8) -> Vec<u8> {
    let mut h = Vec::new();
    literal(&mut h, "grpc-status", &status.to_string());
    h
}

fn grpc_message(m: &[u8]) -> Vec<u8> {
    let mut f = vec![0];
    f.extend_from_slice(&(m.len() as u32).to_be_bytes());
    f.extend_from_slice(m);
    f
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Send,
    Consume,
    Publish,
    Ping,
}

struct Stream {
    kind: Kind,
    buf: Vec<u8>,
}

fn route(path: &[u8]) -> Option<Kind> {
    Some(match path {
        b"/eve_public.gateway.Requests/Send" => Kind::Send,
        b"/eve_public.gateway.Notices/Consume" => Kind::Consume,
        b"/eve_public.gateway.Events/Publish" => Kind::Publish,
        b"/eve_public.gateway.Requests/Ping"
        | b"/eve_public.gateway.Notices/Ping"
        | b"/eve_public.gateway.Events/Ping" => Kind::Ping,
        _ => return None,
    })
}

fn read_exact_or_eof(s: &mut TcpStream, buf: &mut [u8]) -> io::Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        match s.read(&mut buf[got..]) {
            Ok(0) if got == 0 => return Ok(false),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

// ---- notices ----------------------------------------------------------------

/// The write side of one connection. Everything that writes to the socket,
/// the serve loop and [`publish`] alike, goes through the mutex around this,
/// so frames never interleave.
struct Conn {
    sock: TcpStream,
    /// The pilot, learned from the first `Send` on the connection (0 until).
    character: i64,
    /// What the client has left us to send on the connection, h2 flow control.
    window: i64,
    /// `SETTINGS_INITIAL_WINDOW_SIZE` from the client (default 65535).
    initial: i64,
    /// Open `Consume` streams and the window left on each.
    consume: HashMap<u32, i64>,
    dead: bool,
}

type Shared = Arc<Mutex<Conn>>;

/// Write `out` to the connection under its lock. DATA frames in it are
/// counted against the connection window.
fn write_out(c: &Shared, out: &[u8]) -> io::Result<()> {
    let mut g = c.lock().unwrap_or_else(|e| e.into_inner());
    let mut p = 0;
    while p + 9 <= out.len() {
        let n = usize::from(out[p]) << 16 | usize::from(out[p + 1]) << 8 | usize::from(out[p + 2]);
        if out[p + 3] == DATA {
            g.window -= n as i64;
        }
        p += 9 + n;
    }
    g.sock.write_all(out)
}

struct Sub {
    conn: Shared,
    stream: u32,
}

/// Every open `Notices/Consume` stream. Cloning shares it.
#[derive(Clone, Default)]
pub struct NoticeHub(Arc<Mutex<Vec<Sub>>>);

/// Who a notice is for. The group variants carry the member pilots, which the
/// caller resolves from the world BEFORE publishing, so the hub never holds a
/// world lock; the id only goes into the envelope's `target_group`.
pub enum Target {
    Character(i64),
    Corporation { id: i64, members: Vec<i64> },
    Alliance { id: i64, members: Vec<i64> },
    SolarSystem { id: i64, members: Vec<i64> },
}

impl Target {
    /// (`target_group` field number, the id, the pilots it reaches).
    fn parts(&self) -> (u64, i64, Vec<i64>) {
        match self {
            Target::Character(c) => (4, *c, vec![*c]),
            Target::Corporation { id, members } => (5, *id, members.clone()),
            Target::Alliance { id, members } => (6, *id, members.clone()),
            Target::SolarSystem { id, members } => (2, *id, members.clone()),
        }
    }
}

/// The `eve_public.Notice` envelope **[V: gw-protos.txt]**: dispatched 1, uuid 2,
/// payload 5 (Any), target_group 6. `TargetGroup` is a oneof of **scalars**:
/// `solar_system` 2, `character` 4, `corporation` 5 (uint32) and `alliance` 6
/// (int32), each a varint. (It was written as a nested message once; the
/// client's protobuf reader then saw no group at all, and
/// `noticeConsumer._validate_target_group_against_session` passed it as
/// `None == None`, so every notice was dispatched to whoever received it.
/// With the scalar the client checks the id against its own session.)
fn notice(ty: &str, payload: &[u8], group: u64, id: i64) -> Vec<u8> {
    static N: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut uuid = [0u8; 16];
    uuid[..8].copy_from_slice(&nanos.to_le_bytes());
    uuid[8..].copy_from_slice(&N.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &timestamp(now_secs()));
    put_bytes(&mut out, 2, &uuid);
    let mut any = Vec::new();
    put_bytes(&mut any, 1, format!("type.googleapis.com/{ty}").as_bytes());
    put_bytes(&mut any, 2, payload);
    put_bytes(&mut out, 5, &any);
    let mut tg = Vec::new();
    put_uint(&mut tg, group, id as u64);
    put_bytes(&mut out, 6, &tg);
    out
}

/// Send one notice to every `Consume` stream whose pilot `target` reaches.
/// A stream whose write fails, or whose connection or stream is gone, is
/// dropped; a stream whose flow-control window cannot take the notice is
/// skipped (the notice is lost to it, not queued). Returns how many streams
/// got it.
pub fn publish(hub: &NoticeHub, ty: &str, payload: &[u8], target: Target) -> usize {
    let (group, id, members) = target.parts();
    let msg = grpc_message(&notice(ty, payload, group, id));
    let mut sent = 0;
    let mut subs = hub.0.lock().unwrap_or_else(|e| e.into_inner());
    subs.retain(|s| {
        let mut c = s.conn.lock().unwrap_or_else(|e| e.into_inner());
        if c.dead {
            return false;
        }
        let Some(&room) = c.consume.get(&s.stream) else {
            return false;
        };
        if c.character == 0 || !members.contains(&c.character) {
            return true;
        }
        let n = msg.len() as i64;
        if room < n || c.window < n {
            return true;
        }
        let mut wire = Vec::new();
        for chunk in msg.chunks(16384) {
            wire.extend(frame(DATA, 0, s.stream, chunk));
        }
        if c.sock.write_all(&wire).is_err() {
            c.dead = true;
            return false;
        }
        c.window -= n;
        if let Some(r) = c.consume.get_mut(&s.stream) {
            *r -= n;
        }
        sent += 1;
        true
    });
    sent
}

/// Serve one connection until it ends. `log` gets one line per request class
/// the first time it is seen (the inventory of what the client asks).
pub fn serve(
    s: TcpStream,
    instance: [u8; 16],
    seen: &mut Vec<String>,
    ground: &Ground,
    hub: &NoticeHub,
) -> io::Result<()> {
    s.set_nodelay(true)?;
    let conn: Shared = Arc::new(Mutex::new(Conn {
        sock: s.try_clone()?,
        character: 0,
        window: 65535,
        initial: 65535,
        consume: HashMap::new(),
        dead: false,
    }));
    let r = run(s, &conn, instance, seen, ground, hub);
    conn.lock().unwrap_or_else(|e| e.into_inner()).dead = true;
    r
}

fn run(
    mut s: TcpStream,
    conn: &Shared,
    instance: [u8; 16],
    seen: &mut Vec<String>,
    ground: &Ground,
    hub: &NoticeHub,
) -> io::Result<()> {
    s.set_read_timeout(Some(IDLE))?;
    let mut pre = [0u8; 24];
    if !read_exact_or_eof(&mut s, &mut pre)? || pre != PREFACE {
        return Ok(());
    }
    write_out(conn, &frame(SETTINGS, 0, 0, &[]))?;
    let mut hpack = Hpack::new();
    let mut streams: HashMap<u32, Stream> = HashMap::new();
    // A header block split over CONTINUATION frames.
    let mut pending: Option<(u32, u8, Vec<u8>)> = None;
    loop {
        let mut h = [0u8; 9];
        if !read_exact_or_eof(&mut s, &mut h)? {
            return Ok(());
        }
        let len = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
        if len > MAX_FRAME {
            return Ok(());
        }
        let (kind, flags) = (h[3], h[4]);
        let sid = u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff;
        let mut body = vec![0u8; len];
        if len > 0 && !read_exact_or_eof(&mut s, &mut body)? {
            return Ok(());
        }
        let mut out: Vec<u8> = Vec::new();
        // Consume streams opened in this round, registered once their
        // headers are on the wire.
        let mut opened: Vec<u32> = Vec::new();
        match kind {
            SETTINGS if flags & ACK == 0 => {
                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                for e in body.chunks_exact(6) {
                    if u16::from_be_bytes([e[0], e[1]]) == 4 {
                        let new = i64::from(u32::from_be_bytes([e[2], e[3], e[4], e[5]]));
                        let delta = new - c.initial;
                        c.initial = new;
                        for w in c.consume.values_mut() {
                            *w += delta;
                        }
                    }
                }
                drop(c);
                out.extend(frame(SETTINGS, ACK, 0, &[]));
            }
            WINDOW_UPDATE if body.len() == 4 => {
                let inc = i64::from(
                    u32::from_be_bytes([body[0], body[1], body[2], body[3]]) & 0x7fff_ffff,
                );
                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                if sid == 0 {
                    c.window += inc;
                } else if let Some(w) = c.consume.get_mut(&sid) {
                    *w += inc;
                }
            }
            PING if flags & ACK == 0 => out.extend(frame(PING, ACK, 0, &body)),
            GOAWAY => return Ok(()),
            RST_STREAM => {
                streams.remove(&sid);
                conn.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .consume
                    .remove(&sid);
            }
            HEADERS | CONTINUATION => {
                let mut b: &[u8] = &body;
                let mut fl = flags;
                if kind == HEADERS {
                    if fl & PADDED != 0 {
                        let Some((&pad, rest)) = b.split_first() else {
                            return Ok(());
                        };
                        let Some(keep) = rest.len().checked_sub(usize::from(pad)) else {
                            return Ok(());
                        };
                        b = &rest[..keep];
                    }
                    if fl & PRIORITY != 0 {
                        b = b.get(5..).unwrap_or(&[]);
                    }
                    pending = Some((sid, fl, b.to_vec()));
                } else if let Some((_, _, blk)) = pending.as_mut() {
                    blk.extend_from_slice(b);
                    fl |= flags & END_HEADERS;
                }
                if fl & END_HEADERS != 0
                    && let Some((id, first_flags, blk)) = pending.take()
                {
                    let Some(path) = hpack.decode(&blk) else {
                        return Ok(()); // a compression error ends the connection
                    };
                    let known = streams.contains_key(&id);
                    if known {
                        // Trailers from the client: it is done sending.
                        if first_flags & END_STREAM != 0 {
                            finish(&mut streams, id, &mut out);
                        }
                    } else if let Some(k) = path.as_deref().and_then(route) {
                        streams.insert(
                            id,
                            Stream {
                                kind: k,
                                buf: Vec::new(),
                            },
                        );
                        out.extend(frame(HEADERS, END_HEADERS, id, &response_headers()));
                        if k == Kind::Consume {
                            opened.push(id);
                        }
                        if first_flags & END_STREAM != 0 {
                            finish(&mut streams, id, &mut out);
                        }
                    } else {
                        // Unknown method: trailers-only, UNIMPLEMENTED.
                        let mut t = response_headers();
                        literal(&mut t, "grpc-status", "12");
                        out.extend(frame(HEADERS, END_HEADERS | END_STREAM, id, &t));
                    }
                }
            }
            DATA => {
                let mut b: &[u8] = &body;
                if flags & PADDED != 0 {
                    let Some((&pad, rest)) = b.split_first() else {
                        return Ok(());
                    };
                    let Some(keep) = rest.len().checked_sub(usize::from(pad)) else {
                        return Ok(());
                    };
                    b = &rest[..keep];
                }
                if len > 0 {
                    // Keep both windows open: a long-lived Send stream
                    // carries far more than the initial 65535 bytes.
                    let inc = (len as u32).to_be_bytes();
                    out.extend(frame(WINDOW_UPDATE, 0, 0, &inc));
                    if streams.contains_key(&sid) && flags & END_STREAM == 0 {
                        out.extend(frame(WINDOW_UPDATE, 0, sid, &inc));
                    }
                }
                if let Some(st) = streams.get_mut(&sid) {
                    st.buf.extend_from_slice(b);
                    while st.buf.len() >= 5 {
                        let n = u32::from_be_bytes([st.buf[1], st.buf[2], st.buf[3], st.buf[4]])
                            as usize;
                        if n > MAX_FRAME {
                            return Ok(());
                        }
                        if st.buf.len() < 5 + n {
                            break;
                        }
                        let msg: Vec<u8> = st.buf[5..5 + n].to_vec();
                        let compressed = st.buf[0] != 0;
                        st.buf.drain(..5 + n);
                        match st.kind {
                            Kind::Send if !compressed => {
                                let who = request_character(&msg);
                                if who != 0 {
                                    let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                                    if c.character == 0 {
                                        c.character = who;
                                    }
                                }
                                let (resp, ty, status) = respond(&msg, &instance, ground);
                                if !seen.contains(&ty) {
                                    crate::outln!("[gateway] request class {ty} -> {status}");
                                    seen.push(ty);
                                }
                                out.extend(frame(DATA, 0, sid, &grpc_message(&resp)));
                            }
                            Kind::Publish | Kind::Ping => {
                                out.extend(frame(DATA, 0, sid, &grpc_message(&[])));
                            }
                            _ => {}
                        }
                    }
                }
                if flags & END_STREAM != 0 {
                    finish(&mut streams, sid, &mut out);
                }
            }
            _ => {}
        }
        if !out.is_empty() {
            write_out(conn, &out)?;
        }
        for id in opened {
            let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
            let w = c.initial;
            c.consume.insert(id, w);
            drop(c);
            hub.0.lock().unwrap_or_else(|e| e.into_inner()).push(Sub {
                conn: Arc::clone(conn),
                stream: id,
            });
        }
    }
}

/// The client has half-closed `id`: end every stream but `Consume`, which
/// stays open for notices we never send.
fn finish(streams: &mut HashMap<u32, Stream>, id: u32, out: &mut Vec<u8>) {
    if streams.get(&id).is_some_and(|s| s.kind != Kind::Consume) {
        streams.remove(&id);
        out.extend(frame(HEADERS, END_HEADERS | END_STREAM, id, &trailers(0)));
    }
}

/// Bind `addr` and serve in the background. The bound address.
pub fn spawn(addr: SocketAddr, ground: Ground) -> io::Result<SocketAddr> {
    spawn_with(addr, ground, NoticeHub::default())
}

/// [`spawn`] with the caller's [`NoticeHub`], so it can [`publish`].
pub fn spawn_with(addr: SocketAddr, ground: Ground, hub: NoticeHub) -> io::Result<SocketAddr> {
    let socket = TcpListener::bind(addr)?;
    let bound = socket.local_addr()?;
    let live = Arc::new(AtomicUsize::new(0));
    let instance = {
        // Any 16 bytes will do; vary them by start time.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        t.to_le_bytes()
    };
    std::thread::Builder::new()
        .name("gateway-accept".to_owned())
        .spawn(move || {
            for conn in socket.incoming() {
                let Ok(stream) = conn else {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                };
                if live.fetch_add(1, Ordering::SeqCst) >= CAP {
                    live.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let live = Arc::clone(&live);
                let ground = ground.clone();
                let hub = hub.clone();
                let _ = std::thread::Builder::new()
                    .name("gateway-conn".to_owned())
                    .spawn(move || {
                        let mut seen = Vec::new();
                        let _ = serve(stream, instance, &mut seen, &ground, &hub);
                        live.fetch_sub(1, Ordering::SeqCst);
                    });
            }
        })?;
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn huffman_matches_the_rfc_examples() {
        // RFC 7541 C.4.1, C.4.2, C.6.1.
        assert_eq!(
            huffman_decode(&hex("f1e3c2e5f23a6ba0ab90f4ff")).unwrap(),
            b"www.example.com"
        );
        assert_eq!(huffman_decode(&hex("a8eb10649cbf")).unwrap(), b"no-cache");
        assert_eq!(
            huffman_decode(&hex("25a849e95ba97d7f")).unwrap(),
            b"custom-key"
        );
        // Bad padding is refused.
        assert!(huffman_decode(&hex("f1e3c2e5f23a6ba0ab90f4fe")).is_none());
    }

    #[test]
    fn hpack_indexes_into_its_dynamic_table() {
        let mut h = Hpack::new();
        // Literal with incremental indexing, name from static index 4.
        let mut b = vec![0x44, 3];
        b.extend_from_slice(b"/ab");
        assert_eq!(h.decode(&b).unwrap().unwrap(), b"/ab");
        // Index 62 is now that entry.
        assert_eq!(h.decode(&[0xbe]).unwrap().unwrap(), b"/ab");
        // A size update to 0 empties it, and the index is then invalid.
        assert!(h.decode(&[0x20]).unwrap().is_none());
        assert!(h.decode(&[0xbe]).is_none());
    }

    fn request(ty: &str, corr: &[u8]) -> Vec<u8> {
        request_for(ty, &[], corr, 0)
    }

    /// A request from pilot `character` (0: no context) with a payload.
    fn request_for(ty: &str, value: &[u8], corr: &[u8], character: u64) -> Vec<u8> {
        let mut any = Vec::new();
        put_bytes(&mut any, 1, format!("type.googleapis.com/{ty}").as_bytes());
        if !value.is_empty() {
            put_bytes(&mut any, 2, value);
        }
        let mut r = Vec::new();
        put_bytes(&mut r, 2, corr);
        put_bytes(&mut r, 3, &any);
        if character != 0 {
            let mut id = Vec::new();
            put_uint(&mut id, 1, character);
            let mut who = Vec::new();
            put_bytes(&mut who, 1, &id);
            let mut ctx = Vec::new();
            put_bytes(&mut ctx, 10, &who);
            put_bytes(&mut r, 6, &ctx);
        }
        r
    }

    fn decoded(resp: &[u8]) -> (u64, Vec<u8>, Option<String>) {
        let f = fields(resp);
        let get = |n| f.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone());
        let status = u64::from_le_bytes(get(3).unwrap().try_into().unwrap());
        let ty = get(5).and_then(|any| {
            fields(&any)
                .into_iter()
                .find(|(k, _)| *k == 1)
                .map(|(_, v)| String::from_utf8(v).unwrap())
        });
        (status, get(2).unwrap_or_default(), ty)
    }

    /// The `Any.value` of a response.
    fn payload(resp: &[u8]) -> Vec<u8> {
        get_bytes(&get_bytes(resp, 5).unwrap_or_default(), 2).unwrap_or_default()
    }

    /// Ask `ty` as pilot `who` and return `(status, payload)`.
    fn ask(g: &Ground, ty: &str, value: &[u8], who: u64) -> (u64, Vec<u8>) {
        let (r, _, _) = respond(&request_for(ty, value, b"x", who), &[0; 16], g);
        (decoded(&r).0, payload(&r))
    }

    #[test]
    fn answers_by_request_class() {
        let g = Ground::local();
        let (r, ty, status) = respond(
            &request("eve_public.cosmetic.ship.api.GetRequest", b"c1"),
            &[0; 16],
            &g,
        );
        assert_eq!(ty, "eve_public.cosmetic.ship.api.GetRequest");
        assert_eq!(status, 404);
        assert_eq!(decoded(&r), (404, b"c1".to_vec(), None));
        let (r, _, _) = respond(
            &request(
                "eve_public.sovereignty.mercenaryden.activity.api.GetAllRequest",
                b"c2",
            ),
            &[0; 16],
            &g,
        );
        assert_eq!(
            decoded(&r),
            (
                200,
                b"c2".to_vec(),
                Some(
                    "type.googleapis.com/eve_public.sovereignty.mercenaryden.activity.api.GetAllResponse"
                        .to_owned()
                )
            )
        );
        let (r, _, _) = respond(
            &request("eve_public.invented.thing.api.GetRequest", b"c3"),
            &[0; 16],
            &g,
        );
        assert_eq!(
            decoded(&r),
            (
                200,
                b"c3".to_vec(),
                Some("type.googleapis.com/eve_public.invented.thing.api.GetResponse".to_owned())
            )
        );
        // Not a *Request name: payload unset.
        let (r, _, _) = respond(&request("eve_public.invented.Thing", b"c4"), &[0; 16], &g);
        assert_eq!(decoded(&r), (200, b"c4".to_vec(), None));
        // Corruption: total_progress = Fraction { denominator: 1 }.
        let (_, value) = ask(
            &g,
            "eve_public.pirate.corruption.api.GetSystemInfoRequest",
            &[],
            0,
        );
        let progress = get_bytes(&value, 1).unwrap();
        assert_eq!(progress, vec![0x10, 1]);
    }

    #[test]
    fn workforce_hub_answers_set_a_mode() {
        let g = Ground::local();
        for ty in [
            "eve_public.sovereignty.hub.workforce.api.GetConfigurationRequest",
            "eve_public.sovereignty.hub.workforce.api.GetStateRequest",
        ] {
            let (status, value) = ask(&g, ty, &[], 0);
            assert_eq!(status, 200, "{ty}");
            let mode = get_bytes(&value, 1).expect(ty);
            // Exactly one of the oneof fields is set; which one is [A].
            let set = (1..=4)
                .filter(|f| get_uint(&mode, *f).is_some() || get_bytes(&mode, *f).is_some())
                .count();
            assert_eq!(set, 1, "{ty}");
        }
    }

    #[test]
    fn single_records_are_not_found_and_content_checkpoints_too() {
        let g = Ground::local();
        for ty in [
            "eve_public.cosmetic.corporation.palette.api.GetRequest",
            "eve_public.cosmetic.corporation.palette.api.GetOwnRequest",
            "eve_public.cosmetic.structure.paintwork.api.GetRequest",
            "eve_public.stargate.lock.api.GetRequest",
            "eve_public.sovereignty.mercenaryden.api.GetAsOwnerRequest",
            "eve_public.invented.content.api.cdn.GetLatestRequest",
            "eve_public.invented.title.definition.api.GetLatestRequest",
            "eve_public.invented.space.api.cdn.GetUpdateRequest",
        ] {
            assert_eq!(ask(&g, ty, &[], 7), (404, vec![]), "{ty}");
        }
        // A list of the same thing is an empty list, not a 404.
        assert_eq!(
            ask(
                &g,
                "eve_public.stargate.lock.api.GetRestrictedSystemsRequest",
                &[],
                7
            ),
            (200, vec![])
        );
        assert_eq!(
            ask(
                &g,
                "eve_public.cosmetic.corporation.palette.api.SetRequest",
                &[],
                7
            )
            .0,
            403
        );
    }

    #[test]
    fn small_classes_have_their_own_answers() {
        let g = Ground::local();
        // The omega licence runs for years; any other licence is not found.
        let mut kind = Vec::new();
        put_bytes(&mut kind, 1, b"eve_clonestate_omega");
        let mut req = Vec::new();
        put_bytes(&mut req, 1, &kind);
        let (status, v) = ask(&g, "eve_public.user.license.api.GetRequest", &req, 7);
        assert_eq!(status, 200);
        let attrs = get_bytes(&v, 1).unwrap();
        let expiry = get_uint(&get_bytes(&attrs, 2).unwrap(), 1).unwrap();
        assert!(expiry > now_secs() + 365 * 86_400);
        assert!(get_bytes(&attrs, 3).is_some());
        let mut other = Vec::new();
        put_bytes(&mut other, 1, b"invented_license");
        let mut req = Vec::new();
        put_bytes(&mut req, 1, &other);
        assert_eq!(
            ask(&g, "eve_public.user.license.api.GetRequest", &req, 7).0,
            404
        );
        // Title: the no_title branch.
        let (_, v) = ask(
            &g,
            "eve_public.cosmetic.character.title.api.GetEquippedForCharacterRequest",
            &[],
            7,
        );
        assert_eq!(get_uint(&v, 2), Some(1));
        assert_eq!(get_bytes(&v, 1), None);
        // PLEX vault: a balance message, zero.
        let (_, v) = ask(&g, "eve_public.plex.vault.api.BalanceRequest", &[], 7);
        assert_eq!(
            get_uint(&get_bytes(&v, 1).unwrap(), 1),
            Some(PLEX_BALANCE_CENTS)
        );
        // Faction scores echo the asked system.
        let mut sys = Vec::new();
        put_uint(&mut sys, 1, 1234);
        let mut req = Vec::new();
        put_bytes(&mut req, 1, &sys);
        let (_, v) = ask(
            &g,
            "eve_public.faction.activity.GetSolarSystemScoresRequest",
            &req,
            7,
        );
        assert_eq!(get_bytes(&v, 1), Some(sys));
        // Career goals: one per path, each with a target and its career.
        let (_, v) = ask(
            &g,
            "eve_public.career.goal.api.GetDefinitionsRequest",
            &[],
            7,
        );
        let goals = field(&v, 1);
        assert_eq!(goals.len(), 4);
        let mut careers: Vec<u64> = goals
            .iter()
            .map(|goal| get_uint(&get_bytes(goal, 2).unwrap(), 9).unwrap())
            .collect();
        careers.sort_unstable();
        assert_eq!(careers, vec![1, 2, 3, 4]);
        assert!(
            goals
                .iter()
                .all(|goal| { get_bytes(&get_bytes(goal, 1).unwrap(), 1).unwrap().len() == 16 })
        );
    }

    fn read_frame(s: &mut TcpStream) -> (u8, u8, u32, Vec<u8>) {
        let mut h = [0u8; 9];
        s.read_exact(&mut h).unwrap();
        let len = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
        let mut b = vec![0u8; len];
        s.read_exact(&mut b).unwrap();
        (h[3], h[4], u32::from_be_bytes([h[5], h[6], h[7], h[8]]), b)
    }

    fn open_stream(path: &str, id: u32, index: bool) -> Vec<u8> {
        let mut blk = Vec::new();
        if index {
            blk.push(0x44);
            blk.push(path.len() as u8);
            blk.extend_from_slice(path.as_bytes());
        } else {
            blk.push(0xbe); // the entry the first stream added
        }
        frame(HEADERS, END_HEADERS, id, &blk)
    }

    #[test]
    fn serves_send_over_h2c_and_leaves_consume_open() {
        let hub = NoticeHub::default();
        let addr =
            spawn_with("127.0.0.1:0".parse().unwrap(), Ground::local(), hub.clone()).unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut out = PREFACE.to_vec();
        out.extend(frame(SETTINGS, 0, 0, &[]));
        out.extend(open_stream("/eve_public.gateway.Requests/Send", 1, true));
        out.extend(frame(
            DATA,
            0,
            1,
            &grpc_message(&request("eve_public.cosmetic.ship.api.GetRequest", b"k9")),
        ));
        // A path that is not routed ends in UNIMPLEMENTED, trailers only.
        out.extend(open_stream("/eve_public.gateway.Nothing/Here", 3, true));
        // The dynamic entry from stream 1 (now the newest one is the other
        // path, index 62; Send is 63) routes a second Send stream.
        out.extend(frame(HEADERS, END_HEADERS, 5, &[0xbf]));
        out.extend(frame(
            DATA,
            END_STREAM,
            5,
            &grpc_message(&request("eve_public.invented.thing.api.GetRequest", b"k8")),
        ));
        // A Send naming invented pilot 9000001 teaches the connection who it
        // is; a Consume stream then registers with the hub.
        let m = request_for(
            "eve_public.invented.thing.api.GetRequest",
            b"",
            b"k7",
            9000001,
        );
        out.extend(frame(DATA, 0, 1, &grpc_message(&m)));
        out.extend(open_stream("/eve_public.gateway.Notices/Consume", 7, true));
        s.write_all(&out).unwrap();
        let mut got_data = None;
        let mut unimplemented = false;
        let mut headers_on_1 = false;
        let mut done5 = false;
        let mut headers_on_7 = false;
        for _ in 0..16 {
            let (kind, flags, sid, body) = read_frame(&mut s);
            match (kind, sid) {
                (HEADERS, 1) => headers_on_1 = true,
                (HEADERS, 7) => headers_on_7 = true,
                (DATA, 1) if got_data.is_some() => {}
                (DATA, 1) => got_data = Some(body),
                (HEADERS, 3) if flags & END_STREAM != 0 => unimplemented = true,
                // Trailers after the one response: the half-close was seen.
                (HEADERS, 5) if flags & END_STREAM != 0 => done5 = true,
                _ => {}
            }
            if got_data.is_some() && unimplemented && done5 && headers_on_7 {
                break;
            }
        }
        assert!(headers_on_1, "response headers precede the data");
        let body = got_data.expect("a response");
        assert!(done5);
        assert_eq!(body[0], 0);
        let (status, corr, ty) = decoded(&body[5..]);
        assert_eq!((status, corr, ty), (404, b"k9".to_vec(), None));
        assert!(unimplemented);
        assert!(headers_on_7);

        // Publish: the right pilot gets it, another gets nothing. The Send
        // above may still be in flight, so retry for the one that must land.
        let ty = "eve_public.invented.notice.Thing";
        let mut sent = 0;
        for _ in 0..200 {
            sent = publish(&hub, ty, b"pp", Target::Character(9000001));
            if sent > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(sent, 1);
        assert_eq!(publish(&hub, ty, b"pp", Target::Character(9000002)), 0);
        let members = vec![9000001];
        assert_eq!(
            publish(&hub, ty, b"qq", Target::Corporation { id: 5, members }),
            1
        );
        let mut notices = Vec::new();
        while notices.len() < 2 {
            let (kind, _, sid, body) = read_frame(&mut s);
            if kind == DATA && sid == 7 {
                notices.push(body);
            }
        }
        let env = &notices[0][5..];
        let any = get_bytes(env, 5).unwrap();
        assert!(type_name(&get_bytes(&any, 1).unwrap()).ends_with("Thing"));
        assert_eq!(get_bytes(&any, 2).unwrap(), b"pp");
        assert_eq!(get_uint(&get_bytes(env, 6).unwrap(), 4), Some(9000001));
        assert_eq!(get_bytes(env, 2).unwrap().len(), 16);
        let env = &notices[1][5..];
        assert_eq!(get_uint(&get_bytes(env, 6).unwrap(), 5), Some(5));
    }

    /// `Notice.TargetGroup` is a oneof of varint scalars (gw-protos.txt:
    /// `solar_system` 2, `character` 4, `corporation` 5, `alliance` 6): the
    /// id is the field's own value, never a nested message.
    #[test]
    fn the_target_group_is_a_scalar_the_client_can_read() {
        for (target, field, id) in [
            (Target::Character(7), 4u64, 7i64),
            (
                Target::SolarSystem {
                    id: 8,
                    members: vec![],
                },
                2,
                8,
            ),
            (
                Target::Corporation {
                    id: 9,
                    members: vec![],
                },
                5,
                9,
            ),
            (
                Target::Alliance {
                    id: 10,
                    members: vec![],
                },
                6,
                10,
            ),
        ] {
            let (group, gid, _) = target.parts();
            let env = notice("a.B", b"p", group, gid);
            let tg = get_bytes(&env, 6).expect("a target_group");
            assert_eq!(get_uint(&tg, field), Some(id as u64));
            assert!(
                get_bytes(&tg, field).is_none(),
                "not a length-delimited field"
            );
        }
    }

    /// GW-OUT: a notice queued on the world's outbox reaches a subscribed
    /// `Consume` stream once the drained batch is published, and only the
    /// pilot it names gets it (`gateway_out`).
    #[test]
    fn an_outbox_entry_reaches_a_subscribed_stream_after_a_drain() {
        let hub = NoticeHub::default();
        let addr =
            spawn_with("127.0.0.1:0".parse().unwrap(), Ground::local(), hub.clone()).unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut out = PREFACE.to_vec();
        out.extend(frame(SETTINGS, 0, 0, &[]));
        out.extend(open_stream("/eve_public.gateway.Requests/Send", 1, true));
        let m = request_for("eve_public.invented.thing.api.GetRequest", b"", b"k1", 7001);
        out.extend(frame(DATA, 0, 1, &grpc_message(&m)));
        out.extend(open_stream("/eve_public.gateway.Notices/Consume", 3, true));
        s.write_all(&out).unwrap();
        let ty = "eve_public.invented.notice.Echo";
        let mut w = crate::service::Registry::new(crate::model::SandboxConfig::default());
        crate::gateway_out::queue(&mut w, ty, b"hello".to_vec(), Target::Character(7001));
        crate::gateway_out::queue(&mut w, ty, b"other".to_vec(), Target::Character(7002));
        let batch = crate::gateway_out::take(&mut w);
        assert_eq!(batch.len(), 2);
        assert!(crate::gateway_out::take(&mut w).is_empty());
        // The guard is gone (the registry is a plain value here); the Send
        // above may still be in flight, so retry until the stream is known.
        let mut sent = 0;
        for _ in 0..200 {
            let again = batch
                .iter()
                .map(|(t, p, tg)| {
                    let tg = match tg {
                        Target::Character(c) => Target::Character(*c),
                        _ => unreachable!("pilots only"),
                    };
                    (t.clone(), p.clone(), tg)
                })
                .collect();
            sent = crate::gateway_out::publish_all(&hub, again);
            if sent > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(sent, 1, "7001 got it; 7002 has no stream");
        loop {
            let (kind, _, sid, body) = read_frame(&mut s);
            if kind == DATA && sid == 3 {
                let any = get_bytes(&body[5..], 5).unwrap();
                assert_eq!(get_bytes(&any, 2).unwrap(), b"hello");
                break;
            }
        }
    }

    /// A skill plan made and read back over a real h2c socket, as one pilot,
    /// on one long-lived `Send` stream: the same stream carries both calls.
    #[test]
    fn a_skill_plan_round_trips_over_h2c() {
        let addr = spawn("127.0.0.1:0".parse().unwrap(), Ground::local()).unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut attrs = Vec::new();
        put_bytes(&mut attrs, 1, b"Over the wire");
        let mut create = Vec::new();
        put_bytes(&mut create, 1, &attrs);
        let mut out = PREFACE.to_vec();
        out.extend(frame(SETTINGS, 0, 0, &[]));
        out.extend(open_stream("/eve_public.gateway.Requests/Send", 1, true));
        let ty = "eve_public.character.skill.plan";
        for (name, body, corr) in [
            ("CreateRequest", create, b"a".as_slice()),
            ("GetAllRequest", Vec::new(), b"b".as_slice()),
        ] {
            let m = request_for(&format!("{ty}.{name}"), &body, corr, 42);
            out.extend(frame(DATA, 0, 1, &grpc_message(&m)));
        }
        s.write_all(&out).unwrap();
        let mut replies: Vec<Vec<u8>> = Vec::new();
        while replies.len() < 2 {
            let (kind, _, sid, body) = read_frame(&mut s);
            if kind == DATA && sid == 1 {
                replies.push(body[5..].to_vec());
            }
        }
        let (status, corr, _) = decoded(&replies[0]);
        assert_eq!((status, corr), (200, b"a".to_vec()));
        let made = get_bytes(&payload(&replies[0]), 1).unwrap();
        let (status, corr, kind) = decoded(&replies[1]);
        assert_eq!((status, corr), (200, b"b".to_vec()));
        assert!(
            kind.unwrap()
                .ends_with("character.skill.plan.GetAllResponse")
        );
        assert_eq!(field(&payload(&replies[1]), 1), vec![made]);
    }
}
