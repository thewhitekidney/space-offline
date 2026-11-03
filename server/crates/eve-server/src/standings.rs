//! Standings (P5.7) -- who likes whom, per character.
//!
//! Moved verbatim out of `service/character.rs` by IP37-a; the
//! `standingMgr` arms that serve it stay there. `docs/lanes/standings.md`
//! has the research this rests on, the coordinator edits it needs, and what
//! is still not modelled.

use crate::pyutil::bstr;
use crate::service::Registry;
use eve_marshal::{DbRow, DbRowDescriptor, DbType, DbValue, PyValue};

/// The edges of the standings range, **[V]** `standingUtil.py:15-16`
/// (`MAX_STANDING = 10.0`, `MIN_STANDING = -10.0`). Every client formula
/// clamps to these.
pub(crate) const MAX_STANDING: f64 = 10.0;
pub(crate) const MIN_STANDING: f64 = -10.0;

/// `idCheckers.IsNPC`: `maxSystemItem < id < minPlayerOwner`, **[V]**
/// `idCheckers.py:283` with the bounds from `inventorycommon/const.py:7`
/// (9999) and `:99` (90000000).
///
/// This is the test that decides whether a standing is ever *read*.
/// `Standing.GetStanding` (`standingsvc.py:165-167`) consults
/// `npccharstandings` only `if idCheckers.IsNPC(fromID)`; any other `fromID`
/// toward the character falls through to `contactNeutralStanding`. So a
/// stored standing from a non-NPC owner is one no window will ever show.
pub(crate) fn is_npc_owner(id: i64) -> bool {
    9_999 < id && id < 90_000_000
}

/// NPC-to-character standings, for every character in the world.
///
/// **Absent means 0.0, and 0.0 means absent**, which is the client's own
/// rule rather than a storage shortcut: `Standing.OnStandingSet`
/// (`standingsvc.py:53-57`) *deletes* the `npccharstandings` row when told a
/// standing is now `0`, and `GetStanding` answers `contactNeutralStanding`
/// (0) for a missing row. Storing zeros would make `GetCharStandings` send
/// rows the client itself would have removed. **[V]**
///
/// Keyed `character -> from -> standing`. Only the NPC-to-character
/// direction exists: NPC-to-NPC is `GetNPCNPCStandings` (still empty, see
/// the note), NPC-to-corp is `GetCorpStandings`, which our NPC-corp
/// characters never call, and character-to-anything is the address book.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Standings {
    by_character: std::collections::BTreeMap<i64, std::collections::BTreeMap<i64, f64>>,
}

impl Standings {
    /// `from_id`'s standing toward `character_id`; 0.0 when there is none.
    pub(crate) fn get(&self, character_id: i64, from_id: i64) -> f64 {
        self.by_character
            .get(&character_id)
            .and_then(|m| m.get(&from_id))
            .copied()
            .unwrap_or(0.0)
    }

    /// Set one standing, returning the one it replaced. Zero removes it.
    pub(crate) fn set(&mut self, character_id: i64, from_id: i64, standing: f64) -> f64 {
        let old = self.get(character_id, from_id);
        if standing == 0.0 {
            if let Some(m) = self.by_character.get_mut(&character_id) {
                m.remove(&from_id);
                if m.is_empty() {
                    self.by_character.remove(&character_id);
                }
            }
        } else {
            self.by_character
                .entry(character_id)
                .or_default()
                .insert(from_id, standing);
        }
        old
    }

    /// Every non-zero standing toward one character, in `from_id` order.
    pub(crate) fn toward(&self, character_id: i64) -> Vec<(i64, f64)> {
        self.by_character
            .get(&character_id)
            .map(|m| m.iter().map(|(k, v)| (*k, *v)).collect())
            .unwrap_or_default()
    }

