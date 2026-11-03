//! **Who the wing shoots, and the wing's logi** (plan-fc-follow, lane FC-2;
//! `docs/lanes/fc-2.md`).
//!
//! On the FC's grid a wing bot runs the escort's fight ([`crate::escort`]),
//! but *which* ship it fights is chosen here, in this order ([`pick_target`],
//! pure):
//!
//! 1. **The FC's fleet tags** on the grid, in the order the client's menu
//!    offers them ([`rank`]; `0`..`9`, `A`..`J`, `X`..`Z`) **[A]**. A tag is
//!    the FC's word, so it is followed in every stance.
//! 2. **A contest**: a stranger's Entosis Link running while our linkers link,
//!    or in a system our alliance holds. Not under [`Stance::Hold`].
//! 3. **A defence**: a pilot who shoots the FC, a wing mate or a linker
//!    ([`crate::threat::Cause::Shot`] / [`Mate`](crate::threat::Cause::Mate)
//!    from the driver's threat scene). Not under [`Stance::Hold`].
//! 4. **Free** only: the nearest hostile pilot to the FC, but only where the
//!    law lets us (security below 0.0) or against a war target **[A]**.
//! 5. Otherwise nothing is chosen, and the escort's own fight takes the NPCs
//!    on or near the FC (its `pick_target` with the FC as boss). Under
//!    [`Stance::Hold`] that fight is silenced: the wing only keeps its place.
//!
//! The FC's Target *broadcast* (the primary) is handled before any role rule,
//! by [`crate::fleet::engage`].
//!
//! The **winglogi** row ([`LOGI_ROLE`]) is the wing with the logi's two hooks:
//! the remote repairer is fitted docked ([`crate::logi::fit`]), and in space
//! the repair plan runs ahead of the wing's turn with the FC and the wing
//! mates as wards, minus any ship that is linking (`logi::wards`).
//!
//! Every number and rule that is not read off the client is **[A]** with its
//! falsifier in the doc comment; none is asserted by a test.

use crate::brain::{Act, Ball, Hostile, Memory, Observation, Place, RoleRef};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::threat::Cause;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// What the wing may shoot on its own account **[A]**: the default is
/// [`Stance::Defend`]. Falsified by an owner whose wing sits on its hands
/// while he is shot (use Defend) or opens fire on a pilot he meant to spare
/// (use Hold). The FC changes it by command (FC-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stance {
    /// Tags, contests and whoever shoots the wing.
    #[default]
    Defend,
    /// Tags only: the FC's explicit marks, and nothing of its own.
    Hold,
    /// Defend, and the nearest hostile pilot to the FC where it is lawful.
    Free,
}

impl Stance {
    pub fn word(self) -> &'static str {
        match self {
            Stance::Defend => "defend",
            Stance::Hold => "hold",
            Stance::Free => "free",
        }
    }
}

/// Why a ship was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Tag,
    Contest,
    Defend,
    Free,
}

/// A ship chosen to be shot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    pub ball: i64,
    pub why: Why,
}

/// One ship on the grid that could be a target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    pub ball: Ball,
    /// The pilot, for a player's ship; `None` for an NPC.
    pub char_id: Option<i64>,
    /// An NPC, or a pilot the relations call hostile (never an ally or the
    /// FC).
    pub hostile: bool,
    /// A war of ours makes this pilot lawful prey in high sec.
    pub war: bool,
    /// Running an Entosis Link now.
    pub linking: bool,
}

/// Everything [`pick_target`] reads.
pub struct Picture<'a> {
    pub stance: Stance,
    /// The fleet's tags, `item -> label`.
    pub tags: &'a BTreeMap<i64, String>,
    pub contacts: &'a [Contact],
    /// The FC's ship, when it is on the grid.
    pub fc: Option<Ball>,
    /// The pilots shooting the FC, a mate or a linker (the threat scene's
    /// `Shot` and `Mate`).
    pub foes: &'a [i64],
    /// One of ours (the bot, a mate) runs a link now.
    pub our_linking: bool,
    /// This system is held by our alliance.
    pub held: bool,
    /// This system's security; `None`: unknown.
    pub security: Option<f64>,
}

/// The tags the client's menu offers, in its order (`menusvc.py:1926-1931`
/// **[V]**; the server's `fleet::consts::TARGET_TAGS`, which is crate-private).
const TARGET_TAGS: [&str; 23] = [
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "A", "B", "C", "D", "E", "F", "G", "H", "I",
    "J", "X", "Y", "Z",
];

