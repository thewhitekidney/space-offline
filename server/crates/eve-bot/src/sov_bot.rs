//! **Bots hold sovereignty** (SOC-D5, `docs/lanes/sov-d5.md`): an alliance's
//! bot pilots read the sov map the client reads, pick one system to claim,
//! attack or defend, and run it with the player's own tools -- an Entosis
//! Link on a hub or a command node, fuel blocks carried to the hub, a guard
//! on the grid.
//!
//! **Nothing here is a server back door.** Every fact is a call the client
//! makes (`stationSvc.GetAllianceSystems`, `sovMgr.GetSovStructuresInfoForSolarSystem`,
//! `allianceRegistry.GetAllianceSovereigntyStructuresInfo`, `GetPrimeTimeInfo`,
//! `SetPrimeHour`, `devIndexManager`'s rows); every act is a verb a player
//! has (`dogmaIM.AddTarget` and `Activate` on the link, `CmdWarpToStuff`,
//! the gates, the market, `/boostsov`, which the client itself sends to
//! `publicQaToolsServer.SlashCmd` for anybody). No GM line is ever sent, and
//! the role's setup gives nothing (**nothing from nowhere**): a pilot with no
//! link in its hangar buys one at the station it is docked in, or says
//! `MISSING`.
//!
//! ## The pieces
//!
//! - **The view** ([`HubView`], [`Phase`]): one row of the client's structure
//!   list read at `now`. `vulnerabilityState` is the window `(start, end)`
//!   (the current one, or the next while the hub is shut) and
//!   `campaignState` is `(type, defender, start, scores)`; both are blue
//!   times on the **real** clock (a compressed game day shortens the period,
//!   never the anchor), so the bot needs no clock scale.
//! - **The planner** ([`plan`]): the alliance's one campaign. Defend first
//!   (an own hub in a node fight or reinforced), then the best of claiming a
//!   neutral hub and attacking a hub whose holder the society's relation `R`
//!   calls an enemy ([`ATTACK_R`]), by value, distance and the force at hand.
//!   A campaign is kept while it is valid ([`STICK`]).
//! - **Prime-time scheduling** ([`due`], [`prime_hour_for`]): attackers set
//!   out [`LEAD_SECS`] before a window opens and not before; the alliance
//!   staggers its own prime hour by a hash of its id so two alliances rarely
//!   share one.
//! - **The hall** ([`SovHall`]): one per `eve-bots` process. Pilots report
//!   their kit; one of them plans when the plan is stale; [`assign`] gives
//!   each a [`Duty`]: entosis, guard or haul.
//! - **The turn** ([`turn`]): one blocking sortie over the connection, as
//!   [`crate::capital::turn`] is. The role's rules in [`ROLE`] call it.
//!
//! ## The claimer is inert (D-HUB)
//!
//! A hub is 60,000 m3 and no bot can get one honestly yet (a freighter rung
//! or a hub seller is D-HUB's, not done). The planner therefore never
//! targets a system with no hub, and no code here launches one. When a bot
//! can own a hub, the launch (`ship.Drop`) is the one verb to add.
//!
//! Every number is **[A]** policy except the client's and the server's
//! constants, which carry their sources.

use crate::brain::{self, Act, Memory, Observation};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::travel::{self, Map};
use crate::verbs::ItemRow;
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::command_nodes::TYPE_COMMAND_NODE;
use eve_server::entosis::{
    ATTR_FUEL_QUANTITY, ATTR_FUEL_TYPE, ATTR_MAX_RANGE, EFFECT_ENTOSIS_LINK,
};
use eve_server::sov::TYPE_INFRASTRUCTURE_HUB;
use eve_server::sov_hub::{FUEL_PER_DAY, GROUP_FUEL_BLOCK, TRANSFER_RANGE};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

// ---- policy [A] ---------------------------------------------------------------

/// Real seconds before a window opens that attackers set out **[A]**: the
/// flight and the lock are minutes. Falsified by links that arrive after the
/// window shut (too short) or sit an hour idle at the hub (too long).
pub const LEAD_SECS: i64 = 300;
/// A relation `R` at or below this makes a holder an enemy **[A]** (C2's
/// scale is -1..1, a red contact about -0.5). Falsified by bots that never
/// attack (too low) or that attack blues (too high).
pub const ATTACK_R: f64 = -0.25;
/// How far from its home system a pilot looks for hubs, jumps **[A]**.
pub const VIEW_JUMPS: u32 = 4;
/// The most systems one view asks about, nearest first **[A]**: one call each.
pub const VIEW_MAX: usize = 24;
/// The least security a route of this role passes **[A]**: null sec, the
/// whole point of the role. The server's own rule decides what is claimable.
pub const SOV_FLOOR: f64 = -1.0;
/// A system at or below this is null sec for the view **[A]**; the server
/// re-checks with the band (`sov::claimable_system`).
pub const NULL_AT_MOST: f64 = 0.0;
/// The plan is read again this often **[A]**.
pub const PLAN_EVERY: Duration = Duration::from_secs(120);
/// A pilot's report to the hall is dropped after this long **[A]**.
pub const ROSTER_TTL: Duration = Duration::from_secs(1_800);
/// How often a pilot takes a sov turn **[A]**.
pub const TURN_EVERY: (Duration, Duration) = (Duration::from_secs(15), Duration::from_secs(40));
/// A sortie ends after this long **[A]**: a capture is 595 s over the clock
/// scale, plus the flight, plus waiting for a window.
pub const SORTIE_LIMIT: Duration = Duration::from_secs(40 * 60);
/// How long a link waits for its window, lock and range before the sortie
/// gives up on the target **[A]**.
pub const LINK_WAIT: Duration = Duration::from_secs(10 * 60);
/// A link is retried this often while it cannot start **[A]**.
pub const LINK_RETRY: Duration = Duration::from_secs(2);
/// A running link is looked at this often **[A]**.
pub const LINK_POLL: Duration = Duration::from_secs(3);
/// The share of a link's range a pilot warps in at **[A]**.
pub const RANGE_USE: f64 = 0.5;
/// The range a guard lands at, m **[A]**.
pub const GUARD_RANGE: f64 = 15_000.0;
/// The most links, and the most guards, sent on one campaign **[A]**.
pub const LINKERS_MAX: usize = 6;
pub const GUARDS_MAX: usize = 4;
/// Links an attack wants per point of the target's ADM, rounded up **[A]**.
pub const LINKERS_PER_ADM: f64 = 1.0;
/// Cycles of strontium a link carries beyond one capture **[A]**.
pub const FUEL_MARGIN: f64 = 0.25;
/// REVIEW-FIX2: **the fuel a haul to one hub carries** (pure): the demand
/// layer's [`crate::demand::FE_FUEL_DAYS`] of [`FUEL_PER_DAY`], less `held`.
pub fn haul_units(held: i64) -> i64 {
    crate::demand::fuel_units(1, FUEL_PER_DAY, held)
}
/// The share of the wallet a purchase may take **[A]**.
pub const WALLET_SHARE: f64 = 0.5;
/// A hauler re-checks the fuel of an own hub this often **[A]**: the hub
/// burns [`FUEL_PER_DAY`] a game day, and bots cannot read the bay.
pub const REFUEL_EVERY: Duration = Duration::from_secs(6 * 3_600);
/// No shopping before this again after a pass found nothing **[A]**.
pub const SHOP_EVERY: Duration = Duration::from_secs(20 * 60);
/// Plan values **[A]**: a neutral hub is worth this, an attack this plus the
/// target's ADM times [`ADM_VALUE`], a defence far more than either.
pub const CLAIM_VALUE: f64 = 10.0;
pub const ATTACK_VALUE: f64 = 8.0;
pub const ADM_VALUE: f64 = 3.0;
pub const DEFEND_VALUE: f64 = 100.0;
pub const NODE_FIGHT_BONUS: f64 = 50.0;
/// R-REL-5 **[A]**: an enemy the whole coalition named is worth this more to
/// attack; a mate's hub in its node fight is worth this, guards only. Falsified
/// by blocs whose members attack in ones and twos (too low) or never help a
/// mate (too high a bar).
pub const BLOC_ENEMY_VALUE: f64 = 6.0;
pub const AID_VALUE: f64 = 40.0;
/// The cost of a jump in plan value, and the weight of a worse `R` **[A]**.
pub const JUMP_COST: f64 = 1.5;
pub const R_WEIGHT: f64 = 4.0;
/// The bonus of the campaign already running **[A]**.
pub const STICK: f64 = 5.0;
/// Jumps from the hub a node tour reaches **[A]**: nodes fall in the hub's
/// constellation, which the gates approximate.
pub const NODE_TOUR_JUMPS: u32 = 3;
/// `blue` ticks per second.
pub const TICKS: i64 = 10_000_000;