    /// Every standing in the world, as `(character, from, standing)` -- the
    /// shape a snapshot record wants.
    pub(crate) fn entries(&self) -> Vec<(i64, i64, f64)> {
        self.by_character
            .iter()
            .flat_map(|(c, m)| m.iter().map(move |(f, s)| (*c, *f, *s)))
            .collect()
    }

    /// The inverse of [`Self::entries`], for loading a world file.
    pub(crate) fn restore(entries: impl IntoIterator<Item = (i64, i64, f64)>) -> Self {
        let mut out = Standings::default();
        for (c, f, s) in entries {
            out.set(c, f, s);
        }
        out
    }
}

/// The descriptor of one `npccharstandings` row: `(fromID I4, standing R5)`.
///
/// **Copied from the client, not chosen.** `Standing.Run` builds exactly this
/// as `self.fromStandingHeader` (`standingsvc.py:39-41`) and every row the
/// client adds to the dict itself -- `_AppendNPCCharStanding`, `:97` -- is a
/// `blue.DBRow` over it. Sending the same shape means the dict holds one
/// kind of row whoever wrote it. `R5` is the 8-byte real. **[V]**
fn standing_row_descriptor() -> DbRowDescriptor {
    DbRowDescriptor::from_pairs(&[("fromID", DbType::I4), ("standing", DbType::R5)])
        .expect("two fixed-width columns are a valid layout")
}

/// `standingMgr.GetCharStandings`: a **dict** `{fromID: DBRow}`.
///
/// The caller (`standingsvc.py:129` then `:142-143`) is
///
/// ```python
/// self.npccharstandings = standingsMgr.GetCharStandings()
/// ...
/// if not isinstance(self.npccharstandings, types.DictType):
///     self.npccharstandings = self.npccharstandings.Index('fromID')
/// ```
///
/// so a dict skips the rowset path entirely, and every later reader treats
/// the values as rows with a `.standing` attribute that can be **assigned**
/// (`_ModifyCharacterStandings`, `:91`, does `npccharstandings[fromID]
/// .standing = new`). A `blue.DBRow` over [`standing_row_descriptor`] is
/// what the client puts in this dict itself, so it is the one value shape
/// that cannot be wrong for any reader. **[V]**
///
/// Empty for a character with no standings, and for no actor at all --
/// which is still a dict, still the answer the slice shipped with.
pub(crate) fn char_standings_value(reg: &Registry, actor: Option<i64>) -> PyValue {
    let Some(actor) = actor else {
        return PyValue::Dict(vec![]);
    };
    let descriptor = standing_row_descriptor();
    PyValue::Dict(
        reg.standings
            .toward(actor)
            .into_iter()
            .map(|(from_id, standing)| {
                let row = DbRow::new(
                    descriptor.clone(),
                    vec![DbValue::Int(from_id), DbValue::Real(standing)],
                )
                .expect("the values match the descriptor declared above");
                // `PyValue::Dict` is `(value, key)`, which is the marshal
                // stream's order.
                (PyValue::DbRow(Box::new(row)), PyValue::Int(from_id))
            })
            .collect(),
    )
}

/// The client's own `CalculateStandingsByRawChange`, transcribed.
///
/// **[V]** `standingUtil.py:92-99`:
///
/// ```python
/// if rawChange > 0.0:
///     if currentStanding < MAX_STANDING:
///         return min(MAX_STANDING, 10.0 * (1.0 - (1.0 - currentStanding / 10.0) * (1.0 - rawChange)))
/// elif currentStanding > MIN_STANDING:
///     return max(MIN_STANDING, 10.0 * (currentStanding / 10.0 + (1.0 + currentStanding / 10.0) * rawChange))
/// return currentStanding
/// ```
///
/// Kept here so that [`raw_change_for`] can be checked against the thing
/// the client will actually run, rather than against its own algebra.
/// Test-only: the server never applies a raw change, it stores the target.
#[cfg(test)]
pub(crate) fn client_apply_raw_change(current: f64, raw_change: f64) -> f64 {
    if raw_change > 0.0 {
        if current < MAX_STANDING {
            return MAX_STANDING.min(10.0 * (1.0 - (1.0 - current / 10.0) * (1.0 - raw_change)));
        }
    } else if current > MIN_STANDING {
        return MIN_STANDING.max(10.0 * (current / 10.0 + (1.0 + current / 10.0) * raw_change));
    }
    current
}

