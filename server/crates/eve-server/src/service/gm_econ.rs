//! **The GM economy view** (W16 packet CP43, E11): `/econ`, `/econ price
//! <typeID>`, `/econ history [days]`, `/econ closed [days]` and `/econ burn
//! on|off`, C5 §7.4
//! (`docs/content/C5-economy.md`). The figures are
//! [`crate::econ_ledger`]'s; this module only words them.
//!
//! Each command is a function for a `slash::COMMANDS` row, and the GM gate is
//! the slash console's own (`service/gm.rs`, `ROLE_GML`). The text is built
//! by pure functions of the world and a time, so the tests need no clock.
//!
//! - `/econ`: the `[econ]` line of the day so far (the journal since the last
//!   rollover's cursor), the last recorded day's line, the bots over
//!   [`econ_ledger::BOT_WALLET_CAP`], the top ten types by NPC fill value
//!   since the last rollover, and the C5 §5.4 reward line.
//! - `/econ price <typeID>`: `P_ref`, its source, the trading index; Tier
//!   A's factor on `basePrice` (E5b R3) and its orders in the actor's
//!   region; and Tier B's multipliers on `P_ref` at the actor's station.
//! - `/econ history [days]`: the stored days' lines, newest last (default
//!   [`HISTORY_DEFAULT`]).
//! - `/econ closed [days]` (E11, R-ECON §2.6): the closed loop. Today so far
//!   and the stored days: goods minted and destroyed per class, the price
//!   index and its types, the bot-human ISK flow; the falsifiers read off
//!   them; the demand budget spent per human; whether the burn is on.
//! - `/econ burn on|off`: the rollover burn's switch
//!   ([`ledger::set_burn`]), off by default (CP43).

use crate::damage::BlueTime;
use crate::econ_exchange::{self as exchange, Side};
use crate::econ_ledger::{self as ledger, Totals};
use crate::pyutil::bstr;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// How many types `/econ` lists by NPC fill value (C5 §7.4: ten).
pub const TOP_TYPES: usize = 10;

/// `/econ history` with no count: a week, O5's window.
pub const HISTORY_DEFAULT: usize = 7;

const USAGE: &str = "usage: /econ [regionID | report [regionID] [hours] | tsv [hours] | price-tape [regionID] [days] | audit [days] | price <typeID> | history [days] | closed [days] | burn on|off]";

/// `/econ [price <typeID> | history [days] | closed [days] | burn on|off]`.
pub(crate) fn econ(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    if let ["burn", v] = args {
        let on = match *v {
            "on" => true,
            "off" => false,
            _ => return Ok(bstr(&format!("{USAGE} -- burn takes on or off"))),
        };
        ledger::set_burn(reg, on);
        return Ok(bstr(&burn_text(reg)));
    }
    Ok(bstr(&econ_text(
        reg,
        actor,
        crate::pyutil::filetime_now(),
        args,
    )))
}

/// What `/econ` answers at `now`.
pub(crate) fn econ_text(w: &Registry, actor: Option<i64>, now: BlueTime, args: &[&str]) -> String {
    match args {
        [] => overview(w, now),
        // X-ECONREPORT: ISK by class per region, ISK/h by career and tier.
        [region] if region.parse::<i64>().is_ok() => crate::econ_report::text(
            w,
            now,
            region.parse().ok(),
            crate::econ_report::DEFAULT_HOURS,
        ),
        ["report"] => crate::econ_report::text(w, now, None, crate::econ_report::DEFAULT_HOURS),
        ["report", rest @ ..] => report_args(w, now, rest),
        ["tsv"] => crate::econ_report::tsv_text(w, now, crate::econ_report::DEFAULT_HOURS),
        ["tsv", h] => match h.parse::<f64>() {
            Ok(h) if h > 0.0 => crate::econ_report::tsv_text(w, now, h),
            _ => format!("{USAGE} -- '{h}' is not a number of game hours"),
        },
        // FE-MEASURE: the price tape and the manipulation detector.
        ["price-tape", rest @ ..] => match tape_args(rest) {
            Some((region, days)) => crate::econ_report::tape_text(w, now, region, days * 24.0),
            None => format!("{USAGE} -- price-tape takes a region id and game days"),
        },
        ["audit"] => crate::econ_report::audit_text(w, now, 24.0),
        ["audit", d] => match d.parse::<f64>() {
            Ok(d) if d > 0.0 => crate::econ_report::audit_text(w, now, d * 24.0),
            _ => format!("{USAGE} -- '{d}' is not a number of game days"),
        },
        ["price", t] => match t.parse::<i64>() {
            Ok(type_id) => price(w, actor, type_id, now),
            Err(_) => format!("{USAGE} -- '{t}' is not a type id (a number)"),
        },
        ["history"] => history(w, HISTORY_DEFAULT),
        ["history", n] => match n.parse::<usize>() {
            Ok(n) => history(w, n),
            Err(_) => format!("{USAGE} -- '{n}' is not a number of days"),
        },
        ["closed"] => closed(w, HISTORY_DEFAULT),
        ["closed", n] => match n.parse::<usize>() {
            Ok(n) => closed(w, n),
            Err(_) => format!("{USAGE} -- '{n}' is not a number of days"),
        },
        ["burn"] => burn_text(w),
        _ => USAGE.to_owned(),
    }
}

