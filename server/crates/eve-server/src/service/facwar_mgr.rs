//! Factional warfare's militia office: `facWarMgr` and enlistment.
//!
//! W16 packet CP53a (`docs/content/plan.md` §6.2), spec `docs/content/R-FW.md`
//! §1 and §4 (research lane R-FW, W12) and C2 §5. Two services:
//!
//! - **`facWarMgr`**, an unbound service the client reaches through
//!   `facWarSvc.facWarMgr` (`facWarSvc:80-84`, `sm.RemoteSvc`), so the census
//!   never saw its calls. Every method in R-FW §1's table answers here with
//!   the shape its consumer pins.
//! - **`fwCharacterEnlistmentMgr`**, moved here from `service/facwar.rs`
//!   (seam S2's split): enlistment is this packet's subject, and
//!   `GetMyEnlistment` has to read the rows this module writes.
//!
//! **Direct enlistment** (`CreateMyDirectEnlistment`, and its legacy twin
//! `JoinFactionAsCharacter`) writes a [`KIND`] row, queues the
//! `warfactionid` session change both client join paths poll for (R-FW §0
//! item 5, `enlistCont.JoinMilitia_thread:165-175`, `EnlistDirectly:180`)
//! and pushes `OnEnlistmentStateUpdated(factionID, directFactionID,
//! corpFactionID)` [V] (`fwEnlistmentSvc:50-54`). Leaving starts the
//! [`ENLISTMENT_COOLDOWN_SECS`] cooldown and schedules
//! `OnEnlistmentCooldownCleared()` [V] (`fwEnlistmentSvc:138`).
//!
//! **Corporations and alliances** (R-FW §1 #6-#13) join as
//! [`STATUS_JOINING`] and leave as [`STATUS_LEAVING`]; the status settles
//! [`CORP_CHANGE_WAIT_SECS`] later, read lazily ([`corp_state`]). A director
//! of a player corp may restrict which militias its members enlist in
//! ([`ALLOW_KIND`]).
//!
//! **Rank** is R-FW §4's [I]: `clamp(int(standing toward the militia
//! corporation), 0, 9)`, the rule `facWarSvc.OnNPCStandingChange:430-432`
//! recomputes it by. [`RANK_KIND`] keeps the last rank seen per faction, so
//! `CheckForRankChange` can tell a change and the decorations panel can list
//! every faction ever served.
//!
//! The **iHub LP pool** (`GetSolarSystemLPs`, `DonateLPsToSolarSystem`) is
//! [`LP_KIND`]: the donation debits the militia corp's LP
//! (`crate::loyalty::adjust`) and pushes `OnSolarSystemLPChange` to the
//! system.
//!
//! What this does **not** do: kill and VP statistics (zeros until CP53b
//! counts them), the occupation the `systemsCount` column would count
//! (CP53b), a session change or push when a corp's status settles (it is
//! read lazily; a member sees it on the next login).
//!
//! **The militia corporation move** (FW-2): a pilot in an NPC corporation
//! who joins through the window (`JoinFactionAsCharacter`) moves into the
//! militia's NPC corporation, `Character.corporation_id` being the only
//! state, and Retire (`corp.KickOut`) moves them back to the sandbox
//! corporation and starts the cooldown. The client's own logic needs it
//! (`corp.joinmilitia`, `_GetRetireBtns`, `IsFwNpcCorp`); that the server
//! moves `corpid` is [I], no client file states it. A pilot in a player
//! corporation, and the bots (`CreateMyDirectEnlistment`), enlist directly.
//!
//! The [`SLOT`] is built from content only (CA11): the warzone militias and
//! each faction's militia corporation. Everything per character is a delta.

use super::*;
use crate::content::{Boxed, Content};
use crate::pyutil::{bstr, keyval};
use crate::world_ext::SlotSpec;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["facWarMgr", "fwCharacterEnlistmentMgr"];

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
/// Built from content only (never from deltas, CA11).
pub const SLOT: SlotSpec = SlotSpec {
    name: "facwar_mgr",
    make: |c| Box::new(Index::build(c)) as Boxed,
};

/// A direct enlistment (plan §4.2): `char -> faction since cooldown_until`.
/// `faction` is `0` once the character has left and only the cooldown is
/// left; the row goes when both are spent.
pub const KIND: &str = "fwenlist";

/// The last rank seen: `char faction -> rank`. One row per faction ever
/// served (`GetMyCharacterRankOverview`); never expires.
pub const RANK_KIND: &str = "fwrank";

/// A corporation's or alliance's militia: `owner -> faction status since`.
pub const CORP_KIND: &str = "fwcorp";

/// A director's restriction on direct enlistment: `corp -> faction,...`.
pub const ALLOW_KIND: &str = "fwallow";

/// A system's iHub LP pool: `system -> points`.
pub const LP_KIND: &str = "fwlp";

/// `factionwarfare/const.ENLISTMENT_COOLDOWN_SECONDS` [V] (`const:36`).
pub const ENLISTMENT_COOLDOWN_SECS: i64 = 86_400;

/// `appConst.facwarCorporation{Joining,Active,Leaving}` [V]
/// (`appConst:1305-1307`).
pub const STATUS_JOINING: i64 = 0;
pub const STATUS_ACTIVE: i64 = 1;
pub const STATUS_LEAVING: i64 = 2;

/// [A] How long a corporation's or alliance's join or leave takes to settle
/// (R-FW §4, "after a short wait"); short, for a few friends. Falsified by
/// nothing in the client: the wait is server-only.
pub const CORP_CHANGE_WAIT_SECS: i64 = 600;

/// `const.facwarSolarSystemMaxLPPool` [V] (R-FW §1 #27).
pub const MAX_LP_POOL: i64 = 300_000;

/// The highest militia rank [V] (`facWarSvc:432`, `GetRankLabel:637`).
pub const MAX_RANK: i64 = 9;

/// `statKey` [V] (`facWarSvc:469-474`).
pub const STAT_KEYS: [&str; 6] = ["killsY", "killsLW", "killsTotal", "vpY", "vpLW", "vpTotal"];

/// `const.groupCharacter`, `groupCorporation`, `groupAlliance` [V]
/// (`inventorycommon/const:517`, `:552`, `:376`): the keys of
/// `GetStats_TopAndAllKillsAndVPs` (`facWarSvc:480-481`).
pub const STAT_GROUPS: [i64; 3] = [1, 2, 32];

/// FILETIME ticks per second.
const TICKS: i64 = 10_000_000;

/// The content-derived index: the militias a character may join, and each
/// faction's militia corporation (`npc/factions.tsv`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Index {
    /// Every warzone faction, in warzone order.
    pub militias: Vec<i64>,
    /// `faction -> militia corporation`.
    pub militia_corp: BTreeMap<i64, i64>,
    /// `militia corporation -> faction`: the NPC corps a pilot moves into
    /// when enlisting through the window (FW-2).
    pub militia_by_corp: BTreeMap<i64, i64>,
}

impl Index {
    pub fn build(c: &Content) -> Index {
        let militias = c
            .try_get::<crate::facwar::Table>()
            .map(|t| t.warzones().iter().flat_map(|w| w.factions).collect())
            .unwrap_or_default();
        let militia_corp: BTreeMap<i64, i64> = c
            .try_get::<crate::npc_data::Table>()
            .map(|t| {
                t.factions
                    .values()
                    .filter_map(|f| Some((f.faction_id, f.militia_corporation_id?)))
                    .collect()
            })
            .unwrap_or_default();
        let militia_by_corp = militia_corp.iter().map(|(f, c)| (*c, *f)).collect();
        Index {
            militias,
            militia_corp,
            militia_by_corp,
        }
    }
}

// ---------------------------------------------------------------------------
// State, read from the deltas.

