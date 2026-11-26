//! What the world owes the client, and how each one becomes a frame.
//!
//! **Five subsystems' worth of mutators discard their return value entirely**
//! (`POP_TOP` on the result; see `docs/archive/roadmap.md`), so a server that only
//! answers calls is a server whose effects are invisible. A push is how the
//! client finds out at all.
//!
//! ## Why this is a trait and not an enum
//!
//! It *was* an enum -- `service::Notify`, with a 170-line `match` in
//! `dispatch.rs` turning each variant into `(event, args)`. That shape made
//! **every new push in the project an edit to two hot files**, which
//! `docs/lanes/README.md` rule 2 names as the thing to remove rather than
//! schedule: the shared file is the finding. Five of the six remaining docked
//! subsystems are gated on a push, and all five of round 2's remaining
//! increments send one, so the cap was about to bind hard.
//!
//! One variant already escaped it. `Notify::Fleet` carried a
//! `fleet::FleetEvent` and the dispatcher delegated with `event.to_push()`,
//! which put twelve notifications' shapes, evidence and arity tests beside the
//! subsystem that knew about them. [`Push`] is that precedent applied to all of
//! them: `Registry::push` takes anything implementing it,
//! `Registry::take_pending` hands back boxes, and `dispatch::pending_pushes`
//! is a loop with no `match` at all. **A new push is now a new type in the
//! module that owns it, and zero edits here.**
//!
//! ## Where the implementations live
//!
//! With the subsystem, wherever the subsystem is the thing that knows the
//! shape: [`crate::fleet::Push`], [`crate::destiny::StateUpdate`],
//! [`crate::skill::ForcedRefresh`], [`crate::mission::AgentMissionChange`].
//!
//! The five in this file are the ones whose owning module does not know what a
//! `PyValue` is -- `item.rs`, `ledger.rs` and `industry.rs` are domain models
//! with no wire in them, and dragging marshal types into them to hold one
//! struct would be the wrong trade. They live here until their module grows a
//! wire shape of its own, and because the dispatcher no longer matches on
//! them, moving one is a two-file change that nothing else notices.

use eve_marshal::PyValue;

