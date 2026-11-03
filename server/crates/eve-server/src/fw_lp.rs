//! **Loyalty points for Faction Warfare complexes** (packet FU-FWLP, lane
//! note `docs/lanes/fu-fwlp.md`).
//!
//! CP53b (`facwar.rs`) moves a system's victory points when an FW complex
//! is completed, but paid nobody. This module pays the pilots who completed
//! it, in militia LP, through the LP wallet missions already use
//! (`loyalty::adjust`: the `lp` delta, `OnLPChange` to the pilot), so the
//! client's LP window and the militia's LP store show the points.
//!
//! - **When** ([`on_complete`]): at the C3 completion itself, from
//!   `site_spawn`'s completion branch (one coordinator line beside
//!   `dungeon_tracking::complete`). VP are counted much later, at the
//!   despawn ([`crate::facwar::tick`]); by then the pilots have left, so the
//!   LP cannot wait for it.
//! - **Who**: every character on the complex's grid ([`on_grid`]: within
//!   `ARRIVAL_RANGE` of the anchor, in warp or not) whose militia ([`crate::service::facwar_mgr::my_faction`])
//!   is the side the complex works for: the system's attacker when the
//!   occupier owns the complex (a **capture**), the occupier when the
//!   attacker owns it (a **defence**). This is CP53b's reading of R-FW §6
//!   [I]: the complex belongs to the militia its dungeon names, and
//!   completing it works against that owner. A pilot of the owning militia,
//!   an outsider or an unenlisted pilot earns nothing.
//! - **How much** ([`payout`], pure): [`LP_BY_ARCHETYPE`] **[A]** by the
//!   complex's size. A capture pays it in full. A defence pays it scaled by
//!   how contested the system is (`score / VP_THRESHOLD`) **[A]**, so
//!   defending an uncontested system pays nothing (EvEJS's FW model pays a
//!   defence of a zero-score system nothing too, a fact noted, not code
//!   taken). The pool is `base x min(n, SHARE_CAP)`, split evenly among the
//!   `n` eligible pilots **[A]** ([`SHARE_CAP`]).
//! - **Adjacency** ([`lp_multiplier`]): the payout is scaled by the system's
//!   line, `ADJACENCY_STATE_LP_MULTIPLIER` [V] (front 1.5, second 1.0, back
//!   0.01), so a backline system pays one hundredth.
//! - **Kills** ([`on_kill`], #133): a militia pilot who destroys an enemy
//!   militia pilot's ship earns LP sized by the ISK destroyed, see
//!   [`kill_lp`]. Kills never move VP.
//! - **To whom**: the pilot's militia corporation
//!   (`npc/factions.tsv` `militia_corporation_id`, CP53a's `Index`). A
//!   militia with no corporation in the content pays nothing.
//! - **The ledger**: one `fwlp` line per pilot paid in the ledger log
//!   ([`log_line`]). The `[econ]` classes are ISK journal refs
//!   (`econ_ledger::REFS`); LP is not ISK, so it has no class there.
//!
//! **Not persisted beyond the wallet.** A completion is paid once because
//! `site_spawn` marks its pocket complete once; a restart re-instantiates a
//! live site with fresh squads, which is a new completion.

use crate::damage::BlueTime;
use crate::facwar::Adjacency;
use crate::service::Registry;
use crate::service::facwar_mgr;
use eve_marshal::PyValue;

/// **[A] The militia LP one completed complex pays**, by archetype:
/// `dunArchetypeComplex{Novice,Small,Medium,Large}` [V] (`appConst:569-572`)
/// rising with the size. The values are the commonly quoted Tranquility
/// payouts, not read from the client (the payout is server-only).
/// Falsified by a Tranquility capture paying a different amount per size,
/// or by plexers whose LP outruns (or trails) mission runners' by far.
pub const LP_BY_ARCHETYPE: [(i64, i64); 4] =
    [(33, 10_000), (34, 17_500), (35, 25_000), (36, 30_000)];

/// `factionwarfare/const.ADJACENCY_STATE_LP_MULTIPLIER` **[V]**: what a
/// system's line does to the LP paid in it (front 1.5, second 1.0, back
/// 0.01). Applying it to complexes and kills is **[A]**; falsified by a
/// Tranquility backline complex paying the same as a frontline one.
pub fn lp_multiplier(a: Adjacency) -> f64 {
    match a {
        Adjacency::Frontline => 1.5,
        Adjacency::Secondline => 1.0,
        Adjacency::Backline => 0.01,
    }
}

/// `appConst.facwarIskDestroyedToLoyaltyPointRatio` **[V]**. Read as ISK
/// destroyed per LP **[A]** (`LP = isk / ratio`); the constant has no client
/// consumer, so the direction is the guess. Falsified by a Tranquility kill
/// paying about its ISK destroyed divided by something else.
pub const ISK_DESTROYED_PER_LP: f64 = 3077.0;

/// `appConst.facwarMaxLPUnknownGroup` **[V]**: the most one kill pays when
/// the victim's hull group is in no `facwar/max_lp.tsv` row.
pub const MAX_LP_UNKNOWN_GROUP: i64 = 25_000;

// `appConst.facwarMaxLPPayout` **[V]**: the most one kill pays, by the
// victim hull's group id, is the bundle table `facwar/max_lp.tsv`
// (`facwar::Table::max_lp_of`); without it every kill is capped at
// `MAX_LP_UNKNOWN_GROUP`. The companion
// `facwarMaxLPPayoutFactionOverwrites` is **not** applied: nothing says
// which kills it covers **[A]** (falsified by a faction-hull kill paying
// past the table's cap).

