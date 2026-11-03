//! evecap's destiny view (CAPTURE-UNDOCK): the `DoDestinyUpdate` payloads,
//! decoded to what the client's own ballpark reads.
//!
//! A notification's body is `(0, pickle((0, (1, args))))` and, for
//! `DoDestinyUpdate`, `args = ([(stamp, (method, args)), ...], 0)`. A
//! `SetState` carries a `KeyVal` whose `state` is the ball blob; an
//! `AddBalls2` carries `((blob, slims),)`. The blob is read by the same
//! [`Park::decode`] the server's encoder is tested against, so a capture is
//! read by the code that wrote it.

use eve_marshal::{MachoNetMsg, PyValue};
use eve_proto::{CallPayload, MachoAddress, MachoPacket};
use eve_server::capture::{Direction, Record};
use eve_server::destiny::{Ball, Mode, Park};
use eve_server::pyutil::{as_bytes, dict_get};

/// One `(stamp, (method, args))` entry of a `DoDestinyUpdate`.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub stamp: i64,
    pub what: Update,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// `SetState`: the ego ball id and the blob's balls.
    SetState { ego: i64, park: Option<Park> },
    /// `AddBalls2`: the blob's balls.
    AddBalls2 { park: Option<Park>, slims: usize },
    /// Any other method, with its arguments.
    Call { method: String, args: Vec<PyValue> },
}

/// The entries of a `DoDestinyUpdate` notification's body (the first element
/// of the packet body, the pickle holder), or `None` if it is not one.
pub fn entries_of_body(
    body: &PyValue,
    load: &dyn Fn(&[u8]) -> Option<PyValue>,
) -> Option<Vec<Entry>> {
    let PyValue::Tuple(t) = body else { return None };
    let pickle = match t.get(1)? {
        PyValue::Bytes(b) | PyValue::SubStream(b) => b,
        _ => return None,
    };
    let outer = load(pickle)?;
    let PyValue::Tuple(o) = outer else {
        return None;
    };
    let PyValue::Tuple(inner) = o.get(1)? else {
        return None;
    };
    let PyValue::Tuple(args) = inner.get(1)? else {
        return None;
    };
    entries_of_args(args)
}

/// The entries of `([(stamp, (method, args)), ...], 0)`.
pub fn entries_of_args(args: &[PyValue]) -> Option<Vec<Entry>> {
    let PyValue::List(list) = args.first()? else {
        return None;
    };
    let mut out = Vec::new();
    for item in list {
        let PyValue::Tuple(pair) = item else {
            return None;
        };
        let PyValue::Int(stamp) = pair.first()? else {
            return None;
        };
        let PyValue::Tuple(call) = pair.get(1)? else {
            return None;
        };
        let method = String::from_utf8_lossy(as_bytes(call.first()?)?).into_owned();
        let PyValue::Tuple(margs) = call.get(1)? else {
            return None;
        };
        out.push(Entry {
            stamp: *stamp,
            what: classify(&method, margs),
        });
    }
    Some(out)
}

fn classify(method: &str, args: &[PyValue]) -> Update {
    match (method, args.first()) {
        ("SetState", Some(bag)) => {
            let state = bag_get(bag, b"state").and_then(as_bytes);
            let ego = match bag_get(bag, b"ego") {
                Some(PyValue::Int(i)) => *i,
                _ => 0,
            };
            Update::SetState {
                ego,
                park: state.and_then(Park::decode),
            }
        }
        ("AddBalls2", Some(PyValue::Tuple(t))) => {
            let blob = t.first().and_then(as_bytes);
            let slims = match t.get(1) {
                Some(PyValue::List(l)) => l.len(),
                _ => 0,
            };
            Update::AddBalls2 {
                park: blob.and_then(Park::decode),
                slims,
            }
        }
        _ => Update::Call {
            method: method.to_owned(),
            args: args.to_vec(),
        },
    }
}

/// An attribute of a `KeyVal` instance, or a key of a plain dict.
fn bag_get<'a>(bag: &'a PyValue, key: &[u8]) -> Option<&'a PyValue> {
    match bag {
        PyValue::Instance { state, .. } => dict_get(state, key),
        other => dict_get(other, key),
    }
}

