//! **CONCORD answers a crime** (CONCORD-1a, `docs/lanes/concord-1a.md`).
//!
//! The spec is `out/research/R-CONCORD.md` (section 4). CW-1 classifies an
//! act and offers a **criminal** one in high sec to
//! [`crate::crimewatch::ON_CRIME`]; [`on_crime`] is that row. This module
//! never classifies: a war target, a duel, a suspect or a low-sec act never
//! reaches it, which is how CONCORD "stands aside" with no war code of its
//! own.
//!
//! # The response
//!
//! 1. **Open** ([`on_crime`]): one [`Response`] per `(system, offender)`, due
//!    [`delay_secs`] of the system's security after the first crime and
//!    never pushed later by a repeat. The offender is sent the chat warning.
//! 2. **Spawn** ([`tick`], at `due`): the [`squad`] of the band, once, on a
//!    ring round the offender's ball, one `AddBalls2` to the system's pilots.
//!    They are real NPCs (`Npc.law`): their brain sees only the offender, never
//!    their capsule, and does not leash.
//! 3. **Kill**: the commander's ordinary volley (a `damageMultiplier` of
//!    100,000 [V-data]) destroys the ship through `combat::fire`, so the
//!    kill is an ordinary player-ship death: `kill_hooks::PLAYER`, a
//!    killmail whose final blow is the commander with corp CONCORD, **no
//!    insurance** ([`is_concord_kill`], one condition in
//!    `service::insurance::on_kill`).
//! 4. **Pursuit**: the captains and officers scramble, web and jam the
//!    offender ([`ewar_of`], read from their effects 3713, 3714, 3710) until
//!    the kill.
//! 5. **End**: the ship dead, [`LINGER`] and then `RemoveBalls`; the offender
//!    docked, jumped or gone, `RemoveBalls` at once and the response waits
//!    ([`Phase::Away`]) for the next crime, which restarts the delay.
//!
//! 6. **Gate checkpoints** (lane concord-gate): every high-sec stargate holds
//!    standing CONCORD groups ([`tick_checkpoints`]) that idle on their post
//!    ([`STANDBY`]: a law ship with no offender sees nobody). A crime on
//!    their grid ([`on_crime`], within `grid::GRID_ENTER` of a standing ship)
//!    is answered **at once**: those ships take the offender (lock at once,
//!    scram, web, jam, as step 4) and the band's commanders appear beside
//!    them for the volley, because the checkpoint's own types carry no
//!    untankable gun (only the Commander, 11125, has a `damageMultiplier` of
//!    100,000 [V-data]). Ships and commanders end as step 5, and the group
//!    is rebuilt on its post [`CHECKPOINT_RESPAWN`] after it is lost.
//!
//! 7. **Faction police** (NW-7, R-CONCORD section 5, [`tick_police`]): a
//!    pilot in space whose security status puts them inside the net of the
//!    system they fly in (`sec_status::police_call`, the bands of
//!    `crimewatchSvc.GetSecurityWhereAttacked`) gets a [`Case`]: on arrival
//!    by gate or undock, or when the status crosses a band, and
//!    [`POLICE_DELAY`] later one tackling lead and [`POLICE_PLAIN`] plain
//!    ships of the system's owner faction (`npc/police.tsv`) come, with the
//!    faction's chat warning. They are `Npc.law` ships like CONCORD's, with
//!    the faction's own modest guns; unlike CONCORD they can be fought
//!    ([`legal_on_police`]: no crime for the pilot they came for or an
//!    outlaw) and escaped: they go with the pilot (left, docked, status
//!    cleared) or after [`POLICE_PURSUIT`], and one stay in the net draws one
//!    squad. The outlaw's refusal at a high-sec station door is
//!    `sec_status::refuse_dock`. Sentry guns are `crate::sentry` (NW-8).
//!
//! Every span runs on the game clock (`game_clock::span`), so `--time-scale`
//! speeds CONCORD with everything else.
//!
//! # The marks
//!
//! - `[V]`: CONCORD is corp 1,000,125 (`ownerCONCORD`).
//! - `[V-data]`: the three entity types and their effects; the types'
//!   numbers are read from the typedata, never written here.
//! - Owner decision: [`DELAYS`], 1 s in 1.0 to 10 s in 0.5.
//! - `[E]` (EvEJS facts, never its code): the checkpoint of [`GROUP_OFFICERS`]
//!   officers plus [`SPECIAL_POOLS`] draws, [`CHECKPOINT_DISTANCE`] from the
//!   gate's surface with [`CHECKPOINT_SPREAD`] and [`CHECKPOINT_SPACING`],
//!   [`groups_per_gate`], [`CHECKPOINT_RESPAWN`], displayed security 0.5 and
//!   up, a random direction per ship, no pod kill.
//! - `[A]`: [`SQUADS`], the ring ([`RING_NEAR`], [`RING_STEP`]),
//!   [`LINGER`], no pod kill, and one more: the types a **no-bundle** world
//!   lacks are replaced by invented fixtures ([`FIXTURES`]).
//! - `[I]`: CW-1 offers only criminal acts against pilots in high sec, so
//!   `DoesConcordReact`'s "ignored groups" never arise here; an attack on a
//!   CONCORD ship, a station or a gate is not a crime in CW-1 yet (not done).

use crate::crimewatch::{CrimeEvent, CrimeKind, Notice};
use crate::damage::{BlueTime, Damage};
use crate::destiny::hull::Hull;
use crate::npc::{Law, NpcType};
use crate::npc_ewar::{Effect, Kind};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::BTreeMap;
use std::time::Instant;

/// One second, in blue ticks.
const SEC: BlueTime = 10_000_000;

/// `ownerCONCORD`, the corporation: the slim's `ownerID` and `corpID` [V].
pub const CONCORD_CORP: i64 = 1_000_125;

/// The roles of CONCORD's ships. A role's type id is not held here: the
/// bundle's `npc/concord.tsv` (`tools/content_police.py`) maps each role to
/// a type, and a world without it flies the invented fixtures.
///
/// The executioner: CONCORD Police Commander, effect `targetAttack` only
/// [V-data].
pub const EXECUTIONER: &str = "executioner";
/// The pursuers: CONCORD Police Captain and Officer [V-data].
pub const CAPTAIN: &str = "captain";
pub const OFFICER: &str = "officer";

/// `concordTargetJam`, `concordWarpScramble`, `concordModifyTargetSpeed`
/// [V-data].
pub const EFFECT_JAM: i64 = 3_710;
pub const EFFECT_SCRAM: i64 = 3_713;
pub const EFFECT_WEB: i64 = 3_714;

/// The attributes the three read [V-data]: the jam's duration, range and
/// `entityTargetJam` factor; the scram's strength and range are
/// `npc_ewar`'s 105 and 103, its duration 505; the web's 20, 514, 513.
mod attr {
    pub const JAM_DURATION: i64 = 929;
    pub const JAM_RANGE: i64 = 936;
    pub const JAM_FACTOR: i64 = 928;
    pub const SCRAM_STRENGTH: i64 = 105;
    pub const SCRAM_RANGE: i64 = 103;
}

/// The response delay by security rounded to one decimal, seconds: the first
/// row whose floor the rounded security reaches. **Owner decision
/// (2026-10-27):** 1 s in 1.0 rising to 10 s in 0.5, here linear and rounded
/// to whole seconds.
pub const DELAYS: &[(f64, i64)] = &[(1.0, 1), (0.9, 3), (0.8, 5), (0.7, 6), (0.6, 8), (0.0, 10)];

/// **[A]** `(floor, commanders, captains, officers)`: the first row whose
/// floor the rounded security reaches. Falsified by a response the owner
/// finds thin or excessive.
pub const SQUADS: &[(f64, u32, u32, u32)] = &[(0.9, 1, 2, 2), (0.7, 1, 2, 1), (0.0, 1, 1, 1)];

/// **[E]** The checkpoint group: two police officers (EvEJS
/// `concord_gate_checkpoint`, profile `concord_response`, its presentation role is
/// [`OFFICER`]).
pub const GROUP_OFFICERS: usize = 2;
/// **[E]** The special-ops frigate and battleship, and the SWAT frigate and
/// battleship, by role (EvEJS profiles `concord_special_ops_*` and
/// `concord_swat_*`; all four exist in the typedata [V-data]).
pub const SPECIAL_FRIGATE: &str = "special_frigate";
pub const SPECIAL_BATTLESHIP: &str = "special_battleship";
pub const SWAT_FRIGATE: &str = "swat_frigate";
pub const SWAT_BATTLESHIP: &str = "swat_battleship";
/// **[E]** The two pools, `concord_special_ops` and `concord_swat`, each
/// drawn 0 to 1 times (`minCount 0`, `maxCount 1`), frigate to battleship by
/// weight 3 to 1. **[A]** that the 0-or-1 draw is even: EvEJS gives the bounds,
/// not the odds. Falsified by a count of real gate checkpoints.
pub const SPECIAL_POOLS: &[(&str, &str)] = &[
    (SPECIAL_FRIGATE, SPECIAL_BATTLESHIP),
    (SWAT_FRIGATE, SWAT_BATTLESHIP),
];
pub const POOL_WEIGHTS: (u64, u64) = (3, 1);
/// **[E]** From the gate's surface; per ship a random jitter of up to
/// [`CHECKPOINT_SPREAD`] either way, plus [`CHECKPOINT_SPACING`] times its
/// index in the group (the sum capped at [`CHECKPOINT_SPACING_CAP`]).
pub const CHECKPOINT_DISTANCE: f64 = 25_000.0;
pub const CHECKPOINT_SPREAD: f64 = 6_000.0;
pub const CHECKPOINT_SPACING: f64 = 2_200.0;
pub const CHECKPOINT_SPACING_CAP: f64 = 12_000.0;
/// **[E]** A lost group comes back after this long.
pub const CHECKPOINT_RESPAWN: BlueTime = 15 * SEC;
/// **[E]** Displayed security (one decimal) from which a system's gates hold
/// a checkpoint.
pub const CHECKPOINT_MIN_SECURITY: f64 = 0.5;
/// **[A]** Checkpoints go when no pilot has been in the system this long,
/// the belts' own rule (`belts::VACANT_DESPAWN`). Falsified by nothing: it
/// only saves balls nobody can see.
pub const CHECKPOINT_VACANT: BlueTime = crate::belts::VACANT_DESPAWN;

/// A ship of the law with no offender: standing at its post. No character
/// has id 0, so its brain sees nobody.
pub const STANDBY: Law = Law { offender: 0 };

/// **[E]** Groups per gate by displayed security: 1.0 three, 0.9 and 0.8
/// two, lower one.
pub fn groups_per_gate(security: f64) -> u32 {
    let r = (security * 10.0).round() / 10.0;
    if r >= 1.0 {
        3
    } else if r >= 0.8 {
        2
    } else {
        1
    }
}

/// Does a system of this security hold checkpoints ([`CHECKPOINT_MIN_SECURITY`])?
pub fn holds_checkpoints(security: f64) -> bool {
    (security * 10.0).round() / 10.0 >= CHECKPOINT_MIN_SECURITY
}

/// **[A]** How long the ships stay after the kill. Falsified by a human
/// finding them gone too soon (or hanging about).
pub const LINGER: BlueTime = 30 * SEC;

/// **[A]** The ring: the first ship 8 km out, each next 2 km further, back
/// to 8 km every third. Falsified by "they pop in, it looks cheap" (H-C2).
pub const RING_NEAR: f64 = 8_000.0;
pub const RING_STEP: f64 = 2_000.0;

/// The chat warning's key and its one argument, `owner` [V-data].
pub const WARNING_KEY: &str = "chatCONCORDPoliceWarning";

/// **[A]** The seconds CONCORD takes in a system of `security`, by
/// [`DELAYS`] on the security rounded to one decimal (the client's own
/// display rounding).
pub fn delay_secs(security: f64) -> i64 {
    let r = (security * 10.0).round() / 10.0;
    DELAYS
        .iter()
        .find(|(floor, _)| r >= *floor)
        .map_or(10, |(_, s)| *s)
}

/// **[A]** `(commanders, captains, officers)` for a system of `security`.
pub fn squad(security: f64) -> (u32, u32, u32) {
    let r = (security * 10.0).round() / 10.0;
    SQUADS
        .iter()
        .find(|(floor, ..)| r >= *floor)
        .map_or((1, 1, 1), |(_, c, k, o)| (*c, *k, *o))
}

/// Where ship `i` of `n` stands round `centre`: on the XZ ring, spaced by
/// index, 8 to 12 km out.
pub fn ring_position(centre: [f64; 3], i: usize, n: usize) -> [f64; 3] {
    let angle = std::f64::consts::TAU * i as f64 / n.max(1) as f64;
    let radius = RING_NEAR + RING_STEP * (i % 3) as f64;
    [
        centre[0] + radius * angle.cos(),
        centre[1],
        centre[2] + radius * angle.sin(),
    ]
}

// ---------------------------------------------------------------------------
// The invented fixtures a no-bundle world uses
// ---------------------------------------------------------------------------

/// **Invented** stand-ins for the three types, for a world whose typedata
/// does not carry the real ones (no bundle, every test). Not a client row:
/// the commander's volley is merely large enough to end any ship the
/// fixtures fly. Their wrecks are none.
pub const FIXTURE_COMMANDER: NpcType = fixture(
    CONCORD_CORP,
    990_330_001,
    "Fixture Enforcer",
    1_000.0,
    1_000.0,
);
pub const FIXTURE_CAPTAIN: NpcType =
    fixture(CONCORD_CORP, 990_330_002, "Fixture Marshal", 1.0, 0.5);
pub const FIXTURE_OFFICER: NpcType = fixture(CONCORD_CORP, 990_330_003, "Fixture Deputy", 1.0, 0.5);
/// **Invented** faction police (CONCORD-1c): the lead and the plain ship, of an
/// invented corporation, for a world whose typedata lacks the real ones.
pub const FIXTURE_POLICE_LEAD: NpcType = fixture(
    FIXTURE_POLICE_CORP,
    990_330_011,
    "Fixture Commissioner",
    1.0,
    0.5,
);
pub const FIXTURE_POLICE: NpcType = fixture(
    FIXTURE_POLICE_CORP,
    990_330_012,
    "Fixture Constable",
    1.0,
    0.5,
);
/// The fixture empire's NPC corporation (`npc_data::Table::fixture`, faction
/// 599002), which owns the invented police as a real faction's would.
pub const FIXTURE_POLICE_CORP: i64 = 1_999_002;

/// Every fixture, for `npc::find`.
pub const FIXTURES: &[NpcType] = &[
    FIXTURE_COMMANDER,
    FIXTURE_CAPTAIN,
    FIXTURE_OFFICER,
    FIXTURE_POLICE_LEAD,
    FIXTURE_POLICE,
];

pub(crate) const fn fixture(
    owner: i64,
    type_id: i64,
    name: &'static str,
    damage: f64,
    multiplier: f64,
) -> NpcType {
    NpcType {
        type_id,
        name,
        group_id: 990_301,
        owner_corp_id: owner,
        hostile_threshold: crate::npc::RESPONSE_THRESHOLD,
        friendly_threshold: crate::npc::RESPONSE_THRESHOLD,
        hull: Hull {
            type_id,
            mass: 1_500_000.0,
            radius: 40.0,
            max_velocity: 1_800.0,
            agility: crate::npc::FALLBACK_AGILITY,
            shield_capacity: 100_000.0,
            shield_recharge_rate: 1_500_000.0,
            armor_hp: 100_000.0,
            max_target_range: 250_000.0,
            max_locked_targets: 1.0,
            scan_resolution: 1_200.0,
            signature_radius: 35.0,
            warp_speed_au: 0.0,
            hp: 100_000.0,
            capacitor_capacity: 40.0,
            capacitor_recharge_rate: 400_000.0,
        },
        weapon_period_ms: 3_000,
        // Kinetic: the typedata fixture frigate's shield is immune to EM.
        damage: Damage {
            em: 0.0,
            explosive: 0.0,
            kinetic: damage,
            thermal: 0.0,
        },
        damage_multiplier: multiplier,
        weapon_guid: crate::npc::TURRET_FX_GUID,
        attack_range: 200_000.0,
        orbit_range: 5_000.0,
        cruise_fraction: 0.5,
        wreck_type_id: None,
        bounty: 0.0,
        ignore_drones_below_signature: 0.0,
        equipment_min: 0,
        equipment_max: 0,
        equipment: &[],
        cargo: &[],
    }
}

/// The type for a role, the bundle's (`npc/concord.tsv`) if the typedata carries it, else the fixture.
fn kind_for(w: &mut Registry, role: &str, fixture: &'static NpcType) -> &'static NpcType {
    let type_id = w
        .content()
        .try_get::<crate::npc_data::Table>()
        .and_then(|t| t.concord_roles.get(role).copied());
    type_id
        .and_then(|t| crate::npc::resolve_concord(w, t))
        .unwrap_or(fixture)
}

// ---------------------------------------------------------------------------
// The state
// ---------------------------------------------------------------------------

/// Where a [`Response`] stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Due at [`Response::due`]; nothing is in space yet.
    Pending,
    /// The ships are in space.
    Active,
    /// The offender's ship is dead; the ships leave at the time.
    Lingering(BlueTime),
    /// The offender docked, jumped or left: the ships went at once. The next
    /// crime in the system restarts the delay.
    Away,
}

/// One response, keyed `(system, offender)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub offender: i64,
    /// The ball the offender flew at the first crime.
    pub ship: i64,
    pub system: i64,
    /// First crime plus the delay; a repeat never moves it later.
    pub due: BlueTime,
    /// The NPCs, once due.
    pub spawned: Vec<i64>,
    pub last_crime: BlueTime,
    pub phase: Phase,
}

/// One checkpoint group on one gate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Group {
    pub ids: Vec<i64>,
    /// When the lost group is rebuilt; `None` before the first build.
    pub respawn_at: Option<BlueTime>,
    /// How many times it has been built: the draw's seed.
    pub builds: u32,
}

/// A target's jam: its locks are capped at `max locks * factor` until
/// `until`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Jammed {
    until: BlueTime,
    factor: f64,
}