/// The message a pilot with no hall says.
pub const MISSING_HALL: &str = "eve-bots with a sov hall (a bot of this role is given one)";
/// What an entosis pilot lacks without the link: bought, never given.
pub const MISSING_LINK: &str = "an Entosis Link it owns or can buy (never given)";

// ---- the view -----------------------------------------------------------------

/// Where a hub stands, as its structure row says at `now`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// No holder: anyone may link it.
    Neutral,
    /// Inside its vulnerability window, which ends at `closes`.
    Open { closes: i64 },
    /// Outside the window; the next one opens at `opens`.
    Shut { opens: i64 },
    /// Reinforced; the node fight starts at `fight_at`.
    Reinforced { fight_at: i64 },
    /// The command nodes are up.
    NodeFight,
    /// A held hub whose row shows no window.
    Unknown,
}

/// One hub, as a structure row shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct HubView {
    pub system: i64,
    pub hub_id: i64,
    /// The holding alliance; `None` for a neutral hub.
    pub holder: Option<i64>,
    /// The defense multiplier (the ADM), 1.0 when the row has none.
    pub adm: f64,
    pub phase: Phase,
    /// R-REL-2: the campaign's start while the hub is reinforced or in its
    /// node fight.
    pub campaign: Option<i64>,
    /// R-REL-2: the alliances that have scored against the holder
    /// (`campaignState`'s `{team: score}`, a team above 0 is an alliance,
    /// below 0 "everyone but"). **[A]**: a positive score means a link ran
    /// on the hub; falsified by a sov row whose scores name a team that
    /// never linked.
    pub contesters: Vec<i64>,
}

/// The phase of a hub from its row's fields at `now` (pure).
pub fn phase_of(
    holder: Option<i64>,
    campaign_start: Option<i64>,
    window: Option<(i64, i64)>,
    now: i64,
) -> Phase {
    if holder.is_none() {
        return Phase::Neutral;
    }
    if let Some(start) = campaign_start {
        return if now >= start {
            Phase::NodeFight
        } else {
            Phase::Reinforced { fight_at: start }
        };
    }
    match window {
        Some((start, _)) if now < start => Phase::Shut { opens: start },
        Some((_, end)) => Phase::Open {
            closes: end.max(now),
        },
        None => Phase::Unknown,
    }
}

fn kv<'a>(row: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let PyValue::Dict(pairs) = crate::attributes(row) else {
        return None;
    };
    pairs
        .iter()
        .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == name.as_bytes()))
        .map(|(v, _)| v)
}