/// Something that happened in the world and that the client has to be *told*.
///
/// Implementors name the event in **machoNet** terms -- the `On*` method the
/// client's `sm.ScatterEvent` will look for -- and carry the argument list for
/// it. Turning that pair into a packet is still `dispatch.rs`'s job; what
/// changed is that it no longer needs to know which pushes exist.
///
/// `Debug` is required because the tests print the queue when an assertion
/// about its contents fails, and a queue of trait objects that cannot be
/// printed makes those failures unreadable. `Send + Sync` because the queue
/// hangs off `Registry`, which `docs/reference/world-state.md` has going behind an
/// `RwLock` shared by every connection.
pub trait Push: std::fmt::Debug + Send + Sync {
    /// The event name and its arguments, in the order the handler unpacks
    /// them.
    fn to_push(&self) -> (&'static str, Vec<PyValue>);

    /// **What goes on the wire**, when that is not [`Self::to_push`] itself:
    /// the event a client is sent, while `to_push` stays the event the push
    /// *is* (what the server's own readers, the logs and the tests match on).
    /// The default is `to_push`. The one override is
    /// `crate::targeting::TargetChange`, which a live server delivers inside
    /// an `OnMultiEvent` batch rather than on its own.
    fn to_wire(&self) -> (&'static str, Vec<PyValue>) {
        self.to_push()
    }

    /// **Who the notification is for**, when it is not the caller.
    ///
    /// Every notification shipped before fleets told the actor about their own
    /// action, so this was always the acting session and never needed saying
    /// -- see `docs/lanes/fleet.md` §4, which found that every frame
    /// `pending_pushes` built was addressed to `self.session.client_id`, so a
    /// push whose audience was another character was delivered to the caller
    /// instead.
    ///
    /// `None` means the acting session. A list means **character** ids, not
    /// client ids: a character may have two sessions
    /// (`crate::session_table`), and choosing which to write to is the
    /// transport's business.
    fn audience(&self) -> Option<Vec<i64>> {
        None
    }

    /// Take a Destiny stamp issued at **send** time, for a push that was
    /// built earlier.
    ///
    /// **This is what makes a scheduled Destiny batch possible, and it is
    /// simpler than the mechanism `docs/port-tracker.md` §30.4 went looking
    /// for.** That note concluded a delayed batch needed "a stamp allocator
    /// that can mint a future stamp without moving the floor", because a
    /// batch built now and sent in sixty seconds would otherwise be stamped
    /// sixty seconds in the past -- and a Destiny stamp must be monotonic
    /// non-decreasing for ever, so a backwards one is either dropped or sends
    /// `SynchroniseToSimulationTime` rewinding.
    ///
    /// The allocator is unnecessary. The premise to attack is not *which*
    /// future stamp to mint, it is **when the stamp is chosen at all**: bind
    /// it at the moment the batch leaves instead of the moment it is built,
    /// and the value wanted is just `Registry::next_destiny_stamp()` -- now,
    /// monotonic by construction, with the floor moving exactly as it always
    /// does. Nothing has to mint a future anything.
    ///
    /// The default is a no-op, which is right for every push that carries no
    /// stamp: an `OnItemChange` or a fleet invite has no simulation time in
    /// it.
    ///
    /// **Every push that carries a stamp must override it**, and two of the
    /// five carry *two* clocks -- `BallsAdded` and `StateUpdate` write the
    /// stamp into the blob's header as well, and a batch whose envelope and
    /// blob disagree starts the client's park out of step with every action
    /// that follows. `destiny::every_stamped_push_can_be_restamped` is the
    /// forcing function: it is exhaustive, so a new stamped push that forgets
    /// this stops it compiling rather than producing a batch dated minutes in
    /// the past the first time somebody schedules one.
    fn restamp(&mut self, _stamp: u32) {}

    /// **What this push does to a client's park**, for the grid's filter at
    /// the two drains (`crate::grid::filter`, GRID-B).
    ///
    /// The overview lists every ball in the park and nothing else
    /// (`docs/lanes/grid-research.md` §0.1), so the server alone decides
    /// what is on a pilot's grid, and it decides it here: a push that names
    /// a ball reaches only the clients whose park holds it, an `AddBalls2`
    /// only the clients whose grid the ball is on. The default,
    /// [`crate::grid::GridRole::None`], leaves the audience alone, which is
    /// right for everything that is not a Destiny entry.
    ///
    /// **Every `DoDestinyUpdate` must override it.** The drain refuses one
    /// that does not in a debug build, and
    /// `grid::tests::every_destiny_push_declares_its_grid_role` lists them,
    /// so a new Destiny push cannot reach every pilot in a system by
    /// forgetting this.
    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::None
    }

    /// The same push holding only the balls `keep` admits, addressed `to`:
    /// how one batch about several balls -- the NPCs' moves of a tick, a
    /// warm system's rocks -- reaches a pilot who holds some of them.
    /// `None` when nothing is left, and the default: a push about one ball
    /// is never split.
    fn narrowed(&self, _to: Vec<i64>, _keep: &dyn Fn(i64) -> bool) -> Option<Box<dyn Push>> {
        None
    }

    /// **What a later decision may recall this push as**, without building
    /// its arguments (lane fight-stall-fix, step 4).
    ///
    /// `Registry::cancel_scheduled` used to call `to_push` on every queued
    /// entry just to read the event name, so a 200-ship warp burst cost
    /// dropped locks x queue length x a full `PyValue` build (a grid's
    /// `BallsAdded` builds a dict per ball). A push that one of the recall
    /// sites names ([`CancelTag`]) says so here; every other push is `None`
    /// and is skipped at the cost of a call. A tag must agree with what the
    /// old test on `to_push` and `audience` said; the oracles in
    /// `service/space.rs` and `targeting.rs` pin that for each.
    fn cancel_tag(&self) -> Option<CancelTag> {
        None
    }

    /// **Whether this scheduled push is still true when it leaves.** The
    /// clock asks at delivery and drops a push that says `false`
    /// (`Registry::take_due`). For a notification about a state that may end
    /// while it waits: an `'otheradd'` queued behind an acquisition delay
    /// must not land after the `'otherlost'` that undid it (RAT-GHOST).
    fn still_stands(&self, _w: &crate::service::Registry) -> bool {
        true
    }
}

