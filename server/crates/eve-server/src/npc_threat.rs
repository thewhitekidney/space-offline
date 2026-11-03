//! **NPC threat** (lane npc-threat, EVEJS-GAP space §3.8): who has hurt, jammed
//! or mended for the people hurting a rat, and how much it matters.
//!
//! CCP's NPC behaviour trees (the client's own `behaviors` pyc) put the
//! **ship** of any offensive act (damage or ewar) into a target set the
//! rat's group shares; a drone or fighter counts as its owner's ship; a ship
//! that assists one in the set joins it; the pick is the most damage dealt
//! to NPCs plus healing over the last 10 s, re-made on the 60 s switch timer.
//!
//! # The ledger
//!
//! [`Threat`] is a non-persisted world slot (`world_ext::SLOTS`): per rat, the
//! ships that acted on it ([`Aggressor`]: their volleys of the last
//! [`THREAT_WINDOW`] and the time of the last act), and per ship the healing
//! it gave lately. Three hooks write it, each one line in the code that
//! already calls `crimewatch`: [`on_act`] (damage, a miss with 0, and ewar),
//! and [`assisted`] (a remote repair). [`on_act`] also shares the aggressor
//! with the rats within `grid::GRID_ENTER` of the victim (**[A]** that the
//! grid is the nearest proxy for CCP's entity group; falsifier: a rat
//! outside the grid radius that joins in on TQ).
//!
//! # Reading
//!
//! Only the brain's plan reads it, through [`Threat::standing`], in
//! `npc::view_in`/`view_of`. Nothing in the NPC loop writes it (rats do not
//! shoot rats), but [`Threat::revision`] is part of a plan's witness
//! anyway, so a write that did come would send the plans stale and the
//! parallel loop stays bit-identical to the legacy one (PA-8).
//!
//! **[A]** [`AGGRESSION_MEMORY`] of 60 s: CCP's set does not expire inside
//! the tree. Falsifier: a rat that forgets a pilot who stops shooting for a
//! minute on TQ.

use crate::damage::BlueTime;
use crate::service::Registry;
use std::collections::BTreeMap;

/// Blue ticks in a second (100 ns).
const BLUE_PER_S: BlueTime = 10_000_000;
/// The span threat is summed over (V: CCP's 10 s).
pub const THREAT_WINDOW: BlueTime = 10 * BLUE_PER_S;
/// How long an aggressor stays one after its last act. **[A]**
pub const AGGRESSION_MEMORY: BlueTime = 60 * BLUE_PER_S;
/// How often a continuing aggressor is shared with the neighbours again, so
/// their memory does not lapse while the victim is still being shot. **[A]**
const SHARE_EVERY: BlueTime = 5 * BLUE_PER_S;

/// One ship that acted on a rat.
#[derive(Debug, Clone, PartialEq)]
pub struct Aggressor {
    pub ship: i64,
    /// `(time, damage)` of the volleys inside [`THREAT_WINDOW`].
    pub hits: Vec<(BlueTime, f64)>,
    /// The time of its last act.
    pub last: BlueTime,
}

/// The registration row in `world_ext::SLOTS` (coordinator-only).
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "npc_threat",
    make: |_| Box::new(Threat::default()),
};

/// The ledger (a world slot, not persisted).
#[derive(Debug, Default)]
pub struct Threat {
    /// Per rat, who acted on it.
    hits: BTreeMap<i64, Vec<Aggressor>>,
    /// Per ship, the healing it gave lately: `(time, amount)`.
    heals: BTreeMap<i64, Vec<(BlueTime, f64)>>,
    /// Bumped by every write.
    rev: u64,
    /// When the next sweep of dead rats and lapsed entries is due.
    sweep_at: BlueTime,
}

impl Threat {
    /// How many writes the ledger has taken (a plan's witness).
    pub fn revision(&self) -> u64 {
        self.rev
    }

    /// Whether `ship` is an aggressor of `npc` at `now`, and its threat:
    /// the damage it dealt it plus the healing it gave, both inside
    /// [`THREAT_WINDOW`]. `None` when it is not one (never acted, or
    /// lapsed past [`AGGRESSION_MEMORY`]). Pure.
    pub fn standing(&self, npc: i64, ship: i64, now: BlueTime) -> Option<f64> {
        let a = self
            .hits
            .get(&npc)?
            .iter()
            .find(|a| a.ship == ship && a.last.saturating_add(AGGRESSION_MEMORY) >= now)?;
        let recent = |t: BlueTime| t.saturating_add(THREAT_WINDOW) >= now;
        let dealt: f64 = a.hits.iter().filter(|(t, _)| recent(*t)).map(|h| h.1).sum();
        let healed: f64 = self.heals.get(&ship).map_or(0.0, |v| {
            v.iter().filter(|(t, _)| recent(*t)).map(|h| h.1).sum()
        });
        Some(dealt + healed)
    }