fn int_of(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

impl HubView {
    /// One `GetSovStructuresInfoForSolarSystem` row (pure). A row with no
    /// hub id is no hub.
    pub fn from_row(system: i64, row: &PyValue, now: i64) -> Option<HubView> {
        let hub_id = int_of(kv(row, "itemID"))?;
        if int_of(kv(row, "typeID")).is_some_and(|t| t != TYPE_INFRASTRUCTURE_HUB) {
            return None;
        }
        let holder = int_of(kv(row, "allianceID"));
        let adm = match kv(row, "defenseMultiplier") {
            Some(PyValue::Real(r)) => *r,
            Some(PyValue::Int(n)) => *n as f64,
            _ => 1.0,
        };
        let campaign_start = match kv(row, "campaignState") {
            Some(PyValue::Tuple(t)) => int_of(t.get(2)),
            _ => None,
        };
        let contesters = match kv(row, "campaignState") {
            Some(PyValue::Tuple(t)) => match t.get(3) {
                Some(PyValue::Dict(scores)) => scores
                    .iter()
                    .filter_map(|(score, team)| match (score, team) {
                        (PyValue::Real(r), PyValue::Int(t))
                            if *t > 0 && *r > 0.0 && Some(*t) != holder =>
                        {
                            Some(*t)
                        }
                        (PyValue::Int(r), PyValue::Int(t))
                            if *t > 0 && *r > 0 && Some(*t) != holder =>
                        {
                            Some(*t)
                        }
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let window = match kv(row, "vulnerabilityState") {
            Some(PyValue::Tuple(t)) => int_of(t.first()).zip(int_of(t.get(1))),
            _ => None,
        };
        Some(HubView {
            system,
            hub_id,
            holder,
            adm,
            phase: phase_of(holder, campaign_start, window, now),
            campaign: campaign_start,
            contesters,
        })
    }
}

// ---- the planner --------------------------------------------------------------

/// What a campaign is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Take a neutral hub by entosis.
    Claim,
    /// Take a hub from an enemy: the window, then the node fight.
    Attack,
    /// Hold an own hub through a reinforcement and a node fight.
    Defend,
    /// R-REL-5: stand guard at a coalition mate's hub in its node fight
    /// (guards only: no link is ever put on a mate's hub).
    Aid,
}

/// The alliance's campaign.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub system: i64,
    pub hub_id: i64,
    pub kind: Kind,
    pub holder: Option<i64>,
    pub phase: Phase,
    pub score: f64,
}

/// What the planner is shown (pure inputs).
pub struct Facts<'a> {
    /// The planning alliance.
    pub me: i64,
    /// Every hub in view.
    pub hubs: &'a [HubView],
    /// Gate jumps from the alliance's staging system, when known.
    pub jumps: &'a dyn Fn(i64) -> Option<u32>,
    /// The society's relation `R` of `me` toward a holder (negative: enemy).
    /// R-REL-5: through the coalition (`A_eff + D_KA K`).
    pub relation: &'a dyn Fn(i64) -> f64,
    /// R-REL-5: has the holder been named an enemy by `me`'s bloc (the lag
    /// passed)?
    pub bloc_enemy: &'a dyn Fn(i64) -> bool,
    /// R-REL-5: is the holder a member of `me`'s bloc?
    pub bloc_mate: &'a dyn Fn(i64) -> bool,
    pub now: i64,
    /// Pilots with an Entosis Link, reported to the hall.
    pub linkers: usize,
    /// The campaign running now, which is kept while it is valid.
    pub previous: Option<&'a Target>,
}

/// Links an attack on a hub of `adm` wants (pure).
pub fn linkers_wanted(adm: f64) -> usize {
    ((adm.max(1.0) * LINKERS_PER_ADM).ceil() as usize).clamp(1, LINKERS_MAX)
}

/// Whether the phase can be fought for within the lead time (pure): the
/// scheduling rule. A neutral hub and an open window can be fought now; a
/// shut window or a reinforcement can be waited for at the hub from
/// [`LEAD_SECS`] before it opens.
pub fn due(phase: Phase, now: i64) -> bool {
    let lead = LEAD_SECS * TICKS;
    match phase {
        Phase::Neutral | Phase::NodeFight => true,
        Phase::Open { .. } => true,
        Phase::Shut { opens } => opens - now <= lead,
        Phase::Reinforced { fight_at } => fight_at - now <= lead,
        Phase::Unknown => false,
    }
}

/// **The campaign** (pure): `None` when nothing is worth a sortie.
///
/// 1. An own hub in a node fight or reinforced is defended, the fight first.
/// 2. Else the best of a neutral hub to claim and an enemy hub to attack,
///    each only when [`due`] and only with the force at hand
///    ([`linkers_wanted`] for an attack, one link for a claim). A held hub
///    whose holder is not an enemy is left alone.
/// 3. The campaign running now keeps [`STICK`] extra while it is valid.
///
/// A system with no hub is never a target: nobody can launch one yet.
pub fn plan(f: &Facts) -> Option<Target> {
    let mut best: Option<Target> = None;
    for h in f.hubs {
        let jumps = (f.jumps)(h.system).unwrap_or(VIEW_JUMPS + 1);
        let cost = f64::from(jumps) * JUMP_COST;
        let candidate = match (h.holder, h.phase) {
            (Some(a), Phase::NodeFight) if a == f.me => {
                Some((Kind::Defend, DEFEND_VALUE + NODE_FIGHT_BONUS - cost))
            }
            (Some(a), Phase::Reinforced { .. }) if a == f.me && due(h.phase, f.now) => {
                Some((Kind::Defend, DEFEND_VALUE - cost))
            }
            (Some(a), _) if a == f.me => None,
            (Some(a), Phase::NodeFight) if (f.bloc_mate)(a) => Some((Kind::Aid, AID_VALUE - cost)),
            (Some(a), _) if (f.bloc_mate)(a) => None,
            (None, Phase::Neutral) if f.linkers >= 1 => Some((Kind::Claim, CLAIM_VALUE - cost)),
            (Some(a), p)
                if ((f.relation)(a) <= ATTACK_R || (f.bloc_enemy)(a))
                    && due(p, f.now)
                    && !matches!(p, Phase::Unknown | Phase::Reinforced { .. })
                    && f.linkers >= linkers_wanted(h.adm) =>
            {
                Some((
                    Kind::Attack,
                    ATTACK_VALUE + h.adm * ADM_VALUE - cost - (f.relation)(a) * R_WEIGHT
                        + if (f.bloc_enemy)(a) {
                            BLOC_ENEMY_VALUE
                        } else {
                            0.0
                        },
                ))
            }
            _ => None,
        };
        let Some((kind, mut score)) = candidate else {
            continue;
        };
        if f.previous
            .is_some_and(|p| p.hub_id == h.hub_id && p.kind == kind)
        {
            score += STICK;
        }
        if best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(Target {
                system: h.system,
                hub_id: h.hub_id,
                kind,
                holder: h.holder,
                phase: h.phase,
                score,
            });
        }
    }
    best.filter(|t| t.score > 0.0)
}

/// The own hubs that need fuel on the next run (pure): every claimed hub of
/// `me`.
pub fn fuel_runs(me: i64, hubs: &[HubView]) -> Vec<(i64, i64)> {
    hubs.iter()
        .filter(|h| h.holder == Some(me))
        .map(|h| (h.system, h.hub_id))
        .collect()
}

/// **An alliance's own prime hour** (pure): a hash of its id, so two
/// alliances rarely share a window **[A]**. Falsified by bot alliances that
/// are beaten every time in the same hour.
pub fn prime_hour_for(alliance: i64) -> i64 {
    // Knuth's multiplicative hash, high bits.
    let h = (alliance as u64).wrapping_mul(2_654_435_761) >> 8;
    (h % 24) as i64
}

// ---- the hall: what the alliance's pilots share ---------------------------------------

/// What a pilot has to offer a campaign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Kit {
    /// A fitted or owned Entosis Link.
    pub link: bool,
    /// Guns fitted.
    pub guns: bool,
    /// Neither: a ship that can carry fuel.
    pub hauler: bool,
}

/// What a pilot does for the campaign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Duty {
    #[default]
    Idle,
    /// Link the target's hub or nodes.
    Entosis,
    /// Hold a post at the target's hub.
    Guard,
    /// Carry fuel blocks to the own hub with this id.
    Haul(i64),
}

/// The alliance's plan, as the hall keeps it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub target: Option<Target>,
    /// `(system, hub)` of the own hubs to keep fuelled.
    pub fuel: Vec<(i64, i64)>,
}

/// **Who does what** (pure): linkers on the target, guards beside them, and
/// the rest of the haulers over the own hubs, round-robin by character id.
/// Without a target there are no linkers or guards; without hubs no haulers.
pub fn assign(plan: &Plan, roster: &BTreeMap<i64, Kit>) -> BTreeMap<i64, Duty> {
    let mut out = BTreeMap::new();
    let mut linkers = 0;
    let mut guards = 0;
    let mut hauls = 0;
    for (&who, kit) in roster {
        let duty = match &plan.target {
            // An aid puts no link on a mate's hub: guards only.
            Some(t) if kit.link && linkers < LINKERS_MAX && t.kind != Kind::Aid => {
                // A claim needs one link; an attack the ADM's worth.
                let want = match t.kind {
                    Kind::Claim => 1,
                    _ => LINKERS_MAX,
                };
                if linkers < want {
                    linkers += 1;
                    Duty::Entosis
                } else {
                    Duty::Idle
                }
            }
            Some(_) if kit.guns && !kit.link && guards < GUARDS_MAX => {
                guards += 1;
                Duty::Guard
            }
            _ if kit.hauler && !kit.link && !plan.fuel.is_empty() => {
                let (_, hub) = plan.fuel[hauls % plan.fuel.len()];
                hauls += 1;
                Duty::Haul(hub)
            }
            _ => Duty::Idle,
        };
        out.insert(who, duty);
    }
    out
}

#[derive(Debug, Default)]
struct Book {
    roster: BTreeMap<i64, (Kit, Instant)>,
    plan: Plan,
    planned_at: Option<Instant>,
    /// Characters that already tried the prime hour, and when.
    prime_tried: BTreeMap<i64, Instant>,
    log: Vec<String>,
}

/// **The hall**: every alliance's roster and plan, shared by the process.
#[derive(Debug, Default)]
pub struct SovHall {
    inner: Mutex<BTreeMap<i64, Book>>,
}

