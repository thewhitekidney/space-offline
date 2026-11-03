//! **Killmails** (SOC-C6, R-SOCIETY §6 Stage C row C6): a kill report for
//! every player-ship death, kept, listed in the character sheet's combat log
//! and opened in the client's kill report window.
//!
//! Lane note: `docs/lanes/soc-c6.md`. The wire is `service/killmail.rs`.
//!
//! # Three parts
//!
//! - **The tally** ([`Tally`], a world slot, not persisted). Every volley at
//!   a character's ship is booked by [`on_volley`], called from
//!   `combat::fire` once the volley is applied: who fired (a drone's volley
//!   is its owner's, with the drone as the weapon), from what hull, with
//!   what, and how much it did. A hit older than [`TALLY_WINDOW`] is
//!   forgotten. The tally is what turns `KillEvent`'s one `killer` into the
//!   list of attackers and their damage.
//! - **The report** ([`on_kill`], a `kill_hooks::PLAYER` row). It fires
//!   first in `combat::kill`, while the dead ship and its contents are still
//!   in the store, so the items are read as they were. Each top-level item
//!   is **dropped** when `combat::survives` says so and a wreck will be
//!   left, **destroyed** otherwise -- the same rule and the same seed the
//!   death plan runs a moment later, so the report and the wreck agree. A
//!   drone's death writes nothing (it has no character).
//! - **Persistence.** One `killmail` delta row per report, keyed by kill id,
//!   plus a `next` row for the id counter; [`KEEP_REPORTS`] are kept, oldest
//!   evicted first. And **one line per kill to the ledger log** (E4,
//!   [`LOG_TAG`]) so the economy side sees each loss with its value.
//!
//! **Why delta rows and not the ledger log alone** (the packet's choice):
//! the log is append-only with no read-back for a new record kind (`ledger::
//! open` reads journal and market rows only), it is detached in every test
//! and in a world started without one, and the combat log must survive a
//! restart in both. A capped delta kind needs no change to `ledger.rs`'s
//! reader; the log line is the economy's copy, and one line a kill is all
//! R-SOCIETY §5 asks of it.
//!
//! # What the client reads [V]
//!
//! `killReport.KillReportWnd.FormatKillMail` and `charSheetUtil.
//! GetCombatEntries` read, by attribute: `killID`, `killTime`,
//! `solarSystemID`, `moonID`, `victimCharacterID`, `victimCorporationID`,
//! `victimAllianceID`, `victimFactionID`, `victimShipTypeID`,
//! `victimDamageTaken`, the eight `final*` fields, `warID`, `iskLost`,
//! `iskDestroyed`, `bountyClaimed`, `loyaltyPoints`, `killRightSupplied`
//! and `killBlob`. `eveCommonUtils.GetAttackersAndItemsFromKillMail` parses
//! `killBlob` as XML: `<attackers>` of `<a c r a f s w d t>` and `<items>`
//! of `<i t f s d x>`, an `<i>` holding one level of `<i>` contents. The
//! final blow is **not** in the blob [I]: the parser appends it from the
//! `final*` fields, and the window lists the blob's attackers that are not
//! the final blow, so a final blower in the blob would show twice.

use crate::damage::BlueTime;
use crate::kill_hooks::KillEvent;
use crate::service::Registry;
use std::collections::BTreeMap;

/// The delta kind this module owns (`delta_kinds::KINDS`): `kill_id ->`
/// [`Report::to_row`], and `next -> n`, the next kill id.
pub const KIND: &str = "killmail";

/// SEC-FIX 10: the per-character index, `character kill_id(10 digits) ->`
/// nothing, plus one `ready` row once it covers every kept report. A
/// character's combat log reads its own few rows instead of parsing all
/// [`KEEP_REPORTS`] under the world lock on every call.
pub const KIND_BY_CHAR: &str = "killindex";

/// The marker row of [`KIND_BY_CHAR`]: absent in a world written before the
/// index, which [`involving`] then answers by scanning, until the next
/// [`put`] builds the index from what is kept.
const INDEX_READY: &str = "ready";

/// C6B: the corporation rows share [`KIND_BY_CHAR`]: `k<corp>` (a corporation
/// that fired on the victim) or `l<corp>` (the victim's corporation), then the
/// kill id. A character id is all digits, so a letter-led key never meets
/// one. Its own marker, because a world written before C6B has the
/// character index and not these.
const CORP_INDEX_READY: &str = "readycorp";

/// The record tag of a kill's line in the ledger log ([`log_line`]).
pub const LOG_TAG: &str = "kill";

/// Reports kept, oldest evicted first **[A]**: about 300 bytes each, so
/// well under a megabyte of world file. Falsified by a human whose loss is
/// gone from the combat log within a week of bot fights, or by a world file
/// that grows past it.
pub const KEEP_REPORTS: usize = 2_000;

/// FILETIME ticks a second.
const TICKS_PER_SECOND: i64 = 10_000_000;

/// How long a hit keeps its shooter on the victim's next killmail **[A]**:
/// five minutes, a little over crimewatch's weapons timer of one. Falsified
/// by a retail killmail that names a pilot who last shot longer ago, or
/// leaves out one who shot more recently.
pub const TALLY_WINDOW: BlueTime = 5 * 60 * TICKS_PER_SECOND;

/// The `next` row's key.
const NEXT: &str = "next";

// ---- the tally -------------------------------------------------------------------

/// One shooter's part in the fight against one ship.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// The shooter as booked: the character, or the NPC's roster key. A
    /// drone's volleys are its owner's.
    pub who: i64,
    pub character: Option<i64>,
    pub corporation: Option<i64>,
    pub alliance: Option<i64>,
    pub ship_type: Option<i64>,
    /// The last weapon it fired [A]: the gun, the missile or the drone.
    pub weapon_type: Option<i64>,
    pub damage: f64,
    pub last: BlueTime,
}

/// Every player ship's recent attackers, by the ship's item id.
#[derive(Debug, Default)]
pub struct Tally {
    ships: BTreeMap<i64, Vec<Hit>>,
}

impl Tally {
    /// Book `hit` against `ship`, adding to the shooter's earlier hits, and
    /// forget every hit, on any ship, older than [`TALLY_WINDOW`] at
    /// `hit.last`.
    pub fn book(&mut self, ship: i64, hit: Hit) {
        let cutoff = hit.last.saturating_sub(TALLY_WINDOW);
        self.ships.retain(|_, hits| {
            hits.retain(|h| h.last >= cutoff);
            !hits.is_empty()
        });
        let hits = self.ships.entry(ship).or_default();
        match hits.iter_mut().find(|h| h.who == hit.who) {
            Some(h) => {
                h.damage += hit.damage;
                h.last = hit.last;
                h.ship_type = hit.ship_type.or(h.ship_type);
                h.weapon_type = hit.weapon_type.or(h.weapon_type);
            }
            None => hits.push(hit),
        }
    }

