//! Read a `.evecap` back.
//!
//!   evecap FILE [--hex] [--max-bytes N] [--tags]
//!
//! Built as a Rust binary on top of the real decoder rather than as a Python
//! script, deliberately. A second implementation of the marshal format in
//! another language would be a second thing to keep correct, and the whole
//! value of a capture is that it is read by the same code that will have to
//! speak to the client.
//!
//! Payloads that do not decode are the interesting ones: after the handshake
//! every packet is wrapped in an object type code we cannot yet read, so this
//! prints the offending tag and offset and, with `--tags`, a histogram of them
//! across the whole capture. That histogram is the shortest path to knowing
//! which wrapper codes the client actually uses in practice.

#[path = "evecap/destiny.rs"]
mod destiny;
use destiny::{Tracker, View, decoded_lines, view_destiny};

use eve_marshal::{MachoNetMsg, PyValue};
use eve_proto::{CallPayload, MachoPacket};
use eve_server::capture::{Direction, Record, read_all};
// A capture holds the server's responses too, so it is read without the
// inbound limits (`eve_server::wire::loads`).
use eve_server::wire::loads_unbudgeted as loads;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: evecap FILE [options]

  --alerts           print the client's own tracebacks, in full
  --triage           the same tracebacks, one grouped line each, ours vs gateway
  --calls            one line per service call, with the answer we gave
  --hex              hex dump every payload
  --balls ID[,ID]    destiny: only these balls (SetState/AddBalls2 rows and calls on them)
  --timeline         destiny: each named ball's position and velocity per update
  --rel ID           destiny: also give positions relative to ball ID (the station)
  --after MS         only frames at or after this time (milliseconds)
  --before MS        only frames before this time
  --max-bytes N      bytes to show per hex dump (default 64)
  --tags             summarise the type codes that failed to decode
  -h, --help         this text
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("evecap: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut path: Option<PathBuf> = None;
    let mut hex = false;
    let mut tags = false;
    let mut calls = false;
    let mut alerts = false;
    let mut triage = false;
    let mut max_bytes = 64usize;
    let mut balls: Vec<i64> = Vec::new();
    let mut timeline = false;
    let mut rel: Option<i64> = None;
    let mut after = 0.0f64;
    let mut before = f64::MAX;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "--alerts" => alerts = true,
            "--triage" => triage = true,
            "--calls" => calls = true,
            "--hex" => hex = true,
            "--tags" => tags = true,
            "--timeline" => timeline = true,
            "--balls" => {
                let v = args.next().ok_or("--balls needs ID[,ID]")?;
                for part in v.split(',').filter(|p| !p.is_empty()) {
                    balls.push(part.parse().map_err(|e| format!("--balls {part}: {e}"))?);
                }
            }
            "--rel" => {
                let v = args.next().ok_or("--rel needs a ball id")?;
                rel = Some(v.parse().map_err(|e| format!("--rel {v}: {e}"))?);
            }
            "--after" => {
                let v = args.next().ok_or("--after needs ms")?;
                after = v.parse().map_err(|e| format!("--after {v}: {e}"))?;
            }
            "--before" => {
                let v = args.next().ok_or("--before needs ms")?;
                before = v.parse().map_err(|e| format!("--before {v}: {e}"))?;
            }
            "--max-bytes" => {
                let v = args.next().ok_or("--max-bytes needs a value")?;
                max_bytes = v.parse().map_err(|e| format!("--max-bytes {v}: {e}"))?;
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown argument {other}\n\n{USAGE}"));
            }
            other => path = Some(PathBuf::from(other)),
        }
    }
    let path = path.ok_or_else(|| format!("no file given\n\n{USAGE}"))?;
    let records = read_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let view = View {
        balls,
        rel,
        after,
        before,
    };

    println!("{}: {} frames", path.display(), records.len());

    if timeline {
        view_destiny(&records, &view, true);
        return Ok(());
    }
    if triage {
        print_triage(&records);
        return Ok(());
    }
    if alerts {
        print_alerts(&records);
        return Ok(());
    }
    if calls {
        print_calls(&records);
        return Ok(());
    }

    let mut undecodable: BTreeMap<u8, usize> = BTreeMap::new();
    let mut tracker = Tracker::default();

    for (i, r) in records.iter().enumerate() {
        if r.micros as f64 / 1000.0 < view.after || r.micros as f64 / 1000.0 >= view.before {
            continue;
        }
        print_record(i, r, hex, max_bytes, &mut undecodable);
        if let Some(lines) = decoded_lines(r, &mut tracker, &view) {
            for l in lines {
                println!("{l}");
            }
        }
    }

    if tags {
        println!();
        if undecodable.is_empty() {
            println!("every payload decoded; no unsupported type codes");
        } else {
            println!("unsupported type codes, by number of payloads:");
            for (tag, count) in &undecodable {
                let note = match tag {
                    0x17 => "  <- old-style instance (object wrapper)",
                    0x22 => "  <- object, 2.2 form (object wrapper)",
                    0x23 => "  <- object, 2.3 form (object wrapper)",
                    0x2A => "  <- DBRow",
                    0x2B => "  <- SubStream",
                    _ => "",
                };
                println!("  {tag:#04x}  {count}{note}");
            }
        }
    }
    Ok(())
}