impl SovHall {
    pub fn new() -> SovHall {
        SovHall::default()
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<i64, Book>> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A pilot reports its kit.
    pub fn report(&self, alliance: i64, who: i64, kit: Kit, now: Instant) {
        let mut g = self.lock();
        let b = g.entry(alliance).or_default();
        b.roster.insert(who, (kit, now));
        b.roster
            .retain(|_, (_, seen)| now.saturating_duration_since(*seen) <= ROSTER_TTL);
    }

    /// Pilots with a link on the roster.
    pub fn linkers(&self, alliance: i64) -> usize {
        self.lock()
            .get(&alliance)
            .map_or(0, |b| b.roster.values().filter(|(k, _)| k.link).count())
    }

    /// Whether the plan is stale; the caller that gets `true` is the planner
    /// for this round (the stamp is taken here, so two pilots never plan
    /// together).
    pub fn take_planning(&self, alliance: i64, now: Instant) -> bool {
        let mut g = self.lock();
        let b = g.entry(alliance).or_default();
        let stale = b
            .planned_at
            .is_none_or(|t| now.saturating_duration_since(t) >= PLAN_EVERY);
        if stale {
            b.planned_at = Some(now);
        }
        stale
    }

    /// The planner's answer.
    pub fn set_plan(&self, alliance: i64, plan: Plan) {
        let mut g = self.lock();
        let b = g.entry(alliance).or_default();
        if b.plan.target.as_ref().map(|t| (t.hub_id, t.kind))
            != plan.target.as_ref().map(|t| (t.hub_id, t.kind))
        {
            let line = match &plan.target {
                Some(t) => format!(
                    "alliance {alliance}: {:?} hub {} in system {} ({:?})",
                    t.kind, t.hub_id, t.system, t.phase
                ),
                None => format!("alliance {alliance}: no campaign"),
            };
            b.log.push(line);
        }
        b.plan = plan;
    }

    /// The plan now.
    pub fn plan_of(&self, alliance: i64) -> Plan {
        self.lock()
            .get(&alliance)
            .map(|b| b.plan.clone())
            .unwrap_or_default()
    }

    /// `who`'s duty under the plan now.
    pub fn duty(&self, alliance: i64, who: i64) -> Duty {
        let g = self.lock();
        let Some(b) = g.get(&alliance) else {
            return Duty::Idle;
        };
        let roster: BTreeMap<i64, Kit> = b.roster.iter().map(|(k, (kit, _))| (*k, *kit)).collect();
        assign(&b.plan, &roster)
            .get(&who)
            .copied()
            .unwrap_or_default()
    }

    /// Whether `who` may try the prime hour now (once per [`REFUEL_EVERY`]).
    pub fn take_prime(&self, alliance: i64, who: i64, now: Instant) -> bool {
        let mut g = self.lock();
        let b = g.entry(alliance).or_default();
        let ok = b
            .prime_tried
            .get(&who)
            .is_none_or(|t| now.saturating_duration_since(*t) >= REFUEL_EVERY);
        if ok {
            b.prime_tried.insert(who, now);
        }
        ok
    }

    /// The campaign lines so far (for `[sov]` and tests).
    pub fn log(&self, alliance: i64) -> Vec<String> {
        self.lock()
            .get(&alliance)
            .map(|b| b.log.clone())
            .unwrap_or_default()
    }
}

// ---- what a bot remembers --------------------------------------------------------------

/// What a sov pilot remembers across turns (the driver keeps it in
/// [`Memory::sov`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    /// Where the pilot started: its staging system.
    pub home: Option<i64>,
    pub home_station: Option<i64>,
    /// A guard's post: `(system, hub)`.
    pub post: Option<(i64, i64)>,
    pub duty: Duty,
    pub next_turn: Option<Instant>,
    pub shop_next: Option<Instant>,
    /// The last hub a hauler fuelled, and when.
    pub fuelled: BTreeMap<i64, Instant>,
    pub sorties: u32,
}

/// What a turn is given.
pub struct Ctx<'a> {
    pub map: Option<&'a Map>,
    pub content: Option<&'a eve_server::content::Content>,
    pub hall: &'a SovHall,
    /// The society's relation `R` of `(me, holder)`; `None`: all neutral.
    pub relation: Option<&'a dyn Fn(i64, i64) -> f64>,
    /// R-REL-2: the society's hall, which hears the sieges and contests the
    /// defender sees on its own hubs.
    pub society: Option<&'a crate::corpmind::Hall>,
}

/// How much one planning pass that targets a holder's hub adds to the
/// contest between the two alliances, in game-days **[A]**. Falsified by
/// `[rel]` showing two alliances contesting for weeks without a red stance.
pub const CONTEST_STEP_DAYS: f64 = 0.1;

/// **The sieges a defender sees** (R-REL-2, F9; pure): for every hub `me`
/// holds that is reinforced or in its node fight, each alliance that has
/// scored against it, as `(hub, campaign start, attacker)`.
pub fn sieges(hubs: &[HubView], me: i64) -> Vec<(i64, i64, i64)> {
    hubs.iter()
        .filter(|h| h.holder == Some(me))
        .filter(|h| matches!(h.phase, Phase::Reinforced { .. } | Phase::NodeFight))
        .filter_map(|h| h.campaign.map(|c| (h, c)))
        .flat_map(|(h, c)| h.contesters.iter().map(move |a| (h.hub_id, c, *a)))
        .collect()
}

/// Tell the society what a planning pass saw: each siege on an own hub once
/// per campaign (F9, the defender's `Event::Siege`), and the contest with the
/// holder of the hub the plan attacks.
fn tell_society(hall: &crate::corpmind::Hall, me: i64, hubs: &[HubView], target: Option<&Target>) {
    use crate::society::Event;
    for (hub, start, attacker) in sieges(hubs, me) {
        if hall.lock().first(crate::observe::KIND_SIEGE, hub, start) {
            hall.record(&Event::Siege {
                defender: me,
                attacker,
            });
        }
    }
    if let Some(t) = target
        && t.kind == Kind::Attack
        && let Some(holder) = t.holder
    {
        hall.record(&Event::Contest {
            a: me,
            b: holder,
            days: CONTEST_STEP_DAYS,
        });
    }
}

// ---- the role -------------------------------------------------------------------------

/// The `--loop sov` row.
pub static ROLE: RoleSpec = RoleSpec {
    name: "sov",
    greeting: "o7 -- flying for the alliance's sov",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

/// **Nothing from nowhere**: no slash line.
fn setup(_: Lacks) -> Vec<String> {
    Vec::new()
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, reconnects {}",
        s.name, s.phase, s.reconnects
    )
}

/// One [`Act::Sov`] when its time has come, else a wait.
fn turn_act(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return Some(vec![Act::Reconnect]);
    }
    if m.policy.sov.is_none() {
        return Some(vec![
            Act::Missing(MISSING_HALL),
            Act::Wait(brain::MISSING_WAIT),
        ]);
    }
    if m.sov.next_turn.is_some_and(|t| o.now < t) {
        return None;
    }
    let next = o.now + m.jitter(TURN_EVERY);
    m.sov.next_turn = Some(next);
    Some(vec![Act::Sov, Act::Wait(next - o.now)])
}

fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if let Some(acts) = turn_act(m, o) {
        return acts;
    }
    // Nothing to do for the alliance: the escort's quiet life.
    if m.sov.duty == Duty::Idle {
        return crate::escort::docked(m, o);
    }
    vec![Act::Wait(brain::SHORT_WAIT)]
}

fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(park) = &o.park else {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    };
    if let Some(acts) = turn_act(m, o) {
        return acts;
    }
    if m.sov.post.is_some() {
        // A guard on its post fights whoever is hostile on the grid: the
        // hunter's rules with a doctrine, the escort's without.
        return if m.policy.pvp.is_some() {
            (crate::hunt::ROLE.in_space)(m, o)
        } else {
            crate::escort::in_space(m, o)
        };
    }
    if m.sov.duty == Duty::Idle {
        return crate::escort::in_space(m, o);
    }
    brain::go_home(m, o, park)
}