/// A tag's place in the menu order (`0`..`9`, `A`..`J`, `X`, `Y`, `Z`);
/// a label the menu never offers sorts last.
pub fn rank(tag: &str) -> usize {
    TARGET_TAGS
        .iter()
        .position(|t| *t == tag)
        .unwrap_or(TARGET_TAGS.len())
}

/// **The wing's target** (pure; module header). `None`: nothing of the wing's
/// own, and the escort's fight (NPCs on the FC) decides.
pub fn pick_target(p: &Picture) -> Option<Pick> {
    let gap = |c: &Contact| p.fc.map_or(0.0, |f| f.surface_to(&c.ball));
    let nearest = |list: Vec<&Contact>, why: Why| {
        list.into_iter()
            .min_by(|a, b| gap(a).total_cmp(&gap(b)).then(a.ball.id.cmp(&b.ball.id)))
            .map(|c| Pick {
                ball: c.ball.id,
                why,
            })
    };
    // 1. The FC's tags.
    if let Some(c) = p
        .contacts
        .iter()
        .filter(|c| c.hostile)
        .filter_map(|c| Some((rank(p.tags.get(&c.ball.id)?), c)))
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.ball.id.cmp(&b.1.ball.id)))
        .map(|(_, c)| c)
    {
        return Some(Pick {
            ball: c.ball.id,
            why: Why::Tag,
        });
    }
    if p.stance == Stance::Hold {
        return None;
    }
    // 2. A stranger's link while we link, or on our own sov.
    if p.our_linking || p.held {
        let pick = nearest(
            p.contacts
                .iter()
                .filter(|c| c.hostile && c.linking)
                .collect(),
            Why::Contest,
        );
        if pick.is_some() {
            return pick;
        }
    }
    // 3. Whoever shoots the FC, a mate or a linker.
    let pick = nearest(
        p.contacts
            .iter()
            .filter(|c| c.hostile && c.char_id.is_some_and(|ch| p.foes.contains(&ch)))
            .collect(),
        Why::Defend,
    );
    if pick.is_some() {
        return pick;
    }
    // 4. Free: the nearest hostile pilot, where it is lawful.
    if p.stance == Stance::Free {
        let lawless = p.security.is_some_and(|s| s < 0.0);
        return nearest(
            p.contacts
                .iter()
                .filter(|c| c.hostile && c.char_id.is_some() && (lawless || c.war))
                .collect(),
            Why::Free,
        );
    }
    None
}

/// **The fleet's tags from an `OnFleetStateChange` / `GetFleetState` value**
/// (pure): a `KeyVal` whose `targetTags` is `{itemID: label}`, wired as
/// `(label, itemID)` pairs like every id-keyed dict here
/// (`fleet.rs::state_value`). `None`: not a fleet state.
pub fn parse_tags(v: &PyValue) -> Option<BTreeMap<i64, String>> {
    let PyValue::Dict(entries) = crate::attributes(v) else {
        return None;
    };
    let (tags, _) = entries
        .iter()
        .find(|(_, k)| crate::text(k).as_deref() == Some("targetTags"))?;
    let PyValue::Dict(pairs) = tags else {
        // An empty map may come as nothing at all.
        return Some(BTreeMap::new());
    };
    Some(
        pairs
            .iter()
            .filter_map(|(label, item)| match item {
                PyValue::Int(i) => Some((*i, crate::text(label)?)),
                _ => None,
            })
            .collect(),
    )
}

// ---- reading the observation ------------------------------------------------------------------

/// The grid's candidates: the park's NPCs and the hunt scene's pilots.
fn contacts(o: &Observation) -> Vec<Contact> {
    let mut out = Vec::new();
    if let Some(park) = &o.park {
        for h in &park.hostiles {
            out.push(Contact {
                ball: h.ball,
                char_id: None,
                hostile: true,
                war: false,
                linking: o.wing.links.contains_key(&h.ball.id),
            });
        }
    }
    for p in &o.hunt.pilots {
        out.push(Contact {
            ball: p.ball,
            char_id: Some(p.char_id),
            hostile: p.hostile,
            war: p.war,
            linking: o.wing.links.contains_key(&p.ball.id),
        });
    }
    out
}

