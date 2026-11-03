//! **The bot's side of the offline agents** (E1 = R-ECON E12 = R-SOCIETY
//! SOC-E1; the server's half is `eve_server::econ_agents`).
//!
//! A `--society` bot plays in sessions ([`crate::driver::Timetable`]); between
//! them it is logged off and the economy loses its output. The server settles
//! that time lazily, but it cannot see the bot's mind: it does not know which
//! career the bot was playing. So **at its log-off, docked, the bot says so,
//! once**: `econAgent.Park(career)`. That is the whole of this module.
//!
//! **Why a new call and not an existing one.** No existing call carries a
//! career; the character description would hide bot state in a text meant
//! for humans, and a GM slash would be GM-gated (T9, `gm.rs`: GM is for
//! setup only, and a log-off is not setup). The call is not GM: the server
//! answers it for characters it marked bots (`Character::is_bot`), and it
//! **grants nothing**. It names a career; the server then settles only what
//! the character's real hull, modules, rounds, blueprints, skills and wallet
//! can do. A bot that never calls it is never settled.
//!
//! **Which careers.** [`SETTLED`] mirrors the server's
//! `econ_agents::CAREERS` (a test pins the two together). A career the server
//! has no rule for (`mission_runner`, `explorer`, ...) is not
//! parked: [`park`] does not even make the call, and the act line says why,
//! so the bot's log shows the gap instead of a silent idle.
//!
//! **What the bot does at log-on**: nothing. The server settles the span as
//! the character's session is selected, before the bot's first read, so the
//! bot's ordinary reads of its wallet and hangar see exactly the settled
//! state.

use crate::{Conn, Result};
use eve_marshal::PyValue;

/// The service the call goes to.
pub const SERVICE: &str = "econAgent";

/// The careers the server settles offline: the names of
/// `eve_server::econ_agents::CAREERS`, which is where a new one is added
/// first. `tests::the_settled_list_is_the_servers` keeps the two equal.
pub const SETTLED: &[&str] = &[
    "miner",
    "ice_miner",
    "gas_huffer",
    "ratter",
    "site_runner",
    "fw_plexer",
    "builder",
    "trader",
    "pi_farmer",
];

/// Whether the server has an offline rule for `career`.
pub fn is_settled(career: &str) -> bool {
    SETTLED.contains(&career)
}

/// What the server said to a [`park`].
#[derive(Debug, Clone, PartialEq)]
pub enum Parking {
    /// The server holds the career; it settles the bot while it is away.
    Parked(String),
    /// The server refused, with its reason (not a bot, not docked, no rule).
    Refused(String),
    /// The call was not made or not answered: this career has no offline
    /// rule, or the server has no `econAgent` (an older build).
    Unavailable(String),
}

impl Parking {
    /// The act line for the bot's log: one sentence.
    pub fn line(&self, career: &str) -> String {
        match self {
            Parking::Parked(c) => format!("parked as a {c}: the server settles the time away"),
            Parking::Refused(why) => format!("not parked as a {career}: {why}"),
            Parking::Unavailable(why) => format!("not parked as a {career}: {why}"),
        }
    }

    /// Whether the server holds the bot as an agent now.
    pub fn parked(&self) -> bool {
        matches!(self, Parking::Parked(_))
    }
}

/// Read a `Park` reply: `{parked: bool, career: str, reason: str}`.
pub fn read_reply(v: &PyValue) -> Parking {
    let get = |k: &str| eve_server::pyutil::dict_get(v, k.as_bytes());
    let text = |k: &str| get(k).and_then(crate::text).unwrap_or_default();
    match get("parked") {
        Some(PyValue::Bool(true)) => Parking::Parked(text("career")),
        Some(PyValue::Bool(false)) => Parking::Refused(text("reason")),
        _ => Parking::Unavailable("the reply was not a Park answer".to_owned()),
    }
}

/// **Park the bot's career** at its log-off. Docked, as the server requires;
/// a career without a rule makes no call. Only a lost connection is an
/// `Err`: a server without the service, or one that refuses, is a
/// [`Parking`] the caller logs and goes on from.
pub fn park(conn: &mut Conn, career: &str) -> Result<Parking> {
    if !is_settled(career) {
        return Ok(Parking::Unavailable(
            "no offline rule for this career".to_owned(),
        ));
    }
    let outcome = match conn.call(
        SERVICE,
        "Park",
        vec![PyValue::Bytes(career.as_bytes().to_vec())],
    ) {
        Ok(o) => o,
        Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
        Err(e) => return Ok(Parking::Unavailable(e.to_string())),
    };
    if let Some(key) = outcome.refusal() {
        return Ok(Parking::Unavailable(format!("the server raised {key}")));
    }
    Ok(outcome.value().map_or_else(
        || Parking::Unavailable("no answer".to_owned()),
        |v| read_reply(&v),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_server::pyutil::{bstr, pydict};

    #[test]
    fn the_settled_list_is_the_servers() {
        let server: Vec<&str> = eve_server::econ_agents::CAREERS
            .iter()
            .map(|c| c.name())
            .collect();
        assert_eq!(SETTLED, &server[..]);
        // And every one is a career the bot's own table has.
        for name in SETTLED {
            assert!(
                crate::career::find(crate::career::CAREERS, name).is_some(),
                "{name} is not a bot career"
            );
        }
    }

    #[test]
    fn a_career_without_a_rule_is_never_parked() {
        assert!(is_settled("miner") && is_settled("builder"));
        for c in ["mission_runner", "explorer", "pirate", "soldier", ""] {
            assert!(!is_settled(c), "{c}");
        }
    }

    #[test]
    fn the_reply_is_read_by_its_keys() {
        let ok = pydict(vec![
            ("parked", PyValue::Bool(true)),
            ("career", bstr("ratter")),
            ("reason", bstr("")),
        ]);
        assert_eq!(read_reply(&ok), Parking::Parked("ratter".to_owned()));
        let no = pydict(vec![
            ("parked", PyValue::Bool(false)),
            ("career", bstr("")),
            ("reason", bstr("only a docked character can be parked")),
        ]);
        assert_eq!(
            read_reply(&no),
            Parking::Refused("only a docked character can be parked".to_owned())
        );
        assert!(matches!(
            read_reply(&PyValue::None),
            Parking::Unavailable(_)
        ));
    }

    #[test]
    fn the_act_line_says_what_happened() {
        assert!(
            Parking::Parked("miner".to_owned())
                .line("miner")
                .starts_with("parked as a miner")
        );
        let no = Parking::Refused("not a bot".to_owned()).line("miner");
        assert!(no.contains("not parked") && no.contains("not a bot"));
        assert!(Parking::Parked(String::new()).parked());
        assert!(!Parking::Unavailable(String::new()).parked());
    }
}