// ---- reading the sov map, as the client does ------------------------------------------

/// The holders of the whole map: `stationSvc.GetAllianceSystems` rows
/// `(solarSystemID, allianceID)`.
pub fn holders(c: &mut Conn) -> Result<BTreeMap<i64, i64>> {
    let out = c.call("stationSvc", "GetAllianceSystems", vec![])?;
    let mut map = BTreeMap::new();
    if let Some(PyValue::List(rows)) = out.value() {
        for r in &rows {
            if let (Some(s), Some(a)) =
                (int_of(kv(r, "solarSystemID")), int_of(kv(r, "allianceID")))
            {
                map.insert(s, a);
            }
        }
    }
    Ok(map)
}

/// The hubs in `system` as the structure list shows them.
pub fn hubs_in(c: &mut Conn, system: i64, now: i64) -> Result<Vec<HubView>> {
    let out = c.call(
        "sovMgr",
        "GetSovStructuresInfoForSolarSystem",
        vec![PyValue::Int(system)],
    )?;
    Ok(match out.value() {
        Some(PyValue::List(rows)) => rows
            .iter()
            .filter_map(|r| HubView::from_row(system, r, now))
            .collect(),
        _ => Vec::new(),
    })
}

/// Systems to ask about: `home`, the held systems in range, then the null
/// systems within [`VIEW_JUMPS`] by distance, at most [`VIEW_MAX`] (pure).
pub fn candidates(map: &Map, home: i64, held: &BTreeMap<i64, i64>) -> Vec<(i64, u32)> {
    let mut seen: BTreeMap<i64, u32> = BTreeMap::new();
    let mut frontier = vec![home];
    seen.insert(home, 0);
    for depth in 1..=VIEW_JUMPS {
        let mut next = Vec::new();
        for s in &frontier {
            for n in map.neighbours(*s) {
                if let std::collections::btree_map::Entry::Vacant(e) = seen.entry(n) {
                    e.insert(depth);
                    next.push(n);
                }
            }
        }
        frontier = next;
    }
    let mut out: Vec<(i64, u32)> = seen
        .into_iter()
        .filter(|(s, _)| {
            map.security(*s).is_some_and(|x| x <= NULL_AT_MOST) || held.contains_key(s)
        })
        .collect();
    out.sort_by_key(|(s, d)| (*d, !held.contains_key(s), *s));
    out.truncate(VIEW_MAX);
    out
}

/// The view: every hub in the candidate systems.
pub fn observe(c: &mut Conn, map: &Map, home: i64, now: i64) -> Result<Vec<HubView>> {
    let held = holders(c)?;
    let mut hubs = Vec::new();
    for (system, _) in candidates(map, home, &held) {
        hubs.extend(hubs_in(c, system, now)?);
    }
    Ok(hubs)
}

fn alliance_id(c: &Conn) -> Option<i64> {
    c.session().get_i64("allianceid")
}

/// `allianceRegistry.GetPrimeTimeInfo()` -> `(current, pending)`.
pub fn prime_info(c: &mut Conn, alliance: i64) -> Result<(Option<i64>, Option<i64>)> {
    let out = c.moniker_call(
        "allianceRegistry",
        PyValue::Int(alliance),
        "GetPrimeTimeInfo",
        vec![],
        vec![],
    )?;
    Ok(match out.value {
        Some(v) => (
            int_of(kv(&v, "currentPrimeHour")),
            int_of(kv(&v, "newPrimeHour")),
        ),
        None => (None, None),
    })
}

/// Set the alliance's prime hour to [`prime_hour_for`] when it is not that
/// and no change is pending: `allianceRegistry.SetPrimeHour(hour)`. Only the
/// executor corp's director succeeds; the rest are refused and say so.
fn set_prime(c: &mut Conn, alliance: i64) -> Result<Option<String>> {
    let want = prime_hour_for(alliance);
    let (current, pending) = prime_info(c, alliance)?;
    if current == Some(want) || pending == Some(want) {
        return Ok(None);
    }
    let out = c.moniker_call(
        "allianceRegistry",
        PyValue::Int(alliance),
        "SetPrimeHour",
        vec![PyValue::Int(want)],
        vec![],
    );
    Ok(Some(match out {
        Ok(a) => match a.refusal() {
            None => format!("prime hour {want} asked"),
            Some(key) => format!("prime hour {want} refused: {key}"),
        },
        Err(Error::Refused(why)) => format!("prime hour {want} refused: {why}"),
        Err(e) => return Err(e),
    }))
}

// ---- the kit -----------------------------------------------------------------------------------

/// The Entosis Link types of `content`, by the effect 6063 **[V]**.
pub fn link_types(content: &eve_server::content::Content) -> Vec<i64> {
    eve_server::typedata::installed_in(content)
        .table()
        .types()
        .filter(|t| t.has_effect(EFFECT_ENTOSIS_LINK))
        .map(|t| t.type_id)
        .collect()
}

/// The fuel-block types of `content`, by group **[V]**.
pub fn fuel_block_types(content: &eve_server::content::Content) -> Vec<i64> {
    eve_server::typedata::installed_in(content)
        .table()
        .types()
        .filter(|t| t.group_id == GROUP_FUEL_BLOCK)
        .map(|t| t.type_id)
        .collect()
}

/// A link's effect name from the type table (the client sends it to
/// `Activate`).
pub fn effect_name(content: &eve_server::content::Content) -> Option<String> {
    eve_server::typedata::installed_in(content)
        .table()
        .effect(EFFECT_ENTOSIS_LINK)
        .map(|e| e.name.clone())
}

/// What the pilot offers, read from its hangar and fitting (docked or in
/// space: the fitting is readable in both).
fn kit_of(c: &mut Conn, content: Option<&eve_server::content::Content>) -> Result<Kit> {
    let links = content.map(link_types).unwrap_or_default();
    let fitted = c.fitted().unwrap_or_default();
    let has_link = fitted.iter().any(|r| links.contains(&r.type_id))
        || (c.session().station_id().is_some()
            && c.hangar_items()?.iter().any(|r| links.contains(&r.type_id)));
    let guns = fitted
        .iter()
        .any(|r| crate::modules::weapon_effect(content, r.type_id).is_some());
    Ok(Kit {
        link: has_link,
        guns,
        hauler: !has_link,
    })
}

fn blocks_in(rows: &[ItemRow], types: &[i64]) -> i64 {
    rows.iter()
        .filter(|r| types.contains(&r.type_id) && !r.singleton)
        .map(|r| r.quantity)
        .sum()
}

// ---- flying -------------------------------------------------------------------------------

const MOVES_MAX: usize = 60;
const POLL: Duration = Duration::from_millis(250);
const MOVE_CEILING: Duration = Duration::from_secs(120);

fn gap(c: &mut Conn, target: i64) -> Option<f64> {
    let world = c.world_now();
    world.surface_distance(world.ego?, target)
}

fn warping(c: &mut Conn) -> bool {
    c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
}

fn settle(c: &mut Conn, mut done: impl FnMut(&mut Conn) -> bool) -> Result<()> {
    let deadline = Instant::now() + MOVE_CEILING;
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(Error::Refused("sov: still moving after the ceiling".into()));
        }
        c.pump(POLL)?;
    }
    Ok(())
}