    /// The hits on `ship` still inside the window at `now`, taken out.
    pub fn take(&mut self, ship: i64, now: BlueTime) -> Vec<Hit> {
        let cutoff = now.saturating_sub(TALLY_WINDOW);
        let mut hits = self.ships.remove(&ship).unwrap_or_default();
        hits.retain(|h| h.last >= cutoff);
        hits
    }

    /// The ships with a booked hit.
    #[cfg(test)]
    pub fn ships(&self) -> usize {
        self.ships.len()
    }
}

/// The registration row in `world_ext::SLOTS` (coordinator-only).
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "killmail",
    make: |_| Box::new(Tally::default()),
};

/// Who a roster key is on a killmail, with its hull and the weapon a
/// volley of `weapon_type` names: a character, a drone (its owner, the drone
/// as the weapon), or an NPC (its corporation and type, no character).
fn shooter(w: &Registry, key: i64, weapon_type: Option<i64>, at: BlueTime) -> Option<Hit> {
    if let Some(d) = w.modules().drones().get(key) {
        let owner = w.character(d.owner_id)?;
        return Some(Hit {
            who: owner.character_id,
            character: Some(owner.character_id),
            corporation: Some(owner.corporation_id),
            alliance: owner.alliance_id,
            ship_type: Some(owner.ship_type_id),
            weapon_type: Some(d.kind.type_id),
            damage: 0.0,
            last: at,
        });
    }
    let p = w.pilot(key)?;
    Some(Hit {
        who: key,
        character: p.char_id,
        corporation: Some(p.corporation_id),
        alliance: p.alliance_id,
        ship_type: Some(p.ship_type_id),
        weapon_type,
        damage: 0.0,
        last: at,
    })
}

/// **The `combat::fire` hook**: one volley of `shot` by roster key
/// `shooter_key` at `victim`'s ship, dealing `dealt` (0 for a miss, which
/// still puts the shooter on the mail [A]). Only a character's ship is
/// tallied: an NPC's death has no killmail.
pub fn on_volley(
    w: &mut Registry,
    shot: &crate::damage::Shot,
    shooter_key: i64,
    victim: i64,
    dealt: f64,
) {
    // An abandoned ship is tallied under its hull id too (#132).
    if (w.character(victim).is_none() && !crate::pilotless::is_pilotless(w, shot.target_ship_id))
        || shooter_key == victim
    {
        return;
    }
    let Some(mut hit) = shooter(w, shooter_key, Some(shot.weapon_type_id), shot.at) else {
        return;
    };
    hit.damage = if dealt.is_finite() {
        dealt.max(0.0)
    } else {
        0.0
    };
    w.ext_mut::<Tally>().book(shot.target_ship_id, hit);
}

// ---- the report ------------------------------------------------------------------

/// One attacker on a report: the blob's `<a>`, or the `final*` fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Attacker {
    pub character: Option<i64>,
    pub corporation: Option<i64>,
    pub alliance: Option<i64>,
    pub faction: Option<i64>,
    pub ship_type: Option<i64>,
    pub weapon_type: Option<i64>,
    pub damage: i64,
    pub security: Option<f64>,
}

/// One item line: the blob's `<i>`. `contents` is one level deep, as the
/// client draws it; anything deeper is flattened into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemLine {
    pub type_id: i64,
    pub flag: i64,
    pub singleton: i64,
    pub dropped: i64,
    pub destroyed: i64,
    pub contents: Vec<ItemLine>,
}

/// One kill report.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub kill_id: i64,
    pub time: BlueTime,
    pub system: i64,
    pub victim: i64,
    pub victim_corporation: i64,
    pub victim_alliance: Option<i64>,
    pub ship_type: i64,
    /// Every attacker's damage summed, the final blow's included.
    pub damage_taken: i64,
    /// The hull and every item, at the trading index; `None` when nothing
    /// on the mail has a price.
    pub isk_lost: Option<f64>,
    /// The part of [`Self::isk_lost`] that was destroyed rather than dropped.
    pub isk_destroyed: Option<f64>,
    pub final_blow: Attacker,
    /// The others, most damage first. Never the final blow.
    pub attackers: Vec<Attacker>,
    pub items: Vec<ItemLine>,
    /// The ISK of a bounty on the victim that this kill claimed
    /// (`bountyClaimed`). Nothing here places a bounty on a pilot, so
    /// [`on_kill`] writes `None`; the field and its wire are ready for the
    /// day something does.
    pub bounty_claimed: Option<f64>,
    /// The holder of the kill right this kill used (`killRightSupplied`,
    /// "killed on behalf of"), set by [`on_kill`] [V].
    pub kill_right_supplied: Option<i64>,
}

/// The victim as the report names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Victim {
    pub character: i64,
    pub corporation: i64,
    pub alliance: Option<i64>,
    pub ship_type: i64,
}

/// **The report, pure.** `hits` is the tally; `final_hit` the shooter of
/// the killing volley (found in `hits` by `who`, and used as given when the
/// tally lost it); `security` a character's security status; `price` a
/// type's unit value. The final blow leaves the attacker list; the rest are
/// ordered by damage, then by who, so a report is the same however the
/// tally was ordered.
#[allow(clippy::too_many_arguments)]
pub fn build(
    kill_id: i64,
    time: BlueTime,
    system: i64,
    victim: Victim,
    hits: &[Hit],
    final_hit: Option<Hit>,
    items: Vec<ItemLine>,
    security: impl Fn(i64) -> Option<f64>,
    price: impl Fn(i64) -> Option<f64>,
) -> Report {
    let damage = |h: &Hit| h.damage.round() as i64;
    let attacker = |h: &Hit| Attacker {
        character: h.character,
        corporation: h.corporation,
        alliance: h.alliance,
        faction: None,
        ship_type: h.ship_type,
        weapon_type: h.weapon_type,
        damage: damage(h),
        security: h.character.and_then(&security),
    };
    let final_who = final_hit.map(|f| f.who);
    let final_blow = hits
        .iter()
        .find(|h| Some(h.who) == final_who)
        .or(final_hit.as_ref())
        .map(attacker)
        .unwrap_or(Attacker {
            character: None,
            corporation: None,
            alliance: None,
            faction: None,
            ship_type: None,
            weapon_type: None,
            damage: 0,
            security: None,
        });
    let mut rest: Vec<&Hit> = hits.iter().filter(|h| Some(h.who) != final_who).collect();
    rest.sort_by(|a, b| damage(b).cmp(&damage(a)).then(a.who.cmp(&b.who)));
    let in_hits = hits.iter().any(|h| Some(h.who) == final_who);
    let damage_taken =
        hits.iter().map(damage).sum::<i64>() + if in_hits { 0 } else { final_blow.damage };

    let (mut lost, mut destroyed, mut priced) = (0.0, 0.0, false);
    let mut value = |type_id: i64, dropped: i64, gone: i64| {
        if let Some(p) = price(type_id).filter(|p| p.is_finite() && *p > 0.0) {
            priced = true;
            lost += p * (dropped + gone) as f64;
            destroyed += p * gone as f64;
        }
    };
    value(victim.ship_type, 0, 1);
    for i in &items {
        value(i.type_id, i.dropped, i.destroyed);
        for c in &i.contents {
            value(c.type_id, c.dropped, c.destroyed);
        }
    }
    Report {
        kill_id,
        time,
        system,
        victim: victim.character,
        victim_corporation: victim.corporation,
        victim_alliance: victim.alliance,
        ship_type: victim.ship_type,
        damage_taken,
        isk_lost: priced.then_some(lost),
        isk_destroyed: priced.then_some(destroyed),
        final_blow,
        attackers: rest.into_iter().map(attacker).collect(),
        items,
        bounty_claimed: None,
        kill_right_supplied: None,
    }
}