/// One line per service call: what the client asked for, and what we said.
///
/// This is the view that actually drives the debugging loop, because of one
/// property of the protocol: **an unanswered `CallReq` is a hang, not a
/// no-op.** `RemoteServiceCallWithoutTheStars` blocks its tasklet until a
/// response arrives, so a call we drop leaves the client on a black screen
/// with nothing in its own log. The frame dump shows that as an absence --
/// a `CallReq` with no `CallRsp` after it -- which is exactly the thing an
/// eye skips over in fifty lines of packets. So name it instead.
///
/// Requests and responses are paired by **callID**, carried in the client
/// address at both ends: the request's `source` is `Client(clientID, callID)`
/// and the response's `destination` is the same address. Pairing on sequence
/// would be wrong, because the client has several calls outstanding at once
/// (one per tasklet) and they do not come back in order.
fn print_calls(records: &[Record]) {
    struct Call {
        frame: usize,
        micros: u64,
        service: String,
        method: String,
        args: String,
        answer: Option<(usize, u64, String)>,
        /// Whether that answer was a raised exception rather than a value.
        /// Not an error on our side: `docs/guides/debugging-the-client.md` 4 lists
        /// a whole class of calls whose *correct* answer is a raise.
        raised: bool,
    }

    let mut calls: Vec<Call> = Vec::new();
    let mut by_call_id: BTreeMap<i64, usize> = BTreeMap::new();
    // objectID -> the service whose bind produced it, learned from the
    // `MachoBindObject` replies in this capture.
    let mut objects: BTreeMap<String, String> = BTreeMap::new();
    let mut pings = 0usize;
    let mut other = 0usize;

    for (i, r) in records.iter().enumerate() {
        let Ok(value) = loads(&r.bytes) else { continue };
        let Ok(packet) = MachoPacket::from_value(&value) else {
            continue;
        };
        match packet.command {
            MachoNetMsg::CallReq => {
                let mut service = packet.destination.service().unwrap_or("?").to_owned();
                let (method, args) = match packet.body.first().map(CallPayload::from_payload) {
                    Some(Ok(CallPayload::Service { method, args, .. })) => (
                        method,
                        args.iter()
                            .map(|a| render(a, 2))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    Some(Ok(CallPayload::BoundObject {
                        object_id,
                        method,
                        args,
                        ..
                    })) => {
                        // Resolve the object back to the service that handed
                        // it out, if that bind is in this same capture. A
                        // bound-object call carries no service name -- only
                        // the id -- so without this the line reads `?` and
                        // tells you nothing about what is being called.
                        let owner = objects
                            .get(&object_id)
                            .cloned()
                            .unwrap_or_else(|| "?".to_owned());
                        service = format!("{owner}*");
                        (
                            format!("{method} [{object_id}]"),
                            args.iter()
                                .map(|a| render(a, 2))
                                .collect::<Vec<_>>()
                                .join(", "),
                        )
                    }
                    // Say *why*, and show what it actually was. A bare
                    // "unparsed" hides the one thing worth knowing, and a
                    // payload we cannot read is the most interesting line in
                    // a capture. The pickle is decoded separately here
                    // because `CallPayload` rejected its *shape*, not its
                    // bytes -- so the bytes will usually still render, and
                    // what they render as is the answer.
                    Some(Err(e)) => {
                        let shown = packet
                            .body
                            .first()
                            .and_then(|p| match p {
                                PyValue::Tuple(t) if t.len() == 2 => Some(&t[1]),
                                _ => None,
                            })
                            .map(|p| match p {
                                PyValue::SubStream(b) | PyValue::Bytes(b) => match loads(b) {
                                    Ok(v) => render(&v, 0),
                                    Err(e) => {
                                        format!("<pickle: {e}>\n{}", hexdump(b, 48.min(b.len())))
                                    }
                                },
                                other => render(other, 0),
                            })
                            .unwrap_or_else(|| "<no pickle>".to_owned());
                        (format!("<unparsed: {e}>"), shown)
                    }
                    // Only ever arrives as a Notification, so it is not a
                    // call and has no line in this view.
                    Some(Ok(CallPayload::Release { .. })) => continue,
                    None => ("<no payload>".to_owned(), String::new()),
                };
                if let Some(id) = packet.source.call_id() {
                    by_call_id.insert(id, calls.len());
                }
                calls.push(Call {
                    frame: i,
                    micros: r.micros,
                    service,
                    method,
                    args,
                    answer: None,
                    raised: false,
                });
            }
            MachoNetMsg::CallRsp => {
                // The response payload is a *pickle*, not the value: the
                // client hands it straight to `blue.marshal.Load`. So it has
                // to be loaded a second time to be read.
                let rendered = match packet.body.first() {
                    Some(PyValue::Bytes(b)) => match loads(b) {
                        Ok(v) => render(&v, 1),
                        Err(e) => format!("<undecodable answer: {e}>"),
                    },
                    Some(v) => render(v, 1),
                    None => "<empty>".to_owned(),
                };
                let at = packet
                    .destination
                    .call_id()
                    .and_then(|id| by_call_id.get(&id).copied());
                match at {
                    Some(at) => {
                        // Any answer at all, not only a `MachoBindObject`
                        // one. `skillMgr2.GetMySkillHandler` is an ordinary
                        // `CallReq` whose *return* is a reference
                        // (`Registry::OBJECT_RETURNING`), so restricting this
                        // to binds left every call on the skill handler --
                        // `GetBoosters`, `GetSkillQueueAndFreePoints`,
                        // `GetSkillHistory` -- printing as `?*`.
                        for id in object_ids_of(&rendered) {
                            objects.insert(id, calls[at].service.clone());
                        }
                        calls[at].answer = Some((i, r.micros, rendered));
                    }
                    None => other += 1,
                }
            }
            // **A raise is an answer.** This arm was missing, and its
            // absence made the tool lie: an `ErrorResponse` fell through to
            // `other`, the call it answered kept `answer: None`, and the
            // line printed as `HANG ... <- NO ANSWER; the client is still
            // waiting` followed by a summary claiming that is where the
            // client stopped.
            //
            // For `slash.SlashCmd("/")` that was exactly backwards. Raising
            // is the *only* correct answer to it -- `FetchRemoteCommandList`
            // binds `cmds` only inside its `except UserError` handler, so a
            // normal return is an `UnboundLocalError` that breaks every
            // slash command. The capture of the session where the user first
            // tried the console showed `/` as a hang and `/createitem`
            // two seconds later as fine, which reads as a server that died
            // and recovered, and is instead a server doing the right thing.
            MachoNetMsg::ErrorResponse => {
                // `['originalCommand', 'code', 'payload']`, and the payload
                // is the 1-tuple from `wrapped_exception_payload`: one
                // element, itself a nested marshal stream. Unwrap both
                // layers, because the useful text is inside.
                let rendered = match packet.body.get(2) {
                    Some(PyValue::Tuple(t)) => match t.first() {
                        Some(PyValue::SubStream(b) | PyValue::Bytes(b)) => match loads(b) {
                            Ok(v) => render(&v, 1),
                            Err(e) => format!("<undecodable exception: {e}>"),
                        },
                        Some(v) => render(v, 1),
                        None => "<empty payload>".to_owned(),
                    },
                    Some(v) => render(v, 1),
                    None => "<no payload>".to_owned(),
                };
                let at = packet
                    .destination
                    .call_id()
                    .and_then(|id| by_call_id.get(&id).copied());
                match at {
                    Some(at) => {
                        calls[at].answer = Some((i, r.micros, rendered));
                        calls[at].raised = true;
                    }
                    None => other += 1,
                }
            }
            MachoNetMsg::PingReq => pings += 1,
            _ => other += 1,
        }
    }

    let answered = calls.iter().filter(|c| c.answer.is_some()).count();
    let raised = calls.iter().filter(|c| c.raised).count();
    for c in &calls {
        let ms = c.micros as f64 / 1000.0;
        match &c.answer {
            Some((_, micros, rendered)) => {
                let took = (micros.saturating_sub(c.micros)) as f64 / 1000.0;
                let verb = if c.raised { "RAISE" } else { "OK   " };
                let arrow = if c.raised { "!!" } else { "->" };
                println!(
                    "[{:3}] {ms:9.3}ms {verb} {:<16} {}({})  {arrow} {rendered}  ({took:.1}ms)",
                    c.frame, c.service, c.method, c.args
                );
            }
            None => println!(
                "[{:3}] {ms:9.3}ms HANG {:<16} {}({})  <- NO ANSWER; the client is still waiting",
                c.frame, c.service, c.method, c.args
            ),
        }
    }

    println!();
    println!(
        "{} calls: {answered} answered ({raised} by raising), {} unanswered; \
         {pings} pings, {other} other frames",
        calls.len(),
        calls.len() - answered
    );
    if answered != calls.len() {
        println!("the unanswered calls above are where the client stopped.");
    }
}

/// Print every traceback the client mailed us.
///
/// The client reports its own unhandled exceptions to the server, by calling
/// `alert.SendClientStackTraceAlert`. We have always answered it -- it is in
/// the registry, it returns `None`, it never appeared on the unanswered list
/// -- and that is precisely why nobody looked: a call that is *answered*
/// leaves no trace in any of our instruments. One capture of a session that
/// reached the hangar carried five of them, each a complete traceback naming
/// the frame that broke, sitting unread in a file we had already read twice.
///
/// So this is not a nicety. It is the client's own diagnosis of our server,
/// in its own words, and it is the first thing to read after a run.
///
/// The arguments, from the capture (`alert.SendClientStackTraceAlert`):
///   `((bean, <formatted traceback>), <exception text>, severity, extra)`
/// Both blobs are Python 2 `str`, so they print as text once the frame is
/// decoded -- they do not appear in the raw file because machoNet zlib-
/// compresses anything over its threshold, which is why `strings` on a
/// capture finds nothing.
fn print_alerts(records: &[Record]) {
    let mut found = 0usize;
    for (i, r) in records.iter().enumerate() {
        let Some(blobs) = alert_blobs(r) else {
            continue;
        };
        found += 1;
        println!();
        println!(
            "=== [{i:3}] {:.3}ms  client traceback {found} ===",
            r.micros as f64 / 1000.0
        );
        for b in &blobs {
            println!("{b}");
        }
    }
    println!();
    if found == 0 {
        println!("no client tracebacks in this capture.");
    } else {
        println!("{found} client tracebacks.");
    }
}

/// The text blobs of one `SendClientStackTraceAlert`, or `None` if this frame
/// is not one.
///
/// The severity string and the bean are in the same argument list as the two
/// blobs worth reading; returning them would be noise, and deciding which is
/// which by position would break the moment the client changed the call. So
/// the filter is on length instead: anything short is metadata.
fn alert_blobs(r: &Record) -> Option<Vec<String>> {
    let value = loads(&r.bytes).ok()?;
    let packet = MachoPacket::from_value(&value).ok()?;
    if packet.command != MachoNetMsg::CallReq {
        return None;
    }
    let Some(Ok(CallPayload::Service { method, args, .. })) =
        packet.body.first().map(CallPayload::from_payload)
    else {
        return None;
    };
    if method != "SendClientStackTraceAlert" {
        return None;
    }
    let mut out = Vec::new();
    for a in &args {
        match a {
            PyValue::Tuple(t) => out.extend(t.iter().filter_map(text_of)),
            other => out.extend(text_of(other)),
        }
    }
    Some(out)
}

/// A value as text, if it is text and long enough to be one of the two blobs.
fn text_of(v: &PyValue) -> Option<String> {
    let b = match v {
        PyValue::Bytes(b) if b.len() > 40 => b.clone(),
        PyValue::Unicode(s) if s.len() > 40 => s.as_bytes().to_vec(),
        _ => return None,
    };
    Some(String::from_utf8_lossy(&b).trim_end().to_owned())
}

/// Which protocol a traceback came in over -- the first question to ask of
/// any of them, and the one that decides whether it is even our problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Origin {
    /// machoNet. Ours: some call of ours was missing, or answered wrongly.
    Ours,
    /// The protobuf/gRPC public gateway, which we deliberately do not serve.
    /// Deferred, not deleted -- see `docs/archive/roadmap.md`.
    Gateway,
}