/// The slot: the open responses, each pursuer's next cycle, the jams.
/// Never persisted: a restart takes every ship out of space [A].
#[derive(Debug, Clone)]
pub struct Concord {
    /// `--concord`: on by default [A], falsified by an owner who wants a
    /// CONCORD-free world.
    pub enabled: bool,
    responses: BTreeMap<(i64, i64), Response>,
    next_cycle: BTreeMap<(i64, Kind), BlueTime>,
    next_jam: BTreeMap<i64, BlueTime>,
    jammed: BTreeMap<i64, Jammed>,
    /// The last security penalty for shooting the law, per attacker.
    law_hit: BTreeMap<i64, BlueTime>,
    /// The checkpoint groups, by `(system, gate, slot)`.
    posts: BTreeMap<(i64, i64, u32), Group>,
    /// Since when each checkpointed system has had no pilot.
    vacant: BTreeMap<i64, BlueTime>,
    /// The faction police cases, by the pilot they were sent for.
    cases: BTreeMap<i64, Case>,
    /// When the next sweep for pilots newly inside a police net runs.
    next_sweep: BlueTime,
    /// Most faction police squads in space in one system at once.
    max_squads: usize,
    /// The low-sec sentry guns (NW-8, `crate::sentry`).
    pub(crate) sentries: crate::sentry::Sentries,
}

impl Default for Concord {
    fn default() -> Self {
        Concord {
            enabled: true,
            responses: BTreeMap::new(),
            next_cycle: BTreeMap::new(),
            next_jam: BTreeMap::new(),
            jammed: BTreeMap::new(),
            law_hit: BTreeMap::new(),
            posts: BTreeMap::new(),
            vacant: BTreeMap::new(),
            cases: BTreeMap::new(),
            next_sweep: 0,
            max_squads: MAX_SQUADS_PER_SYSTEM,
            sentries: crate::sentry::Sentries::default(),
        }
    }
}

impl Concord {
    pub fn responses(&self) -> impl Iterator<Item = &Response> {
        self.responses.values()
    }

    /// The checkpoint ships standing in `system`, group by group.
    pub fn posts(&self, system: i64) -> impl Iterator<Item = (&(i64, i64, u32), &Group)> {
        self.posts.iter().filter(move |(k, _)| k.0 == system)
    }

    pub fn response(&self, system: i64, offender: i64) -> Option<&Response> {
        self.responses.get(&(system, offender))
    }

    /// The faction police cases, one per pilot inside a net.
    pub fn cases(&self) -> impl Iterator<Item = &Case> {
        self.cases.values()
    }

    pub fn case(&self, character: i64) -> Option<&Case> {
        self.cases.get(&character)
    }
}

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
pub const SLOT: SlotSpec = SlotSpec {
    name: "concord",
    make: |_| Box::new(Concord::default()) as Boxed,
};

/// `--concord` / `--no-concord`.
pub fn set_enabled(w: &mut Registry, on: bool) {
    w.ext_mut::<Concord>().enabled = on;
}

/// Did a CONCORD ship fire the killing volley? Insurance pays nothing then
/// (`appLogConst.eventCrimesInsuranceNoPayoutGCC` [V]).
pub fn is_concord_kill(w: &Registry, killer: Option<i64>) -> bool {
    killer
        .and_then(|k| w.pilot(k))
        .is_some_and(|p| p.char_id.is_none() && p.corporation_id == CONCORD_CORP)
}

// ---------------------------------------------------------------------------
// The seam row
// ---------------------------------------------------------------------------

/// **The `crimewatch::ON_CRIME` row.** Acts only on a criminal act in high
/// sec while CONCORD is on: opens (or refreshes) the response and warns the
/// offender.
pub fn on_crime(w: &mut Registry, ev: &CrimeEvent) {
    use crate::crimewatch::Space;
    if !w.ext::<Concord>().enabled || ev.kind != CrimeKind::Criminal || ev.space != Space::High {
        return;
    }
    let Some(p) = w.roster().get(ev.offender).copied() else {
        return;
    };
    if p.solar_system_id != ev.solar_system_id {
        return;
    }
    let Some(security) = w
        .content()
        .get::<crate::celestial::Universe>()
        .system(ev.solar_system_id)
        .map(|s| s.security)
    else {
        return;
    };
    // On a checkpoint's grid the answer is at once (no delay): its standing
    // ships take the offender and the commanders join them.
    let key = (ev.solar_system_id, ev.offender);
    let open = w
        .ext::<Concord>()
        .responses
        .get(&key)
        .is_some_and(|r| r.phase != Phase::Away);
    let answer = if open {
        Vec::new()
    } else {
        claim(
            w,
            ev.solar_system_id,
            ev.offender,
            p.motion.position,
            security,
        )
    };
    let delay = crate::game_clock::span(w, delay_secs(security) * SEC);
    let c = w.ext_mut::<Concord>();
    match c.responses.get_mut(&key) {
        Some(r) if r.phase == Phase::Away => {
            r.ship = p.ship_id;
            r.last_crime = ev.at;
            if answer.is_empty() {
                r.due = ev.at + delay;
                r.phase = Phase::Pending;
            } else {
                r.due = ev.at;
                r.spawned = answer;
                r.phase = Phase::Active;
            }
        }
        Some(r) => {
            // Never later: a repeat only notes itself.
            r.last_crime = r.last_crime.max(ev.at);
            return;
        }
        None => {
            let answered = !answer.is_empty();
            c.responses.insert(
                key,
                Response {
                    offender: ev.offender,
                    ship: p.ship_id,
                    system: ev.solar_system_id,
                    due: if answered { ev.at } else { ev.at + delay },
                    spawned: answer,
                    last_crime: ev.at,
                    phase: if answered {
                        Phase::Active
                    } else {
                        Phase::Pending
                    },
                },
            );
        }
    }
    let on_grid = w
        .ext::<Concord>()
        .response(ev.solar_system_id, ev.offender)
        .is_some_and(|r| r.phase == Phase::Active);
    crate::outln!(
        "[concord] {} in {}: due in {} s",
        ev.offender,
        ev.solar_system_id,
        if on_grid { 0 } else { delay / SEC }
    );
    w.schedule(
        std::time::Duration::ZERO,
        Notice::new(
            ev.offender,
            "OnRemoteMessage",
            vec![
                crate::pyutil::bstr(WARNING_KEY),
                crate::pyutil::pydict(vec![("owner", eve_marshal::PyValue::Int(ev.offender))]),
            ],
        ),
    );
}

/// **The `kill_hooks::PLAYER` row**: the offender's ship died. Under a live
/// squad they linger [`LINGER`] and leave; before the squad is due there is
/// nothing left to answer. Their capsule's death changes nothing (CONCORD
/// does not pod [A]).
pub fn on_kill(w: &mut Registry, ev: &crate::kill_hooks::KillEvent) {
    let key = (ev.system, ev.victim_character);
    let linger = crate::game_clock::span(w, LINGER);
    let c = w.ext_mut::<Concord>();
    let Some(r) = c.responses.get_mut(&key) else {
        return;
    };
    if r.ship != ev.victim_ship {
        return;
    }
    match r.phase {
        Phase::Pending | Phase::Away => {
            c.responses.remove(&key);
        }
        Phase::Active => r.phase = Phase::Lingering(ev.when + linger),
        Phase::Lingering(_) => {}
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// Where the offender is, against the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Where {
    /// Still in the ship, in the system.
    Same,
    /// In the system but not in that ship: it died (or they reshipped).
    Died,
    /// Not in this system's space.
    Gone,
}

fn where_is(w: &Registry, r: &Response) -> Where {
    let Some(p) = w.roster().get(r.offender) else {
        return Where::Gone;
    };
    if p.solar_system_id != r.system {
        return Where::Gone;
    }
    let capsule = w
        .pilot(r.offender)
        .is_some_and(|x| x.ship_type_id == crate::damage::CAPSULE_TYPE_ID);
    if p.ship_id != r.ship || capsule {
        Where::Died
    } else {
        Where::Same
    }
}

/// **The world clock's stage**, after `npc` (so a pursuer's lock is read the
/// tick it is made): spawn what is due, drive the pursuers' effects, end
/// what is over.
pub fn tick(w: &mut Registry, _at: Instant, now: BlueTime) {
    tick_responses(w, now);
    // After the responses, so a group they just sent away is noticed lost
    // in the same tick.
    tick_checkpoints(w, now);
    tick_police(w, now);
    crate::sentry::tick(w, now);
}

fn tick_responses(w: &mut Registry, now: BlueTime) {
    if w.ext::<Concord>().responses.is_empty() {
        return;
    }
    let keys: Vec<(i64, i64)> = w.ext::<Concord>().responses.keys().copied().collect();
    // The squad warps in, so it starts its flight this long before it is due
    // and lands on the due time: the response time does not grow.
    let lead = crate::npc::warp_in_lead(w);
    for key in keys {
        let Some(mut r) = w.ext::<Concord>().responses.get(&key).cloned() else {
            continue;
        };
        let here = where_is(w, &r);
        let mut done = false;
        match r.phase {
            Phase::Pending => match here {
                Where::Same if now + lead >= r.due => spawn_squad(w, &mut r, now),
                Where::Same => {}
                // Reshipped or dead before it was due: nothing to answer.
                Where::Died => done = true,
                Where::Gone => r.phase = Phase::Away,
            },
            Phase::Active => match here {
                Where::Same => drive(w, &r, now),
                Where::Died => {
                    r.phase = Phase::Lingering(now + crate::game_clock::span(w, LINGER));
                }
                Where::Gone => {
                    despawn(w, &mut r);
                    r.phase = Phase::Away;
                }
            },
            Phase::Lingering(until) => {
                if now >= until {
                    despawn(w, &mut r);
                    done = true;
                }
            }
            Phase::Away => {
                // The criminal timer ended: the response closes.
                if !crate::crimewatch::is_criminal(w, r.offender, now) {
                    done = true;
                }
            }
        }
        let c = w.ext_mut::<Concord>();
        if done {
            c.responses.remove(&key);
        } else {
            c.responses.insert(key, r);
        }
    }
    let c = w.ext_mut::<Concord>();
    c.jammed.retain(|_, j| j.until > now);
}

/// The ships come, once.
fn spawn_squad(w: &mut Registry, r: &mut Response, now: BlueTime) {
    w.advance_character(r.offender);
    let Some(centre) = w.roster().get(r.offender).map(|p| p.motion.position) else {
        return;
    };
    let security = w
        .content()
        .get::<crate::celestial::Universe>()
        .system(r.system)
        .map_or(0.5, |s| s.security);
    let (commanders, captains, officers) = squad(security);
    let mut kinds: Vec<&'static NpcType> = Vec::new();
    for _ in 0..commanders {
        kinds.push(kind_for(w, EXECUTIONER, &FIXTURE_COMMANDER));
    }
    for _ in 0..captains {
        kinds.push(kind_for(w, CAPTAIN, &FIXTURE_CAPTAIN));
    }
    for _ in 0..officers {
        kinds.push(kind_for(w, OFFICER, &FIXTURE_OFFICER));
    }
    let n = kinds.len();
    let law = Law {
        offender: r.offender,
    };
    let mut ids = Vec::new();
    for (i, kind) in kinds.into_iter().enumerate() {
        let at = ring_position(centre, i, n);
        // Early (before the due time): it warps in and lands on it. At or
        // past it (an answered crime, a late tick) it is there at once, as
        // the flight would only add to the response time.
        let made = if now < r.due {
            let out = [at[0] - centre[0], at[1] - centre[1], at[2] - centre[2]];
            crate::npc::spawn_bound_warping_in(w, kind, r.system, at, law, out)
        } else {
            crate::npc::spawn_bound(w, kind, r.system, at, law)
        };
        if let Some(id) = made {
            ids.push(id);
        }
    }
    crate::npc::announce(w, r.system, &ids, true);
    crate::outln!(
        "[concord] {} ships on {} in {} (t={now})",
        ids.len(),
        r.offender,
        r.system
    );
    r.spawned = ids;
    r.phase = Phase::Active;
}

// ---------------------------------------------------------------------------
// The gate checkpoints
// ---------------------------------------------------------------------------

/// SplitMix64: a deterministic draw for a post's ships (EvEJS draws at
/// random; any even source is the same fact).
pub(crate) fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// A uniform draw in `[0, 1)`.
pub(crate) fn unit(x: u64) -> f64 {
    (mix(x) >> 11) as f64 / (1u64 << 53) as f64
}

/// The roles of one checkpoint group, by role, drawn from
/// `seed`: the officers, then each pool 0 or 1 times.
pub fn group_types(seed: u64) -> Vec<&'static str> {
    let mut out = vec![OFFICER; GROUP_OFFICERS];
    for (i, (light, heavy)) in SPECIAL_POOLS.iter().enumerate() {
        let k = mix(seed ^ ((i as u64 + 1) << 56));
        if k & 1 == 0 {
            continue;
        }
        let total = POOL_WEIGHTS.0 + POOL_WEIGHTS.1;
        out.push(if (k >> 8) % total < POOL_WEIGHTS.0 {
            *light
        } else {
            *heavy
        });
    }
    out
}

/// Where ship `i` of a group stands for `gate`: a random direction, at the
/// surface distance plus its jitter and spacing [E].
pub fn post_position(gate: [f64; 3], radius: f64, seed: u64, i: usize) -> [f64; 3] {
    let s = seed ^ ((i as u64 + 1) << 40);
    let z = 2.0 * unit(s) - 1.0;
    let phi = std::f64::consts::TAU * unit(s ^ 0xA5A5);
    let r = (1.0 - z * z).sqrt();
    let jitter = (2.0 * unit(s ^ 0x5A5A) - 1.0) * CHECKPOINT_SPREAD;
    let spacing = (i as f64 * CHECKPOINT_SPACING).min(CHECKPOINT_SPACING_CAP);
    let d = (radius + CHECKPOINT_DISTANCE + jitter + spacing).max(1_000.0);
    [
        gate[0] + d * r * phi.cos(),
        gate[1] + d * z,
        gate[2] + d * r * phi.sin(),
    ]
}

/// The fixture a missing type stands in for: a frigate role by the officer,
/// a battleship role by the captain.
fn fixture_for(role: &str) -> &'static NpcType {
    if matches!(role, SPECIAL_BATTLESHIP | SWAT_BATTLESHIP) {
        &FIXTURE_CAPTAIN
    } else {
        &FIXTURE_OFFICER
    }
}

/// **The checkpoint pass**, every tick: in each system with a pilot and a
/// displayed security of [`CHECKPOINT_MIN_SECURITY`] or more, every served
/// stargate holds [`groups_per_gate`] groups; a group lost is rebuilt
/// [`CHECKPOINT_RESPAWN`] later; a system with no pilot for
/// [`CHECKPOINT_VACANT`] loses its groups.
fn tick_checkpoints(w: &mut Registry, now: BlueTime) {
    if !w.ext::<Concord>().enabled {
        return;
    }
    let mut systems: Vec<i64> = w.ext::<crate::liveness::Liveness>().hot().collect();
    systems.extend(w.ext::<Concord>().posts.keys().map(|k| k.0));
    systems.sort_unstable();
    systems.dedup();
    for system in systems {
        let Some(security) = w
            .content()
            .get::<crate::celestial::Universe>()
            .system(system)
            .filter(|s| s.band() == crate::celestial::Band::High)
            .map(|s| s.security)
            .filter(|s| holds_checkpoints(*s))
        else {
            continue;
        };
        let pilots = w
            .roster()
            .in_system(system)
            .into_iter()
            .any(|p| w.character(p.character_id).is_some());
        if !pilots {
            let since = *w.ext_mut::<Concord>().vacant.entry(system).or_insert(now);
            if now.saturating_sub(since) >= crate::game_clock::span(w, CHECKPOINT_VACANT) {
                clear_system(w, system);
            }
            continue;
        }
        w.ext_mut::<Concord>().vacant.remove(&system);
        w.advance_system(system);
        let gates: Vec<(i64, [f64; 3], f64)> = w
            .celestials()
            .served_in(system)
            .filter(|c| c.kind == crate::celestial::Kind::Gate)
            .map(|c| (c.id, c.position, c.radius))
            .collect();
        for (gate, position, radius) in gates {
            for slot in 0..groups_per_gate(security) {
                step_group(w, (system, gate, slot), position, radius, now);
            }
        }
    }
}

/// One group: forget its lost ships, start the respawn wait when all are
/// lost, build it when the wait is over.
fn step_group(w: &mut Registry, key: (i64, i64, u32), gate: [f64; 3], radius: f64, now: BlueTime) {
    let mut g = w
        .ext::<Concord>()
        .posts
        .get(&key)
        .cloned()
        .unwrap_or_default();
    let had = !g.ids.is_empty();
    g.ids.retain(|id| w.roster().get(*id).is_some());
    if had && g.ids.is_empty() {
        g.respawn_at = Some(now + crate::game_clock::span(w, CHECKPOINT_RESPAWN));
    }
    if g.ids.is_empty() && g.respawn_at.is_none_or(|t| now >= t) {
        let seed = mix(key.1 as u64 ^ (u64::from(key.2) << 48) ^ mix(u64::from(g.builds)));
        let mut ids = Vec::new();
        for (i, role) in group_types(seed).into_iter().enumerate() {
            let kind = kind_for(w, role, fixture_for(role));
            let at = post_position(gate, radius, seed, i);
            // The first build stands before anyone has looked; a rebuilt
            // post (its group was killed) comes in warp, on a grid that may
            // be occupied.
            let made = if g.builds > 0 {
                let out = [at[0] - gate[0], at[1] - gate[1], at[2] - gate[2]];
                crate::npc::spawn_bound_warping_in(w, kind, key.0, at, STANDBY, out)
            } else {
                crate::npc::spawn_bound(w, kind, key.0, at, STANDBY)
            };
            if let Some(id) = made {
                ids.push(id);
            }
        }
        crate::npc::announce(w, key.0, &ids, true);
        g.ids = ids;
        g.builds += 1;
        g.respawn_at = None;
    }
    w.ext_mut::<Concord>().posts.insert(key, g);
}

/// A vacant system: its groups leave.
fn clear_system(w: &mut Registry, system: i64) {
    let keys: Vec<(i64, i64, u32)> = w
        .ext::<Concord>()
        .posts
        .keys()
        .filter(|k| k.0 == system)
        .copied()
        .collect();
    for k in keys {
        if let Some(g) = w.ext_mut::<Concord>().posts.remove(&k) {
            crate::npc::despawn_ids(w, system, &g.ids);
        }
    }
    w.ext_mut::<Concord>().vacant.remove(&system);
}

