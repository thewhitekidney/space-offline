//! **Motherlodes, belt ambushes and rich sites** (lane l6b-mining-events,
//! issue #33; spec `handback/r-nes-econ/report.md` §2.6 B-E).
//!
//! The core gave this module four seams: a mining hook row (`Module::mined`),
//! a stage, a kill-hook row and `eve_server::world_events` (the server
//! message, a bot's Local line, the map, a pirate squad on demand). What
//! happens, how often and what it pays is here.
//!
//! - **Motherlode.** Every [`tunables::ML_INTERVAL_S`] the scheduler picks,
//!   for each region with an active mining system, one belt of one such
//!   system ([`pick_per_region`], seeded). For [`tunables::ML_DURATION_S`]
//!   every cut on that belt's rocks also mints [`tunables::ML_BONUS_YIELD`]
//!   times its yield into the hold (ledgered `motherlode:ore:<type>`), and
//!   the geode chance is times [`tunables::ML_GEODE_MULT`] ([`multiplier`],
//!   the hook `geode` reads). Pilots in the system get an `OnServerMessage`,
//!   a bot there says it in Local, and the systems within
//!   [`tunables::ML_ECHO_JUMPS`] gates hear a Local line, and every pilot
//!   there gets the client's own toast (`world_events::notice`, no dialog),
//!   [`tunables::ML_ECHO_DELAY_S`] later. The Local line carries `[ml:<system
//!   id>]` so a bot can read it ([`heard`]).
//! - **Ambush.** After [`tunables::AMBUSH_FIRST_S`], and then by band, an
//!   extra pirate squad comes to the motherlode belt (and to a rich site that
//!   is being mined). Its leader's wreck rolls the ambush row
//!   ([`on_npc_kill`]): PLEX, an injector, a geode. Survivors leave with the
//!   event.
//! - **Rich site.** An anomaly drawn by a `population.tsv` row whose key
//!   contains [`tunables::RICH_MARK`] pays [`tunables::RICH_BONUS_YIELD`]
//!   extra, times [`tunables::RICH_GEODE_MULT`] geodes, and is ambushed while
//!   it is worked. Not announced: it is scanned down.
//!
//! **Nothing from nowhere.** Minted ore is counted in this module's ledger
//! rows (`motherlode:ore:<type>`, `rich:ore:<type>`, in units) and in the
//! core's goods ledger (`Registry::apply_items`); PLEX, injectors and geodes
//! of an ambush are ledgered by the same helpers a drop uses
//! (`drop:ambush`, `geode:crate:<tier>`).
//!
//! **Not persisted: the rats.** The event rows are (`econmod`, key `ml`), so
//! a restart keeps a running event and its schedule; a wave dies with the
//! process like every NPC.
//!
//! **Departures from the spec**, each **[A]** with its falsifier in
//! [`tunables`]: rocks are not made x3 larger (the bonus is minted per cut);
//! a Local line needs a bot in the system to speak it (the toast to the
//! neighbours does not); a rich site's "rarer
//! ores" are not done (its row draws the band's ordinary ore dungeon).

use crate::tunables::{self, AMBUSH_EVERY_S};
use crate::{KIND, drops, geode, injectors};
use eve_server::celestial::Band;
use eve_server::combat::{FLAG_IN_WRECK, Wreck};
use eve_server::content::Boxed;
use eve_server::item::{Change, Edit, Slot};
use eve_server::module::{Deltas, KillEvent, MinedEvent, SlotSpec};
use eve_server::seed::{self, Kind, Stream};
use eve_server::service::Registry;
use eve_server::skill_trading::{TYPE_LARGE, TYPE_SMALL};
use eve_server::world_events as we;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// Blue time is 100 ns ticks.
const SECOND: i64 = 10_000_000;
const DAY_TICKS: i64 = 864_000_000_000;

/// Mixed into this module's draws.
const SALT_PICK: u64 = 0x4d4c_5049;
const SALT_BONUS: u64 = 0x4d4c_4259;
const SALT_WAVE: u64 = 0x4d4c_5741;
const SALT_PRIZE: u64 = 0x4d4c_5052;

/// An ambush op id for a rich site, apart from the systems' (a motherlode's
/// op is its system id).
const RICH_OP: i64 = 1 << 56;

// ---- the world slot ----------------------------------------------------------

