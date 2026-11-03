//! Sovereignty: the claim ledger, the client's rows built from it, and the
//! claim pushes (SOC-D1; lane note `docs/lanes/sov-d1.md`).
//!
//! **The world's deltas are the only copy.** Three kinds
//! (`delta_kinds::KINDS`):
//!
//! ```text
//! sov       <systemID>   -> allianceID corpID hubID state claimTime stateUntil [more...]
//! sovprime  <allianceID> -> currentHour newHour validAfter
//! sovact    <seq>        -> systemID ownerID oldOwnerID changeTime
//! ```
//!
//! `-` is an absent value. `state` is one of [`HubState`]'s words. A `sov`
//! row may carry **more fields** than the six D1 knows: D2 (hub launch), D3
//! (campaigns) and D4 (indices) append theirs, and D1 keeps them verbatim
//! through every rewrite ([`Claim::rest`]). `sovact` keys are zero-padded
//! so they sort in order; only the newest [`ACTIVITY_KEEP`] are kept.
//!
//! **What a claim looks like on the wire** (R-SOV §1.3, §3) **[V]**: the
//! Sovereignty Hub is the claim, but the alliance window lists a claimed
//! system only when it also has a *Territorial Claim Unit* row
//! (`claimedSystems.IncludesTCU`), and the Sov Hubs page keeps a hub only
//! when a TCU row carries the **same item id** (`sovHubPage
//! .GetMySovHubsAndSystems`). So every claim answers one TCU-typed row and
//! one hub row, both naming the hub's id.
//!
//! **The hub is a real item and ball** since SOC-D2 (`crate::sov_hub`): a
//! launch or `/sov set` writes the row with the hub's item id. A row with a
//! stand-in id ([`stand_in_hub_id`]) is what `/sov set` leaves only when the
//! type table lacks the hub type (a test fixture). The service arms are
//! `service/sov.rs`.

use crate::celestial::{Band, System, Universe};
use crate::delta::Deltas;
use crate::pyutil::{keyval, new_style};
use crate::service::Registry;
use eve_marshal::PyValue;

/// The delta kinds this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "sov";
pub const KIND_PRIME: &str = "sovprime";
pub const KIND_ACTIVITY: &str = "sovact";

/// `inventorycommon.const.typeInfrastructureHub` **[V]** (R-SOV §2): the
/// Sovereignty Hub, the claim itself, and the type of every structure row.
pub const TYPE_INFRASTRUCTURE_HUB: i64 = 32_458;
/// `inventorycommon.const.typeTerritorialClaimUnit` **[V]**: no TCU exists in
/// space any more, but the alliance rows still need a TCU-typed row per
/// claim (module header).
pub const TYPE_TERRITORIAL_CLAIM_UNIT: i64 = 32_226;

/// The hub's `deploy.defaultOwner` **[V-data]** (R-SOV §1.1): the owner of a
/// neutral hub, which belongs to nobody's alliance.
pub const NEUTRAL_HUB_OWNER: i64 = 1_000_132;

/// The client's two sov info classes, whitelisted in `globalsWhitelist`
/// **[V]**; both read underscored attributes (`sovereignty/data_types`).
pub const SOV_CLAIM_INFO: &str = "sovereignty.data_types.SovClaimInfo";
pub const SOV_HUB_INFO: &str = "sovereignty.data_types.SovHubInfo";

/// FILETIME ticks per second and per hour.
const SECOND: i64 = 10_000_000;
const HOUR: i64 = 3_600 * SECOND;

/// `entosis.entosisConst.CHANGE_PRIMETIME_DELAY` **[V]**: 48 hours before a
/// new prime hour applies. The all_ui_sovereignty window adds the same delay
/// to "now" when it warns about overwriting a pending change. A game span:
/// the caller scales it with the world clock (`game_clock::span`).
pub const CHANGE_PRIMETIME_DELAY: i64 = 48 * HOUR;

/// **[A]** the prime hour an alliance that holds a claim but never set one
/// has (EvEJS starts at 0, a fact only; the client's own fallback in
/// `GetCurrentPrimeHour` is also 0 **[V]**). Falsified by a retail capture
/// answering another hour for a fresh alliance.
pub const DEFAULT_PRIME_HOUR: i64 = 0;

/// **[A]** how many `sovact` rows are kept (R-SOV §6.1: "the last 200 or 7
/// days"). Falsified by a map's sov-changes filter that wants more history
/// than a week of a busy society.
pub const ACTIVITY_KEEP: usize = 200;
/// **[A]** how far back `map.GetRecentSovActivity` looks (R-SOV §6.1).
/// Falsified as [`ACTIVITY_KEEP`].
pub const ACTIVITY_WINDOW: i64 = 7 * 24 * HOUR;

/// **[A]** the first stand-in hub id: a GM claim without a launched hub uses
/// `STAND_IN_HUB_BASE + systemID`, a band no allocator in this server
/// reaches (player items count up from `item::MIN_PLAYER_ITEM`, NPCs from
/// 7e9, sites 8.1e9 and 8.7e9, escrow 9e9). Falsified by a client that
/// looks the id up as an item and throws (a Show Info on the hub from the
/// Sov Hubs page is the place to see it); D2 replaces it with a real hub.
pub const STAND_IN_HUB_BASE: i64 = 8_900_000_000;

/// The stand-in hub id of `system_id`'s GM claim ([`STAND_IN_HUB_BASE`]).
pub fn stand_in_hub_id(system_id: i64) -> i64 {
    STAND_IN_HUB_BASE + system_id
}

// ---------------------------------------------------------------------------
// The ledger
// ---------------------------------------------------------------------------

/// A hub's state (R-SOV §6.1). D1 writes only `Claimed`; the rest are for
/// D2 (`Neutral`, `Offline`) and D3 (`Reinforced`, `NodeFight`), and a row
/// with any of them reads back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubState {
    Neutral,
    Claimed,
    Reinforced,
    NodeFight,
    Offline,
}