/// **Fly to `system` by the gates** (undock, warp to each gate, jump), a
/// null-sec route allowed ([`SOV_FLOOR`]). Already there in space: nothing.
pub fn fly_to_system(c: &mut Conn, map: &Map, system: i64) -> Result<()> {
    for _ in 0..MOVES_MAX {
        let here = c
            .session()
            .solar_system_id2()
            .ok_or_else(|| Error::NotSent("sov: no system".to_owned()))?;
        if here == system && c.session().station_id().is_none() {
            return crate::capital::bind(c);
        }
        if c.session().station_id().is_some() {
            c.undock_repaired()?;
            crate::capital::bind(c)?;
            continue;
        }
        if c.ballpark().is_none() || !c.world().has_state() {
            crate::capital::bind(c)?;
        }
        let route = map
            .route(here, system, SOV_FLOOR)
            .ok_or_else(|| Error::NotSent(format!("sov: no route {here} -> {system}")))?;
        let Some(next) = route.get(1).copied() else {
            return crate::capital::bind(c);
        };
        let Some((gate, partner)) = map.gate(here, next) else {
            return Err(Error::NotSent(format!("sov: no gate {here} -> {next}")));
        };
        let aim = travel::Aim::Gate {
            gate,
            partner,
            to: next,
        };
        let state = travel::Here {
            docked_at: None,
            bound: true,
            warping: warping(c),
            gap: gap(c, gate),
        };
        match travel::next_move(&state, 0, aim) {
            travel::Move::Wait => settle(c, |c| !warping(c))?,
            travel::Move::Warp(id) => {
                c.warp_to_item(id, 0.0)?;
                settle(c, |c| {
                    !warping(c) && gap(c, id).is_some_and(|g| g < crate::MIN_WARP_DISTANCE)
                })?;
            }
            travel::Move::Approach(id) => {
                c.follow(id, 0.0)?;
                settle(c, |c| {
                    gap(c, id).is_some_and(|g| g < eve_server::jump::MAX_JUMP_DISTANCE)
                })?;
            }
            travel::Move::Jump { gate, partner } => {
                c.stargate_jump(gate, partner)?;
            }
            travel::Move::Bind => crate::capital::bind(c)?,
            other => {
                return Err(Error::NotSent(format!("sov: cannot fly on ({other:?})")));
            }
        }
    }
    Err(Error::Refused(format!(
        "sov: not in {system} after {MOVES_MAX} moves"
    )))
}

/// Warp to a ball of the park and land `range` from it (a hub or a node is
/// a global ball, in every park of its system).
pub fn warp_near(c: &mut Conn, ball: i64, range: f64) -> Result<()> {
    if gap(c, ball).is_some_and(|g| g >= crate::MIN_WARP_DISTANCE) {
        c.warp_to_item(ball, range)?;
        settle(c, |c| !warping(c))?;
    }
    if gap(c, ball).is_some_and(|g| g > range * 1.2) {
        c.follow(ball, range * RANGE_USE)?;
        settle(c, |c| gap(c, ball).is_some_and(|g| g <= range * 1.2))?;
    }
    Ok(())
}

/// Home: back to the staging station and dock.
fn go_home(c: &mut Conn, m: &Memory, map: &Map) -> Result<String> {
    let Some(station) = m.sov.home_station else {
        return Ok("sov: no home station".to_owned());
    };
    if c.session().station_id() == Some(station) {
        return Ok("sov: home".to_owned());
    }
    let trips = travel::Trips {
        map,
        floor: SOV_FLOOR,
    };
    match travel::go(c, trips, station) {
        Ok(_) => Ok(format!("sov: docked at {station}")),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(format!("sov: could not get home: {e}")),
    }
}

// ---- the link ---------------------------------------------------------------------------------

/// A fitted, online Entosis Link and what it needs.
#[derive(Debug, Clone)]
pub struct Rig {
    pub module: i64,
    pub effect: String,
    pub range: f64,
}

/// Strontium cycles a pilot carries **[A]**: a capture is about twenty
/// cycles; nodes and a retry take the rest.
pub const FUEL_CYCLES: i64 = 120;

/// Fit and stock the link while docked: the fitted one, else one from the
/// hangar, else one bought at this station. Strontium into the hold.
/// `Err(why)` inside is a missing piece (a note), not a broken connection.
fn prepare_link(
    c: &mut Conn,
    content: &eve_server::content::Content,
    shop_next: &mut Option<Instant>,
) -> Result<std::result::Result<Rig, String>> {
    let links = link_types(content);
    if links.is_empty() {
        return Ok(Err("no Entosis Link in the type table".to_owned()));
    }
    let Some(effect) = effect_name(content) else {
        return Ok(Err("no effect row for the link".to_owned()));
    };
    let mut module = c.fitted()?.into_iter().find(|r| links.contains(&r.type_id));
    if module.is_none() {
        let mut have = c
            .hangar_items()?
            .into_iter()
            .find(|r| links.contains(&r.type_id));
        if have.is_none() {
            have = shop_link(c, &links, shop_next)?;
        }
        let Some(item) = have else {
            return Ok(Err(format!("MISSING {MISSING_LINK}")));
        };
        match c.fit_auto(&[item.item_id]) {
            Ok(fitted) => {
                let m = fitted.into_iter().next();
                if let Some(m) = &m {
                    let _ = c.set_online(m.item_id);
                }
                module = m;
            }
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(e) => return Ok(Err(format!("the link would not fit: {e}"))),
        }
    }
    let Some(module) = module else {
        return Ok(Err("no link fitted".to_owned()));
    };
    let table = eve_server::typedata::installed_in(content).table();
    let ty = table.get(module.type_id);
    let fuel = ty
        .and_then(|t| t.attribute(ATTR_FUEL_TYPE))
        .map(|f| f.round() as i64);
    let per = ty
        .and_then(|t| t.attribute(ATTR_FUEL_QUANTITY))
        .map_or(1, |q| q.round().max(1.0) as i64);
    let range = ty
        .and_then(|t| t.attribute(ATTR_MAX_RANGE))
        .unwrap_or(10_000.0);
    if let Some(fuel) = fuel.filter(|f| *f > 0) {
        let ship = c
            .session()
            .ship_id()
            .ok_or_else(|| Error::NotSent("sov: no ship".to_owned()))?;
        let want = FUEL_CYCLES * per;
        let have = crate::capital::held_at(c, ship, crate::verbs::FLAG_CARGO, fuel)?;
        if have < want {
            let in_hangar = blocks_in(&c.hangar_items()?, &[fuel]);
            if in_hangar < want - have {
                let budget = c.cash_balance()? * WALLET_SHARE;
                let book = c.market_orders(fuel)?;
                crate::capital::buy_units(c, &book, fuel, want - have - in_hangar, budget)?;
            }
            crate::capital::load_into(c, ship, fuel, crate::verbs::FLAG_CARGO, want - have)?;
        }
    }
    Ok(Ok(Rig {
        module: module.item_id,
        effect,
        range,
    }))
}