/// The scheduled pushes a recall site looks for; see [`Push::cancel_tag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelTag {
    /// A warp's arrival reconciliation for `ball`: the observers' batch
    /// (`SetBallPosition`, `SetBallVelocity`, `Stop`). A human pilot is sent
    /// none (the client lands itself).
    WarpSettle { ball: i64 },
    /// `OnTarget('add', ship)` to the one character `to`.
    TargetAdd { to: i64, ship: i64 },
    /// `OnTarget('otheradd', ship)` to the one character `to`.
    TargetOtherAdd { to: i64, ship: i64 },
    /// A module's stop pair (`OnGodmaShipEffect` and `OnSpecialFX`, start 0)
    /// booked for a cycle boundary. A new run of that module makes it stale.
    ModuleStop { module: i64 },
}

/// `OnAccountChange(accountKey, ownerID, balance)`.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountChange {
    pub owner_id: i64,
    pub balance: f64,
}

impl AccountChange {
    /// Kept for the callers that name the owner; the audience is the owner
    /// already ([`Push::audience`]).
    pub fn to_owner(self) -> Addressed {
        Addressed {
            to: Some(vec![self.owner_id]),
            inner: Box::new(self),
        }
    }
}

impl Push for AccountChange {
    /// The wallet's own character. A wallet push with no audience would mean
    /// "the caller", and the clock or another player's turn is not the owner.
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.owner_id])
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnAccountChange",
            vec![
                // The account key is the **name**, not the integer
                // `appConst.accountingKeyCash`. `WalletSvc.OnAccountChange`
                // tests `accountKey == 'cash'`, and an integer there passes
                // every guard in the method and updates nothing: the
                // `_OnCharAccountChange` branch is skipped, the neocom does
                // not blink, `_wealth` keeps the old value, and the only
                // visible effect is the local re-scatter reaching handlers
                // that do nothing with it.
                PyValue::Bytes(b"cash".to_vec()),
                // `session.charid`, not `session.userid`:
                // `if accountKey == 'cash' and ownerID == session.charid`.
                // The other branch of that `if` is the corp wallet, keyed by
                // `session.corpid`.
                PyValue::Int(self.owner_id),
                // **Not `None`.** The whole method is wrapped in
                // `if balance is not None`, so a null balance reaches the
                // local scatter and nothing else -- no blink, no popup, no
                // cache update, and no error either.
                PyValue::Real(self.balance),
            ],
        )
    }
}

/// `OnItemChange(item, change, location)` -- the push that redraws.
///
/// **The one that makes a mutator's effect visible in a window that is already
/// open**, where [`InvalidateLocations`] only drops the container so the *next*
/// build re-asks. It is the gate for 618 call sites' worth of mutators that
/// discard their return value entirely.
///
/// Three parts, and each has a trap:
///
/// - **`item` must be a real `blue.DBRow`.** Two places construct from it with
///   `blue.DBRow(item)` -- `invCache.OnItemChange` and
///   `invCacheContainer._ProcessItemChange` -- so a `KeyVal` or a `Row` raises
///   in native code. It is packed against `Registry::item_row`, whose eleven
///   columns are in the exact order the integer indices below require; that
///   order was wrong until it was checked.
/// - **`change` is keyed by column *index*, not by name**, because the
///   handlers do `oldItem[k] = v` with `k` an integer.
/// - **`location` is the badging storage, or `None`.** It is not the item's
///   location. `neocomInventoryBadging.process_item_change` reads it: it
///   records an unseen item only through
///   `mark_as_unseen_in_location(item, location)`, which does nothing for a
///   falsy location. Every other handler sets its own from `self.key[0]` or
///   passes it through untouched (`findings/push-events.tsv`), so a tuple is
///   safe for them. See [`badging_location`] for the tuple.
///
/// The row arrives already packed, rather than as a `crate::item::Item`,
/// because `Registry::item_row` is the thing that knows the layout and the
/// dispatcher is no longer allowed to ask it anything.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemChange {
    pub row: PyValue,
    /// `(column index, previous value)`, from
    /// [`crate::item::Change::previous_columns`].
    pub change: Vec<(i64, i64)>,
    /// Third argument: [`badging_location`], or `PyValue::None`.
    pub location: PyValue,
}

impl ItemChange {
    /// An item that did not exist before: `{ixLocationID: 0}`.
    pub fn spawned(row: PyValue) -> Self {
        ItemChange {
            row,
            change: vec![(crate::item::IX_LOCATION_ID, 0)],
            location: PyValue::None,
        }
    }

