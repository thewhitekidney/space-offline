//! **Escalations** (Y-ESCAL): a completed combat site opens the next, harder
//! one -- in its own system or a near one -- tells the pilots who finished it,
//! and keeps it on their journal's Expeditions list until it is run or runs out.
//!
//! Lane note: `docs/lanes/y-escal.md`. [`crate::combat_site::on_complete`]
//! rolls the chance and the dungeon (the faction's ladder: archetype 31,
//! same faction, one difficulty up, floor 3) and queues an
//! [`combat_site::Escalation`]; [`tick`] drains the queue.
//!
//! # What the client does with one **[V]**
//!
//! - `journal.GetEscalations` calls `dungeonExplorationMgr.
//!   GetMyEscalatingPathDetails()` and keeps an **old-style `Rowset`**
//!   (`.header` and `.lines`) of the caller's own escalations
//!   (`journal.py:345`). Each row is read by attribute: `destDungeon` (its
//!   `dungeonNameID`, `difficulty`, `factionID`), `dungeon` (`instanceID`,
//!   `solarSystemID`, `destDungeonID`), `expiryTime` (a wall FILETIME, an
//!   expired row is dropped by the client) and `journalEntryTemplateID`
//!   (`contentProviderEscalations.py:61-69`,
//!   `escalation_sites/provider.py:84-87`, `escalationContentPiece.py:58`). A row without a `destDungeon` is skipped.
//! - `OnEscalatingPathMessage(instanceID)` is the notification. The journal
//!   finds the row (refetching when it is not in its cache), builds
//!   `data.rec = row` and opens the **Expeditions transmission**
//!   (`journal.py:273-282`, `PopupDungeonDescription`, caption `UI/Common/Expeditions`):
//!   header `localization.GetByMessageID(destDungeon.dungeonNameID)`, body
//!   `GetByMessageID(journalEntryTemplateID)`, and -- from `row.instanceID` and
//!   `row.solarSystemID` -- a Warp To button (same system) or Set Destination.
//!   The agency blinks (`contentProviderEscalations.OnEscalatingPathMessage`).
//! - `OnEscalatingPathChange` makes the journal refetch (`journal.OnEscalatingPathChange`).
//! - Warp To is `CmdWarpToStuff('epinstance', instanceID)`
//!   (`transmissionMgr.py:157`, `escalationContentPiece.
//!   _ExecuteWarpTo`, `EscalationSiteJob._warp_to_location`): [`warp_target`],
//!   a row in `warp_subject::RESOLVERS`.
//!
//! # What the data does not say
//!
//! The bundle carries no link from a dungeon to its escalation, no chance,
//! no time limit and no journal text of an escalation. Every number below is
//! **[A]** with its falsifier at the constant; the pairing is
//! `combat_site::escalation_candidates`.
//!
//! # State
//!
//! One `escal` delta row per live escalation ([`KIND`]), keyed by its site id:
//! `system dungeon from_dungeon journal created expires step chars`, and a
//! `next` row for the counter. A row goes when the site is completed
//! ([`completed`]) or runs out ([`tick`]). The site is a population-band id
//! ([`site_id_of`]) in slots below the wormholes', so the site path treats it
//! as a site of its own ([`is_escalation_site`]); its instance is made when a
//! pilot reaches it, as for any site.

use crate::combat_site;
use crate::damage::BlueTime;
use crate::dungeon_data::{Dungeon, Entry};
use crate::population::{self, GENERATIONS, SLOTS_PER_SYSTEM, Site};
use crate::pyutil::{instance, keyval};
use crate::seed::{Kind, stream};
use crate::service::Registry;
use crate::site::{self, Sites};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// The delta kind (`delta_kinds::KINDS`, `("escal", "escalation")`).
pub const KIND: &str = "escal";
/// The counter's key.
const NEXT: &str = "next";

/// `journal.py`'s `OnEscalatingPathMessage`, `OnEscalatingPathChange` **[V]**.
pub const EVENT_MESSAGE: &str = "OnEscalatingPathMessage";
pub const EVENT_CHANGE: &str = "OnEscalatingPathChange";
/// `CmdWarpToStuff`'s subject for an escalation's instance
/// (`warpSubjects.EPIC_INSTANCE`) **[V]**.
pub const WARP_SUBJECT: &[u8] = b"epinstance";

/// The row's columns. The first six are the ones the client reads (the
/// order is `EvEJS`'s empty header, a fact **[I]**); `solarSystemID` is read
/// off the row by the transmission's Warp To (`transmissionMgr.py:125-128`) **[V]**
/// and `pathStep` decides its Agency link (`:104`) **[V]**.
pub const COLUMNS: [&str; 8] = [
    "instanceID",
    "dungeon",
    "destDungeon",
    "srcDungeon",
    "expiryTime",
    "journalEntryTemplateID",
    "solarSystemID",
    "pathStep",
];

const BLUE_PER_SECOND: BlueTime = 10_000_000;

/// How long an escalation stays open, seconds. **[A]**: the data and the
/// client carry no limit, only the `expiryTime` column. A day is the span a
/// DED escalation is remembered to last; falsified by an owner who finds
/// them lingering (too long) or expiring under a run (too short). Scaled by
/// `--time-scale` ([`crate::game_clock`]).
pub const TTL_SECONDS: i64 = 24 * 3600;

/// How far, in jumps, an escalation may land from the site that made it,
/// the origin included. **[A]**: nothing says where an escalation goes;
/// two jumps keeps it a short trip. Falsified by "I never see it" (raise)
/// or "it sends me across the region" (lower).
pub const MAX_JUMPS: u32 = 2;

