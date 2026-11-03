//! The agents over the wire (E1): `econAgent`, the bot's one call, and the
//! GM's `/settle`.
//!
//! - **`econAgent.Park(career)`**: the bot, docked, at its log-off, names the
//!   career it was playing ([`super::park`]). Answers a dict
//!   `{parked: bool, career: str, reason: str}`; a refusal is data, not an
//!   error, so the bot can log the reason and stay live. A character the
//!   server did not mark a bot, a career with no rule, a character in space:
//!   refused. Nothing in the call grants anything; see the module header of
//!   [`crate::econ_agents`] for why this is a new call and not an existing
//!   one.
//! - **`econAgent.Status()`**: the caller's own row, `{parked: bool,
//!   career, settled_to, runs}`, so a bot (or a test) can read what the
//!   server holds for it.
//! - **`/settle`** (GM, tests and operators):
//!   `/settle` -- the agents: parked by career, the settlements and ISK so
//!   far, the last reports;
//!   `/settle park <career>` -- park the acting character (no bot check);
//!   `/settle <hours> [name]` -- settle the acting (or named) parked
//!   character as if it had been away `<hours>` game hours: the row's
//!   settled-to mark moves back by that span and the settlement runs now;
//!   `/settle sweep` -- run a sweep of every due agent now.

use super::{Agent, Book, Career, agent, park, park_as, parked, sweep};
use crate::pyutil::{bstr, pydict};
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

/// The services dispatched here.
pub const SERVICES: &[&str] = &["econAgent"];

/// `econAgent`'s calls.
pub fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match (req.service, req.method) {
        ("econAgent", "Park") => Ok(park_call(reg, req)),
        ("econAgent", "Status") => Ok(status_call(reg, req)),
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

fn park_call(reg: &mut Registry, req: &Request<'_>) -> PyValue {
    let refused = |why: String| {
        pydict(vec![
            ("parked", PyValue::Bool(false)),
            ("career", bstr("")),
            ("reason", bstr(&why)),
        ])
    };
    let Some(actor) = req.actor else {
        return refused("no character".to_owned());
    };
    let Some(name) = req
        .args
        .first()
        .and_then(crate::pyutil::as_bytes)
        .and_then(|b| std::str::from_utf8(b).ok())
    else {
        return refused("Park(career): the career is a name".to_owned());
    };
    match park(reg, actor, name, crate::pyutil::filetime_now()) {
        Ok(c) => pydict(vec![
            ("parked", PyValue::Bool(true)),
            ("career", bstr(c.name())),
            ("reason", bstr("")),
        ]),
        Err(r) => refused(r.to_string()),
    }
}

fn status_call(reg: &Registry, req: &Request<'_>) -> PyValue {
    let row = req.actor.and_then(|a| agent(reg, a));
    pydict(vec![
        ("parked", PyValue::Bool(row.is_some())),
        ("career", bstr(row.map_or("", |a| a.career.name()))),
        ("settled_to", PyValue::Int(row.map_or(0, |a| a.settled_to))),
        ("runs", PyValue::Int(row.map_or(0, |a| a.runs as i64))),
    ])
}

const USAGE: &str = "usage: /settle | /settle park <career> | /settle <game hours> [character name] | /settle sweep";

/// `/settle ...`: see the module header.
pub(crate) fn settle_cmd(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    Ok(bstr(&settle_text(reg, actor, args)))
}

fn settle_text(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> String {
    let now = crate::pyutil::filetime_now();
    match args {
        [] => overview(reg),
        ["sweep"] => {
            let n = sweep(reg, now, usize::MAX);
            format!("swept: {n} agents settled")
        }
        ["park", career] => {
            let Some(me) = actor else {
                return "no character".to_owned();
            };
            match park_as(reg, me, career, now, false) {
                Ok(c) => format!("parked as a {}", c.name()),
                Err(r) => format!("not parked: {r}"),
            }
        }
        [hours, rest @ ..] => {
            let Ok(h) = hours.parse::<f64>() else {
                return format!("{USAGE} -- '{hours}' is not a number of game hours");
            };
            if !(h.is_finite() && h > 0.0) {
                return format!("{USAGE} -- hours must be positive");
            }
            let who = match rest {
                [] => actor,
                name => reg
                    .config()
                    .characters
                    .iter()
                    .find(|c| c.name == name.join(" "))
                    .map(|c| c.character_id),
            };
            let Some(who) = who else {
                return "no such character".to_owned();
            };
            let Some(a) = agent(reg, who) else {
                return "not parked: /settle park <career> first".to_owned();
            };
            // The span is game time: back off by its wall length, so the
            // clock's own conversion (`elapsed`) gives `h` hours at any scale.
            let clock = crate::game_clock::of(reg);
            let wall = clock.span((h * 3_600.0 * 1.0e7) as i64);
            let moved = Agent {
                settled_to: now - wall,
                ..a
            };
            let span_s = clock.elapsed(wall) as f64 / 1.0e7;
            match super::settle_span(reg, who, moved, span_s, now) {
                Some(r) => r.line(),
                None => "nothing settled".to_owned(),
            }
        }
    }
}

/// `/settle` with no arguments.
fn overview(reg: &Registry) -> String {
    let mut by_career: std::collections::BTreeMap<Career, usize> = Default::default();
    for (_, a) in parked(reg) {
        *by_career.entry(a.career).or_default() += 1;
    }
    let careers: Vec<String> = by_career
        .iter()
        .map(|(c, n)| format!("{}={n}", c.name()))
        .collect();
    let book = reg.ext::<Book>();
    let mut out = vec![format!(
        "[settle] parked {} [{}], settled {} (net {:+.0} ISK) since start",
        by_career.values().sum::<usize>(),
        careers.join(" "),
        book.settled(),
        book.isk()
    )];
    out.extend(book.recent().iter().rev().take(5).map(|r| r.line()));
    out.join("\n")
}