    /// Name the badging storage the item now sits in.
    pub fn with_location(mut self, location: PyValue) -> Self {
        self.location = location;
        self
    }
}

/// The badging storage an item now sits in, as
/// `neocomInventoryBadging._get_item_location` builds it [V]:
/// `('Station', stationID, 'StationShips' | 'StationItems')` for a station
/// hangar (flag 4) the receiver is docked in, else `None`.
///
/// Only when docked there: with no storage yet for the named station the
/// client falls back to the *current* station's, and the item would be
/// marked unseen there (a phantom dot). A remote delivery is told by
/// `OnMarketItemReceived` instead.
///
/// No structure branch: we have no structure docking. When there is one, the
/// client's shapes are `('Structure', id, 'StructureItemHangar' |
/// 'StructureShipHangar')`. Ship cargo (flag 5, `('Ship', loc, 'ShipCargo')`)
/// stays `None`: nothing needs it.
pub fn badging_location(
    flag_id: i64,
    category_id: i64,
    location_id: i64,
    docked_station: Option<i64>,
) -> PyValue {
    if flag_id != crate::model::FLAG_HANGAR || docked_station != Some(location_id) {
        return PyValue::None;
    }
    let inventory: &[u8] = if category_id == crate::model::CATEGORY_SHIP {
        b"StationShips"
    } else {
        b"StationItems"
    };
    PyValue::Tuple(vec![
        PyValue::Bytes(b"Station".to_vec()),
        PyValue::Int(location_id),
        PyValue::Bytes(inventory.to_vec()),
    ])
}

impl Push for ItemChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnItemChange",
            vec![
                self.row.clone(),
                // `change` is keyed by **integer column index**: the handlers
                // do `oldItem[k] = v`, and rebuild the previous row from it
                // alone -- so it has to say where the item really was. See
                // `Change::previous_columns`, and `docs/lanes/fitting.md` §4
                // for the three readers that kept a moved item in its old
                // window while this was always `{3: 0}`.
                //
                // `(value, key)` -- marshal dict entries are inverted, so
                // `{3: 0}` is `(0, 3)` and **not** `(3, 0)`. `pyutil::pydict`
                // encodes the inversion once but only takes `&str` keys, and
                // these keys are integers, so it is written out here. Getting
                // it backwards would send `{ixItemID: 3}` -- a quiet
                // instruction to the client to rewrite the item's own id.
                PyValue::Dict(
                    self.change
                        .iter()
                        .map(|(k, v)| (PyValue::Int(*v), PyValue::Int(*k)))
                        .collect(),
                ),
                // Not the item's location: the badging storage or `None`.
                self.location.clone(),
            ],
        )
    }

    /// **On the wire it is `OnItemsChanged([row], change, location)`**, as a
    /// live server sends every item change: 764 `OnItemsChanged` in a
    /// recorded new-character session and not one `OnItemChange` **[V-TQ]**.
    /// `invCache.OnItemsChanged` validates the location cache, scatters
    /// `OnItemChange(row, change, location)` for each row -- the handler the
    /// bare push reached, unchanged -- and then `OnMultipleItemChange(items,
    /// change)` **[V]** (decompiled 3707965), which only that path raises and
    /// which the node graphs' `Inventory Items Changed` event (atom 69)
    /// hears: the AIR tutorial's task 2.1.05 completes on it once the black
    /// box is in the hold.
    fn to_wire(&self) -> (&'static str, Vec<PyValue>) {
        let (_, mut args) = self.to_push();
        args[0] = PyValue::List(vec![args[0].clone()]);
        ("OnItemsChanged", args)
    }
}

/// Another push, readdressed: how the store's pushes leave a clock tick.
///
/// `Registry::apply_items` is the house path for item pushes and puts them
/// on `pending` with **no audience** -- right inside a call, where that means
/// "the caller". A death has no caller: left there, the victim's item rows
/// would go to whichever client spoke next. So `combat::kill` takes them back off
/// `pending` and schedules them here, `to` the victim. `to: None` keeps the
/// inner push's own audience (used to put back what was on `pending` before).
#[derive(Debug)]
pub struct Addressed {
    pub to: Option<Vec<i64>>,
    pub inner: Box<dyn Push>,
}