impl HubState {
    pub fn as_str(self) -> &'static str {
        match self {
            HubState::Neutral => "neutral",
            HubState::Claimed => "claimed",
            HubState::Reinforced => "reinforced",
            HubState::NodeFight => "nodefight",
            HubState::Offline => "offline",
        }
    }

    pub fn parse(s: &str) -> Option<HubState> {
        Some(match s {
            "neutral" => HubState::Neutral,
            "claimed" => HubState::Claimed,
            "reinforced" => HubState::Reinforced,
            "nodefight" => HubState::NodeFight,
            "offline" => HubState::Offline,
            _ => return None,
        })
    }
}

/// One system's hub, claimed or not.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    pub system_id: i64,
    /// `None` for a neutral hub.
    pub alliance_id: Option<i64>,
    /// The hub's owner corp: the alliance's executor at claim time, or
    /// [`NEUTRAL_HUB_OWNER`] for a neutral hub.
    pub corporation_id: i64,
    pub hub_id: i64,
    pub state: HubState,
    /// FILETIME of the claim; `None` while neutral.
    pub claim_time: Option<i64>,
    /// FILETIME the current state ends, for D3's timers.
    pub state_until: Option<i64>,
    /// Fields a later packet appended, kept verbatim.
    pub rest: Vec<String>,
}

impl Claim {
    /// Whether an alliance holds the system: an owner and a state other than
    /// neutral. An offline or reinforced hub still holds its claim.
    pub fn is_claimed(&self) -> bool {
        self.alliance_id.is_some() && self.state != HubState::Neutral
    }

    /// The alliance that holds the system, if one does.
    pub fn holder(&self) -> Option<i64> {
        self.alliance_id.filter(|_| self.is_claimed())
    }
}

fn opt_field(v: Option<i64>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| v.to_string())
}

fn parse_opt(s: Option<&String>) -> Result<Option<i64>, String> {
    match s.map(String::as_str) {
        None | Some("-") => Ok(None),
        Some(s) => s
            .parse::<i64>()
            .map(Some)
            .map_err(|_| format!("{s:?} is not a number")),
    }
}

fn parse_req(s: Option<&String>) -> Result<i64, String> {
    parse_opt(s)?.ok_or_else(|| "a required field is absent".to_owned())
}

fn parse_claim(key: &[String], value: &[String]) -> Result<Claim, String> {
    let system_id = parse_req(key.first())?;
    let state = value
        .get(3)
        .and_then(|s| HubState::parse(s))
        .ok_or_else(|| format!("bad state {:?}", value.get(3)))?;
    Ok(Claim {
        system_id,
        alliance_id: parse_opt(value.first())?,
        corporation_id: parse_req(value.get(1))?,
        hub_id: parse_req(value.get(2))?,
        state,
        claim_time: parse_opt(value.get(4))?,
        state_until: parse_opt(value.get(5))?,
        rest: value.get(6..).map(<[String]>::to_vec).unwrap_or_default(),
    })
}

/// One system's hub row, if it has one. A row that does not parse is
/// logged and read as absent: one claim lost, not the world.
pub fn claim(d: &Deltas, system_id: i64) -> Option<Claim> {
    let key = vec![system_id.to_string()];
    let value = d.of(KIND).get(&key)?;
    parse_claim(&key, value)
        .map_err(|e| crate::errln!("[sov] skipping row {key:?}: {e}"))
        .ok()
}

/// Every hub row, in system order.
pub fn all(d: &Deltas) -> Vec<Claim> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            parse_claim(k, v)
                .map_err(|e| crate::errln!("[sov] skipping row {k:?}: {e}"))
                .ok()
        })
        .collect()
}

/// The systems `alliance_id` holds.
pub fn claims_of(d: &Deltas, alliance_id: i64) -> Vec<Claim> {
    all(d)
        .into_iter()
        .filter(|c| c.holder() == Some(alliance_id))
        .collect()
}

/// The alliance holding `system_id`, if any. For other packets (D4 indices,
/// D5 planners) and the bots' reads.
pub fn holder(d: &Deltas, system_id: i64) -> Option<i64> {
    claim(d, system_id).and_then(|c| c.holder())
}

/// Write one row.
pub fn put(d: &mut Deltas, c: &Claim) {
    let mut value = vec![
        opt_field(c.alliance_id),
        c.corporation_id.to_string(),
        c.hub_id.to_string(),
        c.state.as_str().to_owned(),
        opt_field(c.claim_time),
        opt_field(c.state_until),
    ];
    value.extend(c.rest.iter().cloned());
    d.put(KIND, vec![c.system_id.to_string()], value);
}

/// What a claim change did: the holder before and after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub system_id: i64,
    pub old: Option<i64>,
    pub new: Option<i64>,
}

/// Give `system_id` to `owner` (`(allianceID, ownerCorpID)`), or take its
/// claim away (`None`). The GM path (`/sov set`); D3's capture and lapse
/// will call it too.
///
/// A new claim keeps the system's hub id when it has a row, else takes the
/// stand-in. A cleared claim drops a stand-in row and turns a real hub
/// neutral. A change of holder stamps `claim_time` and writes a `sovact`
/// row. `None` when nothing changed.
pub fn set_claim(
    d: &mut Deltas,
    system_id: i64,
    owner: Option<(i64, i64)>,
    now: i64,
) -> Option<Change> {
    let before = claim(d, system_id);
    let old = before.as_ref().and_then(Claim::holder);
    match owner {
        Some((alliance_id, corporation_id)) => {
            let mut c = before.clone().unwrap_or_else(|| Claim {
                system_id,
                alliance_id: None,
                corporation_id,
                hub_id: stand_in_hub_id(system_id),
                state: HubState::Neutral,
                claim_time: None,
                state_until: None,
                rest: Vec::new(),
            });
            if old != Some(alliance_id) {
                c.claim_time = Some(now);
                c.state_until = None;
                // SOC-D3a: a campaign and a score belong to the holder.
                if matches!(c.state, HubState::Reinforced | HubState::NodeFight) {
                    c.state = HubState::Claimed;
                }
                crate::sov_campaign::drop_campaign(d, system_id);
                crate::entosis::reset(d, c.hub_id);
            }
            c.alliance_id = Some(alliance_id);
            c.corporation_id = corporation_id;
            if c.state == HubState::Neutral {
                c.state = HubState::Claimed;
            }
            if before.as_ref() == Some(&c) {
                return None;
            }
            put(d, &c);
        }
        None => {
            let c = before?;
            crate::sov_campaign::drop_campaign(d, system_id);
            crate::entosis::reset(d, c.hub_id);
            if c.hub_id == stand_in_hub_id(system_id) {
                d.del(KIND, &[system_id.to_string()]);
            } else {
                let neutral = Claim {
                    alliance_id: None,
                    corporation_id: NEUTRAL_HUB_OWNER,
                    state: HubState::Neutral,
                    claim_time: None,
                    state_until: None,
                    ..c
                };
                put(d, &neutral);
            }
        }
    }
    let new = claim(d, system_id).and_then(|c| c.holder());
    if old != new {
        record_activity(d, system_id, new, old, now);
    }
    Some(Change {
        system_id,
        old,
        new,
    })
}