pub fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn mode_name(m: &Mode) -> String {
    match m {
        Mode::Goto(p) => format!("GOTO({:.0},{:.0},{:.0})", p[0], p[1], p[2]),
        Mode::Follow { ball_id, range } => format!("FOLLOW({ball_id},{range})"),
        Mode::Stop => "STOP".to_owned(),
        Mode::Orbit { ball_id, range } => format!("ORBIT({ball_id},{range})"),
        Mode::Troll { effect_stamp } => format!("TROLL({effect_stamp})"),
        Mode::Warp {
            goto,
            effect_stamp,
            total_distance,
            stop_distance,
            warp_factor,
        } => format!(
            "WARP(stamp {effect_stamp}, len {total_distance:.0}, stop {stop_distance:.0}, factor {warp_factor}, goto=({:.0},{:.0},{:.0}))",
            goto[0], goto[1], goto[2]
        ),
        Mode::Missile { follow_id, .. } => format!("MISSILE({follow_id})"),
    }
}

/// One ball as the client's destiny reads it, on one line. `rel` is a
/// reference point (the station's centre) the position is also given from.
pub fn ball_line(b: &Ball, rel: Option<[f64; 3]>) -> String {
    let p = b.position;
    let mut s = format!(
        "ball {} {} r={} pos=({:.1},{:.1},{:.1})",
        b.ball_id,
        mode_name(&b.mode),
        b.radius,
        p[0],
        p[1],
        p[2]
    );
    if let Some(c) = rel {
        let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
        s += &format!(
            " rel=({:.1},{:.1},{:.1}) dist={:.1}",
            d[0],
            d[1],
            d[2],
            norm(d)
        );
    }
    s += &format!(" flags={:#04x} mass={:.4e}", b.flags, b.mass);
    if let Some(ph) = &b.physics {
        s += &format!(
            " maxV={} v=({:.2},{:.2},{:.2}) |v|={:.2} agility={} frac={}",
            ph.max_velocity,
            ph.velocity[0],
            ph.velocity[1],
            ph.velocity[2],
            norm(ph.velocity),
            ph.agility,
            ph.speed_fraction
        );
    } else {
        s += " (not free)";
    }
    s
}

/// The ball a call acts on: its first argument.
pub fn call_ball(args: &[PyValue]) -> Option<i64> {
    match args.first()? {
        PyValue::Int(i) => Some(*i),
        _ => None,
    }
}

