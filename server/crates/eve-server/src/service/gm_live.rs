//! **The GM live views** (W13 packet CP25): `/live`, `/live <systemID>` and
//! `/rollover [now]`, the operator's in-game window on C7 §7
//! (`docs/content/C7-liveness.md` §7.3-§7.4, plan §4.4).
//!
//! Each command is a function for a `slash::COMMANDS` row, and the GM gate is
//! the slash console's own (`service/gm.rs`, `ROLE_GML`). The text is built
//! by pure functions of the world and a time, so the tests need no clock.
//!
//! - `/live`: the `[live]` line (`live_status::live_line`), with this
//!   window's turn numbers so far.
//! - `/live <systemID>`: that system's status-file row under its header, or
//!   why there is none (cold, or not a system).
//! - `/rollover`: the day, the time to the next rollover, one in progress and
//!   the last `[rollover]` line. `/rollover now` forces one
//!   (`rollover::force`).

use crate::damage::BlueTime;
use crate::live_status::{STATUS_HEADER, live_line, peek_turns, status_tsv};
use crate::liveness::Liveness;
use crate::pyutil::bstr;
use crate::rollover::Rollover;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// `/live [systemID]`.
pub(crate) fn live(
    reg: &mut Registry,
    _actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    Ok(bstr(&live_text(reg, crate::pyutil::filetime_now(), args)))
}

/// `/rollover [now]`.
pub(crate) fn rollover(
    reg: &mut Registry,
    _actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    Ok(bstr(&rollover_text(
        reg,
        crate::pyutil::filetime_now(),
        args,
    )))
}

/// `/essdeposit <isk>`.
pub(crate) fn essdeposit(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    Ok(bstr(&crate::ess::gm_deposit_text(reg, actor, args)))
}

/// What `/live` answers at `now`.
pub(crate) fn live_text(w: &Registry, now: BlueTime, args: &[&str]) -> String {
    let arg = match args {
        [] => return live_line(w, now, &peek_turns(w)),
        [one] => *one,
        _ => return "usage: /live [systemID]".to_owned(),
    };
    let Ok(system) = arg.parse::<i64>() else {
        return format!("usage: /live [systemID] -- '{arg}' is not a system id (a number)");
    };
    if w.ext::<Liveness>().get(system).is_some() {
        let prefix = format!("{system}\t");
        if let Some(row) = status_tsv(w).lines().find(|l| l.starts_with(&prefix)) {
            // Sites come with CP21 (population); the row's site columns say
            // what is known until then.
            return format!("{STATUS_HEADER}\n{row}");
        }
    }
    if w.celestials().system(system).is_some() {
        format!(
            "system {system} is cold: no pilot, NPC or jump keeps it live, so it holds \
             nothing in memory and answers from its generator and deltas"
        )
    } else {
        format!(
            "system {system} is not live and is not in the celestial catalogue: \
             check the id (solar systems are 30000001-30999999)"
        )
    }
}

/// `XhYm` for a span of blue ticks; negative reads as `0h0m`.
fn hours_minutes(ticks: i64) -> String {
    let mins = ticks.max(0) / (crate::seed::HOUR / 60);
    format!("{}h{}m", mins / 60, mins % 60)
}