// ---------------------------------------------------------------------------
// Claimable systems
// ---------------------------------------------------------------------------

/// `sovSvc.IsSystemConquarable` **[V]**: known space, below low-sec, and no
/// faction; plus the hub's deploy rules (`disallowInWormholeSpace`, Abyssal,
/// Triglavian) **[V-data]**, which a system with a wormhole class or a
/// faction (Pochven's systems have one [I]) already fails. The band is the
/// client's own security rounding (`celestial::System::band`).
///
/// **The known-space classes are not wormholes**: the client's map data
/// gives every high-, low- and null-sec system a class too
/// ([`KNOWN_SPACE_CLASSES`]), so "has a class" refused every null-sec system
/// (owner, 2026-10-29: "Sov cannot be claimed here" in P3EN-E).
pub fn claimable_system(s: &System, wormhole_class: Option<i64>) -> bool {
    s.band() == Band::Null
        && s.faction_id.is_none()
        && wormhole_class.is_none_or(|c| KNOWN_SPACE_CLASSES.contains(&c))
}

/// The wormhole classes the map data gives known space: high, low and null
/// sec **[V-data]** (`systems.tsv` `wormhole_class_id`: every high-sec
/// system 7, low 8, null 9; J-space 1-6 and the special classes above 9).
pub const KNOWN_SPACE_CLASSES: std::ops::RangeInclusive<i64> = 7..=9;

/// [`claimable_system`] for a system the universe table knows. A system it
/// does not know is not claimable: there is nothing to check it against.
pub fn claimable(u: &Universe, system_id: i64) -> bool {
    u.system(system_id)
        .is_some_and(|s| claimable_system(s, u.wormhole_class(system_id)))
}

// ---------------------------------------------------------------------------
// Prime hour
// ---------------------------------------------------------------------------

/// An alliance's prime hour (`allianceRegistry.GetPrimeTimeInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrimeHour {
    /// 0..=23, or `None` for an alliance that holds nothing and never set
    /// one (the window shows "Unknown" **[V]**).
    pub current: Option<i64>,
    /// A pending change and when it applies.
    pub new: Option<i64>,
    pub valid_after: Option<i64>,
}

/// The prime hour at `now`: a pending change whose time has come is the
/// current hour (read lazily; the next write stores it).
pub fn prime_hour(d: &Deltas, alliance_id: i64, now: i64) -> PrimeHour {
    let row = d
        .of(KIND_PRIME)
        .get(&vec![alliance_id.to_string()])
        .and_then(|v| {
            Some(PrimeHour {
                current: parse_opt(v.first()).ok()?,
                new: parse_opt(v.get(1)).ok()?,
                valid_after: parse_opt(v.get(2)).ok()?,
            })
        });
    let mut p = row.unwrap_or(PrimeHour {
        current: None,
        new: None,
        valid_after: None,
    });
    if let (Some(new), Some(at)) = (p.new, p.valid_after)
        && now >= at
    {
        p = PrimeHour {
            current: Some(new),
            new: None,
            valid_after: None,
        };
    }
    if p.current.is_none() && !claims_of(d, alliance_id).is_empty() {
        p.current = Some(DEFAULT_PRIME_HOUR);
    }
    p
}

/// `SetPrimeHour(hour)`: the change applies `delay` after `now` (the caller
/// passes [`CHANGE_PRIMETIME_DELAY`] scaled by the world clock). A second
/// change before the first applied replaces it and restarts the wait, as
/// the window's `updateNewPrimeTime` prompt says **[V]**. Setting the hour
/// already current cancels a pending change.
pub fn set_prime_hour(
    d: &mut Deltas,
    alliance_id: i64,
    hour: i64,
    now: i64,
    delay: i64,
) -> Result<PrimeHour, &'static str> {
    if !(0..24).contains(&hour) {
        return Err("hour out of range");
    }
    let mut p = prime_hour(d, alliance_id, now);
    if p.current == Some(hour) {
        p.new = None;
        p.valid_after = None;
    } else {
        p.new = Some(hour);
        p.valid_after = Some(now.saturating_add(delay));
    }
    d.put(
        KIND_PRIME,
        vec![alliance_id.to_string()],
        vec![
            opt_field(p.current),
            opt_field(p.new),
            opt_field(p.valid_after),
        ],
    );
    Ok(p)
}

// ---------------------------------------------------------------------------
// Recent activity
// ---------------------------------------------------------------------------

/// One `sovact` row: a holder change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    pub seq: u64,
    pub system_id: i64,
    pub owner: Option<i64>,
    pub old_owner: Option<i64>,
    pub change_time: i64,
}

fn activity_key(seq: u64) -> Vec<String> {
    vec![format!("{seq:010}")]
}

fn activities(d: &Deltas) -> Vec<Activity> {
    d.of(KIND_ACTIVITY)
        .iter()
        .filter_map(|(k, v)| {
            Some(Activity {
                seq: k.first()?.parse().ok()?,
                system_id: parse_req(v.first()).ok()?,
                owner: parse_opt(v.get(1)).ok()?,
                old_owner: parse_opt(v.get(2)).ok()?,
                change_time: parse_req(v.get(3)).ok()?,
            })
        })
        .collect()
}