/// A motherlode in progress. Persisted (`econmod`, key `["ml", system]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub belt_id: i64,
    pub start: i64,
    pub end: i64,
    pub next_ambush: i64,
    pub echoed: bool,
}

impl Event {
    fn row(&self) -> Vec<String> {
        vec![
            self.belt_id.to_string(),
            self.start.to_string(),
            self.end.to_string(),
            self.next_ambush.to_string(),
            u8::from(self.echoed).to_string(),
        ]
    }

    fn parse(v: &[String]) -> Option<Event> {
        let n = |i: usize| v.get(i)?.parse::<i64>().ok();
        Some(Event {
            belt_id: n(0)?,
            start: n(1)?,
            end: n(2)?,
            next_ambush: n(3)?,
            echoed: n(4)? != 0,
        })
    }
}

/// A rich site that is being worked.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Worked {
    system: i64,
    anchor: [f64; 3],
    last_cut: i64,
    next_ambush: i64,
}

/// One extra squad in space.
#[derive(Debug, Clone, PartialEq)]
struct Wave {
    op: i64,
    system: i64,
    ids: Vec<i64>,
}

/// What the module keeps in RAM: the mirror of the event rows, where miners
/// have been, worked rich sites and the live waves.
#[derive(Debug, Default)]
pub struct State {
    loaded: bool,
    /// The persisted rows, by system.
    events: BTreeMap<i64, Event>,
    /// System to the belt and time of its last belt cut.
    activity: BTreeMap<i64, (i64, i64)>,
    worked: BTreeMap<i64, Worked>,
    waves: Vec<Wave>,
    /// The leader of each live wave.
    leaders: BTreeSet<i64>,
    /// When the scheduler next runs (0: not set yet).
    next_event: i64,
    /// Ambushes made so far, mixed into the squad's draw.
    ambushes: u64,
    /// The last tick's blue time, for the slash command.
    now: i64,
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "econ_motherlode",
    make,
};

fn make(_: &eve_server::content::Content) -> Boxed {
    Box::new(State::default())
}

fn ml_key(system: i64) -> Vec<String> {
    vec!["ml".to_owned(), system.to_string()]
}

fn next_key() -> Vec<String> {
    vec!["mlnext".to_owned()]
}

fn save(d: &mut Deltas, system: i64, e: &Event) {
    d.put(KIND, ml_key(system), e.row());
}

/// The running events, read from the rows.
pub fn events_in(d: &Deltas) -> BTreeMap<i64, Event> {
    d.of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[0] == "ml")
        .filter_map(|(k, v)| Some((k[1].parse().ok()?, Event::parse(v)?)))
        .collect()
}

/// The motherlodes running at `now`, as `(system, belt, ends)`: what a bot
/// or a panel asks for.
pub fn active(reg: &Registry, now: i64) -> Vec<(i64, i64, i64)> {
    events_in(reg.deltas())
        .into_iter()
        .filter(|(_, e)| e.end > now)
        .map(|(s, e)| (s, e.belt_id, e.end))
        .collect()
}

// ---- pure rules --------------------------------------------------------------

/// Where a cut happened.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zone {
    Plain,
    Motherlode,
    Rich { site_id: i64 },
}

/// The ambush interval of a band, in blue ticks.
pub fn ambush_every(band: Band) -> i64 {
    let s = match band {
        Band::High => AMBUSH_EVERY_S[0],
        Band::Low => AMBUSH_EVERY_S[1],
        Band::Null | Band::Wormhole | Band::Void => AMBUSH_EVERY_S[2],
    };
    s * SECOND
}

/// `floor(x)`, plus one with probability `frac(x)`.
pub fn round_rand(s: &mut Stream, x: f64) -> i64 {
    let x = x.max(0.0);
    let whole = x.floor();
    whole as i64 + i64::from(s.chance(x - whole))
}

/// The geode multiplier of a zone.
pub fn zone_geode_mult(z: Zone) -> f64 {
    match z {
        Zone::Plain => 1.0,
        Zone::Motherlode => tunables::ML_GEODE_MULT,
        Zone::Rich { .. } => tunables::RICH_GEODE_MULT,
    }
}