/// Every ball id a call names in its first argument: one id, or a list of
/// them (`RemoveBalls`).
pub fn call_balls(args: &[PyValue]) -> Vec<i64> {
    match args.first() {
        Some(PyValue::Int(i)) => vec![*i],
        Some(PyValue::List(l) | PyValue::Tuple(l)) => l
            .iter()
            .filter_map(|v| {
                if let PyValue::Int(i) = v {
                    Some(*i)
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A float or int argument, for a call's compact rendering.
pub fn arg_text(a: &PyValue) -> String {
    match a {
        PyValue::Int(i) | PyValue::Long(i) => i.to_string(),
        PyValue::Real(r) => format!("{r:.3}"),
        PyValue::Bool(b) => b.to_string(),
        PyValue::None => "None".to_owned(),
        PyValue::Tuple(t) | PyValue::List(t) => {
            format!(
                "[{}]",
                t.iter().map(arg_text).collect::<Vec<_>>().join(", ")
            )
        }
        PyValue::Bytes(b) | PyValue::SubStream(b) => match std::str::from_utf8(b) {
            Ok(t) if !t.is_empty() && t.chars().all(|c| !c.is_control()) => format!("{t:?}"),
            _ => format!("<{} bytes>", b.len()),
        },
        PyValue::Unicode(u) => format!("{u:?}"),
        PyValue::Dict(d) => format!("{{{} entries}}", d.len()),
        _ => "?".to_owned(),
    }
}

/// Every line a decoded update prints (all balls, or only `only`), for the
/// packet view. `rel` as in [`ball_line`].
pub fn describe(e: &Entry, only: &[i64], rel: Option<[f64; 3]>) -> Vec<String> {
    let want = |id: i64| only.is_empty() || only.contains(&id);
    let mut out = Vec::new();
    match &e.what {
        Update::SetState { ego, park } => {
            out.push(format!("  stamp {} SetState ego={ego}", e.stamp));
            match park {
                Some(p) => {
                    out.push(format!(
                        "    clock {} balls {}",
                        p.current_time,
                        p.balls.len()
                    ));
                    out.extend(
                        p.balls
                            .iter()
                            .filter(|b| want(b.ball_id))
                            .map(|b| format!("    {}", ball_line(b, rel))),
                    );
                }
                None => out.push("    <blob did not decode>".to_owned()),
            }
        }
        Update::AddBalls2 { park, slims } => {
            let hit = park
                .as_ref()
                .is_none_or(|p| p.balls.iter().any(|b| want(b.ball_id)));
            if hit {
                out.push(format!("  stamp {} AddBalls2 ({slims} slims)", e.stamp));
            }
            match park {
                Some(p) => out.extend(
                    p.balls
                        .iter()
                        .filter(|b| want(b.ball_id))
                        .map(|b| format!("    {}", ball_line(b, rel))),
                ),
                None => out.push("    <blob did not decode>".to_owned()),
            }
        }
        Update::Call { method, args } => {
            if only.is_empty() || call_balls(args).into_iter().any(want) {
                out.push(format!(
                    "  stamp {} {method}({})",
                    e.stamp,
                    args.iter().map(arg_text).collect::<Vec<_>>().join(", ")
                ));
            }
        }
    }
    out
}

/// What the destiny view is asked for.
#[derive(Debug, Default)]
pub struct View {
    pub balls: Vec<i64>,
    pub rel: Option<i64>,
    pub after: f64,
    pub before: f64,
}

/// Where each ball was last seen, so `--rel` can name the station's centre.
#[derive(Debug, Default)]
pub struct Tracker {
    pos: std::collections::BTreeMap<i64, [f64; 3]>,
}

impl Tracker {
    fn learn(&mut self, e: &Entry) {
        match &e.what {
            Update::SetState { park: Some(p), .. } | Update::AddBalls2 { park: Some(p), .. } => {
                for b in &p.balls {
                    self.pos.insert(b.ball_id, b.position);
                }
            }
            Update::Call { method, args } if method == "SetBallPosition" => {
                if let (
                    Some(id),
                    Some(PyValue::Real(x)),
                    Some(PyValue::Real(y)),
                    Some(PyValue::Real(z)),
                ) = (call_ball(args), args.get(1), args.get(2), args.get(3))
                {
                    self.pos.insert(id, [*x, *y, *z]);
                }
            }
            _ => {}
        }
    }
}

fn load(b: &[u8]) -> Option<PyValue> {
    eve_server::wire::loads_unbudgeted(b).ok()
}

/// The destiny entries of a record, if it is a server `DoDestinyUpdate`.
fn entries_of(r: &Record) -> Option<Vec<Entry>> {
    if r.direction != Direction::ServerToClient {
        return None;
    }
    let packet = MachoPacket::from_value(&load(&r.bytes)?).ok()?;
    if packet.command != MachoNetMsg::Notification {
        return None;
    }
    match &packet.destination {
        MachoAddress::Broadcast { broadcast_id, .. } if broadcast_id == "DoDestinyUpdate" => {}
        _ => return None,
    }
    entries_of_body(packet.body.first()?, &load)
}

/// A client call as one line: `service.method(args)`, with its substream
/// decoded, for the packet view.
fn call_line(r: &Record) -> Option<String> {
    if r.direction != Direction::ClientToServer {
        return None;
    }
    let packet = MachoPacket::from_value(&load(&r.bytes)?).ok()?;
    if packet.command != MachoNetMsg::CallReq {
        return None;
    }
    let service = packet.destination.service().unwrap_or("?").to_owned();
    match CallPayload::from_payload(packet.body.first()?).ok()? {
        CallPayload::Service { method, args, .. } => Some(format!(
            "    call {service}.{method}({})",
            args.iter().map(arg_text).collect::<Vec<_>>().join(", ")
        )),
        CallPayload::BoundObject {
            object_id,
            method,
            args,
            ..
        } => Some(format!(
            "    call [{object_id}].{method}({})",
            args.iter().map(arg_text).collect::<Vec<_>>().join(", ")
        )),
        _ => None,
    }
}

/// The extra lines the packet view prints under a record.
pub fn decoded_lines(r: &Record, t: &mut Tracker, v: &View) -> Option<Vec<String>> {
    if let Some(line) = call_line(r) {
        return if v.balls.is_empty() {
            Some(vec![line])
        } else {
            None
        };
    }
    let entries = entries_of(r)?;
    let mut out = Vec::new();
    for e in &entries {
        t.learn(e);
        let rel = v.rel.and_then(|id| t.pos.get(&id).copied());
        out.extend(describe(e, &v.balls, rel));
    }
    Some(out)
}

/// `--timeline`: one line per update that touches a named ball (every ball
/// when none is named), with the frame's time and the batch's stamp.
pub fn view_destiny(records: &[Record], v: &View, timeline: bool) {
    let _ = timeline;
    let mut t = Tracker::default();
    for (i, r) in records.iter().enumerate() {
        let ms = r.micros as f64 / 1000.0;
        let in_window = ms >= v.after && ms < v.before;
        if let Some(line) = call_line(r) {
            // What the pilot asked for, for context: his own `Cmd*` calls.
            if in_window && line.contains(".Cmd") {
                println!("[{i:4}] {ms:10.1}ms C->S {}", line.trim_start());
            }
            continue;
        }
        let Some(entries) = entries_of(r) else {
            continue;
        };
        for e in &entries {
            t.learn(e);
            if !in_window {
                continue;
            }
            let rel = v.rel.and_then(|id| t.pos.get(&id).copied());
            for l in describe(e, &v.balls, rel) {
                println!("[{i:4}] {ms:10.1}ms {}", l.trim_start());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_server::destiny::{FLAGS_FREE_BALL, NO_FORMATION, Physics};
    use eve_server::pyutil::{bstr, keyval};

    fn ball(id: i64, pos: [f64; 3], vel: [f64; 3], mode: Mode) -> Ball {
        Ball {
            ball_id: id,
            mode,
            radius: 20.0,
            position: pos,
            flags: FLAGS_FREE_BALL,
            mass: 1.0e6,
            cloak: 0,
            harmonic: -1,
            corporation_id: 0,
            alliance_id: 0,
            physics: Some(Physics {
                max_velocity: 250.0,
                velocity: vel,
                agility: 0.5,
                speed_fraction: 0.0,
            }),
            formation_id: NO_FORMATION,
        }
    }

    fn entry(stamp: i64, method: &str, args: Vec<PyValue>) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(stamp),
            PyValue::Tuple(vec![bstr(method), PyValue::Tuple(args)]),
        ])
    }

    #[test]
    fn a_set_state_reads_back_its_ego_and_balls() {
        let park = Park {
            current_time: 77,
            balls: vec![
                ball(9001, [1.0, 2.0, 3.0], [300.0, 0.0, 400.0], Mode::Stop),
                ball(9002, [0.0; 3], [0.0; 3], Mode::Goto([5.0, 6.0, 7.0])),
            ],
        };
        let bag = keyval(vec![
            ("state", PyValue::Bytes(park.encode())),
            ("ego", PyValue::Int(9001)),
        ]);
        let args = vec![
            PyValue::List(vec![entry(77, "SetState", vec![bag])]),
            PyValue::Int(0),
        ];
        let es = entries_of_args(&args).expect("entries");
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].stamp, 77);
        let Update::SetState { ego, park: Some(p) } = &es[0].what else {
            panic!("not a SetState: {:?}", es[0].what)
        };
        assert_eq!(*ego, 9001);
        assert_eq!(p.balls.len(), 2);
        let v = p.balls[0].physics.unwrap().velocity;
        assert!((norm(v) - 500.0).abs() < 1e-9, "|v| {}", norm(v));
        let line = ball_line(&p.balls[0], Some([1.0, 2.0, 3.0]));
        assert!(line.contains("STOP") && line.contains("|v|=500.00") && line.contains("dist=0.0"));
    }

    #[test]
    fn add_balls2_and_plain_calls_decode() {
        let park = Park {
            current_time: 5,
            balls: vec![ball(42, [10.0; 3], [0.0; 3], Mode::Stop)],
        };
        let add = entry(
            5,
            "AddBalls2",
            vec![PyValue::Tuple(vec![
                PyValue::Bytes(park.encode()),
                PyValue::List(vec![PyValue::None]),
            ])],
        );
        let goto = entry(
            6,
            "GotoPoint",
            vec![
                PyValue::Int(42),
                PyValue::Real(1.0),
                PyValue::Real(2.0),
                PyValue::Real(3.5),
            ],
        );
        let es = entries_of_args(&[PyValue::List(vec![add, goto]), PyValue::Int(0)]).unwrap();
        let Update::AddBalls2 {
            park: Some(p),
            slims,
        } = &es[0].what
        else {
            panic!("{:?}", es[0].what)
        };
        assert_eq!((p.balls[0].ball_id, *slims), (42, 1));
        assert_eq!(
            describe(&es[1], &[42], None),
            vec!["  stamp 6 GotoPoint(42, 1.000, 2.000, 3.500)".to_owned()]
        );
        // The ball filter hides other balls' calls.
        assert!(describe(&es[1], &[43], None).is_empty());
    }

    #[test]
    fn a_body_that_is_not_a_destiny_update_is_none() {
        assert!(entries_of_args(&[PyValue::Int(1)]).is_none());
        assert!(entries_of_body(&PyValue::None, &|_| None).is_none());
        // A truncated blob does not decode, and says so.
        let bad = classify(
            "AddBalls2",
            &[PyValue::Tuple(vec![PyValue::Bytes(vec![0, 1])])],
        );
        assert!(matches!(bad, Update::AddBalls2 { park: None, .. }));
    }
}
