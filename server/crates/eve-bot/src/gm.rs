//! GM slash commands, fenced off.
//!
//! **GM is for setup only** (`docs/lanes/bots.md` §2.1, §0 point 5): giving a
//! bot its skill, its hull and its laser when the hangar lacks them. The
//! mining loop never calls this. The fence is visibility: `pub(crate)`, so
//! nothing outside this crate can reach it, and inside the crate the one
//! caller is the driver's setup step (`driver.rs`, `Act::GmSetup`), whose
//! every call is an act-log entry of its own -- which is how T9 proves it
//! never runs after the first undock. A test that needs a
//! GM command calls `slash.SlashCmd` itself, as `tests/bots/npc.rs` does, which is
//! visible at the call site.

use crate::{CallOutcome, Conn, Result};
use eve_marshal::PyValue;

/// `slash.SlashCmd(line)`, a plain service call, as the client's console
/// sends it (`_SlashCmd`, which stringifies the reply; `service.rs::slash`).
///
/// **SKILLGATE (open mode only, the server gate's `/giveskill`)**: before a
/// `/createitem <type> [n]`, the skill each of that type's direct
/// requirements asks is given the same way (`/giveskill <skill> <level>`),
/// so the hull can be boarded and the module fitted and put online. This is
/// the one chokepoint of every open-mode setup, `drone_use` included. The
/// closed economy never calls it: there the skills are trained.
pub(crate) fn slash(conn: &mut Conn, line: &str) -> Result<CallOutcome> {
    let wanted = createitem_requirements(line);
    if !wanted.is_empty() {
        // `/giveskill` SETS the level (`Skills::grant_type`), so only what is
        // lacking is sent: a pilot above the requirement keeps its level.
        let levels = crate::acquire::gate_levels(conn)?;
        for (skill, level) in grants_needed(&wanted, levels.as_ref()) {
            conn.call(
                "slash",
                "SlashCmd",
                vec![PyValue::Bytes(
                    format!("/giveskill {skill} {level}").into_bytes(),
                )],
            )?;
        }
    }
    conn.call(
        "slash",
        "SlashCmd",
        vec![PyValue::Bytes(line.as_bytes().to_vec())],
    )
}

/// The `(skill, level)` of `wanted` the pilot's trained `levels` do not reach
/// (pure): never a level at or below what is trained, since a grant sets it.
/// `None` (the read was refused, or there is no content) grants nothing.
pub(crate) fn grants_needed(
    wanted: &[(i64, i64)],
    levels: Option<&std::collections::BTreeMap<i64, i64>>,
) -> Vec<(i64, i64)> {
    let Some(levels) = levels else {
        return Vec::new();
    };
    wanted
        .iter()
        .copied()
        .filter(|(s, l)| levels.get(s).copied().unwrap_or(0) < *l)
        .collect()
}

/// The direct requirements of the type a `/createitem` line names
/// (pure over the thread's content, [`crate::acquire::use_content`]); empty
/// for any other line, and for a bot with no content.
pub(crate) fn createitem_requirements(line: &str) -> Vec<(i64, i64)> {
    let mut words = line.split_whitespace();
    if words.next() != Some("/createitem") {
        return Vec::new();
    }
    words
        .next()
        .and_then(crate::acquire::type_named)
        .map(crate::acquire::direct_requirements)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const GATED: i64 = 97_001;
    const SKILL: i64 = 97_101;

    #[test]
    fn slash_createitem_gives_the_requirements() {
        let rows = format!(
            "type\t{GATED}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
             182={SKILL};277=3\tGated Hull\n"
        );
        let table =
            eve_server::typedata::Table::parse(&format!("{}{rows}", eve_server::typedata::FIXTURE))
                .expect("the rows parse");
        let mut c = eve_server::content::Content::fixture();
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        crate::acquire::use_content(&Arc::new(c));
        let line = |l: &str| createitem_requirements(l);
        assert_eq!(line(&format!("/createitem {GATED}")), vec![(SKILL, 3)]);
        assert_eq!(line(&format!("/createitem {GATED} 2")), vec![(SKILL, 3)]);
        assert_eq!(line("/createitem gated hull"), Vec::new(), "one word only");
        assert_eq!(line("/createitem Fixture"), Vec::new());
        assert_eq!(line(&format!("/giveskill {GATED} 1")), Vec::new());
        assert_eq!(line("/createitem 97999"), Vec::new());
    }

    /// `/giveskill` sets the level: a pilot above the requirement is sent
    /// nothing, one below it only what it lacks.
    #[test]
    fn a_grant_never_lowers_a_level() {
        use std::collections::BTreeMap;
        let wanted = [(SKILL, 3), (SKILL + 1, 1)];
        let above = BTreeMap::from([(SKILL, 5), (SKILL + 1, 4)]);
        assert_eq!(grants_needed(&wanted, Some(&above)), Vec::new());
        let at = BTreeMap::from([(SKILL, 3)]);
        assert_eq!(grants_needed(&wanted, Some(&at)), vec![(SKILL + 1, 1)]);
        let below = BTreeMap::from([(SKILL, 2)]);
        assert_eq!(
            grants_needed(&wanted, Some(&below)),
            vec![(SKILL, 3), (SKILL + 1, 1)]
        );
        assert_eq!(grants_needed(&wanted, None), Vec::new(), "unread: nothing");
    }
}