/// The extra ore a cut of `normal` units mints in a zone.
pub fn zone_bonus(s: &mut Stream, z: Zone, normal: i64) -> i64 {
    let f = match z {
        Zone::Plain => 0.0,
        Zone::Motherlode => tunables::ML_BONUS_YIELD,
        Zone::Rich { .. } => tunables::RICH_BONUS_YIELD,
    };
    round_rand(s, normal as f64 * f)
}

/// One system per region from `cands` (`region -> systems`), skipping
/// `busy` regions. A pure function of `seed` and `tag`: each region draws
/// from its own stream, so the answer does not depend on the others.
pub fn pick_per_region(
    cands: &BTreeMap<i64, Vec<i64>>,
    busy: &BTreeSet<i64>,
    seed: u64,
    tag: u64,
) -> Vec<i64> {
    let mut out = Vec::new();
    for (region, systems) in cands {
        if busy.contains(region) || systems.is_empty() {
            continue;
        }
        let mut sorted = systems.clone();
        sorted.sort_unstable();
        sorted.dedup();
        let mut s = seed::stream(seed, Kind::Spawn, &[SALT_PICK, *region as u64, tag]);
        out.push(sorted[s.range(0, sorted.len() as u64) as usize]);
    }
    out
}

/// The Local line a bot says, and reads. `[ml:<system>]` is the machine part.
pub fn local_line(name: &str, system: i64, echo: bool) -> String {
    if echo {
        format!("Motherlode reported near here: {name} [ml:{system}] has a very rich belt")
    } else {
        format!("Motherlode in {name} [ml:{system}]: a belt here is very rich")
    }
}

/// The system a Local line announces, if it is one of ours.
pub fn heard(text: &str) -> Option<i64> {
    let from = text.find("[ml:")? + 4;
    let to = text[from..].find(']')? + from;
    text[from..to].parse().ok()
}

/// The server message the pilots in the system get.
pub fn message_text(name: &str) -> String {
    format!(
        "Motherlode! One of the belts in {name} is unusually rich for the next {} minutes: \
         more ore from every cycle, and more chance of a geode. Pirates have noticed.",
        tunables::ML_DURATION_S / 60
    )
}

/// The toast the pilots in a neighbouring system get: `(title, subtitle,
/// text)`. The client draws the first two on the toast and the text in a
/// non-modal window when it is clicked.
pub fn echo_notice(name: &str) -> (String, String, String) {
    (
        "Motherlode nearby".to_owned(),
        format!("{name} has a very rich belt"),
        format!(
            "A belt in {name} is unusually rich for a few more minutes: more ore from \
             every cycle, and more chance of a geode. Pirates have noticed."
        ),
    )
}

// ---- the mining hook ---------------------------------------------------------

/// Which zone a cut was in. Reads only.
pub fn zone_of(reg: &Registry, ev: &MinedEvent) -> Zone {
    if let Some(e) = reg.ext::<State>().events.get(&ev.system)
        && ev.at < e.end
        && reg
            .rocks()
            .get(ev.rock_id)
            .is_some_and(|r| r.belt_id == e.belt_id)
    {
        return Zone::Motherlode;
    }
    match we::rock_site(reg, ev.rock_id, ev.at) {
        Some(s) if s.row_key.contains(tunables::RICH_MARK) => Zone::Rich { site_id: s.site_id },
        _ => Zone::Plain,
    }
}

/// What `geode` multiplies its chance by for this cut.
pub fn multiplier(reg: &Registry, ev: &MinedEvent) -> f64 {
    zone_geode_mult(zone_of(reg, ev))
}

