//! **The friends file** (REL-FIX, owner decision 2026-10-28): `eve-bots
//! --friends FILE` names the human pilots the bots count as friends.
//!
//! One line a friend, `CHARACTER<TAB>STANDING`, the standing optional
//! (default [`DEFAULT_STANDING`]); `CHARACTER` is a character id or an exact
//! name; blank lines and lines starting with `#` are skipped. The file is
//! the owner's own (real pilots), so it lives outside the repository.
//!
//! **Two effects**, both from the bots' side, neither needing the friend to
//! do anything:
//!
//! 1. every bot sets a real **personal contact** on the friend at that
//!    standing (`charMgr.AddContact`, [`apply`]), so the client shows the
//!    friend blue (`GetRelationship`, `contacts.rs`);
//! 2. the society's hall is told ([`Hall::befriend`]), and
//!    `Hall::hostile_to` stops treating the friend as prey. This is the
//!    **seam for the R-REL-1 model lane**: [`load`] returns plain
//!    `(character id, standing)` pairs, the starting personal relations
//!    toward those characters, and the model lane replaces step 2 with its
//!    own stored levels. Nothing else in the bots reads this module.
//!
//! The standing is a plain number in `-10 ..= 10` (the server refuses
//! others). The client's own contact menu offers only -10, -5, 0, 5 and 10,
//! but its colour is by range (`Good` above 0), so the default 3 is blue
//! **[A]**; falsified by the overview showing a +3 contact grey.

use crate::corpmind::Hall;
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;

/// A friend's standing when the line gives none: Friendly (owner decision,
/// 2026-10-28).
pub const DEFAULT_STANDING: f64 = 3.0;

/// The standing bounds the server accepts.
pub const STANDING_RANGE: std::ops::RangeInclusive<f64> = -10.0..=10.0;

/// A friend as the file names them.
#[derive(Debug, Clone, PartialEq)]
pub enum Who {
    Id(i64),
    Name(String),
}

/// Parse the file's text: `(who, standing)` per line. **Pure.**
pub fn parse(text: &str) -> std::result::Result<Vec<(Who, f64)>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let at = |why: String| format!("line {}: {why}", n + 1);
        let (key, standing) = match line.split_once('\t') {
            Some((k, s)) if !s.trim().is_empty() => {
                let v: f64 = s
                    .trim()
                    .parse()
                    .map_err(|_| at(format!("standing {s:?} is not a number")))?;
                if !STANDING_RANGE.contains(&v) {
                    return Err(at(format!("standing {v} is outside -10 ..= 10")));
                }
                (k, v)
            }
            Some((k, _)) => (k, DEFAULT_STANDING),
            None => (line, DEFAULT_STANDING),
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(at("no character".to_owned()));
        }
        let who = match key.parse::<i64>() {
            Ok(id) if id > 0 => Who::Id(id),
            _ => Who::Name(key.to_owned()),
        };
        out.push((who, standing));
    }
    Ok(out)
}

/// **The pairs the society starts from**: the file's text with each name
/// turned into a character id by `resolve` (a name nobody has is an error
/// naming it). **Pure** given `resolve`; a repeated character keeps its last
/// line.
pub fn load(
    text: &str,
    mut resolve: impl FnMut(&str) -> Option<i64>,
) -> std::result::Result<Vec<(i64, f64)>, String> {
    let mut out: Vec<(i64, f64)> = Vec::new();
    for (who, standing) in parse(text)? {
        let id = match who {
            Who::Id(id) => id,
            Who::Name(name) => {
                resolve(&name).ok_or_else(|| format!("no character named {name:?}"))?
            }
        };
        out.retain(|(c, _)| *c != id);
        out.push((id, standing));
    }
    Ok(out)
}

/// A friends file, read at start and resolved at the first login.
#[derive(Debug, Default)]
pub struct Friends {
    text: String,
    resolved: std::sync::Mutex<Option<Vec<(i64, f64)>>>,
}