/// What `/rollover` answers at `now`; `now` as the argument forces one.
pub(crate) fn rollover_text(w: &mut Registry, now: BlueTime, args: &[&str]) -> String {
    match args {
        [] => {}
        ["now"] => {
            return match crate::rollover::force(w, now) {
                Some(line) => line,
                None => format!(
                    "rollover to day {} begun; {} systems still queued, the [rollover] \
                     line follows in the server log when they are done",
                    w.world().day,
                    w.ext::<Rollover>().queued()
                ),
            };
        }
        _ => return "usage: /rollover [now]".to_owned(),
    }
    let world = *w.world();
    // Lane clock: at a `--time-scale` above 1 the day has no 11:00 UTC.
    let clock = crate::game_clock::of(w);
    let next = clock.next_rollover(world.birth, world.last_rollover, now);
    let mut out = format!(
        "day {} | next rollover in {} ({})",
        world.day,
        hours_minutes(next - now),
        if clock.is_real() {
            "11:00 UTC".to_owned()
        } else {
            format!("time scale {}", clock.scale())
        }
    );
    let slot = w.ext::<Rollover>();
    if let Some(d) = slot.in_progress() {
        out.push_str(&format!(
            " | rolling over to day {}: {} systems queued",
            d.day,
            slot.queued()
        ));
    }
    match slot.last() {
        Some(line) => {
            out.push_str(" | last: ");
            out.push_str(line);
        }
        None => out.push_str(" | no rollover since the server started"),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::World;
    use crate::liveness::Reasons;
    use std::collections::BTreeMap;

    fn reg() -> Registry {
        let mut w = Registry::new(crate::model::SandboxConfig::default());
        w.deltas_mut().set_world(World::FIXTURE);
        w
    }

    /// `/live` is the `[live]` line: every field C7 §7.1 names.
    #[test]
    fn live_holds_the_live_fields() {
        let w = reg();
        let now = World::FIXTURE.birth + 1;
        let text = live_text(&w, now, &[]);
        assert_eq!(text, live_line(&w, now, &peek_turns(&w)));
        for field in [
            "[live]",
            "day ",
            "next-rollover ",
            "systems hot ",
            "warm ",
            "pilots ",
            "npcs ",
            "rocks ",
            "deltas",
        ] {
            assert!(text.contains(field), "{field:?} missing from {text}");
        }
    }

    #[test]
    fn live_for_a_live_system_is_its_status_row() {
        let mut w = reg();
        let system = 30_900_001;
        let reasons: BTreeMap<i64, Reasons> = [(system, Reasons::DOCKED)].into();
        w.ext_mut::<Liveness>()
            .apply(&reasons, World::FIXTURE.birth);
        let text = live_text(&w, World::FIXTURE.birth, &[&system.to_string()]);
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(STATUS_HEADER));
        assert!(
            lines
                .next()
                .unwrap()
                .starts_with(&format!("{system}\twarm\t"))
        );
    }

    /// A bad id says what is wrong, rather than an empty answer or an error.
    #[test]
    fn a_bad_system_id_answers_usefully() {
        let w = reg();
        let now = World::FIXTURE.birth;
        let not_a_number = live_text(&w, now, &["Jita"]);
        assert!(not_a_number.contains("not a system id"), "{not_a_number}");
        assert!(not_a_number.contains("usage: /live"), "{not_a_number}");
        let unknown = live_text(&w, now, &["12345"]);
        assert!(unknown.contains("12345"), "{unknown}");
        assert!(
            unknown.contains("not in the celestial catalogue"),
            "{unknown}"
        );
        assert!(live_text(&w, now, &["1", "2"]).starts_with("usage:"));
    }

    #[test]
    fn rollover_now_advances_the_day_and_status_shows_it() {
        let mut w = reg();
        let now = World::FIXTURE.birth + 1;
        let before = rollover_text(&mut w, now, &[]);
        assert!(before.starts_with("day 0 |"), "{before}");
        assert!(before.contains("no rollover since"), "{before}");
        let forced = rollover_text(&mut w, now, &["now"]);
        assert!(forced.starts_with("[rollover] day 1 (+1):"), "{forced}");
        assert_eq!(w.world().day, 1);
        let after = rollover_text(&mut w, now, &[]);
        assert!(after.starts_with("day 1 |"), "{after}");
        assert!(after.contains(&forced), "{after}");
        assert!(rollover_text(&mut w, now, &["later"]).starts_with("usage:"));
        assert_eq!(w.world().day, 1, "a bad argument forces nothing");
    }
}