/// What was inside `ship_id` (the pilot's capsule excepted), each top-level
/// item dropped or destroyed whole by `combat::survives` -- every item
/// destroyed when `wreck` is false, as the death plan does.
pub fn items_of(
    store: &crate::item::Store,
    ship_id: i64,
    capsule: Option<i64>,
    wreck: bool,
) -> Vec<ItemLine> {
    let line = |i: &crate::item::Item, drops: bool| {
        let n = i.quantity.max(1);
        ItemLine {
            type_id: i.type_id,
            flag: i.flag_id,
            singleton: i64::from(i.singleton),
            dropped: if drops { n } else { 0 },
            destroyed: if drops { 0 } else { n },
            contents: Vec::new(),
        }
    };
    store
        .contents_of(ship_id)
        .filter(|i| Some(i.item_id) != capsule)
        .map(|top| {
            let drops = wreck && crate::combat::survives(ship_id, top.item_id);
            let mut l = line(top, drops);
            let mut open = vec![top.item_id];
            while let Some(container) = open.pop() {
                for i in store.contents_of(container) {
                    l.contents.push(line(i, drops));
                    open.push(i.item_id);
                }
            }
            l
        })
        .collect()
}

/// **The `kill_hooks::PLAYER` row**: a character's ship or capsule died;
/// write its report and the ledger line. A drone's death writes nothing.
pub fn on_kill(w: &mut Registry, ev: &KillEvent) {
    let hits = w.ext_mut::<Tally>().take(ev.victim_ship, ev.when);
    let Some(c) = w.character(ev.victim_character).cloned() else {
        return;
    };
    let hull = w
        .store()
        .get(ev.victim_ship)
        .map_or(ev.ship_type, |s| s.type_id);
    let capsule = crate::fitting::capsule_of(w.store(), &c)
        .filter(|pod| {
            pod.location_id == ev.victim_ship && pod.flag_id == crate::fitting::FLAG_CAPSULE
        })
        .map(|pod| pod.item_id);
    let wreck = crate::damage::Death::for_type(hull).leaves_wreck()
        && crate::combat::wreck_type_in(w.content(), hull).is_some()
        && c.solar_system_id.is_some();
    file(w, ev, &c, hull, capsule, wreck, hits, true);
}

/// **An abandoned ship shot dead** (lane sb-km, issue #132; `docs/lanes/
/// be-3.md` option b): called by `combat::kill_pilotless` before the death
/// plan, as [`on_kill`] is. The victim is the hull's owner -- `ev.victim_
/// character`, the boarder's after a boarding -- wherever that character is,
/// online or not; the system is the hull's own (`ev.system`), never the
/// owner's location. No capsule is looked for: the hull's pilot is not
/// aboard. **[A]** a TQ killmail for an empty hull names its owner as the
/// victim; falsifier: a retail report of an abandoned ship with another
/// victim, or none. No kill right applies.
pub fn on_kill_pilotless(w: &mut Registry, ev: &KillEvent) {
    let hits = w.ext_mut::<Tally>().take(ev.victim_ship, ev.when);
    let Some(c) = w.character(ev.victim_character).cloned() else {
        return;
    };
    let hull = w
        .store()
        .get(ev.victim_ship)
        .map_or(ev.ship_type, |s| s.type_id);
    let wreck = crate::damage::Death::for_type(hull).leaves_wreck()
        && crate::combat::wreck_type_in(w.content(), hull).is_some();
    file(w, ev, &c, hull, None, wreck, hits, false);
}

/// The report both hooks write: items as the death plan will leave them,
/// the final blow, the kill right (a flown ship's only), the ledger line and
/// the kept row.
#[allow(clippy::too_many_arguments)]
fn file(
    w: &mut Registry,
    ev: &KillEvent,
    c: &crate::model::Character,
    hull: i64,
    capsule: Option<i64>,
    wreck: bool,
    hits: Vec<Hit>,
    flown: bool,
) {
    // A boarder who shot the empty hull is the victim of its mail, not an
    // attacker on it: on a flown ship's mail the victim is never one.
    let mut hits = hits;
    if !flown {
        hits.retain(|h| h.who != ev.victim_character);
    }
    let mut items = items_of(w.store(), ev.victim_ship, capsule, wreck);
    // A capsule's report lists the head's implants the podding destroys: the
    // revive that follows this hook takes them, and leaves a
    // `nonDestructible` one (`crate::implants::dies_with_the_pod`, the
    // client's own test). [I] the flag is the item's own `flagImplant`, as
    // for any other line.
    if hull == crate::damage::CAPSULE_TYPE_ID {
        items.extend(
            w.store()
                .contents_of(c.character_id)
                .filter(|i| i.flag_id == crate::dogma::FLAG_IMPLANT)
                .filter(|i| crate::implants::dies_with_the_pod(w.content(), i.type_id))
                .map(|i| ItemLine {
                    type_id: i.type_id,
                    flag: i.flag_id,
                    singleton: i64::from(i.singleton),
                    dropped: 0,
                    destroyed: i.quantity.max(1),
                    contents: Vec::new(),
                }),
        );
    }
    let final_hit = ev.killer.and_then(|k| shooter(w, k, None, ev.when));
    let kill_id = next_id(w);
    // C6B: a kill of the target's ship under an activated kill right is
    // "on behalf of" the right's holder -- the same test `crimewatch::
    // killright::on_kill` makes a hook later (this row runs first, so the
    // activation is still there).
    let kill_right_supplied = (flown && ev.ship_type != crate::damage::CAPSULE_TYPE_ID)
        .then(|| {
            w.combat()
                .crimewatch()
                .activation(ev.victim_character, ev.when)
        })
        .flatten()
        .and_then(|a| crate::crimewatch::killright::get(w, a.kill_right_id))
        .map(|kr| kr.from);
    let mut report = build(
        kill_id,
        ev.when,
        ev.system,
        Victim {
            character: c.character_id,
            corporation: c.corporation_id,
            alliance: c.alliance_id,
            ship_type: hull,
        },
        &hits,
        final_hit,
        items,
        |ch| Some(crate::npc::bounty::security_status(w, ch)),
        |t| crate::econ_prices::average(w, t),
    );
    report.kill_right_supplied = kill_right_supplied;
    w.journal().log().append(report.time, log_line(&report));
    put(w, &report);
}