/// The raw change that takes a client holding `current` to `target`.
///
/// The inverse of [`client_apply_raw_change`], one branch per direction:
///
/// - up: `t = 10(1 - (1 - c/10)(1 - r))`, so `r = 1 - (10 - t) / (10 - c)`;
/// - down: `t = c + (10 + c) r`, so `r = (t - c) / (10 + c)`.
///
/// Neither denominator can be zero on the branch that uses it: going up
/// needs `c < t <= 10`, going down needs `-10 <= t < c`.
///
/// **Also right for a row the client does not have.** `_ModifyCharacter
/// Standings` (`standingsvc.py:93`) falls back to `CalculateNewStandings`,
/// `clamp(10 * rawChange)` (`standingUtil.py:110`) -- and with `c = 0` both
/// branches above reduce to `r = t / 10`, the same number. So the one case
/// where server and client could disagree about `c` (the racial-faction row
/// the client seeds at 0.0 itself, `standingsvc.py:145-147`) gives the same
/// answer either way. **[V]**
pub(crate) fn raw_change_for(current: f64, target: f64) -> f64 {
    if target > current {
        1.0 - (MAX_STANDING - target) / (MAX_STANDING - current)
    } else if target < current {
        (target - current) / (MAX_STANDING + current)
    } else {
        0.0
    }
}

/// `OnStandingSet(fromID, toID, standing)` -- the exact value.
///
/// **[V]** `standingsvc.py:49-65`, and the arity is in
/// `findings/push-events.tsv`. For `toID == session.charid` and an NPC
/// `fromID` it replaces the `npccharstandings` row with a fresh `DBRow`, or
/// deletes it when `standing` is `0`. It does **not** scatter
/// `OnNPCStandingChange` unless `session.warfactionid` is set (`:55`, `:63`),
/// which is why [`StandingsModified`] goes first.
///
/// `dockedUI/agents.py:221` also listens: the station lobby's Agents panel
/// reloads on it, so an agent whose standing gate the change crosses moves
/// between "available" and "unavailable" without the player doing anything.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StandingSet {
    pub from_id: i64,
    pub to_id: i64,
    pub standing: f64,
}

impl crate::notify::Push for StandingSet {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnStandingSet",
            vec![
                PyValue::Int(self.from_id),
                PyValue::Int(self.to_id),
                PyValue::Real(self.standing),
            ],
        )
    }

    /// The character the standing is *toward*, who need not be the GM who
    /// typed the command.
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to_id])
    }
}

/// `OnStandingsModified(modifications)` -- the one that repaints.
///
/// **[V]** `standingsvc.py:73-83`: iterated, and each element unpacked into
/// exactly five, `fromID, toID, rawChange, minAbs, maxAbs`. For our
/// character it calls `_ModifyCharacterStandings(fromID, rawChange)`, which
/// for a row the client already holds **scatters `OnNPCStandingChange`
/// unconditionally** (`:89`) -- the event the Standings panel
/// (`standingsPanel.py:221`), the Agency's agent list and factional warfare
/// listen for -- and then `AddToUnreadEntries(fromID, rawChange)`, which is
/// the up/down marker on the row (`standingEntry.py:168`, `> 0` picks the
/// icon).
///
/// `minAbs` and `maxAbs` are unpacked and **never read** anywhere in this
/// build, so they go as `None` rather than as numbers that would look
/// meaningful. **[V]**
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StandingsModified {
    pub from_id: i64,
    pub to_id: i64,
    pub raw_change: f64,
}