/// **[A] How many pilots the pool grows for.** Each of up to this many
/// eligible pilots on the grid earns the full payout; past it the pool of
/// `SHARE_CAP x base` is split evenly. EvEJS's FW model uses five (a fact
/// noted, not code taken). Falsified by a fleet of six on Tranquility each
/// earning the full payout.
pub const SHARE_CAP: usize = 5;

/// `OnLPChange`'s first argument for a complex. The client discards it [V]
/// (`loyaltyPointsWalletSvc.py:67`, `_changeType`); the value is ours [A].
pub const CHANGE_FW: &str = "fwComplex";

/// The record tag of a payout's line in the ledger log ([`log_line`]).
pub const LOG_TAG: &str = "fwlp";

/// Which side a completion works for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The attacker completed the occupier's complex: toward the flip.
    Capture,
    /// The occupier completed the attacker's complex: away from the flip.
    Defence,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Capture => "capture",
            Side::Defence => "defence",
        }
    }
}

/// The LP a complex of `archetype` pays at full value ([`LP_BY_ARCHETYPE`]).
pub fn base_lp(archetype: i64) -> Option<i64> {
    LP_BY_ARCHETYPE
        .iter()
        .find(|(a, _)| *a == archetype)
        .map(|(_, lp)| *lp)
}

/// Which side a complex owned by `owner` works for in a system held by
/// `occupier` against `attacker`, and so which militia is paid. `None` for
/// a complex of neither militia.
pub fn side_of(owner: Option<i64>, occupier: i64, attacker: i64) -> Option<(Side, i64)> {
    match owner? {
        o if o == occupier => Some((Side::Capture, attacker)),
        o if o == attacker => Some((Side::Defence, occupier)),
        _ => None,
    }
}

/// **The payout rule, pure**: what each of `pilots` eligible pilots earns
/// for a complex of `archetype` completed on `side`, in a system whose
/// score was `score` (of `threshold`), on the `adjacency` line. Zero for no
/// pilots, a non-complex archetype or an uncontested defence.
pub fn payout(
    archetype: i64,
    side: Side,
    score: i64,
    threshold: i64,
    pilots: usize,
    adjacency: Adjacency,
) -> i64 {
    let Some(base) = base_lp(archetype) else {
        return 0;
    };
    if pilots == 0 || threshold <= 0 {
        return 0;
    }
    let scaled = match side {
        Side::Capture => base,
        Side::Defence => base * score.clamp(0, threshold) / threshold,
    };
    let pool = scaled * pilots.min(SHARE_CAP) as i64;
    (pool as f64 * lp_multiplier(adjacency) / pilots as f64).round() as i64
}

/// One pilot paid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payout {
    pub character_id: i64,
    pub corp_id: i64,
    pub lp: i64,
    pub side: Side,
    pub archetype: i64,
    pub system: i64,
    pub site_id: i64,
    /// The eligible pilots it was shared among.
    pub pilots: usize,
    pub at: BlueTime,
}

/// A payout's line in the ledger log: tag, time, system, site, character,
/// militia corporation, LP, archetype, side, pilots sharing.
pub fn log_line(p: &Payout) -> String {
    format!(
        "{LOG_TAG}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        p.at,
        p.system,
        p.site_id,
        p.character_id,
        p.corp_id,
        p.lp,
        p.archetype,
        p.side.as_str(),
        p.pilots
    )
}

/// **On the complex's grid** at the completion: within `site::
/// ARRIVAL_RANGE` of its anchor, **in warp or not** [A]. `site_spawn` sees
/// the completion on its next check (up to `site::CHECK_EVERY` after the
/// last kill), and a pilot who warps off at the last kill is still aligning
/// out then (seen in the socket test: in warp, 10 km from the anchor).
/// Falsified by a pilot warping past a complex being paid for it.
pub fn on_grid(position: [f64; 3], anchor: [f64; 3]) -> bool {
    crate::space_state::distance(position, anchor) <= crate::site::ARRIVAL_RANGE
}

/// **The completion hook** (`site_spawn`'s completion branch): the site
/// `site_id` in `system`, anchored at `anchor`, was completed at `now`.
/// Pays the eligible pilots inside, if it is an FW complex of a warzone
/// system. Returns who was paid.
pub fn on_complete(
    w: &mut Registry,
    site_id: i64,
    system: i64,
    anchor: [f64; 3],
    now: BlueTime,
) -> Vec<Payout> {
    let Some(dungeon_id) = w
        .ext::<crate::site::Sites>()
        .get(site_id)
        .map(|i| i.dungeon_id)
    else {
        return Vec::new();
    };
    let Some((archetype, owner)) = w
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&dungeon_id))
        .map(|d| (d.archetype, d.faction))
    else {
        return Vec::new();
    };
    if base_lp(archetype).is_none() {
        return Vec::new();
    }
    let inside: Vec<i64> = crate::dungeon::audience(w, system)
        .into_iter()
        .filter(|c| {
            w.roster()
                .get(*c)
                .is_some_and(|p| on_grid(p.motion.position, anchor))
        })
        .collect();
    announce(w, system, owner, &inside);
    pay(w, system, site_id, archetype, owner, &inside, now)
}

/// `OnFacWarDungeonCompleted(wasConquered, factionID)` [V arity 2,
/// `fwWarzoneSvc`], the push that plays the completion video.
pub const COMPLETED_EVENT: &str = "OnFacWarDungeonCompleted";

/// The completion push, to the pilots on the complex's grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedPush {
    pub was_conquered: bool,
    pub faction: i64,
    pub to: Vec<i64>,
}