/// Slots of a system's 64 the escalations take, just under the wormholes'
/// (`wormhole::static_slot`). **[A]** (as the wormholes' own): population
/// never fills them. Falsified by a site id two sites share.
pub const SLOTS: usize = 4;
const FIRST_SLOT: usize = SLOTS_PER_SYSTEM - 2 * crate::wormhole::MAX_STATICS as usize - SLOTS;

/// The stream's tag, after `combat_site::STREAM_TAG`'s family.
const TAG: u64 = 0x45_5343;

/// One live escalation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Esc {
    pub site_id: i64,
    pub system: i64,
    /// The archetype-31 dungeon.
    pub dungeon: i64,
    /// The site that opened it.
    pub from_dungeon: i64,
    /// The localization message of the transmission's body.
    pub journal: i64,
    pub created: BlueTime,
    pub expires: BlueTime,
    /// 1 for the first rung of the ladder.
    pub step: i64,
    /// The pilots who finished the site: the ones it is shown to.
    pub chars: Vec<i64>,
}

impl Esc {
    fn key(&self) -> Vec<String> {
        vec![self.site_id.to_string()]
    }

    fn value(&self) -> Vec<String> {
        let chars: Vec<String> = self.chars.iter().map(|c| c.to_string()).collect();
        vec![
            self.system.to_string(),
            self.dungeon.to_string(),
            self.from_dungeon.to_string(),
            self.journal.to_string(),
            self.created.to_string(),
            self.expires.to_string(),
            self.step.to_string(),
            chars.join(","),
        ]
    }

    fn parse(key: &[String], v: &[String]) -> Option<Esc> {
        let [k] = key else { return None };
        let [
            system,
            dungeon,
            from,
            journal,
            created,
            expires,
            step,
            chars,
        ] = v
        else {
            return None;
        };
        let chars = chars
            .split(',')
            .filter(|c| !c.is_empty())
            .map(|c| c.parse().ok())
            .collect::<Option<Vec<i64>>>()?;
        Some(Esc {
            site_id: k.parse().ok()?,
            system: system.parse().ok()?,
            dungeon: dungeon.parse().ok()?,
            from_dungeon: from.parse().ok()?,
            journal: journal.parse().ok()?,
            created: created.parse().ok()?,
            expires: expires.parse().ok()?,
            step: step.parse().ok()?,
            chars,
        })
    }

    /// Whether `now` is past the limit.
    pub fn expired(&self, now: BlueTime) -> bool {
        now >= self.expires
    }
}

// ---------------------------------------------------------------------------
// Pure rules
// ---------------------------------------------------------------------------

/// The site id of the `n`th escalation in a system of rank `rank`: one of
/// [`SLOTS`] slots, the generation counting on. `None` past the band.
pub fn site_id_of(rank: usize, n: u64) -> Option<i64> {
    let slot = FIRST_SLOT + (n % SLOTS as u64) as usize;
    let generation = ((n / SLOTS as u64) % u64::from(GENERATIONS)) as u32;
    population::site_id(rank, slot, generation)
}

/// Whether a site id is an escalation's ([`SLOTS`]). `site.rs` leaves these
/// instances alone, as it does a wormhole's.
pub fn is_escalation_site(site_id: i64) -> bool {
    if !(population::SITE_BASE..population::SITE_END).contains(&site_id) {
        return false;
    }
    let cell = (site_id - population::SITE_BASE) / i64::from(GENERATIONS);
    let slot = (cell % SLOTS_PER_SYSTEM as i64) as usize;
    (FIRST_SLOT..FIRST_SLOT + SLOTS).contains(&slot)
}

/// The systems within `jumps` gates of `origin`, the origin first, then in
/// id order. `hops` is a system's gate neighbours.
pub fn within(origin: i64, jumps: u32, hops: &dyn Fn(i64) -> Vec<i64>) -> Vec<i64> {
    let mut seen = BTreeSet::from([origin]);
    let mut edge = vec![origin];
    for _ in 0..jumps {
        let mut next = Vec::new();
        for s in &edge {
            for n in hops(*s) {
                if seen.insert(n) {
                    next.push(n);
                }
            }
        }
        edge = next;
    }
    seen.remove(&origin);
    std::iter::once(origin).chain(seen).collect()
}

/// Where the escalation of `site_id` lands: a seeded pick among the systems
/// within [`MAX_JUMPS`] that `ok` admits (they must hold a site slot).
pub fn pick_system(
    seed: u64,
    from_site: i64,
    origin: i64,
    hops: &dyn Fn(i64) -> Vec<i64>,
    ok: &dyn Fn(i64) -> bool,
) -> Option<i64> {
    let near: Vec<i64> = within(origin, MAX_JUMPS, hops)
        .into_iter()
        .filter(|s| ok(*s))
        .collect();
    if near.is_empty() {
        return None;
    }
    let mut s = stream(seed, Kind::Site, &[TAG, from_site as u64, origin as u64]);
    Some(near[s.range(0, near.len() as u64) as usize])
}

/// The message the transmission's body shows. The data carries no journal
/// text for an escalation, so it is the **first of these that exists [A]**:
/// the destination's entry-room text, any room's text, its own name, the
/// source's name. Falsified by a body that reads wrongly for the site.
pub fn journal_text(dest: &Dungeon, src: &Dungeon) -> Option<i64> {
    let entry = site::entry_room(dest);
    let room_text = |r: &&crate::dungeon_data::Room| r.message_text_id.filter(|t| *t > 0);
    dest.rooms
        .iter()
        .filter(|r| Some(r.id) == entry)
        .find_map(|r| room_text(&r))
        .or_else(|| dest.rooms.iter().find_map(|r| room_text(&r)))
        .or(dest.name_id)
        .or(src.name_id)
}