/// Exception types only the gateway raises.
///
/// `TimeoutException` is on this list because **we always answer**: a
/// machoNet call either gets a reply or shows up as `[svc] UNANSWERED` in our
/// own log, and either way the client is not left waiting on a channel. A
/// `TimeoutException` therefore means a transport we are not speaking.
const GATEWAY_ERRORS: &[&str] = &[
    "TimeoutException",
    "ServiceUnavailableError",
    "GoalMessengerError",
    "CampaignRequestError",
    "StoreUnavailableError",
];

/// Path fragments that only appear in gateway client code.
///
/// `docs/guides/debugging-the-client.md` §3c records the canonical tell -- a
/// `*messenger*` module bottoming out in `stacklesslib/util.py(247)
/// receive`. `message_bus` and `gateway` catch the two subsystems that reach
/// the same transport without a module called "messenger" in the path.
const GATEWAY_FRAMES: &[&str] = &[
    "messenger",
    "message_bus",
    "gateway",
    "stacklesslib/util.py(247)",
];

/// Packages with **no machoNet client at all** in V24.01.
///
/// A frame from one of these cannot be a fault of ours whatever the exception
/// says, because the package never calls us. This catches the case
/// [`GATEWAY_FRAMES`] structurally *cannot*: a gateway error that was caught
/// and **silenced**, leaving a second-order exception whose stack holds no
/// transport frame at all.
///
/// The worked example is `achievement_system`, from capture
/// `1792431326-0002` traceback 5.
/// `achievement_system/client/repository/repository.py`'s
/// `_make_unclaimed_rewards_request` ends with
/// `except ServiceUnavailableError: logger.info('... silenced')` -- info
/// level, so the traceback carrying the `*messenger*` and
/// `stacklesslib/util.py(247)` frames is never logged and never mailed to us.
/// The cache is guarded with `is None`, so it stays unfilled, and
/// `get_all_unclaimed_rewards` falls off the end returning `None`. What
/// reaches us is the `UNPACK_SEQUENCE 2` at `repository.py(257)`, in a
/// different tasklet, over a stack of bluepy, uthread2, a neocom button and a
/// lock wrapper -- not one transport frame among them.
///
/// Membership is a grep over the extracted bytecode, and the count must be
/// zero:
///
/// ```text
/// grep -ra 'RemoteSvc\|ConnectToRemoteService\|RemoteServiceCall' out/pyc/<pkg>/ | wc -l
/// ```
///
/// Kept separate from [`GATEWAY_FRAMES`] rather than appended to it, because
/// the justification is different -- not "this path is gateway code" but
/// "this package cannot call us" -- and keeping the two apart is what lets
/// the next person audit membership instead of trusting it. `cosmetics` looks
/// like a candidate and is **not** one: `cosmetics/client/cosmeticsSvc.pyc`
/// has a real machoNet client.
const GATEWAY_PACKAGES: &[&str] = &["achievement_system/"];

