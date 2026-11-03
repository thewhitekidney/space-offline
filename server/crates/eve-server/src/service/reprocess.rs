//! `reprocessingSvc`: the refinery window's two remote calls.
//!
//! The maths and the static tables are `crate::reprocess`; this file is only
//! the wire shape. Read that module's header first -- every number here comes
//! from it, and every build-delta finding is recorded there.
//!
//! # The remote surface is two methods, and it was counted rather than ported
//!
//! `docs/port-tracker.md` §10.5: service names mislead about twice in both
//! directions, so count call sites. `grep -rl` over `out/pyc/` for each of
//! EvEJS's six handler names reaches:
//!
//! ```text
//!   GetQuotes               reprocessing/ui/quotes.py, reprocessing/ui/inputItemAdder.py
//!   Reprocess               reprocessing/ui/itemReprocessor.py
//!   GetQuote                -- no call site; the hits are substrings of GetQuotes
//!   GetReprocessingInfo     -- nothing, in 12,774 files
//!   GetOptionsForItemTypes  -- nothing, in 12,774 files
//! ```
//!
//! So **`GetReprocessingInfo` and `GetOptionsForItemTypes` are dead in build
//! 3619879** and EvEJS's `reprocessingService.js` implements both. They are
//! not implemented here, and that is the §10.5 rule paying for itself: two of
//! six handlers were work that nothing would have called.
//!
//! `reprocessing/util.pyc` also *defines* `GetReprocessInfoForItem`,
//! `GetStationEfficiencyForTypeID` and `GetCharRefiningYieldPercentageForType`
//! as methods on a `reprocessingSvc` passed in as a parameter -- but nothing
//! in the client calls `GetReprocessInfoForItem`, so that file is CCP's
//! *server* code shipped in the client package. It is the best evidence we
//! have for the yield formula (see `crate::reprocess`) and it is not a call
//! surface.
//!
//! # Binding
//!
//! `eveMoniker.GetReprocessingManager` is
//!
//! ```text
//!     if session.structureid:  moniker = Moniker('reprocessingSvc', session.structureid)
//!     elif session.stationid:  moniker = Moniker('reprocessingSvc', session.stationid)
//!     else: raise RuntimeError('asked for reprocessing manager in a weird place')
//! ```
//!
//! so the bind params are a bare station or structure id and not a tuple.
//! Nothing here reads them: `MachoResolveObject` is answered generically in
//! `Registry::dispatch` and `MachoBindObject` in `dispatch.rs`, and we are one
//! node with one refinery per station. The *docked* station comes from the
//! acting character, which is the only source that cannot disagree with the
//! rest of the server.

use super::*;

/// `reprocessingSvc`, and it is **not claimed by any other module**.
///
/// Checked against every `SERVICES` slice in `service/` and against
/// `Registry::OBJECT_RETURNING` before picking it, because
/// `no_service_is_claimed_by_two_modules` would otherwise fail and because
/// the first `DOMAINS` match wins *silently*. In particular it is not in
/// `industry::SERVICES` (`blueprintManager`, `facilityManager`,
/// `industryManager`, `industryMonitor`) or `inventory::SERVICES`, which were
/// the two likely collisions.
pub(crate) const SERVICES: &[&str] = &["reprocessingSvc"];

/// A `UserError` the client has a real row for.
///
/// **Every key used here was checked against `findings/user-errors.tsv`**,
/// which is the `error-vocabulary` lane's census of `dialogs.static`. That
/// check is not optional: `docs/re/errors.md` §2.4 records that a key with no
/// row renders as `ErrMessageNotFound`, whose `dialogType` is **`fatal`** --
/// so a typo here is a fatal modal naming our typo, not a missing string.
///
/// ```text
///   CannotReprocessActive  dialog 3375  info    modal    raised by the client itself
///   MrNothingToRefine      dialog 1251  notify  logline
///   CustomNotify           dialog  766  notify  logline  {'notify': <text>}
/// ```
fn reprocess_error(msg: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(msg, vec![]))
}

/// `CustomNotify`, the free-text escape hatch, as a **log line** rather than
/// a modal.
///
/// `docs/re/errors.md` §2.3: `CustomNotify` is `notify`/logline and
/// `CustomInfo` is `info`/modal. The refinery window is something a player
/// will click twice, so a modal each time is worse than a line in the log.
fn notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

/// A dict whose keys are **type ids**, which `pyutil::pydict` cannot build.
///
/// Same reasoning as `industry::int_keyed_dict`, and the same trap: the
/// client does `self.stationEfficiency.get(typeID)` with an integer, and
/// a string key misses every key and returns `None` -- which is then
/// multiplied by a float in `CalculateTheoreticalEfficiency` and raises.
///
/// The `(value, key)` ordering is the marshal dict's, not a typo.
fn int_keyed_dict(pairs: Vec<(i64, PyValue)>) -> PyValue {
    PyValue::Dict(
        pairs
            .into_iter()
            .map(|(k, v)| (v, PyValue::Int(k)))
            .collect(),
    )
}

/// The same dict, plus the **`None` key the client subscripts unguarded**.
///
/// `quotes.Quotes.GetStationEfficiencyForType` is one line:
///
/// ```text
///     return self.stationEfficiency.get(typeID, self.stationEfficiency[None])
/// ```
///
/// Python evaluates a `.get` default **eagerly**, so `self.stationEfficiency[None]`
/// is a `BINARY_SUBSCR` that runs on every call -- hit or miss. A dict with
/// no `None` key is therefore a `KeyError: None`, and not on some edge case:
/// `controller.Controller.SetEfficiency` loops over `self.inputGroups.groups`
/// and calls this once per group whenever the window holds anything. **[V]**,
/// disassembled from `reprocessing/ui/quotes.pyc(46)` and
/// `reprocessing/ui/controller.pyc(85)`.
///
/// So the real server's dict carries the station's own default under `None`,
/// and ours has to as well. The existing
/// `an_unrefinable_type_is_still_given_a_station_efficiency` test guards the
/// `.get` **miss**, which is a different problem and was already handled --
/// the subscript happens before the miss can.
///
/// Found by the `reprocess-evidence` lane while reading for something else,
/// in a file it did not own.
fn efficiency_dict(pairs: Vec<(i64, PyValue)>, station_default: f64) -> PyValue {
    let PyValue::Dict(mut entries) = int_keyed_dict(pairs) else {
        unreachable!("int_keyed_dict always builds a Dict")
    };
    entries.push((PyValue::Real(station_default), PyValue::None));
    PyValue::Dict(entries)
}

/// Item ids out of the first argument of `GetQuotes` / `Reprocess`.
///
/// The client passes a plain list of ints -- `inputItems.GetItemIDs()` and
/// `_LockItems`' accumulator -- so that is the case that matters. A tuple and
/// a dict keyed by item id are accepted too, because EvEJS's
/// `extractQuoteItemReferences` accepts them and a capture from a different
/// client path is cheaper to tolerate than to debug.
fn item_ids(arg: Option<&PyValue>) -> Vec<i64> {
    let mut out = Vec::new();
    match arg {
        Some(PyValue::List(v)) | Some(PyValue::Tuple(v)) => {
            for e in v {
                if let PyValue::Int(n) = e {
                    out.push(*n);
                }
            }
        }
        // A marshal dict entry is `(value, key)`; the item id is the key.
        Some(PyValue::Dict(entries)) => {
            for (_, k) in entries {
                if let PyValue::Int(n) = k {
                    out.push(*n);
                }
            }
        }
        _ => {}
    }
    out.dedup();
    out
}