/// `price-tape [regionID] [days]`: a number under 100000 is the days, so
/// `price-tape 2` is two days of everything and `price-tape 10000001 2` is
/// two days of one region.
fn tape_args(rest: &[&str]) -> Option<(Option<i64>, f64)> {
    let days = |d: &str| d.parse::<f64>().ok().filter(|d| *d > 0.0);
    match rest {
        [] => Some((None, 1.0)),
        [x] if x.parse::<i64>().is_ok_and(|r| r >= 100_000) => Some((Some(x.parse().ok()?), 1.0)),
        [x] => Some((None, days(x)?)),
        [r, d] => Some((Some(r.parse().ok()?), days(d)?)),
        _ => None,
    }
}

/// `/econ report [regionID] [hours]`.
fn report_args(w: &Registry, now: BlueTime, rest: &[&str]) -> String {
    let default = crate::econ_report::DEFAULT_HOURS;
    let parsed = match rest {
        [r] => r.parse::<i64>().ok().map(|r| (Some(r), default)),
        [r, h] => r
            .parse::<i64>()
            .ok()
            .zip(h.parse::<f64>().ok().filter(|h| *h > 0.0))
            .map(|(r, h)| (Some(r), h)),
        _ => None,
    };
    match parsed {
        Some((region, hours)) => crate::econ_report::text(w, now, region, hours),
        None => format!("{USAGE} -- report takes a region id and game hours"),
    }
}

/// The burn's state, as `/econ burn` and `/econ closed` say it.
fn burn_text(w: &Registry) -> String {
    let over = ledger::over_cap(w);
    format!(
        "the rollover burn is {} ({} bots over the {:.0} ISK cap)",
        if ledger::burn_enabled(w) { "on" } else { "off" },
        over.len(),
        ledger::BOT_WALLET_CAP
    )
}