/// Classify one traceback by protocol.
///
/// Deliberately generous towards `Gateway`: a gateway failure we mislabel as
/// ours costs someone an afternoon chasing a service method that was never
/// called, whereas one of ours mislabelled as gateway still sits in the list
/// under a heading that says we do not serve it, and the exception text is
/// right there. Erring the other way is the expensive mistake.
fn classify(exception: &str, frames: &[String]) -> Origin {
    if GATEWAY_ERRORS.iter().any(|e| exception.starts_with(e)) {
        return Origin::Gateway;
    }
    let lower: Vec<String> = frames.iter().map(|f| f.to_lowercase()).collect();
    if lower
        .iter()
        .any(|f| GATEWAY_FRAMES.iter().any(|g| f.contains(g)))
    {
        return Origin::Gateway;
    }
    if lower
        .iter()
        .any(|f| GATEWAY_PACKAGES.iter().any(|p| f.contains(p)))
    {
        return Origin::Gateway;
    }
    Origin::Ours
}

/// One traceback, reduced to what a triage needs.
struct Triaged {
    origin: Origin,
    exception: String,
    /// Deepest frames first -- the deepest one is the thing that broke.
    frames: Vec<String>,
    /// Seconds into the session, for the first occurrence.
    at: f64,
}

/// Parse the formatted blob.
///
/// The blob contains the frame list twice: once raw, once again with locals
/// under a `<pre>` marker (the client renders its own alert mail as HTML and
/// does not escape it). The second copy is the one worth reading, so this
/// takes only the `<pre>` lines, and the lines between them -- the locals --
/// are skipped because they do not fit on one line.
fn triage_one(blobs: &[String], at: f64) -> Option<Triaged> {
    let text = blobs.join("\n");
    let exception = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("Formatted exception info:"))
        .map(|e| e.trim().to_owned())
        .filter(|e| !e.is_empty())?;
    let mut frames: Vec<String> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("<pre>"))
        .map(|f| f.trim().to_owned())
        .collect();
    frames.reverse();
    Some(Triaged {
        origin: classify(&exception, &frames),
        exception,
        frames,
        at,
    })
}