/// One `Recoverable` as the `KeyVal` the window reads attributes off.
///
/// **A `KeyVal` and not a dict**: `reprocessing/ui/quotes._GetQuotes` does
/// `r.typeID` and `r.client`, which is attribute access. The four names are
/// the client's own, from the `Bundle(...)` call in
/// `reprocessing/util._GetRecoverablesFromFixedMaterials`.
fn recoverable_row(r: &crate::reprocess::Recoverable) -> PyValue {
    crate::pyutil::keyval(vec![
        ("typeID", PyValue::Int(r.type_id)),
        ("client", PyValue::Int(r.client)),
        ("unrecoverable", PyValue::Int(r.unrecoverable)),
        ("iskCost", PyValue::Real(r.isk_cost)),
    ])
}

/// One quote as a `KeyVal`.
///
/// Five of these nine names are read by the window and verified in the
/// bytecode: `typeID` and `recoverables` (`quotes._GetQuotes`),
/// `totalISKCost` and `iskCost`/`unrecoverable` (`outputItemAdder`), and
/// **`efficiency` and `numPortions`**
/// (`randomizedOutputContainer.AddOutputsFromQuote`). The remaining four --
/// `itemID`, `quantityToProcess`, `leftOvers`, `portions` -- are the other
/// fields of the client's own `GetReprocessInfoForItem` return tuple, sent
/// because they are free and because a window update could start reading one.
/// **[I]** that nothing reads them today.
///
/// `portions` and `numPortions` carry the same number deliberately: EvEJS
/// sends both and the client reads `numPortions`.
fn quote_row(q: &crate::reprocess::Quote) -> PyValue {
    crate::pyutil::keyval(vec![
        ("itemID", PyValue::Int(q.item_id)),
        ("typeID", PyValue::Int(q.type_id)),
        ("quantityToProcess", PyValue::Int(q.quantity_to_process)),
        ("leftOvers", PyValue::Int(q.left_overs)),
        ("portions", PyValue::Int(q.portions)),
        ("numPortions", PyValue::Int(q.portions)),
        ("efficiency", PyValue::Real(q.efficiency)),
        (
            "recoverables",
            PyValue::List(q.recoverables.iter().map(recoverable_row).collect()),
        ),
        ("totalISKCost", PyValue::Real(q.total_isk_cost)),
    ])
}

/// What `GetQuotes` answers when there is no character or no station.
///
/// `(0.0, {}, {})` -- the three-tuple's own shape, with nothing in it.
/// **Not an error.** `Request::actor` is `None` before a character is
/// selected, and the client reaches this through `Quotes.GetQuotes`, which
/// unpacks the result into three locals before touching any of them: an
/// exception there would propagate out of `inputItemAdder.AddItems` and leave
/// the window's loading overlay up forever, where an empty answer greys every
/// row and says nothing.
fn no_quotes() -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Real(0.0),
        PyValue::Dict(vec![]),
        PyValue::Dict(vec![]),
    ])
}

/// The character's inputs, resolved from the store.
///
/// Ownership is the only access rule applied: an item the actor does not own
/// is simply not in the list. EvEJS's `resolveAccessibleInventoryItem` also
/// admits corporation hangars whose division grants Hangar Take, which needs
/// the corporation role model; `crate::roles` has roles but no office
/// divisions, so corporation ore is **out of scope and refused by omission**
/// rather than silently allowed. Recorded in `docs/lanes/reprocess.md`.
fn inputs_for(
    reg: &Registry,
    actor: i64,
    station: i64,
    ids: &[i64],
) -> Vec<crate::reprocess::Input> {
    ids.iter()
        .filter_map(|&id| reg.store().get(id))
        .filter(|i| i.owner_id == actor)
        // The refinery works on what is in the station you are docked in.
        // Without this an itemID from another station's hangar -- which the
        // client can hold in its cache -- would refine remotely.
        .filter(|i| i.location_id == station)
        .filter(|i| offered(reg, i))
        .map(crate::reprocess::Input::of)
        .collect()
}