impl crate::notify::Push for CompletedPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            COMPLETED_EVENT,
            vec![
                PyValue::Bool(self.was_conquered),
                PyValue::Int(self.faction),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// The push for a complex owned by `owner` completed in `system`, to
/// `inside`: `wasConquered` is a capture, `factionID` the militia paid
/// **[A]**. Falsified by the video naming the wrong faction or text. `None`
/// outside the warzones, for a complex of neither militia or with nobody
/// inside.
pub fn completed_push(
    w: &Registry,
    system: i64,
    owner: Option<i64>,
    inside: &[i64],
) -> Option<CompletedPush> {
    if inside.is_empty() {
        return None;
    }
    let o = crate::facwar::occupation(w, system)?;
    let (side, paid) = side_of(owner, o.occupier, o.attacker)?;
    Some(CompletedPush {
        was_conquered: side == Side::Capture,
        faction: paid,
        to: inside.to_vec(),
    })
}

fn announce(w: &mut Registry, system: i64, owner: Option<i64>, inside: &[i64]) {
    if let Some(push) = completed_push(w, system, owner, inside) {
        w.schedule(std::time::Duration::ZERO, push);
    }
}

/// Pay `inside` (the characters on the complex's grid) for a complex of
/// `archetype` owned by `owner`, completed in `system` at `now`: the
/// eligible ones share [`payout`], each credited to their militia
/// corporation's LP wallet with one ledger line. Nothing outside the
/// warzones.
pub fn pay(
    w: &mut Registry,
    system: i64,
    site_id: i64,
    archetype: i64,
    owner: Option<i64>,
    inside: &[i64],
    now: BlueTime,
) -> Vec<Payout> {
    let Some(o) = crate::facwar::occupation(w, system) else {
        return Vec::new();
    };
    let Some((side, paid)) = side_of(owner, o.occupier, o.attacker) else {
        return Vec::new();
    };
    let Some(corp) = w
        .ext::<facwar_mgr::Index>()
        .militia_corp
        .get(&paid)
        .copied()
    else {
        return Vec::new();
    };
    let mut eligible: Vec<i64> = inside
        .iter()
        .copied()
        .filter(|c| facwar_mgr::my_faction(w, *c, now) == Some(paid))
        .collect();
    eligible.sort_unstable();
    eligible.dedup();
    let adjacency = crate::facwar::adjacency(w, system);
    let lp = payout(
        archetype,
        side,
        o.score,
        crate::facwar::VP_THRESHOLD,
        eligible.len(),
        adjacency,
    );
    // FW-5: the points the complex is worth, split among the pilots paid, for
    // the stats windows. [A] The score itself still moves at despawn.
    if !eligible.is_empty()
        && (side == Side::Capture || o.score > 0)
        && let Some(vp) = crate::facwar::vp_for(archetype)
    {
        for ch in eligible.iter().copied() {
            crate::fw_stats::record(w, ch, 0, vp / eligible.len() as i64, now);
        }
    }
    if lp <= 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for ch in eligible.iter().copied() {
        if crate::loyalty::adjust(w, ch, corp, lp, CHANGE_FW).is_none() {
            continue;
        }
        let p = Payout {
            character_id: ch,
            corp_id: corp,
            lp,
            side,
            archetype,
            system,
            site_id,
            pilots: eligible.len(),
            at: now,
        };
        w.journal().log().append(now, log_line(&p));
        crate::outln!(
            "[fwlp] {ch} earned {lp} LP with {corp} for a {} of complex {site_id} in {system}",
            side.as_str()
        );
        out.push(p);
    }
    out
}

/// The most a kill of a hull of `group` pays: the bundle's `facwar/max_lp.tsv`
/// row, else [`MAX_LP_UNKNOWN_GROUP`].
pub fn max_kill_lp(c: &crate::content::Content, group: Option<i64>) -> i64 {
    group
        .and_then(|g| c.try_get::<crate::facwar::Table>()?.max_lp_of(g))
        .unwrap_or(MAX_LP_UNKNOWN_GROUP)
}

/// **[A] How long a kill of one victim by one attacker pays once.** A second
/// kill of the same victim by the same pilot inside this window pays that
/// pilot nothing, so two friends cannot trade insured ships for LP.
/// Falsified by a week of `fwkill` lines showing the same pair earning more
/// than a few times a day, or a pair feeding anyway.
pub const REPEAT_WINDOW: BlueTime = 24 * 3600 * 10_000_000;

/// The insurance payout of the ship the last kill destroyed, which the
/// insurance hook notes and [`pay_kill`] subtracts from the ISK destroyed:
/// an insured loss is no loss to its owner, so it earns no LP.
#[derive(Default)]
pub struct Insured {
    ship: i64,
    amount: f64,
}

/// The world slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "fw_lp",
    make: |_| Box::new(Insured::default()),
};

/// Called by `insurance::on_kill` when it pays `amount` for `ship`.
pub fn note_insured(w: &mut Registry, ship: i64, amount: f64) {
    *w.ext_mut::<Insured>() = Insured { ship, amount };
}

/// The ship the last insurance payout (a policy or the base) was made for,
/// `0` before any: `insurance::pay_out` returns early on the same ship, so a
/// hull the store refused to remove is not paid again by the next kill.
pub fn last_insured_ship(w: &Registry) -> i64 {
    w.ext::<Insured>().ship
}

/// Whether `pilot` already earned from killing `victim` in the window before
/// `at`: any earlier report of that victim with the pilot among its attackers.
fn paid_recently(w: &Registry, pilot: i64, victim: i64, at: BlueTime) -> bool {
    crate::killmail::involving(w, pilot, None, 200)
        .iter()
        .any(|r| {
            r.victim == victim
                && r.time < at
                && at - r.time < REPEAT_WINDOW
                && (r.final_blow.character == Some(pilot)
                    || r.attackers.iter().any(|a| a.character == Some(pilot)))
        })
}

