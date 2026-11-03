//! **Ships with no pilot** (lane `be-2`, issue #77; spec
//! `handback/r-board-eject/report.md`).
//!
//! `ship.Eject` leaves the hull where it is and puts the pilot in the capsule;
//! `ship.Board` in space takes an abandoned hull. Between the two the hull is a
//! ball with a slim and no `charID`, which this module models.
//!
//! # What the client needs [V]
//!
//! - The Board menu is offered only for a ball whose `isInteractive` is false
//!   (`celestialCheckers.OfferBoardShip`). That is flag bit 0x08
//!   (`findings/destiny-ref/30-attr-isInteractive.bin`: 0x1f -> 0x17), and every
//!   ship ball this server sends has it set, so a pilotless ball is sent as
//!   [`FLAGS_PILOTLESS`] and a ball that gets a pilot is told
//!   `SetBallInteractive(ball, 1)`.
//! - `maxBoardingDistance` is 6550 m (`appConst`).
//!
//! # Where it lives
//!
//! - **The roster**, under the ship's own item id, like a drone: the key is not
//!   a character, so `Registry::pilot` answers for it ([`pilot_of`]) with
//!   `char_id: None`, which is also why no NPC attacks it (they pick players
//!   only) and nobody's sheet is built from it.
//! - **The store**: the hull and everything in it at `(solar system, flag 0)`,
//!   the address a wreck has. It is NOT at its station any more; the flown-ship
//!   convention (a ship in space sits at its pilot's station address) holds only
//!   for a ship somebody flies.
//! - **[`State`]**, a world slot, and **delta kind [`KIND`]**: the owner's
//!   decision of 2026-10-31 is that an abandoned ship stays until it is
//!   destroyed or boarded and stolen, so it survives a restart. The row is
//!   `ship_id -> system x y z abandoned_at corp alliance`; the owner and the
//!   type are read back from the item. [`restore`] runs once from the combat
//!   tick, before the orphan sweeps, and drops a row whose item is gone.
//!
//! The calls are in `service/inventory/board_eject.rs`; the kill of an empty
//! ship is `combat::kill_pilotless`.
//!
//! # Marks
//!
//! `[V]` read in our client (build 3662168); `[A]` assumed, with its
//! falsifier. No `[A]` value is written into a test.
//!
//! - **[A]** the native `SetBallInteractive` takes `(ballID, 0|1)`. Falsifier:
//!   `fci_board_eject` step 4 (a logged `SetBallInteractive failed.`).
//! - **[A]** an abandoned ball carries corporation and alliance -1 on the ball
//!   and the owner's corporation on the slim. Falsifier: an overview that
//!   colours it wrongly.
//! - **[A]** the abandoned ship's position after a restart is where it was left;
//!   it coasts a little after the stop, which the row does not follow.

use crate::damage::BlueTime;
use crate::destiny::{self, Ball, Slim};
use crate::item::Item;
use crate::service::Registry;
use crate::space_state::{Motion, Presence};
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::BTreeMap;

/// The delta kind this module owns (`delta_kinds::KINDS`): `ship_id -> system
/// x y z abandoned_at corp alliance` (alliance `-` for none).
pub const KIND: &str = "pilotless";

/// Flag bit 3, `isInteractive` [V]: set on a ball a pilot flies.
pub const FLAG_IS_INTERACTIVE: u8 = 0x08;

/// The flags byte of a pilotless ship: a ship ball without
/// [`FLAG_IS_INTERACTIVE`] (0x13) [V].
pub const FLAGS_PILOTLESS: u8 = destiny::FLAGS_SHIP_BALL & !FLAG_IS_INTERACTIVE;

/// `maxBoardingDistance`, metres between the two hulls' surfaces [V]
/// (`appConst.pyc:1724`; that `DistanceBetween` is surface to surface is
/// `[I]`, falsifier: a board at 6.6 km refused or at 7 km allowed).
pub const BOARD_RANGE: f64 = 6550.0;

/// How long the capsule's ball stays after its pilot boarded a ship, so the
/// client has moved its ego first (`gameui.DoBallAdd`, `bp.ego`). **[A]** one
/// second; falsifier: a client traceback in `GetMyPosition`, or a pod
/// bracket visible well past the delay.
pub const POD_REMOVE_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// One abandoned ship.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Abandoned {
    pub ship_id: i64,
    pub type_id: i64,
    /// Who owns the hull (and what is in it).
    pub owner_id: i64,
    pub corp_id: i64,
    pub alliance_id: Option<i64>,
    pub system: i64,
    pub abandoned_at: BlueTime,
}

/// Every abandoned ship of this run, by ship id.
#[derive(Debug, Default)]
pub struct State {
    ships: BTreeMap<i64, Abandoned>,
    /// [`restore`] has run.
    restored: bool,
}