/// The two rules on what reaches the refinery that need the store and the
/// item types, so they live here and not in `crate::reprocess`'s table.
///
/// 1. **Nothing with contents.** An assembled ship with fittings or cargo,
///    or a container with something in it, is not offered: `Edit::Destroy`
///    removes the one row and would orphan whatever is inside. The client
///    has the dialogs for the real behaviour (`ConfirmReprocessSomethingWithUpgrades`
///    2133, `CanNotReprocessALSC` 2096, `ReprocessingFailedShipHasJumpClones`
///    1496 in `findings/user-errors.tsv`), which moves contents out first;
///    that is not done (`docs/lanes/scrap.md`). **The active ship is
///    exempt**, so `plan` still refuses it with `CannotReprocessActive`
///    rather than the window being told there was nothing to refine.
/// 2. **No scrapmetal type with an output the server cannot create.**
///    `Edit::Create` refuses an unknown type and with it the whole plan, so
///    a module whose outputs include one `item::find_in` does not know is
///    not quoted, rather than quoted and then refused. Ore, ice and gas rows
///    are not filtered here (CP32b's behaviour stands for them).
///
/// Neither is a client check: the client's own `Controller.IsValid`
/// (`reprocessing/ui/controller.pyc`) drops only `flagReward` items,
/// `groupMineral` and the active ship, and lets the server's quote decide
/// the rest -- which is this.
fn offered(reg: &Registry, item: &crate::item::Item) -> bool {
    let active = reg
        .character(item.owner_id)
        .is_some_and(|c| c.ship_id == item.item_id);
    if !active && reg.store().contents_of(item.item_id).next().is_some() {
        return false;
    }
    let table = reg.content().get::<crate::reprocess::Table>();
    !table.is_scrapmetal(item.type_id)
        || table.outputs_known(item.type_id, &|t| {
            crate::item::find_in(reg.content(), t).is_some()
        })
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        "reprocessingSvc" => match req.method {
            // `Quotes.GetQuotes(self, itemIDs, activeShipID)` ->
            //
            //     stationTax, stationEfficiency, rawQuotes = \
            //         ...GetQuotes(itemIDs, activeShipID)
            //
            // **`UNPACK_SEQUENCE 3`**, verified in `reprocessing/ui/quotes.pyc`:
            // a dict, a `None` or a 2-tuple is a `ValueError` inside
            // `inputItemAdder.AddItems`, which runs under
            // `window.ShowInputLoading()` -- so the wrong arity is a window
            // stuck behind its own loading wheel rather than a visible error.
            //
            // Element 1 is keyed by **typeID**, because
            // `Quotes.GetStationEfficiencyForType` is
            // `self.stationEfficiency.get(typeID)` and its result is
            // multiplied by a float in `CalculateTheoreticalEfficiency`. A
            // missing key is therefore `None * float` -- a `TypeError` in the
            // hint path. So we answer for **every type the client asked
            // about**, including the ones that produced no quote, which is
            // strictly more than EvEJS sends (it lists only quoted types plus
            // a `None` key that nothing can look up).
            "GetQuotes" => {
                let Some(actor) = req.actor else {
                    return Ok(no_quotes());
                };
                // Docked-now, not `Character::station_id` ("my station",
                // never cleared on undock -- `docs/lanes/docked.md`).
                let Some(station) = reg.docked_station(actor) else {
                    // In space. The client cannot normally get here --
                    // `GetReprocessingManager` raises locally with no
                    // `session.stationid` -- so this is the belt to that
                    // braces, and it answers empty rather than raising for
                    // the reason in `no_quotes`.
                    return Ok(no_quotes());
                };
                let ids = item_ids(req.args.first());
                let inputs = inputs_for(reg, actor, station, &ids);

                // **The character's standing with the station's owner**, now
                // that there is one (`Standings`, P5.7). It used to be a flat
                // zero because nothing stored standings, so the tax was always
                // the station's base take. **[A]** the raw standing, not one
                // adjusted by Connections / Diplomacy: which of the two CCP's
                // tax curve reads is not settled (`docs/lanes/standings.md`).
                //
                // `npc_station_at` and not `npc_station`: the station's terms
                // are per station (`mapObjects.db` `npcStations`, carried in
                // the bundle's `stations/stations.tsv`), and per-station
                // variation did **not** end with Crius the way this file used
                // to claim. A station with no row gets the modal fallback.
                //
                // The tables come from the world's `Content` (plan §1.5), so
                // a test can hand the server other rows.
                let table = reg.content().get::<crate::reprocess::Table>();
                let facility = crate::reprocess::Facility::npc_station_at(
                    table,
                    station,
                    owner_standing(reg, actor),
                );
                // **The character's own skills**, which is new: this said
                // "no skills and no implants, and both are table gaps rather
                // than choices" on the grounds that `crate::skill::SKILLS`
                // held none of the reprocessing skills. It holds all eighteen
                // now (`07a58d9`, ranks from `typedogma` attribute 275), so
                // the gap closed and the comment outlived it -- every refine
                // was still quoting the bare station rate to a character who
                // had trained for better.
                //
                // Implants stay empty because the server has no implant state
                // at all; `crate::reprocess::pilot` is still the seam.
                let pilot = crate::reprocess::pilot(table, reg.skills_for(Some(actor)), &[]);

                let mut efficiencies: Vec<(i64, PyValue)> = Vec::new();
                let mut quotes: Vec<(PyValue, PyValue)> = Vec::new();
                for input in &inputs {
                    if !efficiencies.iter().any(|(t, _)| *t == input.type_id) {
                        efficiencies.push((input.type_id, PyValue::Real(facility.efficiency)));
                    }
                    if let Some(q) =
                        crate::reprocess::quote(table, *input, &facility, &pilot, false)
                    {
                        quotes.push((quote_row(&q), PyValue::Int(q.item_id)));
                    }
                }
                efficiencies.sort_by_key(|(t, _)| *t);

                Ok(PyValue::Tuple(vec![
                    PyValue::Real(facility.tax),
                    efficiency_dict(efficiencies, facility.efficiency),
                    PyValue::Dict(quotes),
                ]))
            }

            // `ItemReprocessor._DoReprocess` ->
            //
            //     Reprocess(itemIDs, fromLocation, ownerID, outputLocationID,
            //               outputFlagID)
            //
            // **[V]** on the argument order, and it was worth checking: the
            // caller passes `(itemIDs, ownerID, fromLocationID, ...)` while
            // the callee's own `co_varnames` are
            // `(self, itemIDs, ownerID, fromLocation, ...)`, so the second
            // and third look swapped until you read both. They are not --
            // the names line up and the wire order is the one above, which is
            // also what EvEJS reads.
            //
            // The return value is consumed as
            // `inputItemIDs, outputByTypeID = reprocessor.Reprocess(...)`
            // (`reprocessing/ui/controller.Reprocess`, `UNPACK_SEQUENCE 2`),
            // and `ItemReprocessor.Reprocess` builds that by `extend`-ing our
            // answer into a list -- so our answer must be a **2-sequence**:
            // the processed item ids, then `{typeID: quantity}`.
            //
            // (`extend` across location buckets means two buckets produce a
            // four-element list and a `ValueError` on unpack. That is a real
            // bug in the client, not ours to work around: the window groups
            // by location before calling, so a selection spanning two
            // hangars already cannot work.)
            //
            // **This arm refuses, and the refusal is the honest answer.**
            // Everything up to the mutation works and is tested in
            // `crate::reprocess`; performing it needs three things that live
            // in files this lane does not own:
            //
            //   1. `item::Edit::Create { type_id, quantity, at }` -- `Edit`
            //      has no way to bring an item into existence, and
            //      `Store::spawn` is clamped to 1..=1000, forces
            //      `crate::model::FLAG_HANGAR` and sits outside `apply`, so it would cap
            //      an ore refine at 1,000 units of output *and* split the
            //      operation across two non-atomic calls.
            //   2. A `Registry` method to run a plan -- `Store::apply` has no
            //      caller anywhere in the tree yet and `Registry::store` is
            //      read-only.
            //   3. `notify::ItemChange` / `InvalidateLocations` pushes, so the
            //      open window redraws.
            //
            // Answering `(processedItemIDs, outputs)` without mutating would
            // be *believed*: the window removes the input tiles and draws the
            // outputs, so the player would watch their ore turn into minerals
            // that do not exist and survive a relog as ore. A refusal costs
            // them a log line. `docs/lanes/reprocess.md` has the exact
            // replacement for this arm.
            "Reprocess" => {
                let Some(actor) = req.actor else {
                    // No character: the empty answer, in the right shape.
                    // Nothing happened and nothing is claimed to have.
                    return Ok(PyValue::Tuple(vec![
                        PyValue::List(vec![]),
                        PyValue::Dict(vec![]),
                    ]));
                };
                // Docked-now: before this read `Character::station_id`,
                // which is never cleared, so a refine from space took ore out
                // of the station hangar (`docs/lanes/docked.md`).
                let Some(station) = reg.docked_station(actor) else {
                    return Err(notify("You must be docked to use reprocessing."));
                };
                let ids = item_ids(req.args.first());
                let inputs = inputs_for(reg, actor, station, &ids);
                // The same station's own terms as the quote, which is the
                // point: a quote and a refine that disagree would be a
                // player watching the numbers change as they click.
                let facility = crate::reprocess::Facility::npc_station_at(
                    reg.content().get::<crate::reprocess::Table>(),
                    station,
                    owner_standing(reg, actor),
                );
                // **The same skills as the quote, which they were not.**
                // `GetQuotes` was moved onto `reg.skills_for` when the
                // eighteen reprocessing skills landed and this call site was
                // left on `pilot(None, &[])` -- so a trained character was
                // shown the yield they had trained for and then paid the
                // bare station 0.5, losing the difference with nothing
                // logged. `docs/lanes/reprocess.md` §6.4 said "the two
                // `pilot(None, &[])` calls"; one of the two was converted.
                //
                // The borrow is **scoped to the plan**, below, and not held
                // in a local: `skills_for` borrows `reg` immutably and the
                // `Ok` arm needs it mutably to apply and to charge, so a
                // `let pilot = ...` here is an E0502 whose message points at
                // the destructor rather than at this line.
                //
                // The wallet, for the tax. `plan` refuses with
                // `Refusal::AccountFunds` rather than letting
                // `Registry::charge` overdraw -- see that variant, and
                // `charge`'s own doc on why the check belongs to the caller.
                let balance = reg.character(actor).map_or(0.0, |c| c.balance);
                // The hull the character is **in**, by item id -- which is
                // `Character::ship_id`, the same id the session told the
                // client and the id of the capsule row `Registry::new`
                // seeds. The client checks this itself in
                // `_CheckIsRefiningShip` and does not pass it to `Reprocess`,
                // so this is the server's own copy of that check.
                let active_ship = reg.character(actor).map(|c| c.ship_id);
                let output = opt_id(req.args.get(3)).map(|location_id| {
                    crate::item::Slot::new(
                        opt_id(req.args.get(2)).unwrap_or(actor),
                        location_id,
                        opt_id(req.args.get(4)).unwrap_or(crate::model::FLAG_HANGAR),
                    )
                });

                let planned = {
                    let table = reg.content().get::<crate::reprocess::Table>();
                    let pilot = crate::reprocess::pilot(table, reg.skills_for(Some(actor)), &[]);
                    crate::reprocess::plan(
                        table,
                        Some(actor),
                        Some(station),
                        &inputs,
                        &facility,
                        &pilot,
                        active_ship,
                        output,
                        balance,
                    )
                };
                match planned {
                    // `CannotReprocessActive`, dialog 3375 -- the same key
                    // the client raises for itself in `_CheckIsRefiningShip`,
                    // so the wording the player sees is identical whichever
                    // side caught it.
                    Err(crate::reprocess::Refusal::ActiveShip(_)) => {
                        Err(reprocess_error("CannotReprocessActive"))
                    }
                    // `MrNothingToRefine`, dialog 1251, notify/logline.
                    Err(crate::reprocess::Refusal::NothingToDo) => {
                        Err(reprocess_error("MrNothingToRefine"))
                    }
                    // No bundle: the server has no reprocessing rows at all,
                    // so say how to load them rather than that the type
                    // cannot be refined (plan §2.2). A log line, like the
                    // other `CustomNotify` here.
                    Err(crate::reprocess::Refusal::NoContent(type_id)) => {
                        Err(notify(&crate::reprocess::no_content_message(type_id)))
                    }
                    Err(crate::reprocess::Refusal::NotDocked) => {
                        Err(notify("You must be docked to use reprocessing."))
                    }
                    // `NotEnoughMoney`, dialog 1347, info/modal, with the
                    // two loc args the row declares --
                    // `[numeric]amount.isk` and `[numeric]balance.isk` --
                    // so the player is told the shortfall. The names are the
                    // dialog's own; a missing one renders as the literal
                    // `{amount}` in the body text.
                    Err(crate::reprocess::Refusal::AccountFunds(required, have)) => {
                        Err(ServiceError::Raise(eve_proto::user_error(
                            "NotEnoughMoney",
                            crate::pyutil::pypairs(vec![
                                ("amount", PyValue::Real(required)),
                                ("balance", PyValue::Real(have)),
                            ]),
                        )))
                    }
                    // Unreachable: the actor was checked above. Spelled out
                    // rather than `unreachable!()`, like the service arm
                    // below, so a drift is an error and not a panic in front
                    // of a player.
                    Err(crate::reprocess::Refusal::NoActor) => Ok(PyValue::Tuple(vec![
                        PyValue::List(vec![]),
                        PyValue::Dict(vec![]),
                    ])),
                    Ok(plan) => {
                        // **One plan, applied atomically.** `Registry::apply_items`
                        // runs it through `Store::apply`'s two phases and
                        // queues the `OnItemChange` / `OnInvalidateCacheForLocations`
                        // pushes from what actually changed.
                        if reg.apply_items(&plan.edits()).is_err() {
                            // Phase one refused, so **nothing was applied** --
                            // that is the guarantee, not a hope. The usual
                            // cause is a stale plan: an item moved or was
                            // consumed between the quote the player is
                            // reading and the click. `MrNothingToRefine` is
                            // the client's own cue to re-select, which is
                            // what we want them to do.
                            return Err(reprocess_error("MrNothingToRefine"));
                        }
                        // **And the tax is charged.** It was quoted and not
                        // charged for two rounds, for want of a public debit;
                        // `Registry::charge` is that debit and this is its
                        // second caller after industry's install fee. All
                        // three halves come with it -- the balance, the
                        // journal row and the `OnAccountChange` push -- which
                        // is the reason `charge` exists instead of each
                        // caller subtracting from a wallet.
                        //
                        // **Order: apply, then charge.** `apply_items` is
                        // all-or-nothing and has already returned `Ok`, so
                        // the ore is gone and the minerals exist; charging
                        // first would debit a wallet for a refine a stale
                        // plan then refused. Same order and the same reason
                        // as `Registry::install_job`.
                        //
                        // `plan` has already refused if the balance did not
                        // cover it, in this same dispatcher turn under this
                        // same world write lock, so nothing can have spent
                        // the money in between -- which is the contract
                        // `charge` states for not checking the balance
                        // itself.
                        //
                        // A free refine is not journalled: a zero-ISK row is
                        // noise rather than history, and an output with no
                        // price makes it a real case. Same rule as the
                        // install fee.
                        if plan.total_isk_cost > 0.0 {
                            // The station's owner is the counterparty, read
                            // from the character because that is who the
                            // tax is paid to and the character is docked in
                            // that station, in this turn, by construction.
                            // `reference_id` is the station: a reprocess has
                            // no job id, nothing in the Journal tab reads the
                            // column, and naming the refinery is more use to
                            // a human reading the world file than a 0. **[A]**
                            // on what the real server puts there.
                            let station_owner =
                                reg.character(actor).map_or(0, |c| c.station_owner_id);
                            reg.charge(
                                actor,
                                plan.total_isk_cost,
                                crate::reprocess::ENTRY_TYPE_REPROCESSING_TAX,
                                station_owner,
                                station,
                            );
                        }
                        Ok(PyValue::Tuple(vec![
                            PyValue::List(
                                plan.processed.iter().map(|&id| PyValue::Int(id)).collect(),
                            ),
                            int_keyed_dict(
                                plan.produce
                                    .iter()
                                    .map(|&(t, q)| (t, PyValue::Int(q)))
                                    .collect(),
                            ),
                        ]))
                    }
                }
            }

            // **Deliberately unserved**, with the census in this module's
            // header: `GetReprocessingInfo` and `GetOptionsForItemTypes` have
            // zero call sites in build 3619879's 12,774 bytecode files.
            // EvEJS implements both. Left to fall through to `no_method` so
            // that if a future build revives one, the `UNANSWERED` log line
            // names it instead of us having guessed a shape for it years
            // earlier. `GetQuote` (singular) likewise does not exist: every
            // apparent hit was a substring of `GetQuotes`.
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// The actor's standing with the owner of the station they are docked in;
/// 0.0, the client's own default, when either is unknown.
fn owner_standing(reg: &Registry, actor: i64) -> f64 {
    reg.character(actor)
        .map_or(0.0, |c| reg.standings.get(actor, c.station_owner_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyutil::dict_get;
    use crate::reprocess::fixture::{INERT, MODULE, MODULE_RARE, ORE, STATION_PLAIN};

    /// One attribute off a `KeyVal`.
    ///
    /// `pyutil::dict_get` reads a `Dict`; a `KeyVal` is a `0x17` `Instance`
    /// whose `state` is that dict, so a test asserting on a row has to go
    /// through the instance.
    fn attr<'a>(v: &'a PyValue, key: &[u8]) -> Option<&'a PyValue> {
        match v {
            PyValue::Instance { state, .. } => dict_get(state, key),
            other => dict_get(other, key),
        }
    }

    /// The `UserError` args a `ServiceError::Raise` carries: the key, then
    /// the dict.
    fn raised(e: &ServiceError) -> Option<(&PyValue, &PyValue)> {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            return None;
        };
        let PyValue::Tuple(outer) = header.as_ref() else {
            return None;
        };
        let PyValue::Tuple(args) = outer.get(1)? else {
            return None;
        };
        Some((args.first()?, args.get(1)?))
    }

    /// The `UserError` key a `ServiceError::Raise` carries.
    ///
    /// Structural, not a `Debug`-string match: `PyValue::Bytes` prints as a
    /// numeric array, so `format!("{v:?}").contains(..)` can never pass.
    fn raised_key(e: &ServiceError) -> Option<String> {
        let (key, _) = raised(e)?;
        crate::pyutil::as_bytes(key).map(|b| String::from_utf8_lossy(b).into_owned())
    }

    /// The text of a raised `CustomNotify`.
    fn raised_notify(e: &ServiceError) -> Option<String> {
        let (_, dict) = raised(e)?;
        crate::pyutil::as_bytes(dict_get(dict, b"notify")?)
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }

    fn call(reg: &mut Registry, method: &str, args: Vec<PyValue>) -> Result<PyValue, ServiceError> {
        let actor = reg.config().characters.first().map(|c| c.character_id);
        dispatch(
            reg,
            &Request {
                service: "reprocessingSvc",
                method,
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor,
            },
        )
    }

    fn call_without_actor(method: &str, args: Vec<PyValue>) -> Result<PyValue, ServiceError> {
        let mut reg = Registry::new(SandboxConfig::default());
        dispatch(
            &mut reg,
            &Request {
                service: "reprocessingSvc",
                method,
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        )
    }

    /// `Reprocess(itemIDs, ...)` with no destination picked.
    fn reprocess(reg: &mut Registry, ids: &[i64]) -> Result<PyValue, ServiceError> {
        call(
            reg,
            "Reprocess",
            vec![
                PyValue::List(ids.iter().map(|&i| PyValue::Int(i)).collect()),
                PyValue::Int(0),
                PyValue::Int(0),
                PyValue::None,
                PyValue::None,
            ],
        )
    }

    /// `GetQuotes(itemIDs, 0)`.
    fn get_quotes(reg: &mut Registry, ids: &[i64]) -> Result<PyValue, ServiceError> {
        call(
            reg,
            "GetQuotes",
            vec![
                PyValue::List(ids.iter().map(|&i| PyValue::Int(i)).collect()),
                PyValue::Int(0),
            ],
        )
    }

    /// The type a test's reprocess produces: a stackable type `item::find_in`
    /// knows, chosen by rule, because `Edit::Create` refuses an invented one.
    fn output() -> i64 {
        crate::item::SPAWNABLE
            .iter()
            .find(|t| !t.singleton)
            .expect("SPAWNABLE has a stackable type")
            .type_id
    }

    /// A world with **loaded** reprocessing rows: the invented fixture, with
    /// its cheap output swapped for [`output`] so a refine can complete.
    fn world() -> Registry {
        let mut content = crate::content::Content::fixture();
        content.insert("reprocess", crate::reprocess::fixture_with_output(output()));
        Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(content))
    }

    fn table(reg: &Registry) -> &crate::reprocess::Table {
        reg.content().get::<crate::reprocess::Table>()
    }

    /// Put a stack of some type in a hangar, through the snapshot loader.
    ///
    /// `Registry` exposes no other way to add an arbitrary type's stack, and
    /// `Registry::apply_snapshot` is non-destructive about everything it does
    /// not carry.
    fn seed(reg: &mut Registry, type_id: i64, quantity: i64, owner: i64, location: i64) -> i64 {
        let (group_id, category_id) = crate::item::find_in(reg.content(), type_id)
            .map_or((99_001, crate::typedata::CATEGORY_ASTEROID), |t| {
                (t.group_id, t.category_id)
            });
        let mut snap = reg.to_snapshot();
        // **Past the highest id already in the snapshot, not just past the
        // allocator.** `Registry::new` `insert`s each capsule at the
        // character's `ship_id` without minting it, so a seed that trusted
        // the allocator landed exactly on the capsule.
        let id = snap
            .items
            .iter()
            .map(|i| i.item_id)
            .max()
            .unwrap_or(crate::item::MIN_PLAYER_ITEM)
            .max(snap.next_item_id)
            + 1;
        snap.items.push(crate::item::Item {
            item_id: id,
            type_id,
            owner_id: owner,
            location_id: location,
            flag_id: crate::model::FLAG_HANGAR,
            quantity,
            group_id,
            category_id,
            singleton: false,
        });
        snap.next_item_id = id + 1;
        reg.apply_snapshot(snap);
        id
    }

    fn actor_of(reg: &Registry) -> i64 {
        reg.config()
            .characters
            .first()
            .expect("a character")
            .character_id
    }

    fn docked_at(reg: &Registry) -> i64 {
        reg.config()
            .characters
            .first()
            .and_then(|c| c.station_id)
            .expect("docked")
    }

    /// A stack in the acting character's own hangar.
    fn ore_in_hangar(reg: &mut Registry, type_id: i64, quantity: i64) -> i64 {
        let (actor, station) = (actor_of(reg), docked_at(reg));
        seed(reg, type_id, quantity, actor, station)
    }

    /// Ten portions of the fixture ore.
    fn ten_portions(reg: &Registry) -> i64 {
        10 * table(reg).portion_size(ORE)
    }

    /// Give the acting character a wallet, and answer what it now holds.
    /// **The fixture character's balance is 0.0**, so a refine with a
    /// priced output is refused for funds until it has one.
    fn fund(reg: &mut Registry, amount: f64) -> f64 {
        let actor = actor_of(reg);
        let c = reg.character_mut(actor).expect("the fixture character");
        c.balance = amount;
        amount
    }

    /// `totalISKCost` of the first quote `GetQuotes` answers for `item`.
    fn quoted_tax(reg: &mut Registry, item: i64) -> f64 {
        let quotes = get_quotes(reg, &[item]).expect("quotes");
        let PyValue::Tuple(t) = &quotes else {
            panic!("not a tuple")
        };
        let PyValue::Dict(rows) = &t[2] else {
            panic!("not a dict")
        };
        match attr(&rows[0].0, b"totalISKCost") {
            Some(PyValue::Real(v)) => *v,
            other => panic!("totalISKCost is not a Real: {other:?}"),
        }
    }

    /// Every service this module advertises must reach an arm in *this*
    /// module's `dispatch`. The same test every other domain module has.
    #[test]
    fn every_service_here_is_routed_to_this_module() {
        let mut reg = Registry::new(SandboxConfig::default());
        for s in SERVICES {
            let r = dispatch(
                &mut reg,
                &Request {
                    service: s,
                    method: "__no_such_method_exists__",
                    args: &[],
                    kwargs: &[],
                    bind_params: None,
                    actor: None,
                },
            );
            match r {
                Err(ServiceError::NoSuchMethod { .. }) => {}
                other => panic!("{s} is advertised here but answers {other:?}"),
            }
        }
    }

    /// The two dead handlers stay dead, so a future build reviving one shows
    /// up as `UNANSWERED` naming it rather than as a shape we guessed.
    #[test]
    fn the_handlers_with_no_call_sites_are_not_served() {
        let mut reg = Registry::new(SandboxConfig::default());
        for dead in ["GetReprocessingInfo", "GetOptionsForItemTypes", "GetQuote"] {
            assert!(
                matches!(
                    call(&mut reg, dead, vec![]),
                    Err(ServiceError::NoSuchMethod { .. })
                ),
                "{dead} has no call site in this build and should not be served"
            );
        }
    }

    /// `GetQuotes` with no character answers the three-tuple, empty.
    ///
    /// The arity is the whole point: the client unpacks three values before
    /// looking at any of them, and it does so inside the loading overlay.
    #[test]
    fn get_quotes_without_an_actor_answers_an_empty_three_tuple() {
        let v = call_without_actor("GetQuotes", vec![PyValue::List(vec![]), PyValue::None])
            .expect("answers rather than raising");
        match v {
            PyValue::Tuple(ref t) => {
                assert_eq!(t.len(), 3, "UNPACK_SEQUENCE 3 in quotes.py");
                assert_eq!(t[0], PyValue::Real(0.0));
                assert_eq!(t[1], PyValue::Dict(vec![]));
                assert_eq!(t[2], PyValue::Dict(vec![]));
            }
            other => panic!("not a tuple: {other:?}"),
        }
    }

    /// `Reprocess` with no character answers the two-sequence, empty.
    #[test]
    fn reprocess_without_an_actor_answers_an_empty_two_sequence() {
        let v = call_without_actor(
            "Reprocess",
            vec![
                PyValue::List(vec![]),
                PyValue::Int(0),
                PyValue::Int(0),
                PyValue::None,
                PyValue::None,
            ],
        )
        .expect("answers rather than raising");
        match v {
            PyValue::Tuple(ref t) => {
                assert_eq!(t.len(), 2, "UNPACK_SEQUENCE 2 in controller.py");
                assert_eq!(t[0], PyValue::List(vec![]));
                assert_eq!(t[1], PyValue::Dict(vec![]));
            }
            other => panic!("not a tuple: {other:?}"),
        }
    }

    /// A quote over a store row, checked field by field against the names
    /// the window reads.
    #[test]
    fn get_quotes_answers_a_quote_the_window_can_read() {
        let mut reg = world();
        let stack = ten_portions(&reg);
        let item = ore_in_hangar(&mut reg, ORE, stack);
        let station = docked_at(&reg);
        let (efficiency, tax, per_portion) = {
            let t = table(&reg);
            (
                t.station_efficiency(station),
                t.station_base_tax(station),
                t.materials(ORE)[0].1,
            )
        };
        let v = get_quotes(&mut reg, &[item]).expect("quotes");
        let PyValue::Tuple(t) = v else {
            panic!("not a tuple")
        };
        assert_eq!(t.len(), 3);
        assert_eq!(t[0], PyValue::Real(tax), "the station's base take");

        // Element 1: keyed by typeID, as an **integer** key.
        let PyValue::Dict(ref eff) = t[1] else {
            panic!("not a dict")
        };
        assert_eq!(
            eff.len(),
            2,
            "one asked-about type, plus the `None` key the client subscripts"
        );
        assert_eq!(eff[0].1, PyValue::Int(ORE), "keyed by typeID, not by name");
        assert_eq!(eff[0].0, PyValue::Real(efficiency));
        // `GetStationEfficiencyForType` evaluates `self.stationEfficiency[None]`
        // eagerly as the `.get` default, so a dict without this key is a
        // `KeyError` on every call. See `efficiency_dict`.
        assert_eq!(
            eff.iter()
                .find(|(_, k)| *k == PyValue::None)
                .map(|(v, _)| v),
            Some(&PyValue::Real(efficiency)),
            "the station default must be under the `None` key"
        );

        // Element 2: keyed by itemID, value a KeyVal with the window's names.
        let PyValue::Dict(ref quotes) = t[2] else {
            panic!("not a dict")
        };
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].1, PyValue::Int(item));
        let q = &quotes[0].0;
        assert_eq!(attr(q, b"typeID"), Some(&PyValue::Int(ORE)));
        assert_eq!(attr(q, b"numPortions"), Some(&PyValue::Int(10)));
        assert_eq!(attr(q, b"portions"), Some(&PyValue::Int(10)));
        assert_eq!(attr(q, b"efficiency"), Some(&PyValue::Real(efficiency)));
        assert_eq!(attr(q, b"quantityToProcess"), Some(&PyValue::Int(stack)));
        assert_eq!(attr(q, b"leftOvers"), Some(&PyValue::Int(0)));
        let Some(PyValue::List(rs)) = attr(q, b"recoverables") else {
            panic!("recoverables is not a list")
        };
        let raw = 10 * per_portion;
        let client = crate::reprocess::floor_yield(raw, efficiency);
        assert_eq!(rs.len(), 1);
        assert_eq!(attr(&rs[0], b"typeID"), Some(&PyValue::Int(output())));
        assert_eq!(attr(&rs[0], b"client"), Some(&PyValue::Int(client)));
        assert_eq!(
            attr(&rs[0], b"unrecoverable"),
            Some(&PyValue::Int(raw - client))
        );
    }

    /// An item the actor does not own, and one in another station, are both
    /// absent from the answer rather than refined.
    #[test]
    fn get_quotes_ignores_items_the_actor_cannot_reach() {
        let mut reg = world();
        let stack = ten_portions(&reg);
        let mine = ore_in_hangar(&mut reg, ORE, stack);
        let station = docked_at(&reg);
        let theirs = seed(&mut reg, ORE, stack, 90_000_999, station);
        // A different station: the id is in the client's cache and the ore
        // is not here.
        let actor = actor_of(&reg);
        assert_ne!(station, STATION_PLAIN);
        let elsewhere = seed(&mut reg, ORE, stack, actor, STATION_PLAIN);
        let v = get_quotes(&mut reg, &[mine, theirs, elsewhere, 999_999_999]).expect("quotes");
        let PyValue::Tuple(t) = v else { panic!() };
        let PyValue::Dict(ref quotes) = t[2] else {
            panic!()
        };
        assert_eq!(quotes.len(), 1, "only the actor's own, in this station");
        assert_eq!(quotes[0].1, PyValue::Int(mine));
    }

    /// A type with no reprocessing row produces no quote, and is still listed
    /// in the efficiency dict, because a missing key is a `TypeError` in the
    /// client's hint path.
    #[test]
    fn an_unrefinable_type_is_still_given_a_station_efficiency() {
        let mut reg = world();
        let module = ore_in_hangar(&mut reg, INERT, 1);
        let v = get_quotes(&mut reg, &[module]).expect("quotes");
        let PyValue::Tuple(t) = v else { panic!() };
        let PyValue::Dict(ref eff) = t[1] else {
            panic!()
        };
        assert_eq!(
            eff.len(),
            2,
            "the type the client asked about must be in the efficiency dict \
             even with no quote -- plus the `None` key"
        );
        assert_eq!(eff[0].1, PyValue::Int(INERT));
        assert!(eff.iter().any(|(_, k)| *k == PyValue::None));
        let PyValue::Dict(ref quotes) = t[2] else {
            panic!()
        };
        assert!(quotes.is_empty());
    }

    /// The active ship refuses with the key the client uses for itself, and
    /// the hull survives.
    #[test]
    fn reprocessing_the_active_ship_raises_cannot_reprocess_active() {
        let mut reg = world();
        let ship = reg
            .config()
            .characters
            .iter()
            .map(|c| c.ship_id)
            .next()
            .expect("the fixture has a character");
        let r = reprocess(&mut reg, &[ship]);
        match r {
            Err(ref e) => assert_eq!(raised_key(e).as_deref(), Some("CannotReprocessActive")),
            Ok(other) => panic!("expected a raise, got {other:?}"),
        }
        assert!(
            reg.store().get(ship).is_some(),
            "the hull they are sitting in must still exist"
        );
    }

    /// Nothing refinable in the selection is `MrNothingToRefine`, which has a
    /// real `dialogs.static` row (1251, notify/logline).
    #[test]
    fn nothing_to_refine_raises_a_key_the_client_has_a_row_for() {
        let mut reg = world();
        let module = ore_in_hangar(&mut reg, INERT, 1);
        match reprocess(&mut reg, &[module]) {
            Err(ref e) => assert_eq!(raised_key(e).as_deref(), Some("MrNothingToRefine")),
            Ok(other) => panic!("expected a raise, got {other:?}"),
        }
    }

    /// Plan §2.2: **with no bundle**, a type outside the fixture is refused
    /// with a log line naming `--content` -- not `MrNothingToRefine`, which
    /// would say the type cannot be refined, and not a panic. Nothing is
    /// consumed.
    #[test]
    fn without_a_bundle_a_type_outside_the_fixture_names_content() {
        let mut reg = Registry::new(SandboxConfig::default());
        assert!(!table(&reg).is_loaded(), "the default world has no bundle");
        let unknown = 990_999;
        assert!(!table(&reg).is_reprocessable(unknown));
        let item = ore_in_hangar(&mut reg, unknown, 100);
        fund(&mut reg, 1_000.0);
        let r = reprocess(&mut reg, &[item]);
        match r {
            Err(ref e) => {
                assert_eq!(raised_key(e).as_deref(), Some("CustomNotify"));
                let text = raised_notify(e).expect("a notify text");
                assert!(text.contains("--content"), "{text}");
            }
            Ok(other) => panic!("expected a raise, got {other:?}"),
        }
        assert_eq!(reg.store().get(item).map(|i| i.quantity), Some(100));
    }

    /// A refused `Reprocess` consumes nothing: the hull and the ore in one
    /// selection refuse the whole plan, not the refinable part of it.
    #[test]
    fn a_refused_reprocess_consumes_nothing() {
        let mut reg = world();
        let stack = ten_portions(&reg);
        let ore = ore_in_hangar(&mut reg, ORE, stack);
        let ship = reg
            .config()
            .characters
            .iter()
            .map(|c| c.ship_id)
            .next()
            .expect("the fixture has a character");
        let before = reg.store().items().len();
        let r = reprocess(&mut reg, &[ship, ore]);
        assert!(matches!(r, Err(ServiceError::Raise(_))), "got {r:?}");
        assert_eq!(reg.store().items().len(), before);
        assert_eq!(
            reg.store().get(ore).map(|i| i.quantity),
            Some(stack),
            "the ore must be untouched: phase one never ran"
        );
    }

    /// **In space, ore at home is neither quoted nor refined**, and none of
    /// it is consumed (`docs/lanes/docked.md`).
    #[test]
    fn in_space_ore_at_home_is_neither_quoted_nor_refined() {
        let mut reg = world();
        let stack = ten_portions(&reg);
        let ore = ore_in_hangar(&mut reg, ORE, stack);
        fund(&mut reg, 1_000.0);
        let actor = actor_of(&reg);
        crate::destiny::on_ballpark_bind(&mut reg, Some(actor));
        let _ = reg.take_pending();
        assert_eq!(reg.docked_station(actor), None);

        let quote = get_quotes(&mut reg, &[ore]).expect("an in-space quote answers, empty");
        assert_eq!(quote, no_quotes());

        let refine = reprocess(&mut reg, &[ore]);
        assert!(matches!(refine, Err(ServiceError::Raise(_))), "{refine:?}");
        assert_eq!(reg.store().get(ore).map(|i| i.quantity), Some(stack));
        assert_eq!(reg.character(actor).map(|c| c.balance), Some(1_000.0));
    }

    /// **A reprocess completes: ore out of the hangar, output into it**,
    /// through the real dispatch path, and the client is told.
    #[test]
    fn a_reprocess_consumes_the_ore_and_produces_the_minerals() {
        let mut reg = world();
        let stack = ten_portions(&reg);
        let ore = ore_in_hangar(&mut reg, ORE, stack);
        fund(&mut reg, 1_000_000.0);
        let answer = reprocess(&mut reg, &[ore]).expect("a valid reprocess must not raise");

        // `(processedItemIDs, {typeID: quantity})`.
        let PyValue::Tuple(t) = &answer else {
            panic!("a 2-tuple, got {answer:?}")
        };
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], PyValue::List(vec![PyValue::Int(ore)]));

        assert!(reg.store().get(ore).is_none(), "the ore was consumed");
        let made: i64 = reg
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == output())
            .map(|i| i.quantity)
            .sum();
        assert!(made > 0, "the ore must yield its output");

        let events: Vec<&'static str> = reg
            .take_pending()
            .iter()
            .map(|(_, n)| n.to_push().0)
            .collect();
        assert!(
            events.contains(&"OnItemChange") && events.contains(&"OnInvalidateCacheForLocations"),
            "the hangar must be told to re-read: {events:?}"
        );
        // The ore row itself goes to the junkyard, naming the hangar it left:
        // the invalidation alone misses the client's hangar list.
        let _ = reg.take_pending();
        let ore2 = ore_in_hangar(&mut reg, ORE, stack);
        let _ = reg.take_pending();
        reg.apply_items(&[crate::item::Edit::Destroy { item_id: ore2 }])
            .expect("destroy");
        let station = docked_at(&reg);
        assert_eq!(
            left_places(&mut reg),
            vec![station],
            "one row, naming the hangar the stack left"
        );
    }

    /// The previous locations named by the pending `OnItemChange` pushes
    /// that took an item away from one (a stack-size change names none).
    fn left_places(reg: &mut Registry) -> Vec<i64> {
        let mut out = Vec::new();
        for (_, p) in reg.take_pending() {
            let (name, args) = p.to_push();
            if name != "OnItemChange" {
                continue;
            }
            let PyValue::Dict(change) = &args[1] else {
                panic!("the change is a dict: {args:?}");
            };
            for (v, k) in change {
                if let (PyValue::Int(v), PyValue::Int(k)) = (v, k)
                    && *k == crate::item::IX_LOCATION_ID
                {
                    out.push(*v);
                }
            }
        }
        out
    }

    #[test]
    fn a_merged_away_stack_leaves_the_hangar_list() {
        let mut reg = world();
        let a = ore_in_hangar(&mut reg, ORE, 5);
        let b = ore_in_hangar(&mut reg, ORE, 7);
        let _ = reg.take_pending();
        reg.apply_items(&[crate::item::Edit::Merge { from: b, into: a }])
            .expect("merge");
        let station = docked_at(&reg);
        assert_eq!(
            left_places(&mut reg),
            vec![station],
            "the merged-away stack"
        );
    }

    #[test]
    fn a_destroyed_loaded_charge_sends_no_junkyard_row() {
        let mut reg = world();
        let id = ore_in_hangar(&mut reg, ORE, 5);
        let mut snap = reg.to_snapshot();
        for i in snap.items.iter_mut().filter(|i| i.item_id == id) {
            i.category_id = crate::item::CATEGORY_CHARGE;
            i.flag_id = 27;
        }
        reg.apply_snapshot(snap);
        let _ = reg.take_pending();
        reg.apply_items(&[crate::item::Edit::Destroy { item_id: id }])
            .expect("destroy");
        assert!(left_places(&mut reg).is_empty(), "a charge is no item row");
    }

    /// How many units of `type_id` the store holds, over every stack.
    fn held(reg: &Registry, type_id: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    /// Loot reprocesses (lane `scrap`): a module in the hangar is quoted and
    /// refined on the scrapmetal branch, at the character's own Scrapmetal
    /// Processing, and pays exactly what the quote said it would.
    #[test]
    fn a_module_in_the_hangar_reprocesses_at_the_scrapmetal_yield() {
        let mut reg = world();
        let actor = actor_of(&reg);
        let module = ore_in_hangar(&mut reg, MODULE, 2);
        fund(&mut reg, 1_000_000.0);
        let quoted = quoted_tax(&mut reg, module);
        let t = table(&reg);
        let efficiency = (t.station_efficiency(docked_at(&reg))
            * crate::reprocess::multiplier_for(t, MODULE, reg.skills_for(Some(actor)), &[]))
        .min(1.0);
        let want = crate::reprocess::floor_yield(2 * t.materials(MODULE)[0].1, efficiency);
        assert!(want > 0);
        let before = held(&reg, output());
        let balance = reg.character(actor).map(|c| c.balance).unwrap_or_default();

        let answer = reprocess(&mut reg, &[module]).expect("a module refines");
        let PyValue::Tuple(t) = &answer else {
            panic!("a 2-tuple, got {answer:?}")
        };
        assert_eq!(t[0], PyValue::List(vec![PyValue::Int(module)]));
        assert!(reg.store().get(module).is_none(), "the module was consumed");
        assert_eq!(held(&reg, output()) - before, want);
        let after = reg.character(actor).map(|c| c.balance).unwrap_or_default();
        assert!((balance - after - quoted).abs() < 1e-6, "the quoted tax");
    }

    /// A scrapmetal type with an output no `item::find_in` knows is neither
    /// quoted nor refined: `Edit::Create` would refuse the whole plan.
    #[test]
    fn a_module_with_an_output_the_server_cannot_create_is_not_offered() {
        let mut reg = world();
        let rare = ore_in_hangar(&mut reg, MODULE_RARE, 1);
        assert!(table(&reg).is_reprocessable(MODULE_RARE));
        fund(&mut reg, 1_000_000.0);
        let v = get_quotes(&mut reg, &[rare]).expect("quotes");
        let PyValue::Tuple(t) = v else { panic!() };
        assert_eq!(t[2], PyValue::Dict(vec![]), "no quote");
        match reprocess(&mut reg, &[rare]) {
            Err(ref e) => assert_eq!(raised_key(e).as_deref(), Some("MrNothingToRefine")),
            Ok(other) => panic!("expected a raise, got {other:?}"),
        }
        assert_eq!(reg.store().get(rare).map(|i| i.quantity), Some(1));
    }

    /// An item with something inside it is not offered, and both it and its
    /// contents survive: a destroy would orphan the contents.
    #[test]
    fn an_item_with_contents_is_not_offered() {
        let mut reg = world();
        let actor = actor_of(&reg);
        let module = ore_in_hangar(&mut reg, MODULE, 1);
        let inside = seed(&mut reg, output(), 5, actor, module);
        fund(&mut reg, 1_000_000.0);
        let v = get_quotes(&mut reg, &[module]).expect("quotes");
        let PyValue::Tuple(t) = v else { panic!() };
        assert_eq!(t[2], PyValue::Dict(vec![]), "no quote");
        match reprocess(&mut reg, &[module]) {
            Err(ref e) => assert_eq!(raised_key(e).as_deref(), Some("MrNothingToRefine")),
            Ok(other) => panic!("expected a raise, got {other:?}"),
        }
        assert!(reg.store().get(module).is_some());
        assert_eq!(reg.store().get(inside).map(|i| i.location_id), Some(module));
    }

    /// **The quoted tax is charged, all three halves of it**: the balance,
    /// the journal row and the push. Asserted against the quote the window
    /// was shown, because the thing that must hold is that the two agree.
    #[test]
    fn the_quoted_tax_is_debited_journalled_and_pushed() {
        let mut reg = world();
        let stack = ten_portions(&reg);
        let ore = ore_in_hangar(&mut reg, ORE, stack);
        let opening = fund(&mut reg, 1_000_000.0);
        let quoted = quoted_tax(&mut reg, ore);
        assert!(quoted > 0.0, "the fixture's output is priced");

        reprocess(&mut reg, &[ore]).expect("a funded reprocess must not raise");

        let actor = actor_of(&reg);
        assert_eq!(
            reg.character(actor).map(|c| c.balance),
            Some(opening - quoted),
            "the wallet must move by exactly what was quoted"
        );
        let entry = reg
            .journal()
            .entries()
            .last()
            .expect("a debit writes a journal row")
            .clone();
        assert_eq!(
            entry.entry_type_id,
            crate::reprocess::ENTRY_TYPE_REPROCESSING_TAX
        );
        assert!(
            crate::ledger::entry_type_name_id(entry.entry_type_id).is_some(),
            "an entry type the client cannot name blanks the Type column"
        );
        assert_eq!(entry.amount, -quoted, "a debit is signed negative");
        assert_eq!(entry.balance, opening - quoted, "the running total");
        assert_eq!(entry.owner_id_1, actor, "the payer");
        let events: Vec<&'static str> = reg
            .take_pending()
            .iter()
            .map(|(_, n)| n.to_push().0)
            .collect();
        assert!(
            events.contains(&"OnAccountChange"),
            "the wallet must be told: {events:?}"
        );
    }

    /// **A standing with the station's owner lowers the quoted tax.**
    /// Asserted as "less than the zero-standing quote": the curve is
    /// `crate::reprocess::station_tax`'s, and what is under test here is only
    /// that the call site reads the character's real standing.
    #[test]
    fn a_standing_with_the_station_owner_lowers_the_tax() {
        let mut reg = world();
        let stack = ten_portions(&reg);
        let ore = ore_in_hangar(&mut reg, ORE, stack);
        fund(&mut reg, 1_000_000.0);
        let actor = actor_of(&reg);
        let stranger = quoted_tax(&mut reg, ore);
        let owner = reg
            .character(actor)
            .map(|c| c.station_owner_id)
            .expect("docked");
        reg.standings.set(actor, owner, 5.0);
        let liked = quoted_tax(&mut reg, ore);
        assert!(
            liked < stranger,
            "standing 5.0 with the owner must cost less than 0.0: {liked} vs {stranger}"
        );
    }

    /// A wallet that cannot cover the tax refuses with `NotEnoughMoney`
    /// (dialog 1347), and **the ore and the wallet survive**.
    #[test]
    fn a_wallet_that_cannot_pay_the_tax_refuses_and_keeps_the_ore() {
        let mut reg = world();
        let stack = ten_portions(&reg);
        let ore = ore_in_hangar(&mut reg, ORE, stack);
        let quoted = quoted_tax(&mut reg, ore);
        let short = fund(&mut reg, quoted - 1.0);
        match reprocess(&mut reg, &[ore]) {
            Err(ref e) => assert_eq!(raised_key(e).as_deref(), Some("NotEnoughMoney")),
            Ok(other) => panic!("expected a raise, got {other:?}"),
        }
        assert_eq!(reg.store().get(ore).map(|i| i.quantity), Some(stack));
        assert_eq!(
            reg.character(actor_of(&reg)).map(|c| c.balance),
            Some(short),
            "and the wallet must not have moved"
        );
        assert!(
            reg.journal().entries().is_empty(),
            "a refusal writes no journal row"
        );
    }

    /// **The refine pays the skilled yield, not the bare station rate**, and
    /// the hangar gets what the window promised.
    #[test]
    fn a_trained_character_receives_what_the_quote_promised() {
        let mut reg = world();
        let stack = ten_portions(&reg);
        let ore = ore_in_hangar(&mut reg, ORE, stack);
        fund(&mut reg, 1_000_000.0);
        let actor = actor_of(&reg);
        let untrained = {
            let t = table(&reg);
            crate::reprocess::floor_yield(
                10 * t.materials(ORE)[0].1,
                t.station_efficiency(docked_at(&reg)),
            )
        };
        // The fixture ore has attribute 790, so 3385 and 3389 apply to it.
        {
            let skills = reg
                .skills
                .get_mut(&actor)
                .expect("the fixture character has a skill sheet");
            for id in [
                crate::reprocess::TYPE_REPROCESSING,
                crate::reprocess::TYPE_REPROCESSING_EFFICIENCY,
            ] {
                assert!(
                    skills.grant_in(&reg.content, id, 5).is_some(),
                    "skill {id} must be in the content skill table for this to be reachable"
                );
            }
        }

        let quotes = get_quotes(&mut reg, &[ore]).expect("quotes");
        let PyValue::Tuple(t) = &quotes else { panic!() };
        let PyValue::Dict(rows) = &t[2] else { panic!() };
        let Some(PyValue::List(rs)) = attr(&rows[0].0, b"recoverables") else {
            panic!("no recoverables")
        };
        let promised = match attr(&rs[0], b"client") {
            Some(PyValue::Int(n)) => *n,
            other => panic!("client is not an Int: {other:?}"),
        };
        assert!(
            promised > untrained,
            "a level-5 refiner must be quoted more than the bare {untrained}: {promised}"
        );

        reprocess(&mut reg, &[ore]).expect("a funded reprocess must not raise");
        let made: i64 = reg
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == output() && i.owner_id == actor)
            .map(|i| i.quantity)
            .sum();
        assert_eq!(
            made, promised,
            "the hangar must hold what the window promised"
        );
    }

    /// Item ids survive every argument shape the client or a capture could
    /// use.
    #[test]
    fn item_ids_are_read_from_a_list_a_tuple_or_a_dict() {
        assert_eq!(
            item_ids(Some(&PyValue::List(vec![PyValue::Int(7), PyValue::Int(9)]))),
            vec![7, 9]
        );
        assert_eq!(
            item_ids(Some(&PyValue::Tuple(vec![PyValue::Int(7)]))),
            vec![7]
        );
        // A marshal dict entry is `(value, key)`, so the id is element 1.
        assert_eq!(
            item_ids(Some(&PyValue::Dict(vec![(PyValue::None, PyValue::Int(7))]))),
            vec![7]
        );
        assert_eq!(item_ids(None), Vec::<i64>::new());
        assert_eq!(item_ids(Some(&PyValue::None)), Vec::<i64>::new());
    }

    /// `0` and `None` are both "the player picked nothing", which is the
    /// client's own `(None, None)` sentinel.
    #[test]
    fn a_zero_id_argument_means_nothing_was_picked() {
        assert_eq!(opt_id(Some(&PyValue::Int(0))), None);
        assert_eq!(opt_id(Some(&PyValue::None)), None);
        assert_eq!(opt_id(None), None);
        assert_eq!(
            opt_id(Some(&PyValue::Int(STATION_PLAIN))),
            Some(STATION_PLAIN)
        );
    }
}