/// The wing's pick on this observation.
pub fn pick(m: &Memory, o: &Observation) -> Option<Pick> {
    let park = o.park.as_ref()?;
    let contacts = contacts(o);
    let foes: Vec<i64> = o
        .threat
        .foes
        .iter()
        .filter(|f| matches!(f.cause, Cause::Shot | Cause::Mate))
        .map(|f| f.char_id)
        .collect();
    let security = match o.place {
        Place::InSpace { system, .. } => {
            m.policy.travel.as_deref().and_then(|t| t.security(system))
        }
        _ => None,
    };
    let ours = |id: i64| id == park.ego.id || o.logi.mates.iter().any(|b| b.id == id);
    pick_target(&Picture {
        stance: m.wing.stance,
        tags: &o.wing.tags,
        contacts: &contacts,
        fc: park.boss,
        foes: &foes,
        our_linking: o.wing.links.keys().any(|s| ours(*s)),
        held: o.wing.held,
        security,
    })
}

/// The pick as a target the escort's fight understands.
fn hostile_of(o: &Observation, ball: i64) -> Option<Hostile> {
    if let Some(h) = o.park.as_ref()?.hostiles.iter().find(|h| h.ball.id == ball) {
        return Some(h.clone());
    }
    let p = o.hunt.pilots.iter().find(|p| p.ball.id == ball)?;
    Some(Hostile {
        ball: p.ball,
        type_id: p.type_id.unwrap_or(0),
        name: crate::hunt::PREY_NAME.to_owned(),
        shooting: None,
    })
}

/// **The wing on the FC's grid**: the escort's turn, shown the wing's pick as
/// the only hostile (as [`crate::fleet::engage`] shows it a primary); with no
/// pick the escort's own fight (NPCs on or near the FC); under
/// [`Stance::Hold`] with no tag, the escort's turn with no hostiles at all.
pub fn on_grid(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if let Some(p) = pick(m, o)
        && !o.guns.is_empty()
        && let Some(target) = hostile_of(o, p.ball)
    {
        // The FC changed the mark: drop the engaged one quietly, not as a
        // kill the escort would announce.
        if m.target.as_ref().is_some_and(|(id, _)| {
            *id != target.ball.id
                && o.park
                    .as_ref()
                    .is_some_and(|p| p.hostiles.iter().any(|h| h.ball.id == *id))
        }) {
            m.target = None;
        }
        let mut seen = o.clone();
        if let Some(park) = &mut seen.park {
            park.hostiles = vec![target];
        }
        return crate::escort::in_space(m, &seen);
    }
    if m.wing.stance == Stance::Hold {
        let mut seen = o.clone();
        if let Some(park) = &mut seen.park {
            park.hostiles.clear();
        }
        return crate::escort::in_space(m, &seen);
    }
    crate::escort::in_space(m, o)
}

// ---- the winglogi role --------------------------------------------------------------------------

/// A wing bot that carries a remote repairer for the FC and the wing.
pub static LOGI_ROLE: RoleSpec = RoleSpec {
    name: "winglogi",
    greeting: "o7 -- repairing for the wing, FC",
    civilian: false,
    docked: logi_docked,
    in_space: logi_in_space,
    banner,
    setup,
};

/// Whether `role` is a wing row (`wing` or `winglogi`).
pub fn is_wing(role: RoleRef) -> bool {
    std::ptr::eq(role, &crate::wing::ROLE) || std::ptr::eq(role, &LOGI_ROLE)
}

/// Docked: the repairer is fitted first, then the wing's kit turn.
fn logi_docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if let Some(acts) = crate::logi::fit(m, o) {
        return acts;
    }
    crate::wing::docked(m, o)
}

/// In space: the repair plan ahead of the wing's turn, as
/// [`crate::logi::in_space`] puts it ahead of the escort's.
fn logi_in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let turn = crate::wing::in_space(m, o);
    let Some(park) = &o.park else { return turn };
    if m.homing.is_some() {
        return turn;
    }
    let called = m.heal.filter(|c| o.now < c.until);
    let mut acts = crate::logi::plan_called(o, park, called);
    if acts.is_empty() {
        return turn;
    }
    acts.extend(turn);
    acts
}

fn banner(s: &Summary) -> String {
    (crate::logi::ROLE.banner)(s)
}

fn setup(lacks: Lacks) -> Vec<String> {
    (crate::logi::ROLE.setup)(lacks)
}

#[cfg(test)]
mod tests;