/// Buy a link at this station's asks, the cheapest inside the budget.
fn shop_link(
    c: &mut Conn,
    links: &[i64],
    shop_next: &mut Option<Instant>,
) -> Result<Option<ItemRow>> {
    let now = Instant::now();
    if shop_next.is_some_and(|t| now < t) {
        return Ok(None);
    }
    *shop_next = Some(now + SHOP_EVERY);
    let Some(station) = c.session().station_id() else {
        return Ok(None);
    };
    let budget = c.cash_balance()? * WALLET_SHARE;
    let mut best: Option<(i64, f64)> = None;
    for &t in links {
        let book = c.market_orders(t)?;
        if let Some(a) = book
            .asks
            .iter()
            .filter(|a| !a.bid && a.station_id == station && a.vol_remaining > 0)
            .min_by(|a, b| a.price.total_cmp(&b.price))
            && a.price <= budget
            && best.is_none_or(|(_, p)| a.price < p)
        {
            best = Some((t, a.price));
        }
    }
    let Some((t, price)) = best else {
        return Ok(None);
    };
    match c.buy(station, t, price, 1, 0, crate::pricing::BROKER_FEE) {
        Ok(_) => {}
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(_) => return Ok(None),
    }
    Ok(c.hangar_items()?.into_iter().find(|r| r.type_id == t))
}

fn stop_link(c: &mut Conn, rig: &Rig) {
    let _ = c.deactivate(rig.module, &rig.effect);
}