/// **The checkpoint answers a crime on its grid**: every standing ship
/// within `grid::GRID_ENTER` of the offender takes them (the law's lock is
/// at once), and the band's commanders appear at the nearest of them. The
/// ships answered, empty when none stood on the offender's grid.
fn claim(w: &mut Registry, system: i64, offender: i64, at: [f64; 3], security: f64) -> Vec<i64> {
    let mut near: Vec<(f64, i64, [f64; 3])> = w
        .npcs()
        .iter()
        .filter(|n| n.solar_system_id == system && n.law == Some(STANDBY))
        .filter_map(|n| {
            let p = w.roster().get(n.id)?.motion.position;
            let d = crate::space_state::distance(p, at);
            (d <= crate::grid::GRID_ENTER).then_some((d, n.id, p))
        })
        .collect();
    if near.is_empty() {
        return Vec::new();
    }
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut ids: Vec<i64> = near.iter().map(|n| n.1).collect();
    for id in &ids {
        crate::npc::set_law(w, *id, Law { offender });
    }
    let (commanders, _, _) = squad(security);
    let mut made = Vec::new();
    for i in 0..commanders as usize {
        let kind = kind_for(w, EXECUTIONER, &FIXTURE_COMMANDER);
        let p = ring_position(near[0].2, i, commanders as usize);
        if let Some(id) = crate::npc::spawn_bound(w, kind, system, p, Law { offender }) {
            made.push(id);
        }
    }
    crate::npc::announce(w, system, &made, true);
    ids.extend(made);
    ids
}

/// The ships leave (`RemoveBalls`), and their cycles with them.
fn despawn(w: &mut Registry, r: &mut Response) {
    let ids = std::mem::take(&mut r.spawned);
    crate::npc::despawn_ids(w, r.system, &ids);
    let c = w.ext_mut::<Concord>();
    c.next_cycle.retain(|(id, _), _| !ids.contains(id));
}

// ---------------------------------------------------------------------------
// The pursuers' effects
// ---------------------------------------------------------------------------

/// A jam, in the server's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jam {
    pub range: f64,
    pub period: BlueTime,
    /// `entityTargetJam` (928): the factor the target's `maxLockedTargets`
    /// is multiplied by. Below one it jams; 0.0 is a full wipe [V-data].
    pub factor: f64,
}

/// What a type's CONCORD effects do.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Ewar {
    pub scram: Option<Effect>,
    pub web: Option<Effect>,
    pub jam: Option<Jam>,
}

fn ticks(ms: f64) -> BlueTime {
    (ms.max(0.0) * 10_000.0) as BlueTime
}

/// **The mapping of the CONCORD effect ids** onto the machinery that
/// exists: 3713 to `npc_ewar`'s scram (duration 505, range 103, strength
/// 105), 3714 to its web (513, 514, 20), 3710 to the new jam (929, 936,
/// factor 928). Every cycle lands (`entityWarpScrambleChance` is 1.0 on the
/// pursuers [V-data]); an effect with no usable numbers maps to nothing.
pub fn ewar_of(td: &crate::typedata::Table, type_id: i64) -> Ewar {
    ewar_with(td, type_id, EFFECT_SCRAM, EFFECT_WEB)
}

/// A faction police ship's tackle (NW-7): the same numbers, read where the
/// group-182 types carry them, under the module effects `warpScramble` (563)
/// and `modifyTargetSpeed2` (575) [V-data]. Never a jam.
pub fn police_ewar_of(td: &crate::typedata::Table, type_id: i64) -> Ewar {
    Ewar {
        jam: None,
        ..ewar_with(td, type_id, POLICE_TACKLE[0], POLICE_TACKLE[1])
    }
}

fn ewar_with(td: &crate::typedata::Table, type_id: i64, scram_id: i64, web_id: i64) -> Ewar {
    let Some(row) = td.get(type_id) else {
        return Ewar::default();
    };
    let a = |id: i64| td.attribute(type_id, id);
    let scram = row.has_effect(scram_id).then(|| Effect {
        kind: Kind::Scram,
        range: a(attr::SCRAM_RANGE),
        strength: a(attr::SCRAM_STRENGTH),
        period: ticks(a(crate::npc_ewar::attr::SCRAM_DURATION)),
        chance: 1.0,
    });
    let web = row.has_effect(web_id).then(|| Effect {
        kind: Kind::Web,
        range: a(crate::npc_ewar::attr::WEB_RANGE),
        strength: a(crate::npc_ewar::attr::SPEED_FACTOR),
        period: ticks(a(crate::npc_ewar::attr::WEB_DURATION)),
        chance: 1.0,
    });
    let jam = row.has_effect(EFFECT_JAM).then(|| Jam {
        range: a(attr::JAM_RANGE),
        period: ticks(a(attr::JAM_DURATION)),
        // Absent means the attribute's own default, 1.0: no jam [V-data].
        factor: row.attribute(attr::JAM_FACTOR).unwrap_or(1.0),
    });
    Ewar {
        scram: scram.filter(|e| e.range > 0.0 && e.strength > 0.0 && e.period > 0),
        web: web.filter(|e| e.range > 0.0 && e.strength < 0.0 && e.period > 0),
        jam: jam.filter(|j| j.range > 0.0 && j.period > 0 && j.factor < 1.0),
    }
}

/// One pursuer pass: each engaged ship cycles its effects on the offender
/// on its own period.
fn drive(w: &mut Registry, r: &Response, now: BlueTime) {
    for id in r.spawned.clone() {
        let Some(npc) = w.npcs().get(id).copied() else {
            continue;
        };
        let Some(target) = npc.target else { continue };
        // Engaged: its lock is complete and in the graph.
        if npc.lock_ready_at.is_none_or(|t| t > now) || !w.locks().targets_of(id).contains(&target)
        {
            continue;
        }
        let ewar = ewar_of(
            crate::typedata::installed_in(w.content()).table(),
            npc.kind.type_id,
        );
        for e in [ewar.scram, ewar.web].into_iter().flatten() {
            let next = w
                .ext::<Concord>()
                .next_cycle
                .get(&(id, e.kind))
                .copied()
                .unwrap_or(0);
            if now >= next {
                crate::npc_ewar::cycle(w, &npc, &e, target, now);
                w.ext_mut::<Concord>()
                    .next_cycle
                    .insert((id, e.kind), now + e.period);
            }
        }
        if let Some(j) = ewar.jam {
            let next = w.ext::<Concord>().next_jam.get(&id).copied().unwrap_or(0);
            if now >= next {
                jam(w, &npc, target, now, &j);
                w.ext_mut::<Concord>().next_jam.insert(id, now + j.period);
            }
        }
    }
}

/// **`concordTargetJam`**: while the cycle lasts the target's maximum locks
/// are multiplied by the jam's factor; the locks over the new cap are
/// dropped now (all of them at 0.0), and [`jam_limit`] refuses new ones.
/// `[A]` that a jammed pilot may not lock past the cap for the cycle
/// (falsified by a jammed pilot who can still lock).
pub fn jam(w: &mut Registry, npc: &crate::npc::Npc, target: i64, now: BlueTime, j: &Jam) {
    let Some(p) = w.roster().by_ship(target).copied() else {
        return;
    };
    if p.solar_system_id != npc.solar_system_id || w.character(p.character_id).is_none() {
        return;
    }
    let Some(source) = w.roster().get(npc.id) else {
        return;
    };
    if crate::space_state::distance(source.motion.position, p.motion.position) > j.range {
        return;
    }
    w.ext_mut::<Concord>().jammed.insert(
        target,
        Jammed {
            until: now + j.period,
            factor: j.factor,
        },
    );
    let limit = crate::service::max_locked_targets_of(
        w.content(),
        w.pilot(p.character_id).map_or(0, |x| x.ship_type_id),
    );
    let cap = jam_cap(limit, j.factor);
    let held = w.locks().targets_of(target);
    for t in held.into_iter().skip(cap) {
        if !w.locks_mut().remove(target, t) {
            continue;
        }
        use crate::targeting::{Reason, TargetChange, What};
        w.schedule(
            std::time::Duration::ZERO,
            TargetChange {
                to: vec![p.character_id],
                what: What::Lost,
                ship: Some(t),
                reason: Some(Reason::Unlocked),
            },
        );
        if let Some(other) = w
            .roster()
            .by_ship(t)
            .map(|x| x.character_id)
            .filter(|c| w.character(*c).is_some())
        {
            w.schedule(
                std::time::Duration::ZERO,
                TargetChange {
                    to: vec![other],
                    what: What::OtherLost,
                    ship: Some(target),
                    reason: Some(Reason::Unlocked),
                },
            );
        }
    }
}

/// The locks a jammed ship may hold: `limit * factor`, whole.
pub fn jam_cap(limit: f64, factor: f64) -> usize {
    let v = (limit * factor).floor();
    if v.is_finite() && v > 0.0 {
        v as usize
    } else {
        0
    }
}

/// **`targeting::lock`'s limit while jammed**: `limit` itself when `ship` is
/// not jammed at `now`, else the cap.
pub fn jam_limit_at(w: &Registry, ship: i64, limit: f64, now: BlueTime) -> f64 {
    match w.ext::<Concord>().jammed.get(&ship) {
        Some(j) if j.until > now => jam_cap(limit, j.factor) as f64,
        _ => limit,
    }
}

/// [`jam_limit_at`] at the present: the one line `targeting::lock` calls.
pub fn jam_limit(w: &Registry, ship: i64, limit: f64) -> f64 {
    jam_limit_at(w, ship, limit, crate::pyutil::filetime_now())
}

// ---------------------------------------------------------------------------
// No escape: the refusals (CONCORD-1b)
// ---------------------------------------------------------------------------

/// The client's keys for a criminal's refused moves (`user-errors.tsv`);
/// no client code raises them [V]. The first four take a `time`.
pub const REFUSE_WARP: &str = "CantWarpAwayGlobalCriminalFlag";
pub const REFUSE_DOCK: &str = "CantDockGlobalCriminalFlag";
pub const REFUSE_GATE: &str = "CantJumpGlobalCriminalFlag";
pub const REFUSE_WORMHOLE: &str = "CantEnterWormholeGlobalCriminalFlag";
pub const REFUSE_BEACON: &str = "CantBeaconJumpGlobalCriminalFlag";
pub const REFUSE_BOARD: &str = "CannotBoardShipInHighSecAsCriminal";
pub const REFUSE_BOARD_POLICE: &str = "ShipInvolvedInPoliceCrime";
pub const REFUSE_CLOAK: &str = "CantCloakPolicePursuit";
pub const CLOAK_DISENGAGED: &str = "CloakingDisengagedPolicePursuit";

/// How long the criminal timer still runs for `character_id`, in blue ticks;
/// `None` when they are no criminal. This is the `time` of the refusals,
/// passed as the raw interval [I]; falsified by a dialog that shows a date
/// or a bare number where it should read "14 minutes".
pub fn criminal_for(w: &Registry, character_id: i64, now: BlueTime) -> Option<BlueTime> {
    if !crate::crimewatch::is_criminal(w, character_id, now) {
        return None;
    }
    let end =
        w.combat()
            .crimewatch()
            .expiry(character_id, crate::crimewatch::Timer::Criminal, now)?;
    Some(end - now)
}

/// The system a pilot in space is in, when flags run there (not null sec).
fn empire_system_of(w: &Registry, character_id: i64) -> Option<i64> {
    let system = w.roster().get(character_id)?.solar_system_id;
    (crate::crimewatch::space_of(w, system) != crate::crimewatch::Space::Zero).then_some(system)
}

/// Is a CONCORD response open for `character_id` in the system they are in:
/// the squad due or in space? A lingering one (the ship is dead, the pod is
/// free) or an `Away` one is not.
pub fn pursued(w: &Registry, character_id: i64) -> bool {
    let c = w.ext::<Concord>();
    c.enabled
        && w.roster().get(character_id).is_some_and(|p| {
            c.response(p.solar_system_id, character_id)
                .is_some_and(|r| matches!(r.phase, Phase::Pending | Phase::Active))
        })
}

fn timed(key: &'static str, left: BlueTime) -> crate::service::ServiceError {
    crate::service::ServiceError::Raise(eve_proto::user_error(
        key,
        crate::pyutil::pypairs(vec![("time", eve_marshal::PyValue::Int(left))]),
    ))
}

fn bare(key: &'static str) -> crate::service::ServiceError {
    crate::service::ServiceError::Raise(eve_proto::user_error(key, Vec::new()))
}

/// The empire criminal's remaining time, when CONCORD is on: the condition
/// of the dock, gate and wormhole refusals, which run the whole criminal
/// timer [A] (falsified by a criminal who can dock on TQ).
fn grounded(w: &Registry, actor: Option<i64>, now: BlueTime) -> Option<BlueTime> {
    let c = actor?;
    if !w.ext::<Concord>().enabled {
        return None;
    }
    empire_system_of(w, c)?;
    criminal_for(w, c, now)
}

/// `CmdDock`'s refusal for a criminal. Runs before
/// `crimewatch::refuse_dock`: the criminal flag's own words replace the
/// weapons timer's.
pub fn refuse_dock(
    w: &Registry,
    actor: Option<i64>,
    now: BlueTime,
) -> Result<(), crate::service::ServiceError> {
    grounded(w, actor, now).map_or(Ok(()), |left| Err(timed(REFUSE_DOCK, left)))
}

/// `CmdStargateJump`'s refusal for a criminal.
pub fn refuse_jump(
    w: &Registry,
    actor: Option<i64>,
    now: BlueTime,
) -> Result<(), crate::service::ServiceError> {
    grounded(w, actor, now).map_or(Ok(()), |left| Err(timed(REFUSE_GATE, left)))
}

/// `WormholeJump`'s refusal for a criminal.
pub fn refuse_wormhole(
    w: &Registry,
    actor: Option<i64>,
    now: BlueTime,
) -> Result<(), crate::service::ServiceError> {
    grounded(w, actor, now).map_or(Ok(()), |left| Err(timed(REFUSE_WORMHOLE, left)))
}

/// **The warp guard** (a `movement_guard::WARP_GUARDS` row): a criminal with
/// a CONCORD response open cannot warp. Only then [A]: a low-sec criminal,
/// with no CONCORD coming, warps freely; falsified by the owner's memory of
/// retail. Reads the wall clock, as every guard does.
pub fn warp_guard(w: &Registry, character_id: i64) -> Option<crate::movement_guard::Refusal> {
    if !pursued(w, character_id) {
        return None;
    }
    let left = criminal_for(w, character_id, crate::pyutil::filetime_now())?;
    Some(crate::movement_guard::Refusal {
        user_error: REFUSE_WARP,
        args: crate::pyutil::pypairs(vec![("time", eve_marshal::PyValue::Int(left))]),
    })
}

/// A cyno or beacon jump. No jump drive exists here yet, so nothing calls
/// this: it is the refusal waiting for one.
pub fn refuse_beacon_jump(
    w: &Registry,
    actor: Option<i64>,
    now: BlueTime,
) -> Result<(), crate::service::ServiceError> {
    grounded(w, actor, now).map_or(Ok(()), |_| Err(bare(REFUSE_BEACON)))
}

/// Boarding a ship in space (impossible here yet, see
/// `service::inventory::board`): a criminal may not, and nobody may board a
/// ship a police response is engaging. Nothing calls it yet.
pub fn refuse_board(
    w: &Registry,
    actor: Option<i64>,
    engaged_by_police: bool,
    now: BlueTime,
) -> Result<(), crate::service::ServiceError> {
    if engaged_by_police && w.ext::<Concord>().enabled {
        return Err(bare(REFUSE_BOARD_POLICE));
    }
    grounded(w, actor, now).map_or(Ok(()), |_| Err(bare(REFUSE_BOARD)))
}

/// Cloak activation (no cloak is modelled yet; nothing calls this): refused
/// while a response pursues the pilot.
pub fn refuse_cloak(w: &Registry, character_id: i64) -> Result<(), crate::service::ServiceError> {
    if pursued(w, character_id) {
        Err(bare(REFUSE_CLOAK))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Faction police (NW-7, R-CONCORD section 5)
// ---------------------------------------------------------------------------

/// **[A]** The pause between a pilot entering a police net and the squad.
/// Falsified by a human who finds them too quick or too slow (H-C3).
pub const POLICE_DELAY: BlueTime = 10 * SEC;
/// **[A]** The squad gives up after this long. Falsified by the owner.
pub const POLICE_PURSUIT: BlueTime = 15 * 60 * SEC;
/// How often the sweep for pilots newly inside a net runs (a case already
/// open is looked at every tick).
pub const POLICE_SWEEP: BlueTime = SEC;
/// **[A]** The ring of a police squad: 10 to 15 km out, back to 10 km every
/// third ship.
pub const POLICE_RING_NEAR: f64 = 10_000.0;
pub const POLICE_RING_STEP: f64 = 2_500.0;
/// **[A]** How often an active squad checks that it is on the pilot's grid.
/// The police do not warp, so a pilot who warped off is rejoined by a fresh
/// squad at the landing, without a second warning. Falsified by a human who
/// warps away from police and finds them gone or late.
pub const POLICE_FOLLOW: BlueTime = 5 * SEC;
/// **[A]** At most this many faction police squads fly in one system; a
/// case that is due waits for a squad to go (BOT-LAW, review S2: a crowd of
/// outlaws in one system would otherwise raise three ships each). Falsified
/// by a soak that shows the broadcasts cheap (raise it), or by a crowded
/// system where outlaws go unpoliced for minutes (raise it).
pub const MAX_SQUADS_PER_SYSTEM: usize = 8;
/// **[A]** One tackling lead and this many plain ships.
pub const POLICE_PLAIN: usize = 2;

/// `warpScramble`, `modifyTargetSpeed2` and the ECM jam effects, which mark
/// a police type as the tackling lead [V-data].
pub const POLICE_TACKLE: [i64; 3] = [563, 575, 6_695];

/// The CONCORD Assembly's faction id: its `npc/police.tsv` row (the SWAT
/// ships) is the force of a system no faction polices, and its squads are
/// owned by `CONCORD_CORP`, so a SWAT kill stays a CONCORD kill for insurance.
pub const CONCORD_FACTION: i64 = 500_006;
/// **[A]** The warning for a force with no row, or none of its own. Falsified
/// by the owner, or by a client capture of a faction's police arrival.
pub const DEFAULT_WARNING: &str = "chatDefaultPoliceWarning";

/// Where a [`Case`] stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CasePhase {
    /// In the net; the squad is due at [`Case::due`].
    Pending,
    /// The squad is in space until [`Case::ends`].
    Active,
    /// The squad is gone (it gave up, or the ship died). The pilot is still
    /// in the net, so no second squad comes until they leave and return.
    Spent,
}

/// One pilot inside a police net.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub character: i64,
    pub system: i64,
    /// The ball they flew in when the net took them.
    pub ship: i64,
    pub due: BlueTime,
    pub ends: BlueTime,
    pub spawned: Vec<i64>,
    pub phase: CasePhase,
    /// When the squad next checks that it is still with the pilot
    /// ([`follow_pilot`]).
    pub next_follow: BlueTime,
    /// The squad went because the pilot is in a capsule (the ship died, or
    /// they ejected), not because the pursuit timed out: boarding a ship
    /// again brings it back ([`look_after`]).
    pub from_pod: bool,
    /// The warning has been sent, so a squad that comes back does not warn
    /// twice.
    pub warned: bool,
}