/// **What a kill is worth, pure** (#133): the ISK destroyed over
/// [`ISK_DESTROYED_PER_LP`], capped at `cap` (see [`max_kill_lp`]), then scaled
/// by the system's `adjacency`. Zero for no ISK.
pub fn kill_lp(isk_destroyed: f64, cap: i64, adjacency: Adjacency) -> i64 {
    if isk_destroyed.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return 0;
    }
    let raw = (isk_destroyed / ISK_DESTROYED_PER_LP).min(cap as f64);
    (raw * lp_multiplier(adjacency)).round() as i64
}

/// **[A] A kill's pool is split by damage dealt.** Of `pool`, the share of
/// an attacker who did `damage` of the `total` the victim took; `total` is
/// every attacker's, so damage by pilots outside the enemy militia pays
/// nobody. Falsified by Tranquility splitting a kill evenly or paying the
/// final blow alone.
pub fn damage_share(pool: i64, damage: i64, total: i64) -> i64 {
    if total <= 0 || damage <= 0 {
        return 0;
    }
    (pool as i128 * damage.min(total) as i128 / total as i128) as i64
}

/// The record tag of a kill payout's line in the ledger log.
pub const KILL_LOG_TAG: &str = "fwkill";

/// One pilot paid for a kill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillPayout {
    pub character_id: i64,
    pub corp_id: i64,
    pub lp: i64,
    pub victim: i64,
    pub system: i64,
    /// The pool the kill paid before it was split, in LP.
    pub pool: i64,
    pub at: BlueTime,
}

/// A kill payout's ledger line: tag, time, system, victim, character,
/// militia corporation, LP, pool.
pub fn kill_log_line(p: &KillPayout) -> String {
    format!(
        "{KILL_LOG_TAG}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        p.at, p.system, p.victim, p.character_id, p.corp_id, p.lp, p.pool
    )
}

/// **The `kill_hooks::PLAYER` row** (after `killmail_on_kill`, which writes
/// the report [`pay_kill`] reads), #133.
pub fn on_kill(w: &mut Registry, ev: &crate::kill_hooks::KillEvent) {
    let _ = pay_kill(w, ev);
}