fn parse(s: Option<&String>) -> i64 {
    s.and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn nonzero(n: i64) -> Option<i64> {
    (n != 0).then_some(n)
}

/// A direct enlistment row: `(faction, since, cooldown_until)`.
fn enlist_row(reg: &Registry, ch: i64) -> (Option<i64>, i64, i64) {
    reg.deltas()
        .of(KIND)
        .get(&[ch.to_string()][..])
        .map_or((None, 0, 0), |v| {
            (nonzero(parse(v.first())), parse(v.get(1)), parse(v.get(2)))
        })
}

/// The faction `ch` is directly enlisted in.
pub fn direct_faction(reg: &Registry, ch: i64) -> Option<i64> {
    enlist_row(reg, ch).0
}

/// A corporation's or alliance's militia state at `now`: `(faction, status,
/// since)`, the status settled lazily. A `Joining` row older than
/// [`CORP_CHANGE_WAIT_SECS`] reads `Active`, and a `Leaving` one reads as
/// no row at all.
pub fn corp_state(reg: &Registry, owner: i64, now: i64) -> Option<(i64, i64, i64)> {
    let v = reg.deltas().of(CORP_KIND).get(&[owner.to_string()][..])?;
    let (faction, status, since) = (parse(v.first()), parse(v.get(1)), parse(v.get(2)));
    let settled = now >= since.saturating_add(CORP_CHANGE_WAIT_SECS * TICKS);
    match status {
        STATUS_JOINING if settled => Some((faction, STATUS_ACTIVE, since)),
        STATUS_LEAVING if settled => None,
        _ => Some((faction, status, since)),
    }
}

/// The faction an owner fights for: `Active` or `Leaving` (still serving).
fn serving(reg: &Registry, owner: i64, now: i64) -> Option<i64> {
    corp_state(reg, owner, now)
        .filter(|(_, s, _)| *s != STATUS_JOINING)
        .map(|(f, _, _)| f)
}

/// `ch`'s player corporation, if any.
fn player_corp(reg: &Registry, ch: i64) -> Option<i64> {
    reg.corporations.membership(ch).map(|m| m.corporation_id)
}

/// The militia whose NPC corporation `ch` sits in (FW-2): the pilot moved
/// into it by `corp.joinmilitia`, and `Character.corporation_id` is the
/// whole state.
pub(crate) fn militia_of_corp(reg: &Registry, ch: i64) -> Option<i64> {
    let corp = reg.character(ch)?.corporation_id;
    reg.ext::<Index>().militia_by_corp.get(&corp).copied()
}

/// The militia `ch` serves through their militia corporation, their player
/// corporation or its alliance.
pub fn corp_faction(reg: &Registry, ch: i64, now: i64) -> Option<i64> {
    if let Some(f) = militia_of_corp(reg, ch) {
        return Some(f);
    }
    let corp = player_corp(reg, ch).and_then(|c| serving(reg, c, now));
    corp.or_else(|| {
        let alliance = reg.character(ch)?.alliance_id?;
        serving(reg, alliance, now)
    })
}

/// "My faction": the direct enlistment, else the corporation's (R-FW §4,
/// `fwEnlistmentSvc.GetMyFaction`).
pub fn my_faction(reg: &Registry, ch: i64, now: i64) -> Option<i64> {
    direct_faction(reg, ch).or_else(|| corp_faction(reg, ch, now))
}

/// The `warfactionid` session value for `ch`, now: the login change reads
/// it (coordinator hunk in `Registry::character_change`).
pub(crate) fn war_faction(reg: &Registry, ch: i64) -> Option<i64> {
    my_faction(reg, ch, crate::pyutil::filetime_now())
}

/// What an owner (a corporation or alliance) serves, for the corporation
/// row's `warFactionID`: `Active` or `Leaving`, as [`serving`].
pub(crate) fn owner_faction(reg: &Registry, owner: i64) -> Option<i64> {
    serving(reg, owner, crate::pyutil::filetime_now())
}

/// **The `slim_extra::SOURCES` row** (FW-1): a pilot's ship or capsule
/// carries `warFactionID` while the pilot is enlisted, and no key when not.
/// `crimewatchSvc` reads it for the free-target test
/// (`hasattr(item, 'warFactionID') and IsCombatEnemyFaction(...)`), and
/// `state.CheckAtWarMilitia` and `CheckSameMilitia` read it for the overview
/// **[V code]**.
pub fn slim_extras(reg: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    ids.iter()
        .filter_map(|id| {
            let p = reg.roster().by_ship(*id)?;
            let faction = war_faction(reg, p.character_id)?;
            Some((*id, vec![("warFactionID", PyValue::Int(faction))]))
        })
        .collect()
}

/// `ch`'s rank in `faction`'s militia: R-FW §4's [I], the standing toward
/// the militia corporation truncated and clamped to `0..=`[`MAX_RANK`]. A
/// faction with no militia corporation in the content ranks 0.
pub fn rank(reg: &Registry, ch: i64, faction: i64) -> i64 {
    let Some(corp) = reg.ext::<Index>().militia_corp.get(&faction) else {
        return 0;
    };
    (reg.npc_standing(ch, *corp).trunc() as i64).clamp(0, MAX_RANK)
}

fn stored_rank(reg: &Registry, ch: i64, faction: i64) -> Option<i64> {
    reg.deltas()
        .of(RANK_KIND)
        .get(&[ch.to_string(), faction.to_string()][..])
        .map(|v| parse(v.first()))
}

fn store_rank(reg: &mut Registry, ch: i64, faction: i64, rank: i64) {
    reg.deltas_mut().put(
        RANK_KIND,
        vec![ch.to_string(), faction.to_string()],
        vec![rank.to_string()],
    );
}

/// The factions a member of `corp` may enlist in directly: the director's
/// list, else every warzone militia ([A] R-FW §4; falsified by the enlist
/// window warning about a faction the director never restricted).
pub fn allowed_factions(reg: &Registry, corp: i64) -> Vec<i64> {
    match reg.deltas().of(ALLOW_KIND).get(&[corp.to_string()][..]) {
        Some(v) => v
            .first()
            .map(|s| s.split(',').filter_map(|f| f.parse().ok()).collect())
            .unwrap_or_default(),
        None => reg.ext::<Index>().militias.clone(),
    }
}

/// A system's iHub LP pool.
pub fn lp_pool(reg: &Registry, system: i64) -> i64 {
    parse(
        reg.deltas()
            .of(LP_KIND)
            .get(&[system.to_string()][..])
            .and_then(|v| v.first()),
    )
}

// ---------------------------------------------------------------------------
// Pushes.

/// `OnEnlistmentStateUpdated(factionID, directFactionID, corpFactionID)`
/// [V] (`fwEnlistmentSvc:50-54`: the three stored as `_myFactionID`,
/// `_directFactionID`, `_corpFactionID`), to one character.
#[derive(Debug, Clone, PartialEq)]
pub struct EnlistmentStateUpdated {
    pub to: i64,
    pub faction: Option<i64>,
    pub direct: Option<i64>,
    pub corp: Option<i64>,
}

fn opt(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

impl crate::notify::Push for EnlistmentStateUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnEnlistmentStateUpdated",
            vec![opt(self.faction), opt(self.direct), opt(self.corp)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnEnlistmentCooldownCleared()` [V] (arity 0, `fwEnlistmentSvc:138`).
#[derive(Debug, Clone, PartialEq)]
pub struct EnlistmentCooldownCleared {
    pub to: i64,
}

impl crate::notify::Push for EnlistmentCooldownCleared {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnEnlistmentCooldownCleared", vec![])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnSolarSystemLPChange(oldpoints, newpoints)` [V] (`facWarSvc:87`), to
/// the characters in the system.
#[derive(Debug, Clone, PartialEq)]
pub struct SolarSystemLpChange {
    pub to: Vec<i64>,
    pub old: i64,
    pub new: i64,
}

impl crate::notify::Push for SolarSystemLpChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnSolarSystemLPChange",
            vec![PyValue::Int(self.old), PyValue::Int(self.new)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// The `warfactionid` session change, `None` when it does not move (the
/// client drops an unchanged pair anyway).
pub(crate) fn war_change(old: Option<i64>, new: Option<i64>) -> Option<Vec<(PyValue, PyValue)>> {
    if old == new {
        return None;
    }
    match crate::pyutil::pydict(vec![(
        "warfactionid",
        PyValue::Tuple(vec![opt(old), opt(new)]),
    )]) {
        PyValue::Dict(entries) => Some(entries),
        _ => None,
    }
}

/// After `ch`'s enlistment moved from `before`: the session change and the
/// state push.
fn tell(reg: &mut Registry, ch: i64, before: Option<i64>, now: i64) {
    let faction = my_faction(reg, ch, now);
    if let Some(change) = war_change(before, faction) {
        reg.queue_session_change(ch, change);
    }
    announce(reg, ch, before, now);
}

/// The push half of [`tell`]: `OnEnlistmentStateUpdated`, and the slim and
/// chat refresh when the faction moved from `before`. A corporation move
/// (`corp::move_character`) carries the session change itself.
pub(crate) fn announce(reg: &mut Registry, ch: i64, before: Option<i64>, now: i64) {
    let direct = direct_faction(reg, ch);
    let corp = corp_faction(reg, ch, now);
    let faction = direct.or(corp);
    reg.push(EnlistmentStateUpdated {
        to: ch,
        faction,
        direct,
        corp,
    });
    // A pilot in space: everyone near sees the new slim; chat's user data
    // follows the same value.
    if before != faction {
        crate::stance::refresh_slim(reg, ch);
        crate::chat_dir::refresh(reg, ch);
    }
}

// ---------------------------------------------------------------------------
// Mutators. Each takes `now`, so the tests can move time.

/// Our own words: `CustomNotify`, the shape the client raises itself
/// (`fwEnlistmentSvc:95`) [I].
fn refuse(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", bstr(text))]),
    ))
}

/// `facwarMinStandingsToJoinDefault` [V] (`appConst`, read with `tools/dis.py`):
/// a standing below it with the faction refuses (`enlistmentChecks.
/// GetStandingConst`, which reads `GetStanding(factionID, charID)`).
pub const MIN_STANDING_TO_JOIN: f64 = -0.0001;

/// `facWarSvc.IsCharacterInFwCooldown` [V]: walks the employment record newest
/// first, stops at the first job older than a day, and is true for a job in
/// that day at a militia corporation. It counts from joining, not leaving.
/// The day runs at the game clock's pace, as `leave` does [A]; falsified by
/// a scaled-clock test round where the window offers a rejoin early.
fn joined_a_militia_lately(reg: &Registry, ch: i64, now: i64) -> bool {
    let window = crate::game_clock::of(reg).span(ENLISTMENT_COOLDOWN_SECS * TICKS);
    let militia: Vec<i64> = reg.ext::<Index>().militia_corp.values().copied().collect();
    for (start, corp) in super::corp::employment_rows(reg, ch) {
        if start < now.saturating_sub(window) {
            break;
        }
        if militia.contains(&corp) {
            return true;
        }
    }
    false
}

/// The faction whose station `ch` is docked at: the station's owner
/// corporation's faction (`GetCurrentStationFactionIDs` [V]). Zarzakh's
/// hard-coded case is pirates only, so it never matters for a militia.
fn station_faction(reg: &Registry, ch: i64) -> Option<i64> {
    reg.docked_station(ch)?;
    // The owner the client shows for the station it is in (`stationItem.
    // ownerID`): set from the station row at every dock (`service.rs`).
    let owner = reg.character(ch)?.station_owner_id;
    reg.content()
        .try_get::<crate::npc_data::Table>()?
        .corporations
        .get(&owner)?
        .faction_id
}

/// The gates every enlistment passes (R-FW §4 steps 1-3): the faction the
/// pilot serves now, which is `None`.
fn check_enlist(reg: &Registry, ch: i64, faction: i64, now: i64) -> Result<(), ServiceError> {
    if !reg.ext::<Index>().militias.contains(&faction) {
        return Err(refuse("That faction has no militia to enlist in."));
    }
    if my_faction(reg, ch, now).is_some() {
        return Err(refuse("You are already enlisted in a militia."));
    }
    let (_, _, cooldown) = enlist_row(reg, ch);
    if now < cooldown {
        return Err(refuse(
            "You left a militia too recently to enlist again yet.",
        ));
    }
    Ok(())
}

/// **The office's own gates** (FW-4, #100), on top of [`check_enlist`]:
/// standing, a militia job in the last day, and the station. They hold for
/// a pilot asking at the window, and not for [`enlist`], which the tests and
/// tools call to seat a pilot wherever he is.
fn check_office(reg: &Registry, ch: i64, faction: i64, now: i64) -> Result<(), ServiceError> {
    if reg.npc_standing(ch, faction) < MIN_STANDING_TO_JOIN {
        return Err(refuse(
            "Your standing with that faction is too low to enlist in its militia.",
        ));
    }
    if joined_a_militia_lately(reg, ch, now) {
        return Err(refuse(
            "You joined a militia too recently to enlist again yet.",
        ));
    }
    if station_faction(reg, ch) != Some(faction) {
        return Err(refuse(
            "You must be docked at a station of that faction to enlist in its militia.",
        ));
    }
    Ok(())
}

/// Direct enlistment as a pilot asks for it: [`check_office`], then
/// [`enlist`].
pub fn request_enlist(
    reg: &mut Registry,
    ch: i64,
    faction: i64,
    now: i64,
) -> Result<(), ServiceError> {
    check_enlist(reg, ch, faction, now)?;
    check_office(reg, ch, faction, now)?;
    enlist(reg, ch, faction, now)
}

/// Direct enlistment in `faction` (R-FW §4 steps 1-4), without the office's
/// gates ([`request_enlist`] adds them).
pub fn enlist(reg: &mut Registry, ch: i64, faction: i64, now: i64) -> Result<(), ServiceError> {
    check_enlist(reg, ch, faction, now)?;
    let before = my_faction(reg, ch, now);
    if player_corp(reg, ch).is_some_and(|corp| !allowed_factions(reg, corp).contains(&faction)) {
        return Err(refuse("Your corporation does not allow that militia."));
    }
    reg.deltas_mut().put(
        KIND,
        vec![ch.to_string()],
        vec![faction.to_string(), now.to_string(), "0".to_owned()],
    );
    if stored_rank(reg, ch, faction).is_none() {
        let r = rank(reg, ch, faction);
        store_rank(reg, ch, faction, r);
    }
    tell(reg, ch, before, now);
    Ok(())
}

/// `JoinFactionAsCharacter` (FW-2): a pilot in an NPC corporation moves into
/// the militia's corporation, with no `fwenlist` row, in one session change
/// carrying `corpid` and `warfactionid` (`corp.joinmilitia`). A pilot in a
/// player corporation enlists directly [A]: the window never reaches this
/// call for them, so nothing local falsifies it.
pub fn join_militia(
    reg: &mut Registry,
    ch: i64,
    faction: i64,
    now: i64,
) -> Result<(), ServiceError> {
    if player_corp(reg, ch).is_some() {
        return request_enlist(reg, ch, faction, now);
    }
    check_enlist(reg, ch, faction, now)?;
    check_office(reg, ch, faction, now)?;
    let Some(corp) = reg.ext::<Index>().militia_corp.get(&faction).copied() else {
        return Err(refuse("That faction has no militia corporation."));
    };
    if stored_rank(reg, ch, faction).is_none() {
        let r = rank(reg, ch, faction);
        store_rank(reg, ch, faction, r);
    }
    super::corp::join_npc_corp(reg, ch, corp);
    Ok(())
}

/// A pilot left a militia corporation by retiring (or was moved out): the
/// 24 h cooldown starts, as a direct leave starts it, and the rank is kept.
/// Called by `corp::move_character`, which sends the session change.
pub(crate) fn retired(reg: &mut Registry, ch: i64, faction: i64, now: i64) {
    let clock = crate::game_clock::of(reg);
    let until = now.saturating_add(clock.span(ENLISTMENT_COOLDOWN_SECS * TICKS));
    reg.deltas_mut().put(
        KIND,
        vec![ch.to_string()],
        vec!["0".to_owned(), now.to_string(), until.to_string()],
    );
    let r = rank(reg, ch, faction);
    store_rank(reg, ch, faction, r);
    reg.schedule(
        clock.duration(Duration::from_secs(ENLISTMENT_COOLDOWN_SECS as u64)),
        EnlistmentCooldownCleared { to: ch },
    );
}

/// Leave the direct enlistment: the cooldown starts, and
/// `OnEnlistmentCooldownCleared` is scheduled for its end ([A] a restart
/// loses the scheduled push; the client re-reads the timestamp on login).
pub fn leave(reg: &mut Registry, ch: i64, now: i64) -> Result<(), ServiceError> {
    let before = my_faction(reg, ch, now);
    let Some(faction) = direct_faction(reg, ch) else {
        return Err(refuse("You are not enlisted in a militia directly."));
    };
    // Lane clock: the cooldown at the game clock's pace. `until` is stored;
    // the scheduled push is lost on a restart anyway (the [A] above).
    let clock = crate::game_clock::of(reg);
    let until = now.saturating_add(clock.span(ENLISTMENT_COOLDOWN_SECS * TICKS));
    reg.deltas_mut().put(
        KIND,
        vec![ch.to_string()],
        vec!["0".to_owned(), now.to_string(), until.to_string()],
    );
    let r = rank(reg, ch, faction);
    store_rank(reg, ch, faction, r);
    tell(reg, ch, before, now);
    reg.schedule(
        clock.duration(Duration::from_secs(ENLISTMENT_COOLDOWN_SECS as u64)),
        EnlistmentCooldownCleared { to: ch },
    );
    Ok(())
}

/// The cooldown's end as a FILETIME, `None` when there is none (`None >
/// now` is `False` in Python 2: "no cooldown", `facWarSvc:406-407`).
pub fn cooldown_until(reg: &Registry, ch: i64, now: i64) -> Option<i64> {
    let (_, _, until) = enlist_row(reg, ch);
    (until > now).then_some(until)
}

/// Which owner a corporation or alliance call acts for, and whether the
/// caller may: a director of a player corp (`enlistmentUtil` offers these
/// only to directors [I]).
fn acting_owner(reg: &Registry, ch: i64, alliance: bool) -> Result<i64, ServiceError> {
    let Some(m) = reg.corporations.membership(ch) else {
        return Err(refuse("Only a player corporation can do that."));
    };
    if m.roles & super::corp::CORP_ROLE_DIRECTOR == 0 {
        return Err(refuse("Only a director can do that."));
    }
    if alliance {
        reg.character(ch)
            .and_then(|c| c.alliance_id)
            .ok_or_else(|| refuse("Your corporation is in no alliance."))
    } else {
        Ok(m.corporation_id)
    }
}

/// The corporation and alliance arms (R-FW §1 #6-#13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpOp {
    Join,
    Leave,
    WithdrawJoin,
    WithdrawLeave,
}

/// Apply one corporation or alliance arm for `owner`, in `faction`.
pub fn corp_op(
    reg: &mut Registry,
    owner: i64,
    faction: i64,
    op: CorpOp,
    now: i64,
) -> Result<(), ServiceError> {
    let state = corp_state(reg, owner, now);
    let key = vec![owner.to_string()];
    let put = |reg: &mut Registry, status: i64, since: i64| {
        reg.deltas_mut().put(
            CORP_KIND,
            key.clone(),
            vec![faction.to_string(), status.to_string(), since.to_string()],
        );
    };
    match (op, state) {
        (CorpOp::Join, None) => {
            if !reg.ext::<Index>().militias.contains(&faction) {
                return Err(refuse("That faction has no militia to join."));
            }
            put(reg, STATUS_JOINING, now);
        }
        (CorpOp::Leave, Some((f, STATUS_ACTIVE, _))) if f == faction => {
            put(reg, STATUS_LEAVING, now);
        }
        (CorpOp::WithdrawJoin, Some((f, STATUS_JOINING, _))) if f == faction => {
            reg.deltas_mut().del(CORP_KIND, &key);
        }
        (CorpOp::WithdrawLeave, Some((f, STATUS_LEAVING, since))) if f == faction => {
            // Back to the service it never left: the original join date is
            // lost [A], so the leave request's date stands in.
            put(reg, STATUS_ACTIVE, since);
        }
        _ => return Err(refuse("That is not possible from the current status.")),
    }
    super::notifications::militia_request(reg, owner, faction, op);
    Ok(())
}

/// `DonateLPsToSolarSystem(pointsDonated, pointsToIhub)` for `ch`: debit
/// the militia corp's LP, add to `system`'s pool, push the change to the
/// system. The client pre-checks both refusals with its own `UserError`s
/// (`facWarSvc:671-679`); these are the server's half.
pub fn donate(
    reg: &mut Registry,
    ch: i64,
    system: i64,
    donated: i64,
    to_ihub: i64,
    now: i64,
) -> Result<(), ServiceError> {
    let corp = my_faction(reg, ch, now)
        .and_then(|f| reg.ext::<Index>().militia_corp.get(&f).copied())
        .ok_or_else(|| refuse("You are not in a militia."))?;
    let old = lp_pool(reg, system);
    let new = old.saturating_add(to_ihub.max(0));
    if donated < 0 || new > MAX_LP_POOL {
        return Err(refuse("The pool cannot take that many points."));
    }
    crate::loyalty::adjust(reg, ch, corp, -donated, "FacWarDonation")
        .ok_or_else(|| refuse("You do not have that many loyalty points."))?;
    reg.deltas_mut()
        .put(LP_KIND, vec![system.to_string()], vec![new.to_string()]);
    let to = in_system(reg, system);
    reg.push(SolarSystemLpChange { to, old, new });
    Ok(())
}

// ---------------------------------------------------------------------------
// Readers that build wire values.

/// Every character whose session system is `system`.
fn in_system(reg: &Registry, system: i64) -> Vec<i64> {
    reg.config()
        .characters
        .iter()
        .map(|c| c.character_id)
        .filter(|c| reg.session_system(*c) == Some(system))
        .collect()
}

/// `KeyVal(currentRank, factionID)` for `ch`, `None` outside FW (R-FW §1
/// #15: falsy is "not in FW", `progression:125`).
fn rank_info(reg: &Registry, ch: i64, now: i64) -> PyValue {
    match my_faction(reg, ch, now) {
        Some(f) => keyval(vec![
            ("currentRank", PyValue::Int(rank(reg, ch, f))),
            ("factionID", PyValue::Int(f)),
        ]),
        None => PyValue::None,
    }
}

/// `[KeyVal(factionID, currentRank)]`, one per faction ever served (R-FW §1
/// #17), the current one at its live rank. Never `None`: it is iterated
/// (`infosvc:2803`, `decorationsPanel:207-208`).
fn rank_overview(reg: &Registry, ch: i64, now: i64) -> PyValue {
    let me = ch.to_string();
    let mut ranks: BTreeMap<i64, i64> = reg
        .deltas()
        .of(RANK_KIND)
        .iter()
        .filter(|(k, _)| k.first() == Some(&me))
        .filter_map(|(k, v)| Some((k.get(1)?.parse().ok()?, parse(v.first()))))
        .collect();
    if let Some(f) = my_faction(reg, ch, now) {
        ranks.insert(f, rank(reg, ch, f));
    }
    PyValue::List(
        ranks
            .into_iter()
            .map(|(f, r)| {
                keyval(vec![
                    ("factionID", PyValue::Int(f)),
                    ("currentRank", PyValue::Int(r)),
                ])
            })
            .collect(),
    )
}

/// `{statKey: n}`, all six keys (`personal:174,187` subscript them), from
/// [`crate::fw_stats::stats_of`]'s order.
fn stat_dict(n: [i64; 6]) -> PyValue {
    crate::pyutil::pydict(
        STAT_KEYS
            .iter()
            .zip(n)
            .map(|(k, n)| (*k, PyValue::Int(n)))
            .collect(),
    )
}

/// The size of the largest player corporation serving `faction`
/// (`topMemberCount`).
fn top_member_count(reg: &Registry, faction: i64, now: i64) -> i64 {
    let mut per: BTreeMap<i64, i64> = BTreeMap::new();
    for c in &reg.config().characters {
        if my_faction(reg, c.character_id, now) == Some(faction)
            && let Some(corp) = player_corp(reg, c.character_id)
        {
            *per.entry(corp).or_default() += 1;
        }
    }
    per.values().copied().max().unwrap_or(0)
}

/// Every character enlisted in `faction`, directly or through a corp.
fn members_of(reg: &Registry, faction: i64, now: i64) -> (usize, usize) {
    let ids: BTreeSet<i64> = reg
        .config()
        .characters
        .iter()
        .map(|c| c.character_id)
        .collect();
    // The militia count (`statisticsController.GetCorpPilots`) takes the
    // pilots in the militia corporation too (FW-2).
    let direct = ids
        .iter()
        .filter(|c| {
            direct_faction(reg, **c) == Some(faction) || militia_of_corp(reg, **c) == Some(faction)
        })
        .count();
    let total = ids
        .iter()
        .filter(|c| my_faction(reg, **c, now) == Some(faction))
        .count();
    (total, direct)
}

fn int_arg(req: &Request<'_>, i: usize) -> Result<i64, ServiceError> {
    match req.args.get(i) {
        Some(PyValue::Int(n)) => Ok(*n),
        other => Err(ServiceError::BadArguments {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
            why: format!("argument {i} is not an int: {other:?}"),
        }),
    }
}

fn actor(req: &Request<'_>) -> Result<i64, ServiceError> {
    req.actor.ok_or_else(|| ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: "no character on this session".to_owned(),
    })
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    dispatch_at(reg, req, crate::pyutil::filetime_now())
}