/// `--triage`: every traceback in the capture as one grouped entry, sorted so
/// the ones we can actually fix come first.
///
/// This exists because the triage was being done by hand, with a shell
/// pipeline reinvented at each session, every time the user relaunched. The
/// grouping matters as much as the classification: the same failure fires
/// repeatedly within one session -- a panel that reloads, a badge that
/// refreshes -- and a list of twenty-one tracebacks that is really eight
/// distinct bugs invites fixing the loudest rather than the worst.
fn print_triage(records: &[Record]) {
    let mut seen: Vec<Triaged> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    let mut total = 0usize;

    for r in records {
        let Some(blobs) = alert_blobs(r) else {
            continue;
        };
        total += 1;
        let Some(t) = triage_one(&blobs, r.micros as f64 / 1_000_000.0) else {
            continue;
        };
        // Group on the exception plus the deepest frame. Not on the whole
        // stack: the same break is reached by several callers (a badge
        // refresh and a panel load hitting one repository method), and those
        // are one bug, not several.
        let key = seen
            .iter()
            .position(|s| s.exception == t.exception && s.frames.first() == t.frames.first());
        match key {
            Some(i) => counts[i] += 1,
            None => {
                seen.push(t);
                counts.push(1);
            }
        }
    }

    if total == 0 {
        println!("\nno client tracebacks in this capture.");
        return;
    }

    let mut order: Vec<usize> = (0..seen.len()).collect();
    order.sort_by_key(|&i| (seen[i].origin, seen[i].at as i64));

    let ours = order
        .iter()
        .filter(|&&i| seen[i].origin == Origin::Ours)
        .count();
    println!(
        "\n{total} tracebacks, {} distinct: {ours} ours, {} gateway",
        seen.len(),
        seen.len() - ours
    );

    let mut heading = None;
    for &i in &order {
        let t = &seen[i];
        if heading != Some(t.origin) {
            heading = Some(t.origin);
            println!();
            match t.origin {
                Origin::Ours => println!("OURS -- machoNet. A call of ours is missing or wrong."),
                Origin::Gateway => println!(
                    "GATEWAY -- the protobuf gateway we do not serve. See docs/guides/debugging-the-client.md 3c."
                ),
            }
        }
        println!("\n  {:>2}x  {:6.1}s  {}", counts[i], t.at, t.exception);
        for f in t.frames.iter().take(2) {
            println!("          {f}");
        }
    }
    println!();
}