// ---- persistence -----------------------------------------------------------------

/// The next kill id, counted on: the `next` row, or one past the highest
/// report kept.
fn next_id(w: &mut Registry) -> i64 {
    let rows = w.deltas().of(KIND);
    let stored = rows
        .get(&vec![NEXT.to_owned()])
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok());
    let id = stored
        .unwrap_or(0)
        .max(ids(w).last().map_or(0, |l| l + 1))
        .max(1);
    w.deltas_mut()
        .put(KIND, vec![NEXT.to_owned()], vec![(id + 1).to_string()]);
    id
}

/// Every kept report's id, ascending.
fn ids(w: &Registry) -> Vec<i64> {
    let mut v: Vec<i64> = w
        .deltas()
        .of(KIND)
        .keys()
        .filter_map(|k| k.first()?.parse().ok())
        .collect();
    v.sort_unstable();
    v
}

fn index_key(character: i64, kill_id: i64) -> Vec<String> {
    vec![character.to_string(), format!("{kill_id:010}")]
}

fn corp_key(losses: bool, corporation: i64, kill_id: i64) -> Vec<String> {
    vec![
        format!("{}{corporation}", if losses { 'l' } else { 'k' }),
        format!("{kill_id:010}"),
    ]
}

fn index_ready(w: &Registry) -> bool {
    w.deltas()
        .of(KIND_BY_CHAR)
        .contains_key(&vec![INDEX_READY.to_owned()])
}

fn corp_index_ready(w: &Registry) -> bool {
    w.deltas()
        .of(KIND_BY_CHAR)
        .contains_key(&vec![CORP_INDEX_READY.to_owned()])
}

/// Every character a report names: the victim, the final blow, the attackers.
fn characters_of(r: &Report) -> Vec<i64> {
    let mut v = vec![r.victim];
    v.extend(r.final_blow.character);
    v.extend(r.attackers.iter().filter_map(|a| a.character));
    v.sort_unstable();
    v.dedup();
    v
}

/// Every corporation that fired on the victim: the final blow's and the
/// attackers'.
fn shooting_corps_of(r: &Report) -> Vec<i64> {
    let mut v: Vec<i64> = r.final_blow.corporation.into_iter().collect();
    v.extend(r.attackers.iter().filter_map(|a| a.corporation));
    v.sort_unstable();
    v.dedup();
    v
}

/// Every index key a report owns.
fn keys_of(r: &Report) -> Vec<Vec<String>> {
    let mut keys: Vec<Vec<String>> = characters_of(r)
        .into_iter()
        .map(|c| index_key(c, r.kill_id))
        .collect();
    keys.extend(
        shooting_corps_of(r)
            .into_iter()
            .map(|c| corp_key(false, c, r.kill_id)),
    );
    keys.push(corp_key(true, r.victim_corporation, r.kill_id));
    keys
}

/// Keep `r`, evicting the oldest past [`KEEP_REPORTS`].
pub fn put(w: &mut Registry, r: &Report) {
    if !index_ready(w) || !corp_index_ready(w) {
        // A world from before the index (or before C6B's corporation rows):
        // build it from what is kept.
        for id in ids(w) {
            if let Some(old) = get(w, id) {
                for k in keys_of(&old) {
                    w.deltas_mut().put(KIND_BY_CHAR, k, Vec::new());
                }
            }
        }
        for marker in [INDEX_READY, CORP_INDEX_READY] {
            w.deltas_mut()
                .put(KIND_BY_CHAR, vec![marker.to_owned()], Vec::new());
        }
    }
    w.deltas_mut()
        .put(KIND, vec![r.kill_id.to_string()], r.to_row());
    for k in keys_of(r) {
        w.deltas_mut().put(KIND_BY_CHAR, k, Vec::new());
    }
    let ids = ids(w);
    if ids.len() > KEEP_REPORTS {
        for id in &ids[..ids.len() - KEEP_REPORTS] {
            if let Some(old) = get(w, *id) {
                for k in keys_of(&old) {
                    w.deltas_mut().del(KIND_BY_CHAR, &k);
                }
            }
            w.deltas_mut().del(KIND, &[id.to_string()]);
        }
    }
}

/// One kept report.
pub fn get(w: &Registry, kill_id: i64) -> Option<Report> {
    let v = w.deltas().of(KIND).get(&vec![kill_id.to_string()])?;
    Report::from_row(kill_id, v)
}

/// The reports `character` is on (victim, final blow or attacker), newest
/// first, those with an id below `before` when it is given, at most `n`.
///
/// Through [`KIND_BY_CHAR`]: only this character's reports are read. A world
/// from before the index is scanned instead, once, until the next [`put`].
pub fn involving(w: &Registry, character: i64, before: Option<i64>, n: usize) -> Vec<Report> {
    if !index_ready(w) {
        let mut out: Vec<Report> = w
            .deltas()
            .of(KIND)
            .iter()
            .filter_map(|(k, v)| Report::from_row(k.first()?.parse().ok()?, v))
            .filter(|r| before.is_none_or(|b| r.kill_id < b))
            .filter(|r| r.involves(character))
            .collect();
        out.sort_by(|a, b| b.kill_id.cmp(&a.kill_id));
        out.truncate(n);
        return out;
    }
    let who = character.to_string();
    let lo = vec![who.clone(), String::new()];
    let hi = vec![who, "~".to_owned()];
    w.deltas()
        .of(KIND_BY_CHAR)
        .range(lo..=hi)
        .rev()
        .filter_map(|(k, _)| k.get(1)?.parse::<i64>().ok())
        .filter(|id| before.is_none_or(|b| *id < b))
        .filter_map(|id| get(w, id))
        .take(n)
        .collect()
}