/// [`dispatch`] at `now`, a FILETIME.
pub(crate) fn dispatch_at(
    reg: &mut Registry,
    req: &Request<'_>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    match req.service {
        "fwCharacterEnlistmentMgr" => enlistment_mgr(reg, req, now),
        "facWarMgr" => fac_war_mgr(reg, req, now),
        // Unreachable: `SERVICES` gates the call.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// `fwCharacterEnlistmentMgr` (R-FW §1, sibling remotes).
fn enlistment_mgr(
    reg: &mut Registry,
    req: &Request<'_>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    match req.method {
        // **A 3-tuple, unpacked before anything is tested**
        // (`fwEnlistmentSvc:80`, `UNPACK_SEQUENCE 3`, moved here from
        // `service/facwar.rs`): `None` is the LP live lock, a `TypeError`
        // that skips the three stores so the call re-issues on every read.
        // `(None,)*3` fills the cache because the client's sentinel is the
        // string `'FACTION_ID_NOT_SET'` (`:6`), and it is "not enlisted":
        // `IsEnlistedDirectly`/`IsEnlistedViaCorp` test the last two `is not
        // None`. The first is "my faction" (R-FW §4).
        "GetMyEnlistment" => {
            let (direct, corp) = match req.actor {
                Some(ch) => (direct_faction(reg, ch), corp_faction(reg, ch, now)),
                None => (None, None),
            };
            Ok(PyValue::Tuple(vec![
                opt(direct.or(corp)),
                opt(direct),
                opt(corp),
            ]))
        }
        // Returned to the caller and ignored (`fwEnlistmentSvc:98`).
        "CreateMyDirectEnlistment" => {
            let (ch, f) = (actor(req)?, int_arg(req, 0)?);
            request_enlist(reg, ch, f, now).map(|()| PyValue::None)
        }
        "RemoveMyDirectEnlistment" => leave(reg, actor(req)?, now).map(|()| PyValue::None),
        // A list of faction ids: `in` at `enlistmentChecks:74`, `sorted` at
        // `corp_ui_home:605`.
        "GetCorpAllowedEnlistmentFactions" => {
            let corp = int_arg(req, 0)?;
            Ok(PyValue::List(
                allowed_factions(reg, corp)
                    .into_iter()
                    .map(PyValue::Int)
                    .collect(),
            ))
        }
        "SetMyCorpAllowedEnlistmentFactions" => {
            let corp = acting_owner(reg, actor(req)?, false)?;
            let ids: Vec<String> = match req.args.first() {
                Some(PyValue::List(v) | PyValue::Tuple(v)) => v
                    .iter()
                    .filter_map(|f| match f {
                        PyValue::Int(n) => Some(n.to_string()),
                        _ => None,
                    })
                    .collect(),
                other => {
                    return Err(ServiceError::BadArguments {
                        service: req.service.to_owned(),
                        method: req.method.to_owned(),
                        why: format!("not a list of faction ids: {other:?}"),
                    });
                }
            };
            reg.deltas_mut()
                .put(ALLOW_KIND, vec![corp.to_string()], vec![ids.join(",")]);
            Ok(PyValue::None)
        }
        // **The name the client calls** (R-FW §0 item 3,
        // `fwEnlistmentSvc:135`): a FILETIME, or `None` for "no cooldown".
        "GetMyDirectEnlistmentCooldownTimestamp" => {
            Ok(opt(req.actor.and_then(|ch| cooldown_until(reg, ch, now))))
        }
        _ => Err(reg.no_method(req)),
    }
}