/// The nearest command node on the grid.
fn nearest_node(c: &mut Conn) -> Option<i64> {
    let world = c.world_now();
    let ego = world.ego?;
    world
        .balls()
        .filter(|b| b.type_id() == Some(TYPE_COMMAND_NODE))
        .filter_map(|b| {
            world
                .surface_distance(ego, b.ball_id)
                .map(|d| (d, b.ball_id))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

fn in_park(c: &Conn, ball: i64) -> bool {
    c.world().get(ball).is_some()
}

/// The systems a node tour visits from `hub_system`, nearest first (pure).
pub fn tour(map: &Map, hub_system: i64) -> Vec<i64> {
    let mut seen = vec![hub_system];
    let mut frontier = vec![hub_system];
    for _ in 0..NODE_TOUR_JUMPS {
        let mut next = Vec::new();
        for s in &frontier {
            for n in map.neighbours(*s) {
                if !seen.contains(&n) {
                    seen.push(n);
                    next.push(n);
                }
            }
        }
        frontier = next;
    }
    seen
}

/// **One entosis sortie**: fly to the target, link the hub (or, in a node
/// fight, the nodes -- the hub's system first, then the constellation's
/// gates), and hold until the link ends, the plan moves on or
/// [`SORTIE_LIMIT`] passes. Then home.
fn sortie_link(
    c: &mut Conn,
    m: &mut Memory,
    ctx: &Ctx<'_>,
    alliance: i64,
    who: i64,
    rig: Rig,
    target: &Target,
) -> Result<String> {
    let Some(map) = ctx.map else {
        return Ok("sov: no map".to_owned());
    };
    let deadline = Instant::now() + SORTIE_LIMIT;
    let mut waiting = Instant::now();
    let mut linking: Option<i64> = None;
    let mut polled = Instant::now();
    let mut last_refusal = String::from("never tried");
    let mut captured = 0u32;
    let mut visited: BTreeSet<i64> = BTreeSet::new();
    let order = tour(map, target.system);
    fly_to_system(c, map, target.system)?;
    let note = loop {
        if Instant::now() >= deadline {
            break "sortie limit".to_owned();
        }
        c.pump(Duration::from_millis(300))?;
        let plan = ctx.hall.plan_of(alliance);
        let ours = plan
            .target
            .as_ref()
            .is_some_and(|t| t.hub_id == target.hub_id)
            && ctx.hall.duty(alliance, who) == Duty::Entosis;
        if !ours {
            break "campaign over, standing down".to_owned();
        }
        let nodes = plan
            .target
            .as_ref()
            .is_some_and(|t| matches!(t.phase, Phase::NodeFight));
        let here = c.session().solar_system_id2().unwrap_or(0);
        if let Some(aim) = linking {
            if polled.elapsed() < LINK_POLL {
                continue;
            }
            polled = Instant::now();
            let running = c
                .active_effects(rig.module)
                .map(|e| e.contains(&EFFECT_ENTOSIS_LINK))
                .unwrap_or(false);
            if running && in_park(c, aim) {
                continue;
            }
            stop_link(c, &rig);
            linking = None;
            waiting = Instant::now();
            if nodes {
                captured += 1;
                continue;
            }
            break format!("link ended on hub {}", target.hub_id);
        }
        // Not linking: find the thing to link.
        let aim = if nodes {
            nearest_node(c)
        } else {
            in_park(c, target.hub_id).then_some(target.hub_id)
        };
        let Some(aim) = aim else {
            if waiting.elapsed() >= LINK_WAIT {
                break format!("nothing to link in {here}");
            }
            if nodes {
                visited.insert(here);
                if let Some(next) = order.iter().find(|s| !visited.contains(s)) {
                    fly_to_system(c, map, *next)?;
                } else {
                    visited.clear();
                    fly_to_system(c, map, target.system)?;
                }
            } else if here != target.system {
                fly_to_system(c, map, target.system)?;
            }
            continue;
        };
        warp_near(c, aim, rig.range * RANGE_USE)?;
        let _ = c.lock_target(aim);
        match c.activate(
            rig.module,
            (&rig.effect, EFFECT_ENTOSIS_LINK),
            Some(aim),
            1000,
        ) {
            Ok(_) => {
                linking = Some(aim);
                polled = Instant::now();
            }
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(e) => {
                last_refusal = e.to_string();
                if waiting.elapsed() >= LINK_WAIT {
                    break format!("the link would not start: {last_refusal}");
                }
                std::thread::sleep(LINK_RETRY);
            }
        }
    };
    if linking.is_some() {
        stop_link(c, &rig);
    }
    m.sov.sorties += 1;
    let home = go_home(c, m, map)?;
    Ok(format!(
        "sov entosis: {note} ({captured} nodes, last refusal: {last_refusal}); {home}"
    ))
}

/// **A guard's sortie**: to the target's system, near its hub; the role's
/// in-space rules fight from there.
fn sortie_guard(c: &mut Conn, m: &mut Memory, ctx: &Ctx<'_>, target: &Target) -> Result<String> {
    let Some(map) = ctx.map else {
        return Ok("sov: no map".to_owned());
    };
    if m.sov.post == Some((target.system, target.hub_id))
        && c.session().solar_system_id2() == Some(target.system)
    {
        return Ok(format!("sov guard: holding {}", target.system));
    }
    fly_to_system(c, map, target.system)?;
    if in_park(c, target.hub_id) {
        warp_near(c, target.hub_id, GUARD_RANGE)?;
    }
    m.sov.post = Some((target.system, target.hub_id));
    Ok(format!(
        "sov guard: at {} beside hub {}",
        target.system, target.hub_id
    ))
}

/// **Fuel to an own hub**: blocks from the hangar or the market into the
/// hold, to the hub, `/boostsov all` (the client's own line), home.
fn sortie_haul(
    c: &mut Conn,
    m: &mut Memory,
    ctx: &Ctx<'_>,
    plan: &Plan,
    hub: i64,
) -> Result<String> {
    let (Some(map), Some(content)) = (ctx.map, ctx.content) else {
        return Ok("sov haul: no map or type table".to_owned());
    };
    let Some(&(system, _)) = plan.fuel.iter().find(|(_, h)| *h == hub) else {
        return Ok("sov haul: the hub is not ours any more".to_owned());
    };
    if m.sov
        .fuelled
        .get(&hub)
        .is_some_and(|t| t.elapsed() < REFUEL_EVERY)
    {
        return Ok(format!("sov haul: hub {hub} fuelled lately"));
    }
    let types = fuel_block_types(content);
    if types.is_empty() {
        return Ok("sov haul: no fuel block in the type table".to_owned());
    }
    let ship = c
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("sov haul: no ship".to_owned()))?;
    let want = haul_units(0);
    if c.session().station_id().is_some() {
        let mut carried: i64 = 0;
        for &t in &types {
            carried += crate::capital::held_at(c, ship, crate::verbs::FLAG_CARGO, t)?;
        }
        if carried < want {
            let owned = blocks_in(&c.hangar_items()?, &types);
            let budget = c.cash_balance()? * WALLET_SHARE;
            if owned < want - carried
                && let Some(&t) = types.first()
            {
                let book = c.market_orders(t)?;
                crate::capital::buy_units(c, &book, t, want - carried - owned, budget)?;
            }
            for &t in &types {
                crate::capital::load_into(c, ship, t, crate::verbs::FLAG_CARGO, want - carried)?;
            }
        }
    }
    let mut carried: i64 = 0;
    for &t in &types {
        carried += crate::capital::held_at(c, ship, crate::verbs::FLAG_CARGO, t)?;
    }
    if carried == 0 {
        return Ok(format!(
            "sov haul: MISSING fuel blocks (none owned or on sale here), hub {hub}"
        ));
    }
    fly_to_system(c, map, system)?;
    if !in_park(c, hub) {
        return Ok(format!(
            "sov haul: hub {hub} is not on the grid of {system}"
        ));
    }
    warp_near(c, hub, TRANSFER_RANGE * RANGE_USE)?;
    c.call(
        "publicQaToolsServer",
        "SlashCmd",
        vec![PyValue::Bytes(b"/boostsov all".to_vec())],
    )?;
    // The line answers nothing the client reads; the cargo is the evidence.
    std::thread::sleep(Duration::from_millis(300));
    let _ = c.pump(Duration::from_millis(300));
    let mut after: i64 = 0;
    for &t in &types {
        after += crate::capital::held_at(c, ship, crate::verbs::FLAG_CARGO, t)?;
    }
    let moved = carried - after;
    if moved > 0 {
        m.sov.fuelled.insert(hub, Instant::now());
    }
    m.sov.sorties += 1;
    let home = go_home(c, m, map)?;
    Ok(format!(
        "sov haul: {moved} of {carried} blocks into hub {hub}; {home}"
    ))
}

// ---- the turn ------------------------------------------------------------------------------------

/// **One sov turn**, over the connection: report the kit, plan if the plan
/// is stale, set the prime hour if the alliance's is not its own, take the
/// duty and fly it. A sortie that cannot be done is a note; only a dead
/// connection is the error.
pub fn turn(c: &mut Conn, m: &mut Memory, ctx: &Ctx<'_>) -> Result<String> {
    let Some(alliance) = alliance_id(c) else {
        return Ok("sov: no alliance (a pilot flies for an alliance's sov)".to_owned());
    };
    let who = c
        .session()
        .char_id()
        .ok_or_else(|| Error::NotSent("sov: no character".to_owned()))?;
    let system = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("sov: no system".to_owned()))?;
    if m.sov.home.is_none() {
        m.sov.home = Some(system);
    }
    if m.sov.home_station.is_none() {
        m.sov.home_station = c.session().station_id();
    }
    let home = m.sov.home.unwrap_or(system);
    let now = Instant::now();
    let kit = kit_of(c, ctx.content)?;
    ctx.hall.report(alliance, who, kit, now);

    let mut notes: Vec<String> = Vec::new();
    if ctx.hall.take_planning(alliance, now)
        && let Some(map) = ctx.map
    {
        let stamp = eve_server::pyutil::filetime_now();
        let hubs = observe(c, map, home, stamp)?;
        let previous = ctx.hall.plan_of(alliance).target;
        let jumps = |s: i64| map.jumps(home, s, SOV_FLOOR);
        // R-REL-5: with a society hall, the alliance's view through its
        // coalition and the bloc's enemies and mates; else the plain `R`.
        let relation = |a: i64| match ctx.society {
            Some(h) => h.lock().coalition_relation(alliance, a),
            None => ctx.relation.map_or(0.0, |f| f(alliance, a)),
        };
        let bloc_enemy = |a: i64| {
            ctx.society
                .is_some_and(|h| h.lock().bloc_enemy(alliance, a))
        };
        let bloc_mate = |a: i64| ctx.society.is_some_and(|h| h.lock().bloc_mate(alliance, a));
        let target = plan(&Facts {
            me: alliance,
            hubs: &hubs,
            jumps: &jumps,
            relation: &relation,
            bloc_enemy: &bloc_enemy,
            bloc_mate: &bloc_mate,
            now: stamp,
            linkers: ctx.hall.linkers(alliance),
            previous: previous.as_ref(),
        });
        let fuel = fuel_runs(alliance, &hubs);
        notes.push(format!(
            "planned over {} hubs: {}",
            hubs.len(),
            target
                .as_ref()
                .map_or("no campaign".to_owned(), |t| format!(
                    "{:?} system {}",
                    t.kind, t.system
                ))
        ));
        if let Some(society) = ctx.society {
            tell_society(society, alliance, &hubs, target.as_ref());
        }
        ctx.hall.set_plan(alliance, Plan { target, fuel });
    }
    let plan = ctx.hall.plan_of(alliance);
    if !plan.fuel.is_empty() && ctx.hall.take_prime(alliance, who, now) {
        match set_prime(c, alliance) {
            Ok(Some(line)) => notes.push(line),
            Ok(None) => {}
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(e) => notes.push(format!("prime hour: {e}")),
        }
    }
    let duty = ctx.hall.duty(alliance, who);
    m.sov.duty = duty;
    let line = match (duty, &plan.target) {
        (Duty::Entosis, Some(t)) => {
            let Some(content) = ctx.content else {
                return Ok("sov: no type table (eve-bots --content)".to_owned());
            };
            let rig = if c.session().station_id().is_some() {
                prepare_link(c, content, &mut m.sov.shop_next)?
            } else {
                // In space already: the fitted link, if any.
                let links = link_types(content);
                let fitted = c.fitted()?;
                match (
                    fitted.into_iter().find(|r| links.contains(&r.type_id)),
                    effect_name(content),
                ) {
                    (Some(r), Some(effect)) => Ok(Rig {
                        module: r.item_id,
                        effect,
                        range: eve_server::typedata::attribute_in(
                            content,
                            r.type_id,
                            ATTR_MAX_RANGE,
                        )
                        .unwrap_or(10_000.0),
                    }),
                    _ => Err(format!("MISSING {MISSING_LINK}")),
                }
            };
            match rig {
                Ok(rig) => sortie_link(c, m, ctx, alliance, who, rig, &t.clone())?,
                Err(why) => format!("sov entosis: {why}"),
            }
        }
        (Duty::Guard, Some(t)) => sortie_guard(c, m, ctx, &t.clone())?,
        (Duty::Haul(hub), _) => sortie_haul(c, m, ctx, &plan, hub)?,
        _ => {
            // Stood down: a guard leaves its post for home; anyone else
            // goes on with the escort's life.
            match (m.sov.post.take(), ctx.map) {
                (Some(_), Some(map)) => go_home(c, m, map)?,
                _ => "sov: no duty".to_owned(),
            }
        }
    };
    notes.push(line);
    Ok(notes.join("; "))
}

/// Two halls are equal when they are the same hall (a policy compares).
impl PartialEq for SovHall {
    fn eq(&self, other: &SovHall) -> bool {
        std::ptr::eq(self, other)
    }
}

#[cfg(test)]
mod tests;
