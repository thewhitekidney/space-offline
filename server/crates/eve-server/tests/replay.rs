//! Replay the committed captures against the dispatcher.
//!
//! **The failure mode this exists for is a call we used to answer and no
//! longer do.** Every answer in this server was worked out from one client
//! session, and the only record of that session is the capture. Four of them
//! are committed under `captures/regression/` precisely because they cannot be
//! regenerated -- and until now nothing read them back. A refactor that moved
//! an arm into the wrong subsystem module, or an arm deleted as dead, would be
//! found by the owner relaunching the client, which costs an evening.
//!
//! This is tier 1 of the replay in `docs/archive/architecture-sweep.md`: it asserts
//! only that **every call is answered**, not that the answer is the same one.
//! Values are deliberately not compared, for two reasons. Some answers embed
//! the wall clock (`machoNet.GetTime`, `GetDirtTimestamp`), which is why the
//! sweep lists a clock seam before tier 2. And more importantly, a changed
//! *value* is usually the work: pinning every byte of every answer against a
//! capture would make every improvement a test failure. A missing *answer* is
//! never the work. That asymmetry is the whole design of this file.
//!
//! Bound calls are replayed too, resolved to the service that handed the
//! object out the same way `evecap --calls` does it -- so `List [N=1:11]` is
//! dispatched as `invbroker::inventory.List` and not skipped.

use eve_marshal::{MachoNetMsg, PyValue};
use eve_proto::{CallPayload, MachoPacket};
use eve_server::capture::{Direction, read_all};
use eve_server::model::SandboxConfig;
use eve_server::service::{Registry, Request, ServiceError};
use eve_server::{dumps, loads};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn regression_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../captures/regression")
        .canonicalize()
        .expect("the committed captures must be where the README says they are")
}

/// The object ids a reply hands out, as `evecap` reads them.
fn object_ids(v: &PyValue, out: &mut Vec<String>) {
    match v {
        // An object reference is a `Callback` wrapping a pickled
        // `(objectID, refID)` -- `eve_proto::object_reference`. The id is
        // inside a nested marshal stream, which is why this has to load
        // again rather than pattern-match through.
        PyValue::Callback(inner) => {
            if let PyValue::Bytes(pickle) = &**inner
                && let Ok(PyValue::Tuple(pair)) = loads(pickle)
                && let Some(PyValue::Bytes(id)) = pair.first()
            {
                out.push(String::from_utf8_lossy(id).into_owned());
            }
        }
        PyValue::Tuple(items) | PyValue::List(items) => {
            for i in items {
                object_ids(i, out);
            }
        }
        _ => {}
    }
}

struct Replay {
    /// Calls dispatched, by `service.method`.
    answered: u64,
    /// `service.method` we refused, which is the only failure this test has.
    refused: Vec<String>,
    /// Bound calls whose object we could not attribute, so could not replay.
    unresolved: u64,
}