impl Push for Addressed {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        self.inner.to_push()
    }

    fn to_wire(&self) -> (&'static str, Vec<PyValue>) {
        self.inner.to_wire()
    }

    fn audience(&self) -> Option<Vec<i64>> {
        self.to.clone().or_else(|| self.inner.audience())
    }

    fn restamp(&mut self, stamp: u32) {
        self.inner.restamp(stamp);
    }

    fn grid(&self) -> crate::grid::GridRole {
        self.inner.grid()
    }

    fn narrowed(&self, to: Vec<i64>, keep: &dyn Fn(i64) -> bool) -> Option<Box<dyn Push>> {
        self.inner.narrowed(to, keep)
    }

    /// The inner push's tag, readdressed: a target tag names its one
    /// recipient, which is `to` here when it is set (and then must be one).
    /// A readdressed push whose inner push has no tag is not tagged, which
    /// differs from the old test only for a target change that was sent to
    /// several and readdressed to one; nothing builds that (`Addressed`
    /// carries item pushes).
    fn still_stands(&self, w: &crate::service::Registry) -> bool {
        self.inner.still_stands(w)
    }

    fn cancel_tag(&self) -> Option<CancelTag> {
        let tag = self.inner.cancel_tag()?;
        let Some(to) = &self.to else { return Some(tag) };
        let [one] = to[..] else { return None };
        Some(match tag {
            CancelTag::WarpSettle { .. } | CancelTag::ModuleStop { .. } => tag,
            CancelTag::TargetAdd { ship, .. } => CancelTag::TargetAdd { to: one, ship },
            CancelTag::TargetOtherAdd { ship, .. } => CancelTag::TargetOtherAdd { to: one, ship },
        })
    }
}

/// `OnInvalidateCacheForLocations([locationID, ...])`.
///
/// **The cheapest inventory push there is**, and the reason it is the one we
/// send first. The handler is two lines --
///
/// `OnInvalidateCacheForLocations` (`[V]`) calls
/// `InvalidateLocationCache` for each location id it is given.
///
/// -- and `InvalidateLocationCache` drops `self.inventories[(locID, None)]`
/// inside `except KeyError: pass`. So it needs no `DBRow`, no descriptor and
/// no `change` dict, it is idempotent, and a location the client has never
/// cached is harmless.
///
/// It is necessary rather than merely nice: `invCacheContainer.listed` has
/// **no TTL**, so closing and reopening the inventory window emits no call at
/// all and a spawned item stays invisible until a dock, an undock or a
/// restart.
///
/// What it does *not* do is redraw an already-open window -- it drops the
/// container so the *next* build re-asks. [`ItemChange`] is the one that
/// redraws, and it needs a real `DBRow` over the eleven-slot layout.
#[derive(Debug, Clone, PartialEq)]
pub struct InvalidateLocations {
    pub location_ids: Vec<i64>,
}

impl Push for InvalidateLocations {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnInvalidateCacheForLocations",
            // **One argument, and it is a list even for one location.** The
            // handler is `for locID in locationIDs:`, so a bare integer would
            // raise `TypeError` inside the notify chain -- where
            // `ExceptionWrapper.NotifyUp` swallows it, leaving no traceback on
            // either side and an inventory that silently never refreshes. The
            // census marks this event `raises-on-shape` for exactly that
            // reason.
            vec![PyValue::List(
                self.location_ids
                    .iter()
                    .copied()
                    .map(PyValue::Int)
                    .collect(),
            )],
        )
    }
}

/// `OnDockingAccepted(stationID)` or `OnDockingFinished(stationID)`, to the
/// pilot who docks. A live server sent the first straight after `CmdDock`
/// and the second once the dock was done, each with the station's id
/// **[V-TQ]** (a recorded session: `+0.00 s` and `+5.50 s`).
/// `tacticalNavigationService` turns them into `OnClientEvent_DockingStarted`
/// and `OnClientEvent_DockingFinished` **[V]**, which the node graphs'
/// `Docking Accepted` (83) and `Docking Completed` (223) hear -- the AIR
/// tutorial's task 2.1.06 waits on them.
#[derive(Debug, Clone, PartialEq)]
pub struct Docking {
    pub to: i64,
    pub station: i64,
    /// `false`: accepted; `true`: finished.
    pub finished: bool,
}