/// Append a holder change, keeping the newest [`ACTIVITY_KEEP`].
pub fn record_activity(
    d: &mut Deltas,
    system_id: i64,
    owner: Option<i64>,
    old_owner: Option<i64>,
    now: i64,
) {
    let rows = activities(d);
    let seq = rows.iter().map(|a| a.seq).max().map_or(1, |s| s + 1);
    d.put(
        KIND_ACTIVITY,
        activity_key(seq),
        vec![
            system_id.to_string(),
            opt_field(owner),
            opt_field(old_owner),
            now.to_string(),
        ],
    );
    let extra = (rows.len() + 1).saturating_sub(ACTIVITY_KEEP);
    for a in rows.iter().take(extra) {
        d.del(KIND_ACTIVITY, &activity_key(a.seq));
    }
}

/// The changes inside [`ACTIVITY_WINDOW`] before `now`, oldest first.
pub fn recent_activity(d: &Deltas, now: i64) -> Vec<Activity> {
    activities(d)
        .into_iter()
        .filter(|a| a.change_time >= now.saturating_sub(ACTIVITY_WINDOW))
        .collect()
}

// ---------------------------------------------------------------------------
// The client's rows (R-SOV §3)
// ---------------------------------------------------------------------------

fn id(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

/// `sovMgr.GetSystemSovereigntyInfo`: a `SovClaimInfo` for a held system,
/// `None` otherwise. New-style, state keys underscored as `__init__`
/// assigns them **[V]** (`sovereignty/data_types`).
pub fn claim_info(c: Option<&Claim>) -> PyValue {
    match c.filter(|c| c.is_claimed()) {
        Some(c) => new_style(
            SOV_CLAIM_INFO,
            vec![
                ("_claimStructureID", PyValue::Int(c.hub_id)),
                ("_corporationID", PyValue::Int(c.corporation_id)),
                ("_allianceID", id(c.alliance_id)),
            ],
        ),
        None => PyValue::None,
    }
}

/// `sovMgr.GetInfrastructureHubInfo`: a `SovHubInfo` for a held system's
/// hub, `None` otherwise (a neutral hub too [I], R-SOV §3).
pub fn hub_info(c: Option<&Claim>) -> PyValue {
    match c.filter(|c| c.is_claimed()) {
        Some(c) => new_style(
            SOV_HUB_INFO,
            vec![
                ("_hubID", PyValue::Int(c.hub_id)),
                ("_corporationID", PyValue::Int(c.corporation_id)),
                ("_allianceID", id(c.alliance_id)),
                ("_claimTime", id(c.claim_time)),
            ],
        ),
        None => PyValue::None,
    }
}

/// One structure row (`GetSovStructuresInfoForSolarSystem` and the
/// `OnSolarSystemSovStructuresUpdated` payload). A `KeyVal`: `sovSvc`
/// writes `solarSystemID` and `constellationID` into it by item assignment,
/// and every reader goes by attribute or `.get` **[V]**. `isCapital` and
/// `defenseMultiplier` (the ADM) are D4's ([`crate::sov_index::Bonus`]); the
/// two states wait for D3 [I: no window yet].
pub fn structure_row(
    c: &Claim,
    bonus: crate::sov_index::Bonus,
    (campaign_state, vulnerability_state): (PyValue, PyValue),
) -> PyValue {
    keyval(vec![
        ("itemID", PyValue::Int(c.hub_id)),
        ("typeID", PyValue::Int(TYPE_INFRASTRUCTURE_HUB)),
        ("ownerID", PyValue::Int(c.corporation_id)),
        ("corporationID", PyValue::Int(c.corporation_id)),
        ("allianceID", id(c.alliance_id)),
        ("solarSystemID", PyValue::Int(c.system_id)),
        ("isCapital", PyValue::Bool(bonus.is_capital)),
        ("defenseMultiplier", PyValue::Real(bonus.adm)),
        ("campaignState", campaign_state),
        ("vulnerabilityState", vulnerability_state),
    ])
}

/// A system's structure list: the hub only, never a TCU row (a second row
/// would draw a second icon in the location panel) [I, R-SOV §3]. `[]` for
/// a system with no hub; the panel draws its own placeholder **[V]**.
/// The ADM and capital as of now on the real clock: a caller holding the
/// registry uses [`structure_rows_at`] with the world's clock.
pub fn structure_rows(d: &Deltas, system_id: i64) -> PyValue {
    structure_rows_at(
        d,
        system_id,
        crate::pyutil::filetime_now(),
        crate::game_clock::GameClock::REAL,
    )
}

/// [`structure_rows`] at `now` on `clock` (the strategic index counts game
/// time).
pub fn structure_rows_at(
    d: &Deltas,
    system_id: i64,
    now: i64,
    clock: crate::game_clock::GameClock,
) -> PyValue {
    PyValue::List(
        claim(d, system_id)
            .iter()
            .map(|c| {
                structure_row(
                    c,
                    crate::sov_index::bonus(d, c, now, clock),
                    crate::entosis::row_states(d, c, now, clock),
                )
            })
            .collect(),
    )
}

/// One alliance row of `GetAllianceSovereigntyStructuresInfo`, for either
/// list. "Absent" is `None` for times and event types: the client tests
/// `campaignStartTime and campaignEventType`, then `vulnerableStartTime and
/// vulnerableEndTime` **[V]** (`sovSvc:398-429`).
/// Both occupancy levels are the ADM (D4), which the client shows as the
/// `defenseMultiplier` while a campaign or a window is set **[V]**.
fn alliance_row(c: &Claim, hub: bool, adm: f64, times: &crate::entosis::AllianceTimes) -> PyValue {
    let mut attrs = vec![
        ("structureID", PyValue::Int(c.hub_id)),
        ("solarSystemID", PyValue::Int(c.system_id)),
        ("campaignStartTime", id(times.campaign_start)),
        ("campaignEventType", id(times.campaign_type)),
        ("campaignOccupancyLevel", PyValue::Real(adm)),
        ("vulnerableStartTime", id(times.vulnerable.map(|w| w.start))),
        ("vulnerableEndTime", id(times.vulnerable.map(|w| w.end))),
        ("vulnerabilityOccupancyLevel", PyValue::Real(adm)),
    ];
    if hub {
        attrs.push(("corporationID", PyValue::Int(c.corporation_id)));
    }
    keyval(attrs)
}

/// `allianceRegistry.GetAllianceSovereigntyStructuresInfo()`: the 3-tuple
/// `(tcuRows, iHubRows, campaignScores)` **[V]**, one TCU-typed row and one
/// hub row per held system, **both carrying the hub's id** (module header).
/// No campaigns before D3. Now on the real clock, as [`structure_rows`].
pub fn alliance_structures(d: &Deltas, alliance_id: i64) -> PyValue {
    alliance_structures_at(
        d,
        alliance_id,
        crate::pyutil::filetime_now(),
        crate::game_clock::GameClock::REAL,
    )
}

/// [`alliance_structures`] at `now` on `clock`.
pub fn alliance_structures_at(
    d: &Deltas,
    alliance_id: i64,
    now: i64,
    clock: crate::game_clock::GameClock,
) -> PyValue {
    let claims = claims_of(d, alliance_id);
    let adm = |c: &Claim| crate::sov_index::bonus(d, c, now, clock).adm;
    let times = |c: &Claim| crate::entosis::alliance_times(d, c, now, clock);
    PyValue::Tuple(vec![
        PyValue::List(
            claims
                .iter()
                .map(|c| alliance_row(c, false, adm(c), &times(c)))
                .collect(),
        ),
        PyValue::List(
            claims
                .iter()
                .map(|c| alliance_row(c, true, adm(c), &times(c)))
                .collect(),
        ),
        crate::entosis::campaign_scores(d, &claims),
    ])
}

/// `stationSvc.GetAllianceSystems()`: `KeyVal(solarSystemID, allianceID)`
/// per held system, both attribute reads in `mapViewData` **[V]**.
pub fn alliance_systems(d: &Deltas) -> PyValue {
    PyValue::List(
        all(d)
            .iter()
            .filter_map(|c| {
                c.holder().map(|a| {
                    keyval(vec![
                        ("solarSystemID", PyValue::Int(c.system_id)),
                        ("allianceID", PyValue::Int(a)),
                    ])
                })
            })
            .collect(),
    )
}

/// `map.GetRecentSovActivity()`: `KeyVal(solarSystemID, ownerID,
/// oldOwnerID, stationID, changeTime)` **[V]** (`mapFilterSovChanges` reads
/// `stationID is None` first, then the owners).
pub fn recent_activity_rows(d: &Deltas, now: i64) -> PyValue {
    PyValue::List(
        recent_activity(d, now)
            .iter()
            .map(|a| {
                keyval(vec![
                    ("solarSystemID", PyValue::Int(a.system_id)),
                    ("ownerID", id(a.owner)),
                    ("oldOwnerID", id(a.old_owner)),
                    ("stationID", PyValue::None),
                    ("changeTime", PyValue::Int(a.change_time)),
                ])
            })
            .collect(),
    )
}

/// `allianceRegistry.GetPrimeTimeInfo()` **[V]** (`all_ui_sovereignty`).
pub fn prime_time_info(p: &PrimeHour) -> PyValue {
    keyval(vec![
        ("currentPrimeHour", id(p.current)),
        ("newPrimeHour", id(p.new)),
        ("newPrimeHourValidAfter", id(p.valid_after)),
    ])
}

// ---------------------------------------------------------------------------
// The pushes (R-SOV §4)
// ---------------------------------------------------------------------------

/// One sov notification to a list of characters.
#[derive(Debug, Clone, PartialEq)]
pub struct SovPush {
    pub to: Vec<i64>,
    pub event: &'static str,
    pub args: Vec<PyValue>,
}

impl crate::notify::Push for SovPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

fn push(reg: &mut Registry, to: Vec<i64>, event: &'static str, args: Vec<PyValue>) {
    if !to.is_empty() {
        reg.push(SovPush { to, event, args });
    }
}

/// Online characters whose system is `system_id`.
fn online_in(reg: &Registry, system_id: i64) -> Vec<i64> {
    reg.sessions()
        .online_characters()
        .into_iter()
        .filter(|id| reg.session_system(*id) == Some(system_id))
        .collect()
}

/// Online members of any of `alliances`.
fn online_members(reg: &Registry, alliances: &[i64]) -> Vec<i64> {
    reg.sessions()
        .online_characters()
        .into_iter()
        .filter(|id| {
            reg.character(*id)
                .and_then(|c| c.alliance_id)
                .is_some_and(|a| alliances.contains(&a))
        })
        .collect()
}

/// `OnSolarSystemSovStructuresUpdated(solarSystemID, rows, changes)`: the
/// full list, which primes `sovSvc`'s memo **[V]**, to the sessions in the
/// system and the online members of `alliances` (R-SOV §4). `changes` is
/// `None` or `{hubID: [whatChanged...]}`; D2 and D3 call this.
pub fn push_structures(
    reg: &mut Registry,
    system_id: i64,
    alliances: &[i64],
    changes: Option<PyValue>,
) {
    let mut to = online_in(reg, system_id);
    for id in online_members(reg, alliances) {
        if !to.contains(&id) {
            to.push(id);
        }
    }
    let rows = structure_rows_at(
        reg.deltas(),
        system_id,
        crate::pyutil::filetime_now(),
        crate::game_clock::of(reg),
    );
    push(
        reg,
        to,
        "OnSolarSystemSovStructuresUpdated",
        vec![
            PyValue::Int(system_id),
            rows,
            changes.unwrap_or(PyValue::None),
        ],
    );
}

/// Tell the world about a claim change (R-SOV §4):
///
/// - `ProcessSovStatusChanged(systemID, SovClaimInfo | None)` to **everyone
///   online** [A]: `sovSvc` caches claims galaxy-wide with no expiry, so a
///   pilot elsewhere who looked once would keep a stale owner. Falsified by
///   a retail capture showing the push only in the system.
/// - `OnSovereigntyChanged(systemID, allianceID | None)` to the system.
/// - [`push_structures`] to the system and both alliances' members.
pub fn announce(reg: &mut Registry, change: &Change) {
    let c = claim(reg.deltas(), change.system_id);
    let everyone = reg.sessions().online_characters();
    push(
        reg,
        everyone,
        "ProcessSovStatusChanged",
        vec![PyValue::Int(change.system_id), claim_info(c.as_ref())],
    );
    let here = online_in(reg, change.system_id);
    push(
        reg,
        here,
        "OnSovereigntyChanged",
        vec![PyValue::Int(change.system_id), id(change.new)],
    );
    let alliances: Vec<i64> = change.old.into_iter().chain(change.new).collect();
    push_structures(reg, change.system_id, &alliances, None);
    // D4: both alliances' capitals, and the system's indices.
    crate::sov_index::on_claim_change(reg, change);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyutil::dict_get;
    use eve_marshal::ObjectForm;

    // Invented ids (R-SOV §9's suggestion): nothing here is a client value.
    const SYSTEM: i64 = 30_900_001;
    const OTHER_SYSTEM: i64 = 30_900_002;
    const ALLIANCE: i64 = 99_100_001;
    const OTHER_ALLIANCE: i64 = 99_100_002;
    const CORP: i64 = 98_100_001;
    const OTHER_CORP: i64 = 98_100_002;
    const NOW: i64 = 134_200_000_000_000_000;

    fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
        match v {
            PyValue::Instance { state, .. } => dict_get(state, name.as_bytes()),
            _ => None,
        }
    }

    fn list(v: &PyValue) -> &[PyValue] {
        match v {
            PyValue::List(l) => l,
            other => panic!("not a list: {other:?}"),
        }
    }

    fn claimed(d: &mut Deltas, system: i64, alliance: i64, corp: i64) {
        set_claim(d, system, Some((alliance, corp)), NOW).expect("a change");
    }

    #[test]
    fn unclaimed_system_answers_none_and_empty_lists() {
        let d = Deltas::default();
        assert_eq!(claim(&d, SYSTEM), None);
        assert_eq!(claim_info(claim(&d, SYSTEM).as_ref()), PyValue::None);
        assert_eq!(hub_info(claim(&d, SYSTEM).as_ref()), PyValue::None);
        assert_eq!(structure_rows(&d, SYSTEM), PyValue::List(vec![]));
        assert_eq!(alliance_systems(&d), PyValue::List(vec![]));
        assert_eq!(recent_activity_rows(&d, NOW), PyValue::List(vec![]));
        assert_eq!(
            alliance_structures(&d, ALLIANCE),
            PyValue::Tuple(vec![
                PyValue::List(vec![]),
                PyValue::List(vec![]),
                PyValue::List(vec![]),
            ])
        );
        assert_eq!(prime_hour(&d, ALLIANCE, NOW).current, None);
    }

    #[test]
    fn claim_info_is_a_whitelisted_new_style_object_with_underscored_state() {
        let mut d = Deltas::default();
        claimed(&mut d, SYSTEM, ALLIANCE, CORP);
        let c = claim(&d, SYSTEM);
        for (v, class, keys) in [
            (
                claim_info(c.as_ref()),
                SOV_CLAIM_INFO,
                &["_claimStructureID", "_corporationID", "_allianceID"][..],
            ),
            (
                hub_info(c.as_ref()),
                SOV_HUB_INFO,
                &["_hubID", "_corporationID", "_allianceID", "_claimTime"][..],
            ),
        ] {
            assert!(eve_rowset::is_whitelisted(class.as_bytes()));
            let PyValue::ObjectEx { form, header, .. } = &v else {
                panic!("{v:?}")
            };
            assert_eq!(*form, ObjectForm::New);
            let PyValue::Tuple(parts) = &**header else {
                panic!("{header:?}")
            };
            assert_eq!(
                parts[0],
                PyValue::Tuple(vec![PyValue::Global(class.as_bytes().to_vec())])
            );
            for k in keys {
                assert!(dict_get(&parts[1], k.as_bytes()).is_some(), "{class} {k}");
            }
        }
        let PyValue::ObjectEx { header, .. } = claim_info(c.as_ref()) else {
            unreachable!()
        };
        let PyValue::Tuple(parts) = *header else {
            unreachable!()
        };
        assert_eq!(
            dict_get(&parts[1], b"_claimStructureID"),
            Some(&PyValue::Int(stand_in_hub_id(SYSTEM)))
        );
        assert_eq!(
            dict_get(&parts[1], b"_allianceID"),
            Some(&PyValue::Int(ALLIANCE))
        );
    }

    #[test]
    fn alliance_rows_put_the_hub_id_in_both_tcu_and_ihub_rows() {
        let mut d = Deltas::default();
        claimed(&mut d, SYSTEM, ALLIANCE, CORP);
        claimed(&mut d, OTHER_SYSTEM, OTHER_ALLIANCE, OTHER_CORP);
        let PyValue::Tuple(t) = alliance_structures(&d, ALLIANCE) else {
            panic!()
        };
        assert_eq!(t.len(), 3);
        let (tcu, hubs) = (list(&t[0]), list(&t[1]));
        assert_eq!((tcu.len(), hubs.len()), (1, 1), "only this alliance's");
        let hub = PyValue::Int(stand_in_hub_id(SYSTEM));
        assert_eq!(attr(&tcu[0], "structureID"), Some(&hub));
        assert_eq!(attr(&hubs[0], "structureID"), Some(&hub));
        assert_eq!(attr(&hubs[0], "corporationID"), Some(&PyValue::Int(CORP)));
        assert_eq!(attr(&tcu[0], "corporationID"), None);
        for row in [&tcu[0], &hubs[0]] {
            assert_eq!(attr(row, "solarSystemID"), Some(&PyValue::Int(SYSTEM)));
            for k in ["campaignStartTime", "campaignEventType"] {
                assert_eq!(attr(row, k), Some(&PyValue::None), "{k} absent");
            }
            // SOC-D3a: a held hub shows its vulnerability window.
            let (Some(PyValue::Int(from)), Some(PyValue::Int(to))) = (
                attr(row, "vulnerableStartTime"),
                attr(row, "vulnerableEndTime"),
            ) else {
                panic!("a window")
            };
            assert!(from < to);
            assert!(attr(row, "campaignOccupancyLevel").is_some());
            assert!(attr(row, "vulnerabilityOccupancyLevel").is_some());
        }
        assert_eq!(t[2], PyValue::List(vec![]));
    }

    #[test]
    fn structure_row_has_every_key_the_client_reads() {
        let mut d = Deltas::default();
        claimed(&mut d, SYSTEM, ALLIANCE, CORP);
        let rows = structure_rows(&d, SYSTEM);
        let rows = list(&rows);
        assert_eq!(rows.len(), 1, "the hub only, no TCU row");
        let PyValue::Instance { class, .. } = &rows[0] else {
            panic!()
        };
        assert_eq!(class, crate::pyutil::KEYVAL_CLASS.as_bytes());
        for k in [
            "itemID",
            "typeID",
            "ownerID",
            "corporationID",
            "allianceID",
            "solarSystemID",
            "isCapital",
            "defenseMultiplier",
            "campaignState",
            "vulnerabilityState",
        ] {
            assert!(attr(&rows[0], k).is_some(), "{k}");
        }
        assert_eq!(
            attr(&rows[0], "typeID"),
            Some(&PyValue::Int(TYPE_INFRASTRUCTURE_HUB))
        );
        assert_eq!(attr(&rows[0], "ownerID"), Some(&PyValue::Int(CORP)));
        assert_eq!(attr(&rows[0], "allianceID"), Some(&PyValue::Int(ALLIANCE)));
    }

    #[test]
    fn alliance_systems_are_keyvals_with_attribute_fields() {
        let mut d = Deltas::default();
        claimed(&mut d, SYSTEM, ALLIANCE, CORP);
        let v = alliance_systems(&d);
        let rows = list(&v);
        assert_eq!(rows.len(), 1);
        let PyValue::Instance { class, .. } = &rows[0] else {
            panic!()
        };
        assert_eq!(class, crate::pyutil::KEYVAL_CLASS.as_bytes());
        assert_eq!(attr(&rows[0], "solarSystemID"), Some(&PyValue::Int(SYSTEM)));
        assert_eq!(attr(&rows[0], "allianceID"), Some(&PyValue::Int(ALLIANCE)));
    }

    #[test]
    fn a_new_holder_stamps_the_time_and_logs_the_change() {
        let mut d = Deltas::default();
        let c = set_claim(&mut d, SYSTEM, Some((ALLIANCE, CORP)), NOW).unwrap();
        assert_eq!((c.old, c.new), (None, Some(ALLIANCE)));
        assert_eq!(
            set_claim(&mut d, SYSTEM, Some((ALLIANCE, CORP)), NOW + 1),
            None
        );
        let later = NOW + HOUR;
        let c = set_claim(&mut d, SYSTEM, Some((OTHER_ALLIANCE, OTHER_CORP)), later).unwrap();
        assert_eq!((c.old, c.new), (Some(ALLIANCE), Some(OTHER_ALLIANCE)));
        let now = claim(&d, SYSTEM).unwrap();
        assert_eq!(now.claim_time, Some(later));
        assert_eq!(now.hub_id, stand_in_hub_id(SYSTEM), "the hub is kept");
        let c = set_claim(&mut d, SYSTEM, None, later + 1).unwrap();
        assert_eq!((c.old, c.new), (Some(OTHER_ALLIANCE), None));
        assert_eq!(claim(&d, SYSTEM), None, "a stand-in claim leaves no row");
        let log = recent_activity(&d, later + 1);
        assert_eq!(
            log.iter()
                .map(|a| (a.owner, a.old_owner))
                .collect::<Vec<_>>(),
            vec![
                (Some(ALLIANCE), None),
                (Some(OTHER_ALLIANCE), Some(ALLIANCE)),
                (None, Some(OTHER_ALLIANCE)),
            ]
        );
        assert!(recent_activity(&d, later + 1 + ACTIVITY_WINDOW + HOUR).is_empty());
    }

    #[test]
    fn clearing_a_real_hub_leaves_it_neutral_and_later_fields_survive() {
        let mut d = Deltas::default();
        let hub = 1_234_567;
        put(
            &mut d,
            &Claim {
                system_id: SYSTEM,
                alliance_id: Some(ALLIANCE),
                corporation_id: CORP,
                hub_id: hub,
                state: HubState::Claimed,
                claim_time: Some(NOW),
                state_until: None,
                rest: vec!["d3-field".to_owned()],
            },
        );
        set_claim(&mut d, SYSTEM, None, NOW + 1).unwrap();
        let c = claim(&d, SYSTEM).unwrap();
        assert_eq!(c.state, HubState::Neutral);
        assert_eq!(c.hub_id, hub);
        assert_eq!(c.corporation_id, NEUTRAL_HUB_OWNER);
        assert_eq!(c.rest, vec!["d3-field".to_owned()]);
        assert_eq!(claim_info(Some(&c)), PyValue::None);
        assert_eq!(alliance_systems(&d), PyValue::List(vec![]));
        // A neutral hub still has its structure row, with no alliance.
        let v = structure_rows(&d, SYSTEM);
        assert_eq!(attr(&list(&v)[0], "allianceID"), Some(&PyValue::None));
    }

    #[test]
    fn activity_keeps_only_the_newest_rows() {
        let mut d = Deltas::default();
        for i in 0..(ACTIVITY_KEEP + 3) {
            let owner = Some(if i % 2 == 0 { ALLIANCE } else { OTHER_ALLIANCE });
            record_activity(&mut d, SYSTEM, owner, None, NOW + i as i64);
        }
        let all = activities(&d);
        assert_eq!(all.len(), ACTIVITY_KEEP);
        assert_eq!(all.first().map(|a| a.change_time), Some(NOW + 3));
    }

    #[test]
    fn prime_hour_change_waits_the_delay_constant() {
        let mut d = Deltas::default();
        claimed(&mut d, SYSTEM, ALLIANCE, CORP);
        assert_eq!(
            prime_hour(&d, ALLIANCE, NOW).current,
            Some(DEFAULT_PRIME_HOUR)
        );
        let hour = (DEFAULT_PRIME_HOUR + 5) % 24;
        let p = set_prime_hour(&mut d, ALLIANCE, hour, NOW, CHANGE_PRIMETIME_DELAY).unwrap();
        assert_eq!(p.current, Some(DEFAULT_PRIME_HOUR));
        assert_eq!(p.new, Some(hour));
        assert_eq!(p.valid_after, Some(NOW + CHANGE_PRIMETIME_DELAY));
        let before = prime_hour(&d, ALLIANCE, NOW + CHANGE_PRIMETIME_DELAY - 1);
        assert_eq!(before, p);
        let after = prime_hour(&d, ALLIANCE, NOW + CHANGE_PRIMETIME_DELAY);
        assert_eq!(
            after,
            PrimeHour {
                current: Some(hour),
                new: None,
                valid_after: None
            }
        );
        assert!(set_prime_hour(&mut d, ALLIANCE, 24, NOW, CHANGE_PRIMETIME_DELAY).is_err());
        assert!(set_prime_hour(&mut d, ALLIANCE, -1, NOW, CHANGE_PRIMETIME_DELAY).is_err());
        let v = prime_time_info(&after);
        assert_eq!(attr(&v, "currentPrimeHour"), Some(&PyValue::Int(hour)));
        assert_eq!(attr(&v, "newPrimeHour"), Some(&PyValue::None));
    }

    #[test]
    fn faction_and_wormhole_systems_are_not_claimable() {
        let row = |id, security, faction_id| System {
            id,
            security,
            faction_id,
            ..System::default()
        };
        assert!(claimable_system(&row(SYSTEM, -0.4, None), None));
        assert!(
            !claimable_system(&row(SYSTEM, -0.4, Some(500_999)), None),
            "faction null"
        );
        assert!(
            !claimable_system(&row(SYSTEM, -0.4, None), Some(990_001)),
            "a wormhole class"
        );
        for c in KNOWN_SPACE_CLASSES {
            assert!(
                claimable_system(&row(SYSTEM, -0.4, None), Some(c)),
                "a known-space class is not a wormhole"
            );
        }
        assert!(
            !claimable_system(&row(31_900_001, -1.0, None), None),
            "J-space ids"
        );
        assert!(!claimable_system(&row(SYSTEM, 0.3, None), None), "low sec");
        assert!(!claimable_system(&row(SYSTEM, 0.8, None), None), "high sec");
        assert!(!claimable(&Universe::default(), SYSTEM), "unknown system");
    }

    #[test]
    fn claim_change_pushes_process_sov_status_changed_to_everyone() {
        const HERE: i64 = 90_100_001;
        const ELSEWHERE: i64 = 90_100_002;
        const MEMBER: i64 = 90_100_003;
        let pilot = |id: i64, system: i64, alliance: Option<i64>| crate::model::Character {
            character_id: id,
            name: format!("pilot {id}"),
            ship_id: id + 1_000,
            solar_system_id: Some(system),
            alliance_id: alliance,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(HERE, SYSTEM, None),
                pilot(ELSEWHERE, OTHER_SYSTEM, None),
                pilot(MEMBER, OTHER_SYSTEM, Some(ALLIANCE)),
            ],
            ..crate::model::SandboxConfig::default()
        });
        for (n, who) in [(1, HERE), (2, ELSEWHERE), (3, MEMBER)] {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(who),
            });
        }
        let _ = reg.take_pending();
        let change = set_claim(reg.deltas_mut(), SYSTEM, Some((ALLIANCE, CORP)), NOW).unwrap();
        announce(&mut reg, &change);
        let pushes: Vec<(&'static str, Vec<PyValue>, Vec<i64>)> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (e, a) = p.to_push();
                let mut to = p.audience().unwrap_or_default();
                to.sort_unstable();
                (e, a, to)
            })
            .collect();
        let find = |name: &str| {
            pushes
                .iter()
                .find(|p| p.0 == name)
                .unwrap_or_else(|| panic!("no {name}: {pushes:?}"))
        };
        let status = find("ProcessSovStatusChanged");
        assert_eq!(status.2, vec![HERE, ELSEWHERE, MEMBER]);
        assert_eq!(status.1[0], PyValue::Int(SYSTEM));
        assert!(matches!(status.1[1], PyValue::ObjectEx { .. }));
        let sov = find("OnSovereigntyChanged");
        assert_eq!(sov.2, vec![HERE]);
        assert_eq!(sov.1, vec![PyValue::Int(SYSTEM), PyValue::Int(ALLIANCE)]);
        let rows = find("OnSolarSystemSovStructuresUpdated");
        assert_eq!(rows.2, vec![HERE, MEMBER]);
        assert_eq!(list(&rows.1[1]).len(), 1);
        assert_eq!(rows.1[2], PyValue::None);
    }

    #[test]
    fn claim_pushes_reach_a_pilot_in_space_in_the_system() {
        use crate::space_state::{Motion, Presence};
        let pilot = |id: i64, system: i64| crate::model::Character {
            character_id: id,
            name: format!("pilot {id}"),
            ship_id: id + 1_000,
            solar_system_id: Some(system),
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            // Docked elsewhere (not an alliance member), in space in SYSTEM.
            characters: vec![pilot(90_100_011, OTHER_SYSTEM)],
            ..crate::model::SandboxConfig::default()
        });
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 1,
            session_id: 1,
            user_id: 1,
            character_id: Some(90_100_011),
        });
        reg.roster_mut().arrive(Presence::arriving(
            90_100_011,
            90_101_011,
            SYSTEM,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        let _ = reg.take_pending();
        let change = set_claim(reg.deltas_mut(), SYSTEM, Some((ALLIANCE, CORP)), NOW).unwrap();
        announce(&mut reg, &change);
        let names: Vec<(&'static str, Vec<i64>)> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| (p.to_push().0, p.audience().unwrap_or_default()))
            .collect();
        for event in ["OnSovereigntyChanged", "OnSolarSystemSovStructuresUpdated"] {
            let (_, to) = names
                .iter()
                .find(|p| p.0 == event)
                .unwrap_or_else(|| panic!("no {event}: {names:?}"));
            assert_eq!(to, &vec![90_100_011], "{event}");
        }
    }
}