/// The mining-hook row: note where miners are, and pay a zone's extra ore.
pub fn on_mined(reg: &mut Registry, ev: &MinedEvent) {
    // A rock mined out by this very cut is gone from the field; the belt is
    // then unknown and the cut counts as plain. [A]
    let belt = reg.rocks().get(ev.rock_id).map(|r| r.belt_id);
    if let Some(b) = belt.filter(|b| *b != 0) {
        reg.ext_mut::<State>()
            .activity
            .insert(ev.system, (b, ev.at));
    }
    let zone = zone_of(reg, ev);
    if zone == Zone::Plain {
        return;
    }
    if let Zone::Rich { site_id } = zone
        && let Some(site) = we::rock_site(reg, ev.rock_id, ev.at)
    {
        let st = reg.ext_mut::<State>();
        let e = st.worked.entry(site_id).or_insert(Worked {
            system: site.system,
            anchor: site.anchor,
            last_cut: ev.at,
            next_ambush: ev.at + tunables::AMBUSH_FIRST_S * SECOND,
        });
        e.last_cut = ev.at;
    }
    let mut s = seed::stream(
        reg.world().seed,
        Kind::Loot,
        &[
            SALT_BONUS,
            ev.rock_id as u64,
            ev.source_id as u64,
            ev.miner as u64,
            ev.at as u64,
        ],
    );
    let bonus = zone_bonus(&mut s, zone, ev.normal);
    if bonus < 1 {
        return;
    }
    let Some(ore) = eve_server::item::find_in(reg.content(), ev.rock_type).copied() else {
        return;
    };
    let room = eve_server::mining_hold::room_for(reg, ev.ship_id, ev.ship_type_id, &ore);
    let (to_hold, to_cargo) = room.split(bonus);
    let mut edits = Vec::new();
    for (flag, n) in [
        (eve_server::mining_hold::FLAG_GENERAL_MINING_HOLD, to_hold),
        (eve_server::fitting::FLAG_CARGO, to_cargo),
    ] {
        if n < 1 {
            continue;
        }
        let slot = Slot::new(ev.miner, ev.ship_id, flag);
        let stack = reg
            .store()
            .at(slot)
            .filter(|i| i.type_id == ev.rock_type && !i.singleton)
            .map(|i| i.item_id)
            .min();
        edits.push(match stack {
            Some(item_id) => Edit::Grow {
                item_id,
                quantity: n,
            },
            None => Edit::Create {
                type_id: ev.rock_type,
                quantity: n,
                to: slot,
            },
        });
    }
    if edits.is_empty() || reg.apply_items(&edits).is_err() {
        return;
    }
    let made = to_hold + to_cargo;
    let name = match zone {
        Zone::Motherlode => "motherlode",
        _ => "rich",
    };
    geode::ledger_add(
        reg.deltas_mut(),
        &format!("{name}:ore:{}", ev.rock_type),
        made,
        0,
    );
}

// ---- the stage ---------------------------------------------------------------

fn load(reg: &mut Registry) {
    let events = events_in(reg.deltas());
    let next = reg
        .deltas()
        .of(KIND)
        .get(&next_key())
        .and_then(|v| v.first())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let st = reg.ext_mut::<State>();
    st.loaded = true;
    st.events = events;
    st.next_event = next;
}

/// The stage: begin, announce, ambush and end motherlodes, and ambush rich
/// sites that are being worked. After `asteroid` (its rocks are the field's).
pub fn tick(reg: &mut Registry, _now: Instant, ft: i64) {
    if !reg.ext::<State>().loaded {
        load(reg);
    }
    reg.ext_mut::<State>().now = ft;
    expire_and_ambush(reg, ft);
    ambush_worked(reg, ft);
    prune_waves(reg);
    let due = reg.ext::<State>().next_event;
    if due == 0 {
        set_next(reg, ft + tunables::ML_FIRST_DELAY_S * SECOND);
    } else if ft >= due {
        schedule(reg, ft);
        set_next(reg, ft + tunables::ML_INTERVAL_S * SECOND);
    }
}

fn set_next(reg: &mut Registry, at: i64) {
    reg.ext_mut::<State>().next_event = at;
    reg.deltas_mut().put(KIND, next_key(), vec![at.to_string()]);
}

/// Start a motherlode in each region that has an active mining system and
/// none running.
fn schedule(reg: &mut Registry, ft: i64) {
    let window = tunables::ML_ACTIVE_WINDOW_S * SECOND;
    let (active, running): (Vec<(i64, i64)>, BTreeSet<i64>) = {
        let st = reg.ext::<State>();
        (
            st.activity
                .iter()
                .filter(|(s, (_, at))| ft - *at <= window && !st.events.contains_key(s))
                .map(|(s, (belt, _))| (*s, *belt))
                .collect(),
            st.events.keys().copied().collect(),
        )
    };
    let mut cands: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let mut belts = BTreeMap::new();
    for (system, belt) in active {
        if let Some(f) = we::system_facts(reg, system) {
            cands.entry(f.region_id).or_default().push(system);
            belts.insert(system, belt);
        }
    }
    let busy: BTreeSet<i64> = running
        .iter()
        .filter_map(|s| we::system_facts(reg, *s).map(|f| f.region_id))
        .collect();
    for system in pick_per_region(&cands, &busy, reg.world().seed, ft as u64) {
        if let Some(belt) = belts.get(&system) {
            begin(reg, system, *belt, ft);
        }
    }
}