impl Push for Docking {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let name = if self.finished {
            "OnDockingFinished"
        } else {
            "OnDockingAccepted"
        };
        (name, vec![PyValue::Int(self.station)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnIndustryJob(jobID, ownerID, blueprintID, installerID, status,
/// successfulRuns)` -- **six positional arguments**, no dict.
///
/// `status` is `industry::Status::code()` and nothing else.
/// `docs/port-tracker.md` §14.1 is why that is not an invented integer:
/// `blueprintSvc._UpdateOnIndustryJobChange` does
/// `blueprint.jobID = jobID if status < STATUS_COMPLETED else None`, so a code
/// below 100 locks a blueprint to a job for ever and one at or above 100 frees
/// a blueprint that is still in one -- **silently, with nothing in either
/// log**. 4..99 are unused and `Status::from_code` refuses them.
///
/// `successfulRuns` is `None` on every push the client generates for itself
/// (`industrySvc._PollJobCompletion`), so the handlers already see `None`:
/// send the real count on a delivery and `None` otherwise.
#[derive(Debug, Clone, PartialEq)]
pub struct IndustryJob {
    pub to: Vec<i64>,
    pub job: crate::industry::Job,
    pub successful_runs: Option<i64>,
}

impl Push for IndustryJob {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnIndustryJob",
            vec![
                PyValue::Int(self.job.job_id),
                PyValue::Int(self.job.owner_id),
                // The blueprint **item** id, not its type:
                // `_UpdateOnIndustryJobChange` matches it against
                // `blueprint.itemID`. Sending the type id would patch the
                // `jobID` onto every copy of that blueprint the pilot owns.
                PyValue::Int(self.job.blueprint_id),
                PyValue::Int(self.job.installer_id),
                PyValue::Int(self.job.status.code()),
                self.successful_runs
                    .map(PyValue::Int)
                    .unwrap_or(PyValue::None),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnCharNowInStation(rec)` / `OnCharNoLongerInStation(rec)`.
///
/// **One argument, and it is the `station.GetGuests` row verbatim**:
/// `guests.GuestsPanelController.OnCharNowInStation` does
/// `UNPACK_SEQUENCE 4` into
/// `(character_id, corporation_id, alliance_id, war_faction_id)` **[V]**,
/// which is exactly the four elements that arm already builds. A wrong arity
/// is a `ValueError` inside the notify chain, where
/// `ExceptionWrapper.NotifyUp` swallows it -- no traceback on either side and
/// a guest list that silently never updates.
///
/// `OnCharNoLongerInStation` reads only `rec[0]`, so the same row serves both
/// and there is one type rather than two.
///
/// **Why this exists at all**: `GetGuests` is asked once, when the panel
/// loads. A player already docked when somebody else arrives is never told,
/// so their lobby shows a station they are alone in while the arriver sees
/// them both. Found by two real clients, not by a test -- the loopback suite
/// connects in one order and asks afterwards, which is precisely the order
/// that works.
#[derive(Debug, Clone, PartialEq)]
pub struct StationGuest {
    pub to: Vec<i64>,
    pub row: Vec<PyValue>,
    pub arrived: bool,
}

impl Push for StationGuest {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            if self.arrived {
                "OnCharNowInStation"
            } else {
                "OnCharNoLongerInStation"
            },
            // **One argument, and it is a sequence.** The handler does
            // `UNPACK_SEQUENCE 4` on it, so passing the four elements as four
            // arguments would be a `TypeError` and passing a bare id would be
            // one too -- both swallowed by `NotifyUp`.
            vec![PyValue::List(self.row.clone())],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trait's whole point: a queue can hold pushes from different
    /// subsystems and the dispatcher reads them without knowing which is
    /// which.
    ///
    /// Pins the *polymorphism*, not any one payload -- the tempting wrong
    /// version asserts the two events' names, which the per-type tests
    /// already do and which would pass just as well with a `match` in the
    /// dispatcher.
    #[test]
    fn a_queue_of_boxes_answers_for_every_subsystem() {
        let queue: Vec<Box<dyn Push>> = vec![
            Box::new(AccountChange {
                owner_id: 7,
                balance: 1.0,
            }),
            Box::new(InvalidateLocations {
                location_ids: vec![60_712_526],
            }),
            Box::new(crate::skill::ForcedRefresh),
        ];
        let names: Vec<&str> = queue.iter().map(|p| p.to_push().0).collect();
        assert_eq!(
            names,
            vec![
                "OnAccountChange",
                "OnInvalidateCacheForLocations",
                "OnSkillForcedRefresh"
            ]
        );
        // The wallet push names its owner; the other two are the caller's.
        assert_eq!(queue[0].audience(), Some(vec![7]));
        assert!(queue[1..].iter().all(|p| p.audience().is_none()));
    }

    /// The default `audience` is the acting session, and an addressed push
    /// says so.
    ///
    /// Pins that the *default* is "the caller" rather than "nobody": an
    /// implementation that returned `Some(vec![])` by accident would deliver
    /// every unaddressed push to no one at all, which is silent.
    #[test]
    fn an_addressed_push_names_its_characters_and_the_rest_do_not() {
        let guest = StationGuest {
            to: vec![98, 99],
            row: vec![PyValue::Int(98)],
            arrived: true,
        };
        assert_eq!(guest.audience(), Some(vec![98, 99]));
        assert_eq!(
            AccountChange {
                owner_id: 98,
                balance: 0.0,
            }
            .audience(),
            Some(vec![98])
        );
        assert_eq!(
            InvalidateLocations {
                location_ids: vec![60_712_526],
            }
            .audience(),
            None
        );
        let (event, args) = guest.to_push();
        assert_eq!(event, "OnCharNowInStation");
        assert_eq!(args.len(), 1, "one argument, and it is a sequence");

        let left = StationGuest {
            arrived: false,
            ..guest
        };
        assert_eq!(left.to_push().0, "OnCharNoLongerInStation");
    }

    /// A spawn's `change` dict is `{3: 0}`, inverted on the wire.
    ///
    /// Pins the *meaning* -- "location was 0, index 3" -- rather than the
    /// literal pair order, by naming both numbers in the assertion message.
    #[test]
    fn an_item_change_rewrites_column_three_and_nothing_else() {
        let (event, args) = ItemChange::spawned(PyValue::Int(1)).to_push();
        assert_eq!(event, "OnItemChange");
        assert_eq!(args.len(), 3);
        assert_eq!(
            args[1],
            PyValue::Dict(vec![(PyValue::Int(0), PyValue::Int(3))]),
            "marshal dict entries are (value, key): this is {{3: 0}}, the \
             fabricated previous location that fails IsItemHere"
        );
        assert_eq!(args[2], PyValue::None, "spawned names no storage");
        let place = PyValue::Int(7);
        let (_, args) = ItemChange::spawned(PyValue::Int(1))
            .with_location(place.clone())
            .to_push();
        assert_eq!(args[2], place);
    }

    /// On the wire an item change is `OnItemsChanged([row], change,
    /// location)`, the same row, change and location as the bare push; and
    /// `Addressed` keeps the wire form of what it wraps.
    #[test]
    fn an_item_change_goes_out_as_a_one_row_items_changed() {
        let c = ItemChange::spawned(PyValue::Int(1)).with_location(PyValue::Int(7));
        let (_, bare) = c.to_push();
        let (event, args) = c.to_wire();
        assert_eq!(event, "OnItemsChanged");
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], PyValue::List(vec![bare[0].clone()]));
        assert_eq!(args[1..], bare[1..]);
        let wrapped = Addressed {
            to: Some(vec![9]),
            inner: Box::new(c),
        };
        assert_eq!(wrapped.to_push().0, "OnItemChange");
        assert_eq!(wrapped.to_wire().0, "OnItemsChanged");
    }

    #[test]
    fn badging_location_names_the_station_storage_only_when_docked_there() {
        let tuple = |inv: &[u8]| {
            PyValue::Tuple(vec![
                PyValue::Bytes(b"Station".to_vec()),
                PyValue::Int(7),
                PyValue::Bytes(inv.to_vec()),
            ])
        };
        assert_eq!(badging_location(4, 4, 7, Some(7)), tuple(b"StationItems"));
        assert_eq!(badging_location(4, 6, 7, Some(7)), tuple(b"StationShips"));
        assert_eq!(badging_location(4, 4, 7, Some(8)), PyValue::None);
        assert_eq!(badging_location(4, 4, 7, None), PyValue::None);
        assert_eq!(badging_location(5, 4, 7, Some(7)), PyValue::None);
    }
}