/// **A corporation's kill list** (C6B, `corpRegistry.GetRecentKills` and
/// `GetRecentLosses`): the reports in which a corporation fired on the victim
/// (`losses` false) or was the victim's (`losses` true), newest first, those
/// with an id below `before` when it is given, at most `n`. Through the
/// index; a world without the corporation rows is scanned until the next
/// [`put`].
pub fn corp_involving(
    w: &Registry,
    corporation: i64,
    losses: bool,
    before: Option<i64>,
    n: usize,
) -> Vec<Report> {
    if !corp_index_ready(w) {
        let mut out: Vec<Report> = w
            .deltas()
            .of(KIND)
            .iter()
            .filter_map(|(k, v)| Report::from_row(k.first()?.parse().ok()?, v))
            .filter(|r| before.is_none_or(|b| r.kill_id < b))
            .filter(|r| {
                if losses {
                    r.victim_corporation == corporation
                } else {
                    shooting_corps_of(r).contains(&corporation)
                }
            })
            .collect();
        out.sort_by(|a, b| b.kill_id.cmp(&a.kill_id));
        out.truncate(n);
        return out;
    }
    let who = corp_key(losses, corporation, 0).swap_remove(0);
    let lo = vec![who.clone(), String::new()];
    let hi = vec![who, "~".to_owned()];
    w.deltas()
        .of(KIND_BY_CHAR)
        .range(lo..=hi)
        .rev()
        .filter_map(|(k, _)| k.get(1)?.parse::<i64>().ok())
        .filter(|id| before.is_none_or(|b| *id < b))
        .filter_map(|id| get(w, id))
        .take(n)
        .collect()
}

/// A kill's line in the ledger log: tag, kill id, time, system, victim,
/// hull, final blow's character (empty for an NPC) and corporation, ISK
/// lost and destroyed (empty when unpriced), damage, attackers counted with
/// the final blow.
pub fn log_line(r: &Report) -> String {
    let o = |v: Option<i64>| v.map_or(String::new(), |n| n.to_string());
    let f = |v: Option<f64>| v.map_or(String::new(), |n| format!("{n:.2}"));
    format!(
        "{LOG_TAG}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        r.kill_id,
        r.time,
        r.system,
        r.victim,
        r.ship_type,
        o(r.final_blow.character),
        o(r.final_blow.corporation),
        f(r.isk_lost),
        f(r.isk_destroyed),
        r.damage_taken,
        r.attackers.len() + 1
    )
}

fn opt<T: std::str::FromStr>(s: &str) -> Option<T> {
    if s.is_empty() { None } else { s.parse().ok() }
}

fn show<T: ToString>(v: Option<T>) -> String {
    v.map_or(String::new(), |n| n.to_string())
}

impl Attacker {
    /// `c,r,a,f,s,w,d,t`, an absent field empty.
    fn encode(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{}",
            show(self.character),
            show(self.corporation),
            show(self.alliance),
            show(self.faction),
            show(self.ship_type),
            show(self.weapon_type),
            self.damage,
            show(self.security)
        )
    }

    fn decode(s: &str) -> Option<Attacker> {
        let f: Vec<&str> = s.split(',').collect();
        let [c, r, a, fa, sh, we, d, t] = f.as_slice() else {
            return None;
        };
        Some(Attacker {
            character: opt(c),
            corporation: opt(r),
            alliance: opt(a),
            faction: opt(fa),
            ship_type: opt(sh),
            weapon_type: opt(we),
            damage: d.parse().ok()?,
            security: opt(t),
        })
    }

    /// The blob's `t`: one decimal, `0.0` when unknown (the client's own
    /// default).
    fn security_text(&self) -> String {
        format!("{:.1}", self.security.unwrap_or(0.0))
    }
}

impl ItemLine {
    fn encode_one(&self) -> String {
        format!(
            "{},{},{},{},{}",
            self.type_id, self.flag, self.singleton, self.dropped, self.destroyed
        )
    }

    fn decode_one(s: &str) -> Option<ItemLine> {
        let f: Vec<i64> = s
            .split(',')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        let [t, fl, s, d, x] = f.as_slice() else {
            return None;
        };
        Some(ItemLine {
            type_id: *t,
            flag: *fl,
            singleton: *s,
            dropped: *d,
            destroyed: *x,
            contents: Vec::new(),
        })
    }

    /// The line, then its contents, `/`-separated.
    fn encode(&self) -> String {
        std::iter::once(self.encode_one())
            .chain(self.contents.iter().map(ItemLine::encode_one))
            .collect::<Vec<_>>()
            .join("/")
    }

    fn decode(s: &str) -> Option<ItemLine> {
        let mut parts = s.split('/');
        let mut top = ItemLine::decode_one(parts.next()?)?;
        for p in parts {
            top.contents.push(ItemLine::decode_one(p)?);
        }
        Some(top)
    }
}

/// `;`-joined, empty for none.
fn join<T>(v: &[T], f: impl Fn(&T) -> String) -> String {
    v.iter().map(f).collect::<Vec<_>>().join(";")
}

fn split<T>(s: &str, f: impl Fn(&str) -> Option<T>) -> Option<Vec<T>> {
    if s.is_empty() {
        return Some(Vec::new());
    }
    s.split(';').map(f).collect()
}

impl Report {
    /// Whether `character` is the victim, the final blow or an attacker.
    pub fn involves(&self, character: i64) -> bool {
        self.victim == character
            || self.final_blow.character == Some(character)
            || self
                .attackers
                .iter()
                .any(|a| a.character == Some(character))
    }

    /// The delta row's value: time, system, victim, corporation, alliance,
    /// hull, damage, ISK lost, ISK destroyed, the final blow, the attackers,
    /// the items, the bounty claimed, the kill right's holder. A row from
    /// before C6B has twelve fields and reads with both absent.
    pub fn to_row(&self) -> Vec<String> {
        vec![
            self.time.to_string(),
            self.system.to_string(),
            self.victim.to_string(),
            self.victim_corporation.to_string(),
            show(self.victim_alliance),
            self.ship_type.to_string(),
            self.damage_taken.to_string(),
            show(self.isk_lost),
            show(self.isk_destroyed),
            self.final_blow.encode(),
            join(&self.attackers, Attacker::encode),
            join(&self.items, ItemLine::encode),
            show(self.bounty_claimed),
            show(self.kill_right_supplied),
        ]
    }

    /// The inverse of [`Self::to_row`]; `None` for a row it cannot read.
    pub fn from_row(kill_id: i64, v: &[String]) -> Option<Report> {
        let [
            t,
            sys,
            vic,
            corp,
            all,
            ship,
            dmg,
            lost,
            gone,
            fin,
            att,
            items,
            extra @ ..,
        ] = v
        else {
            return None;
        };
        let (bounty, right) = match extra {
            [] => (None, None),
            [b, r] => (opt(b), opt(r)),
            _ => return None,
        };
        Some(Report {
            kill_id,
            time: t.parse().ok()?,
            system: sys.parse().ok()?,
            victim: vic.parse().ok()?,
            victim_corporation: corp.parse().ok()?,
            victim_alliance: opt(all),
            ship_type: ship.parse().ok()?,
            damage_taken: dmg.parse().ok()?,
            isk_lost: opt(lost),
            isk_destroyed: opt(gone),
            final_blow: Attacker::decode(fin)?,
            attackers: split(att, Attacker::decode)?,
            items: split(items, ItemLine::decode)?,
            bounty_claimed: bounty,
            kill_right_supplied: right,
        })
    }