/// A pilot of one militia destroyed a ship of the other's: each such
/// attacker earns a damage share of [`kill_lp`] in their militia
/// corporation's LP. Kills pay in warzone systems only (owner ruling); the line
/// multiplier is the system's. The insurance payout is taken off the ISK, and a
/// pilot who killed this victim in the last [`REPEAT_WINDOW`] earns nothing.
/// Never moves VP.
pub fn pay_kill(w: &mut Registry, ev: &crate::kill_hooks::KillEvent) -> Vec<KillPayout> {
    let Some(report) = crate::killmail::involving(w, ev.victim_character, None, 1)
        .into_iter()
        .find(|r| r.victim == ev.victim_character && r.time == ev.when)
    else {
        return Vec::new();
    };
    let Some(isk) = report.isk_destroyed else {
        return Vec::new();
    };
    // Outside the warzones a kill pays nothing (owner ruling, #133).
    if crate::facwar::occupation(w, ev.system).is_none() {
        return Vec::new();
    }
    let adjacency = crate::facwar::adjacency(w, ev.system);
    let group = crate::typedata::item_in(w.content(), report.ship_type).map(|t| t.group_id);
    let insured = {
        let i = w.ext::<Insured>();
        if i.ship == ev.victim_ship {
            i.amount
        } else {
            0.0
        }
    };
    let cap = max_kill_lp(w.content(), group);
    let pool = kill_lp(isk - insured, cap, adjacency);
    if pool <= 0 {
        return Vec::new();
    }
    let total: i64 = std::iter::once(&report.final_blow)
        .chain(report.attackers.iter())
        .map(|a| a.damage.max(0))
        .sum();
    // One entry per pilot: the final blow and the others, summed by pilot.
    let mut by_pilot: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
    for a in std::iter::once(&report.final_blow).chain(report.attackers.iter()) {
        if let Some(c) = a.character
            && c != ev.victim_character
            && crate::crimewatch::fw_enemies(w, c, ev.victim_character)
            && !paid_recently(w, c, ev.victim_character, ev.when)
        {
            *by_pilot.entry(c).or_default() += a.damage.max(0);
        }
    }
    let mut out = Vec::new();
    for (ch, damage) in by_pilot {
        let lp = damage_share(pool, damage, total);
        let Some(corp) = facwar_mgr::war_faction(w, ch)
            .and_then(|f| w.ext::<facwar_mgr::Index>().militia_corp.get(&f).copied())
        else {
            continue;
        };
        if lp <= 0 || crate::loyalty::adjust(w, ch, corp, lp, CHANGE_FW).is_none() {
            continue;
        }
        let p = KillPayout {
            character_id: ch,
            corp_id: corp,
            lp,
            victim: ev.victim_character,
            system: ev.system,
            pool,
            at: ev.when,
        };
        w.journal().log().append(ev.when, kill_log_line(&p));
        crate::outln!(
            "[fwlp] {ch} earned {lp} LP with {corp} for destroying {}'s ship in {}",
            ev.victim_character,
            ev.system
        );
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::celestial::Universe;
    use crate::content::Content;
    use crate::facwar::{self, Table, VP_THRESHOLD, Warzone};
    use crate::model::{Character, SandboxConfig};
    use std::sync::Arc;

    /// Invented militias A and B of one warzone, an outsider C, their
    /// militia corporations, and one warzone system held by A.
    const A: i64 = 599_701;
    const B: i64 = 599_702;
    const C: i64 = 599_703;
    const CORP_A: i64 = 1_999_701;
    const CORP_B: i64 = 1_999_702;
    const ZONE: i64 = 96;
    const S1: i64 = 39_000_701;
    const S2: i64 = 39_000_702;
    const OUTSIDE: i64 = 39_000_703;
    const SITE: i64 = 4_000_700;
    const NOW: BlueTime = 134_000_000_000_000_000;
    /// The archetypes by name: the size order is [V], the LP is not used.
    const NOVICE: i64 = 33;
    const LARGE: i64 = 36;
    const NOT_A_COMPLEX: i64 = 1;

    fn universe() -> Universe {
        let row = |id: i64, faction: Option<i64>| {
            let f = faction.map_or_else(String::new, |f| f.to_string());
            format!(
                "{id}\t19000701\t29000701\t{f}\t\t0.3\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture {id}"
            )
        };
        Universe::parse(
            &[row(S1, Some(A)), row(S2, Some(B)), row(OUTSIDE, None)],
            &[],
            &[],
        )
        .expect("the fixture universe")
    }

    fn npc_data() -> crate::npc_data::Table {
        let mut t = crate::npc_data::Table::default();
        for (f, corp) in [(A, CORP_A), (B, CORP_B)] {
            t.factions.insert(
                f,
                crate::npc_data::Faction {
                    faction_id: f,
                    corporation_id: corp + 10,
                    militia_corporation_id: Some(corp),
                    ..crate::npc_data::Faction::default()
                },
            );
        }
        t
    }

    fn pilot(id: i64) -> Character {
        Character {
            character_id: id,
            ship_id: id + 100,
            solar_system_id: Some(S1),
            ..Character::default()
        }
    }

    /// Pilots 1..=7.
    fn ch(n: i64) -> i64 {
        90_000_700 + n
    }

    fn world() -> Registry {
        let mut c = Content::fixture();
        let zone = Warzone {
            id: ZONE,
            factions: [A, B],
            hqs: [S1, S2],
        };
        c.insert("facwar", Table::new(vec![zone]).expect("ok"));
        c.insert("universe", universe());
        c.insert("npc_data", npc_data());
        Registry::new(SandboxConfig {
            characters: (1..=7).map(|n| pilot(ch(n))).collect(),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c))
    }

    fn enlist(w: &mut Registry, who: i64, faction: i64) {
        facwar_mgr::enlist(w, who, faction, NOW).expect("enlists");
        let _ = w.take_pending();
    }

    fn set_score(w: &mut Registry, system: i64, score: i64, occupier: i64) {
        w.deltas_mut().put(
            crate::facwar::KIND,
            vec![system.to_string()],
            vec![score.to_string(), occupier.to_string(), String::new()],
        );
    }

    #[test]
    fn a_capture_pays_the_full_base_and_bigger_complexes_pay_more() {
        let novice = payout(NOVICE, Side::Capture, 0, 100, 1, Adjacency::Secondline);
        assert_eq!(Some(novice), base_lp(NOVICE));
        let mut last = 0;
        for (a, _) in LP_BY_ARCHETYPE {
            let lp = payout(a, Side::Capture, 0, 100, 1, Adjacency::Secondline);
            assert!(lp > last, "archetype {a} pays more than the smaller");
            last = lp;
        }
        assert_eq!(
            payout(
                NOT_A_COMPLEX,
                Side::Capture,
                0,
                100,
                1,
                Adjacency::Secondline
            ),
            0
        );
        assert_eq!(
            payout(NOVICE, Side::Capture, 0, 100, 0, Adjacency::Secondline),
            0
        );
    }

    #[test]
    fn a_defence_pays_by_how_contested_the_system_is() {
        let base = base_lp(LARGE).expect("large");
        assert_eq!(
            payout(LARGE, Side::Defence, 0, 100, 1, Adjacency::Secondline),
            0
        );
        assert_eq!(
            payout(LARGE, Side::Defence, 50, 100, 1, Adjacency::Secondline),
            base / 2
        );
        assert_eq!(
            payout(LARGE, Side::Defence, 100, 100, 1, Adjacency::Secondline),
            base
        );
        assert_eq!(
            payout(LARGE, Side::Defence, 500, 100, 1, Adjacency::Secondline),
            base
        );
        assert_eq!(
            payout(LARGE, Side::Capture, 0, 100, 1, Adjacency::Secondline),
            base
        );
    }

    #[test]
    fn up_to_the_cap_each_pilot_earns_in_full_and_past_it_they_share() {
        let base = base_lp(NOVICE).expect("novice");
        for n in 1..=SHARE_CAP {
            assert_eq!(
                payout(NOVICE, Side::Capture, 0, 100, n, Adjacency::Secondline),
                base
            );
        }
        let n = SHARE_CAP * 2;
        assert_eq!(
            payout(NOVICE, Side::Capture, 0, 100, n, Adjacency::Secondline),
            base / 2
        );
    }

    #[test]
    fn the_side_is_against_the_complexs_owner() {
        assert_eq!(side_of(Some(A), A, B), Some((Side::Capture, B)));
        assert_eq!(side_of(Some(B), A, B), Some((Side::Defence, A)));
        assert_eq!(side_of(Some(C), A, B), None);
        assert_eq!(side_of(None, A, B), None);
    }

    /// The attacker's pilots inside share the capture; the owner's own
    /// pilot, an unenlisted one and a pilot not on the grid earn nothing.
    /// Each paid pilot's wallet holds the LP with B's militia corporation
    /// and hears `OnLPChange`.
    #[test]
    fn a_capture_pays_the_attackers_pilots_inside_to_their_militia_corp() {
        let mut w = world();
        enlist(&mut w, ch(1), B);
        enlist(&mut w, ch(2), B);
        enlist(&mut w, ch(3), A);
        enlist(&mut w, ch(5), B);
        let inside = [ch(1), ch(2), ch(3), ch(4)];
        let paid = pay(&mut w, S1, SITE, NOVICE, Some(A), &inside, NOW);
        let lp = payout(
            NOVICE,
            Side::Capture,
            0,
            VP_THRESHOLD,
            2,
            facwar::adjacency(&mut w, S1),
        );
        assert_eq!(
            paid.iter().map(|p| p.character_id).collect::<Vec<_>>(),
            vec![ch(1), ch(2)]
        );
        assert!(paid.iter().all(|p| p.lp == lp && p.corp_id == CORP_B));
        assert_eq!(crate::loyalty::balance(&w, ch(1), CORP_B), lp);
        assert_eq!(crate::loyalty::balance(&w, ch(2), CORP_B), lp);
        for n in [3, 4, 5] {
            assert!(crate::loyalty::balances(&w, ch(n)).is_empty(), "pilot {n}");
        }
        let lp_pushes: Vec<_> = w
            .take_pending()
            .into_iter()
            .map(|(_, p)| (p.to_push(), p.audience()))
            .filter(|((name, _), _)| *name == "OnLPChange")
            .collect();
        assert_eq!(lp_pushes.len(), 2);
        let ((_, args), to) = &lp_pushes[0];
        assert_eq!(to, &Some(vec![ch(1)]));
        assert_eq!(
            args[1..],
            [PyValue::Int(CORP_B), PyValue::Int(0), PyValue::Int(lp)]
        );
        let line = log_line(&paid[0]);
        assert!(line.starts_with(LOG_TAG) && line.ends_with('\n'));
        assert_eq!(line.split('\t').count(), 10);
    }

    #[test]
    fn a_defence_pays_the_occupier_by_the_score_and_nothing_when_uncontested() {
        let mut w = world();
        enlist(&mut w, ch(1), A);
        assert!(pay(&mut w, S1, SITE, LARGE, Some(B), &[ch(1)], NOW).is_empty());
        set_score(&mut w, S1, VP_THRESHOLD / 4, A);
        let paid = pay(&mut w, S1, SITE, LARGE, Some(B), &[ch(1)], NOW);
        let lp = payout(
            LARGE,
            Side::Defence,
            VP_THRESHOLD / 4,
            VP_THRESHOLD,
            1,
            facwar::adjacency(&mut w, S1),
        );
        assert!(lp > 0);
        assert_eq!(paid.len(), 1);
        assert_eq!(paid[0].side, Side::Defence);
        assert_eq!(crate::loyalty::balance(&w, ch(1), CORP_A), lp);
    }

    #[test]
    fn outside_the_warzones_or_for_an_outsiders_complex_nobody_is_paid() {
        let mut w = world();
        enlist(&mut w, ch(1), B);
        assert!(pay(&mut w, OUTSIDE, SITE, NOVICE, Some(A), &[ch(1)], NOW).is_empty());
        assert!(pay(&mut w, S1, SITE, NOVICE, Some(C), &[ch(1)], NOW).is_empty());
        assert!(pay(&mut w, S1, SITE, NOT_A_COMPLEX, Some(A), &[ch(1)], NOW).is_empty());
        assert!(crate::loyalty::balances(&w, ch(1)).is_empty());
    }

    #[test]
    fn the_grid_is_the_arrival_range_around_the_anchor() {
        let r = crate::site::ARRIVAL_RANGE;
        assert!(on_grid([r, 0.0, 0.0], [0.0; 3]));
        assert!(!on_grid([r * 2.0, 0.0, 0.0], [0.0; 3]));
    }

    /// No site instance: the hook pays nothing and does not panic.
    #[test]
    fn the_hook_ignores_a_site_with_no_instance() {
        let mut w = world();
        assert!(on_complete(&mut w, SITE, S1, [0.0; 3], NOW).is_empty());
    }

    /// `OnFacWarDungeonCompleted(wasConquered, factionID)`: arity 2, a
    /// capture is conquered, the faction is the militia paid, and nothing
    /// goes out for a stranger's complex, other space or an empty grid.
    #[test]
    fn the_completion_push_names_the_side_and_the_militia_paid() {
        let w = world();
        // S1 is held by A, so B is the attacker: A's complex is a capture
        // for B, B's complex a defence for A.
        let capture = completed_push(&w, S1, Some(A), &[ch(1), ch(2)]).expect("a push");
        assert!(capture.was_conquered);
        assert_eq!(capture.faction, B);
        assert_eq!(capture.to, [ch(1), ch(2)]);
        let defence = completed_push(&w, S1, Some(B), &[ch(1)]).expect("a push");
        assert!(!defence.was_conquered);
        assert_eq!(defence.faction, A);
        let (name, args) = crate::notify::Push::to_push(&capture);
        assert_eq!(name, "OnFacWarDungeonCompleted");
        assert_eq!(args, [PyValue::Bool(true), PyValue::Int(B)]);
        assert_eq!(completed_push(&w, S1, Some(C), &[ch(1)]), None);
        assert_eq!(completed_push(&w, OUTSIDE, Some(A), &[ch(1)]), None);
        assert_eq!(completed_push(&w, S1, Some(A), &[]), None);
    }

    #[test]
    fn the_line_scales_a_complexs_payout() {
        let sl = payout(LARGE, Side::Capture, 0, 100, 1, Adjacency::Secondline);
        let fl = payout(LARGE, Side::Capture, 0, 100, 1, Adjacency::Frontline);
        let bl = payout(LARGE, Side::Capture, 0, 100, 1, Adjacency::Backline);
        assert_eq!(Some(sl), base_lp(LARGE));
        assert!(fl > sl && sl > bl && bl > 0);
        assert_eq!(fl, (sl as f64 * lp_multiplier(Adjacency::Frontline)) as i64);
        assert_eq!(bl, (sl as f64 * lp_multiplier(Adjacency::Backline)) as i64);
    }

    const GROUP_SMALL: i64 = 9_001;
    const GROUP_BIG: i64 = 9_002;
    const A_BIG_ISK: f64 = 1.0e12;

    /// Invented caps for two invented hull groups.
    fn caps_content() -> Content {
        let mut c = Content::fixture();
        let caps = [(GROUP_SMALL, 2_000), (GROUP_BIG, 50_000)];
        c.insert(
            "facwar",
            Table::new(vec![Warzone {
                id: ZONE,
                factions: [A, B],
                hqs: [S1, S2],
            }])
            .expect("ok")
            .with_max_lp(caps.into_iter().collect()),
        );
        c
    }

    #[test]
    fn a_kill_never_pays_less_for_more_isk_and_never_past_its_cap() {
        let c = caps_content();
        let small = max_kill_lp(&c, Some(GROUP_SMALL));
        let big = max_kill_lp(&c, Some(GROUP_BIG));
        assert!(big > small, "a bigger hull group has the higher cap");
        assert_eq!(max_kill_lp(&c, Some(1)), MAX_LP_UNKNOWN_GROUP, "no row");
        assert_eq!(max_kill_lp(&c, None), MAX_LP_UNKNOWN_GROUP);
        assert_eq!(
            max_kill_lp(&Content::fixture(), Some(GROUP_SMALL)),
            MAX_LP_UNKNOWN_GROUP
        );
        for adj in [
            Adjacency::Frontline,
            Adjacency::Secondline,
            Adjacency::Backline,
        ] {
            assert_eq!(kill_lp(0.0, small, adj), 0);
            assert_eq!(kill_lp(-5.0, small, adj), 0);
            let mut last = 0;
            for step in 0..60 {
                let isk = ISK_DESTROYED_PER_LP * 10.0_f64.powf(f64::from(step) / 5.0);
                let lp = kill_lp(isk, small, adj);
                assert!(lp >= last, "more ISK never pays less");
                last = lp;
            }
            let ceiling = (small as f64 * lp_multiplier(adj)).round() as i64;
            assert_eq!(kill_lp(A_BIG_ISK, small, adj), ceiling, "the cap holds");
            assert!(kill_lp(A_BIG_ISK, big, adj) >= kill_lp(A_BIG_ISK, small, adj));
        }
        let isk = ISK_DESTROYED_PER_LP * 100.0;
        assert!(
            kill_lp(isk, small, Adjacency::Backline) <= kill_lp(isk, small, Adjacency::Secondline)
        );
        assert!(
            kill_lp(isk, small, Adjacency::Secondline) <= kill_lp(isk, small, Adjacency::Frontline)
        );
    }

    #[test]
    fn a_kills_shares_never_sum_past_the_pool() {
        assert_eq!(damage_share(100, 0, 100), 0);
        assert_eq!(damage_share(100, 10, 0), 0);
        assert_eq!(damage_share(100, 500, 100), 100, "more than all is all");
        for pool in [0, 1, 7, 100, 12_345] {
            for parts in [&[1_i64][..], &[1, 1, 1], &[30, 10, 40, 20], &[5, 0, 3]] {
                let total: i64 = parts.iter().sum();
                let paid: i64 = parts.iter().map(|d| damage_share(pool, *d, total)).sum();
                assert!(paid <= pool, "{parts:?} of {pool}");
            }
        }
    }

    fn kill_event(victim: i64, ship_type: i64) -> crate::kill_hooks::KillEvent {
        crate::kill_hooks::KillEvent {
            victim_character: victim,
            victim_ship: victim + 100,
            ship_type,
            killer: None,
            system: S1,
            npc: None,
            when: NOW,
            park: Vec::new(),
        }
    }

    /// A mail for `victim` destroying `isk`, attacked by `(pilot, damage)`.
    fn kill_report(victim: i64, isk: f64, by: &[(i64, i64)]) -> crate::killmail::Report {
        let at = |(c, damage): (i64, i64)| crate::killmail::Attacker {
            character: Some(c),
            corporation: None,
            alliance: None,
            faction: None,
            ship_type: None,
            weapon_type: None,
            damage,
            security: None,
        };
        crate::killmail::Report {
            kill_id: 1,
            time: NOW,
            system: S1,
            victim,
            victim_corporation: 1,
            victim_alliance: None,
            ship_type: 1,
            damage_taken: by.iter().map(|b| b.1).sum(),
            isk_lost: Some(isk),
            isk_destroyed: Some(isk),
            final_blow: at(by[0]),
            attackers: by[1..].iter().map(|b| at(*b)).collect(),
            items: Vec::new(),
            bounty_claimed: None,
            kill_right_supplied: None,
        }
    }

    fn pool_of(w: &mut Registry, isk: f64) -> i64 {
        let cap = max_kill_lp(w.content(), None);
        kill_lp(isk, cap, facwar::adjacency(w, S1))
    }

    /// B's pilots kill A's pilot: attackers of the other militia earn from
    /// the pool, to B's militia corporation; the dead pilot's side, a
    /// bystander and a pilot of no militia earn nothing, the shares never sum
    /// past the pool, and no victory points move.
    #[test]
    fn a_kill_of_the_enemy_militia_pays_only_the_enemy_attackers() {
        let mut w = world();
        enlist(&mut w, ch(1), A);
        enlist(&mut w, ch(2), B);
        enlist(&mut w, ch(3), B);
        enlist(&mut w, ch(4), A);
        let isk = ISK_DESTROYED_PER_LP * 1000.0;
        crate::killmail::put(
            &mut w,
            &kill_report(
                ch(1),
                isk,
                &[(ch(2), 30), (ch(3), 10), (ch(4), 40), (ch(5), 20)],
            ),
        );
        let paid = pay_kill(&mut w, &kill_event(ch(1), 1));
        let pool = pool_of(&mut w, isk);
        assert!(pool > 0);
        assert_eq!(paid.len(), 2);
        let by = |c: i64| paid.iter().find(|p| p.character_id == c).map(|p| p.lp);
        let (big, small) = (by(ch(2)).expect("paid"), by(ch(3)).expect("paid"));
        assert!(big > small, "more damage never earns less");
        assert!(big + small <= pool, "the shares fit in the pool");
        assert!(paid.iter().all(|p| p.corp_id == CORP_B && p.pool == pool));
        assert_eq!(crate::loyalty::balance(&w, ch(2), CORP_B), big);
        assert!(
            crate::loyalty::balances(&w, ch(4)).is_empty(),
            "his own side"
        );
        assert!(crate::loyalty::balances(&w, ch(5)).is_empty(), "no militia");
        assert!(crate::loyalty::balances(&w, ch(1)).is_empty(), "the dead");
        assert!(kill_log_line(&paid[0]).starts_with(KILL_LOG_TAG));
        assert_eq!(kill_log_line(&paid[0]).split('\t').count(), 8);
        assert!(
            w.deltas().of(crate::fw_stats::KIND).is_empty(),
            "a kill moves no points"
        );
    }

    #[test]
    fn a_neutral_victim_or_a_mail_without_isk_pays_nothing() {
        let mut w = world();
        enlist(&mut w, ch(2), B);
        let isk = ISK_DESTROYED_PER_LP * 1000.0;
        crate::killmail::put(&mut w, &kill_report(ch(5), isk, &[(ch(2), 10)]));
        assert!(pay_kill(&mut w, &kill_event(ch(5), 1)).is_empty());
        enlist(&mut w, ch(1), A);
        let mut r = kill_report(ch(1), isk, &[(ch(2), 10)]);
        r.isk_destroyed = None;
        crate::killmail::put(&mut w, &r);
        assert!(pay_kill(&mut w, &kill_event(ch(1), 1)).is_empty());
        assert!(crate::loyalty::balances(&w, ch(2)).is_empty());
    }

    /// Owner ruling: a kill outside the warzones pays 0, however big.
    #[test]
    fn a_kill_outside_the_warzones_pays_nothing() {
        let mut w = world();
        enlist(&mut w, ch(1), A);
        enlist(&mut w, ch(2), B);
        let mut r = kill_report(ch(1), A_BIG_ISK, &[(ch(2), 10)]);
        r.system = OUTSIDE;
        crate::killmail::put(&mut w, &r);
        let mut ev = kill_event(ch(1), 1);
        ev.system = OUTSIDE;
        assert!(pay_kill(&mut w, &ev).is_empty());
        assert!(crate::loyalty::balances(&w, ch(2)).is_empty());
        // The same mail in a warzone system pays.
        assert!(!pay_kill(&mut w, &kill_event(ch(1), 1)).is_empty());
    }

    /// An insured loss is no loss: the payout comes off the ISK, so a hull
    /// insured for all it is worth pays nobody, and the note is the ship's.
    #[test]
    fn the_insurance_payout_comes_off_the_isk_destroyed() {
        let isk = ISK_DESTROYED_PER_LP * 1000.0;
        let run = |insured: Option<(i64, f64)>| {
            let mut w = world();
            enlist(&mut w, ch(1), A);
            enlist(&mut w, ch(2), B);
            crate::killmail::put(&mut w, &kill_report(ch(1), isk, &[(ch(2), 10)]));
            if let Some((ship, amount)) = insured {
                note_insured(&mut w, ship, amount);
            }
            pay_kill(&mut w, &kill_event(ch(1), 1))
                .iter()
                .map(|p| p.lp)
                .sum::<i64>()
        };
        let none = run(None);
        let part = run(Some((ch(1) + 100, isk / 2.0)));
        assert!(none > 0);
        assert!(part < none, "an insured ship pays less");
        assert_eq!(run(Some((ch(1) + 100, isk))), 0, "fully insured pays 0");
        assert_eq!(run(Some((ch(1) + 100, isk * 2.0))), 0, "never negative");
        assert_eq!(run(Some((ch(3) + 100, isk))), none, "another ship's note");
    }

    /// The same pilot killing the same victim twice inside the window earns
    /// once; after the window, or another pilot, earns again.
    #[test]
    fn the_same_pair_earns_once_inside_the_window() {
        let isk = ISK_DESTROYED_PER_LP * 1000.0;
        let mut w = world();
        enlist(&mut w, ch(1), A);
        enlist(&mut w, ch(2), B);
        enlist(&mut w, ch(3), B);
        let mut first = kill_report(ch(1), isk, &[(ch(2), 10)]);
        first.time = NOW - 3_600 * 10_000_000;
        crate::killmail::put(&mut w, &first);
        let mut second = kill_report(ch(1), isk, &[(ch(2), 10), (ch(3), 10)]);
        second.kill_id = 2;
        crate::killmail::put(&mut w, &second);
        let paid = pay_kill(&mut w, &kill_event(ch(1), 1));
        assert_eq!(paid.len(), 1, "only the newcomer");
        assert_eq!(paid[0].character_id, ch(3));
        assert!(crate::loyalty::balances(&w, ch(2)).is_empty());

        // A kill two windows later pays the first pilot again.
        let mut late = kill_report(ch(1), isk, &[(ch(2), 10)]);
        late.kill_id = 3;
        late.time = NOW + 2 * REPEAT_WINDOW;
        crate::killmail::put(&mut w, &late);
        let mut ev = kill_event(ch(1), 1);
        ev.when = late.time;
        assert_eq!(pay_kill(&mut w, &ev).len(), 1);
    }
}