/// **Whether `owner` is a corporation that owns ships of the law**: CONCORD,
/// or the NPC corporation of a faction with a row in `npc/police.tsv`. The
/// bots read it to leave police alone unless the police shoot them.
pub fn is_law_owner(c: &crate::content::Content, owner: i64) -> bool {
    owner == CONCORD_CORP
        || c.try_get::<crate::npc_data::Table>().is_some_and(|t| {
            t.police
                .keys()
                .any(|f| t.factions.get(f).is_some_and(|x| x.corporation_id == owner))
        })
}

/// Who polices a system of `faction`, from `npc/police.tsv`: the owner, the
/// warning key and the types. A faction with no row is policed by the
/// CONCORD row; a world with neither (no bundle) gets no types, and so the
/// invented fixtures. The owner is the faction's own NPC corporation
/// (`npc/factions.tsv`), a real one the bots see as an NPC; CONCORD's row is
/// owned by `CONCORD_CORP`.
pub(crate) fn police_of(w: &Registry, faction: Option<i64>) -> (i64, String, Vec<i64>) {
    let Some(t) = w.content().try_get::<crate::npc_data::Table>() else {
        return (CONCORD_CORP, DEFAULT_WARNING.to_owned(), Vec::new());
    };
    let Some(row) = faction
        .and_then(|f| t.police.get(&f))
        .or_else(|| t.police.get(&CONCORD_FACTION))
    else {
        return (CONCORD_CORP, DEFAULT_WARNING.to_owned(), Vec::new());
    };
    let owner = if row.faction_id == CONCORD_FACTION {
        CONCORD_CORP
    } else {
        t.factions
            .get(&row.faction_id)
            .map_or(CONCORD_CORP, |f| f.corporation_id)
    };
    (owner, row.warning.clone(), row.types.clone())
}

/// The lead and the plain ship of a force, read from the typedata: the lead
/// is the first type that scrams, else the first that tackles at all, else
/// the first; the plain ship the first that does not tackle. With no bundle
/// a force the typedata lacks is the invented fixtures; with one it is
/// `None`, so a stale bundle spawns nothing rather than invented ships.
fn police_kinds(
    w: &mut Registry,
    types: &[i64],
    owner: i64,
) -> Option<(&'static NpcType, &'static NpcType)> {
    // (type, scrams, tackles)
    let marks: Vec<(i64, bool, bool)> = {
        let td = crate::typedata::installed_in(w.content()).table();
        types
            .iter()
            .filter_map(|t| {
                td.get(*t).map(|r| {
                    (
                        *t,
                        r.has_effect(POLICE_TACKLE[0]),
                        POLICE_TACKLE.iter().any(|e| r.has_effect(*e)),
                    )
                })
            })
            .collect()
    };
    let lead = marks
        .iter()
        .find(|m| m.1)
        .or_else(|| marks.iter().find(|m| m.2))
        .or(marks.first())
        .map(|m| m.0);
    let plain = marks.iter().find(|m| !m.2).or(marks.first()).map(|m| m.0);
    let bundle = w.content().is_bundle();
    let kind = |w: &mut Registry, t: Option<i64>, fixture: &'static NpcType| match t
        .and_then(|t| crate::npc::resolve_law(w, t, owner))
    {
        Some(k) => Some(k),
        None if bundle => None,
        None => Some(fixture),
    };
    Some((
        kind(w, lead, &FIXTURE_POLICE_LEAD)?,
        kind(w, plain, &FIXTURE_POLICE)?,
    ))
}

/// Where ship `i` of `n` in a police squad stands round `centre`.
pub fn police_ring(centre: [f64; 3], i: usize, n: usize) -> [f64; 3] {
    let angle = std::f64::consts::TAU * i as f64 / n.max(1) as f64;
    let radius = POLICE_RING_NEAR + POLICE_RING_STEP * (i % 3) as f64;
    [
        centre[0] + radius * angle.cos(),
        centre[1],
        centre[2] + radius * angle.sin(),
    ]
}

/// The pilot a police ship was sent for, if `ship` is one.
pub fn police_target(w: &Registry, ship: i64) -> Option<i64> {
    w.ext::<Concord>()
        .cases
        .values()
        .find(|c| c.spawned.contains(&ship))
        .map(|c| c.character)
}

/// **Is a volley at `victim` legal?** A faction police ship is fair game for
/// the pilot it came for and for any outlaw [I, retail lore]: it is no crime
/// event, even when the squad is CONCORD's own SWAT.
fn legal_on_police(w: &Registry, attacker: i64, victim: i64) -> bool {
    police_target(w, victim).is_some_and(|sent_for| {
        sent_for == attacker
            || crate::sec_status::is_outlaw(crate::npc::bounty::security_status(w, attacker))
    })
}

/// **Would a volley from `character` at `ball` be a crime against the law?**
/// A CONCORD or faction police ship (or a station or gate) in high sec, and
/// not the pilot the squad came for or an outlaw: [`attack_on_law`] would
/// flag them criminal. `crimewatch::attack_refusal` answers it with the
/// safety refusal, so a smartbomb cycle, drones on guard or assist, and the
/// modules all spare it at any safety below red.
///
/// **[V]** For CONCORD-owned ships, stations and gates green safety refuses
/// on the client too: `crimewatchSvc._GetMinimumSafetyLevelForNpc` returns
/// `shipSafetyLevelNone` for owner CONCORD, and `illegalTargetNpcOwnedGroups`
/// covers stations and gates.
///
/// **[A]** For faction police owned by a non-CONCORD corporation the client
/// falls through to `shipSafetyLevelFull` (group 182, no owner match), so our
/// refusal there is stricter than the client's. Falsified by a client whose
/// green safety lets a volley at such a police ship through (then drop the
/// refusal for them), or by `fci_concord` showing it refused.
pub fn illegal_on_law(w: &Registry, character: i64, ball: i64) -> bool {
    use crate::crimewatch::Space;
    if !w.ext::<Concord>().enabled || legal_on_police(w, character, ball) {
        return false;
    }
    law_act(w, ball).is_some()
        && w.session_system(character)
            .is_some_and(|s| crate::crimewatch::space_of(w, s) == Space::High)
}

/// **The police pass**, every tick: open cases are looked after, and once a
/// [`POLICE_SWEEP`] the hot systems are swept for pilots newly inside a net
/// (arrival by gate or undock, or a status that crossed a band while in
/// space; `sec_status::police_call` is the rule).
fn tick_police(w: &mut Registry, now: BlueTime) {
    if !w.ext::<Concord>().enabled {
        return;
    }
    let open: Vec<i64> = w.ext::<Concord>().cases.keys().copied().collect();
    for c in open {
        look_after(w, c, now);
    }
    if now < w.ext::<Concord>().next_sweep {
        return;
    }
    let every = crate::game_clock::span(w, POLICE_SWEEP);
    w.ext_mut::<Concord>().next_sweep = now + every;
    let systems: Vec<i64> = w.ext::<crate::liveness::Liveness>().hot().collect();
    let mut taken = Vec::new();
    for system in systems {
        for p in w.roster().in_system(system) {
            if w.ext::<Concord>().cases.contains_key(&p.character_id)
                || w.character(p.character_id).is_none()
                || w.pilot(p.character_id)
                    .is_none_or(|x| x.ship_type_id == crate::damage::CAPSULE_TYPE_ID)
            {
                continue;
            }
            if crate::sec_status::police_call(w, p.character_id).is_some() {
                taken.push((p.character_id, system, p.ship_id));
            }
        }
    }
    let delay = crate::game_clock::span(w, POLICE_DELAY);
    for (character, system, ship) in taken {
        w.ext_mut::<Concord>().cases.insert(
            character,
            Case {
                character,
                system,
                ship,
                due: now + delay,
                ends: 0,
                spawned: Vec::new(),
                phase: CasePhase::Pending,
                next_follow: 0,
                from_pod: false,
                warned: false,
            },
        );
    }
}

/// One case, one tick: the squad comes at its time, gives up at the end of
/// its pursuit, and goes when the pilot leaves the system, docks, loses the
/// ship or climbs out of the net.
fn look_after(w: &mut Registry, character: i64, now: BlueTime) {
    let Some(mut case) = w.ext::<Concord>().cases.get(&character).cloned() else {
        return;
    };
    let here = w.roster().get(character).copied();
    let in_system = here.is_some_and(|p| p.solar_system_id == case.system);
    let in_ship = in_system
        && here.is_some_and(|p| p.ship_id == case.ship)
        && w.pilot(character)
            .is_some_and(|x| x.ship_type_id != crate::damage::CAPSULE_TYPE_ID);
    let in_net = in_system && crate::sec_status::police_call(w, character).is_some();
    if !in_system || !in_net {
        // Left, docked, or no longer wanted: the case closes, and a return
        // is a fresh arrival.
        despawn_police(w, &mut case);
        w.ext_mut::<Concord>().cases.remove(&character);
        return;
    }
    let warping = here.is_some_and(|p| p.motion.warp_commanded());
    let pod = w
        .pilot(character)
        .is_some_and(|x| x.ship_type_id == crate::damage::CAPSULE_TYPE_ID);
    if !in_ship && pod {
        // The ship died: the squad's work is done; the pod is never theirs
        // [A].
        despawn_police(w, &mut case);
        case.ship = here.map_or(case.ship, |p| p.ship_id);
        case.phase = CasePhase::Spent;
        case.from_pod = true;
    } else {
        if !in_ship {
            // Another ship boarded in space: the case follows the pilot, so
            // changing hulls sheds no police.
            case.ship = here.map_or(case.ship, |p| p.ship_id);
            if case.from_pod && !pod {
                // Out of the capsule again (an eject and a reboard): the
                // squad comes back at once, without a second warning. A
                // pursuit that timed out stays spent.
                case.from_pod = false;
                case.phase = CasePhase::Pending;
                case.due = now;
            }
        }
        match case.phase {
            // A pilot in warp is nowhere to put a squad: it waits for the
            // landing.
            CasePhase::Pending if now + crate::npc::warp_in_lead(w) >= case.due && !warping => {
                spawn_police(w, &mut case, now)
            }
            CasePhase::Active if now >= case.ends => {
                despawn_police(w, &mut case);
                case.phase = CasePhase::Spent;
            }
            CasePhase::Active if !warping && now >= case.next_follow => {
                follow_pilot(w, &mut case, now)
            }
            _ => {}
        }
    }
    w.ext_mut::<Concord>().cases.insert(character, case);
}

/// The squad comes, once, with the faction's warning.
fn spawn_police(w: &mut Registry, case: &mut Case, now: BlueTime) {
    let c = w.ext::<Concord>();
    let flying = c
        .cases
        .values()
        .filter(|o| {
            o.system == case.system
                && o.phase == CasePhase::Active
                && o.spawned.iter().any(|id| w.npcs().get(*id).is_some())
        })
        .count();
    if flying >= c.max_squads {
        // This case is Pending, so only other pilots' squads are counted.
        // It stays due and is tried again at the next tick.
        return;
    }
    w.advance_character(case.character);
    let Some(centre) = w.roster().get(case.character).map(|p| p.motion.position) else {
        return;
    };
    let Some(call) = crate::sec_status::police_call(w, case.character) else {
        return;
    };
    let (owner, warning, types) = police_of(w, call.faction);
    let Some((lead, plain)) = police_kinds(w, &types, owner) else {
        // A bundle with typedata but no usable police row (a stale one):
        // no invented ships on a real client. The case is spent so the
        // error is logged once; regenerate with `tools/content_police.py`.
        crate::outln!(
            "[concord] ERROR: no police types for {} in {} (faction {:?}, row types {types:?}): \
             the bundle's npc/police.tsv is missing or stale; nothing spawned",
            case.character,
            case.system,
            call.faction
        );
        case.phase = CasePhase::Spent;
        return;
    };
    let mut kinds = vec![lead];
    kinds.extend(std::iter::repeat_n(plain, POLICE_PLAIN));
    let ids = place_squad(w, case, &kinds, centre, now);
    crate::outln!(
        "[concord] police: {} ships on {} in {} owner {} warning {warning} (t={now})",
        ids.len(),
        case.character,
        case.system,
        lead.owner_corp_id
    );
    if !case.warned {
        case.warned = true;
        w.schedule(
            std::time::Duration::ZERO,
            Notice::new(
                case.character,
                "OnRemoteMessage",
                vec![
                    crate::pyutil::bstr(&warning),
                    crate::pyutil::pydict(vec![(
                        "owner",
                        eve_marshal::PyValue::Int(case.character),
                    )]),
                ],
            ),
        );
    }
    case.spawned = ids;
    case.ends = now + crate::game_clock::span(w, POLICE_PURSUIT);
    case.next_follow = now + crate::game_clock::span(w, POLICE_FOLLOW);
    case.phase = CasePhase::Active;
}

/// `kinds` in a ring round `centre`, bound to the case's pilot, announced.
fn place_squad(
    w: &mut Registry,
    case: &Case,
    kinds: &[&'static NpcType],
    centre: [f64; 3],
    now: BlueTime,
) -> Vec<i64> {
    let n = kinds.len();
    let law = Law {
        offender: case.character,
    };
    let mut ids = Vec::new();
    // Early (before the due time) the squad warps in and lands on it; a
    // pilot back from a pod is answered at once, with no flight to add.
    let warp = now < case.due;
    for (i, kind) in kinds.iter().enumerate() {
        let at = police_ring(centre, i, n);
        let made = if warp {
            let out = [at[0] - centre[0], at[1] - centre[1], at[2] - centre[2]];
            crate::npc::spawn_bound_warping_in(w, kind, case.system, at, law, out)
        } else {
            crate::npc::spawn_bound(w, kind, case.system, at, law)
        };
        if let Some(id) = made {
            ids.push(id);
        }
    }
    crate::npc::announce(w, case.system, &ids, true);
    ids
}

/// **The squad rejoins a pilot who has warped off.** NPCs do not warp, so a
/// pilot landed off the squad's grid would be left alone for the rest of the
/// stay; every [`POLICE_FOLLOW`] the lead's distance is checked, and past
/// the follow range the survivors are moved to a fresh ring at the pilot
/// (the same balls, so damage and locks stay; no second warning).
///
/// **[A]** The follow range is the lead's own lock range
/// ([`follow_range`]): a lead that could still lock the pilot keeps the
/// chase, one that could not has lost them. Falsified by a human who warps
/// a few hundred km to a bookmark and finds the squad stranded, or flung
/// after them from a distance a lead could still have covered.
fn follow_pilot(w: &mut Registry, case: &mut Case, now: BlueTime) {
    case.next_follow = now + crate::game_clock::span(w, POLICE_FOLLOW);
    w.advance_character(case.character);
    let Some(centre) = w.roster().get(case.character).map(|p| p.motion.position) else {
        return;
    };
    case.spawned.retain(|id| w.npcs().get(*id).is_some());
    let Some(lead) = case.spawned.first().copied() else {
        return;
    };
    let range = follow_range(w, lead);
    let off = w
        .roster()
        .get(lead)
        .map(|p| p.motion.position)
        .is_some_and(|at| crate::space_state::distance(at, centre) > range);
    if !off {
        return;
    }
    let n = case.spawned.len();
    let moves: Vec<(i64, [f64; 3])> = case
        .spawned
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, police_ring(centre, i, n)))
        .collect();
    crate::npc::move_ids(w, case.system, &moves);
}

/// How far from the pilot the squad's lead gives up the chase on its own:
/// its lock range ([`follow_range_of`]), read off its ship type.
fn follow_range(w: &Registry, lead: i64) -> f64 {
    follow_range_of(w.roster().get(lead).map_or(0.0, |p| {
        w.max_target_range_of(w.pilot(p.character_id).map_or(0, |q| q.ship_type_id))
    }))
}

/// The follow range for a lead that locks out to `lock` metres: that, but
/// never under a fresh ring's reach (or the squad would be moved every
/// pass), and the grid-leave distance for a ship whose range is unknown.
fn follow_range_of(lock: f64) -> f64 {
    let ring = POLICE_RING_NEAR + 2.0 * POLICE_RING_STEP + 1_000.0;
    if lock > 0.0 {
        lock.max(ring)
    } else {
        crate::grid::GRID_LEAVE
    }
}

/// The squad leaves (`RemoveBalls`).
fn despawn_police(w: &mut Registry, case: &mut Case) {
    let ids = std::mem::take(&mut case.spawned);
    crate::npc::despawn_ids(w, case.system, &ids);
}

// ---------------------------------------------------------------------------
// Shooting the law (CONCORD-1b)
// ---------------------------------------------------------------------------

/// How long after a penalty the next one for shooting the law is swallowed:
/// the same minute `sec_status` gives a pilot-on-pilot repeat [A].
pub const LAW_SUPPRESS: BlueTime = 60 * SEC;

/// What a shot ball is to the law: a CONCORD ship, or a station or a
/// stargate; with its type for the history row.
fn law_act(w: &Registry, victim: i64) -> Option<(crate::sec_status::Act, i64)> {
    if let Some(p) = w.pilot(victim)
        && p.char_id.is_none()
        && (p.corporation_id == CONCORD_CORP || police_target(w, victim).is_some())
    {
        return Some((crate::sec_status::Act::AttackConcord, p.ship_type_id));
    }
    w.celestials()
        .get(victim)
        .filter(|c| {
            matches!(
                c.kind,
                crate::celestial::Kind::Station | crate::celestial::Kind::Gate
            )
        })
        .map(|c| (crate::sec_status::Act::AttackFacility, c.type_id))
}

