//! IP19: arm fuzz (`docs/review/V2-wire.md` P7, proposal 9).
//!
//! Every `(service, method)` the dispatch table can route is called with
//! wrong-typed arguments under `catch_unwind`. A panic in an arm runs under
//! the world write lock: it poisons the world (recovered and tainted since
//! IP9, so never saved, but the flag is permanent) and leaves a half-applied
//! mutation behind. This answers `docs/reference/threads-and-locks.md` §11's open
//! question, "whether a dispatch arm can be made to panic at all": it can, and
//! [`KNOWN_PANICS`] lists every one this generator reaches.
//!
//! **The pairs.** `DOMAINS` is private to `service.rs`, and its methods are
//! match arms, not data, so neither list is reachable from here. The services
//! come from [`Registry::services`], which is derived from `DOMAINS`. The
//! methods are every string literal in `src/` used as a match pattern (`"X" =>`
//! or `"X" |`), which is a superset of the arms' method names; every service is
//! called with every one of them. A name that is not a method of that service
//! costs one `NoSuchMethod`.
//!
//! **The arguments.** A fixed set of shapes (no arguments, one `None`, one of
//! each wrong scalar, empty containers, extreme ints, NaN, and a real id of
//! the default world beside an `i64` edge), then rounds drawn
//! from a SplitMix64 seeded by an FNV-1a hash of `service`, `method` and the
//! round. No dependency, no clock, no thread: the same tree makes the same
//! calls in the same order. Every call is a client session with
//! `roles::SANDBOX_DOCKED`, the role a real client has today, with the acting
//! character, no character, or an unknown one.
//!
//! **The rule.** A panic whose `(service, method)` is not in [`KNOWN_PANICS`]
//! fails the test, and so does a listed pair that no longer panics: the list
//! may only shrink, and a fix removes its row. After a panic the world is
//! rebuilt, so one bug does not hide or cause another.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use eve_marshal::PyValue;
use eve_server::model::SandboxConfig;
use eve_server::roles;
use eve_server::service::{Caller, Registry, Request};

/// Every `(service, method)` this fuzz is known to panic, with the panic site
/// at the time it was found and the panic message. **May only shrink**: the
/// test fails when a listed pair stops panicking, so a fix deletes its row in
/// the same commit. The site is documentation, not part of the match (other
/// lanes move lines); a pair is matched on service and method only.
const KNOWN_PANICS: &[(&str, &str, &str, &str)] = &[];

/// Rounds of generated arguments per pair, after the fixed shapes.
const ROUNDS: u64 = 8;

/// The innermost panic site and message of the call in progress, written by
/// the hook installed in the test.
static LAST_PANIC: Mutex<Option<(String, String)>> = Mutex::new(None);

/// SplitMix64: one `u64` of state, no dependency.
struct Rng(u64);