/// The `objectID` in a rendered `MachoBindObject` answer.
///
/// Crude on purpose: it reads the id back out of the rendering rather than
/// re-decoding the reply, because this is a diagnostic and the rendering is
/// right there. The form is `<callback ("N=1:1", <refID>)>`.
/// Every object id in a rendered answer, not just the first.
///
/// **One bind can hand out two objects**, and the second one used to go
/// unrecorded. `invbroker.MachoBindObject` with a fused
/// `GetInventoryFromId` answers `(<callback N=1:20>, <callback N=1:19>)`: the
/// outer reference to the thing we bound, and the inner reference the fused
/// call *returned*. Taking only the first left later calls on the second
/// printing as `?* List [N=1:19]`, which is the one row in `--calls` that
/// tells you nothing -- a bound call carries no service name, only the id, so
/// following it meant grepping back through the capture by hand for a bind
/// whose id matched.
fn object_ids_of(rendered: &str) -> Vec<String> {
    const OPEN: &str = "<callback (\"";
    let mut out = Vec::new();
    let mut rest = rendered;
    while let Some(start) = rest.find(OPEN) {
        rest = &rest[start + OPEN.len()..];
        match rest.find('"') {
            Some(end) => {
                out.push(rest[..end].to_owned());
                rest = &rest[end..];
            }
            None => break,
        }
    }
    out
}

fn print_record(
    i: usize,
    r: &Record,
    hex: bool,
    max_bytes: usize,
    undecodable: &mut BTreeMap<u8, usize>,
) {
    let arrow = match r.direction {
        Direction::ClientToServer => "C->S",
        Direction::ServerToClient => "S->C",
    };
    let ms = r.micros as f64 / 1000.0;
    print!("[{i:3}] {ms:9.3}ms {arrow} {:6} bytes  ", r.bytes.len());
    match loads(&r.bytes) {
        Ok(v) => println!("{}", render(&v, 0)),
        Err(e) => {
            if let eve_marshal::WireError::UnsupportedTag { tag, .. } = e {
                *undecodable.entry(tag).or_insert(0) += 1;
            }
            println!("UNDECODED: {e}");
        }
    }
    if hex {
        println!("{}", hexdump(&r.bytes, max_bytes));
    }
}