/// **A capsuleer's volley at a CONCORD ship, a station or a gate** in high
/// sec is a criminal act. CW-1 does not classify NPC targets, so the call is
/// one line in `crimewatch::aggression`'s capsuleer-at-NPC arm. The act flags
/// the offender criminal (renewed by every volley), costs security status
/// once per [`LAW_SUPPRESS`] (`sec_status::on_crime` with the `AttackConcord`
/// or `AttackFacility` act, never the ship-aggression one), and opens the
/// response through this module's own row rather than `ON_CRIME`, which
/// would charge the status a second time. It cannot change the outcome.
pub fn attack_on_law(w: &mut Registry, attacker: i64, victim: i64, at: BlueTime, now: BlueTime) {
    use crate::crimewatch::Space;
    if !w.ext::<Concord>().enabled {
        return;
    }
    let Some(offender) = w.pilot(attacker).and_then(|p| p.char_id) else {
        return;
    };
    if legal_on_police(w, offender, victim) {
        return;
    }
    let Some((act, victim_type)) = law_act(w, victim) else {
        return;
    };
    let Some(p) = w.roster().get(offender).copied() else {
        return;
    };
    let system = p.solar_system_id;
    if crate::crimewatch::space_of(w, system) != Space::High {
        return;
    }
    crate::crimewatch::flag_criminal(w, offender, at, now);
    let window = crate::game_clock::span(w, LAW_SUPPRESS);
    let c = w.ext_mut::<Concord>();
    let fresh = c.law_hit.get(&offender).is_none_or(|t| at >= *t + window);
    if fresh {
        c.law_hit.insert(offender, at);
        crate::sec_status::on_crime(
            w,
            &crate::sec_status::Crime {
                offender,
                victim: None,
                act,
                system,
                victim_type,
                at,
            },
        );
    }
    on_crime(
        w,
        &CrimeEvent {
            offender,
            victim: None,
            kind: CrimeKind::Criminal,
            act: crate::crimewatch::ActKind::Attack,
            solar_system_id: system,
            space: Space::High,
            at,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crimewatch::{ActKind, Space, aggression};
    use eve_marshal::PyValue;

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const SHIP: [i64; 2] = [100_000_001, 100_000_002];
    /// An invented hull: the typedata fixture's frigate.
    const HULL: i64 = 990_000_001;
    const SYS: i64 = 30_000_003;
    const T0: BlueTime = 133_000_000_000_000_000;
    /// An invented security for the world, in the fastest band.
    const HIGH: f64 = 0.95;
    const LOW: f64 = 0.25;

    fn universe(security: f64) -> crate::celestial::Universe {
        universe_of(security, "")
    }

    /// The same, owned by `faction` (a column of the row; empty for none).
    fn universe_of(security: f64, faction: &str) -> crate::celestial::Universe {
        crate::celestial::Universe::parse(
            &[format!(
                "{SYS}\t19000001\t29000001\t\t{faction}\t{security}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx"
            )],
            &[],
            &[],
        )
        .expect("the invented row parses")
    }

    /// Alice and Bob in space in `SYS`, in invented hulls, in two player
    /// corporations.
    fn world(security: f64) -> Registry {
        world_in(universe(security))
    }

    fn world_in(universe: crate::celestial::Universe) -> Registry {
        world_with(universe, crate::npc_data::Table::fixture())
    }

    fn world_with(universe: crate::celestial::Universe, npc: crate::npc_data::Table) -> Registry {
        let mut content = crate::content::Content::fixture();
        content.insert("universe", universe);
        content.insert("npc_data", npc);
        world_on(content)
    }

    fn world_on(content: crate::content::Content) -> Registry {
        let pilot = |i: usize, id: i64| crate::model::Character {
            character_id: id,
            name: format!("pilot {id}"),
            ship_id: SHIP[i],
            ship_type_id: HULL,
            corporation_id: 98_000_001 + i as i64,
            balance: 1_000_000.0,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![pilot(0, ALICE), pilot(1, BOB)],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
        for (i, id) in [ALICE, BOB].into_iter().enumerate() {
            reg.character_mut(id).expect("a pilot").solar_system_id = Some(SYS);
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    id,
                    SHIP[i],
                    SYS,
                    crate::space_state::Motion::arrived([i as f64 * 5_000.0, 0.0, 0.0], [0.0; 3]),
                ));
        }
        let _ = reg.take_pending();
        reg
    }

    fn event(kind: CrimeKind, space: Space, at: BlueTime) -> CrimeEvent {
        CrimeEvent {
            offender: ALICE,
            victim: Some(BOB),
            kind,
            act: ActKind::Attack,
            solar_system_id: SYS,
            space,
            at,
        }
    }

    fn due_of(reg: &Registry) -> Option<BlueTime> {
        reg.ext::<Concord>().response(SYS, ALICE).map(|r| r.due)
    }

    fn pushes(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
        reg.take_due(Instant::now() + std::time::Duration::from_secs(1))
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (p.audience().unwrap_or_default(), name, args)
            })
            .collect()
    }

    fn mentions(v: &PyValue, want: &[u8]) -> bool {
        match v {
            PyValue::Bytes(b) => b.as_slice() == want,
            PyValue::List(items) | PyValue::Tuple(items) => items.iter().any(|i| mentions(i, want)),
            _ => false,
        }
    }

    fn law_ships(reg: &Registry) -> Vec<i64> {
        reg.npcs()
            .iter()
            .filter(|n| n.law.is_some())
            .map(|n| n.id)
            .collect()
    }

    // ---- the tables --------------------------------------------------------

    /// Every row of the delay table answers at its own floor, and a rounded
    /// security reads as the client rounds it.
    #[test]
    fn the_delay_table_answers_by_the_rounded_security() {
        for (floor, secs) in DELAYS {
            assert_eq!(delay_secs(*floor), *secs);
        }
        let (floor, secs) = DELAYS[2];
        assert_eq!(delay_secs(floor + 0.04), secs, "0.84 rounds to 0.8");
        assert_eq!(delay_secs(1.0), 1, "1 s in 1.0 (owner)");
        assert_eq!(delay_secs(0.5), 10, "10 s in 0.5 (owner)");
        assert!(
            DELAYS.windows(2).all(|p| p[0].1 <= p[1].1),
            "the lower the security, the longer the wait"
        );
        for (floor, c, k, o) in SQUADS {
            assert_eq!(squad(*floor), (*c, *k, *o));
        }
    }

    #[test]
    fn the_ring_keeps_every_ship_in_its_band_and_apart() {
        let centre = [1_000.0, 50.0, -2_000.0];
        let n = 5;
        let ps: Vec<[f64; 3]> = (0..n).map(|i| ring_position(centre, i, n)).collect();
        for p in &ps {
            let d = crate::space_state::distance(*p, centre);
            assert!(
                (RING_NEAR - 1.0..=RING_NEAR + 2.0 * RING_STEP + 1.0).contains(&d),
                "{d}"
            );
            assert_eq!(p[1], centre[1]);
        }
        for i in 0..n {
            for j in i + 1..n {
                assert!(crate::space_state::distance(ps[i], ps[j]) > 1_000.0);
            }
        }
    }

    // ---- the seam ------------------------------------------------------------

    /// **A criminal act in high sec opens one response**, due at the first
    /// crime plus the delay, and spawns nothing before; the offender is
    /// warned; a repeat does not push the due time later.
    #[test]
    fn a_criminal_act_in_high_sec_opens_one_response() {
        let mut reg = world(HIGH);
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        let delay = delay_secs(HIGH) * SEC;
        assert_eq!(due_of(&reg), Some(T0 + delay));
        assert!(law_ships(&reg).is_empty(), "nothing before the time");
        let out = pushes(&mut reg);
        let warn: Vec<_> = out
            .iter()
            .filter(|(to, n, _)| *n == "OnRemoteMessage" && to == &vec![ALICE])
            .collect();
        assert_eq!(warn.len(), 1);
        assert_eq!(
            warn[0].2[0],
            crate::pyutil::bstr(WARNING_KEY),
            "the client's own key"
        );
        // A repeat before the time: due stays.
        on_crime(
            &mut reg,
            &event(CrimeKind::Criminal, Space::High, T0 + 2 * SEC),
        );
        assert_eq!(due_of(&reg), Some(T0 + delay));
        assert_eq!(reg.ext::<Concord>().responses().count(), 1);
        assert!(
            pushes(&mut reg).is_empty(),
            "one warning per response, not per volley"
        );
    }

    /// **No response** for a suspect act, a low-sec crime, a null-sec one, an
    /// offender not in space, or with CONCORD switched off.
    #[test]
    fn suspects_low_sec_and_a_switch_open_no_response() {
        let mut reg = world(HIGH);
        on_crime(&mut reg, &event(CrimeKind::Suspect, Space::High, T0));
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::Low, T0));
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::Zero, T0));
        assert_eq!(reg.ext::<Concord>().responses().count(), 0);
        set_enabled(&mut reg, false);
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        assert_eq!(reg.ext::<Concord>().responses().count(), 0);
        set_enabled(&mut reg, true);
        reg.roster_mut().leave(ALICE);
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        assert_eq!(reg.ext::<Concord>().responses().count(), 0);
    }

    /// **Through CW-1's real classifier**: shooting in high sec makes a
    /// response; in low sec it does not (a suspect act); a duel and a war
    /// target are no crime, so CONCORD never hears of them.
    #[test]
    fn crimewatch_offers_only_what_is_a_crime() {
        let mut reg = world(HIGH);
        aggression(&mut reg, ALICE, BOB, T0, T0);
        assert!(reg.ext::<Concord>().response(SYS, ALICE).is_some());

        let mut reg = world(LOW);
        aggression(&mut reg, ALICE, BOB, T0, T0);
        assert_eq!(reg.ext::<Concord>().responses().count(), 0, "low sec");

        // A duel.
        let mut reg = world(HIGH);
        crate::crimewatch::engage(&mut reg, ALICE, BOB, T0, T0);
        aggression(&mut reg, ALICE, BOB, T0 + SEC, T0 + SEC);
        assert_eq!(reg.ext::<Concord>().responses().count(), 0, "a duel");

        // A war target.
        let mut reg = world(HIGH);
        let war = crate::war::War::new(1, 98_000_001, 98_000_002, T0 - 10 * SEC, SEC, None);
        crate::war::put(reg.deltas_mut(), &war);
        aggression(&mut reg, ALICE, BOB, T0, T0);
        assert_eq!(reg.ext::<Concord>().responses().count(), 0, "a war target");
    }

    // ---- the clock -----------------------------------------------------------

    /// One world tick as the clock runs it.
    fn step(reg: &mut Registry, now: BlueTime) {
        crate::combat::tick(reg, now);
        crate::npc::tick(reg, now);
        tick(reg, Instant::now(), now);
    }

    /// **At the time the squad comes, once**: the band's count, owner
    /// CONCORD, on the ring, in one `AddBalls2`; and not again.
    #[test]
    fn the_squad_spawns_once_at_the_time() {
        let mut reg = world(HIGH);
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        let _ = pushes(&mut reg);
        let delay = delay_secs(HIGH) * SEC;
        // The squad warps in: it starts one flight before the due time, so
        // it lands on it and the response time does not grow.
        let lead = crate::npc::warp_in_lead(&reg);
        assert!(lead > 0);
        tick(&mut reg, Instant::now(), T0 + delay - lead - 1);
        assert!(law_ships(&reg).is_empty(), "not a flight early");
        tick(&mut reg, Instant::now(), T0 + delay - lead);
        let (c, k, o) = squad(HIGH);
        let ids = law_ships(&reg);
        assert_eq!(ids.len() as u32, c + k + o);
        let centre = reg.roster().get(ALICE).expect("in space").motion.position;
        for id in &ids {
            let n = reg.npcs().get(*id).expect("an npc");
            assert_eq!(n.kind.owner_corp_id, CONCORD_CORP);
            assert_eq!(n.law, Some(Law { offender: ALICE }));
            assert_eq!(n.mind.home, None, "no leash");
            let m = reg.roster().get(*id).expect("placed").motion;
            let crate::destiny::Mode::Warp { goto, .. } = m.mode else {
                panic!("a squad that is early arrives in warp");
            };
            assert!(
                crate::space_state::distance(m.position, goto) > crate::npc::WARP_IN_DISTANCE - 1.0,
                "it starts its flight from afar"
            );
            let d = crate::space_state::distance(goto, centre);
            assert!((RING_NEAR - 1.0..=RING_NEAR + 2.0 * RING_STEP + 1.0).contains(&d));
        }
        let out = pushes(&mut reg);
        let added: Vec<_> = out
            .iter()
            .filter(|(_, n, _)| *n == "DoDestinyUpdate")
            .collect();
        assert_eq!(added.len(), 1, "one AddBalls2 for the whole squad");
        assert!(added[0].0.contains(&ALICE) && added[0].0.contains(&BOB));
        tick(&mut reg, Instant::now(), T0 + delay + SEC);
        assert_eq!(law_ships(&reg), ids, "once");
        assert_eq!(
            reg.ext::<Concord>().response(SYS, ALICE).map(|r| r.phase),
            Some(Phase::Active)
        );
    }

    /// **A ship of the law sees only its offender**: with two player ships in
    /// view it locks Alice and never Bob.
    #[test]
    fn a_ship_of_the_law_locks_only_its_offender() {
        let mut reg = world(HIGH);
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        let delay = delay_secs(HIGH) * SEC;
        let mut now = T0 + delay;
        // Both pilots sit inside the ring's reach.
        tick(&mut reg, Instant::now(), now);
        // The law locks at once (owner) and the volley may follow within the
        // second, so every lock seen along the way counts.
        let ids = law_ships(&reg);
        assert!(!ids.is_empty());
        let mut locked: Vec<i64> = Vec::new();
        let mut alice_died = false;
        for _ in 0..8 {
            now += SEC / 8;
            crate::npc::tick(&mut reg, now);
            locked.extend(ids.iter().flat_map(|id| reg.locks().targets_of(*id)));
            alice_died |= reg.roster().get(ALICE).is_none_or(|p| p.ship_id != SHIP[0]);
        }
        assert!(
            locked.contains(&SHIP[0]) || alice_died,
            "Alice is locked (or already dead)"
        );
        assert!(!locked.contains(&SHIP[1]), "Bob never");
        assert!(
            reg.roster().get(BOB).is_some_and(|p| p.ship_id == SHIP[1]),
            "Bob's ship is untouched"
        );
    }

    /// **The kill**: the commander's volley ends the offender's ship and the
    /// killmail's final blow is a CONCORD ship; the pod is not touched while
    /// the ships linger, and they leave after [`LINGER`].
    #[test]
    fn the_offenders_ship_dies_to_concord_and_they_leave() {
        let mut reg = world(HIGH);
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        let delay = delay_secs(HIGH) * SEC;
        let mut now = T0 + delay;
        let mut died = None;
        for i in 0..400 {
            step(&mut reg, now);
            if reg.roster().get(ALICE).is_none_or(|p| p.ship_id != SHIP[0]) {
                died = Some(i);
                break;
            }
            now += SEC / 4;
        }
        assert!(died.is_some(), "the ship was destroyed");
        let reports = crate::killmail::involving(&reg, ALICE, None, 5);
        assert_eq!(reports.len(), 1, "one killmail");
        let fb = &reports[0].final_blow;
        assert_eq!(fb.corporation, Some(CONCORD_CORP));
        assert_eq!(fb.character, None, "an NPC");
        assert!(FIXTURES.iter().any(|f| Some(f.type_id) == fb.ship_type));
        // Lingering, then gone.
        step(&mut reg, now + SEC);
        assert!(matches!(
            reg.ext::<Concord>().response(SYS, ALICE).map(|r| r.phase),
            Some(Phase::Lingering(_))
        ));
        assert!(!law_ships(&reg).is_empty(), "still here");
        let after = now + SEC + LINGER;
        tick(&mut reg, Instant::now(), after + SEC);
        assert!(law_ships(&reg).is_empty(), "gone after the linger");
        assert!(reg.ext::<Concord>().response(SYS, ALICE).is_none());
        let out = pushes(&mut reg);
        assert!(
            out.iter().any(|(_, n, args)| *n == "DoDestinyUpdate"
                && args.iter().any(|a| mentions(a, b"RemoveBalls"))),
            "RemoveBalls tells the system"
        );
    }

    /// **An offender who docks** loses the squad at once; the response waits
    /// and a new crime restarts the delay.
    #[test]
    fn a_docked_offender_sheds_the_squad_and_the_next_crime_restarts_the_delay() {
        let mut reg = world(HIGH);
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        let delay = delay_secs(HIGH) * SEC;
        tick(&mut reg, Instant::now(), T0 + delay);
        assert!(!law_ships(&reg).is_empty());
        let docked = reg.roster_mut().leave(ALICE).expect("was in space");
        tick(&mut reg, Instant::now(), T0 + delay + SEC);
        assert!(law_ships(&reg).is_empty(), "gone at once");
        assert_eq!(
            reg.ext::<Concord>().response(SYS, ALICE).map(|r| r.phase),
            Some(Phase::Away)
        );
        // Undocks and offends again: a fresh delay from that crime.
        reg.roster_mut().arrive(docked);
        let again = T0 + delay + 100 * SEC;
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, again));
        assert_eq!(due_of(&reg), Some(again + delay));
        assert_eq!(
            reg.ext::<Concord>().response(SYS, ALICE).map(|r| r.phase),
            Some(Phase::Pending)
        );
    }

    /// **Spans follow `--time-scale`**: at 10 times the clock the delay is a
    /// tenth.
    #[test]
    fn the_delay_runs_on_the_game_clock() {
        let mut reg = world(HIGH);
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(10).unwrap());
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        assert_eq!(due_of(&reg), Some(T0 + delay_secs(HIGH) * SEC / 10));
    }

    // ---- the effects -----------------------------------------------------------

    /// An invented type row with the CONCORD effects and invented numbers.
    fn td(effects: &str, attrs: &str) -> crate::typedata::Table {
        crate::typedata::Table::parse(&format!(
            "type\t990330010\t990301\t11\t1\t1\t0.0\t0.0\t1.0\t1.0\t0\t{effects}\t{attrs}\tFixture Law\n"
        ))
        .expect("parses")
    }

    #[test]
    fn the_concord_effect_ids_map_onto_the_existing_ewar() {
        let t = td(
            &format!("{EFFECT_SCRAM},{EFFECT_WEB}"),
            "103=21000.0;105=3.0;505=7000.0;20=-60.0;513=4000.0;514=19000.0",
        );
        let e = ewar_of(&t, 990_330_010);
        let s = e.scram.expect("a scram");
        assert_eq!(
            (s.kind, s.range, s.strength, s.period),
            (Kind::Scram, 21_000.0, 3.0, 70_000_000)
        );
        let b = e.web.expect("a web");
        assert_eq!(
            (b.kind, b.range, b.strength, b.period),
            (Kind::Web, 19_000.0, -60.0, 40_000_000)
        );
        assert_eq!(e.jam, None);
        // No effect, no mapping; a web that would not slow is nothing.
        assert_eq!(
            ewar_of(&td("", "103=1.0;105=1.0;505=1.0"), 990_330_010),
            Ewar::default()
        );
        let weak = td(&EFFECT_WEB.to_string(), "20=0.0;513=4000.0;514=19000.0");
        assert_eq!(ewar_of(&weak, 990_330_010).web, None);
    }

    /// **The jam** reads its factor from 928 and only a factor below one jams;
    /// the locks over the new cap go, and new ones are refused for the cycle.
    #[test]
    fn a_jam_wipes_the_locks_and_caps_new_ones_for_its_cycle() {
        let t = td(&EFFECT_JAM.to_string(), "936=200000.0;929=3000.0;928=0.0");
        let j = ewar_of(&t, 990_330_010).jam.expect("a jam");
        assert_eq!((j.factor, j.period), (0.0, 30_000_000));
        let lax = td(&EFFECT_JAM.to_string(), "936=200000.0;929=3000.0");
        assert_eq!(
            ewar_of(&lax, 990_330_010).jam,
            None,
            "default 1.0 jams nothing"
        );
        assert_eq!(jam_cap(5.0, 0.0), 0);
        assert_eq!(jam_cap(5.0, 0.5), 2);

        let mut reg = world(HIGH);
        // Alice holds a lock on Bob.
        assert!(reg.locks_mut().add(SHIP[0], SHIP[1]));
        let cop =
            crate::npc::spawn(&mut reg, &FIXTURE_CAPTAIN, SYS, [3_000.0, 0.0, 0.0]).expect("an id");
        let npc = *reg.npcs().get(cop).expect("npc");
        let _ = pushes(&mut reg);
        jam(&mut reg, &npc, SHIP[0], T0, &j);
        assert!(
            reg.locks().targets_of(SHIP[0]).is_empty(),
            "the lock is gone"
        );
        let out = pushes(&mut reg);
        assert_eq!(
            out.iter()
                .filter(|(to, n, _)| *n == "OnTarget" && to == &vec![ALICE])
                .count(),
            1,
            "Alice's box closes"
        );
        assert_eq!(jam_limit_at(&reg, SHIP[0], 5.0, T0 + j.period - 1), 0.0);
        assert_eq!(
            jam_limit_at(&reg, SHIP[0], 5.0, T0 + j.period),
            5.0,
            "the cycle ends"
        );
        assert_eq!(jam_limit_at(&reg, SHIP[1], 5.0, T0), 5.0, "only the target");
    }

    /// A pursuer with an effect cycles it on the offender through the
    /// existing scram hold.
    #[test]
    fn a_pursuer_with_a_scram_effect_holds_the_offender() {
        let t = td(&EFFECT_SCRAM.to_string(), "103=40000.0;105=3.0;505=7000.0");
        let e = ewar_of(&t, 990_330_010).scram.expect("a scram");
        let mut reg = world(HIGH);
        let cop =
            crate::npc::spawn(&mut reg, &FIXTURE_OFFICER, SYS, [3_000.0, 0.0, 0.0]).expect("an id");
        let npc = *reg.npcs().get(cop).expect("npc");
        reg.locks_mut().add(cop, SHIP[0]);
        crate::npc_ewar::cycle(&mut reg, &npc, &e, SHIP[0], T0);
        assert_eq!(crate::npc_ewar::incoming(&reg, SHIP[0], T0 + SEC), 3.0);
    }

    // ---- no escape (CONCORD-1b) --------------------------------------------

    /// The wall clock, for the guard that reads it.
    fn wall() -> BlueTime {
        crate::pyutil::filetime_now()
    }

    /// Alice criminal at `at` in `reg`, with the response a high-sec crime
    /// opens.
    fn criminal(reg: &mut Registry, space: Space, at: BlueTime) {
        crate::crimewatch::flag_criminal(reg, ALICE, at, at);
        on_crime(reg, &event(CrimeKind::Criminal, space, at));
    }

    fn key_and_time(e: crate::service::ServiceError) -> (String, Option<i64>) {
        let crate::service::ServiceError::Raise(v) = e else {
            panic!("a raised user error, got {e:?}");
        };
        let (key, dict) = user_error_parts(&v);
        // The wire's dict is `{value: key}` (`pyutil::pypairs`).
        let time = dict.iter().find_map(|(t, k)| match (t, k) {
            (PyValue::Int(t), PyValue::Bytes(k)) if k.as_slice() == b"time" => Some(*t),
            _ => None,
        });
        (key, time)
    }

    fn user_error_parts(v: &PyValue) -> (String, Vec<(PyValue, PyValue)>) {
        let PyValue::ObjectEx { header, .. } = v else {
            panic!("a user error, got {v:?}");
        };
        let PyValue::Tuple(h) = header.as_ref() else {
            panic!("header");
        };
        let PyValue::Tuple(a) = &h[1] else {
            panic!("args");
        };
        let (PyValue::Bytes(k), PyValue::Dict(d)) = (&a[0], &a[1]) else {
            panic!("key and dict");
        };
        (String::from_utf8(k.clone()).expect("utf8"), d.clone())
    }

    /// **Dock, gate and wormhole are refused for the whole criminal timer**,
    /// each in its own key with the time left; a pilot who is no criminal,
    /// and a world with CONCORD off, are not refused.
    #[test]
    fn a_criminal_is_refused_the_dock_the_gate_and_the_wormhole() {
        let mut reg = world(HIGH);
        let t = wall();
        let left = crate::crimewatch::CRIMINAL_TIMEOUT;
        assert!(refuse_dock(&reg, Some(ALICE), t).is_ok(), "not yet");
        crate::crimewatch::flag_criminal(&mut reg, ALICE, t, t);
        for (call, want) in [
            (refuse_dock(&reg, Some(ALICE), t), REFUSE_DOCK),
            (refuse_jump(&reg, Some(ALICE), t), REFUSE_GATE),
            (refuse_wormhole(&reg, Some(ALICE), t), REFUSE_WORMHOLE),
        ] {
            let (key, time) = key_and_time(call.expect_err("refused"));
            assert_eq!(key, want);
            assert_eq!(time, Some(left), "the whole timer is left");
        }
        // Part of the timer is gone: the time is what is left.
        let (_, time) = key_and_time(refuse_dock(&reg, Some(ALICE), t + 60 * SEC).unwrap_err());
        assert_eq!(time, Some(left - 60 * SEC));
        // Bob is no criminal; nobody at all is none.
        assert!(refuse_dock(&reg, Some(BOB), t).is_ok());
        assert!(refuse_dock(&reg, None, t).is_ok());
        // After the timer, free.
        assert!(refuse_dock(&reg, Some(ALICE), t + left).is_ok());
        // A suspect is not a criminal.
        crate::crimewatch::suspect(&mut reg, BOB, t);
        assert!(refuse_jump(&reg, Some(BOB), t).is_ok());
        set_enabled(&mut reg, false);
        assert!(refuse_dock(&reg, Some(ALICE), t).is_ok(), "CONCORD off");
    }

    /// A low-sec criminal is refused the dock too (any empire system) but
    /// the warp is only refused with a response open [A].
    #[test]
    fn the_warp_is_refused_only_while_a_response_is_open() {
        let t = wall();
        // Low sec: flagged, no response, so the warp is free; the dock not.
        let mut low = world(LOW);
        crate::crimewatch::flag_criminal(&mut low, ALICE, t, t);
        assert!(warp_guard(&low, ALICE).is_none());
        assert!(refuse_dock(&low, Some(ALICE), t).is_err());

        // High sec: the response is due, then in space.
        let mut reg = world(HIGH);
        assert!(warp_guard(&reg, ALICE).is_none(), "no crime, no refusal");
        criminal(&mut reg, Space::High, t);
        let r = warp_guard(&reg, ALICE).expect("pending refuses");
        assert_eq!(r.user_error, REFUSE_WARP);
        assert!(
            r.args.iter().any(|(v, k)| matches!(
                (v, k),
                (PyValue::Int(n), PyValue::Bytes(k)) if k.as_slice() == b"time" && *n > 0
            )),
            "the remaining time rides in `time`: {:?}",
            r.args
        );
        let due = delay_secs(HIGH) * SEC;
        tick(&mut reg, Instant::now(), t + due);
        assert!(!law_ships(&reg).is_empty());
        assert!(warp_guard(&reg, ALICE).is_some(), "active refuses");
        // The refusal is a row of the real table.
        assert!(
            crate::movement_guard::check(&crate::module::plan().warp_guards, &reg, ALICE).is_some()
        );
        // Bob is no offender.
        assert!(warp_guard(&reg, BOB).is_none());

        // The ship dies: the pod is free to go.
        crate::concord::on_kill(
            &mut reg,
            &crate::kill_hooks::KillEvent {
                victim_character: ALICE,
                victim_ship: SHIP[0],
                system: SYS,
                ship_type: HULL,
                killer: None,
                npc: None,
                when: t + due,
                park: vec![],
            },
        );
        assert!(
            warp_guard(&reg, ALICE).is_none(),
            "lingering is not pursuit"
        );

        let mut off = world(HIGH);
        criminal(&mut off, Space::High, t);
        set_enabled(&mut off, false);
        assert!(warp_guard(&off, ALICE).is_none());
    }

    /// Cyno, board and cloak refusals, and their keys.
    #[test]
    fn beacon_board_and_cloak_are_refused_under_pursuit() {
        let mut reg = world(HIGH);
        let t = wall();
        assert!(refuse_cloak(&reg, ALICE).is_ok());
        assert!(refuse_board(&reg, Some(ALICE), false, t).is_ok());
        criminal(&mut reg, Space::High, t);
        assert_eq!(
            key_and_time(refuse_cloak(&reg, ALICE).unwrap_err()).0,
            REFUSE_CLOAK
        );
        assert_eq!(
            key_and_time(refuse_board(&reg, Some(ALICE), false, t).unwrap_err()).0,
            REFUSE_BOARD
        );
        assert_eq!(
            key_and_time(refuse_beacon_jump(&reg, Some(ALICE), t).unwrap_err()).0,
            REFUSE_BEACON
        );
        // A ship the police are engaging is refused to anyone.
        assert_eq!(
            key_and_time(refuse_board(&reg, Some(BOB), true, t).unwrap_err()).0,
            REFUSE_BOARD_POLICE
        );
        assert!(refuse_board(&reg, Some(BOB), false, t).is_ok());
    }

    /// A squad ship of the invented fixture, spawned for Alice.
    fn squad_ship(reg: &mut Registry, at: BlueTime) -> i64 {
        on_crime(reg, &event(CrimeKind::Criminal, Space::High, at));
        tick(reg, Instant::now(), at + delay_secs(HIGH) * SEC);
        *law_ships(reg).first().expect("a squad ship")
    }

    /// **Shooting CONCORD is a crime**: the shooter is flagged criminal, pays
    /// the police-aggression status once a minute, and a response opens.
    /// Shooting anything else, or shooting from low sec, is nothing.
    #[test]
    fn shooting_concord_is_a_criminal_act_in_high_sec() {
        let mut reg = world(HIGH);
        let cop = squad_ship(&mut reg, T0);
        // Start fresh: Bob (no crime of his own) shoots a CONCORD ship.
        let now = T0 + 100 * SEC;
        assert!(!crate::crimewatch::is_criminal(&reg, BOB, now));
        let before = crate::sec_status::transactions(&reg, BOB).len();
        attack_on_law(&mut reg, BOB, cop, now, now);
        assert!(crate::crimewatch::is_criminal(&reg, BOB, now), "flagged");
        assert!(
            reg.ext::<Concord>().response(SYS, BOB).is_some(),
            "answered"
        );
        let after = crate::sec_status::transactions(&reg, BOB);
        assert_eq!(after.len(), before + 1, "a penalty row");
        // Every volley renews the flag but the status is charged once a minute.
        attack_on_law(&mut reg, BOB, cop, now + 5 * SEC, now + 5 * SEC);
        assert_eq!(crate::sec_status::transactions(&reg, BOB).len(), before + 1);
        let later = now + LAW_SUPPRESS + SEC;
        attack_on_law(&mut reg, BOB, cop, later, later);
        assert_eq!(crate::sec_status::transactions(&reg, BOB).len(), before + 2);
    }

    #[test]
    fn shooting_anything_else_or_from_low_sec_is_no_crime_against_the_law() {
        let mut reg = world(HIGH);
        let cop = squad_ship(&mut reg, T0);
        // A pilot's ship is CW-1's to classify, not this adapter's.
        attack_on_law(&mut reg, BOB, SHIP[0], T0 + SEC, T0 + SEC);
        assert!(!crate::crimewatch::is_criminal(&reg, BOB, T0 + SEC));
        // An NPC (the law) shooting the law is nobody's crime.
        attack_on_law(&mut reg, cop, cop, T0 + SEC, T0 + SEC);
        assert!(reg.ext::<Concord>().response(SYS, BOB).is_none());
        // CONCORD off.
        set_enabled(&mut reg, false);
        attack_on_law(&mut reg, BOB, cop, T0 + SEC, T0 + SEC);
        assert!(!crate::crimewatch::is_criminal(&reg, BOB, T0 + SEC));
    }

    /// The server's `Config` defaults to CONCORD on.
    #[test]
    fn the_server_config_has_concord_on_by_default() {
        assert!(crate::server::Config::new("127.0.0.1:0".parse().unwrap()).concord);
    }

    // ---- the gate checkpoints (lane concord-gate) ------------------------

    /// Invented gates: `(id, x)`, radius 5 km, each with an invented partner.
    const GATES: [(i64, f64); 2] = [(50_000_001, 100_000.0), (50_000_002, 40_000_000.0)];
    const GATE_RADIUS: f64 = 5_000.0;

    fn gate_world(security: f64) -> Registry {
        let mut text = String::new();
        for (id, x) in GATES {
            text += &format!(
                "gate\t{id}\t990001\t{GATE_RADIUS}\t{x}\t0\t0\t{SYS}\t{}\t30000099\n",
                id + 1000
            );
        }
        let pilot = |i: usize, id: i64| crate::model::Character {
            character_id: id,
            name: format!("pilot {id}"),
            ship_id: SHIP[i],
            ship_type_id: HULL,
            corporation_id: 98_000_001 + i as i64,
            balance: 1_000_000.0,
            ..crate::model::Character::default()
        };
        let mut content = crate::content::Content::fixture();
        content.insert("universe", universe(security));
        let celestials = crate::celestial::Catalogue::parse(&text);
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![pilot(0, ALICE), pilot(1, BOB)],
            celestials,
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
        for (i, id) in [ALICE, BOB].into_iter().enumerate() {
            reg.character_mut(id).expect("a pilot").solar_system_id = Some(SYS);
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    id,
                    SHIP[i],
                    SYS,
                    crate::space_state::Motion::arrived([i as f64 * 5_000.0, 0.0, 0.0], [0.0; 3]),
                ));
        }
        let _ = reg.take_pending();
        reg
    }

    /// One tick of the world as far as these tests need: liveness, then the
    /// checkpoint pass.
    fn live(reg: &mut Registry, now: BlueTime) {
        crate::liveness::refresh(reg, Instant::now(), now);
        tick(reg, Instant::now(), now);
    }

    fn standing(reg: &Registry) -> Vec<i64> {
        reg.npcs()
            .iter()
            .filter(|n| n.law == Some(STANDBY))
            .map(|n| n.id)
            .collect()
    }

    /// **[E] The group counts by displayed security**, and the 0.5 floor.
    #[test]
    fn groups_per_gate_follow_the_displayed_security() {
        for (sec, n) in [(1.0, 3), (0.95, 3), (0.9, 2), (0.8, 2), (0.7, 1), (0.5, 1)] {
            assert_eq!(groups_per_gate(sec), n, "{sec}");
            assert!(holds_checkpoints(sec));
        }
        assert!(holds_checkpoints(0.45), "0.45 displays as 0.5");
        assert!(!holds_checkpoints(0.44));
        assert!(!holds_checkpoints(0.0));
    }

    /// A group is two officers and at most one draw from each pool, every
    /// draw a pool member; the draw is deterministic and reaches every
    /// outcome.
    #[test]
    fn a_group_holds_officers_and_at_most_one_of_each_pool() {
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..400u64 {
            let g = group_types(seed);
            assert_eq!(g, group_types(seed));
            assert_eq!(g.iter().filter(|t| **t == OFFICER).count(), GROUP_OFFICERS);
            for (light, heavy) in SPECIAL_POOLS {
                let n = g.iter().filter(|t| *t == light || *t == heavy).count();
                assert!(n <= 1);
            }
            assert!(g.len() <= GROUP_OFFICERS + SPECIAL_POOLS.len());
            seen.extend(g);
        }
        for (light, heavy) in SPECIAL_POOLS {
            assert!(seen.contains(light) && seen.contains(heavy));
        }
    }

    /// **Every served gate of a high-sec system with a pilot** gets its
    /// groups, each ship within the surface distance, spread and spacing of
    /// the gate; a low-sec system gets none.
    #[test]
    fn checkpoints_stand_at_every_gate_by_security() {
        for sec in [1.0, 0.9, 0.8, 0.6] {
            let mut reg = gate_world(sec);
            live(&mut reg, T0);
            let posts: Vec<_> = reg.ext::<Concord>().posts(SYS).collect();
            assert_eq!(
                posts.len() as u32,
                GATES.len() as u32 * groups_per_gate(sec)
            );
            for ((_, gate, _), g) in &posts {
                assert!(g.ids.len() >= GROUP_OFFICERS);
                let centre = GATES.iter().find(|(id, _)| id == gate).map(|(_, x)| *x);
                for id in &g.ids {
                    let n = reg.npcs().get(*id).expect("an npc");
                    assert_eq!(n.kind.owner_corp_id, CONCORD_CORP);
                    assert_eq!(n.law, Some(STANDBY));
                    let at = reg.roster().get(*id).expect("placed").motion.position;
                    let d = crate::space_state::distance(at, [centre.expect("a gate"), 0.0, 0.0]);
                    let lo = GATE_RADIUS + CHECKPOINT_DISTANCE - CHECKPOINT_SPREAD;
                    let hi = GATE_RADIUS
                        + CHECKPOINT_DISTANCE
                        + CHECKPOINT_SPREAD
                        + CHECKPOINT_SPACING_CAP;
                    assert!((lo - 1.0..=hi + 1.0).contains(&d), "{d}");
                }
            }
            let out = pushes(&mut reg);
            assert!(
                out.iter().any(|(_, n, _)| *n == "DoDestinyUpdate"),
                "the pilots are told"
            );
            // Nothing is rebuilt while it stands.
            let before = standing(&reg);
            live(&mut reg, T0 + 10 * SEC);
            assert_eq!(standing(&reg), before);
        }
        let mut reg = gate_world(0.4);
        live(&mut reg, T0);
        assert!(standing(&reg).is_empty(), "low sec holds none");
        let mut reg = gate_world(HIGH);
        set_enabled(&mut reg, false);
        live(&mut reg, T0);
        assert!(standing(&reg).is_empty(), "--no-concord holds none");
    }

    /// Standing ships keep to their post: a pilot beside them is not hunted.
    #[test]
    fn standing_ships_do_not_hunt() {
        let mut reg = gate_world(HIGH);
        live(&mut reg, T0);
        let first = *standing(&reg).first().expect("a ship");
        let at = reg.roster().get(first).expect("placed").motion.position;
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .position = [at[0] + 3_000.0, at[1], at[2]];
        let mut now = T0;
        for _ in 0..40 {
            now += SEC / 4;
            step(&mut reg, now);
        }
        assert!(reg.npcs().iter().all(|n| n.target.is_none()));
        assert_eq!(reg.roster().get(ALICE).map(|p| p.ship_id), Some(SHIP[0]));
    }

    /// **A crime on the checkpoint's grid is answered at once**: the response
    /// is already active with no delay, every standing ship in reach
    /// has the offender, the commanders came with them, and the ship dies.
    #[test]
    fn a_crime_on_the_gate_grid_is_answered_with_no_delay() {
        let mut reg = gate_world(HIGH);
        live(&mut reg, T0);
        let stood = standing(&reg);
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        let r = reg
            .ext::<Concord>()
            .response(SYS, ALICE)
            .cloned()
            .expect("open");
        assert_eq!(r.phase, Phase::Active, "no wait");
        assert_eq!(r.due, T0);
        // The first gate's groups answer; the far gate's stand.
        let law: Vec<i64> = reg
            .npcs()
            .iter()
            .filter(|n| n.law == Some(Law { offender: ALICE }))
            .map(|n| n.id)
            .collect();
        assert!(!law.is_empty());
        let (commanders, _, _) = squad(HIGH);
        assert_eq!(
            law.iter().filter(|id| !stood.contains(id)).count() as u32,
            commanders,
            "the commanders are new"
        );
        for id in &r.spawned {
            assert!(law.contains(id));
        }
        let far = standing(&reg);
        assert!(!far.is_empty(), "the far gate's groups still stand");
        for id in &far {
            let x = reg.roster().get(*id).expect("placed").motion.position[0];
            assert!(x > 1_000_000.0, "only the far gate's ships stand");
        }
        // The same tick they lock; the ship dies.
        let mut now = T0;
        let mut died = false;
        for _ in 0..400 {
            step(&mut reg, now);
            if reg.roster().get(ALICE).is_none_or(|p| p.ship_id != SHIP[0]) {
                died = true;
                break;
            }
            now += SEC / 4;
        }
        assert!(died, "the ship was destroyed");
        let reports = crate::killmail::involving(&reg, ALICE, None, 5);
        assert_eq!(reports[0].final_blow.corporation, Some(CONCORD_CORP));
    }

    /// **No pod kill**: while the ships linger the capsule is untouched.
    #[test]
    fn the_checkpoint_does_not_pod() {
        let mut reg = gate_world(HIGH);
        live(&mut reg, T0);
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        let mut now = T0;
        for _ in 0..400 {
            step(&mut reg, now);
            if reg.roster().get(ALICE).is_none_or(|p| p.ship_id != SHIP[0]) {
                break;
            }
            now += SEC / 4;
        }
        for _ in 0..40 {
            now += SEC / 4;
            step(&mut reg, now);
        }
        assert!(
            reg.npcs().iter().all(|n| n.target.is_none()),
            "nothing is locked on the pod"
        );
        assert_eq!(
            crate::killmail::involving(&reg, ALICE, None, 5).len(),
            1,
            "the ship's killmail only"
        );
    }

    /// A crime away from every post takes the ordinary delay.
    #[test]
    fn a_crime_off_the_gate_grid_takes_the_delay() {
        let mut reg = gate_world(HIGH);
        live(&mut reg, T0);
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .position = [-20_000_000.0, 0.0, 0.0];
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        let r = reg.ext::<Concord>().response(SYS, ALICE).expect("open");
        assert_eq!(r.phase, Phase::Pending);
        assert_eq!(r.due, T0 + delay_secs(HIGH) * SEC);
        assert!(reg.npcs().iter().all(|n| n.law == Some(STANDBY)));
    }

    /// **Despawn and respawn**: when the response ends its ships leave, the
    /// lost groups come back [`CHECKPOINT_RESPAWN`] later on their posts, and
    /// not before.
    #[test]
    fn a_lost_group_is_rebuilt_after_the_respawn_wait() {
        let mut reg = gate_world(0.6);
        live(&mut reg, T0);
        let before: Vec<_> = reg.ext::<Concord>().posts(SYS).map(|(k, _)| *k).collect();
        on_crime(&mut reg, &event(CrimeKind::Criminal, Space::High, T0));
        let mut now = T0;
        for _ in 0..400 {
            step(&mut reg, now);
            if reg.roster().get(ALICE).is_none_or(|p| p.ship_id != SHIP[0]) {
                break;
            }
            now += SEC / 4;
        }
        let end = now + SEC + LINGER;
        step(&mut reg, now + SEC);
        live(&mut reg, end + SEC);
        assert!(
            reg.ext::<Concord>().response(SYS, ALICE).is_none(),
            "the response is over"
        );
        let gone: Vec<_> = reg
            .ext::<Concord>()
            .posts(SYS)
            .filter(|(_, g)| g.ids.is_empty())
            .map(|(k, _)| *k)
            .collect();
        assert!(!gone.is_empty(), "the answering group is down");
        live(&mut reg, end + SEC + CHECKPOINT_RESPAWN - 1);
        assert!(
            reg.ext::<Concord>()
                .posts(SYS)
                .any(|(_, g)| g.ids.is_empty()),
            "not before the wait"
        );
        live(&mut reg, end + SEC + CHECKPOINT_RESPAWN + 1);
        let after: Vec<_> = reg.ext::<Concord>().posts(SYS).map(|(k, _)| *k).collect();
        assert_eq!(before, after);
        assert!(
            reg.ext::<Concord>()
                .posts(SYS)
                .all(|(_, g)| !g.ids.is_empty())
        );
        assert!(!standing(&reg).is_empty());
    }

    /// A system nobody is in loses its checkpoints after the vacancy, and
    /// they come back with the next pilot.
    #[test]
    fn an_empty_system_loses_its_checkpoints() {
        let mut reg = gate_world(HIGH);
        live(&mut reg, T0);
        assert!(!standing(&reg).is_empty());
        for id in [ALICE, BOB] {
            reg.roster_mut().leave(id);
        }
        live(&mut reg, T0 + SEC);
        assert!(!standing(&reg).is_empty(), "not at once");
        live(&mut reg, T0 + SEC + CHECKPOINT_VACANT);
        assert!(standing(&reg).is_empty());
        assert_eq!(reg.ext::<Concord>().posts(SYS).count(), 0);
    }

    // ---- faction police (NW-7) -------------------------------------------------

    /// The fixture empire's faction id (`npc_data::Table::fixture`), whose
    /// police row and corporation are invented.
    const EMPIRE: &str = "599002";
    /// Its invented warning key.
    const EMPIRE_WARNING: &str = "chatFixturePoliceWarning";
    /// A status that puts the pilot in the 0.8 and up net.
    const WANTED: f64 = -3.0;

    fn net_world(security: f64, faction: &str, status: f64) -> Registry {
        let mut reg = world_in(universe_of(security, faction));
        crate::npc::bounty::set_security_status(&mut reg, ALICE, status);
        let _ = reg.take_pending();
        reg
    }

    fn police_ships(reg: &Registry) -> Vec<i64> {
        reg.ext::<Concord>()
            .case(ALICE)
            .map(|c| c.spawned.clone())
            .unwrap_or_default()
    }

    fn phase_of(reg: &Registry) -> Option<CasePhase> {
        reg.ext::<Concord>().case(ALICE).map(|c| c.phase)
    }

    /// **The net takes a pilot, the squad comes after the pause**, once, as
    /// one lead and two plain ships of the faction's corporation, ringed
    /// 10 to 15 km out, with the faction's warning and no second squad.
    #[test]
    fn a_pilot_in_the_net_gets_a_squad_after_the_pause() {
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        live(&mut reg, T0);
        assert_eq!(phase_of(&reg), Some(CasePhase::Pending));
        assert_eq!(
            reg.ext::<Concord>().case(BOB),
            None,
            "the clean are left alone"
        );
        assert!(law_ships(&reg).is_empty());
        let lead = crate::npc::warp_in_lead(&reg);
        live(&mut reg, T0 + POLICE_DELAY - lead - 1);
        assert!(law_ships(&reg).is_empty(), "not a flight early");
        let _ = pushes(&mut reg);
        live(&mut reg, T0 + POLICE_DELAY - lead);
        assert_eq!(phase_of(&reg), Some(CasePhase::Active));
        let ships = police_ships(&reg);
        assert_eq!(ships.len(), 1 + POLICE_PLAIN);
        let centre = reg.roster().get(ALICE).expect("in space").motion.position;
        for id in &ships {
            let n = reg.npcs().get(*id).expect("an npc");
            assert_eq!(n.law, Some(Law { offender: ALICE }));
            assert_eq!(n.kind.owner_corp_id, FIXTURE_POLICE_CORP);
            let crate::destiny::Mode::Warp { goto, .. } =
                reg.roster().get(*id).expect("a ball").motion.mode
            else {
                panic!("a squad that is early arrives in warp");
            };
            let d = crate::space_state::distance(goto, centre);
            assert!(
                (POLICE_RING_NEAR - 1.0..=POLICE_RING_NEAR + 2.0 * POLICE_RING_STEP + 1.0)
                    .contains(&d),
                "{d}"
            );
        }
        assert_eq!(
            ships
                .iter()
                .filter(|id| reg
                    .npcs()
                    .get(**id)
                    .is_some_and(|n| n.kind.type_id == FIXTURE_POLICE_LEAD.type_id))
                .count(),
            1
        );
        let out = pushes(&mut reg);
        let warn: Vec<_> = out
            .iter()
            .filter(|(to, n, _)| *n == "OnRemoteMessage" && to == &vec![ALICE])
            .collect();
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].2[0], crate::pyutil::bstr(EMPIRE_WARNING));
        live(&mut reg, T0 + POLICE_DELAY + 5 * SEC);
        assert_eq!(police_ships(&reg), ships, "once");
        assert!(
            pushes(&mut reg)
                .iter()
                .all(|(_, n, _)| *n != "OnRemoteMessage")
        );
    }

    /// A system no faction owns is policed all the same, with a warning.
    #[test]
    fn an_unowned_system_gets_a_squad_and_a_warning() {
        let mut reg = net_world(0.9, "", WANTED);
        live(&mut reg, T0);
        let _ = pushes(&mut reg);
        live(&mut reg, T0 + POLICE_DELAY);
        let out = pushes(&mut reg);
        assert!(out.iter().any(|(_, n, _)| *n == "OnRemoteMessage"));
        assert_eq!(police_ships(&reg).len(), 1 + POLICE_PLAIN);
    }

    /// **Only the net**: a status above -2.0, a system under the status's
    /// band, a capsule, a switched-off CONCORD and a pilot in a station
    /// draw no case.
    #[test]
    fn nobody_outside_the_net_is_policed() {
        // Clean status in the safest system.
        let mut reg = net_world(1.0, EMPIRE, -1.0);
        live(&mut reg, T0);
        assert_eq!(phase_of(&reg), None);
        // -3.0 reaches 0.8 and up, not 0.7.
        let mut reg = net_world(0.7, EMPIRE, WANTED);
        live(&mut reg, T0);
        assert_eq!(phase_of(&reg), None, "under the band");
        // The deepest band reaches 0.5.
        let mut reg = net_world(0.5, EMPIRE, -4.6);
        live(&mut reg, T0);
        assert_eq!(phase_of(&reg), Some(CasePhase::Pending));
        // Low sec has no police.
        let mut reg = net_world(0.4, EMPIRE, -9.0);
        live(&mut reg, T0);
        assert_eq!(phase_of(&reg), None, "low sec");
        // The pod is never theirs.
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        reg.character_mut(ALICE).expect("a pilot").ship_type_id = crate::damage::CAPSULE_TYPE_ID;
        live(&mut reg, T0);
        assert_eq!(phase_of(&reg), None, "a capsule");
        // Switched off.
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        set_enabled(&mut reg, false);
        live(&mut reg, T0);
        assert_eq!(phase_of(&reg), None, "off");
        // Docked: not in space.
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        reg.roster_mut().leave(ALICE);
        live(&mut reg, T0);
        assert_eq!(phase_of(&reg), None, "in a station");
    }

    /// **A status that crosses a band while in space** brings the police.
    #[test]
    fn a_status_that_crosses_a_band_brings_the_police() {
        let mut reg = net_world(0.9, EMPIRE, -1.0);
        live(&mut reg, T0);
        assert_eq!(phase_of(&reg), None);
        crate::npc::bounty::set_security_status(&mut reg, ALICE, WANTED);
        live(&mut reg, T0 + 2 * SEC);
        assert_eq!(phase_of(&reg), Some(CasePhase::Pending));
        live(&mut reg, T0 + 2 * SEC + POLICE_DELAY);
        assert_eq!(police_ships(&reg).len(), 1 + POLICE_PLAIN);
        // And back out of it: they go.
        crate::npc::bounty::set_security_status(&mut reg, ALICE, -1.0);
        live(&mut reg, T0 + 3 * SEC + POLICE_DELAY);
        assert_eq!(phase_of(&reg), None);
        assert!(law_ships(&reg).is_empty());
    }

    /// **They go** when the pilot leaves the system or docks (and come anew
    /// on the return), and give up after the pursuit, without a second squad
    /// while the pilot stays.
    #[test]
    fn the_squad_goes_when_the_pilot_does_or_after_the_pursuit() {
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        live(&mut reg, T0);
        live(&mut reg, T0 + POLICE_DELAY);
        assert_eq!(police_ships(&reg).len(), 1 + POLICE_PLAIN);
        let now = T0 + POLICE_DELAY + 10 * SEC;
        // Docks.
        let arrival = |reg: &mut Registry| {
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    ALICE,
                    SHIP[0],
                    SYS,
                    crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
                ));
        };
        reg.roster_mut().leave(ALICE);
        live(&mut reg, now);
        assert_eq!(phase_of(&reg), None);
        assert!(law_ships(&reg).is_empty(), "RemoveBalls at once");
        // Undocks: a fresh pause, a fresh squad.
        arrival(&mut reg);
        live(&mut reg, now + SEC);
        assert_eq!(phase_of(&reg), Some(CasePhase::Pending));
        live(&mut reg, now + SEC + POLICE_DELAY);
        assert_eq!(police_ships(&reg).len(), 1 + POLICE_PLAIN);
        // The pursuit ends.
        let given_up = now + SEC + POLICE_DELAY + POLICE_PURSUIT;
        live(&mut reg, given_up - 1);
        assert_eq!(phase_of(&reg), Some(CasePhase::Active));
        live(&mut reg, given_up);
        assert_eq!(phase_of(&reg), Some(CasePhase::Spent));
        assert!(law_ships(&reg).is_empty());
        live(&mut reg, given_up + 30 * SEC);
        assert!(law_ships(&reg).is_empty(), "not twice for one stay");
        // A jump away and back is a new arrival.
        reg.roster_mut().leave(ALICE);
        live(&mut reg, given_up + 31 * SEC);
        arrival(&mut reg);
        live(&mut reg, given_up + 32 * SEC);
        assert_eq!(phase_of(&reg), Some(CasePhase::Pending));
    }

    /// The ship dying ends the squad's work; the pod is not pursued and the
    /// pilot, still in the net, gets no second squad.
    #[test]
    fn a_dead_ship_ends_the_squad_and_the_pod_is_left_alone() {
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        live(&mut reg, T0);
        live(&mut reg, T0 + POLICE_DELAY);
        assert_eq!(police_ships(&reg).len(), 1 + POLICE_PLAIN);
        // The ship goes; the pod comes up in its place.
        reg.roster_mut().leave(ALICE);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ALICE,
                SHIP[0] + 50,
                SYS,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        reg.character_mut(ALICE).expect("a pilot").ship_type_id = crate::damage::CAPSULE_TYPE_ID;
        live(&mut reg, T0 + POLICE_DELAY + SEC);
        assert!(law_ships(&reg).is_empty());
        assert_eq!(phase_of(&reg), Some(CasePhase::Spent));
        live(&mut reg, T0 + POLICE_DELAY + 60 * SEC);
        assert!(law_ships(&reg).is_empty(), "no second squad for the pod");
    }

    /// **Attacking police is no crime** for the pilot they came for or for
    /// an outlaw, even when the ship is CONCORD's own; a clean pilot who
    /// shoots a CONCORD ship still is a criminal.
    #[test]
    fn attacking_police_is_no_crime_for_the_wanted_or_an_outlaw() {
        let mut reg = world(HIGH);
        let cop = squad_ship(&mut reg, T0);
        reg.ext_mut::<Concord>().cases.insert(
            ALICE,
            Case {
                character: ALICE,
                system: SYS,
                ship: SHIP[0],
                due: T0,
                ends: T0 + POLICE_PURSUIT,
                spawned: vec![cop],
                phase: CasePhase::Active,
                next_follow: T0,
                from_pod: false,
                warned: true,
            },
        );
        let at = T0 + 30 * SEC;
        attack_on_law(&mut reg, ALICE, cop, at, at);
        assert!(
            !crate::crimewatch::is_criminal(&reg, ALICE, at),
            "the pilot they came for"
        );
        // A clean third party who shoots them is not covered.
        attack_on_law(&mut reg, BOB, cop, at, at);
        assert!(crate::crimewatch::is_criminal(&reg, BOB, at));
        // An outlaw is.
        let mut reg2 = world(HIGH);
        let cop2 = squad_ship(&mut reg2, T0);
        reg2.ext_mut::<Concord>().cases.insert(
            ALICE,
            Case {
                character: ALICE,
                system: SYS,
                ship: SHIP[0],
                due: T0,
                ends: T0 + POLICE_PURSUIT,
                spawned: vec![cop2],
                phase: CasePhase::Active,
                next_follow: T0,
                from_pod: false,
                warned: true,
            },
        );
        crate::npc::bounty::set_security_status(&mut reg2, BOB, -6.0);
        attack_on_law(&mut reg2, BOB, cop2, at, at);
        assert!(!crate::crimewatch::is_criminal(&reg2, BOB, at), "an outlaw");
    }

    /// **Shooting a faction's police is a crime for a clean pilot**, as
    /// shooting CONCORD is; the pilot they came for may shoot back freely.
    #[test]
    fn a_clean_pilot_who_shoots_faction_police_is_a_criminal() {
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        live(&mut reg, T0);
        live(&mut reg, T0 + POLICE_DELAY);
        let cop = *police_ships(&reg).first().expect("a squad");
        assert!(
            !is_concord_kill(&reg, Some(cop)),
            "a faction's kill pays insurance"
        );
        let now = T0 + 30 * SEC;
        attack_on_law(&mut reg, ALICE, cop, now, now);
        assert!(!crate::crimewatch::is_criminal(&reg, ALICE, now), "theirs");
        let before = crate::sec_status::transactions(&reg, BOB).len();
        attack_on_law(&mut reg, BOB, cop, now, now);
        assert!(crate::crimewatch::is_criminal(&reg, BOB, now), "flagged");
        assert_eq!(
            crate::sec_status::transactions(&reg, BOB).len(),
            before + 1,
            "and charged"
        );
    }

    /// **A safety below red spares the law (NW-7 fix2 F3).** The gate answers
    /// with the criminal refusal for a faction police ship and a CONCORD
    /// ship alike, for the pilot they came for and an outlaw never, and for
    /// any other volley (a smartbomb, drones) the same way: only the
    /// mechanism is asserted, not the [A] rule behind it.
    #[test]
    fn a_clean_pilots_safety_spares_police_and_concord() {
        use crate::crimewatch::{Safety, attack_refusal, set_safety};
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        live(&mut reg, T0);
        live(&mut reg, T0 + POLICE_DELAY);
        let cop = *police_ships(&reg).first().expect("a squad");
        let now = T0 + 30 * SEC;
        assert!(illegal_on_law(&reg, BOB, cop));
        assert!(!illegal_on_law(&reg, ALICE, cop), "theirs to shoot at");
        let r = attack_refusal(&reg, BOB, Some(HULL), cop, now).expect("green refuses");
        assert_eq!(r.key, "ModuleActivationDeniedSafetyPreventsCriminal");
        set_safety(&mut reg, BOB, Safety::Partial);
        assert!(attack_refusal(&reg, BOB, Some(HULL), cop, now).is_some());
        set_safety(&mut reg, BOB, Safety::None);
        assert!(attack_refusal(&reg, BOB, Some(HULL), cop, now).is_none());
        assert!(attack_refusal(&reg, ALICE, Some(HULL), cop, now).is_none());
        // CONCORD's own ship the same way.
        let mut reg = world(HIGH);
        let cop = squad_ship(&mut reg, T0);
        assert!(illegal_on_law(&reg, BOB, cop));
        assert!(attack_refusal(&reg, BOB, Some(HULL), cop, T0 + SEC).is_some());
        // Not with CONCORD off.
        set_enabled(&mut reg, false);
        assert!(!illegal_on_law(&reg, BOB, cop));
    }

    /// **A pilot in warp is not policed yet**: the squad waits for the
    /// landing, then comes at once with its warning.
    #[test]
    fn a_squad_waits_while_the_pilot_is_in_warp() {
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        live(&mut reg, T0);
        warp(&mut reg, ALICE, true);
        live(&mut reg, T0 + POLICE_DELAY + 5 * SEC);
        assert_eq!(phase_of(&reg), Some(CasePhase::Pending));
        assert!(law_ships(&reg).is_empty());
        warp(&mut reg, ALICE, false);
        live(&mut reg, T0 + POLICE_DELAY + 6 * SEC);
        assert_eq!(phase_of(&reg), Some(CasePhase::Active));
        assert_eq!(police_ships(&reg).len(), 1 + POLICE_PLAIN);
    }

    /// **A system holds only so many squads** (review S2): with room for
    /// one, the second outlaw's squad waits, still due, until the first is
    /// gone, and then comes.
    #[test]
    fn a_crowded_system_holds_a_capped_number_of_squads() {
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        crate::npc::bounty::set_security_status(&mut reg, BOB, WANTED);
        reg.ext_mut::<Concord>().max_squads = 1;
        live(&mut reg, T0);
        live(&mut reg, T0 + POLICE_DELAY);
        let phases: Vec<_> = [ALICE, BOB]
            .iter()
            .map(|c| reg.ext::<Concord>().case(*c).map(|c| c.phase))
            .collect();
        assert_eq!(
            phases
                .iter()
                .filter(|p| **p == Some(CasePhase::Active))
                .count(),
            1,
            "{phases:?}"
        );
        assert_eq!(
            phases
                .iter()
                .filter(|p| **p == Some(CasePhase::Pending))
                .count(),
            1
        );
        assert_eq!(law_ships(&reg).len(), 1 + POLICE_PLAIN);
        // Room again: the waiting one comes.
        reg.ext_mut::<Concord>().max_squads = 2;
        live(&mut reg, T0 + POLICE_DELAY + SEC);
        assert_eq!(law_ships(&reg).len(), 2 * (1 + POLICE_PLAIN));
    }

    fn warp(reg: &mut Registry, id: i64, on: bool) {
        let p = reg.roster_mut().get_mut(id).expect("in space");
        p.motion.mode = if on {
            crate::destiny::Mode::Warp {
                goto: crate::destiny::Mode::warp_goto(
                    p.motion.position,
                    [1.0e6, 0.0, 0.0],
                    0.0,
                    -1,
                ),
                effect_stamp: -1,
                total_distance: crate::destiny::Mode::warp_total_distance(-1, 0.0),
                stop_distance: 0.0,
                warp_factor: 3,
            }
        } else {
            crate::destiny::Mode::Stop
        };
    }

    /// **A squad follows a pilot who landed off grid**, once the throttle has
    /// passed, with no second warning; one still on grid is left in place.
    #[test]
    fn the_squad_rejoins_a_pilot_who_landed_off_grid() {
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        live(&mut reg, T0);
        live(&mut reg, T0 + POLICE_DELAY);
        let first = police_ships(&reg);
        let _ = pushes(&mut reg);
        let t = T0 + POLICE_DELAY + POLICE_FOLLOW;
        live(&mut reg, t);
        assert_eq!(police_ships(&reg), first, "on grid: in place");
        let far = crate::grid::GRID_LEAVE * 3.0;
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .position = [far, 0.0, 0.0];
        live(&mut reg, t + POLICE_FOLLOW);
        let after = police_ships(&reg);
        assert_eq!(after, first, "the same balls, moved");
        let centre = reg.roster().get(ALICE).expect("in space").motion.position;
        for id in &after {
            let at = reg.roster().get(*id).expect("a ball").motion.position;
            assert!(crate::space_state::distance(at, centre) < crate::grid::GRID_LEAVE);
        }
        assert!(
            pushes(&mut reg)
                .iter()
                .all(|(_, n, _)| *n != "OnRemoteMessage"),
            "no second warning"
        );
    }

    /// **The follow range is the lead's lock range (NW-7 fix2 S-A)**, with a
    /// floor under a fresh ring and the grid-leave distance for an unknown
    /// range; mechanism only, the [A] is not asserted.
    #[test]
    fn the_follow_range_comes_from_the_lock_range() {
        assert_eq!(follow_range_of(150_000.0), 150_000.0);
        assert_eq!(follow_range_of(0.0), crate::grid::GRID_LEAVE);
        assert!(follow_range_of(1.0) > POLICE_RING_NEAR + 2.0 * POLICE_RING_STEP);
    }

    /// **The squad is moved, not respawned (NW-7 fix2 F5)**: the same balls
    /// keep the damage they took, and a pilot who is not past the follow
    /// range leaves them where they are.
    #[test]
    fn rejoining_keeps_the_damage() {
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        live(&mut reg, T0);
        live(&mut reg, T0 + POLICE_DELAY);
        let lead = *police_ships(&reg).first().expect("a squad");
        let kind = reg.npcs().get(lead).expect("an npc").kind;
        let profile = crate::damage::Profile::from_hull_in(reg.content(), &kind.hull);
        let hurt = crate::damage::ShipDamage::with_damage(profile, T0, 1e9, 1e9);
        reg.combat_mut().set_ship(lead, hurt);
        let before = reg.combat().ship(lead).copied();
        assert!(before.is_some());
        let ships = police_ships(&reg);
        let far = crate::grid::GRID_LEAVE * 3.0;
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .position = [far, 0.0, 0.0];
        live(&mut reg, T0 + POLICE_DELAY + POLICE_FOLLOW);
        assert_eq!(police_ships(&reg), ships, "the same balls");
        assert_eq!(reg.combat().ship(lead).copied(), before, "and their damage");
        let centre = reg.roster().get(ALICE).expect("in space").motion.position;
        let at = reg.roster().get(lead).expect("a ball").motion.position;
        assert!(crate::space_state::distance(at, centre) < crate::grid::GRID_LEAVE);
    }

    /// **Boarding another ship in space sheds no police**: only a pod ends
    /// the squad's work.
    #[test]
    fn changing_hulls_in_space_keeps_the_case() {
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        live(&mut reg, T0);
        live(&mut reg, T0 + POLICE_DELAY);
        let ships = police_ships(&reg);
        reg.roster_mut().leave(ALICE);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ALICE,
                SHIP[0] + 50,
                SYS,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        live(&mut reg, T0 + POLICE_DELAY + SEC);
        assert_eq!(phase_of(&reg), Some(CasePhase::Active));
        assert_eq!(police_ships(&reg), ships);
        assert_eq!(
            reg.ext::<Concord>().case(ALICE).map(|c| c.ship),
            Some(SHIP[0] + 50)
        );
    }

    /// **An eject and a reboard bring the squad back (NW-7 fix2 F2)**, at
    /// once and without a second warning; a pursuit that timed out does not
    /// come back for a hull swap.
    #[test]
    fn ejecting_and_reboarding_brings_the_squad_back() {
        let mut reg = net_world(0.9, EMPIRE, WANTED);
        live(&mut reg, T0);
        live(&mut reg, T0 + POLICE_DELAY);
        assert_eq!(police_ships(&reg).len(), 1 + POLICE_PLAIN);
        let board = |reg: &mut Registry, ship: i64, type_id: i64| {
            reg.roster_mut().leave(ALICE);
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    ALICE,
                    ship,
                    SYS,
                    crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
                ));
            reg.character_mut(ALICE).expect("a pilot").ship_type_id = type_id;
        };
        board(&mut reg, SHIP[0] + 50, crate::damage::CAPSULE_TYPE_ID);
        let now = T0 + POLICE_DELAY + SEC;
        live(&mut reg, now);
        assert_eq!(phase_of(&reg), Some(CasePhase::Spent));
        assert!(law_ships(&reg).is_empty());
        let _ = pushes(&mut reg);
        board(&mut reg, SHIP[0], HULL);
        live(&mut reg, now + SEC);
        assert_eq!(phase_of(&reg), Some(CasePhase::Active));
        assert_eq!(police_ships(&reg).len(), 1 + POLICE_PLAIN);
        assert!(
            pushes(&mut reg)
                .iter()
                .all(|(_, n, _)| *n != "OnRemoteMessage"),
            "no second warning"
        );
        // A timed-out pursuit stays spent through a hull swap.
        let given_up = now + SEC + POLICE_PURSUIT;
        live(&mut reg, given_up);
        assert_eq!(phase_of(&reg), Some(CasePhase::Spent));
        board(&mut reg, SHIP[0] + 60, HULL);
        live(&mut reg, given_up + SEC);
        assert_eq!(phase_of(&reg), Some(CasePhase::Spent));
        assert!(law_ships(&reg).is_empty());
    }

    /// **A stale bundle spawns no invented ships (NW-7 fix2 F4).** With a
    /// bundle and a force whose types the typedata lacks (a bundle with no
    /// `npc/police.tsv`, or a row naming types it does not carry), nothing is
    /// spawned, the case is spent and no warning goes out; with no bundle
    /// the fixtures serve.
    #[test]
    fn a_bundle_without_the_forces_types_spawns_nothing() {
        let build = |bundle: bool| {
            let mut content = crate::content::Content::fixture();
            content.insert("universe", universe_of(0.9, EMPIRE));
            if bundle {
                content.mark_bundle();
            }
            let mut reg = world_on(content);
            crate::npc::bounty::set_security_status(&mut reg, ALICE, WANTED);
            let _ = reg.take_pending();
            live(&mut reg, T0);
            live(&mut reg, T0 + POLICE_DELAY);
            reg
        };
        let mut stale = build(true);
        assert!(law_ships(&stale).is_empty());
        assert_eq!(phase_of(&stale), Some(CasePhase::Spent));
        assert!(
            pushes(&mut stale)
                .iter()
                .all(|(_, n, _)| *n != "OnRemoteMessage")
        );
        assert_eq!(police_ships(&build(false)).len(), 1 + POLICE_PLAIN);
    }

    /// A table with the invented empire's row and one for the CONCORD
    /// faction, as `npc/police.tsv` would carry them.
    fn table_with_two_forces() -> crate::npc_data::Table {
        let mut t = crate::npc_data::Table::fixture();
        t.police.insert(
            CONCORD_FACTION,
            crate::npc_data::Police {
                faction_id: CONCORD_FACTION,
                warning: "chatFixtureDefaultWarning".to_owned(),
                types: vec![990_330_021],
            },
        );
        t
    }

    /// The pure rules: who polices, and the ring.
    #[test]
    fn the_force_table_and_the_police_ring() {
        let reg = world_with(universe(HIGH), table_with_two_forces());
        // A faction with a row: its own corporation owns the squad.
        let (owner, key, types) = police_of(&reg, Some(599_002));
        assert_eq!(owner, FIXTURE_POLICE_CORP);
        assert_eq!(key, EMPIRE_WARNING);
        assert_eq!(types, vec![990_330_011, 990_330_012]);
        // A system no listed faction owns is CONCORD's row, owned by CONCORD.
        for f in [None, Some(1)] {
            let (owner, key, types) = police_of(&reg, f);
            assert_eq!(owner, CONCORD_CORP);
            assert_eq!(key, "chatFixtureDefaultWarning");
            assert_eq!(types, vec![990_330_021]);
        }
        // No bundle rows at all: no types, and so the fixtures.
        let bare = world_with(universe(HIGH), crate::npc_data::Table::default());
        assert!(police_of(&bare, Some(599_002)).2.is_empty());
        let mut ids: Vec<_> = reg
            .content()
            .get::<crate::npc_data::Table>()
            .police
            .values()
            .flat_map(|f| f.types.iter())
            .collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "no type serves two forces");
        assert!(is_law_owner(reg.content(), CONCORD_CORP));
        assert!(is_law_owner(reg.content(), FIXTURE_POLICE_CORP));
        assert!(!is_law_owner(reg.content(), 98_000_001));
        let centre = [0.0, 0.0, 0.0];
        for i in 0..POLICE_PLAIN + 1 {
            let d = crate::space_state::distance(police_ring(centre, i, 3), centre);
            assert!(
                (POLICE_RING_NEAR - 1.0..=POLICE_RING_NEAR + 2.0 * POLICE_RING_STEP + 1.0)
                    .contains(&d)
            );
        }
    }

    /// **The police lead really holds its pilot (NW-7 fix2 F1).** The
    /// group-182 types are not in `npc/types.tsv`, so the brain's Scram and
    /// Web arms must read the tackle off the typedata through `behaviour_of`;
    /// with the plain `Behaviour::of` the timers advanced and nothing held.
    /// Invented rows: a lead with the two tackle effects, a plain ship.
    #[test]
    fn the_police_lead_scrams_and_webs_its_pilot() {
        let rows = format!(
            "{}type\t990330011\t990301\t11\t1\t1\t0.0\t0.0\t1.0\t1.0\t0\t{},{}\t103=30000.0;105=3.0;505=10000.0;20=-90.0;513=10000.0;514=30000.0\tFixture Law Lead\n\
             type\t990330012\t990301\t11\t1\t1\t0.0\t0.0\t1.0\t1.0\t0\t\t\tFixture Law Plain\n",
            crate::typedata::FIXTURE,
            POLICE_TACKLE[0],
            POLICE_TACKLE[1]
        );
        let mut content = crate::content::Content::fixture();
        content.insert("universe", universe_of(0.9, EMPIRE));
        content.insert(
            "typedata",
            crate::typedata::Installed::build(
                crate::typedata::Table::parse(&rows).expect("the invented rows parse"),
            ),
        );
        let mut reg = world_on(content);
        crate::npc::bounty::set_security_status(&mut reg, ALICE, WANTED);
        let _ = reg.take_pending();
        live(&mut reg, T0);
        live(&mut reg, T0 + POLICE_DELAY);
        let lead = police_ships(&reg)
            .into_iter()
            .find(|id| {
                reg.npcs()
                    .get(*id)
                    .is_some_and(|n| n.kind.type_id == 990_330_011)
            })
            .expect("the typedata lead was resolved, not the fixture");
        assert_eq!(
            crate::npc::bounty::security_status(&reg, ALICE),
            WANTED,
            "untouched"
        );
        let mut now = T0 + POLICE_DELAY;
        let mut held = 0.0_f64;
        let mut slowed = false;
        for _ in 0..160 {
            now += SEC / 8;
            crate::npc::tick(&mut reg, now);
            held = held.max(crate::npc_ewar::incoming(&reg, SHIP[0], now));
            slowed |= crate::npc_ewar::sources(&reg, SHIP[0], now)
                .iter()
                .any(|(n, h)| *n == lead && h.effect.kind == Kind::Web);
        }
        assert!(held > 0.0, "a scram hold landed on the wanted pilot");
        assert!(slowed, "and a web");
    }
}