impl Rng {
    fn seeded(parts: &[&[u8]]) -> Rng {
        // FNV-1a over the parts, with a separator so ("ab","c") != ("a","bc").
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for p in parts {
            for b in p.iter().chain(std::iter::once(&0xff)) {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        Rng(h)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const EXTREME_INTS: &[i64] = &[
    0,
    -1,
    1,
    i64::MIN,
    i64::MAX,
    i32::MIN as i64,
    i32::MAX as i64,
    u32::MAX as i64,
];

fn value(rng: &mut Rng, ids: &[i64], depth: u32) -> PyValue {
    let leaf_kinds = 7;
    let kinds = if depth == 0 {
        leaf_kinds
    } else {
        leaf_kinds + 3
    };
    match rng.below(kinds) {
        0 => PyValue::None,
        1 => PyValue::Bool(rng.below(2) == 1),
        2 => {
            if rng.below(2) == 0 {
                PyValue::Int(EXTREME_INTS[rng.below(EXTREME_INTS.len() as u64) as usize])
            } else {
                PyValue::Int(rng.next() as i64)
            }
        }
        3 => PyValue::Real(match rng.below(5) {
            0 => f64::NAN,
            1 => f64::INFINITY,
            2 => f64::NEG_INFINITY,
            3 => -0.0,
            _ => (rng.next() as i64) as f64,
        }),
        4 => {
            let n = rng.below(6) as usize;
            PyValue::Bytes((0..n).map(|_| rng.next() as u8).collect())
        }
        5 => PyValue::Unicode(match rng.below(4) {
            0 => String::new(),
            1 => "/".to_owned(),
            2 => "\u{0}\u{ffff}".to_owned(),
            _ => format!("x{}", rng.next()),
        }),
        // A real id in a wrong place: the arm's lookup succeeds and the
        // next step trusts it.
        6 => PyValue::Int(ids[rng.below(ids.len() as u64) as usize]),
        7 => PyValue::Tuple(children(rng, ids, depth)),
        8 => PyValue::List(children(rng, ids, depth)),
        _ => {
            let n = rng.below(4) as usize;
            PyValue::Dict(
                (0..n)
                    .map(|_| (value(rng, ids, depth - 1), value(rng, ids, depth - 1)))
                    .collect(),
            )
        }
    }
}

fn children(rng: &mut Rng, ids: &[i64], depth: u32) -> Vec<PyValue> {
    let n = rng.below(4) as usize;
    (0..n).map(|_| value(rng, ids, depth - 1)).collect()
}

/// One call's inputs: arguments, kwargs, bind params, actor.
struct Shape {
    args: Vec<PyValue>,
    kwargs: Vec<(PyValue, PyValue)>,
    bind: Option<PyValue>,
    actor: Option<i64>,
}

/// The fixed shapes every pair gets, then `ROUNDS` generated ones.
fn shapes(service: &str, method: &str, me: i64, ids: &[i64]) -> Vec<Shape> {
    let mut fixed: Vec<Vec<PyValue>> = vec![
        vec![],
        vec![PyValue::None],
        vec![PyValue::Int(-1)],
        vec![PyValue::Int(i64::MIN)],
        vec![PyValue::Real(f64::NAN)],
        vec![PyValue::Unicode(String::new())],
        vec![PyValue::Bytes(vec![])],
        vec![PyValue::List(vec![])],
        vec![PyValue::Tuple(vec![])],
        vec![PyValue::Dict(vec![])],
        vec![PyValue::None; 8],
        vec![PyValue::Int(0); 8],
    ];
    // Right-typed but hostile: a real id followed by a quantity or index
    // at the edge of `i64`, which is where an unchecked `+`, `*`, `as` or
    // slice index lives.
    for &id in ids {
        for &edge in &[i64::MAX, i64::MIN, -1] {
            fixed.push(vec![PyValue::Int(id), PyValue::Int(edge)]);
            fixed.push(vec![
                PyValue::List(vec![PyValue::Int(id)]),
                PyValue::Int(edge),
            ]);
            fixed.push(vec![
                PyValue::Int(id),
                PyValue::Int(id),
                PyValue::Int(edge),
                PyValue::Real(edge as f64),
            ]);
        }
    }
    // `SlashCmd` takes one string and parses numbers out of it, so a wrong
    // type there is a right type with a hostile body: every command we
    // advertise, with `i64` edges and junk for its operands.
    if method == "SlashCmd" {
        for cmd in Registry::SLASH_COMMANDS {
            for tail in [
                "",
                " -1",
                " x y z",
                " 9223372036854775807 9223372036854775807",
                " -9223372036854775808 -9223372036854775808 -9223372036854775808",
                " 99999999999999999999",
            ] {
                fixed.push(vec![PyValue::Unicode(format!("/{cmd}{tail}"))]);
            }
        }
    }
    let mut out: Vec<Shape> = fixed
        .into_iter()
        .map(|args| Shape {
            args,
            kwargs: vec![],
            bind: None,
            actor: Some(me),
        })
        .collect();
    for round in 0..ROUNDS {
        let mut rng = Rng::seeded(&[service.as_bytes(), method.as_bytes(), &round.to_le_bytes()]);
        let n = rng.below(6) as usize;
        let args = (0..n).map(|_| value(&mut rng, ids, 2)).collect();
        let kwargs = if rng.below(3) == 0 {
            vec![(
                PyValue::Unicode(format!("k{}", rng.below(4))),
                value(&mut rng, ids, 1),
            )]
        } else {
            vec![]
        };
        let bind = (rng.below(2) == 0).then(|| value(&mut rng, ids, 2));
        let actor = match rng.below(4) {
            0 => None,
            1 => Some(rng.next() as i64),
            _ => Some(me),
        };
        out.push(Shape {
            args,
            kwargs,
            bind,
            actor,
        });
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("src/ is readable")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Every string literal in `src/` used as a match pattern: `"X" =>` or `"X" |`.
fn method_names() -> BTreeSet<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut out = BTreeSet::new();
    for f in files {
        let text = std::fs::read_to_string(&f).expect("source is readable");
        let b = text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'"' {
                i += 1;
                continue;
            }
            let start = i + 1;
            let mut j = start;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j < b.len() && b[j] == b'"' && j > start && !b[start].is_ascii_digit() {
                let rest = text[j + 1..].trim_start();
                if rest.starts_with("=>") || (rest.starts_with('|') && !rest.starts_with("||")) {
                    out.insert(text[start..j].to_owned());
                }
                i = j + 1;
            } else {
                i = start;
            }
        }
    }
    out
}

#[test]
fn no_arm_panics_on_wrong_typed_args() {
    let config = SandboxConfig::default();
    let c = &config.characters[0];
    let me = c.character_id;
    // Ids the default world really holds.
    let ids: Vec<i64> = [
        Some(me),
        Some(c.ship_id),
        c.station_id,
        c.solar_system_id,
        Some(c.corporation_id),
        Some(c.ship_type_id),
    ]
    .into_iter()
    .flatten()
    .collect();
    let caller = Caller::Session {
        user_id: config.user_id,
        role: roles::SANDBOX_DOCKED,
    };
    let services = Registry::services();
    let methods = method_names();
    assert!(services.len() > 20, "Registry::services() looks empty");
    assert!(
        methods.len() > 100,
        "the method scrape found almost nothing"
    );

    let previous = panic::take_hook();
    panic::set_hook(Box::new(|info| {
        let site = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let p = info.payload();
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        *LAST_PANIC.lock().unwrap_or_else(|e| e.into_inner()) = Some((site, msg));
    }));

    let mut reg = Registry::new(SandboxConfig::default());
    // (service, method) -> (site, message, how many shapes panicked)
    let mut found: BTreeMap<(String, String), (String, String, usize)> = BTreeMap::new();
    let mut calls = 0u64;
    for service in &services {
        for method in &methods {
            for s in shapes(service, method, me, &ids) {
                let req = Request {
                    service,
                    method,
                    args: &s.args,
                    kwargs: &s.kwargs,
                    bind_params: s.bind.as_ref(),
                    actor: s.actor,
                };
                calls += 1;
                let r = panic::catch_unwind(AssertUnwindSafe(|| {
                    let _ = reg.dispatch_as(caller, &req);
                }));
                // Keep the queues from growing over a million calls.
                let _ = reg.take_pending();
                if r.is_err() {
                    let (site, msg) = LAST_PANIC
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take()
                        .unwrap_or_default();
                    found
                        .entry((service.to_string(), method.clone()))
                        .or_insert((site, msg, 0))
                        .2 += 1;
                    reg = Registry::new(SandboxConfig::default());
                }
            }
        }
    }
    panic::set_hook(previous);

    let known: BTreeSet<(&str, &str)> = KNOWN_PANICS.iter().map(|(s, m, _, _)| (*s, *m)).collect();
    let mut report = String::new();
    for ((svc, m), (site, msg, n)) in &found {
        let tag = if known.contains(&(svc.as_str(), m.as_str())) {
            "known"
        } else {
            "NEW"
        };
        report.push_str(&format!(
            "  {tag}: (\"{svc}\", \"{m}\", \"{site}\", {msg:?}), // {n} shapes\n"
        ));
    }
    eprintln!(
        "arm_fuzz: {} services x {} methods, {calls} calls, {} panicking pairs\n{report}",
        services.len(),
        methods.len(),
        found.len()
    );

    let new: Vec<_> = found
        .keys()
        .filter(|(s, m)| !known.contains(&(s.as_str(), m.as_str())))
        .collect();
    let fixed: Vec<_> = KNOWN_PANICS
        .iter()
        .filter(|(s, m, _, _)| !found.contains_key(&(s.to_string(), m.to_string())))
        .map(|(s, m, _, _)| (*s, *m))
        .collect();
    assert!(
        new.is_empty(),
        "arms panic on wrong-typed args and are not in KNOWN_PANICS: {new:?}\n{report}"
    );
    assert!(
        fixed.is_empty(),
        "KNOWN_PANICS lists pairs that no longer panic; delete their rows: {fixed:?}"
    );
}
