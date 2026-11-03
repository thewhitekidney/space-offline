//! `probe` — drive a machoNet server and write down what it streams back.
//!
//! ```text
//! probe run     [options]            log in and run a script
//! probe replay  <file> [options]     decode a recorded byte stream
//! probe decode  <hex>                decode one marshal stream from hex
//! ```
//!
//! `run` needs a network, so it is built in the container and executed on the
//! host. `replay` and `decode` need nothing: they are how a capture taken
//! last week is read today.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use eve_marshal::{MachoNetMsg, PyValue};
use eve_probe::render::{self, Budget};
use eve_probe::session::{Credentials, Session, dict_keys, object_node, unpickle};
use eve_probe::transcript::{Dir, Transcript, read_capture};
use eve_probe::{frames, script};
use eve_proto::{CallPayload, MachoPacket};

const USAGE: &str = "\
probe — a non-graphical EVE client, for recording what a server streams

  probe run [options]
    --host <addr>        default 127.0.0.1:26000
    --user <name>        default probe
    --password <pw>      default probe
    --sso <token>        take the SSO path with this token (default: 'offline')
    --no-sso             take the username/password path instead
    --language <id>      default EN
    --script <file>      a script of calls; omit for handshake-and-listen
    --out <dir>          where the transcript goes (required)
    --stem <name>        transcript basename, default 'probe'
    --settle <ms>        how long to listen after each call, default 400
    --listen <ms>        how long to listen after the script, default 3000

  probe replay <file> [--raw] [--dir in|out] [--limit <n>]
    Decode a recorded stream. Without --raw the file is an .evecap; with it,
    the file is a bare TCP byte stream in one direction (e.g. one side of a
    pcap), and --dir says which.

  probe decode <hex>
    Decode one marshal stream given as hex.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("replay") => replay(&args[1..]),
        Some("decode") => decode(&args[1..]),
        Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("unknown subcommand {other:?}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("probe: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A flat `--key value` parser. No dependencies, and the probe's options are
/// a dozen strings.
struct Opts {
    named: BTreeMap<String, String>,
    flags: Vec<String>,
    positional: Vec<String>,
}

fn parse_opts(args: &[String], flag_names: &[&str]) -> Result<Opts, String> {
    let mut o = Opts {
        named: BTreeMap::new(),
        flags: Vec::new(),
        positional: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(name) = a.strip_prefix("--") {
            if flag_names.contains(&name) {
                o.flags.push(name.to_string());
                i += 1;
                continue;
            }
            let value = args
                .get(i + 1)
                .ok_or_else(|| format!("--{name} wants a value"))?;
            o.named.insert(name.to_string(), value.clone());
            i += 2;
        } else {
            o.positional.push(a.clone());
            i += 1;
        }
    }
    Ok(o)
}

impl Opts {
    fn get(&self, key: &str, default: &str) -> String {
        self.named
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.into())
    }

    fn millis(&self, key: &str, default: u64) -> Result<Duration, String> {
        match self.named.get(key) {
            None => Ok(Duration::from_millis(default)),
            Some(v) => v
                .parse()
                .map(Duration::from_millis)
                .map_err(|_| format!("--{key} wants a number of milliseconds, got {v:?}")),
        }
    }

    fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let o = parse_opts(args, &["no-sso"])?;
    let out = o
        .named
        .get("out")
        .ok_or("--out <dir> is required: transcripts do not go in the repository")?;
    let host = o.get("host", "127.0.0.1:26000");
    let stem = o.get("stem", "probe");
    let settle = o.millis("settle", 400)?;
    let listen = o.millis("listen", 3000)?;

    let steps = match o.named.get("script") {
        None => Vec::new(),
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read script {path}: {e}"))?;
            script::parse(&text).map_err(|e| format!("{path}: {e}"))?
        }
    };

    let creds = Credentials {
        user: o.get("user", "probe"),
        password: o.get("password", "probe"),
        sso_token: if o.has("no-sso") {
            None
        } else {
            Some(o.get("sso", "offline"))
        },
        language: o.get("language", "EN"),
    };

    let tr = Transcript::create(&PathBuf::from(out), &stem)
        .map_err(|e| format!("cannot open transcript in {out}: {e}"))?;
    let paths = tr.paths().clone();
    println!("transcript: {}", paths[0].display());
    println!("frames:     {}", paths[1].display());
    println!("schemas:    {}", paths[2].display());

    let (mut session, version) = Session::connect(&host, tr, Duration::from_millis(100))
        .map_err(|e| format!("connect to {host}: {e}"))?;
    println!(
        "server: build {} codename {}@{} macho {} ({} users online)",
        version.boot_build,
        version.codename,
        version.region,
        version.macho_version,
        version.user_count
    );

    // Stage 2 onward. A failure past here is still a successful run: the
    // transcript is the deliverable, and where it stopped is the finding.
    let login = match session.login(&creds) {
        Ok(l) => l,
        Err(e) => {
            let _ = session.transcript().line(&format!("STOPPED  {e}"));
            let _ = session.finish();
            return Err(format!("handshake: {e}"));
        }
    };
    println!(
        "logged in: userid={} clientID={} proxy_nodeid={}",
        login.user_id, login.client_id, login.proxy_node_id
    );
    println!(
        "session_init: {}",
        dict_keys(&login.session_init).join(", ")
    );

    // Stage 3: whatever the server pushes once it thinks we are logged in,
    // before we ask for anything. These arrive unprompted and are the
    // shapes no request would have revealed.
    let _ = session
        .transcript()
        .heading("stage 3: unsolicited pushes after login");
    if let Err(e) = session.drain(listen) {
        let _ = session.transcript().line(&format!("STOPPED  {e}"));
    }

    if !steps.is_empty() {
        let _ = session.transcript().heading("stage 4: scripted calls");
        if let Err(e) = session.run(&steps, settle) {
            let _ = session.transcript().line(&format!("STOPPED  {e}"));
        }
        if let Err(e) = session.drain(listen) {
            let _ = session.transcript().line(&format!("STOPPED  {e}"));
        }
    }

    session.finish().map_err(|e| format!("finishing: {e}"))?;
    let unanswered = session.unanswered();
    println!(
        "done: {} unanswered call(s), {} delta(s)",
        unanswered.len(),
        session.deltas.len()
    );
    for (service, method) in &unanswered {
        println!("  UNANSWERED {service}.{method}");
    }
    for d in &session.deltas {
        println!("  DELTA {d}");
    }
    Ok(())
}