fn replay(path: &Path) -> Replay {
    let records = read_all(path).expect("a committed capture must be readable");
    let mut reg = Registry::new(SandboxConfig::default());
    let mut objects: BTreeMap<String, String> = BTreeMap::new();
    // callID -> the service we are waiting on, so a reply can be attributed.
    let mut pending: BTreeMap<i64, String> = BTreeMap::new();
    let mut out = Replay {
        answered: 0,
        refused: Vec::new(),
        unresolved: 0,
    };

    for r in &records {
        let Ok(value) = loads(&r.bytes) else { continue };
        let Ok(packet) = MachoPacket::from_value(&value) else {
            continue;
        };

        if r.direction == Direction::ServerToClient {
            // Harvest the object ids our own past answers handed out, so the
            // bound calls that follow can be attributed to a service.
            if packet.command == MachoNetMsg::CallRsp
                && let Some(call_id) = packet.destination.call_id()
                && let Some(service) = pending.remove(&call_id)
                && let Some(PyValue::Bytes(pickle)) = packet.body.first()
                && let Ok(answer) = loads(pickle)
            {
                let mut ids = Vec::new();
                object_ids(&answer, &mut ids);
                for id in ids {
                    objects.insert(id, service.clone());
                }
            }
            continue;
        }

        if packet.command != MachoNetMsg::CallReq {
            continue;
        }
        let Some(payload) = packet.body.first() else {
            continue;
        };
        let Ok(call) = CallPayload::from_payload(payload) else {
            continue;
        };

        let (service, method, args, kwargs, bind) = match &call {
            CallPayload::Service {
                method,
                args,
                kwargs,
            } => {
                let Some(service) = packet.destination.service() else {
                    continue;
                };
                (service.to_owned(), method, args, kwargs, None)
            }
            CallPayload::BoundObject {
                object_id,
                method,
                args,
                kwargs,
            } => match objects.get(object_id) {
                Some(service) => (service.clone(), method, args, kwargs, Some(object_id)),
                None => {
                    out.unresolved += 1;
                    continue;
                }
            },
            // Not a call. Nothing is expected back.
            CallPayload::Release { .. } => continue,
        };

        // Three kinds of call are answered a layer *above*
        // `Registry::dispatch`, in `dispatch.rs`, so they have no arm and
        // replaying them here would assert the wrong thing:
        //
        //   - `MachoBindObject` / `MachoResolveObject`, which mint and
        //     resolve objects,
        //   - `charUnboundMgr.SelectCharacterID`, which must emit a session
        //     change *before* its answer and so cannot be a plain return,
        //   - the `OBJECT_RETURNING` calls below, whose return is a reference.
        //
        // Their replies are still harvested for object ids, which is how the
        // bound calls that follow get a service name.
        if method == "MachoBindObject"
            || method == "MachoResolveObject"
            || (service == "charUnboundMgr" && method == "SelectCharacterID")
        {
            if let Some(call_id) = packet.destination.call_id() {
                pending.insert(call_id, service);
            }
            continue;
        }
        // Likewise the object-returning calls: the arm is one layer up.
        let service = match Registry::object_returning(&service, method) {
            Some(bound) => {
                if let Some(call_id) = packet.destination.call_id() {
                    pending.insert(call_id, bound.to_owned());
                }
                continue;
            }
            None => service,
        };

        if let Some(call_id) = packet.destination.call_id() {
            pending.insert(call_id, service.clone());
        }

        let req = Request {
            service: &service,
            method,
            args,
            kwargs,
            bind_params: bind.map(|_| &PyValue::None),
            // The replay harness drives the registry directly, with no
            // session behind it, and the captures it replays come from a
            // one-character world. Pass that character so the arms that need
            // an actor answer as they did when the capture was taken.
            actor: reg.config().characters.first().map(|c| c.character_id),
        };
        match reg.dispatch(&req) {
            // A raise is an answer: a whole class of calls is specified by
            // what it throws. `docs/guides/debugging-the-client.md` §4.
            Ok(_) | Err(ServiceError::Raise(_)) => out.answered += 1,
            Err(e) => out.refused.push(format!("{service}.{method}: {e}")),
        }
        let _ = dumps;
    }
    out
}

/// Every call in every committed capture is still answered.
#[test]
fn the_committed_captures_still_get_answers() {
    let dir = regression_dir();
    let mut total = 0;
    let mut files = 0;
    for entry in std::fs::read_dir(&dir).expect("read the regression directory") {
        let path = entry.expect("a directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("evecap") {
            continue;
        }
        files += 1;
        let r = replay(&path);
        assert!(
            r.refused.is_empty(),
            "{} calls in {} are no longer answered -- a refused call is a hung \
             client, and this capture is the only record that we used to \
             answer them:\n  {}",
            r.refused.len(),
            path.file_name().expect("a file name").to_string_lossy(),
            r.refused.join("\n  ")
        );
        assert!(
            r.answered > 0,
            "{} replayed no calls at all, which means this test is not testing \
             anything: either the capture is empty or the decode is broken",
            path.display()
        );
        println!(
            "{}: {} calls answered, {} bound calls unattributed",
            path.file_name().expect("a file name").to_string_lossy(),
            r.answered,
            r.unresolved
        );
        total += r.answered;
    }
    assert_eq!(
        files, 4,
        "four captures are committed; see the README there"
    );
    // 753 as of the commit that added this file (100 + 132 + 202 + 319). The
    // floor is deliberately close: it is not a coverage target, it is a guard
    // against this test quietly replaying *nothing* because a decode broke or
    // a capture stopped parsing, which would make a green run meaningless.
    // Raise it when the number rises.
    assert!(
        total >= 700,
        "only {total} calls replayed across {files} captures; it was 753 when          this was written, so something stopped decoding rather than something          stopping being called"
    );
}