/// A compact one-line rendering. Containers are truncated: a capture is read
/// to find a shape, and a thousand-element rowset printed in full buries it.
fn render(v: &PyValue, depth: usize) -> String {
    const MAX_DEPTH: usize = 6;
    const MAX_ITEMS: usize = 12;
    if depth > MAX_DEPTH {
        return "...".to_owned();
    }
    match v {
        PyValue::None => "None".to_owned(),
        PyValue::Bool(b) => b.to_string(),
        PyValue::Int(n) | PyValue::Long(n) => n.to_string(),
        PyValue::Real(x) => format!("{x:?}"),
        PyValue::Bytes(b) => render_bytes(b),
        PyValue::Unicode(s) => format!("u{:?}", truncate(s)),
        PyValue::Tuple(items) => format!("({})", join(items, depth, MAX_ITEMS)),
        PyValue::List(items) => format!("[{}]", join(items, depth, MAX_ITEMS)),
        PyValue::Dict(entries) => {
            // Stored value-first on the wire; printed key-first, as Python
            // would show it.
            let shown: Vec<String> = entries
                .iter()
                .take(MAX_ITEMS)
                .map(|(value, key)| {
                    format!("{}: {}", render(key, depth + 1), render(value, depth + 1))
                })
                .collect();
            let more = if entries.len() > MAX_ITEMS {
                format!(", +{}", entries.len() - MAX_ITEMS)
            } else {
                String::new()
            };
            format!("{{{}{more}}}", shown.join(", "))
        }
        PyValue::Instance { class, state } => format!(
            "<{} {}>",
            String::from_utf8_lossy(class),
            render(state, depth + 1)
        ),
        PyValue::ObjectEx {
            form,
            header,
            list,
            dict,
        } => {
            let mut s = format!("<{form:?} {}", render(header, depth + 1));
            if !list.is_empty() {
                s.push_str(&format!(" +[{}]", join(list, depth, MAX_ITEMS)));
            }
            if !dict.is_empty() {
                let shown: Vec<String> = dict
                    .iter()
                    .take(MAX_ITEMS)
                    .map(|(k, v)| format!("{}: {}", render(k, depth + 1), render(v, depth + 1)))
                    .collect();
                s.push_str(&format!(" +{{{}}}", shown.join(", ")));
            }
            s.push('>');
            s
        }
        PyValue::Global(name) => format!("<global {}>", String::from_utf8_lossy(name)),
        PyValue::DbRow(row) => format!(
            "<DBRow {}>",
            row.descriptor()
                .names()
                .iter()
                .zip(row.values())
                .map(|(n, v)| format!("{}={:?}", String::from_utf8_lossy(n), v))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        PyValue::SubStream(bytes) => format!("<substream {} bytes>", bytes.len()),
        // Normally a bound-object reference: a byte string holding a second
        // marshal stream of `(objectID, refID)`. Decode it, since that nested
        // tuple is the interesting part and a byte count says nothing.
        PyValue::Callback(child) => match child.as_ref() {
            PyValue::Bytes(b) => match loads(b) {
                Ok(inner) => format!("<callback {}>", render(&inner, depth + 1)),
                Err(_) => format!("<callback {}>", render_bytes(b)),
            },
            other => format!("<callback {}>", render(other, depth + 1)),
        },
    }
}

fn join(items: &[PyValue], depth: usize, max: usize) -> String {
    let shown: Vec<String> = items
        .iter()
        .take(max)
        .map(|v| render(v, depth + 1))
        .collect();
    if items.len() > max {
        format!("{}, +{}", shown.join(", "), items.len() - max)
    } else {
        shown.join(", ")
    }
}

/// Python 2 `str` is bytes. Printing it as text when it is text makes a
/// capture readable; falling back to hex when it is not keeps it honest.
fn render_bytes(b: &[u8]) -> String {
    if b.first() == Some(&0x7e)
        && let Ok(inner) = loads(b)
    {
        return format!("<<{}>>", render(&inner, 0));
    }
    match std::str::from_utf8(b) {
        Ok(s) if s.chars().all(|c| !c.is_control()) => format!("{:?}", truncate(s)),
        _ => format!("<{} bytes {}>", b.len(), short_hex(b, 16)),
    }
}

fn truncate(s: &str) -> String {
    const MAX: usize = 48;
    if s.chars().count() <= MAX {
        return s.to_owned();
    }
    let head: String = s.chars().take(MAX).collect();
    format!("{head}...")
}

fn short_hex(b: &[u8], n: usize) -> String {
    let body: String = b.iter().take(n).map(|x| format!("{x:02x}")).collect();
    if b.len() > n {
        format!("{body}...")
    } else {
        body
    }
}

fn hexdump(b: &[u8], max: usize) -> String {
    let mut out = String::new();
    for (row, chunk) in b
        .iter()
        .take(max)
        .collect::<Vec<_>>()
        .chunks(16)
        .enumerate()
    {
        let hex: Vec<String> = chunk.iter().map(|x| format!("{x:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&&x| {
                if (0x20..0x7F).contains(&x) {
                    x as char
                } else {
                    '.'
                }
            })
            .collect();
        out.push_str(&format!(
            "      {:04x}  {:<47}  {ascii}\n",
            row * 16,
            hex.join(" ")
        ));
    }
    if b.len() > max {
        out.push_str(&format!("      ... {} more bytes\n", b.len() - max));
    }
    out.pop();
    out
}

#[cfg(test)]
mod tests {

    /// One bind, two objects. The second one is the one that used to be lost.
    #[test]
    fn both_objects_of_a_fused_bind_are_recorded() {
        let rendered = "(<callback (\"N=1:20\", 134369134582638028)>, \
                        <callback (\"N=1:19\", 134369134582637977)>)";
        assert_eq!(object_ids_of(rendered), vec!["N=1:20", "N=1:19"]);
    }

    #[test]
    fn an_answer_with_no_object_yields_none() {
        assert!(object_ids_of("(None, None)").is_empty());
        assert!(object_ids_of("").is_empty());
    }
    use super::*;

    /// A real blob, trimmed to the shape the parser cares about. Taken from
    /// `captures/1792431326-0002.evecap`, traceback 7 -- the one that cost a
    /// whole run: the client undocked, reached the ballpark, and died there,
    /// and the player saw a black screen.
    const BALLPARK: &str = "\
Caught at:
 <pre>/packages/bluepy/__init__.py(55) CallWrapper
Thrown at:
 <pre>/carbon/common/script/sys/serviceManager.py(1014) MollycoddledUthread
 <pre>/eve/client/script/remote/michelle.py(213) OnPrimaryViewChanged
 <pre>/eve/client/script/remote/michelle.py(453) AddBallpark
        formations = []
        solarsystemID = 30000001
Formatted exception info: TypeError: must be tuple, not list
Stackhash: -4787970
";

    /// Same capture, traceback 8. The canonical gateway tell.
    const COSMETICS: &str = "\
Thrown at:
 <pre>/packages/cosmetics/client/messengers/cosmetics/structures/requestMessenger.py(95) blocking_request
 <pre>/carbon/common/stdlib/stacklesslib/util.py(247) receive
Formatted exception info: TimeoutException
";

    #[test]
    fn the_deepest_frame_is_the_one_that_broke() {
        let t = triage_one(&[BALLPARK.to_owned()], 43.4).expect("parses");
        assert_eq!(t.exception, "TypeError: must be tuple, not list");
        assert!(
            t.frames[0].contains("michelle.py(453) AddBallpark"),
            "deepest frame first, got {:?}",
            t.frames[0]
        );
    }

    /// The locals are printed between the frames in the real blob. They are
    /// the most useful thing in a full `--alerts` dump and the least useful
    /// thing in a one-line triage, so they must not be mistaken for frames.
    #[test]
    fn locals_are_not_frames() {
        let t = triage_one(&[BALLPARK.to_owned()], 0.0).expect("parses");
        assert!(
            !t.frames.iter().any(|f| f.contains("formations")),
            "locals leaked into the frame list: {:?}",
            t.frames
        );
    }

    #[test]
    fn a_native_type_error_is_ours_to_fix() {
        let t = triage_one(&[BALLPARK.to_owned()], 0.0).expect("parses");
        assert_eq!(t.origin, Origin::Ours);
    }

    #[test]
    fn a_messenger_timing_out_is_the_gateway() {
        let t = triage_one(&[COSMETICS.to_owned()], 0.0).expect("parses");
        assert_eq!(t.origin, Origin::Gateway);
    }

    /// We always answer, so nothing on machoNet can time out: a
    /// `TimeoutException` is a transport we are not speaking, whatever the
    /// stack looks like. This is the single rule that keeps the OURS list
    /// short enough to act on.
    #[test]
    fn a_timeout_is_never_ours_even_with_no_telltale_frame() {
        let t = triage_one(
            &["Thrown at:\n <pre>/some/unknown/module.py(1) f\nFormatted exception info: TimeoutException\n".to_owned()],
            0.0,
        )
        .expect("parses");
        assert_eq!(t.origin, Origin::Gateway);
    }

    /// A blob with no exception line is not a traceback we can say anything
    /// about, and must not become an entry with an empty heading.
    /// Same capture, traceback 5. The gateway error was **silenced** at info
    /// level by `_make_unclaimed_rewards_request`, so the stack that reaches
    /// us holds no transport frame at all -- bluepy, uthread2, a neocom
    /// button, a lock wrapper, the repository. `GATEWAY_FRAMES` cannot catch
    /// this and never will; the package name is the only signal left.
    const ACHIEVEMENT_BADGE: &str = "\
Thrown at:
 <pre>/packages/bluepy/__init__.py(55) CallWrapper
 <pre>/packages/uthread2/bluepyimpl.py(34) inner
 <pre>/eve/client/script/ui/shared/neocom/neocom/btnData/btnDataAchievements.py(46) update_badge
 <pre>/packages/locks/singleFlight.py(23) inner
 <pre>/packages/achievement_system/client/repository/repository.py(257) get_unclaimed_rewards_count
        reward_count = 0
Formatted exception info: TypeError: 'NoneType' object is not iterable
";

    #[test]
    fn a_silenced_gateway_error_is_still_the_gateway() {
        let t = triage_one(&[ACHIEVEMENT_BADGE.to_owned()], 33.4).expect("parses");
        assert_eq!(t.origin, Origin::Gateway);
    }

    #[test]
    fn a_blob_without_an_exception_is_skipped() {
        assert!(triage_one(&["Caught at:\n <pre>/x.py(1) f\n".to_owned()], 0.0).is_none());
    }
}