    /// **`killBlob`**: the XML `GetAttackersAndItemsFromKillMail` parses.
    /// Every value quoted (the client's own regex quotes bare numbers, and
    /// leaves quoted ones alone); an absent id left out, which the parser
    /// reads as `None`.
    pub fn blob(&self) -> String {
        let mut s = String::from("<attackers>");
        for a in &self.attackers {
            s.push_str("<a");
            for (k, v) in [
                ("c", a.character),
                ("r", a.corporation),
                ("a", a.alliance),
                ("f", a.faction),
                ("s", a.ship_type),
                ("w", a.weapon_type),
            ] {
                if let Some(v) = v {
                    s.push_str(&format!(" {k}=\"{v}\""));
                }
            }
            s.push_str(&format!(
                " d=\"{}\" t=\"{}\" />",
                a.damage.max(0),
                a.security_text()
            ));
        }
        s.push_str("</attackers><items>");
        fn item(s: &mut String, i: &ItemLine) {
            s.push_str(&format!(
                "<i t=\"{}\" f=\"{}\" s=\"{}\" d=\"{}\" x=\"{}\">",
                i.type_id, i.flag, i.singleton, i.dropped, i.destroyed
            ));
            for c in &i.contents {
                item(s, c);
            }
            s.push_str("</i>");
        }
        for i in &self.items {
            item(&mut s, i);
        }
        s.push_str("</items>");
        s
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const ALICE: i64 = 90_000_101;
    const BOB: i64 = 90_000_102;
    const CAROL: i64 = 90_000_103;
    const CORP: i64 = 98_000_001;
    const HULL: i64 = 990_600_001;
    const GUN: i64 = 990_600_002;
    const DRONE: i64 = 990_600_003;
    const MODULE: i64 = 990_600_010;
    const AMMO: i64 = 990_600_011;
    const T0: BlueTime = 134_116_992_000_000_000;
    /// An invented capsule type: `items_of` is told which item is the pod.
    const POD: i64 = 990_600_020;

    fn hit(who: i64, damage: f64, last: BlueTime) -> Hit {
        Hit {
            who,
            character: Some(who),
            corporation: Some(CORP),
            alliance: None,
            ship_type: Some(HULL),
            weapon_type: Some(GUN),
            damage,
            last,
        }
    }

    fn victim() -> Victim {
        Victim {
            character: ALICE,
            corporation: CORP,
            alliance: None,
            ship_type: HULL,
        }
    }

    fn line(type_id: i64, dropped: i64, destroyed: i64) -> ItemLine {
        ItemLine {
            type_id,
            flag: 27,
            singleton: 1,
            dropped,
            destroyed,
            contents: Vec::new(),
        }
    }

    /// The final blow leaves the attacker list; the rest go most damage
    /// first; the damage taken is everyone's; the security is asked for
    /// characters only.
    #[test]
    fn the_final_blow_is_split_out_and_the_rest_ordered_by_damage() {
        let hits = [hit(BOB, 100.0, T0), hit(CAROL, 300.0, T0), {
            let mut npc = hit(7_000_000_001, 50.0, T0);
            npc.character = None;
            npc
        }];
        let r = build(
            1,
            T0,
            30_000_001,
            victim(),
            &hits,
            Some(hit(BOB, 0.0, T0)),
            Vec::new(),
            |c| Some(if c == BOB { -2.5 } else { 1.0 }),
            |_| None,
        );
        assert_eq!(r.final_blow.character, Some(BOB));
        assert_eq!(r.final_blow.damage, 100);
        assert_eq!(r.final_blow.security, Some(-2.5));
        let order: Vec<_> = r.attackers.iter().map(|a| a.damage).collect();
        assert_eq!(order, vec![300, 50]);
        assert_eq!(r.attackers[1].security, None, "an NPC has none");
        assert_eq!(r.damage_taken, 450);
        assert_eq!((r.isk_lost, r.isk_destroyed), (None, None));
        assert!(r.involves(ALICE) && r.involves(BOB) && r.involves(CAROL));
        assert!(!r.involves(90_000_999));
    }

    /// A final blower the tally lost is named all the same, with its
    /// damage counted once.
    #[test]
    fn a_final_blow_missing_from_the_tally_is_still_named() {
        let r = build(
            1,
            T0,
            1,
            victim(),
            &[hit(CAROL, 10.0, T0)],
            Some(hit(BOB, 5.0, T0)),
            Vec::new(),
            |_| None,
            |_| None,
        );
        assert_eq!(r.final_blow.character, Some(BOB));
        assert_eq!(r.attackers.len(), 1);
        assert_eq!(r.damage_taken, 15);
    }

    /// The value is the hull and every item at its price; destroyed is the
    /// part not dropped.
    #[test]
    fn the_isk_is_hull_and_items_and_destroyed_is_the_unrecovered_part() {
        let mut box_ = line(MODULE, 0, 1);
        box_.contents.push(line(AMMO, 0, 10));
        let items = vec![line(MODULE, 2, 0), box_];
        let price = |t: i64| match t {
            HULL => Some(1_000.0),
            MODULE => Some(100.0),
            AMMO => Some(1.0),
            _ => None,
        };
        let r = build(1, T0, 1, victim(), &[], None, items, |_| None, price);
        assert_eq!(r.isk_lost, Some(1_000.0 + 200.0 + 100.0 + 10.0));
        assert_eq!(r.isk_destroyed, Some(1_000.0 + 100.0 + 10.0));
    }

    /// A report survives its delta row, attackers, contents and absent
    /// fields included; a short row is refused rather than misread.
    #[test]
    fn a_report_round_trips_through_its_row() {
        let mut nested = line(MODULE, 1, 0);
        nested.contents.push(line(AMMO, 5, 0));
        let mut npc = hit(7_000_000_001, 50.0, T0);
        npc.character = None;
        npc.weapon_type = None;
        let r = build(
            42,
            T0,
            30_000_001,
            Victim {
                alliance: Some(99_000_001),
                ..victim()
            },
            &[hit(BOB, 120.4, T0), npc],
            Some(hit(BOB, 0.0, T0)),
            vec![nested, line(GUN, 0, 1)],
            |_| Some(0.5),
            |_| Some(3.0),
        );
        let back = Report::from_row(42, &r.to_row()).expect("reads back");
        assert_eq!(back, r);
        assert!(Report::from_row(1, &r.to_row()[..5]).is_none());
    }

    /// The blob is what the client's parser takes: the final blow not in
    /// it, absent ids left out, contents nested one level.
    #[test]
    fn the_blob_holds_the_other_attackers_and_the_items() {
        let mut npc = hit(7_000_000_001, 50.0, T0);
        npc.character = None;
        let mut cargo = line(MODULE, 0, 1);
        cargo.contents.push(line(AMMO, 0, 3));
        let r = build(
            1,
            T0,
            1,
            victim(),
            &[hit(BOB, 10.0, T0), npc],
            Some(hit(BOB, 0.0, T0)),
            vec![cargo],
            |_| None,
            |_| None,
        );
        assert_eq!(
            r.blob(),
            format!(
                "<attackers><a r=\"{CORP}\" s=\"{HULL}\" w=\"{GUN}\" d=\"50\" t=\"0.0\" /></attackers>\
                 <items><i t=\"{MODULE}\" f=\"27\" s=\"1\" d=\"0\" x=\"1\">\
                 <i t=\"{AMMO}\" f=\"27\" s=\"1\" d=\"0\" x=\"3\"></i></i></items>"
            )
        );
    }

    /// The tally adds a shooter's hits, keeps the last weapon, and forgets
    /// a hit older than the window.
    #[test]
    fn the_tally_sums_per_shooter_and_forgets_old_hits() {
        let mut t = Tally::default();
        t.book(1, hit(BOB, 10.0, T0));
        let mut drone = hit(BOB, 5.0, T0 + 1);
        drone.weapon_type = Some(DRONE);
        t.book(1, drone);
        t.book(2, hit(CAROL, 1.0, T0));
        let hits = t.take(1, T0 + 2);
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].damage, hits[0].weapon_type), (15.0, Some(DRONE)));
        // Ship 2's hit ages out of the window when ship 3 is hit later.
        t.book(3, hit(BOB, 1.0, T0 + TALLY_WINDOW + 1));
        assert_eq!(t.ships(), 1);
        assert!(t.take(3, T0 + 3 * TALLY_WINDOW).is_empty(), "too old");
    }

    /// A ledger line per kill, tagged, its fields in order.
    #[test]
    fn the_log_line_names_the_kill() {
        let r = build(
            9,
            T0,
            30_000_001,
            victim(),
            &[hit(BOB, 10.0, T0)],
            Some(hit(BOB, 0.0, T0)),
            Vec::new(),
            |_| None,
            |_| Some(2.0),
        );
        assert_eq!(
            log_line(&r),
            format!("kill\t9\t{T0}\t30000001\t{ALICE}\t{HULL}\t{BOB}\t{CORP}\t2.00\t2.00\t10\t1\n")
        );
    }

    /// Items: the capsule left out, a top-level item and everything under
    /// it share one fate, and with no wreck everything is destroyed.
    #[test]
    fn items_take_the_death_plans_fate() {
        use crate::item::Item;
        const SHIP: i64 = 100_000_500;
        let item = |id: i64, loc: i64, type_id: i64, q: i64| Item {
            item_id: id,
            type_id,
            owner_id: ALICE,
            location_id: loc,
            flag_id: 5,
            quantity: q,
            group_id: 1,
            category_id: 7,
            singleton: q == 1,
        };
        let mut reg = crate::testkit::registry();
        reg.apply_snapshot(crate::store::Snapshot {
            items: vec![
                item(SHIP, 60_000_001, HULL, 1),
                item(100_000_501, SHIP, POD, 1),
                item(100_000_502, SHIP, MODULE, 1),
                item(100_000_503, 100_000_502, AMMO, 40),
                item(100_000_504, SHIP, AMMO, 7),
            ],
            next_item_id: 100_000_505,
            ..crate::store::Snapshot::default()
        });
        let with = items_of(reg.store(), SHIP, Some(100_000_501), true);
        assert_eq!(with.len(), 2, "the capsule is not on the mail");
        let boxed = with.iter().find(|l| l.type_id == MODULE).expect("module");
        assert_eq!(boxed.contents.len(), 1);
        let survives = crate::combat::survives(SHIP, 100_000_502);
        assert_eq!(boxed.dropped > 0, survives);
        assert_eq!(boxed.contents[0].dropped > 0, survives);
        assert_eq!(boxed.contents[0].dropped + boxed.contents[0].destroyed, 40);
        let without = items_of(reg.store(), SHIP, Some(100_000_501), false);
        assert!(without.iter().all(|l| l.dropped == 0 && l.destroyed > 0));
    }

    /// Kept reports are capped, the oldest evicted; ids keep counting.
    #[test]
    fn the_store_keeps_the_newest_and_counts_on() {
        let mut reg = crate::testkit::registry();
        let mut last = 0;
        for _ in 0..3 {
            let id = next_id(&mut reg);
            assert!(id > last);
            last = id;
            let r = build(
                id,
                T0 + id,
                1,
                victim(),
                &[],
                None,
                Vec::new(),
                |_| None,
                |_| None,
            );
            put(&mut reg, &r);
        }
        assert_eq!(get(&reg, last).map(|r| r.kill_id), Some(last));
        let mine = involving(&reg, ALICE, None, 2);
        assert_eq!(
            mine.iter().map(|r| r.kill_id).collect::<Vec<_>>(),
            vec![last, last - 1]
        );
        let older = involving(&reg, ALICE, Some(last), 10);
        assert_eq!(older.len(), 2);
        assert!(involving(&reg, BOB, None, 10).is_empty());
    }

    /// SEC-FIX 10: a character's reports come from the per-character index
    /// (the index is how the combat log stops parsing every kept row), it
    /// follows the eviction, and a world from before it is read by scan and
    /// then indexed by the next report.
    #[test]
    fn the_per_character_index_follows_put_eviction_and_old_worlds() {
        let mut reg = crate::testkit::registry();
        let mk = |reg: &mut Registry, who: i64| {
            let id = next_id(reg);
            let mut v = victim();
            v.character = who;
            let r = build(id, T0 + id, 1, v, &[], None, Vec::new(), |_| None, |_| None);
            put(reg, &r);
            id
        };
        let a = mk(&mut reg, ALICE);
        let b = mk(&mut reg, BOB);
        let keys: Vec<Vec<String>> = reg.deltas().of(KIND_BY_CHAR).keys().cloned().collect();
        assert!(keys.contains(&index_key(ALICE, a)) && keys.contains(&index_key(BOB, b)));
        assert_eq!(involving(&reg, BOB, None, 10).len(), 1);
        // An old world: reports, no index.
        let mut old = crate::testkit::registry();
        for who in [ALICE, BOB, ALICE] {
            let id = next_id(&mut old);
            let mut v = victim();
            v.character = who;
            let r = build(id, T0 + id, 1, v, &[], None, Vec::new(), |_| None, |_| None);
            old.deltas_mut().put(KIND, vec![id.to_string()], r.to_row());
        }
        assert!(!index_ready(&old));
        assert_eq!(involving(&old, ALICE, None, 10).len(), 2, "by scan");
        mk(&mut old, BOB);
        assert!(index_ready(&old));
        assert_eq!(involving(&old, ALICE, None, 10).len(), 2, "now by index");
        assert_eq!(involving(&old, BOB, None, 10).len(), 2);
        // Eviction drops the index rows with the report.
        for _ in 0..KEEP_REPORTS {
            mk(&mut old, BOB);
        }
        assert!(involving(&old, ALICE, None, 10).is_empty());
        assert!(
            !old.deltas()
                .of(KIND_BY_CHAR)
                .keys()
                .any(|k| k.first().map(String::as_str) == Some(&ALICE.to_string())),
            "no index row outlives its report"
        );
    }

    /// C6B: the bounty and the kill right's holder survive the row; a row
    /// from before them (twelve fields) reads with both absent; a row with
    /// one stray extra field is refused.
    #[test]
    fn the_c6b_fields_round_trip_and_an_old_row_still_reads() {
        let mut r = build(
            5,
            T0,
            1,
            victim(),
            &[hit(BOB, 10.0, T0)],
            Some(hit(BOB, 0.0, T0)),
            Vec::new(),
            |_| None,
            |_| None,
        );
        r.bounty_claimed = Some(1234.5);
        r.kill_right_supplied = Some(CAROL);
        assert_eq!(Report::from_row(5, &r.to_row()).as_ref(), Some(&r));
        let old = r.to_row()[..12].to_vec();
        let read = Report::from_row(5, &old).expect("an old row reads");
        assert_eq!(
            (read.bounty_claimed, read.kill_right_supplied),
            (None, None)
        );
        let mut stray = r.to_row();
        stray.push(String::new());
        assert!(Report::from_row(5, &stray).is_none());
    }

    /// C6B: a corporation's kills are the reports it fired in, its losses
    /// the ones it was the victim of, newest first and paged by id; a world
    /// with the character index but not the corporation rows is scanned and
    /// then indexed; eviction takes the corporation rows too.
    #[test]
    fn a_corporations_kills_and_losses_come_from_its_index() {
        const THEIRS: i64 = 98_000_002;
        let mut reg = crate::testkit::registry();
        let mk = |reg: &mut Registry, shooter_corp: i64, victim_corp: i64| {
            let id = next_id(reg);
            let mut shooter = hit(BOB, 10.0, T0);
            shooter.corporation = Some(shooter_corp);
            let mut v = victim();
            v.corporation = victim_corp;
            let r = build(
                id,
                T0 + id,
                1,
                v,
                &[shooter],
                Some(shooter),
                Vec::new(),
                |_| None,
                |_| None,
            );
            put(reg, &r);
            id
        };
        let a = mk(&mut reg, CORP, THEIRS);
        let b = mk(&mut reg, THEIRS, CORP);
        let c = mk(&mut reg, CORP, THEIRS);
        let ids = |v: Vec<Report>| v.iter().map(|r| r.kill_id).collect::<Vec<_>>();
        assert_eq!(ids(corp_involving(&reg, CORP, false, None, 10)), vec![c, a]);
        assert_eq!(ids(corp_involving(&reg, CORP, true, None, 10)), vec![b]);
        assert_eq!(ids(corp_involving(&reg, THEIRS, false, None, 10)), vec![b]);
        assert_eq!(ids(corp_involving(&reg, CORP, false, Some(c), 10)), vec![a]);
        assert_eq!(ids(corp_involving(&reg, CORP, false, None, 1)), vec![c]);
        assert!(corp_involving(&reg, 98_000_999, false, None, 10).is_empty());

        // A world written after SEC-FIX 10 and before C6B: the character
        // index is ready, the corporation rows are not.
        let mut old = crate::testkit::registry();
        let id = next_id(&mut old);
        let r = build(
            id,
            T0 + id,
            1,
            victim(),
            &[hit(BOB, 10.0, T0)],
            Some(hit(BOB, 0.0, T0)),
            Vec::new(),
            |_| None,
            |_| None,
        );
        old.deltas_mut().put(KIND, vec![id.to_string()], r.to_row());
        old.deltas_mut()
            .put(KIND_BY_CHAR, vec![INDEX_READY.to_owned()], Vec::new());
        assert!(!corp_index_ready(&old));
        assert_eq!(ids(corp_involving(&old, CORP, false, None, 10)), vec![id]);
        mk(&mut old, CORP, THEIRS);
        assert!(corp_index_ready(&old));
        assert_eq!(ids(corp_involving(&old, CORP, true, None, 10)), vec![id]);

        // Eviction leaves no corporation row behind.
        for _ in 0..KEEP_REPORTS {
            mk(&mut old, THEIRS, THEIRS);
        }
        assert!(corp_involving(&old, CORP, true, None, 10).is_empty());
        assert!(
            !old.deltas().of(KIND_BY_CHAR).keys().any(|k| {
                k.first()
                    .is_some_and(|f| f.starts_with('k') || f.starts_with('l'))
                    && k.first()
                        .map(String::as_str)
                        .is_some_and(|f| f[1..] == *CORP.to_string())
            }),
            "no corporation row outlives its report"
        );
    }

    /// C6B: a kill's ledger line is pruned by the ledger's own rule: a kill
    /// with a human on it keeps the human window, a bots-only kill goes with
    /// the bots' day, and a line it cannot read is kept.
    #[test]
    fn a_kill_line_is_pruned_like_the_rows_around_it() {
        use crate::ledger::{BOT_LEDGER_DAYS, Kind, Rules, meta};
        const DAY: i64 = 864_000_000_000;
        let r = build(
            9,
            T0,
            1,
            victim(),
            &[hit(BOB, 10.0, T0)],
            Some(hit(BOB, 0.0, T0)),
            Vec::new(),
            |_| None,
            |_| None,
        );
        let m = meta(&log_line(&r)).expect("a kill line has a meta");
        assert_eq!((m.kind, m.id, m.date), (Kind::Kill, 9, T0));
        assert_eq!(m.owners, vec![ALICE, BOB]);
        let rules = |now: i64, humans: &[i64]| Rules {
            now,
            humans: humans.iter().copied().collect(),
            cursor: 0,
            market_from: 0,
        };
        let later = T0 + (BOT_LEDGER_DAYS + 2) * DAY;
        assert!(rules(later, &[]).drops(&m), "bots only: gone after the day");
        assert!(!rules(T0, &[]).drops(&m), "bots only, same day: kept");
        assert!(
            !rules(later, &[ALICE]).drops(&m),
            "a human on it: the human window"
        );
        assert!(meta("kill\tx\ty").is_none(), "a torn line is kept");
    }
}