fn blue(seconds: i64) -> BlueTime {
    seconds * BLUE_PER_SECOND
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

fn opt(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

fn dungeon_row(d: Option<&Dungeon>, id: i64) -> PyValue {
    keyval(vec![
        ("dungeonID", PyValue::Int(id)),
        ("dungeonNameID", opt(d.and_then(|d| d.name_id))),
        ("difficulty", opt(d.and_then(|d| d.difficulty))),
        ("factionID", opt(d.and_then(|d| d.faction))),
        ("archetypeID", opt(d.map(|d| d.archetype))),
    ])
}

/// One row's `lines` entry, in [`COLUMNS`] order.
pub fn line(reg: &Registry, e: &Esc) -> Vec<PyValue> {
    let table = reg.content().try_get::<crate::dungeon_data::Table>();
    let get = |id: i64| table.and_then(|t| t.dungeons.get(&id));
    vec![
        PyValue::Int(e.site_id),
        keyval(vec![
            ("instanceID", PyValue::Int(e.site_id)),
            ("solarSystemID", PyValue::Int(e.system)),
            ("destDungeonID", PyValue::Int(e.dungeon)),
            ("dungeonID", PyValue::Int(e.dungeon)),
        ]),
        dungeon_row(get(e.dungeon), e.dungeon),
        dungeon_row(get(e.from_dungeon), e.from_dungeon),
        PyValue::Int(e.expires),
        PyValue::Int(e.journal),
        PyValue::Int(e.system),
        PyValue::Int(e.step),
    ]
}

/// **`GetMyEscalatingPathDetails`** for `actor`: an old-style `Rowset`
/// instance (`header`, `lines`), with the escalations shown to the caller
/// that have not run out **[V shape]**.
pub fn rowset(reg: &Registry, actor: Option<i64>) -> PyValue {
    let now = crate::pyutil::filetime_now();
    let lines: Vec<PyValue> = actor
        .map(|a| {
            all(reg)
                .into_iter()
                .filter(|e| !e.expired(now) && e.chars.contains(&a))
                .map(|e| PyValue::List(line(reg, &e)))
                .collect()
        })
        .unwrap_or_default();
    instance(
        "eve.common.script.sys.rowset.Rowset",
        vec![
            (
                "header",
                PyValue::List(COLUMNS.iter().map(|c| crate::pyutil::bstr(c)).collect()),
            ),
            ("lines", PyValue::List(lines)),
        ],
    )
}

/// `OnEscalatingPathMessage(instanceID)` to one pilot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMessage {
    pub instance_id: i64,
    pub to: i64,
}

impl crate::notify::Push for PathMessage {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (EVENT_MESSAGE, vec![PyValue::Int(self.instance_id)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnEscalatingPathChange()` to one pilot: refetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathChange {
    pub to: i64,
}

impl crate::notify::Push for PathChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (EVENT_CHANGE, vec![])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---------------------------------------------------------------------------
// The Registry side
// ---------------------------------------------------------------------------

/// Every live escalation, by site id.
pub fn all(reg: &Registry) -> Vec<Esc> {
    reg.deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| Esc::parse(k, v))
        .collect()
}

/// One escalation by its site id.
pub fn get(reg: &Registry, site_id: i64) -> Option<Esc> {
    let key = vec![site_id.to_string()];
    reg.deltas()
        .of(KIND)
        .get(&key)
        .and_then(|v| Esc::parse(&key, v))
}

fn next(reg: &mut Registry) -> u64 {
    let key = vec![NEXT.to_owned()];
    let n = reg
        .deltas()
        .of(KIND)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0u64);
    reg.deltas_mut().put(KIND, key, vec![(n + 1).to_string()]);
    n
}

fn hops_of(reg: &Registry, system: i64) -> Vec<i64> {
    let cat = reg.celestials();
    let mut out: Vec<i64> = cat
        .served_in(system)
        .filter(|c| c.kind == crate::celestial::Kind::Gate)
        .filter(|c| cat.system_of(c.id) == Some(system))
        .filter_map(|c| cat.jump_of(c.id))
        .map(|(_, to)| to)
        .filter(|to| *to != system)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn rank_of(reg: &Registry, system: i64) -> Option<usize> {
    reg.ext::<population::Population>()
        .prepared()
        .sys_rank(system)
}

/// The pilots who finished `from`'s site: the characters on its grid.
fn finishers(reg: &Registry, e: &combat_site::Escalation) -> Vec<i64> {
    let Some(anchor) = reg.ext::<Sites>().get(e.from_site).map(|i| i.anchor) else {
        return Vec::new();
    };
    crate::dungeon::audience(reg, e.system)
        .into_iter()
        .filter(|c| {
            reg.roster().get(*c).is_some_and(|p| {
                crate::space_state::distance(p.motion.position, anchor) <= site::ARRIVAL_RANGE
            })
        })
        .collect()
}

/// **Open the escalation `e` queued**: the pilots who finished its site get
/// it. The site id, or `None` when nobody is on that grid any more.
pub fn spawn(reg: &mut Registry, e: &combat_site::Escalation, ft: BlueTime) -> Option<i64> {
    let chars = finishers(reg, e);
    if chars.is_empty() {
        return None;
    }
    open(reg, e, chars, ft)
}

/// **Make the escalation** `e` names for `chars`: pick its system, write its
/// row, tell the pilots. `None` when no system near the origin holds it, an
/// id cannot be found or the data has no text for it.
pub fn open(
    reg: &mut Registry,
    e: &combat_site::Escalation,
    chars: Vec<i64>,
    ft: BlueTime,
) -> Option<i64> {
    let (dest, src) = {
        let t = reg.content().try_get::<crate::dungeon_data::Table>()?;
        (
            t.dungeons.get(&e.dungeon_id)?.clone(),
            t.dungeons.get(&e.from_dungeon)?.clone(),
        )
    };
    let journal = journal_text(&dest, &src)?;
    let seed = reg.world().seed;
    let system = {
        let r: &Registry = reg;
        pick_system(seed, e.from_site, e.system, &|s| hops_of(r, s), &|s| {
            rank_of(r, s).is_some()
        })?
    };
    let rank = rank_of(reg, system)?;
    // A free id: not a live row, nor a live instance.
    let site_id = loop {
        let n = next(reg);
        let id = site_id_of(rank, n)?;
        if get(reg, id).is_none() && reg.ext::<Sites>().get(id).is_none() {
            break id;
        }
        if n > u64::from(GENERATIONS) * SLOTS as u64 {
            return None;
        }
    };
    let esc = Esc {
        site_id,
        system,
        dungeon: e.dungeon_id,
        from_dungeon: e.from_dungeon,
        journal,
        created: ft,
        expires: ft + crate::game_clock::span(reg, blue(TTL_SECONDS)),
        step: rung(&dest),
        chars: chars.clone(),
    };
    reg.deltas_mut().put(KIND, esc.key(), esc.value());
    for c in chars {
        reg.schedule(
            std::time::Duration::ZERO,
            PathMessage {
                instance_id: site_id,
                to: c,
            },
        );
    }
    crate::outln!(
        "[escalation] site {} escalates to dungeon {} as site {site_id} in {system}",
        e.from_site,
        e.dungeon_id
    );
    Some(site_id)
}

/// A dungeon's rung on the ladder: 1 at the floor difficulty
/// ([`combat_site::ESCALATION_MIN_DIFFICULTY`]).
pub fn rung(d: &Dungeon) -> i64 {
    (d.difficulty
        .unwrap_or(combat_site::ESCALATION_MIN_DIFFICULTY)
        - combat_site::ESCALATION_MIN_DIFFICULTY
        + 1)
    .max(1)
}

/// `/escalate [dungeonID]` (a `slash::COMMANDS` row, GM only): open an
/// escalation for the caller in the caller's own system, as if a combat
/// site had been cleared -- the dungeon given, else the lowest archetype-31
/// id. For a test round and for the client scenario; nothing is asked of the
/// world's chances.
pub(crate) fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, crate::service::ServiceError> {
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    let Some(who) = actor.filter(|a| reg.character(*a).is_some()) else {
        return say("no such character");
    };
    let Some(system) = where_is(reg, who) else {
        return say("you are in no system");
    };
    let dungeon = match args {
        [] => reg
            .content()
            .try_get::<crate::dungeon_data::Table>()
            .and_then(|t| {
                t.dungeons
                    .values()
                    .find(|d| d.archetype == combat_site::ESCALATION_ARCHETYPE)
                    .map(|d| d.id)
            }),
        [id] => id.parse().ok(),
        _ => return say("usage: /escalate [dungeonID]"),
    };
    let Some(dungeon) = dungeon else {
        return say("no escalation dungeon to open");
    };
    let e = combat_site::Escalation {
        from_site: 0,
        from_dungeon: dungeon,
        system,
        dungeon_id: dungeon,
        when: crate::pyutil::filetime_now(),
    };
    match open(reg, &e, vec![who], e.when) {
        Some(id) => say(&format!("escalation {id}: dungeon {dungeon}")),
        None => {
            say("it could not be opened (an unknown dungeon, no text for it, or no system near)")
        }
    }
}

/// Where a character is (`Registry::session_system`).
fn where_is(reg: &Registry, who: i64) -> Option<i64> {
    reg.session_system(who)
}

fn forget(reg: &mut Registry, e: &Esc) {
    reg.deltas_mut().del(KIND, &e.key());
    for c in &e.chars {
        if reg.character(*c).is_some() {
            reg.schedule(std::time::Duration::ZERO, PathChange { to: *c });
        }
    }
}

/// **The completion hook** (`combat_site::on_complete`): the escalation
/// site `site_id` was cleared. Its row goes and the pilots' journals
/// refetch; the instance stays for the pocket's own despawn.
pub fn completed(reg: &mut Registry, site_id: i64) -> bool {
    match get(reg, site_id) {
        Some(e) => {
            forget(reg, &e);
            true
        }
        None => false,
    }
}

/// `(system, dungeon)` of an escalation's live instance, for a test that
/// reads the world from outside the crate.
pub fn placed(reg: &Registry, site_id: i64) -> Option<(i64, i64)> {
    reg.ext::<Sites>()
        .get(site_id)
        .map(|i| (i.system, i.dungeon_id))
}

/// Set the world's seed. **Only for a test that must know what the chance
/// will say**: a world's seed is drawn once at its birth and never changes.
#[doc(hidden)]
pub fn set_seed_for_test(reg: &mut Registry, seed: u64) {
    let world = crate::delta::World {
        seed,
        ..*reg.world()
    };
    reg.deltas_mut().set_world(world);
}

/// The site as the site path takes it. The anchor is a function of the
/// seed and the id ([`population::anchor`]).
pub fn as_site(reg: &Registry, e: &Esc) -> Site {
    let seed = reg.world().seed;
    let mut r = stream(seed, Kind::Site, &[TAG, e.site_id as u64]);
    let anchor = population::anchor(&mut r, reg.celestials(), e.system);
    let target_id = population::target_id(&mut r);
    Site {
        system: e.system,
        row: 0,
        k: 0,
        slot: 0,
        generation: 0,
        site_id: e.site_id,
        dungeon_id: e.dungeon,
        archetype: combat_site::ESCALATION_ARCHETYPE,
        entry: Entry::Anomaly,
        anchor,
        target_id,
        state: population::State::Live,
        at: e.created,
    }
}

/// **The `epinstance` warp row** (`warp_subject::RESOLVERS`): the pilot's
/// own escalation, in the pilot's own system, to the site's anchor. Nothing
/// for another pilot's escalation, an expired one or one in another system.
pub fn warp_target(
    w: &Registry,
    actor: i64,
    id: i64,
    _raw: Option<&PyValue>,
) -> Option<crate::model::Target> {
    let e = get(w, id)?;
    if !e.chars.contains(&actor) || e.expired(crate::pyutil::filetime_now()) {
        return None;
    }
    if where_is(w, actor) != Some(e.system) {
        return None;
    }
    Some(crate::model::Target {
        position: as_site(w, &e).anchor,
        radius: 0.0,
        warp_in: None,
    })
}

/// **The stage** (before `site`): open what the combat sites queued, then
/// for every escalation: past its limit, it goes (and its instance with it);
/// in a hot system with a pilot at its anchor, it is placed.
pub fn tick(reg: &mut Registry, _now: Instant, ft: BlueTime) {
    for e in combat_site::take_escalations(reg) {
        spawn(reg, &e, ft);
    }
    let rows = all(reg);
    if rows.is_empty() {
        return;
    }
    let hot: BTreeSet<i64> = reg.ext::<crate::liveness::Liveness>().hot().collect();
    let mut pilots: BTreeMap<i64, Vec<[f64; 3]>> = BTreeMap::new();
    for e in rows {
        if e.expired(ft) {
            site::vacate(reg, e.site_id);
            forget(reg, &e);
            crate::outln!("[escalation] site {} ran out", e.site_id);
            continue;
        }
        if !hot.contains(&e.system) || reg.ext::<Sites>().get(e.site_id).is_some() {
            continue;
        }
        let there = pilots.entry(e.system).or_insert_with(|| {
            reg.advance_system(e.system);
            reg.roster()
                .in_system(e.system)
                .into_iter()
                .filter(|p| reg.character(p.character_id).is_some())
                .map(|p| p.motion.position)
                .collect()
        });
        let s = as_site(reg, &e);
        if !site::arrivals(std::slice::from_ref(&s), &BTreeSet::new(), there).is_empty() {
            site::instantiate(reg, &s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::celestial::{Catalogue, Universe};
    use crate::content::Content;
    use crate::dungeon_data::{Room, Table};
    use crate::model::{Character, SandboxConfig};
    use crate::population::{Population, Prepared};
    use crate::space_state::{Motion, Presence};
    use std::sync::Arc;

    const PILOT: i64 = 1_000_001;
    const OTHER: i64 = 1_000_002;
    const HIGH: i64 = 39_000_001;
    const FACTION: i64 = 599_041;
    const FROM: i64 = 999_411;
    const TO: i64 = 999_412;
    const TEXT: i64 = 999_499;

    fn dungeons() -> Table {
        let d = |id, archetype, difficulty, rooms: Vec<Room>| Dungeon {
            id,
            archetype,
            faction: Some(FACTION),
            difficulty: Some(difficulty),
            entry_type: Some(Entry::Anomaly.type_id()),
            name_id: Some(id + 1),
            rooms,
            ..Dungeon::default()
        };
        let room = Room {
            id: 7,
            message_text_id: Some(TEXT),
            ..Room::default()
        };
        let list = [
            d(FROM, combat_site::COMBAT_ARCHETYPE, 3, vec![]),
            d(TO, combat_site::ESCALATION_ARCHETYPE, 3, vec![room]),
        ];
        Table {
            dungeons: list.into_iter().map(|d| (d.id, d)).collect(),
            ..Table::default()
        }
    }

    fn bodies() -> Catalogue {
        let mut text = String::new();
        for (n, s) in Universe::fixture().systems().enumerate() {
            let x = 1.0e11 * (n as f64 + 1.0);
            text += &format!(
                "planet\t{}\t990011\t6000000.0\t{x}\t0.0\t0.0\t{}\n",
                40_000_000 + n,
                s.id
            );
        }
        Catalogue::parse(&text)
    }

    /// A world with one pilot in [`HIGH`] standing at the origin of a live
    /// instance of [`FROM`], so the pilot is one of its finishers.
    fn world() -> Registry {
        let mut c = Content::fixture();
        c.insert("universe", Universe::fixture());
        c.insert("dungeons", dungeons());
        let mut reg = Registry::new(SandboxConfig {
            celestials: bodies(),
            characters: [PILOT, OTHER]
                .into_iter()
                .map(|id| Character {
                    character_id: id,
                    ship_id: 100_000_000 + id,
                    solar_system_id: Some(HIGH),
                    ..Character::default()
                })
                .collect(),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        let prep = Prepared::new(
            &population::Table::fixture(),
            &Universe::fixture(),
            &dungeons(),
            &|_| Some(FACTION),
        );
        *reg.ext_mut::<Population>() = Population::new(prep);
        reg
    }

    fn standing(reg: &mut Registry, who: i64, at: [f64; 3]) {
        let p = Presence::arriving(who, 100_000_000 + who, HIGH, Motion::arrived(at, [0.0; 3]));
        reg.roster_mut().arrive(p);
    }

    fn queued() -> combat_site::Escalation {
        combat_site::Escalation {
            from_site: 1,
            from_dungeon: FROM,
            system: HIGH,
            dungeon_id: TO,
            when: 0,
        }
    }

    #[test]
    fn a_site_id_is_in_the_escalation_slots_and_is_not_a_wormholes() {
        for n in 0..(SLOTS as u64 * 3) {
            let id = site_id_of(3, n).unwrap();
            assert!(is_escalation_site(id), "{id}");
            assert!(!crate::wormhole::is_wormhole_site(id), "{id}");
        }
        let a = site_id_of(3, 0).unwrap();
        assert_ne!(a, site_id_of(3, 1).unwrap());
        assert_ne!(a, site_id_of(3, SLOTS as u64).unwrap(), "the generation");
        assert_ne!(a, site_id_of(4, 0).unwrap(), "the rank");
        let pop = population::site_id(3, 0, 0).unwrap();
        assert!(!is_escalation_site(pop));
        assert!(!is_escalation_site(population::SITE_END));
    }

    #[test]
    fn near_systems_are_the_origin_then_the_gate_neighbours_to_the_limit() {
        // 1 - 2 - 3 - 4, and 2 - 5.
        let hops = |s: i64| match s {
            1 => vec![2],
            2 => vec![1, 3, 5],
            3 => vec![2, 4],
            4 => vec![3],
            _ => vec![2],
        };
        assert_eq!(within(1, 0, &hops), vec![1]);
        assert_eq!(within(1, 1, &hops), vec![1, 2]);
        assert_eq!(within(2, 1, &hops), vec![2, 1, 3, 5]);
        assert_eq!(within(1, 2, &hops), vec![1, 2, 3, 5]);
        assert_eq!(within(1, 9, &hops), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn the_pick_is_stable_near_and_only_among_the_admitted() {
        let hops = |s: i64| match s {
            1 => vec![2],
            2 => vec![1, 3],
            3 => vec![2, 4],
            _ => vec![3],
        };
        let all = |_: i64| true;
        let near = within(1, MAX_JUMPS, &hops);
        let mut seen = BTreeSet::new();
        for site in 0..200 {
            let a = pick_system(7, site, 1, &hops, &all).unwrap();
            assert_eq!(Some(a), pick_system(7, site, 1, &hops, &all));
            assert!(near.contains(&a), "{a}");
            seen.insert(a);
        }
        assert_eq!(seen.len(), near.len(), "every near system comes up");
        let only3 = |s: i64| s == 3;
        assert!((0..50).all(|s| pick_system(7, s, 1, &hops, &only3) == Some(3)));
        assert_eq!(pick_system(7, 1, 1, &hops, &|_| false), None);
    }

    #[test]
    fn the_journal_text_is_the_entry_rooms_else_a_name() {
        let t = dungeons();
        assert_eq!(
            journal_text(&t.dungeons[&TO], &t.dungeons[&FROM]),
            Some(TEXT)
        );
        let mut bare = t.dungeons[&TO].clone();
        bare.rooms.clear();
        assert_eq!(journal_text(&bare, &t.dungeons[&FROM]), bare.name_id);
        bare.name_id = None;
        assert_eq!(
            journal_text(&bare, &t.dungeons[&FROM]),
            t.dungeons[&FROM].name_id
        );
        let mut none = t.dungeons[&FROM].clone();
        none.name_id = None;
        assert_eq!(journal_text(&bare, &none), None);
    }

    #[test]
    fn a_row_round_trips_and_a_bad_one_is_refused() {
        let e = Esc {
            site_id: 8_100_000_123,
            system: HIGH,
            dungeon: TO,
            from_dungeon: FROM,
            journal: TEXT,
            created: 5,
            expires: 9,
            step: 2,
            chars: vec![PILOT, OTHER],
        };
        assert_eq!(Esc::parse(&e.key(), &e.value()), Some(e.clone()));
        let none = Esc {
            chars: vec![],
            ..e.clone()
        };
        assert_eq!(Esc::parse(&none.key(), &none.value()), Some(none));
        assert_eq!(Esc::parse(&e.key(), &e.value()[1..]), None);
        let mut bad = e.value();
        bad[7] = "x".into();
        assert_eq!(Esc::parse(&e.key(), &bad), None);
        assert!(!e.expired(8) && e.expired(9));
    }

    #[test]
    fn nobody_on_the_grid_means_nothing_is_opened() {
        let mut reg = world();
        let now = reg.world().birth;
        assert_eq!(spawn(&mut reg, &queued(), now), None);
        assert!(all(&reg).is_empty());
    }

    #[test]
    fn a_pilot_who_finished_a_site_gets_the_escalation_and_its_row() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        standing(&mut reg, PILOT, [0.0; 3]);
        // A pilot far from the site is not told.
        standing(&mut reg, OTHER, [0.0, 0.0, 10.0 * site::ARRIVAL_RANGE]);
        // The site the queue names must have an instance to read its anchor
        // from; a fixture instance at the origin does.
        let from = Site {
            system: HIGH,
            site_id: queued().from_site,
            dungeon_id: FROM,
            anchor: [0.0; 3],
            ..site_fixture()
        };
        assert!(site::instantiate(&mut reg, &from));
        let id = spawn(&mut reg, &queued(), now).expect("spawned");
        let e = get(&reg, id).unwrap();
        assert_eq!((e.dungeon, e.from_dungeon, e.system), (TO, FROM, HIGH));
        assert_eq!(e.chars, vec![PILOT]);
        assert_eq!(e.journal, TEXT);
        assert_eq!(e.step, 1);
        assert!(e.expires > now);
        assert!(is_escalation_site(id));
        let rows = rowset(&reg, Some(PILOT));
        assert_eq!(lines_of(&rows).len(), 1);
        assert!(lines_of(&rowset(&reg, Some(OTHER))).is_empty());
        assert!(lines_of(&rowset(&reg, None)).is_empty());
    }

    fn site_fixture() -> Site {
        Site {
            system: HIGH,
            row: 0,
            k: 0,
            slot: 0,
            generation: 0,
            site_id: 1,
            dungeon_id: FROM,
            archetype: combat_site::COMBAT_ARCHETYPE,
            entry: Entry::Anomaly,
            anchor: [0.0; 3],
            target_id: "AAA-000".to_owned(),
            state: population::State::Live,
            at: 0,
        }
    }

    fn lines_of(v: &PyValue) -> Vec<PyValue> {
        let PyValue::Instance { state, .. } = v else {
            panic!("an instance: {v:?}")
        };
        match crate::pyutil::dict_get(state, b"lines") {
            Some(PyValue::List(l)) => l.clone(),
            other => panic!("lines: {other:?}"),
        }
    }

    /// What the clock has for the pilots now, as `(event, args, to)`.
    type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

    fn sent(reg: &mut Registry) -> Vec<Sent> {
        reg.take_due(Instant::now() + std::time::Duration::from_secs(5))
            .into_iter()
            .map(|p| {
                let (event, args) = p.to_push();
                (event, args, p.audience())
            })
            .collect()
    }

    /// The finisher is told with the client's own event and one argument, the
    /// instance id the journal looks up; nobody else is.
    #[test]
    fn the_finisher_is_sent_on_escalating_path_message_with_the_instance() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        standing(&mut reg, PILOT, [0.0; 3]);
        let from = Site {
            system: HIGH,
            site_id: queued().from_site,
            dungeon_id: FROM,
            ..site_fixture()
        };
        assert!(site::instantiate(&mut reg, &from));
        let _ = sent(&mut reg);
        let id = spawn(&mut reg, &queued(), now).expect("spawned");
        assert_eq!(
            sent(&mut reg),
            vec![(EVENT_MESSAGE, vec![PyValue::Int(id)], Some(vec![PILOT]))]
        );
    }

    /// The row's cells are what the client reads off it, in the header's
    /// order, and the attributes it dereferences are there.
    #[test]
    fn a_row_carries_every_attribute_the_client_reads() {
        let reg = world();
        let e = Esc {
            site_id: 8_100_000_001,
            system: HIGH,
            dungeon: TO,
            from_dungeon: FROM,
            journal: TEXT,
            created: 1,
            expires: 2,
            step: 1,
            chars: vec![PILOT],
        };
        let cells = line(&reg, &e);
        assert_eq!(cells.len(), COLUMNS.len());
        let attr = |v: &PyValue, name: &str| {
            let PyValue::Instance { state, .. } = v else {
                panic!("an instance: {v:?}")
            };
            crate::pyutil::dict_get(state, name.as_bytes()).cloned()
        };
        let at = |name: &str| COLUMNS.iter().position(|c| *c == name).unwrap();
        assert_eq!(cells[at("instanceID")], PyValue::Int(e.site_id));
        let dest = &cells[at("destDungeon")];
        assert_eq!(attr(dest, "dungeonNameID"), Some(PyValue::Int(TO + 1)));
        assert_eq!(attr(dest, "difficulty"), Some(PyValue::Int(3)));
        assert_eq!(attr(dest, "factionID"), Some(PyValue::Int(FACTION)));
        let site = &cells[at("dungeon")];
        assert_eq!(attr(site, "instanceID"), Some(PyValue::Int(e.site_id)));
        assert_eq!(attr(site, "solarSystemID"), Some(PyValue::Int(HIGH)));
        assert_eq!(attr(site, "destDungeonID"), Some(PyValue::Int(TO)));
        assert_eq!(cells[at("expiryTime")], PyValue::Int(2));
        assert_eq!(cells[at("journalEntryTemplateID")], PyValue::Int(TEXT));
        assert_eq!(cells[at("solarSystemID")], PyValue::Int(HIGH));
        let src = &cells[at("srcDungeon")];
        assert_eq!(attr(src, "dungeonNameID"), Some(PyValue::Int(FROM + 1)));
    }

    /// A pilot in a hot system, at the anchor, gets the room; an escalation
    /// past its limit goes (row, instance and the pilot's journal).
    #[test]
    fn an_escalation_is_placed_on_arrival_and_goes_at_its_limit() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        standing(&mut reg, PILOT, [0.0; 3]);
        let e = Esc {
            site_id: site_id_of(rank_of(&reg, HIGH).unwrap(), 0).unwrap(),
            system: HIGH,
            dungeon: TO,
            from_dungeon: FROM,
            journal: TEXT,
            created: now,
            expires: now + blue(60),
            step: 1,
            chars: vec![PILOT],
        };
        reg.deltas_mut().put(KIND, e.key(), e.value());
        crate::liveness::refresh(&mut reg, Instant::now(), now);
        // The pilot is nowhere near the anchor: nothing is placed.
        let anchor = as_site(&reg, &e).anchor;
        tick(&mut reg, Instant::now(), now);
        assert!(reg.ext::<Sites>().get(e.site_id).is_none());
        // At the anchor: placed once.
        standing(&mut reg, PILOT, anchor);
        tick(&mut reg, Instant::now(), now);
        let inst = reg.ext::<Sites>().get(e.site_id).expect("placed").clone();
        assert_eq!(
            (inst.system, inst.dungeon_id, inst.anchor),
            (HIGH, TO, anchor)
        );
        tick(&mut reg, Instant::now(), now);
        assert!(get(&reg, e.site_id).is_some());
        // The site path leaves an escalation's instance alone.
        assert!(is_escalation_site(inst.site_id));
        let _ = sent(&mut reg);
        // Past the limit.
        tick(&mut reg, Instant::now(), e.expires);
        assert!(get(&reg, e.site_id).is_none(), "the row");
        assert!(reg.ext::<Sites>().get(e.site_id).is_none(), "the instance");
        assert_eq!(
            sent(&mut reg),
            vec![(EVENT_CHANGE, vec![], Some(vec![PILOT]))]
        );
    }

    #[test]
    fn clearing_the_site_takes_the_row_off_the_journal() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        let e = Esc {
            site_id: 8_100_000_002,
            system: HIGH,
            dungeon: TO,
            from_dungeon: FROM,
            journal: TEXT,
            created: now,
            expires: now + blue(60),
            step: 1,
            chars: vec![PILOT],
        };
        reg.deltas_mut().put(KIND, e.key(), e.value());
        assert!(!completed(&mut reg, 8_100_000_999), "not an escalation");
        assert!(completed(&mut reg, e.site_id));
        assert!(get(&reg, e.site_id).is_none());
        assert!(!completed(&mut reg, e.site_id), "once");
        assert_eq!(
            sent(&mut reg),
            vec![(EVENT_CHANGE, vec![], Some(vec![PILOT]))]
        );
        assert!(lines_of(&rowset(&reg, Some(PILOT))).is_empty());
    }

    /// `epinstance` warps: the pilot's own, live escalation, in their own
    /// system, to the site's anchor; nothing else.
    #[test]
    fn the_epinstance_warp_resolves_only_the_pilots_own_live_escalation() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        standing(&mut reg, PILOT, [0.0; 3]);
        let mut e = Esc {
            site_id: 8_100_000_003,
            system: HIGH,
            dungeon: TO,
            from_dungeon: FROM,
            journal: TEXT,
            created: now,
            expires: now + blue(3600),
            step: 1,
            chars: vec![PILOT],
        };
        reg.deltas_mut().put(KIND, e.key(), e.value());
        let table: &[(&[u8], crate::warp_subject::Resolver)] = &[(WARP_SUBJECT, warp_target)];
        let go = |reg: &Registry, who: i64, id: i64| {
            crate::warp_subject::resolve(table, reg, WARP_SUBJECT, who, id, None)
        };
        let t = go(&reg, PILOT, e.site_id).expect("a target");
        assert_eq!(t.position, as_site(&reg, &e).anchor);
        assert!(go(&reg, OTHER, e.site_id).is_none(), "another pilot's");
        assert!(go(&reg, PILOT, e.site_id + 1).is_none(), "not an instance");
        e.expires = now - 1;
        reg.deltas_mut().put(KIND, e.key(), e.value());
        assert!(go(&reg, PILOT, e.site_id).is_none(), "run out");
    }

    /// The row the server's table carries: the subject is the client's.
    #[test]
    fn the_warp_row_is_registered_for_the_clients_subject() {
        assert!(
            crate::warp_subject::RESOLVERS
                .iter()
                .any(|(s, _)| *s == WARP_SUBJECT)
        );
        assert_eq!(WARP_SUBJECT, b"epinstance");
    }

    /// A cleared escalation climbs one rung of the same faction's ladder.
    #[test]
    fn a_cleared_escalation_climbs_to_the_next_difficulty() {
        let mut t = dungeons();
        for (id, d) in [(3, TO + 10), (4, TO + 11), (5, TO + 12)] {
            t.dungeons.insert(
                d,
                Dungeon {
                    id: d,
                    archetype: combat_site::ESCALATION_ARCHETYPE,
                    faction: Some(FACTION),
                    difficulty: Some(id),
                    ..Dungeon::default()
                },
            );
        }
        let first = &t.dungeons[&TO];
        assert_eq!(rung(first), 1);
        assert_eq!(combat_site::escalation_candidates(&t, first), vec![TO + 11]);
        let next = &t.dungeons[&(TO + 11)];
        assert_eq!(rung(next), 2);
        assert_eq!(combat_site::escalation_candidates(&t, next), vec![TO + 12]);
        let top = &t.dungeons[&(TO + 12)];
        assert!(combat_site::escalation_candidates(&t, top).is_empty());
        let n = 4_000;
        let climbs = (0..n)
            .filter(|s| combat_site::escalation_for(&t, first, 9, *s, 1).is_some())
            .count() as f64
            / n as f64;
        assert!(
            (climbs - combat_site::ESCALATION_STEP_CHANCE).abs() < 0.03,
            "{climbs}"
        );
    }
}