/// Open a motherlode on `belt` of `system` at `ft` and announce it.
pub fn begin(reg: &mut Registry, system: i64, belt: i64, ft: i64) {
    let e = Event {
        belt_id: belt,
        start: ft,
        end: ft + tunables::ML_DURATION_S * SECOND,
        next_ambush: ft + tunables::AMBUSH_FIRST_S * SECOND,
        echoed: false,
    };
    reg.ext_mut::<State>().events.insert(system, e);
    save(reg.deltas_mut(), system, &e);
    let name = we::system_facts(reg, system).map_or_else(|| system.to_string(), |f| f.name);
    let pilots = we::online_in(reg, system);
    we::message(reg, pilots, &message_text(&name));
    we::say_in_local(reg, system, &local_line(&name, system, false));
    eve_server::outln!("[econ-mod] motherlode: begins in {name} ({system}), belt {belt}");
}

fn expire_and_ambush(reg: &mut Registry, ft: i64) {
    let events: Vec<(i64, Event)> = reg
        .ext::<State>()
        .events
        .iter()
        .map(|(s, e)| (*s, *e))
        .collect();
    for (system, mut e) in events {
        if ft >= e.end {
            end(reg, system);
            continue;
        }
        let mut changed = false;
        if !e.echoed && ft >= e.start + tunables::ML_ECHO_DELAY_S * SECOND {
            echo(reg, system);
            e.echoed = true;
            changed = true;
        }
        if ft >= e.next_ambush {
            let facts = we::system_facts(reg, system);
            let band = facts.map_or(Band::High, |f| f.band);
            if let Some(at) = we::belt_centre(reg, system, e.belt_id) {
                wave(reg, system, system, e.belt_id, at);
            }
            e.next_ambush = ft + ambush_every(band);
            changed = true;
        }
        if changed {
            reg.ext_mut::<State>().events.insert(system, e);
            save(reg.deltas_mut(), system, &e);
        }
    }
}

fn echo(reg: &mut Registry, system: i64) {
    let name = we::system_facts(reg, system).map_or_else(|| system.to_string(), |f| f.name);
    let line = local_line(&name, system, true);
    let (title, subtitle, text) = echo_notice(&name);
    for n in we::near(reg, system, tunables::ML_ECHO_JUMPS) {
        we::say_in_local(reg, n, &line);
        let pilots = we::online_in(reg, n);
        we::notice(reg, pilots, &title, &subtitle, &text);
    }
}

fn end(reg: &mut Registry, system: i64) {
    reg.ext_mut::<State>().events.remove(&system);
    reg.deltas_mut().del(KIND, &ml_key(system));
    drop_waves(reg, system);
    eve_server::outln!("[econ-mod] motherlode: ends in {system}");
}

/// Take `op`'s waves out of space.
fn drop_waves(reg: &mut Registry, op: i64) {
    let (gone, keep): (Vec<Wave>, Vec<Wave>) = std::mem::take(&mut reg.ext_mut::<State>().waves)
        .into_iter()
        .partition(|w| w.op == op);
    reg.ext_mut::<State>().waves = keep;
    for w in gone {
        we::clear_ambush(reg, w.system, &w.ids);
        let st = reg.ext_mut::<State>();
        for id in &w.ids {
            st.leaders.remove(id);
        }
    }
}

/// Spawn one extra squad for `op` unless the last is still alive.
fn wave(reg: &mut Registry, op: i64, system: i64, belt: i64, at: [f64; 3]) {
    let alive = {
        let st = reg.ext::<State>();
        st.waves
            .iter()
            .filter(|w| w.op == op)
            .any(|w| w.ids.iter().any(|id| we::in_space(reg, *id)))
    };
    if alive {
        return;
    }
    let n = {
        let st = reg.ext_mut::<State>();
        st.ambushes += 1;
        st.ambushes
    };
    let salt = SALT_WAVE ^ (op as u64).rotate_left(7) ^ n;
    let ids = we::ambush(reg, system, belt, at, salt);
    let Some(leader) = ids.first().copied() else {
        return;
    };
    eve_server::outln!(
        "[econ-mod] ambush: {} rats at belt {belt} of {system} (op {op})",
        ids.len()
    );
    let st = reg.ext_mut::<State>();
    st.leaders.insert(leader);
    st.waves.push(Wave { op, system, ids });
}

