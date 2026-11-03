//! **The campaign**: what a reinforced hub waits for (SOC-D3a's half of the
//! packet pair; D3b adds the command nodes and the node fight). Lane note
//! `docs/lanes/sov-d3a.md`, spec R-SOV §6.4 (reinforce), §6.5.
//!
//! # What D3a owns here
//!
//! When an attack on a held hub completes (`entosis::reinforce`), the hub
//! goes `reinforced` and **one [`Campaign`] row** is written and scheduled:
//!
//! ```text
//! sovcamp <systemID> -> hubID eventType defenderAlliance startTime started createdTime attackers scores [more...]
//! ```
//!
//! - `startTime` is the first prime-window start at least
//!   [`REINFORCE_DELAY`] (a game span, so `--time-scale` shortens it) after
//!   the attack completed **[V2: 48 h; the alignment to the window is
//!   A]**;
//! - `attackers` is the alliances whose links were scoring at completion
//!   (`-` for none: the GM line), `scores` the `team:score` pairs the
//!   client draws, `{defender: 0.6, -defender: 0.4}` at the start
//!   ([`DEFENDER_START_SCORE`], **[V2]**);
//! - the fields after `scores` belong to D3b ([`Campaign::rest`]) and are
//!   kept verbatim through every rewrite, as D1 keeps a `sov` row's.
//!
//! [`tick`] begins a campaign whose time came ([`begin`]): the hub goes
//! `nodefight`, the defender's and attackers' managers are told
//! (`SovCommandNodeEventStarted`, 161), the structure rows are pushed and
//! [`on_begin`] is called.
//!
//! # The seam D3b fills
//!
//! - [`on_begin`]: **spawn the initial command nodes** (R-SOV §6.5). It does
//!   nothing in D3a.
//! - [`resolve`]: the end of the node fight, with an [`Outcome`]. D3a
//!   implements both outcomes plainly (attackers: the hub is destroyed and
//!   the claim lapses; defenders: the campaign clears and the hub is
//!   `claimed` again) so a campaign always ends (the GM's `/sov resolve`
//!   calls it); D3b calls it when a side reaches 1.0 and may extend it.
//! - [`Campaign::scores`] is the number the dashboard bar draws; D3b moves
//!   it by [`NODE_STEP`] a node and pushes
//!   `OnSolarSystemSovStructuresUpdated(system, rows, {hub: [0]})`
//!   (`entosis::STRUCTURE_SCORE_UPDATED`).
//! - [`Campaign::rest`]: D3b's own fields (node ids, spawn clock).

use crate::damage::BlueTime;
use crate::delta::Deltas;
use crate::game_clock::GameClock;
use crate::notifications::{self as notif, Value, data};
use crate::service::Registry;
use crate::sov::{self, Claim, HubState};
use crate::sov_hub;
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "sovcamp";

/// **[V2]** how long a reinforced hub waits: 48 hours, a game span.
/// R-SOV §2, EvEJS; the client has no constant. Falsified by an owner who
/// wants another delay.
pub const REINFORCE_DELAY: i64 = 48 * crate::seed::HOUR;

/// **[V2]** the defender's share of the campaign bar at the start, and the
/// step a captured node moves it (for D3b). Not in the client.
pub const DEFENDER_START_SCORE: f64 = 0.6;
pub const NODE_STEP: f64 = 0.07;

/// One campaign: the handoff between D3a (reinforcement) and D3b (the node
/// fight).
#[derive(Debug, Clone, PartialEq)]
pub struct Campaign {
    pub system_id: i64,
    pub hub_id: i64,
    /// `entosisConst.EVENT_TYPE_IHUB_DEFENSE` for a hub.
    pub event_type: i64,
    /// The alliance defending.
    pub defender: i64,
    /// The alliances that took the hub to 1.0.
    pub attackers: Vec<i64>,
    /// FILETIME the campaign starts (nodes appear).
    pub start_time: BlueTime,
    /// Whether [`begin`] has run.
    pub started: bool,
    pub created: BlueTime,
    /// `(team, score)` as the client draws them: positive is an alliance,
    /// `-A` everyone but alliance `A`.
    pub scores: Vec<(i64, f64)>,
    /// D3b's fields, verbatim.
    pub rest: Vec<String>,
}

impl Campaign {
    /// `campaignState` for a structure row: `(eventType, defenderAllianceID,
    /// startTime, {team: score})` **[V]**.
    pub fn state_value(&self) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(self.event_type),
            PyValue::Int(self.defender),
            PyValue::Int(self.start_time),
            PyValue::Dict(
                self.scores
                    .iter()
                    .map(|(team, score)| (PyValue::Real(*score), PyValue::Int(*team)))
                    .collect(),
            ),
        ])
    }
}

fn join(v: &[i64]) -> String {
    if v.is_empty() {
        "-".to_owned()
    } else {
        v.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
    }
}

