//! **The rollover's steps**, in order (W13 packet CP25; `docs/content/plan.md`
//! §4.4). [`crate::rollover`] advances `day` before the first row and writes
//! the `[rollover]` line after the last, so neither is a row here.
//!
//! **Coordinator-only** from CP25 on, like `tick::STAGES`: a packet exports
//! its `on_rollover` and names the row in its report. A row adapts the
//! packet's own signature with a closure, as `STAGES` rows do. The order is
//! plan §4.4's: econ_agents_close (E1), asteroid (CP14'), population (CP21), econ_exchange (CP35),
//! market_history (CP42), mission (CP26a), belts (CP30), econ_ledger (CP43).

use crate::rollover::{Run, Step};

/// Every rollover step, in the order the rollover runs them.
pub const STEPS: &[Step] = &[
    // E1: settle every parked bot's offline career up to the rollover while
    // the rocks are still the day's, before the asteroid reset below resets
    // every belt. Its count is the agents settled.
    Step {
        name: "econ_agents_close",
        run: Run::Once(crate::econ_agents::on_rollover),
    },
    // CP14': the daily full reset: every gone or bitten rock comes back, the
    // live ones get their balls back. Its count is the rocks that came back.
    Step {
        name: "asteroid",
        run: Run::Once(|w, d| crate::asteroid::on_rollover(w, d.days)),
    },
    // MN-C: drop the saved rows of every moon belt whose window has passed.
    // Its count is the rows dropped.
    Step {
        name: "moon_belt",
        run: Run::Once(|w, d| crate::moon_belt::on_rollover(w, d.day)),
    },
    // CP21: expire every unvisited live site older than its row's `ttl_s`;
    // its slot refills as the next generation. Its count is the sites expired.
    Step {
        name: "population",
        run: Run::Once(|w, d| crate::population::on_rollover(w, d.day)),
    },
    // CP54a: drop every `wh` row whose pair has expired or matches no slot.
    // Its count is the rows dropped.
    Step {
        name: "wormhole",
        run: Run::Once(|w, d| crate::wormhole::on_rollover(w, d.day)),
    },
    // CP35: fold the day's trades into the trading index, move the Tier B
    // multipliers that traded, prune the NPC order rows that refilled. Its
    // count is the rows pruned.
    Step {
        name: "econ_exchange",
        run: Run::Once(crate::econ_exchange::on_rollover),
    },
    // CP42: fold every complete day's market trades into `mkthist`, then drop
    // the trades and closed orders older than `HISTORY_DAYS`. Its count is
    // the rows dropped.
    Step {
        name: "market_history",
        run: Run::Once(|w, d| w.market_history_rollover(d)),
    },
    // CP26a: drop expired mission offers. It reports no count, so the step
    // counts 0.
    Step {
        name: "missions",
        run: Run::Once(|w, d| {
            crate::mission::on_rollover(w, d.day);
            0
        }),
    },
    // CP30: drop every belt respawn wait that has passed. Its count is the
    // waits dropped.
    Step {
        name: "belts",
        run: Run::Once(|w, d| crate::belts::on_rollover(w, d.now)),
    },
    // CP38: drop every dynamic-bounty row that has recovered to the default,
    // and tell each system that climbed a push step. Its count is the rows
    // dropped.
    Step {
        name: "dbs",
        run: Run::Once(|w, d| crate::econ_dbs::on_rollover(w, d.now)),
    },
    // SOC-D4: one game day of decay on every sov development index, the
    // "increasing" arrows, and `OnSolarSystemDevIndexChanged` to every
    // claimed system. Its count is the index rows decayed.
    Step {
        name: "sov_index",
        run: Run::Once(crate::sov_index::on_rollover),
    },
    // SOC-D2: every claimed sov hub burns its fuel blocks for the day(s);
    // a hub out of fuel goes offline, and lapses after its grace days.
    // Before the ledger, so the burn is in the day's destroyed goods. Its
    // count is the blocks burned.
    Step {
        name: "sov_fuel",
        run: Run::Once(crate::sov_hub::on_rollover),
    },
    // CORP-O: bill every office whose rent is due from its corporation's
    // Master Wallet, and lapse (impound) the ones it cannot pay. Its count
    // is the offices billed or lapsed.
    Step {
        name: "corp_offices",
        run: Run::Once(|w, d| crate::service::corp_office::on_rollover(w, d.now)),
    },
    // E1: every parked bot's offline career is settled up to the rollover,
    // so the day's `[econ]` line holds the day's offline flows. Before the
    // ledger. Its count is the agents settled.
    Step {
        name: "econ_agents",
        run: Run::Once(crate::econ_agents::on_rollover),
    },
    // MINE-UI: drop Mining Ledger rows older than the window. Its count is the rows dropped.
    Step {
        name: "mining_ledger",
        run: Run::Once(|w, d| crate::mining_ledger::prune(w.deltas_mut(), d.now)),
    },
    // CP43: sum the journal since the last rollover into the day's `econ`
    // rows and print the `[econ]` and reward lines (the bot burn waits on
    // O5). Its count is the journal rows read. Last, so it sees every
    // step's money.
    Step {
        name: "econ_ledger",
        run: Run::Once(crate::econ_ledger::on_rollover),
    },
    // X-ECONREPORT: the `[econ] health` line and the TSV of ISK by class and
    // of bot ISK/h by career. After `econ_ledger`, before the journal prune.
    // Its count is the bots active.
    Step {
        name: "econ_report",
        run: Run::Once(crate::econ_report::on_rollover),
    },
    // T3-ANCIENT-RATE: the `[ancient]` line and its TSV rows. After
    // `econ_report`, which writes the TSV header. Its count is the cans won.
    Step {
        name: "ancient_rate",
        run: Run::Once(crate::ancient_rate::on_rollover),
    },
    // E4: drop the journal rows the ledger's retention drops (bot rows a
    // day after the fold above, human rows after six months) from RAM, and
    // have the ledger writer drop them and the market's from the log
    // files. After `econ_ledger`, so its cursor covers today's fold. Its
    // count is the RAM rows dropped.
    Step {
        name: "ledger",
        run: Run::Once(crate::ledger::on_rollover),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A step's name labels its count in the `[rollover]` line, so two rows
    /// with one name would make the line ambiguous.
    #[test]
    fn step_names_are_unique() {
        let mut names: Vec<&str> = STEPS.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "{names:?}");
    }
}