/// `facWarMgr` (R-FW §1, rows #1-#29).
fn fac_war_mgr(reg: &mut Registry, req: &Request<'_>, now: i64) -> Result<PyValue, ServiceError> {
    let corp_arm = |op: CorpOp, alliance: bool, reg: &mut Registry| {
        let owner = acting_owner(reg, actor(req)?, alliance)?;
        corp_op(reg, owner, int_arg(req, 0)?, op, now).map(|()| PyValue::None)
    };
    match req.method {
        // #1, #3: a factionID or `None` (`facWarSvc:147-150`).
        "GetCorporationWarFactionID" | "GetAllianceWarFactionID" => {
            Ok(opt(serving(reg, int_arg(req, 0)?, now)))
        }
        // #2: the corporations serving `factionID`.
        "GetFactionCorporations" => {
            let f = int_arg(req, 0)?;
            let ids: Vec<PyValue> = reg
                .corporations
                .all()
                .map(|c| c.corporation_id)
                .filter(|c| serving(reg, *c, now) == Some(f))
                .map(PyValue::Int)
                .collect();
            Ok(PyValue::List(ids))
        }
        // #4: a corpID, or `None` (which breaks only the donation, `:668`).
        "GetFactionMilitiaCorporation" => {
            let f = int_arg(req, 0)?;
            Ok(opt(reg.ext::<Index>().militia_corp.get(&f).copied()))
        }
        // #5: through `PerformSessionChange('corp.joinmilitia')`; the answer
        // is ignored, the `warfactionid` change is the effect.
        "JoinFactionAsCharacter" => {
            let (ch, f) = (actor(req)?, int_arg(req, 0)?);
            join_militia(reg, ch, f, now).map(|()| PyValue::None)
        }
        // #6-#13: answers ignored (`POP_TOP`).
        "JoinFactionAsAlliance" => corp_arm(CorpOp::Join, true, reg),
        "JoinFactionAsCorporation" => corp_arm(CorpOp::Join, false, reg),
        "LeaveFactionAsAlliance" => corp_arm(CorpOp::Leave, true, reg),
        "LeaveFactionAsCorporation" => corp_arm(CorpOp::Leave, false, reg),
        "WithdrawJoinFactionAsAlliance" => corp_arm(CorpOp::WithdrawJoin, true, reg),
        "WithdrawJoinFactionAsCorporation" => corp_arm(CorpOp::WithdrawJoin, false, reg),
        "WithdrawLeaveFactionAsAlliance" => corp_arm(CorpOp::WithdrawLeave, true, reg),
        "WithdrawLeaveFactionAsCorporation" => corp_arm(CorpOp::WithdrawLeave, false, reg),
        // #14: `None` in an NPC corp [I]; a player corp's
        // `KeyVal(status, factionID, startDate)`, all `None` outside FW [A]
        // (`enlistCont:114-117` reads `.status` unguarded for a director).
        "GetCorpFactionalWarStatus" => {
            let Some(corp) = req.actor.and_then(|ch| player_corp(reg, ch)) else {
                return Ok(PyValue::None);
            };
            let (f, s, d) = corp_state(reg, corp, now)
                .map_or((None, None, None), |(f, s, d)| (Some(f), Some(s), Some(d)));
            Ok(keyval(vec![
                ("status", opt(s)),
                ("factionID", opt(f)),
                ("startDate", opt(d)),
            ]))
        }
        // #15, #16.
        "GetMyCharacterRankInfo" => Ok(req
            .actor
            .map_or(PyValue::None, |ch| rank_info(reg, ch, now))),
        "GetCharacterRankInfo" => Ok(rank_info(reg, int_arg(req, 0)?, now)),
        // #17.
        "GetMyCharacterRankOverview" => Ok(req
            .actor
            .map_or(PyValue::List(vec![]), |ch| rank_overview(reg, ch, now))),
        // #18: no UI caller.
        "RefreshCorps" => Ok(PyValue::None),
        // #19: the new rank when it differs from the stored one, else `None`
        // (`facWarSvc:434`).
        "CheckForRankChange" => {
            let Some(ch) = req.actor else {
                return Ok(PyValue::None);
            };
            let Some(f) = my_faction(reg, ch, now) else {
                return Ok(PyValue::None);
            };
            let r = rank(reg, ch, f);
            if stored_rank(reg, ch, f) == Some(r) {
                return Ok(PyValue::None);
            }
            crate::notifications::militia_rank(reg, ch, f, stored_rank(reg, ch, f), r);
            store_rank(reg, ch, f, r);
            Ok(PyValue::Int(r))
        }
        // #20: `{factionID: KeyVal(...)}`, read with `getattr(v, name, 0)`
        // (`statisticsController:162-163`). `systemsCount` is the warzone
        // systems the militia holds (CP53b); `topMemberCount` is the largest
        // player corporation serving it (FW-5).
        "GetStats_FactionInfo" => {
            let militias = reg.ext::<Index>().militias.clone();
            Ok(PyValue::Dict(
                militias
                    .into_iter()
                    .map(|f| {
                        let (total, direct) = members_of(reg, f, now);
                        let v = keyval(vec![
                            ("totalMembersCount", PyValue::Int(total as i64)),
                            (
                                "systemsCount",
                                PyValue::Int(crate::facwar::systems_held(reg, f) as i64),
                            ),
                            ("militiaMembersCount", PyValue::Int(direct as i64)),
                            (
                                "topMemberCount",
                                PyValue::Int(top_member_count(reg, f, now)),
                            ),
                        ]);
                        (v, PyValue::Int(f))
                    })
                    .collect(),
            ))
        }
        // #21: `(top, all)`, each `{groupID: {statKey: n}}`
        // (`facWarSvc:480-481` subscripts `[0|1][groupCharacter][k]`).
        "GetStats_TopAndAllKillsAndVPs" => {
            let per: Vec<([i64; 6], [i64; 6])> = STAT_GROUPS
                .iter()
                .map(|g| crate::fw_stats::top_and_all(reg, *g, now))
                .collect();
            let half = |all: bool| {
                PyValue::Dict(
                    STAT_GROUPS
                        .iter()
                        .zip(&per)
                        .map(|(g, p)| (stat_dict(if all { p.1 } else { p.0 }), PyValue::Int(*g)))
                        .collect(),
                )
            };
            Ok(PyValue::Tuple(vec![half(false), half(true)]))
        }
        // #22-#24: `{statKey: n}`, every key present.
        // Zeros for an owner with no row, or for a caller with no corp or
        // alliance.
        "GetStats_Character" | "GetStats_Corp" | "GetStats_Alliance" => {
            let (group, owner) = match req.method {
                "GetStats_Character" => (STAT_GROUPS[0], req.actor),
                "GetStats_Corp" => (STAT_GROUPS[1], req.actor.and_then(|c| player_corp(reg, c))),
                _ => (
                    STAT_GROUPS[2],
                    req.actor
                        .and_then(|c| reg.character(c))
                        .and_then(|c| c.alliance_id),
                ),
            };
            Ok(stat_dict(owner.map_or([0; 6], |o| {
                crate::fw_stats::stats_of(reg, group, o, now)
            })))
        }
        // #25: `{'header': [factionID...], 'data': {statKey: {factionID: n}}}`.
        "GetStats_Militia" => {
            let militias = reg.ext::<Index>().militias.clone();
            let rows: Vec<[i64; 6]> = militias
                .iter()
                .map(|f| crate::fw_stats::stats_of(reg, crate::fw_stats::FACTION, *f, now))
                .collect();
            let per = |i: usize| {
                PyValue::Dict(
                    militias
                        .iter()
                        .zip(&rows)
                        .map(|(f, r)| (PyValue::Int(r[i]), PyValue::Int(*f)))
                        .collect(),
                )
            };
            Ok(crate::pyutil::pydict(vec![
                (
                    "header",
                    PyValue::List(militias.iter().map(|f| PyValue::Int(*f)).collect()),
                ),
                (
                    "data",
                    crate::pyutil::pydict(
                        STAT_KEYS
                            .iter()
                            .enumerate()
                            .map(|(i, k)| (*k, per(i)))
                            .collect(),
                    ),
                ),
            ]))
        }
        // #26: an int; the enlisted members of the caller's corp.
        "GetStats_CorpPilots" => {
            let n = req
                .actor
                .and_then(|ch| player_corp(reg, ch))
                .map_or(0, |corp| {
                    reg.corporations
                        .members_of(corp)
                        .into_iter()
                        .filter(|m| my_faction(reg, *m, now).is_some())
                        .count()
                });
            Ok(PyValue::Int(n as i64))
        }
        // #27: an int (`facWarSvc:676` adds it).
        "GetSolarSystemLPs" => Ok(PyValue::Int(lp_pool(reg, int_arg(req, 0)?))),
        // #28: returned and ignored.
        "DonateLPsToSolarSystem" => {
            let ch = actor(req)?;
            let (donated, to_ihub) = (int_arg(req, 0)?, int_arg(req, 1)?);
            let system = reg
                .session_system(ch)
                .ok_or_else(|| refuse("You are nowhere."))?;
            donate(reg, ch, system, donated, to_ihub, now).map(|()| PyValue::None)
        }
        // #29: an itemID or `None`; falsy returns (`:53-55`). No bunker
        // balls until CP53b.
        "GetControlBunkerIDInSystem" => Ok(PyValue::None),
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const ME: i64 = 90_000_101;
    const YOU: i64 = 90_000_102;
    const SYS: i64 = 39_000_101;
    /// Invented militias, and F's militia corp (G has none).
    const F: i64 = 599_101;
    const G: i64 = 599_102;
    const F_CORP: i64 = 1_999_101;
    const NOT_A_MILITIA: i64 = 599_199;
    const F_STN: i64 = 6_900_101;
    const G_STN: i64 = 6_900_102;
    const G_CORP: i64 = 1_999_102;
    const CORP: i64 = 98_000_101;
    const T0: i64 = 130_000_000_000_000_000;
    const FW: &str = "facWarMgr";
    const EN: &str = "fwCharacterEnlistmentMgr";

    fn i(n: i64) -> PyValue {
        PyValue::Int(n)
    }

    fn pilot(id: i64) -> crate::model::Character {
        crate::model::Character {
            character_id: id,
            ship_id: id + 100,
            solar_system_id: Some(SYS),
            station_id: Some(F_STN),
            station_owner_id: F_CORP,
            ..crate::model::Character::default()
        }
    }

    fn world() -> Registry {
        let mut c = Content::fixture();
        let zone = crate::facwar::Warzone {
            id: 97,
            factions: [F, G],
            hqs: [SYS, SYS + 1],
        };
        c.insert("facwar", crate::facwar::Table::new(vec![zone]).expect("ok"));
        let mut npc = crate::npc_data::Table::default();
        let militia = crate::npc_data::Faction {
            faction_id: F,
            militia_corporation_id: Some(F_CORP),
            ..crate::npc_data::Faction::default()
        };
        npc.factions.insert(F, militia);
        for (corp, faction) in [(F_CORP, F), (G_CORP, G)] {
            let row = crate::npc_data::Corporation {
                corp_id: corp,
                faction_id: Some(faction),
                ..crate::npc_data::Corporation::default()
            };
            npc.corporations.insert(corp, row);
        }
        c.insert("npc_data", npc);
        let cfg = SandboxConfig {
            characters: vec![pilot(ME), pilot(YOU)],
            ..SandboxConfig::default()
        };
        Registry::new(cfg).with_content(Arc::new(c))
    }

    fn dock(reg: &mut Registry, who: i64, station: i64) {
        let owner = if station == F_STN { F_CORP } else { G_CORP };
        let pilot = reg.character_mut(who).expect("a pilot");
        pilot.station_id = Some(station);
        pilot.station_owner_id = owner;
    }

    /// `ME` a director of player corp `CORP`, `YOU` a plain member.
    fn with_corp(reg: &mut Registry) {
        let director = super::super::corp::CORP_ROLE_DIRECTOR;
        for rec in [
            format!(
                "corp\t{CORP}\t{ME}\t{ME}\t0\t0.1\t0.1\t1\t0\t-1\t-1\t-1\t-1\t-1\t-1\tF\tF\t\t"
            ),
            format!("corpmember\t{ME}\t{CORP}\t{director}\t0"),
            format!("corpmember\t{YOU}\t{CORP}\t0\t0"),
        ] {
            let f: Vec<&str> = rec.split('\t').collect();
            reg.corporations.parse_record(&f).expect("a record");
        }
    }

    fn try_at(
        reg: &mut Registry,
        who: i64,
        (service, method): (&str, &str),
        args: &[PyValue],
        now: i64,
    ) -> Result<PyValue, ServiceError> {
        let req = Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        };
        dispatch_at(reg, &req, now)
    }

    fn at(reg: &mut Registry, who: i64, sm: (&str, &str), args: &[PyValue], now: i64) -> PyValue {
        try_at(reg, who, sm, args, now).unwrap_or_else(|e| panic!("{sm:?}: {e:?}"))
    }

    fn call(reg: &mut Registry, who: i64, sm: (&str, &str), args: &[PyValue]) -> PyValue {
        at(reg, who, sm, args, T0)
    }

    fn enlistment(reg: &mut Registry, who: i64, now: i64) -> PyValue {
        at(reg, who, (EN, "GetMyEnlistment"), &[], now)
    }

    fn tuple3(a: Option<i64>, b: Option<i64>, c: Option<i64>) -> PyValue {
        PyValue::Tuple(vec![opt(a), opt(b), opt(c)])
    }

    /// Every push named `event`, as `(args, audience)`.
    fn pushes(reg: &mut Registry, event: &str) -> Vec<(Vec<PyValue>, Option<Vec<i64>>)> {
        reg.take_pending()
            .into_iter()
            .filter_map(|(_, p)| {
                let (name, args) = p.to_push();
                (name == event).then(|| (args, p.audience()))
            })
            .collect()
    }

    fn war_changes(reg: &mut Registry) -> Vec<(i64, PyValue)> {
        reg.take_session_changes()
            .into_iter()
            .filter_map(|(ch, kv)| {
                Some((ch, crate::pyutil::pairs_get(&kv, b"warfactionid")?.clone()))
            })
            .collect()
    }

    /// An attribute of a `KeyVal`, or a key of a dict.
    fn attr(v: &PyValue, name: &str) -> PyValue {
        let dict = match v {
            PyValue::Instance { state, .. } => &**state,
            other => other,
        };
        crate::pyutil::dict_get(dict, name.as_bytes())
            .cloned()
            .unwrap_or_else(|| panic!("{name} missing on {v:?}"))
    }

    /// The value under int key `k` of a dict.
    fn at_key(v: &PyValue, k: i64) -> PyValue {
        let kv = v.dict_as_kv().expect("a dict");
        let (_, v) = kv
            .into_iter()
            .find(|(key, _)| **key == i(k))
            .expect("the key");
        v.clone()
    }

    fn has_stat_keys(v: &PyValue) -> bool {
        STAT_KEYS
            .iter()
            .all(|k| matches!(attr(v, k), PyValue::Int(_)))
    }

    /// R-FW §1: every `facWarMgr` method and every enlistment sibling
    /// answers, with the type its consumer pins.
    #[test]
    fn every_method_r_fw_lists_answers_its_shape() {
        let mut reg = world();
        let mut m = |name: &str, args: &[PyValue]| call(&mut reg, ME, (FW, name), args);
        let none_or_int = |v: PyValue| matches!(v, PyValue::None | PyValue::Int(_));
        let list = |v: PyValue| matches!(v, PyValue::List(_));
        assert!(none_or_int(m("GetCorporationWarFactionID", &[i(CORP)])));
        assert!(none_or_int(m("GetAllianceWarFactionID", &[i(1)])));
        assert!(list(m("GetFactionCorporations", &[i(F)])));
        assert_eq!(m("GetFactionMilitiaCorporation", &[i(F)]), i(F_CORP));
        assert_eq!(
            m("GetCorpFactionalWarStatus", &[]),
            PyValue::None,
            "NPC corp"
        );
        assert_eq!(m("GetMyCharacterRankInfo", &[]), PyValue::None, "not in FW");
        assert!(list(m("GetMyCharacterRankOverview", &[])));
        assert_eq!(m("RefreshCorps", &[]), PyValue::None);
        assert!(none_or_int(m("CheckForRankChange", &[])));

        let info = m("GetStats_FactionInfo", &[]);
        for f in [F, G] {
            let row = at_key(&info, f);
            for a in [
                "totalMembersCount",
                "systemsCount",
                "militiaMembersCount",
                "topMemberCount",
            ] {
                assert!(matches!(attr(&row, a), PyValue::Int(_)), "{a}");
            }
        }
        let PyValue::Tuple(top_all) = m("GetStats_TopAndAllKillsAndVPs", &[]) else {
            panic!("(top, all)")
        };
        assert_eq!(top_all.len(), 2);
        for half in &top_all {
            for g in STAT_GROUPS {
                assert!(has_stat_keys(&at_key(half, g)), "group {g}");
            }
        }
        for name in ["GetStats_Character", "GetStats_Corp", "GetStats_Alliance"] {
            assert!(has_stat_keys(&m(name, &[])), "{name}");
        }
        let militia = m("GetStats_Militia", &[]);
        assert!(list(attr(&militia, "header")));
        assert!(matches!(attr(&militia, "data"), PyValue::Dict(_)));
        assert!(matches!(m("GetStats_CorpPilots", &[]), PyValue::Int(_)));
        assert!(matches!(m("GetSolarSystemLPs", &[i(SYS)]), PyValue::Int(_)));
        assert_eq!(m("GetControlBunkerIDInSystem", &[]), PyValue::None);

        let mut e = |name: &str, args: &[PyValue]| call(&mut reg, ME, (EN, name), args);
        assert_eq!(e("GetMyEnlistment", &[]), tuple3(None, None, None));
        assert!(list(e("GetCorpAllowedEnlistmentFactions", &[i(CORP)])));
        // R-FW §0 item 3: the name the client calls; `None` is no cooldown.
        assert_eq!(
            e("GetMyDirectEnlistmentCooldownTimestamp", &[]),
            PyValue::None
        );
    }

    /// Joining and leaving a militia change `GetMyEnlistment`, queue the
    /// `warfactionid` session change and push `OnEnlistmentStateUpdated`
    /// with three arguments to the character [V arity]; leaving starts the
    /// cooldown, which refuses a rejoin and schedules the cleared push.
    #[test]
    fn join_and_leave_change_my_enlistment_and_push_the_state() {
        let mut reg = world();
        const STATE: &str = "OnEnlistmentStateUpdated";
        call(&mut reg, ME, (EN, "CreateMyDirectEnlistment"), &[i(F)]);
        assert_eq!(enlistment(&mut reg, ME, T0), tuple3(Some(F), Some(F), None));
        assert_eq!(enlistment(&mut reg, YOU, T0), tuple3(None, None, None));
        let joined = vec![(vec![i(F), i(F), PyValue::None], Some(vec![ME]))];
        assert_eq!(pushes(&mut reg, STATE), joined);
        let to_f = PyValue::Tuple(vec![PyValue::None, i(F)]);
        assert_eq!(war_changes(&mut reg), vec![(ME, to_f)]);

        let scheduled = reg.scheduled_count();
        call(&mut reg, ME, (EN, "RemoveMyDirectEnlistment"), &[]);
        assert_eq!(enlistment(&mut reg, ME, T0), tuple3(None, None, None));
        let left = vec![(vec![PyValue::None; 3], Some(vec![ME]))];
        assert_eq!(pushes(&mut reg, STATE), left);
        let from_f = PyValue::Tuple(vec![i(F), PyValue::None]);
        assert_eq!(war_changes(&mut reg), vec![(ME, from_f)]);
        assert_eq!(reg.scheduled_count(), scheduled + 1, "the cleared push");
        let until = cooldown_until(&reg, ME, T0).expect("a cooldown");
        let stamp = call(
            &mut reg,
            ME,
            (EN, "GetMyDirectEnlistmentCooldownTimestamp"),
            &[],
        );
        assert_eq!(stamp, i(until));
        assert!(
            enlist(&mut reg, ME, F, until - 1).is_err(),
            "in the cooldown"
        );
        assert!(pushes(&mut reg, STATE).is_empty());
        dock(&mut reg, ME, G_STN);
        enlist(&mut reg, ME, G, until).expect("the cooldown is over");
        assert_eq!(
            enlistment(&mut reg, ME, until),
            tuple3(Some(G), Some(G), None)
        );
    }

    /// The legacy `facWarMgr.JoinFactionAsCharacter` is the same enlistment,
    /// and a faction outside every warzone, or a second enlistment, is
    /// refused without a push. A login afterwards sets `warfactionid`.
    #[test]
    fn join_faction_as_character_enlists_and_refusals_change_nothing() {
        let mut reg = world();
        let join = (FW, "JoinFactionAsCharacter");
        assert!(try_at(&mut reg, ME, join, &[i(NOT_A_MILITIA)], T0).is_err());
        assert_eq!(enlistment(&mut reg, ME, T0), tuple3(None, None, None));
        call(&mut reg, ME, join, &[i(F)]);
        // The window's Join from an NPC corp moves the pilot into the
        // militia corporation (FW-2): enlisted via the corp, not directly.
        assert_eq!(enlistment(&mut reg, ME, T0), tuple3(Some(F), None, Some(F)));
        let _ = reg.take_pending();
        assert!(enlist(&mut reg, ME, G, T0).is_err(), "already enlisted");
        assert!(reg.take_pending().is_empty());
        assert_eq!(war_faction(&reg, ME), Some(F));
        let login = reg.character_change(ME).expect("a character");
        assert_eq!(
            crate::pyutil::pairs_get(&login, b"warfactionid"),
            Some(&PyValue::Tuple(vec![PyValue::None, i(F)])),
            "the login carries the militia (coordinator hunk)"
        );
    }

    /// The rank UI methods answer non-`None` for an enlisted character: a
    /// `KeyVal(currentRank, factionID)` for me and for another caller asking
    /// about me, and an overview listing the faction.
    #[test]
    fn the_rank_ui_methods_answer_for_an_enlisted_character() {
        let mut reg = world();
        enlist(&mut reg, ME, F, T0).expect("enlisted");
        let mine = call(&mut reg, ME, (FW, "GetMyCharacterRankInfo"), &[]);
        assert!(matches!(
            attr(&mine, "currentRank"),
            PyValue::Int(0..=MAX_RANK)
        ));
        assert_eq!(attr(&mine, "factionID"), i(F));
        let theirs = call(&mut reg, YOU, (FW, "GetCharacterRankInfo"), &[i(ME)]);
        assert_eq!(theirs, mine);
        let PyValue::List(o) = call(&mut reg, ME, (FW, "GetMyCharacterRankOverview"), &[]) else {
            panic!("a list")
        };
        assert_eq!(o.len(), 1);
        assert_eq!(attr(&o[0], "factionID"), i(F));
    }

    /// The rank follows the standing toward the militia corp, truncated and
    /// clamped; `CheckForRankChange` reports a change once, then `None`. A
    /// faction served before stays in the overview after leaving.
    #[test]
    fn the_rank_follows_the_militia_standing_and_changes_are_reported_once() {
        let mut reg = world();
        let check = (FW, "CheckForRankChange");
        enlist(&mut reg, ME, F, T0).expect("enlisted");
        assert_eq!(call(&mut reg, ME, check, &[]), PyValue::None);
        reg.set_npc_standing(ME, F_CORP, 3.7);
        assert_eq!(rank(&reg, ME, F), 3);
        assert_eq!(call(&mut reg, ME, check, &[]), i(3));
        assert_eq!(call(&mut reg, ME, check, &[]), PyValue::None);
        reg.set_npc_standing(ME, F_CORP, 10.0);
        assert_eq!(rank(&reg, ME, F), MAX_RANK);
        reg.set_npc_standing(ME, F_CORP, -4.0);
        assert_eq!(rank(&reg, ME, F), 0);
        assert_eq!(rank(&reg, ME, G), 0, "no militia corp in the content");

        leave(&mut reg, ME, T0).expect("left");
        let info = call(&mut reg, ME, (FW, "GetMyCharacterRankInfo"), &[]);
        assert_eq!(info, PyValue::None);
        let PyValue::List(o) = call(&mut reg, ME, (FW, "GetMyCharacterRankOverview"), &[]) else {
            panic!("a list")
        };
        assert_eq!(o.len(), 1, "served once");
    }

    /// FW-4 (#100): a pilot below the standing is refused; at zero, accepted.
    #[test]
    fn a_pilot_below_the_standing_is_refused() {
        let mut reg = world();
        reg.set_npc_standing(ME, F, MIN_STANDING_TO_JOIN - 0.5);
        assert!(request_enlist(&mut reg, ME, F, T0).is_err());
        assert_eq!(my_faction(&reg, ME, T0), None);
    }

    #[test]
    fn a_pilot_at_zero_is_accepted() {
        let mut reg = world();
        reg.set_npc_standing(ME, F, 0.0);
        request_enlist(&mut reg, ME, F, T0).expect("zero passes");
    }

    /// The cooldown counts from joining a militia corporation, not leaving.
    #[test]
    fn enlisting_within_a_day_of_joining_a_militia_is_refused() {
        let mut reg = world();
        let day = ENLISTMENT_COOLDOWN_SECS * TICKS;
        reg.deltas_mut().put(
            super::super::corp::HIST_KIND,
            vec![ME.to_string(), (T0 - 1).to_string()],
            vec![F_CORP.to_string()],
        );
        assert!(
            request_enlist(&mut reg, ME, F, T0).is_err(),
            "joined a moment ago"
        );
        request_enlist(&mut reg, ME, F, T0 + day).expect("a day on");
    }

    #[test]
    fn enlisting_away_from_a_militia_station_is_refused() {
        let mut reg = world();
        reg.character_mut(ME).expect("a pilot").station_id = None;
        assert!(request_enlist(&mut reg, ME, F, T0).is_err(), "in space");
    }

    #[test]
    fn a_foreign_faction_station_is_refused() {
        let mut reg = world();
        dock(&mut reg, ME, G_STN);
        assert!(
            request_enlist(&mut reg, ME, F, T0).is_err(),
            "G's station, F's militia"
        );
        request_enlist(&mut reg, ME, G, T0).expect("its own faction's station");
    }

    /// Lane clock: at scale N the enlistment cooldown is N times shorter.
    #[test]
    fn at_scale_n_the_enlistment_cooldown_is_n_times_shorter() {
        let mut reg = world();
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(6).unwrap());
        enlist(&mut reg, ME, F, T0).expect("enlisted");
        leave(&mut reg, ME, T0).expect("left");
        let end = T0 + ENLISTMENT_COOLDOWN_SECS * TICKS / 6;
        assert_eq!(cooldown_until(&reg, ME, end - 1), Some(end));
        assert_eq!(cooldown_until(&reg, ME, end), None);
    }

    /// A director joins as a corporation: `Joining`, then `Active` once the
    /// wait has passed, when every member serves through the corp. Leaving
    /// and withdrawing walk the statuses; a plain member is refused.
    #[test]
    fn a_corporation_joins_settles_and_leaves() {
        let mut reg = world();
        with_corp(&mut reg);
        let later = T0 + CORP_CHANGE_WAIT_SECS * TICKS;
        let gone = later + CORP_CHANGE_WAIT_SECS * TICKS;
        let op = |reg: &mut Registry, who: i64, name: &str, now: i64| {
            try_at(reg, who, (FW, name), &[i(F)], now)
        };
        let status = |reg: &mut Registry, now: i64| {
            let v = at(reg, YOU, (FW, "GetCorpFactionalWarStatus"), &[], now);
            attr(&v, "status")
        };
        assert!(op(&mut reg, YOU, "JoinFactionAsCorporation", T0).is_err());
        assert_eq!(
            status(&mut reg, T0),
            PyValue::None,
            "a player corp outside FW"
        );

        op(&mut reg, ME, "JoinFactionAsCorporation", T0).expect("join");
        assert_eq!(status(&mut reg, T0), i(STATUS_JOINING));
        assert_eq!(enlistment(&mut reg, YOU, T0), tuple3(None, None, None));
        op(&mut reg, ME, "WithdrawJoinFactionAsCorporation", T0).expect("withdraw");
        assert_eq!(status(&mut reg, T0), PyValue::None);

        op(&mut reg, ME, "JoinFactionAsCorporation", T0).expect("join");
        assert_eq!(status(&mut reg, later), i(STATUS_ACTIVE));
        assert_eq!(
            enlistment(&mut reg, YOU, later),
            tuple3(Some(F), None, Some(F))
        );
        let war = at(
            &mut reg,
            YOU,
            (FW, "GetCorporationWarFactionID"),
            &[i(CORP)],
            later,
        );
        assert_eq!(war, i(F));
        assert!(enlist(&mut reg, YOU, G, later).is_err(), "already serving");

        op(&mut reg, ME, "LeaveFactionAsCorporation", later).expect("leave");
        assert_eq!(status(&mut reg, later), i(STATUS_LEAVING));
        op(&mut reg, ME, "WithdrawLeaveFactionAsCorporation", later).expect("stay");
        assert_eq!(status(&mut reg, later), i(STATUS_ACTIVE));
        op(&mut reg, ME, "LeaveFactionAsCorporation", later).expect("leave");
        assert_eq!(status(&mut reg, gone), PyValue::None);
        assert_eq!(enlistment(&mut reg, YOU, gone), tuple3(None, None, None));
    }

    /// A director's allowed list is what the corp's members may enlist in.
    #[test]
    fn a_director_restricts_the_militias_members_may_enlist_in() {
        let mut reg = world();
        with_corp(&mut reg);
        let get = (EN, "GetCorpAllowedEnlistmentFactions");
        let set = (EN, "SetMyCorpAllowedEnlistmentFactions");
        let all = PyValue::List(vec![i(F), i(G)]);
        assert_eq!(call(&mut reg, YOU, get, &[i(CORP)]), all);
        let only_g = [PyValue::List(vec![i(G)])];
        assert!(
            try_at(&mut reg, YOU, set, &only_g, T0).is_err(),
            "not a director"
        );
        call(&mut reg, ME, set, &only_g);
        assert_eq!(call(&mut reg, YOU, get, &[i(CORP)]), only_g[0]);
        assert!(enlist(&mut reg, YOU, F, T0).is_err());
        dock(&mut reg, YOU, G_STN);
        enlist(&mut reg, YOU, G, T0).expect("allowed");
    }

    /// A donation moves the militia corp's LP into the system's pool, capped
    /// at `MAX_LP_POOL`, and tells the system.
    #[test]
    fn a_donation_fills_the_pool_and_tells_the_system() {
        let mut reg = world();
        enlist(&mut reg, ME, F, T0).expect("enlisted");
        crate::loyalty::adjust(&mut reg, ME, F_CORP, 1_000, "test").expect("credited");
        let _ = reg.take_pending();
        call(
            &mut reg,
            ME,
            (FW, "DonateLPsToSolarSystem"),
            &[i(400), i(300)],
        );
        assert_eq!(crate::loyalty::balance(&reg, ME, F_CORP), 600);
        assert_eq!(
            call(&mut reg, YOU, (FW, "GetSolarSystemLPs"), &[i(SYS)]),
            i(300)
        );
        let told = pushes(&mut reg, "OnSolarSystemLPChange");
        assert_eq!(told, vec![(vec![i(0), i(300)], Some(vec![ME, YOU]))]);
        assert!(
            donate(&mut reg, ME, SYS, 1, MAX_LP_POOL, T0).is_err(),
            "over the cap"
        );
        assert!(
            donate(&mut reg, ME, SYS, 601, 1, T0).is_err(),
            "more LP than held"
        );
        assert_eq!(lp_pool(&reg, SYS), 300);
    }

    /// Enlistment, ranks, corp state and the pool are deltas: a world
    /// rebuilt from the rows reads the same.
    #[test]
    fn enlistment_survives_a_restart() {
        let mut reg = world();
        with_corp(&mut reg);
        enlist(&mut reg, ME, F, T0).expect("enlisted");
        corp_op(&mut reg, CORP, G, CorpOp::Join, T0).expect("joining");
        let mut again = world();
        with_corp(&mut again);
        for kind in [KIND, RANK_KIND, CORP_KIND, ALLOW_KIND, LP_KIND] {
            for (k, v) in reg.deltas().of(kind).clone() {
                again.deltas_mut().put(kind, k, v);
            }
        }
        assert_eq!(
            enlistment(&mut again, ME, T0),
            tuple3(Some(F), Some(F), None)
        );
        assert_eq!(corp_state(&again, CORP, T0), Some((G, STATUS_JOINING, T0)));
    }

    /// The slot reads content only: the warzone militias in table order and
    /// each faction's militia corp.
    #[test]
    fn the_index_is_built_from_content() {
        let reg = world();
        let ix = reg.ext::<Index>();
        assert_eq!(ix.militias, vec![F, G]);
        assert_eq!(ix.militia_corp.get(&F), Some(&F_CORP));
        assert_eq!(ix.militia_corp.get(&G), None);
    }

    /// FW-1: the militia reaches every place the client reads it. An
    /// enlisted pilot's ship slim carries `warFactionID`, an unenlisted
    /// one none, and the public info, the station guests and chat's user
    /// data name the militia.
    #[test]
    fn an_enlisted_pilots_ship_slim_carries_war_faction_id() {
        use crate::combat::tests::{ALICE, ALICE_SHIP, BOB_SHIP, alice_has_bob_locked};
        let mut reg = alice_has_bob_locked();
        assert!(slim_extras(&reg, &[ALICE_SHIP]).is_empty(), "not enlisted");
        reg.deltas_mut().put(
            KIND,
            vec![ALICE.to_string()],
            vec![F.to_string(), T0.to_string(), "0".to_owned()],
        );
        assert_eq!(
            slim_extras(&reg, &[ALICE_SHIP, BOB_SHIP]),
            vec![(ALICE_SHIP, vec![("warFactionID", PyValue::Int(F))])]
        );
    }

    /// A call through the whole registry, not this module's dispatcher.
    fn whole(
        reg: &mut Registry,
        who: i64,
        (service, method): (&str, &str),
        args: &[PyValue],
    ) -> PyValue {
        reg.dispatch(&Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
        .expect("answered")
    }

    #[test]
    fn public_info_names_the_militia() {
        let mut reg = world();
        let ask = |reg: &mut Registry| {
            let got = whole(reg, YOU, ("charMgr", "GetPublicInfo"), &[i(ME)]);
            attr(&got, "warFactionID")
        };
        assert_eq!(ask(&mut reg), PyValue::None);
        enlist(&mut reg, ME, F, T0).expect("enlisted");
        assert_eq!(ask(&mut reg), i(F));
    }

    #[test]
    fn guests_carry_the_militia() {
        let mut reg = world();
        enlist(&mut reg, ME, F, T0).expect("enlisted");
        reg.character_mut(ME).expect("me").station_id = Some(60_000_101);
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 10_000_000_001,
            session_id: 10_000_000_001,
            user_id: 1,
            character_id: None,
        });
        reg.session_selected_character(10_000_000_001, ME);
        let got = whole(&mut reg, ME, ("station", "GetGuests"), &[]);
        let PyValue::List(rows) = got else {
            panic!("a list: {got:?}")
        };
        let PyValue::List(row) = &rows[0] else {
            panic!("a row: {rows:?}")
        };
        assert_eq!(row[3], i(F));
    }

    #[test]
    fn chat_user_data_names_the_militia() {
        let mut reg = world();
        assert_eq!(crate::chat::user_data(&reg, ME).warfactionid, 0);
        enlist(&mut reg, ME, F, T0).expect("enlisted");
        assert_eq!(crate::chat::user_data(&reg, ME).warfactionid, F);
    }

    fn session_pairs(reg: &mut Registry, ch: i64) -> Vec<Vec<(PyValue, PyValue)>> {
        reg.take_session_changes()
            .into_iter()
            .filter(|(c, _)| *c == ch)
            .map(|(_, kv)| kv)
            .collect()
    }

    /// FW-2: the window's Join moves an NPC-corp pilot into the militia
    /// corporation, and one session change carries both keys.
    #[test]
    fn enlisting_from_an_npc_corp_moves_the_pilot_into_the_militia_corp() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        at(&mut reg, ME, (FW, "JoinFactionAsCharacter"), &[i(F)], now);
        assert_eq!(reg.character(ME).expect("me").corporation_id, F_CORP);
        assert_eq!(direct_faction(&reg, ME), None, "no fwenlist row");
        assert_eq!(
            enlistment(&mut reg, ME, now),
            tuple3(Some(F), None, Some(F))
        );
        let changes = session_pairs(&mut reg, ME);
        assert_eq!(changes.len(), 1, "one change: {changes:?}");
        assert_eq!(
            crate::pyutil::pairs_get(&changes[0], b"corpid"),
            Some(&PyValue::Tuple(vec![PyValue::None, i(F_CORP)]))
        );
        assert_eq!(
            crate::pyutil::pairs_get(&changes[0], b"warfactionid"),
            Some(&PyValue::Tuple(vec![PyValue::None, i(F)]))
        );
        assert_eq!(pushes(&mut reg, "OnEnlistmentStateUpdated").len(), 1);
    }

    #[test]
    fn retiring_from_the_militia_corp_returns_to_the_npc_corp() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        at(&mut reg, ME, (FW, "JoinFactionAsCharacter"), &[i(F)], now);
        session_pairs(&mut reg, ME);
        whole(&mut reg, ME, ("corpRegistry", "KickOutMember"), &[i(ME)]);
        assert_eq!(
            reg.character(ME).expect("me").corporation_id,
            crate::model::NPC_CORPORATION_ID
        );
        assert_eq!(my_faction(&reg, ME, now), None);
        let changes = session_pairs(&mut reg, ME);
        assert_eq!(changes.len(), 1);
        assert_eq!(
            crate::pyutil::pairs_get(&changes[0], b"warfactionid"),
            Some(&PyValue::Tuple(vec![i(F), PyValue::None]))
        );
        let again = try_at(&mut reg, ME, (FW, "JoinFactionAsCharacter"), &[i(F)], now);
        assert!(again.is_err(), "the cooldown holds: {again:?}");
        assert!(cooldown_until(&reg, ME, now).is_some());
    }

    #[test]
    fn the_militia_corp_pilot_is_can_be_kicked_out_true() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        let ask = |reg: &mut Registry, who: i64, target: i64| {
            whole(reg, who, ("corpRegistry", "CanBeKickedOut"), &[i(target)])
        };
        assert_eq!(
            ask(&mut reg, ME, ME),
            PyValue::Bool(false),
            "a plain NPC pilot"
        );
        at(&mut reg, ME, (FW, "JoinFactionAsCharacter"), &[i(F)], now);
        assert_eq!(ask(&mut reg, ME, ME), PyValue::Bool(true));
        assert_eq!(ask(&mut reg, YOU, ME), PyValue::Bool(false), "only oneself");
    }

    #[test]
    fn a_player_corp_pilot_enlists_directly_and_keeps_his_corp() {
        let mut reg = world();
        with_corp(&mut reg);
        reg.character_mut(YOU).expect("you").corporation_id = CORP;
        let now = crate::pyutil::filetime_now();
        at(&mut reg, YOU, (FW, "JoinFactionAsCharacter"), &[i(F)], now);
        assert_eq!(direct_faction(&reg, YOU), Some(F));
        assert_eq!(reg.character(YOU).expect("you").corporation_id, CORP);
    }

    #[test]
    fn the_move_survives_a_reload() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        at(&mut reg, ME, (FW, "JoinFactionAsCharacter"), &[i(F)], now);
        super::super::corp::sync_characters(&mut reg);
        assert_eq!(reg.character(ME).expect("me").corporation_id, F_CORP);
        assert_eq!(war_faction(&reg, ME), Some(F));
    }

    #[test]
    fn the_militia_corp_pilot_counts_as_a_militia_member() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        at(&mut reg, ME, (FW, "JoinFactionAsCharacter"), &[i(F)], now);
        assert_eq!(members_of(&reg, F, now), (1, 1));
    }
}