fn parse_list(s: &str) -> Option<Vec<i64>> {
    if s == "-" {
        return Some(Vec::new());
    }
    s.split(',').map(|x| x.parse().ok()).collect()
}

fn parse_scores(s: &str) -> Option<Vec<(i64, f64)>> {
    if s == "-" {
        return Some(Vec::new());
    }
    s.split(',')
        .map(|p| {
            let (t, v) = p.split_once(':')?;
            Some((t.parse().ok()?, v.parse().ok()?))
        })
        .collect()
}

fn parse_row(key: &[String], v: &[String]) -> Option<Campaign> {
    Some(Campaign {
        system_id: key.first()?.parse().ok()?,
        hub_id: v.first()?.parse().ok()?,
        event_type: v.get(1)?.parse().ok()?,
        defender: v.get(2)?.parse().ok()?,
        start_time: v.get(3)?.parse().ok()?,
        started: v.get(4)? == "1",
        created: v.get(5)?.parse().ok()?,
        attackers: parse_list(v.get(6)?)?,
        scores: parse_scores(v.get(7)?)?,
        rest: v.get(8..).map(<[String]>::to_vec).unwrap_or_default(),
    })
}

/// `system_id`'s campaign, if one is scheduled or running. A row that does
/// not parse is logged and read as absent.
pub fn campaign(d: &Deltas, system_id: i64) -> Option<Campaign> {
    let key = vec![system_id.to_string()];
    let v = d.of(KIND).get(&key)?;
    let k = parse_row(&key, v);
    if k.is_none() {
        crate::errln!("[sov_campaign] skipping row {key:?}: {v:?}");
    }
    k
}

/// Every campaign, in system order.
pub fn all(d: &Deltas) -> Vec<Campaign> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| parse_row(k, v))
        .collect()
}