impl crate::notify::Push for StandingsModified {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnStandingsModified",
            vec![PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(self.from_id),
                PyValue::Int(self.to_id),
                PyValue::Real(self.raw_change),
                PyValue::None,
                PyValue::None,
            ])])],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to_id])
    }
}

/// Set one standing and tell the character's client, both ways.
///
/// **Two pushes, and the order matters.** **[A]** that this is the pair a
/// real server sends for a GM set -- the shapes are **[V]**, the choice is
/// ours, made from what each one does on the client:
///
/// 1. [`StandingsModified`], with the raw change that takes the client's
///    current value to the new one ([`raw_change_for`]). This is the only
///    push that scatters `OnNPCStandingChange` for a character outside
///    factional warfare, so it is what makes an open Standings tab repaint
///    and marks the row changed, up or down.
/// 2. [`StandingSet`], with the exact value. The first push lands on the
///    target only to floating-point precision; this one makes the client's
///    cached row bit-for-bit the number we store, deletes it outright when
///    the target is zero, and reloads the lobby's Agents panel.
///
/// **What still needs a reopen.** `StandingsPanel.OnNPCStandingChange`
/// repopulates only `if fromID == self.fromID` -- the row that is
/// *selected* -- and a row the client did not hold before is appended by
/// `_ModifyCharacterStandings` without any scatter (`standingsvc.py:93-94`).
/// Either way the client's dict is right after these two pushes, so
/// reopening the tab shows the new value with no relog.
pub(crate) fn set_standing_and_push(
    reg: &mut Registry,
    character_id: i64,
    from_id: i64,
    standing: f64,
) -> f64 {
    // CORP-S: the character's corp's derived standing moves with theirs.
    let corp = reg.character(character_id).map_or(0, |c| c.corporation_id);
    let corp_was = crate::corp_standings::before(reg, corp);
    let old = reg.standings.set(character_id, from_id, standing);
    crate::corp_standings::announce(reg, corp_was, None);
    reg.push(StandingsModified {
        from_id,
        to_id: character_id,
        raw_change: raw_change_for(old, standing),
    });
    reg.push(StandingSet {
        from_id,
        to_id: character_id,
        standing,
    });
    old
}