impl State {
    pub fn get(&self, ship_id: i64) -> Option<&Abandoned> {
        self.ships.get(&ship_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Abandoned> {
        self.ships.values()
    }

    pub fn len(&self) -> usize {
        self.ships.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ships.is_empty()
    }
}

/// The world slot (`world_ext::SLOTS`, coordinator row).
pub const SLOT: SlotSpec = SlotSpec {
    name: "pilotless",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(State::default())
}

fn state(w: &Registry) -> &State {
    w.ext::<State>()
}

fn state_mut(w: &mut Registry) -> &mut State {
    w.ext_mut::<State>()
}

/// The abandoned ship `ship_id`, if it is one.
pub fn get(w: &Registry, ship_id: i64) -> Option<Abandoned> {
    state(w).get(ship_id).copied()
}

/// Whether `ship_id` is an abandoned ship.
pub fn is_pilotless(w: &Registry, ship_id: i64) -> bool {
    state(w).get(ship_id).is_some()
}

/// `Registry::pilot`'s arm: an abandoned ship is in the roster under its own
/// id, flown by nobody, owned by a character.
pub fn pilot_of(w: &Registry, roster_key: i64) -> Option<crate::npc::Pilot> {
    let a = state(w).get(roster_key)?;
    Some(crate::npc::Pilot {
        ship_type_id: a.type_id,
        owner_id: a.owner_id,
        corporation_id: a.corp_id,
        alliance_id: a.alliance_id,
        char_id: None,
    })
}

/// An abandoned ship's ball: an ordinary ship ball with the interactive bit
/// clear and no corporation or alliance [A].
pub fn ball(w: &Registry, a: &Abandoned, motion: &Motion) -> Ball {
    let mut b = destiny::pilot_ball(w, a.ship_id, a.type_id, -1, -1, motion);
    b.flags = FLAGS_PILOTLESS;
    b
}

/// An abandoned ship's slim: no `charID`, the owner's corporation.
pub fn slim(a: &Abandoned) -> Slim {
    Slim {
        item_id: a.ship_id,
        type_id: a.type_id,
        owner_id: a.owner_id,
        char_id: None,
        corp_id: Some(a.corp_id),
        response_thresholds: None,
    }
}

// ---------------------------------------------------------------------------
// The table and its rows
// ---------------------------------------------------------------------------

fn key(ship_id: i64) -> Vec<String> {
    vec![ship_id.to_string()]
}

fn write_row(w: &mut Registry, a: &Abandoned, at: [f64; 3]) {
    w.deltas_mut().put(
        KIND,
        key(a.ship_id),
        vec![
            a.system.to_string(),
            at[0].to_string(),
            at[1].to_string(),
            at[2].to_string(),
            a.abandoned_at.to_string(),
            a.corp_id.to_string(),
            a.alliance_id.map_or("-".to_owned(), |x| x.to_string()),
        ],
    );
}

/// A stored row: `(system, position, abandoned_at, corp, alliance)`.
type Row = (i64, [f64; 3], BlueTime, i64, Option<i64>);

/// A stored row with its key, parsed or not.
type Parsed = (Vec<String>, Option<(i64, Row)>);

fn parse_row(v: &[String]) -> Option<Row> {
    let n = |i: usize| v.get(i)?.parse::<i64>().ok();
    let f = |i: usize| v.get(i)?.parse::<f64>().ok().filter(|x| x.is_finite());
    Some((
        n(0)?,
        [f(1)?, f(2)?, f(3)?],
        n(4)?,
        n(5)?,
        v.get(6).and_then(|s| s.parse().ok()),
    ))
}

/// **A ship stays in space with nobody in it**: the record, the row that
/// outlives a restart, and the roster entry under the ship's id. The caller
/// has moved the hull to `(system, 0)` in the store and brought `motion` up
/// to now.
pub fn abandon(w: &mut Registry, a: Abandoned, motion: Motion) {
    state_mut(w).ships.insert(a.ship_id, a);
    write_row(w, &a, motion.position);
    w.roster_mut()
        .arrive(Presence::arriving(a.ship_id, a.ship_id, a.system, motion));
}

/// The ship is no longer abandoned (boarded, or dead): the record and the row
/// go, and so does the roster entry under its id. The record, if there was
/// one. The caller tells the park.
pub fn release(w: &mut Registry, ship_id: i64) -> Option<Abandoned> {
    let a = state_mut(w).ships.remove(&ship_id);
    if w.deltas().of(KIND).contains_key(&key(ship_id)) {
        w.deltas_mut().del(KIND, &key(ship_id));
    }
    // The roster key is the ship id only while it is abandoned; a pilot's
    // key is a character, never an item id.
    if a.is_some() {
        w.roster_mut().leave(ship_id);
    }
    a
}

/// Bring every abandoned ship back after a restart, once per process: a
/// record and a roster entry for each row whose hull is still in the store at
/// `(system, flag 0)`. A row whose hull is gone, or is no longer there, is
/// dropped. Called from the combat tick's first pass (`combat::tick`).
pub fn restore(w: &mut Registry) {
    if state(w).restored {
        return;
    }
    state_mut(w).restored = true;
    let rows: Vec<Parsed> = w
        .deltas()
        .of(KIND)
        .iter()
        .map(|(k, v)| {
            let parsed = k
                .first()
                .and_then(|s| s.parse::<i64>().ok())
                .zip(parse_row(v));
            (k.clone(), parsed)
        })
        .collect();
    for (k, parsed) in rows {
        // A row that does not parse is junk.
        let Some((id, (system, at, abandoned_at, corp_id, alliance_id))) = parsed else {
            w.deltas_mut().del(KIND, &k);
            continue;
        };
        let item = w
            .item(id)
            .filter(|i| is_abandonable(i) && i.location_id == system)
            .cloned();
        let Some(item) = item else {
            w.deltas_mut().del(KIND, &k);
            continue;
        };
        let a = Abandoned {
            ship_id: id,
            type_id: item.type_id,
            owner_id: item.owner_id,
            corp_id,
            alliance_id,
            system,
            abandoned_at,
        };
        state_mut(w).ships.insert(id, a);
        w.roster_mut().arrive(Presence::arriving(
            id,
            id,
            system,
            Motion::arrived(at, [0.0; 3]),
        ));
    }
}

/// A hull that can be abandoned: a singleton ship.
pub fn is_abandonable(item: &Item) -> bool {
    item.category_id == crate::model::CATEGORY_SHIP && item.singleton
}

#[cfg(test)]
mod tests;