impl Friends {
    /// Read `path` (checked for syntax now, names resolved on a connection).
    pub fn read(path: &std::path::Path) -> std::result::Result<Friends, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("--friends {}: {e}", path.display()))?;
        parse(&text).map_err(|e| format!("--friends {}: {e}", path.display()))?;
        Ok(Friends::from_text(text))
    }

    /// From the file's text (tests).
    pub fn from_text(text: String) -> Friends {
        Friends {
            text,
            resolved: std::sync::Mutex::new(None),
        }
    }

    /// The pairs, resolved once over `conn` (`lookupSvc.LookupCharacters`,
    /// exact) and then kept.
    pub fn pairs(&self, conn: &mut Conn) -> Result<Vec<(i64, f64)>> {
        let mut held = self.resolved.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = held.as_ref() {
            return Ok(p.clone());
        }
        let pairs = load(&self.text, |name| lookup(conn, name))
            .map_err(|e| Error::NotSent(format!("--friends: {e}")))?;
        *held = Some(pairs.clone());
        Ok(pairs)
    }
}

/// A character's id by exact name, `None` if the server has none.
pub fn lookup(conn: &mut Conn, name: &str) -> Option<i64> {
    let out = conn
        .call(
            "lookupSvc",
            "LookupCharacters",
            vec![PyValue::Unicode(name.to_owned()), PyValue::Int(1)],
        )
        .ok()?;
    let Some(PyValue::List(rows)) = out.value() else {
        return None;
    };
    rows.iter().find_map(|r| {
        let PyValue::Instance { state, .. } = r else {
            return None;
        };
        match eve_server::pyutil::dict_get(state, b"characterID")? {
            PyValue::Int(id) => Some(*id),
            _ => None,
        }
    })
}

/// **Set the personal contacts** on `conn`'s pilot for every friend that is
/// not already a contact at or above its standing, and tell `hall`. Returns
/// how many contacts were set. A friend that is the pilot itself is skipped.
pub fn apply(conn: &mut Conn, friends: &Friends, hall: Option<&Hall>) -> Result<usize> {
    apply_with_mind(conn, friends, hall, None, 0)
}

/// R-REL-4: [`apply`], and the file also seeds the bot's own regard
/// ([`crate::relations::seed_friends`]): the file wins over what the bot
/// learned, at each login.
pub fn apply_with_mind(
    conn: &mut Conn,
    friends: &Friends,
    hall: Option<&Hall>,
    mind: Option<&std::sync::Mutex<crate::mind::Mind>>,
    now: u64,
) -> Result<usize> {
    let me = conn.session().char_id();
    let mut set = 0;
    let pairs = friends.pairs(conn)?;
    if let Some(m) = mind {
        let own: Vec<(i64, f64)> = pairs
            .iter()
            .copied()
            .filter(|(c, _)| Some(*c) != me)
            .collect();
        let mut m = m.lock().unwrap_or_else(|e| e.into_inner());
        crate::relations::seed_friends(&mut m.regard, &own, now);
    }
    for (id, standing) in pairs {
        if Some(id) == me {
            continue;
        }
        if let Some(h) = hall {
            h.befriend(id, standing);
        }
        let out = conn.call(
            "charMgr",
            "AddContact",
            vec![
                PyValue::Int(id),
                PyValue::Real(standing),
                PyValue::Bool(false),
            ],
        )?;
        if out.refusal().is_none() {
            set += 1;
        }
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented ids and names.
    const A: i64 = 90_000_901;
    const B: i64 = 90_000_902;

    #[test]
    fn a_line_is_an_id_or_a_name_with_an_optional_standing() {
        let text = format!("# friends\n\n{A}\t5\nPilot Nine\n{B}\t\nOther Nine\t-2.5\r\n");
        let got = parse(&text).unwrap();
        assert_eq!(
            got,
            vec![
                (Who::Id(A), 5.0),
                (Who::Name("Pilot Nine".into()), DEFAULT_STANDING),
                (Who::Id(B), DEFAULT_STANDING),
                (Who::Name("Other Nine".into()), -2.5),
            ]
        );
    }

    #[test]
    fn a_bad_standing_names_its_line() {
        assert!(
            parse(&format!("{A}\tx\n"))
                .unwrap_err()
                .starts_with("line 1")
        );
        assert!(
            parse(&format!("\n{A}\t11\n"))
                .unwrap_err()
                .starts_with("line 2")
        );
        assert!(parse("\t3\n").is_err());
    }

    #[test]
    fn load_resolves_names_and_the_last_line_for_a_character_wins() {
        let text = format!("{A}\t2\nPilot Nine\t7\n{B}\n");
        let got = load(&text, |n| (n == "Pilot Nine").then_some(A)).unwrap();
        assert_eq!(got, vec![(A, 7.0), (B, DEFAULT_STANDING)]);
        let err = load("Nobody\n", |_| None).unwrap_err();
        assert!(err.contains("Nobody"), "{err}");
    }
}