fn ambush_worked(reg: &mut Registry, ft: i64) {
    let sites: Vec<(i64, Worked)> = reg
        .ext::<State>()
        .worked
        .iter()
        .map(|(s, w)| (*s, *w))
        .collect();
    for (site, w) in sites {
        if ft - w.last_cut > tunables::RICH_ACTIVE_S * SECOND {
            reg.ext_mut::<State>().worked.remove(&site);
            drop_waves(reg, RICH_OP | site);
            continue;
        }
        if ft >= w.next_ambush {
            let band = we::system_facts(reg, w.system).map_or(Band::Low, |f| f.band);
            wave(reg, RICH_OP | site, w.system, site, w.anchor);
            if let Some(e) = reg.ext_mut::<State>().worked.get_mut(&site) {
                e.next_ambush = ft + ambush_every(band);
            }
        }
    }
}

/// Forget waves whose rats are all gone (killed, or removed with the
/// system), and their leaders.
fn prune_waves(reg: &mut Registry) {
    let gone: Vec<usize> = {
        let st = reg.ext::<State>();
        st.waves
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.ids.iter().any(|id| we::in_space(reg, *id)))
            .map(|(i, _)| i)
            .collect()
    };
    if gone.is_empty() {
        return;
    }
    let st = reg.ext_mut::<State>();
    for i in gone.into_iter().rev() {
        let w = st.waves.remove(i);
        for id in w.ids {
            st.leaders.remove(&id);
        }
    }
}

// ---- the ambush prize --------------------------------------------------------

/// What a leader's wreck holds, rolled. Draws every chance, so the stream's
/// use does not depend on the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Prize {
    pub plex: i64,
    pub small: bool,
    pub large: bool,
    pub geode: bool,
}

/// Roll the ambush row. `plex_scale` and `sp_scale` are the soft-cap
/// multipliers of the day's PLEX and skill-point budgets.
pub fn roll_prize(s: &mut Stream, plex_scale: f64, sp_scale: f64) -> Prize {
    let (p, lo, hi) = tunables::AMBUSH_PLEX;
    let plex_hit = s.chance(p * plex_scale);
    let qty = s.range(lo, hi + 1) as i64;
    let small = s.chance(tunables::AMBUSH_SMALL * sp_scale);
    let large = s.chance(tunables::AMBUSH_LARGE * sp_scale);
    let geode = s.chance(tunables::AMBUSH_GEODE);
    Prize {
        plex: if plex_hit { qty } else { 0 },
        small: small && !large,
        large,
        geode,
    }
}

/// The kill-hook row: the leader of an ambush leaves its prize.
pub fn on_npc_kill(reg: &mut Registry, ev: &KillEvent) {
    if !reg.ext_mut::<State>().leaders.remove(&ev.victim_character) {
        return;
    }
    let day = ev.when.div_euclid(DAY_TICKS);
    let plex_scale = if drops::minted_on(reg.deltas(), day) >= tunables::DAILY_BUDGET {
        tunables::OVER_BUDGET_MULT
    } else {
        1.0
    };
    let sp_scale = if injectors::minted_on(reg.deltas(), day) >= tunables::DAILY_SP_BUDGET {
        tunables::OVER_BUDGET_MULT
    } else {
        1.0
    };
    let mut s = seed::stream(
        reg.world().seed,
        Kind::Loot,
        &[SALT_PRIZE, ev.victim_character as u64],
    );
    let prize = roll_prize(&mut s, plex_scale, sp_scale);
    let Some(wreck) = reg
        .combat()
        .wrecks()
        .find(|w| w.ship_id == ev.victim_ship)
        .copied()
    else {
        return;
    };
    if prize.plex > 0 {
        drops::leave_in(reg, &wreck, "ambush", prize.plex, day);
    }
    if prize.large {
        injectors::leave_in(reg, &wreck, "ambush", TYPE_LARGE, day);
    } else if prize.small {
        injectors::leave_in(reg, &wreck, "ambush", TYPE_SMALL, day);
    }
    if prize.geode {
        leave_geode(reg, &wreck);
    }
    eve_server::outln!(
        "[econ-mod] ambush prize at wreck {}: plex {}, small {}, large {}, geode {}",
        wreck.wreck_id,
        prize.plex,
        prize.small,
        prize.large,
        prize.geode
    );
}