/// Decode a recorded stream, with no server and no network.
///
/// This is how the probe was developed: the 2026-10-20 pcap of the real
/// client against EvEJS, split into its two directions, replayed through our
/// own codec. Every shape the probe expects on the wire was read off that
/// before a line of the handshake was sent.
fn replay(args: &[String]) -> Result<(), String> {
    let o = parse_opts(args, &["raw"])?;
    let path = o
        .positional
        .first()
        .ok_or("replay wants a file\n\n(see --help)")?;
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let limit: usize = o
        .get("limit", "0")
        .parse()
        .map_err(|_| "--limit wants a number".to_string())?;

    let records: Vec<(Dir, u64, Vec<u8>)> = if o.has("raw") {
        let dir = match o.get("dir", "in").as_str() {
            "in" => Dir::In,
            "out" => Dir::Out,
            other => return Err(format!("--dir wants in or out, got {other:?}")),
        };
        let mut reader = frames::FrameReader::new();
        reader.push(&bytes);
        let mut out = Vec::new();
        loop {
            match reader.next_payload() {
                Ok(Some(p)) => out.push((dir, 0, p.bytes)),
                Ok(None) => break,
                Err(e) => {
                    eprintln!("probe: stopped after {} frame(s): {e}", out.len());
                    break;
                }
            }
        }
        if reader.buffered() != 0 {
            eprintln!(
                "probe: {} trailing byte(s) are not a whole frame",
                reader.buffered()
            );
        }
        out
    } else {
        read_capture(&bytes)?
    };

    println!("{}: {} frame(s)", path, records.len());
    let mut pending: BTreeMap<i64, String> = BTreeMap::new();
    let mut rewrites: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (i, (dir, micros, body)) in records.iter().enumerate() {
        if limit != 0 && i >= limit {
            println!("... {} more frame(s)", records.len() - i);
            break;
        }
        let arrow = match dir {
            Dir::Out => ">>",
            Dir::In => "<<",
        };
        let stamp = if *micros == 0 {
            String::new()
        } else {
            format!("{:9.1}ms ", *micros as f64 / 1000.0)
        };
        let value = match eve_probe::decode_stream(body) {
            Ok(v) => v,
            Err(e) => {
                println!(
                    "[{i:4}] {stamp}{arrow} {:7} bytes  UNDECODABLE: {e}",
                    body.len()
                );
                continue;
            }
        };
        // A foreign server may name the address class after the wrong
        // module; see eve_probe::normalise_foreign.
        let (value, rewritten) = eve_probe::normalise_foreign(&value);
        for class in rewritten {
            if rewrites.insert(class.clone()) {
                eprintln!("probe: rewrote address class {class:?} to the canonical name");
            }
        }
        let label = describe_frame(&value, &mut pending);
        println!("[{i:4}] {stamp}{arrow} {:7} bytes  {label}", body.len());
        println!("        {}", render::value(&value, Budget::default()));
    }
    Ok(())
}