    /// Record an act of `ship` on `npc`; true when it is worth sharing with
    /// the neighbours (new, lapsed, or not shared for [`SHARE_EVERY`]).
    fn add(&mut self, npc: i64, ship: i64, dealt: f64, at: BlueTime) -> bool {
        self.rev += 1;
        let list = self.hits.entry(npc).or_default();
        list.retain(|a| a.last.saturating_add(AGGRESSION_MEMORY) >= at);
        let mut share = false;
        let a = match list.iter().position(|a| a.ship == ship) {
            Some(i) => &mut list[i],
            None => {
                list.push(Aggressor {
                    ship,
                    hits: Vec::new(),
                    last: at,
                });
                share = true;
                list.last_mut().expect("just pushed")
            }
        };
        if a.last.saturating_add(SHARE_EVERY) < at {
            share = true;
        }
        if dealt > 0.0 {
            a.hits.push((at, dealt));
        }
        a.hits
            .retain(|(t, _)| t.saturating_add(THREAT_WINDOW) >= at);
        a.last = a.last.max(at);
        share
    }

    /// Forget rats that are gone and entries that have lapsed.
    fn sweep(&mut self, alive: impl Fn(i64) -> bool, now: BlueTime) {
        self.hits.retain(|id, list| {
            list.retain(|a| a.last.saturating_add(AGGRESSION_MEMORY) >= now);
            alive(*id) && !list.is_empty()
        });
        self.heals.retain(|_, v| {
            v.retain(|(t, _)| t.saturating_add(THREAT_WINDOW) >= now);
            !v.is_empty()
        });
        self.sweep_at = now.saturating_add(AGGRESSION_MEMORY);
        self.rev += 1;
    }
}

/// The ship a roster key acts as: a drone is its owner's ship, a character
/// its own; an NPC (or anything else out of the roster) none.
fn acting_ship(w: &Registry, key: i64) -> Option<i64> {
    if let Some(d) = w.modules().drones().get(key) {
        return w
            .roster()
            .get(d.owner_id)
            .map(|p| p.ship_id)
            .or(Some(d.controller_id));
    }
    if w.npcs().get(key).is_some() {
        return None;
    }
    w.roster().get(key).map(|p| p.ship_id)
}

/// **The damage and ewar hook**: roster key `shooter` acted on roster key
/// `victim` at `at`, dealing `dealt` (0 for a miss or an ewar cycle). A
/// victim that is not a rat (or is a ship of the law, which keeps its own
/// brain) is ignored. The aggressor is shared with the rats of the victim's
/// grid.
pub fn on_act(w: &mut Registry, shooter: i64, victim: i64, dealt: f64, at: BlueTime) {
    let Some(npc) = w.npcs().get(victim).copied() else {
        return;
    };
    if npc.law.is_some() {
        return;
    }
    let Some(ship) = acting_ship(w, shooter) else {
        return;
    };
    if w.roster().get(victim).is_some_and(|p| p.ship_id == ship) {
        return;
    }
    let dealt = if dealt.is_finite() {
        dealt.max(0.0)
    } else {
        0.0
    };
    if !w.ext_mut::<Threat>().add(victim, ship, dealt, at) {
        return;
    }
    let Some(here) = w.roster().get(victim).map(|p| p.motion.position) else {
        return;
    };
    let near: Vec<i64> = w
        .npcs()
        .iter()
        .filter(|n| n.id != victim && n.solar_system_id == npc.solar_system_id)
        .filter(|n| n.law.is_none())
        .filter(|n| {
            w.roster().get(n.id).is_some_and(|p| {
                crate::space_state::distance(here, p.motion.position) <= crate::grid::GRID_ENTER
            })
        })
        .map(|n| n.id)
        .collect();
    let t = w.ext_mut::<Threat>();
    for id in near {
        t.add(id, ship, 0.0, at);
    }
    if at >= t.sweep_at {
        let alive: std::collections::BTreeSet<i64> = w.npcs().iter().map(|n| n.id).collect();
        w.ext_mut::<Threat>().sweep(|id| alive.contains(&id), at);
    }
}

/// **The remote-repair hook**: ship `repper` mended ship `target` by
/// `amount` at `at`. If `target` is an aggressor of any rat, the heal is
/// booked to the repper, who becomes an aggressor of those rats.
pub fn assisted(w: &mut Registry, repper: i64, target: i64, amount: f64, at: BlueTime) {
    if repper == target || !amount.is_finite() || amount <= 0.0 {
        return;
    }
    let t = w.ext_mut::<Threat>();
    let rats: Vec<i64> = t
        .hits
        .iter()
        .filter(|(_, l)| {
            l.iter()
                .any(|a| a.ship == target && a.last.saturating_add(AGGRESSION_MEMORY) >= at)
        })
        .map(|(id, _)| *id)
        .collect();
    if rats.is_empty() {
        return;
    }
    let v = t.heals.entry(repper).or_default();
    v.retain(|(when, _)| when.saturating_add(THREAT_WINDOW) >= at);
    v.push((at, amount));
    for id in rats {
        t.add(id, repper, 0.0, at);
    }
}