/// `/econ closed [days]`.
fn closed(w: &Registry, n: usize) -> String {
    let world = *w.world();
    let today = ledger::Closed::of(w, world.last_rollover, ledger::goods_today(w));
    let mut out = vec![format!(
        "[econ] day {} so far {}",
        world.day.saturating_add(1),
        today.part()
    )];
    if !today.index_types.is_empty() {
        let list = today
            .index_types
            .iter()
            .map(|(t, r)| format!("{t} x{r:.2}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!("index types (price / P_ref): {list}"));
    }
    let days: Vec<(u32, ledger::Closed)> = ledger::history(w)
        .iter()
        .filter_map(|t| Some((t.day, ledger::closed_of_day(w, t.day)?)))
        .collect();
    let from = days.len().saturating_sub(n.max(1));
    let days = &days[from..];
    for (day, c) in days {
        out.push(format!("day {day}: {}", c.part()));
    }
    // R-ECON §2.6's falsifiers, read off the stored days.
    let (lo, hi) = ledger::INDEX_BAND;
    let outside = days
        .iter()
        .rev()
        .take_while(|(_, c)| c.index.is_some_and(|i| i < lo || i > hi))
        .count();
    let (minted, destroyed) = days.iter().fold((0, 0), |(m, d), (_, c)| {
        let (a, b) = c.goods.total();
        (m + a, d + b)
    });
    out.push(format!(
        "falsifiers: index outside [{lo}, {hi}] for the last {outside} day(s) (alarm at {}); \
         over {} day(s) goods minted {minted}, destroyed {destroyed}",
        ledger::INDEX_DAYS,
        days.len()
    ));
    let spent = ledger::demand_spent(w);
    if spent.is_empty() {
        out.push(format!(
            "demand budget: no bot bought from a human today (cap {:.0} per human)",
            ledger::BOT_DEMAND_PER_HUMAN
        ));
    } else {
        let list = spent
            .iter()
            .map(|(h, isk)| format!("{h} {isk:.0}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!(
            "demand budget (cap {:.0} per human): {list}",
            ledger::BOT_DEMAND_PER_HUMAN
        ));
    }
    out.push(burn_text(w));
    out.join("\n")
}

/// `/econ` with no argument.
fn overview(w: &Registry, now: BlueTime) -> String {
    let world = *w.world();
    let s = ledger::since(w, ledger::cursor(w));
    let today = Totals::of(w, world.day.saturating_add(1), &s, world.last_rollover, now);
    let mut out = vec![today.line(" so far")];
    out.push(match ledger::history(w).last() {
        Some(t) => format!("last: {}", t.line("")),
        None => "last: no day recorded yet (the first rollover writes one)".to_owned(),
    });
    let over = ledger::over_cap(w);
    let excess = over.iter().fold(0.0, |a, (_, x)| a + x);
    out.push(format!(
        "bots over the {:.0} ISK cap: {} holding {excess:.0} over it; the rollover burn is {}",
        ledger::BOT_WALLET_CAP,
        over.len(),
        if ledger::burn_enabled(w) {
            "on"
        } else {
            "off (CP43: until O5's week of ledger drift; /econ burn on)"
        }
    ));
    let top = ledger::top_npc_fills(w, world.last_rollover, TOP_TYPES);
    if top.is_empty() {
        out.push("NPC fills since the last rollover: none".to_owned());
    } else {
        let list = top
            .iter()
            .map(|(t, v, q)| format!("{t} {v:.0} ISK ({q} units)"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!("top NPC fills since the last rollover: {list}"));
    }
    out.push(ledger::reward_line(w, &s));
    out.join("\n")
}

/// `/econ price <typeID>`.
fn price(w: &Registry, actor: Option<i64>, type_id: i64, now: BlueTime) -> String {
    let prices = w.content().get::<crate::econ_prices::Table>();
    let Some(p) = prices.prices.get(&type_id) else {
        return format!(
            "type {type_id} has no P_ref: it is not priced, or the server runs without --content"
        );
    };
    let avg = crate::econ_prices::average(w, type_id).unwrap_or(p.ref_price);
    let ask_factor = crate::item::find_in(w.content(), type_id)
        .and_then(|t| exchange::ask_factor(t.category_id))
        .map_or("none".to_owned(), |f| format!("x{f}"));
    let mut out = format!(
        "type {type_id}: P_ref {:.2} ({:?}, floor {:.2}), avg {avg:.2} | Tier A on basePrice: ask {ask_factor}, bid x{}",
        p.ref_price,
        p.source,
        p.floor_price,
        exchange::TIER_A_BID,
    );
    let station = actor
        .and_then(|a| w.character(a))
        .and_then(|c| c.station_id);
    if let Some((_, region)) = station.and_then(|s| exchange::place(w, s)) {
        let tier_a: Vec<_> = exchange::orders(w, region, Some(type_id), now)
            .into_iter()
            .filter(|o| o.tier == exchange::Tier::A)
            .collect();
        let best = |side: Side| {
            let v: Vec<f64> = tier_a
                .iter()
                .filter(|o| o.side == side)
                .map(|o| o.price)
                .collect();
            let p = match side {
                Side::Ask => v.iter().copied().reduce(f64::min),
                Side::Bid => v.iter().copied().reduce(f64::max),
            };
            p.map_or("none".to_owned(), |p| {
                format!("{p:.2} at {} station(s)", v.len())
            })
        };
        out.push_str(&format!(
            " | Tier A in region {region}: ask {}, bid {}",
            best(Side::Ask),
            best(Side::Bid)
        ));
    }
    match station {
        Some(station) => {
            let ask = exchange::multiplier(w, station, type_id, Side::Ask);
            let bid = exchange::multiplier(w, station, type_id, Side::Bid);
            out.push_str(&format!(
                " | Tier B on P_ref at station {station} (Tier B applies at a hub): ask x{ask:.3} = {:.2}, bid x{bid:.3} = {:.2}",
                ask * p.ref_price,
                bid * p.ref_price,
            ));
        }
        None => out.push_str(" | Tier B: dock to see a station's multipliers"),
    }
    out
}

/// `/econ history [days]`.
fn history(w: &Registry, n: usize) -> String {
    let all = ledger::history(w);
    if all.is_empty() {
        return "no day recorded yet (the first rollover writes one)".to_owned();
    }
    let from = all.len().saturating_sub(n.max(1));
    all[from..]
        .iter()
        .map(|t| t.line(""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::World;
    use crate::model::SandboxConfig;
    use crate::rollover::Day;

    const ME: i64 = 90_000_001;

    fn reg() -> Registry {
        let mut w = Registry::new(SandboxConfig::default());
        w.deltas_mut().set_world(World::FIXTURE);
        w
    }

    fn now() -> BlueTime {
        World::FIXTURE.birth + 1
    }

    #[test]
    fn econ_prints_the_line_the_last_day_the_bots_and_the_rewards() {
        let mut w = reg();
        let text = econ_text(&w, Some(ME), now(), &[]);
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].starts_with("[econ] day 1 so far faucet="),
            "{text}"
        );
        assert!(lines[1].contains("no day recorded yet"), "{text}");
        assert!(lines[2].starts_with("bots over the"), "{text}");
        assert!(
            lines[3].contains("NPC fills since the last rollover"),
            "{text}"
        );
        assert!(lines[4].starts_with("[econ] rewards L1"), "{text}");
        // What the rollover does before its steps run: advance the day.
        let world = World {
            day: 1,
            ..World::FIXTURE
        };
        w.deltas_mut().set_world(world);
        ledger::step(
            &mut w,
            &Day {
                day: 1,
                days: 1,
                now: now(),
            },
            false,
        );
        let text = econ_text(&w, Some(ME), now(), &[]);
        assert!(text.contains("last: [econ] day 1 faucet="), "{text}");
        assert!(text.starts_with("[econ] day 2 so far"), "{text}");
    }

    #[test]
    fn econ_price_names_p_ref_its_source_and_the_multipliers() {
        let w = reg();
        let (&type_id, p) = w
            .content()
            .get::<crate::econ_prices::Table>()
            .prices
            .iter()
            .next()
            .expect("the fixture prices a type");
        let text = econ_text(&w, Some(ME), now(), &["price", &type_id.to_string()]);
        assert!(
            text.contains(&format!("P_ref {:.2}", p.ref_price)),
            "{text}"
        );
        assert!(text.contains(&format!("{:?}", p.source)), "{text}");
        assert!(text.contains("Tier A on basePrice"), "{text}");
        assert!(
            !text.contains("Tier A ask x"),
            "E5b: Tier A is no multiple of P_ref: {text}"
        );
        assert!(text.contains("Tier B on P_ref at station"), "{text}");
        let undocked = econ_text(&w, None, now(), &["price", &type_id.to_string()]);
        assert!(undocked.contains("dock to see"), "{undocked}");
    }

    #[test]
    fn a_bad_argument_answers_usefully() {
        let w = reg();
        let n = now();
        assert!(econ_text(&w, None, n, &["price", "Tritanium"]).contains("not a type id"));
        assert!(econ_text(&w, None, n, &["price", "1"]).contains("has no P_ref"));
        assert!(econ_text(&w, None, n, &["history", "x"]).contains("not a number"));
        assert!(econ_text(&w, None, n, &["what"]).starts_with("usage: /econ"));
        assert!(econ_text(&w, None, n, &["history"]).contains("no day recorded"));
    }

    /// E11: `/econ closed` prints today's closed-loop figures, the stored
    /// days, the falsifiers, the demand budget and the burn's state.
    #[test]
    fn econ_closed_prints_the_loop_the_falsifiers_and_the_controls() {
        let mut w = reg();
        let text = econ_text(&w, None, now(), &["closed"]);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("[econ] day 1 so far index="), "{text}");
        assert!(text.contains("goods="), "{text}");
        assert!(text.contains("falsifiers: index outside"), "{text}");
        assert!(text.contains("demand budget"), "{text}");
        assert!(
            lines.last().unwrap().starts_with("the rollover burn is"),
            "{text}"
        );
        for day in 1..=2 {
            ledger::step(
                &mut w,
                &Day {
                    day,
                    days: 1,
                    now: now(),
                },
                false,
            );
        }
        let text = econ_text(&w, None, now(), &["closed", "1"]);
        assert!(text.contains("\nday 2: index="), "{text}");
        assert!(!text.contains("\nday 1: "), "{text}");
        assert!(econ_text(&w, None, now(), &["closed", "x"]).contains("not a number"));
    }

    #[test]
    fn econ_burn_switches_the_rollover_burn() {
        let mut w = reg();
        let say = |w: &mut Registry, args: &[&str]| match econ(w, None, args).unwrap() {
            PyValue::Bytes(b) => String::from_utf8(b).unwrap(),
            other => panic!("{other:?}"),
        };
        assert!(say(&mut w, &["burn", "on"]).starts_with("the rollover burn is on"));
        assert!(ledger::burn_enabled(&w));
        assert!(say(&mut w, &["burn", "off"]).starts_with("the rollover burn is off"));
        assert!(!ledger::burn_enabled(&w));
        assert!(say(&mut w, &["burn", "maybe"]).starts_with("usage: /econ"));
    }

    #[test]
    fn history_prints_the_last_n_days_oldest_first() {
        let mut w = reg();
        for day in 1..=3 {
            ledger::step(
                &mut w,
                &Day {
                    day,
                    days: 1,
                    now: now(),
                },
                false,
            );
        }
        let text = econ_text(&w, None, now(), &["history", "2"]);
        let days: Vec<&str> = text.lines().map(|l| &l[..13]).collect();
        assert_eq!(days, vec!["[econ] day 2 ", "[econ] day 3 "]);
    }
}