/// Name a frame the way a reader would: a handshake step by its number, a
/// packet by its command and what it is about.
fn describe_frame(value: &PyValue, pending: &mut BTreeMap<i64, String>) -> String {
    if let Ok(packet) = MachoPacket::from_value(value) {
        let cmd = format!("{:?}", packet.command);
        let call_id = packet
            .source
            .call_id()
            .or_else(|| packet.destination.call_id());
        match packet.command {
            MachoNetMsg::CallReq => {
                let service = packet.destination.service().unwrap_or("").to_string();
                let what = match packet.body.first().map(CallPayload::from_payload) {
                    Some(Ok(CallPayload::Service { method, args, .. })) => {
                        format!("{service}.{method}({})", render_args(&args))
                    }
                    Some(Ok(CallPayload::BoundObject {
                        object_id,
                        method,
                        args,
                        ..
                    })) => format!("{object_id}.{method}({})", render_args(&args)),
                    Some(Ok(CallPayload::Release { objects })) => {
                        format!("ClientHasReleasedTheseObjects x{}", objects.len())
                    }
                    _ => service,
                };
                if let Some(id) = call_id {
                    pending.insert(id, what.clone());
                }
                format!("{cmd} #{} {what}", call_id.unwrap_or(-1))
            }
            MachoNetMsg::CallRsp => {
                let who = call_id
                    .and_then(|id| pending.remove(&id))
                    .unwrap_or_else(|| "?".into());
                format!("{cmd} #{} {who}", call_id.unwrap_or(-1))
            }
            _ => {
                let keys = packet
                    .body
                    .iter()
                    .find_map(|v| match v {
                        PyValue::Dict(d) => Some(format!(" [{}]", dict_keys(d).join(", "))),
                        _ => None,
                    })
                    .unwrap_or_default();
                format!("{cmd}{keys}")
            }
        }
    } else {
        handshake_step(value)
    }
}

fn render_args(args: &[PyValue]) -> String {
    args.iter()
        .map(|a| render::value(a, Budget::default()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Which handshake step a bare (non-packet) value is, by its shape. The
/// handshake has no command ids, so the shape is all there is to go on —
/// which is itself worth knowing when reading a capture.
fn handshake_step(value: &PyValue) -> String {
    match value {
        PyValue::Tuple(t) if t.len() == 7 => "H1 version exchange (server)".into(),
        PyValue::Tuple(t) if t.len() == 6 => "H2 version exchange (client)".into(),
        PyValue::Tuple(t) if t.len() == 3 && matches!(&t[1], PyValue::Bytes(b) if b == b"VK") => {
            "H3 VK command".into()
        }
        PyValue::Tuple(t) if t.len() == 2 && matches!(&t[1], PyValue::Dict(_)) => match &t[0] {
            PyValue::Bytes(b) if b == b"placebo" => "H4 crypto request".into(),
            _ => "H6 crypto challenge (login)".into(),
        },
        PyValue::Bytes(b) if b == b"OK CC" => "H5 'OK CC'".into(),
        PyValue::Int(n) => format!("H7 passwordVersion {n}"),
        PyValue::Tuple(t) if t.len() == 4 => "H8 server handshake".into(),
        PyValue::Dict(d) => format!("H10 authenticationResult [{}]", dict_keys(d).join(", ")),
        other => format!("bare value {}", render::shape(other)),
    }
}

fn decode(args: &[String]) -> Result<(), String> {
    let o = parse_opts(args, &[])?;
    let hex = o.positional.first().ok_or("decode wants a hex string")?;
    let bytes = from_hex(hex)?;
    let value = eve_probe::decode_stream(&bytes).map_err(|e| format!("{e}"))?;
    println!("{}", render::value(&value, Budget::sample()));
    println!("shape: {}", render::shape(&value));
    if let Some(cols) = render::columns_of(&value) {
        println!("columns: {}", cols.join(", "));
    }
    // A pickled response is one level deeper; say so rather than making the
    // reader notice.
    let inner = unpickle(&value);
    if inner != value {
        println!("unpickled: {}", render::value(&inner, Budget::sample()));
    }
    if let Some(node) = value_object_node(&value) {
        println!("bound object on node {node}");
    }
    Ok(())
}

fn value_object_node(v: &PyValue) -> Option<i64> {
    let PyValue::Callback(child) = v else {
        return None;
    };
    let (id, _) = render::bound_object(child)?;
    object_node(&id)
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':')
        .collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err("hex input has an odd number of digits".into());
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&cleaned[i..i + 2], 16)
                .map_err(|_| format!("not hex at offset {i}: {:?}", &cleaned[i..i + 2]))
        })
        .collect()
}