/// One tier-1 geode in `wreck`, ledgered like a drop. Quiet, as PLEX's.
fn leave_geode(reg: &mut Registry, wreck: &Wreck) -> bool {
    let Some((type_id, tier)) = geode::geodes(reg).and_then(|g| g.type_for(1)) else {
        return false;
    };
    let made = reg.apply_items_quiet(&[Edit::Create {
        type_id,
        quantity: 1,
        to: Slot::new(wreck.owner_id, wreck.wreck_id, FLAG_IN_WRECK),
    }]);
    if !matches!(made.as_deref(), Ok([Change::Created { .. }, ..])) {
        return false;
    }
    geode::ledger_add(reg.deltas_mut(), &format!("geode:crate:{tier}"), 1, 0);
    true
}

/// `/motherlode [begin|end|status]`: a GM tool for the client scenario and
/// for a person who wants to see one. `begin` opens one in the caller's
/// system on the belt they last mined, else the first belt; `end` closes it.
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<eve_marshal::PyValue, eve_server::service::ServiceError> {
    let say = |t: &str| Ok(eve_marshal::PyValue::Bytes(t.as_bytes().to_vec()));
    let Some(actor) = actor else {
        return say("no character");
    };
    let Some(system) = we::system_of(reg, actor) else {
        return say("no system");
    };
    let now = reg.ext::<State>().now;
    match args.first().copied().unwrap_or("status") {
        "begin" => {
            if reg.ext::<State>().events.contains_key(&system) {
                return say("a motherlode is already running here");
            }
            let belt = reg
                .ext::<State>()
                .activity
                .get(&system)
                .map(|(b, _)| *b)
                .or_else(|| we::first_belt(reg, system));
            let Some(belt) = belt else {
                return say("this system has no belt");
            };
            begin(reg, system, belt, now);
            say(&format!("A motherlode begins on belt {belt}."))
        }
        "end" => {
            if !reg.ext::<State>().events.contains_key(&system) {
                return say("no motherlode is running here");
            }
            end(reg, system);
            say("The motherlode is over.")
        }
        "status" => {
            let left: Vec<String> = active(reg, now)
                .iter()
                .map(|(s, b, e)| format!("{s} belt {b} for {} min", (e - now) / SECOND / 60))
                .collect();
            if left.is_empty() {
                say("no motherlode is running")
            } else {
                say(&format!("running: {}", left.join("; ")))
            }
        }
        _ => say("usage: /motherlode [begin|end|status]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_row_round_trips() {
        let e = Event {
            belt_id: 41,
            start: 5,
            end: 9,
            next_ambush: 7,
            echoed: true,
        };
        assert_eq!(Event::parse(&e.row()), Some(e));
        assert_eq!(Event::parse(&["x".to_owned()]), None);
    }

    #[test]
    fn the_scheduler_picks_one_system_per_region_and_skips_busy_ones() {
        let cands = BTreeMap::from([
            (10, vec![3, 1, 2]),
            (11, vec![8, 9]),
            (12, vec![]),
            (13, vec![5]),
        ]);
        let busy = BTreeSet::from([13]);
        let got = pick_per_region(&cands, &busy, 77, 1);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!([1, 2, 3].contains(&got[0]));
        assert!([8, 9].contains(&got[1]));
        assert_eq!(got, pick_per_region(&cands, &busy, 77, 1), "deterministic");
    }

    #[test]
    fn a_regions_pick_does_not_depend_on_the_other_regions() {
        let a = BTreeMap::from([(10, vec![1, 2, 3, 4, 5]), (11, vec![6, 7, 8, 9])]);
        let b = BTreeMap::from([(10, vec![1, 2, 3, 4, 5])]);
        let none = BTreeSet::new();
        for tag in 0..20 {
            assert_eq!(
                pick_per_region(&a, &none, 5, tag)[0],
                pick_per_region(&b, &none, 5, tag)[0]
            );
        }
    }

    #[test]
    fn the_local_line_can_be_read_back_and_nothing_else_is_an_announcement() {
        let l = local_line("Testia", 31_999, false);
        assert_eq!(heard(&l), Some(31_999));
        assert_eq!(heard(&local_line("Testia", 31_999, true)), Some(31_999));
        assert_eq!(heard("o7 [ml:] nope"), None);
        assert_eq!(heard("no tag"), None);
    }

    #[test]
    fn the_neighbours_toast_names_the_system_and_is_not_the_bots_line() {
        let (title, sub, text) = echo_notice("Testia");
        assert!(sub.contains("Testia") && text.contains("Testia"));
        assert!(!title.is_empty());
        assert_eq!(heard(&format!("{title} {sub} {text}")), None);
    }

    #[test]
    fn the_bonus_is_zero_on_a_plain_cut_and_scales_with_the_zone() {
        let mean = |z| {
            (0..2_000u64)
                .map(|i| zone_bonus(&mut Stream::new(i), z, 10))
                .sum::<i64>() as f64
                / 2_000.0
        };
        assert_eq!(mean(Zone::Plain), 0.0);
        let ml = mean(Zone::Motherlode);
        let rich = mean(Zone::Rich { site_id: 1 });
        assert!((19.0..21.0).contains(&ml), "{ml}");
        assert!((9.0..11.0).contains(&rich), "{rich}");
    }

    #[test]
    fn round_rand_keeps_the_mean_and_the_whole_part() {
        let mut s = Stream::new(3);
        let v: Vec<i64> = (0..1_000).map(|_| round_rand(&mut s, 2.25)).collect();
        assert!(v.iter().all(|x| *x == 2 || *x == 3));
        let mean = v.iter().sum::<i64>() as f64 / 1_000.0;
        assert!((2.15..2.35).contains(&mean), "{mean}");
        assert_eq!(round_rand(&mut Stream::new(1), -4.0), 0);
    }

    #[test]
    fn ambushes_come_faster_the_less_safe_the_band() {
        assert!(ambush_every(Band::High) > ambush_every(Band::Low));
        assert!(ambush_every(Band::Low) > ambush_every(Band::Null));
        assert_eq!(ambush_every(Band::Null), ambush_every(Band::Wormhole));
    }

    #[test]
    fn the_prize_stream_use_does_not_depend_on_the_outcome() {
        let mut a = Stream::new(9);
        let mut b = Stream::new(9);
        let _ = roll_prize(&mut a, 1.0, 1.0);
        let _ = roll_prize(&mut b, 0.0, 0.0);
        assert_eq!(a.next(), b.next());
    }

    #[test]
    fn the_prize_rates_are_the_rows() {
        let n = 20_000u64;
        let (mut plex, mut small, mut large, mut geode) = (0, 0, 0, 0);
        for i in 0..n {
            let p = roll_prize(&mut Stream::new(i), 1.0, 1.0);
            if p.plex > 0 {
                plex += 1;
                assert!((5..=20).contains(&p.plex));
            }
            small += i32::from(p.small);
            large += i32::from(p.large);
            geode += i32::from(p.geode);
            assert!(!(p.small && p.large), "one injector at most");
        }
        let f = |c: i32| f64::from(c) / n as f64;
        assert!((0.23..0.27).contains(&f(plex)), "{}", f(plex));
        assert!((0.04..0.06).contains(&f(small)), "{}", f(small));
        assert!((0.005..0.016).contains(&f(large)), "{}", f(large));
        assert!((0.28..0.32).contains(&f(geode)), "{}", f(geode));
    }

    #[test]
    fn the_numbers_are_sane() {
        const { assert!(tunables::ML_DURATION_S < tunables::ML_INTERVAL_S) };
        const { assert!(tunables::ML_ECHO_DELAY_S < tunables::ML_DURATION_S) };
        const { assert!(tunables::AMBUSH_FIRST_S < tunables::ML_DURATION_S) };
        assert!(tunables::AMBUSH_EVERY_S.windows(2).all(|w| w[0] > w[1]));
        const { assert!(tunables::ML_GEODE_MULT > tunables::RICH_GEODE_MULT) };
        const { assert!(tunables::ML_BONUS_YIELD > tunables::RICH_BONUS_YIELD) };
    }
}