/// Write one campaign.
pub fn put(d: &mut Deltas, k: &Campaign) {
    let scores = if k.scores.is_empty() {
        "-".to_owned()
    } else {
        k.scores
            .iter()
            .map(|(t, s)| format!("{t}:{s}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut value = vec![
        k.hub_id.to_string(),
        k.event_type.to_string(),
        k.defender.to_string(),
        k.start_time.to_string(),
        if k.started { "1" } else { "0" }.to_owned(),
        k.created.to_string(),
        join(&k.attackers),
        scores,
    ];
    value.extend(k.rest.iter().cloned());
    d.put(KIND, vec![k.system_id.to_string()], value);
}

/// Remove a system's campaign.
pub fn drop_campaign(d: &mut Deltas, system_id: i64) {
    d.del(KIND, &[system_id.to_string()]);
}

/// The campaign to schedule for `c` after an attack by `attackers`
/// completed at `now`: it starts at the first prime-window start at least
/// [`REINFORCE_DELAY`] away (compressed by `clock`), or at `now` when
/// `immediately` (the GM line). The scores start at `{defender: 0.6,
/// -defender: 0.4}`.
pub fn schedule(
    d: &Deltas,
    c: &Claim,
    attackers: Vec<i64>,
    now: BlueTime,
    clock: GameClock,
    immediately: bool,
) -> Campaign {
    let defender = c.alliance_id.unwrap_or(0);
    let start_time = if immediately {
        now
    } else {
        let prime = sov::prime_hour(d, defender, now)
            .current
            .unwrap_or(sov::DEFAULT_PRIME_HOUR);
        let adm = crate::sov_index::system_adm(d, c.system_id, now, clock);
        crate::entosis::window_start_at_or_after(
            prime,
            crate::sov_index::vulnerability_window(adm),
            now + clock.span(REINFORCE_DELAY),
            clock,
        )
    };
    Campaign {
        system_id: c.system_id,
        hub_id: c.hub_id,
        event_type: crate::entosis::EVENT_TYPE_HUB,
        defender,
        attackers,
        start_time,
        started: false,
        created: now,
        scores: vec![
            (defender, DEFENDER_START_SCORE),
            (-defender, 1.0 - DEFENDER_START_SCORE),
        ],
        rest: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Starting
// ---------------------------------------------------------------------------

/// **The stage's half** (called by `entosis::tick`, so no stage of its own):
/// begin every campaign whose time came.
pub fn tick(reg: &mut Registry, now: BlueTime) {
    for k in all(reg.deltas()) {
        if !k.started && now >= k.start_time {
            begin(reg, &k);
        }
    }
    // SOC-D3b: the fight itself -- nodes due, scores, the result.
    crate::command_nodes::tick(reg, now);
}

/// Start a campaign: the hub goes to `nodefight`, the structure rows are
/// pushed, the two sides' managers are told, and [`on_begin`] spawns what
/// D3b spawns.
pub fn begin(reg: &mut Registry, k: &Campaign) {
    let Some(c) = sov::claim(reg.deltas(), k.system_id).filter(|c| c.hub_id == k.hub_id) else {
        // The hub changed hands or went away: the campaign is stale.
        drop_campaign(reg.deltas_mut(), k.system_id);
        return;
    };
    let started = Campaign {
        started: true,
        ..k.clone()
    };
    put(reg.deltas_mut(), &started);
    sov::put(
        reg.deltas_mut(),
        &Claim {
            state: HubState::NodeFight,
            state_until: None,
            ..c.clone()
        },
    );
    let mut alliances = vec![k.defender];
    alliances.extend(k.attackers.iter().copied());
    sov::push_structures(
        reg,
        k.system_id,
        &alliances,
        Some(crate::entosis::changed(k.hub_id)),
    );
    let constellation = reg
        .content()
        .try_get::<crate::celestial::Universe>()
        .and_then(|u| u.system(k.system_id))
        .map_or(0, |s| s.constellation_id);
    let mut to: Vec<i64> = alliances
        .iter()
        .flat_map(|a| crate::entosis::alliance_managers(reg, *a, None))
        .collect();
    to.sort_unstable();
    to.dedup();
    for who in to {
        notif::notify(
            reg,
            who,
            crate::entosis::kind::COMMAND_NODE_EVENT_STARTED,
            data([
                ("campaignEventType", Value::Int(k.event_type)),
                ("solarSystemID", Value::Int(k.system_id)),
                ("constellationID", Value::Int(constellation)),
            ]),
        );
    }
    crate::errln!(
        "[sov] campaign in {} begins on hub {}",
        k.system_id,
        k.hub_id
    );
    on_begin(reg, &started);
}

/// **D3b's seam: the campaign has started.** Spawn the initial command
/// nodes (R-SOV §6.5: four, spread over the system and its constellation,
/// within the caps), store their ids in [`Campaign::rest`], and keep the
/// "one more every half hour" clock. D3a leaves it empty: a campaign with
/// no nodes sits in `nodefight` until [`resolve`] (the GM's
/// `/sov resolve`).
pub fn on_begin(reg: &mut Registry, campaign: &Campaign) {
    crate::command_nodes::on_begin(reg, campaign);
}

// ---------------------------------------------------------------------------
// Ending
// ---------------------------------------------------------------------------

/// How a campaign ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The attackers took the nodes: the hub is destroyed, the claim lapses
    /// and the system is open for a new hub.
    AttackersWin,
    /// The defenders held: the campaign clears and the hub is `claimed`
    /// again, with a fresh window.
    DefendersWin,
}

/// **End a campaign** (D3b's node fight calls this when a side reaches 1.0;
/// the GM's `/sov resolve` calls it too). The managers of both sides hear:
/// `SovStructureDestroyed` (162) and `SovAllClaimLostMsg` (41) to the
/// defender when the hub falls.
pub fn resolve(reg: &mut Registry, system: i64, outcome: Outcome) -> Result<(), String> {
    let k =
        campaign(reg.deltas(), system).ok_or_else(|| format!("system {system} has no campaign"))?;
    let c = sov::claim(reg.deltas(), system)
        .filter(|c| c.hub_id == k.hub_id)
        .ok_or_else(|| format!("system {system} has no hub row"))?;
    let now = crate::pyutil::filetime_now();
    drop_campaign(reg.deltas_mut(), system);
    // SOC-D3b: the nodes go, and the attackers hear a fallen hub too.
    crate::command_nodes::on_resolve(reg, &k, outcome);
    match outcome {
        Outcome::AttackersWin => {
            let h = sov_hub::hub(reg.deltas(), system)
                .ok_or_else(|| format!("system {system} has no launched Sovereignty Hub"))?;
            let tell: Vec<i64> = crate::entosis::alliance_managers(reg, k.defender, None);
            sov_hub::lapse(reg, &h, now);
            for who in tell {
                notif::notify(
                    reg,
                    who,
                    crate::entosis::kind::STRUCTURE_DESTROYED,
                    data([
                        ("structureTypeID", Value::Int(sov::TYPE_INFRASTRUCTURE_HUB)),
                        ("solarSystemID", Value::Int(system)),
                    ]),
                );
                notif::notify_from(
                    reg,
                    who,
                    crate::entosis::kind::ALLIANCE_CLAIM_LOST,
                    c.corporation_id,
                    data([
                        ("corpID", Value::Int(c.corporation_id)),
                        ("allianceID", Value::Int(k.defender)),
                        ("solarSystemID", Value::Int(system)),
                    ]),
                );
            }
        }
        Outcome::DefendersWin => {
            sov::put(
                reg.deltas_mut(),
                &Claim {
                    state: HubState::Claimed,
                    state_until: None,
                    ..c
                },
            );
            crate::entosis::put_score(
                reg.deltas_mut(),
                k.hub_id,
                system,
                &crate::entosis::Score::idle(now),
            );
            sov_hub::claim_changed(reg, system);
            let mut alliances = vec![k.defender];
            alliances.extend(k.attackers.iter().copied());
            sov::push_structures(
                reg,
                system,
                &alliances,
                Some(crate::entosis::changed(k.hub_id)),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