/// `/setstanding <fromID> <toID|me> <value> ["reason"]` -- the slash arm.
///
/// **The grammar is the client's own**, from two QA tools that build the
/// line for the server to parse (**[V]** both):
///
/// - `evemissions/client/qa_tools.py:156-166`:
///   `'setstanding %s me 0.0' % agent_id`, then the agent's corporation and
///   faction -- `me` for the caller, a bare id, a bare real;
/// - `eve/devtools/script/svc_slash.py:1397` (`/massstanding`):
///   `'/setstanding "%s" %d %s "%s"' % (fromID, charID, newStanding,
///   reason)` -- a **quoted** from-id, a numeric character id, and a quoted
///   free-text reason with spaces in it.
///
/// So quotes are stripped from the first three words and everything after
/// the third is the reason, which is accepted and not stored -- there is no
/// standing-transaction log yet for it to go into (see the note). Two words,
/// `/setstanding <fromID> <value>`, mean `me`; that short form is ours.
///
/// Refusals are answered as text, like every other arm here: the console
/// prints the string, and this is a sandbox for people, not a test harness.
pub(crate) fn set_standing_slash(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> PyValue {
    const USAGE: &str = "usage: /setstanding <fromID> [me|<characterID>] <value>";
    let unquote = |s: &str| s.trim_matches('"').to_owned();
    let (from, to, value) = match args {
        [from, value] => (unquote(from), "me".to_owned(), unquote(value)),
        [from, to, value, ..] => (unquote(from), unquote(to), unquote(value)),
        _ => return bstr(USAGE),
    };
    let Ok(from_id) = from.parse::<i64>() else {
        return bstr(&format!("not an owner id: {from}. {USAGE}"));
    };
    if !is_npc_owner(from_id) {
        // Not stored, because nothing would ever read it: see
        // `is_npc_owner`. Player-to-player standings are the address book's
        // contacts, which are a different table with a different push.
        return bstr(&format!(
            "{from_id} is not an NPC owner (faction, corporation or agent), \
             and only NPC standings toward a character exist"
        ));
    }
    let character_id = if to.eq_ignore_ascii_case("me") {
        match actor {
            Some(a) => a,
            None => return bstr("no character to set a standing for"),
        }
    } else {
        match to.parse::<i64>() {
            Ok(id) if reg.character(id).is_some() => id,
            _ => return bstr(&format!("no character {to} in this world")),
        }
    };
    let Ok(standing) = value.parse::<f64>() else {
        return bstr(&format!("not a number: {value}"));
    };
    // **[A]** Refused rather than clamped. Every client formula clamps to
    // these bounds (`standingUtil.py:94`, `:97`, `:110`), so a value outside
    // them is one the client could never hold; telling the GM is better
    // than silently storing 10 for a typed 100.
    if !standing.is_finite() || !(MIN_STANDING..=MAX_STANDING).contains(&standing) {
        return bstr(&format!(
            "standing must be between {MIN_STANDING} and {MAX_STANDING}, got {value}"
        ));
    }
    let old = set_standing_and_push(reg, character_id, from_id, standing);
    reg.mark_dirty();
    bstr(&format!(
        "standing of {from_id} toward {character_id}: {standing:.2} (was {old:.2})."
    ))
}

#[cfg(test)]
mod standings_tests {
    use super::*;
    use crate::model::SandboxConfig;
    use crate::service::{Request, ServiceError};

    /// The tests were written inside `service/character.rs` and called its
    /// `dispatch`; `Registry::dispatch` routes `standingMgr` to that one
    /// function (no service is claimed by two modules), so this is the same
    /// call made from outside the private `service::character` module.
    fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
        reg.dispatch(req)
    }

    fn registry() -> (Registry, i64) {
        let reg = Registry::new(SandboxConfig::default());
        let me = reg.config().characters[0].character_id;
        (reg, me)
    }

    fn said(v: &PyValue) -> String {
        match v {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            PyValue::Unicode(u) => u.clone(),
            other => panic!("the console prints a string, got {other:?}"),
        }
    }

    fn get_char_standings(reg: &mut Registry, actor: Option<i64>) -> PyValue {
        dispatch(
            reg,
            &Request {
                service: "standingMgr",
                method: "GetCharStandings",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor,
            },
        )
        .expect("answered")
    }

    /// The `(from, standing)` pairs in a `GetCharStandings` answer, checking
    /// on the way that each value is a row keyed by its own `fromID`.
    fn rows_of(v: &PyValue) -> Vec<(i64, f64)> {
        let PyValue::Dict(entries) = v else {
            panic!("must be a dict, so `isinstance(..., DictType)` skips `.Index`; got {v:?}")
        };
        entries
            .iter()
            .map(|(value, key)| {
                let PyValue::Int(key) = key else {
                    panic!("keyed by fromID, got {key:?}")
                };
                let PyValue::DbRow(row) = value else {
                    panic!("each value must be a DBRow with a .standing, got {value:?}")
                };
                assert_eq!(
                    row.descriptor(),
                    &standing_row_descriptor(),
                    "the client's own fromStandingHeader"
                );
                let (DbValue::Int(from), DbValue::Real(s)) = (&row.values()[0], &row.values()[1])
                else {
                    panic!("(fromID I4, standing R5), got {:?}", row.values())
                };
                assert_eq!(from, key, "the row's fromID is its key");
                (*from, *s)
            })
            .collect()
    }

    /// `Standing.Run`'s header, `standingsvc.py:39-41`, column for column.
    #[test]
    fn the_row_descriptor_is_the_clients_from_standing_header() {
        let names: Vec<Vec<u8>> = standing_row_descriptor().names();
        assert_eq!(names, vec![b"fromID".to_vec(), b"standing".to_vec()]);
        let types: Vec<DbType> = standing_row_descriptor()
            .columns()
            .iter()
            .map(|c| c.db_type)
            .collect();
        assert_eq!(types, vec![DbType::I4, DbType::R5]);
    }

    #[test]
    fn a_character_with_no_standings_gets_an_empty_dict() {
        let (mut reg, me) = registry();
        assert!(rows_of(&get_char_standings(&mut reg, Some(me))).is_empty());
        assert!(rows_of(&get_char_standings(&mut reg, None)).is_empty());
    }

    #[test]
    fn a_set_standing_is_served_back_as_a_row() {
        let (mut reg, me) = registry();
        let answer = set_standing_slash(&mut reg, Some(me), &["500001", "me", "5.5"]);
        assert!(said(&answer).contains("5.50"), "{}", said(&answer));
        assert_eq!(
            rows_of(&get_char_standings(&mut reg, Some(me))),
            vec![(500_001, 5.5)]
        );
    }

    /// The client deletes a row it is told is now 0 (`standingsvc.py:53-57`),
    /// so a zero must not come back as a row on the next login either.
    #[test]
    fn setting_zero_removes_the_row() {
        let (mut reg, me) = registry();
        set_standing_slash(&mut reg, Some(me), &["1000125", "me", "3"]);
        set_standing_slash(&mut reg, Some(me), &["1000125", "me", "0.0"]);
        assert!(rows_of(&get_char_standings(&mut reg, Some(me))).is_empty());
        assert!(reg.standings.entries().is_empty());
    }

    /// Both client spellings, from the two tools that build the line.
    #[test]
    fn both_client_grammars_parse() {
        let (mut reg, me) = registry();
        // `qa_tools.py:156`, which sends no leading slash.
        set_standing_slash(&mut reg, Some(me), &["3010110", "me", "10.0"]);
        assert_eq!(reg.standings.get(me, 3_010_110), 10.0);
        // `svc_slash.py:1397`, split on whitespace by `Registry::slash`:
        // a quoted from-id, a numeric character id, and a quoted reason
        // that the split has broken into words.
        let line = format!("\"500002\" {me} -2.5 \"Mass standing change by GM Person\"");
        let words: Vec<&str> = line.split_whitespace().collect();
        set_standing_slash(&mut reg, Some(me), &words);
        assert_eq!(reg.standings.get(me, 500_002), -2.5);
    }

    /// `GetStanding` reads `npccharstandings` only for an `IsNPC` from-id,
    /// with the bounds `idCheckers.py:283` and `const.py` give.
    #[test]
    fn the_npc_test_is_the_clients() {
        assert!(!is_npc_owner(9_999));
        assert!(is_npc_owner(10_000));
        assert!(is_npc_owner(500_001));
        assert!(is_npc_owner(1_000_125));
        assert!(is_npc_owner(3_010_110));
        assert!(is_npc_owner(89_999_999));
        assert!(!is_npc_owner(90_000_000));
        let (mut reg, me) = registry();
        set_standing_slash(&mut reg, Some(me), &[&me.to_string(), "me", "5"]);
        assert!(
            reg.standings.entries().is_empty(),
            "a player's standing toward a player is never read from this table"
        );
    }

    /// The raw change we send must land, **through the client's formula**,
    /// on the value we store -- from every starting point, both directions,
    /// and the edges.
    #[test]
    fn the_raw_change_lands_on_the_target_through_the_clients_formula() {
        let points = [-10.0, -9.99, -5.0, -0.01, 0.0, 0.01, 2.5, 6.67, 9.99, 10.0];
        for &c in &points {
            for &t in &points {
                let r = raw_change_for(c, t);
                let got = client_apply_raw_change(c, r);
                assert!(
                    (got - t).abs() < 1e-9,
                    "from {c} to {t}: raw {r} lands on {got}"
                );
            }
        }
    }

    /// A row the client does not hold takes `CalculateNewStandings`,
    /// `clamp(10 * rawChange)` (`standingUtil.py:110`) -- which must agree.
    #[test]
    fn a_row_the_client_lacks_lands_on_the_target_too() {
        for t in [-10.0, -3.3, 0.5, 7.0, 10.0] {
            let r = raw_change_for(0.0, t);
            let got = (10.0 * r).clamp(MIN_STANDING, MAX_STANDING);
            assert!((got - t).abs() < 1e-12, "{t}: {got}");
        }
    }

    /// The two pushes, their shapes, and who they are for.
    #[test]
    fn a_set_pushes_the_modification_then_the_exact_value_to_the_target() {
        let (mut reg, me) = registry();
        reg.take_pending();
        set_standing_slash(&mut reg, Some(me), &["500001", "me", "4"]);
        let pushes: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| (p.to_push(), p.audience()))
            .collect();
        assert_eq!(pushes.len(), 2, "{pushes:?}");

        let ((event, args), audience) = &pushes[0];
        assert_eq!(*event, "OnStandingsModified");
        assert_eq!(audience, &Some(vec![me]));
        // One argument, a list of 5-tuples: `standingsvc.py:74-75`.
        let [PyValue::List(mods)] = args.as_slice() else {
            panic!("{args:?}")
        };
        let [PyValue::Tuple(m)] = mods.as_slice() else {
            panic!("{mods:?}")
        };
        assert_eq!(m.len(), 5, "unpacked into exactly five");
        assert_eq!(m[0], PyValue::Int(500_001));
        assert_eq!(m[1], PyValue::Int(me));
        let PyValue::Real(r) = m[2] else {
            panic!("{m:?}")
        };
        assert!((client_apply_raw_change(0.0, r) - 4.0).abs() < 1e-12);

        let ((event, args), audience) = &pushes[1];
        assert_eq!(*event, "OnStandingSet");
        assert_eq!(audience, &Some(vec![me]));
        // `OnStandingSet(self, fromID, toID, standing)`.
        assert_eq!(
            args,
            &vec![PyValue::Int(500_001), PyValue::Int(me), PyValue::Real(4.0)]
        );
    }

    #[test]
    fn standings_round_trip_through_entries() {
        let mut s = Standings::default();
        s.set(1, 500_001, 2.0);
        s.set(1, 1_000_125, -1.5);
        s.set(2, 500_001, 9.0);
        assert_eq!(Standings::restore(s.entries()), s);
    }

    /// Through the world file and back into a fresh world, the way a restart
    /// does it -- and the save is asked for, so it happens at all.
    #[test]
    fn a_set_standing_survives_the_world_file() {
        let (mut reg, me) = registry();
        reg.take_dirty();
        set_standing_slash(&mut reg, Some(me), &["500001", "me", "-7.25"]);
        assert!(reg.take_dirty(), "a set must mark the world dirty");
        let dir = std::env::temp_dir().join(format!(
            "eve-standings-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a directory to write in");
        let store = crate::store::FileStore::new(dir.join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
        let snap = crate::store::Store::load(&store)
            .expect("read the world")
            .expect("a world was written");
        let _ = std::fs::remove_dir_all(&dir);
        let (mut fresh, _) = registry();
        assert_eq!(
            fresh.standings.get(me, 500_001),
            0.0,
            "or this proves nothing"
        );
        fresh.apply_snapshot(snap);
        assert_eq!(fresh.standings.get(me, 500_001), -7.25);
        assert_eq!(
            rows_of(&get_char_standings(&mut fresh, Some(me))),
            vec![(500_001, -7.25)]
        );
    }
}
