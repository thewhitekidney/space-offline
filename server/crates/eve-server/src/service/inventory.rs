//! Items, the hangar, fitting and ships.
//!
//! Split out of `service.rs`, which had grown to 7374 lines in one file --
//! and that, rather than anything about the work itself, was what capped
//! parallel implementation at one agent. A lane working on this subsystem
//! now opens one file and cannot collide with a lane working on another.
//!
//! The arms are unchanged by the move: `self` became the `reg` argument and
//! nothing else. Helpers deliberately stayed in the parent for now, because
//! several are shared across domains and moving those is where a split of
//! this kind goes wrong.

use super::args::{self, Args};
use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &[
    "invbroker::inventory",
    "invbroker",
    "ship",
    "shipKillCounter",
    "fighterMgr",
    "dogmaIM",
    // Fittings. They arrived here rather than in `store` because the split
    // script's arm-matcher only recognised single-pattern arms, so this
    // multi-pattern one was absorbed into whichever arm preceded it in the
    // file -- which was `vaultManager`. The two drift tests caught it
    // immediately, which is the argument for having them.
    "charFittingMgr",
    "corpFittingMgr",
    "allianceFittingMgr",
    // Repackaging. It had no arm at all once ("Repackage" in the hangar did
    // nothing, silently), then an honest refusal, and now CCP's own rules:
    // `repackaging/__init__.py` ships in this client.
    "repackagingSvc",
    // Ship insurance moved to `service/insurance.rs` (W16 CP44).
];

/// The calls [`dispatch_ro`] answers (PA-12a): the two handle-getters, as
/// service calls.
pub(crate) const READ_METHODS: &[(&str, &str)] = &[
    ("invbroker", "GetInventoryFromId"),
    ("invbroker", "GetInventory"),
];

/// The read twin of what [`dispatch`] answers for these: neither the customs
/// nor the corp-office preamble claims `invbroker`'s handle-getters
/// (`invbroker::inventory` and `DeliverToCorpHangar` only), neither has an
/// `invbroker` arm, and `gaps2::invbroker` answers `None` for them -- so the
/// write path ends in `NoSuchMethod`, which `handle_object_returning_call`
/// swallows before it mints. The differential test pins this.
pub(crate) fn dispatch_ro(
    reg: &Registry,
    _caller: Caller,
    req: &Request<'_>,
) -> Option<Result<RoValue, ServiceError>> {
    match (req.service, req.method) {
        ("invbroker", "GetInventoryFromId" | "GetInventory") => Some(Err(reg.no_method(req))),
        _ => None,
    }
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    // **A pilot docked in an Upwell structure has the hangar under another
    // name** (U4): `GetInventory(containerHangar)` becomes
    // `GetInventory(containerStructure)` once `session.structureid` is set,
    // and `GetInventoryFromId(structureid)` is the same container
    // (`invCache.py:489-492`) **[V]**. Read as the hangar, before any of the
    // arms below look at the container.
    // **A pilot at a structure's controls reads its bays and slots as
    // themselves** (U6C): `List(flag=fuel)` on the structure's inventory is
    // the fuel bay, not the hangar the alias below would answer.
    if req.service == "invbroker::inventory"
        && req.method == "List"
        && let Some(rows) = crate::structure_dock::control::bay_rows(
            reg,
            req.actor,
            container_of(req.bind_params),
            Args::new(req).int_kw_or("flag", 0),
        )
    {
        return Ok(PyValue::List(rows));
    }
    // **The deed and fuel bays take items** (STR-CORE, `crate::structure_bays`):
    // an `Add` into either, and an `Add` out of either, at the controls.
    if let Some(answer) = crate::structure_bays::inventory_call(reg, req) {
        return answer;
    }
    // **Fit and unfit a service module at a structure's controls** (#41,
    // `crate::structure_services`): `Add(itemID, source, qty, flag=164..171)`
    // on the structure's inventory fits, and `Add(moduleID, structure,
    // flag=4)` of a module fitted there unfits. Before the hangar alias below,
    // which would read both as the hangar.
    if req.service == "invbroker::inventory"
        && matches!(req.method, "Add" | "MultiAdd")
        && let Some(answer) = structure_module_call(reg, req)
    {
        return answer;
    }
    if req.service == "invbroker::inventory"
        && let Some(hangar) =
            crate::structure_dock::hangar_alias(reg, req.actor, container_of(req.bind_params))
    {
        let bound = PyValue::Tuple(vec![PyValue::Int(hangar), PyValue::None]);
        let mut aliased = req.clone();
        aliased.bind_params = Some(&bound);
        return dispatch(reg, &aliased);
    }
    // **Customs offices** (W9 L5, `docs/lanes/planet.md` §11): an inventory
    // bound on an office, and an item leaving one, are PI's. `None` for
    // everything else, which carries on below unchanged.
    if let Some(answer) = super::planet::customs_inventory_call(reg, req) {
        return answer;
    }
    // **Corporation offices** (CORP-O, `docs/lanes/corp-o.md`): an office's
    // hangar divisions, the deliveries hangar, and an item leaving either.
    // `None` for everything else.
    if let Some(answer) = super::corp_office::inventory_call(reg, req) {
        return answer;
    }
    // **Cargo containers** (#57, `crate::containers`): an inventory bound on
    // an assembled container or a courier package, and an item leaving a
    // container for the hangar or a ship. `None` for everything else.
    if let Some(answer) = crate::containers::inventory_call(reg, req) {
        return answer;
    }
    match req.service {
        // The skill handler's methods. Not a machoNet service: this is
        // the object `skillMgr2.GetMySkillHandler` hands back, and the
        // name is synthetic (the `::` cannot collide with a real service
        // name). See `Registry::OBJECT_RETURNING`.
        // An inventory we handed out. `List` is read as a sequence of
        // item rows, and `invCacheContainer.List` **silently drops** any
        // row whose length differs from the `GetItemDescriptor` header,
        // logging 'Invalid Item' -- so an empty list is the only honest
        // answer until we have real items.
        "invbroker::inventory" => match req.method {
            // The hangar listing. Which inventory this is comes from the
            // object's own bind params -- the arguments of the
            // `GetInventory` / `GetInventoryFromId` call that minted it.
            //
            // The station hangar (`containerHangar`, 10004) holds the
            // capsule; the capsule's own inventory is empty. There is no
            // separate "active ship" list anywhere in the client: every
            // consumer reads this listing and picks the row whose
            // `itemID == session.shipid`, including
            // `hangarView.GetShipItemFromHangar`, which is what decides
            // whether a ship model is drawn at all.
            // **"Stack All" is implemented now** -- see its own arm below,
            // which also corrects two things this comment used to assert.
            // The `PermissionDenied` retry at `invControllers.py:484` is
            // unreachable from the hangar (that path returns at `:475`,
            // *before* the `SETUP_EXCEPT` at `:477`), and the census's
            // `if_x_is_none+try_except:UserError` guard for this call site
            // describes the password-protected-container path, not this one.
            // **The other list-shaped reads on this object, all iterated and
            // all raising on `None` until now.**
            //
            // Routed exactly like `List`, from the object's own bind params,
            // rather than answered `[]` flatly -- because two of them are the
            // hangar under another name, and a flat empty list would have
            // contradicted `List` for the same container. `ListByFlags` and
            // `ListIncludingContainers` on *this* object are the container's
            // own contents; `charMgr.ListIncludingContainers` is the
            // cross-station query the assets window merges, and that is a
            // different call with a different answer.
            //
            // `ListCargo`, `ListHardwareModules` and `ListFighterBay` stay
            // empty. They were true answers for a capsule; a fitted ship
            // has hardware now, but the client's own `ListCargo` /
            // `ListHardwareModules` are wrappers over `List` that never call
            // these (`invCache.py:1291-1300`, `docs/lanes/fitting.md` §6) [V],
            // so only devtools reach them.
            //
            // `ValidateItemListCanBeOpened` is iterated for the items that
            // *cannot* be opened, so empty means "all of them are fine",
            // which is true of a hangar holding a pod.
            //
            // **An item container lists its contents** now that a ship can
            // have some (`docs/lanes/fitting.md` §6): `ListByFlags(flags=[…])`
            // is the client's own keyword (`invCache.py:1137`) [V].
            "ListByFlags" => Ok(match container_of(req.bind_params) {
                Some(CONTAINER_HANGAR) => PyValue::List(reg.hangar_item_rows(req.actor)),
                Some(id) => {
                    let a = Args::new(req);
                    let flags = args::as_int_list(a.kw("flags").or(a.get(0)));
                    PyValue::List(item_contents_rows(reg, req.actor, id, |f| {
                        flags.contains(&f)
                    }))
                }
                None => PyValue::List(vec![]),
            }),
            "ListIncludingContainers" => Ok(match container_of(req.bind_params) {
                Some(CONTAINER_HANGAR) => PyValue::List(reg.hangar_item_rows(req.actor)),
                Some(id) => PyValue::List(item_contents_rows(reg, req.actor, id, |_| true)),
                None => PyValue::List(vec![]),
            }),
            // `{capacity, used}`, the shape `invControllers.py:69` builds
            // its own zero sentinel with. See `Registry::container_capacity`
            // for why this was parked rather than guessed at.
            // `GetCapacity(flag)` is what `BaseInvContainer.GetCapacity` asks
            // (`invControllers.py:410`) [V]: for the fighter bay's flag it is
            // the bay's own `fighterCapacity` and what is in it (FT-B).
            "GetCapacity" => Ok(
                match (container_of(req.bind_params), Args::new(req).int(0)) {
                    (Some(id), Some(crate::fighters::FLAG_FIGHTER_BAY))
                        if reg
                            .item(id)
                            .is_some_and(|s| s.owner_id == req.actor.unwrap_or(0)) =>
                    {
                        crate::fighters::bay_capacity_value(reg, id)
                    }
                    (container, _) => reg.container_capacity(req.actor, container),
                },
            ),
            // **`ListFighterBay`** is what the squadron menu reads
            // (`squadronMenu.py:41`, rows with `.typeID`, `.stacksize`,
            // `.itemID`) [V]: the ship's flag-158 items.
            "ListFighterBay" => Ok(match container_of(req.bind_params) {
                Some(id) if id != CONTAINER_HANGAR => {
                    PyValue::List(item_contents_rows(reg, req.actor, id, |f| {
                        f == crate::fighters::FLAG_FIGHTER_BAY
                    }))
                }
                _ => PyValue::List(vec![]),
            }),
            "ListCargo" | "ListHardwareModules" | "ValidateItemListCanBeOpened" => {
                Ok(PyValue::List(vec![]))
            }
            // `invCache.ValidateItemCanBeOpened` returns this straight to
            // `invWindow._IsCurrentInventoryValid`, where a falsy answer marks
            // the open inventory invalid and the window closes
            // **[V out/pyc `invCache.py:493`, `invWindow.py:688`]**. The list variant
            // above names none as unopenable, so the single one is true.
            "ValidateItemCanBeOpened" => Ok(PyValue::Bool(true)),
            // **"Stack All", and it stacks.**
            //
            // The client has already done the hard half. `invCacheContainer.StackAll`
            // (`invCache.py:1141`) walks its own `List`, skips the container's
            // own item, anything with `quantity < 0`, and anything the
            // `lockedItems` service holds, and sets `hasSomethingToStack`
            // **only when it sees a second item whose `typeID` it has already
            // seen** (`:1155`-`:1159`). The server is called at
            // `invCache.py:1168` *only if* that flag is true, inside a
            // `try/finally` that unlocks the ids it locked. So when this arm
            // runs, the client has counted at least two stacks of one type in
            // this container -- which is precisely why the old
            // "cannot stack items yet" was a lie by §5's test: the window
            // offered an action the player could not use, on evidence the
            // client had already gathered.
            //
            // Three things it taught us that the census did not:
            //
            // 1. **`idCheckers.IsFakeItem(self.itemID)` returns before we are
            //    asked** (`:1142`), so a fake location never reaches here.
            // 2. **For the station hangar the call is not guarded.**
            //    `invControllers.StackAll` (`:469`) returns at `:475` when
            //    `self.locationFlag` is truthy -- *before* the `SETUP_EXCEPT`
            //    at `:477` -- so the `try_except:UserError` the census records
            //    covers only the password-protected-container path. The
            //    `PermissionDenied` retry that lane note 1 warned about
            //    cannot be reached from the hangar at all.
            // 3. **`container.StackAll` (`:1080`) discards our return value
            //    and then calls `uthread.new(self.Refresh)`.** That is the
            //    answer to "what does the client need to be told": it
            //    re-lists itself, so all we owe it is a cache that will give
            //    a different answer when it does -- which is
            //    `OnInvalidateCacheForLocations`, queued by
            //    `Registry::apply_items`. `OnItemChange` goes out too and is
            //    *not* what redraws: with our `{ixLocationID: 0}` change dict
            //    `InventorySvc.OnItemChange` computes `wasHere == False` and
            //    takes the `AddItem` branch (`inventorysvc.py:142`), and
            //    `container.AddItem` for an id already in the scroll logs
            //    `'Tried to add an item that is already there??'` and calls
            //    `UpdateItem` (`container.py:921`-`:923`). Harmless, and
            //    worth knowing before someone reads that warning as a bug.
            "StackAll" => {
                let Some(slot) = stackable_slot(reg, req) else {
                    // Not a container we can stack: the only one with
                    // contents today is the station hangar, and a pod's own
                    // inventory is genuinely empty, so nothing is left
                    // unstacked by saying so.
                    return Ok(PyValue::None);
                };
                let plan = reg.store().stack_plan(slot);
                if plan.is_empty() {
                    // The client thought there was something to stack and we
                    // disagree. Not an error: `hasSomethingToStack` counts
                    // `typeID`s across the whole container listing while
                    // `stack_plan` groups by address, and the two can differ
                    // for an item we have not placed in this slot. Nothing
                    // changed, so nothing is pushed.
                    return Ok(PyValue::None);
                }
                if reg.apply_items(&plan).is_err() {
                    // Phase one refused, so **nothing was applied**. A plan
                    // this arm built from the store it just read should not
                    // be refusable, so this is our bug rather than the
                    // player's -- which is why it says so instead of naming
                    // one of CCP's keys.
                    return Err(ServiceError::Raise(eve_proto::user_error(
                        "CustomNotify",
                        crate::pyutil::pypairs(vec![(
                            "notify",
                            crate::pyutil::bstr("These items could not be stacked."),
                        )]),
                    )));
                }
                // Discarded by every caller (`container.py:1082` is a bare
                // `POP_TOP`), and `invCacheContainer.StackAll` returns ours
                // through untouched.
                Ok(PyValue::None)
            }
            "List" => Ok(match container_of(req.bind_params) {
                // **Everything in the hangar, not just the capsule.** This
                // used to synthesise the one row it knew about; it now reads
                // the item store, which is what makes a `/spawn` visible
                // here without any further change.
                Some(CONTAINER_HANGAR) => PyValue::List(reg.hangar_item_rows(req.actor)),
                // **A ship's contents, filtered by the `flag` keyword.**
                // `invCacheContainer.List` sends `self.moniker.List(flag=flag)`
                // (`invCache.py:1101`) [V], and this is what
                // `LoadItemsInLocation` reads on every board (`cdl:344`): an
                // empty answer here is a fitted ship that boards bare.
                Some(id) => {
                    let flag = Args::new(req).int_kw_or("flag", 0);
                    // CAP-H: the fleet hangar and the maintenance bay list
                    // for the owner and for a pilot the ship's config admits.
                    let bay = req
                        .actor
                        .and_then(|a| crate::ship_config::bay_listing(reg, a, id, flag));
                    if let Some(items) = bay {
                        PyValue::List(items.iter().map(|i| reg.item_row_value(i)).collect())
                    } else {
                        PyValue::List(item_contents_rows(reg, req.actor, id, |f| {
                            flag.is_none_or(|want| want == f)
                        }))
                    }
                }
                None => PyValue::List(vec![]),
            }),
            // **Fit, unfit, and move a module between slots: one call.**
            // Fitting is an inventory move, not a dogma call
            // (`docs/lanes/fitting.md` §0.2): `Add(itemID, sourceID, qty=1,
            // flag=slot)` on the ship's inventory fits (`cdl:817`), and
            // `Add(moduleID, shipID, flag=4)` on the hangar's unfits
            // (`fittingSlotController.py:235`) [V]. Every caller discards
            // the reply [V], so it is `None`; what the client acts on is the
            // `OnItemChange` that `apply_items` sends.
            "Add" => crate::fitstate::around(reg, req.actor, |reg| add(reg, req)),
            // X-RIG: the fitting window's confirmed rig unfit (`RigUnFittingInfo`).
            "DestroyFitting" => crate::fitstate::around(reg, req.actor, |reg| {
                crate::rigs::destroy_fitting(reg, req.actor, Args::new(req).int(0))
            }),
            // "Fit to Active Ship" over a selection: `MultiAdd([ids], src,
            // flag=0)` (`fittingUtil.py:171`) [V].
            "MultiAdd" => crate::fitstate::around(reg, req.actor, |reg| multi_add(reg, req)),
            // GAP-FITAPPLY: "Fit" on a saved fitting, and "Strip Fitting"
            // (`fittingSvc.LoadFitting`, `invCache.StripFitting`) [V];
            // `fit_apply.rs` has the shapes.
            // GAP-INVPACK: a stack dropped on a stack of its type
            // (`invCacheContainer.MultiMerge`) [V]; `pack.rs`.
            "MultiMerge" => pack::multi_merge(reg, req),
            "FitFitting" => {
                crate::fitstate::around(reg, req.actor, |reg| fit_apply::fit_fitting(reg, req))
            }
            "StripFitting" => {
                crate::fitstate::around(reg, req.actor, |reg| fit_apply::strip(reg, req))
            }
            // **The call that was stopping the hangar from drawing.**
            //
            // `invCacheContainer.__init__` keys itself on the container id
            // and takes a shortcut for ordinary containers:
            //
            // for a key whose first element is not one of the container ids it
            // takes that element as its item id `[V]`
            //
            // `containerIDs` is the nine magic ids -- `containerHangar`
            // (10004), `containerWallet`, `containerGlobal` and so on --
            // so for a *real* item id the container already knows what it
            // is and never asks us. For the hangar it does not, and
            // `_SetItemData` has to go and get it:
            //
            // when it has no item yet and has a moniker, it asks the moniker for
            // the inventory's own item and copies the item id and type id from
            // the result `[V]`
            //
            // There is no guard on that. Answering `None` made
            // `reg._item.itemID` an `AttributeError`, which the `itemID`
            // property re-raises as a `RuntimeError`, which came out of
            // `hangarView.SetUpModels` -> `GetShipItemFromHangar` ->
            // `invCache.List` and aborted `LoadView` for the whole hangar.
            // The station UI still drew, because the lobby window is built
            // by the session-change handler rather than by the view -- so
            // the symptom was a complete docked UI over a black scene.
            //
            // **The container is the station**, which is why the station's
            // own ids are the answer. `invCache.GetInventory` routes
            // `containerHangar` to `reg.stationInventoryMgr`, a moniker
            // bound on the station, and `StationItems.IsItemHere` confirms
            // the model: `item.locationID == session.stationid`. So the
            // hangar's "self item" is the station holding it.
            //
            // Only `.itemID` and `.typeID` are read here, but a full item
            // row costs nothing and keeps one descriptor in the server.
            //
            // **[I]** The container key is not visible from here. Every
            // inventory we hand out is minted from the same `invbroker`
            // bind params, so this arm cannot yet tell the hangar from any
            // other container -- it is right because the hangar is the
            // only container that asks: the nine magic ids are exactly the
            // ones that reach `_SetItemData`, and the other inventory the
            // client binds, `GetInventoryFromId(90000001, 1)`, is keyed on
            // a real item id and so takes the shortcut above. The day a
            // second magic container is bound, `mint_object` has to start
            // remembering the `GetInventory` arguments.
            // **Per bound object, and this was wrong before.**
            //
            // The earlier note here claimed a container keyed on a real
            // item id "takes the shortcut" in `invCacheContainer.__init__`
            // and never asks us. That is true of `.itemID` only. The
            // `.typeID` and `.item` properties also call `_SetItemData`,
            // and its body is
            //
            // it asks the moniker for the container's own item when it has none
            // yet `[V]`
            //
            // -- `reg._item` starts `None` for *every* container, magic
            // key or not. So the capsule's own inventory asks too, and
            // with one shared answer it was told it was a station: its
            // dogma item would have been built with `typeID 2502` and
            // `GetClassForItem` would have picked structure semantics
            // instead of `ShipDogmaItem`.
            //
            // Reached synchronously from the hangar draw --
            // `SetShipDamage` -> `LoadItem` -> `GetItem` -> a bind on the
            // ship's inventory -> `GetSelfInvItem` -- so it is on the
            // critical path, not a side road.
            //
            // **Any item of mine, not only the active ship.** `cdl.GetItem`
            // asks this for the ship being boarded *before* `shipid` moves
            // (`docs/lanes/fitting.md` §6) [I], and the old "active ship or
            // the station" rule handed it the station's `typeID`. An item
            // that is not mine still gets the station row, as before.
            "GetSelfInvItem" => Ok(match container_of(req.bind_params) {
                Some(CONTAINER_HANGAR) | None => reg.station_self_item(req.actor),
                Some(id) => match reg.item(id) {
                    Some(i) if Some(i.owner_id) == req.actor => reg.item_row_value(i),
                    // CAP-H: a fleet member's ship whose bay I may open is its
                    // own self item, not the station's.
                    Some(i)
                        if req
                            .actor
                            .is_some_and(|a| crate::ship_config::can_open_any(reg, a, id)) =>
                    {
                        reg.item_row_value(i)
                    }
                    // A wreck is its own self item, whoever owns it: a
                    // wreck's `invCacheContainer` built with the station's
                    // `typeID` would be a structure to the client (P2.4).
                    Some(i) if req.actor.is_some_and(|a| wreck_open_to(reg, a, id)) => {
                        reg.item_row_value(i)
                    }
                    // A hacked can (HK3) is no store item: its own row.
                    None if req.actor.is_some_and(|a| wreck_open_to(reg, a, id)) => {
                        crate::hacking_loot::self_item(reg, id).map_or_else(
                            || reg.station_self_item(req.actor),
                            |i| reg.item_row_value(&i),
                        )
                    }
                    _ => reg.station_self_item(req.actor),
                },
            }),
            // A count of free turret hardpoints, as a plain int. Zero is
            // the true answer for a capsule, which has none.
            //
            // `None` is genuinely tolerated here -- all three callers
            // compare it (`None >= 2`, `None > n`) and Python 2 answers
            // `False` without raising -- so this is correctness rather
            // than a fix. It is worth doing anyway for the same reason
            // `GetMySecurityStatus` was worth doing: a `None` that
            // survives a comparison is a wrong answer that leaves no
            // trace, and the next caller may not be a comparison.
            //
            // A hull's `turretSlotsLeft` minus the turrets fitted to it, now
            // that both exist (`crate::fitting::HULLS`).
            "GetAvailableTurretSlots" => Ok(PyValue::Int(
                container_of(req.bind_params).map_or(0, |id| free_turrets(reg, req.actor, id)),
            )),
            // The GM item menu's "QA LOCK ITEM" / "QA UNLOCK ITEM"
            // entries, which `menusvc.GetGMMenu` puts on every inventory
            // item for a `ROLE_GML` session -- so they are reachable today
            // and, unanswered, they hang the tasklet that opened the menu.
            // Both return values are discarded.
            "QALockItem" | "QAUnlockItem" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // The item row descriptor `invCache` caches as `__itemhdr`.
        //
        // This one is **not** optional and not a stub. `invCache
        // .GetItemHeader` blocks on it, and `godma.sublocrd` then does
        // `for key, dbtype, size in list(reg.itemrd)`, so it must be a
        // real `blue.DBRowDescriptor`. Leaving it unanswered hung the
        // tasklet applying the session's initial state: the client sat at
        // `changing:ApplyInitialState` for 26 seconds, never activated a
        // view, and `viewStateSvc._GetOverlaysToEnable` then raised
        // `AttributeError: 'NoneType' object has no attribute 'view'`
        // on a black screen.
        "invbroker" => match req.method {
            "GetItemDescriptor" => Ok(item_descriptor()),
            // What is inside one container, named by the container's own
            // item id.
            //
            // **Three arguments, and only the first one selects
            // anything.** Three of the four call sites pass
            // `(containerItemID, dockableLocationID, solarSystemID)` --
            // `eve/client/script/environment/fittingSvc.py:173`
            // (`FittingDictForRemoteShip`),
            // `.../ui/shared/info/panels/panelFitting.py:335`
            // (`GetShipContents`) and
            // `.../ui/shared/info/shipInfoWindow.py:287`
            // (`_get_ship_contents`). The fourth,
            // `.../ui/shared/inventory/containerContentWindow.py:78`,
            // passes only `(containerItemID, stationID)`, so the arm must
            // tolerate two arguments as well as three.
            //
            // The owner's capture has
            // `GetContainerContents(100000001, 60012526, None)`, which is
            // our sandbox character's `ship_id` and their `station_id`:
            // the pod, and the station it is parked in.
            //
            // **Arguments 2 and 3 say where to look, not what to look
            // for.** `panelFitting.GetShipContents` builds them as
            //
            // `panelFitting.GetShipContents` starts from the item's location id;
            // when the item sits in cargo or a ship hangar it asks for the parent
            // ship's info and, if there is one, takes the docking location and
            // solar system from it `[V]`.
            //
            // -- i.e. the dockable (or solar system) that holds the
            // container, so a real cluster can route the query to the node
            // owning it. One node does not need them, and the honest
            // filter is **argument 1** matched against each row's
            // `locationID`: a fitted module or a cargo item has
            // `locationID == shipID`. `shipInfoWindow` catches
            // `RuntimeError('InvalidLocationID')` from this call, so
            // validating argument 2 would be legal -- we do not, because
            // refusing a question we can answer buys nothing.
            //
            // **This is the one inventory call where the bind params are
            // not the answer**, which is worth saying next to
            // `GetSelfInvItem` above. There, `(10004, None)` versus
            // `(<itemID>, 1)` *was* the whole question, because the call
            // carries no arguments of its own. Here the moniker is
            // `eveMoniker.GetInventoryMgr()` -- `('invbroker',
            // (stationID, groupStation))` while docked, or
            // `(solarsystemID, groupSolarSystem)` in space -- so
            // `container_of(req.bind_params)` would hand us the station
            // and tell us nothing argument 2 did not already say.
            //
            // **The row shape is not this lane's to choose.** `invCache
            // .ListIncludingContainers` merges `charMgr`'s answer and
            // this one into `cachedItemsAll[realItem.itemID]`, so both
            // services must emit item rows over `item_columns()`. That is
            // `Registry::owned_item_rows`, and its doc comment is the
            // contract -- see `docs/archive/plan-docked-subsystems.md`, "the one
            // cross-lane contract".
            //
            // `None` was wrong in three different ways at once:
            //
            // - `fittingSvc.GetFittingDictFromItems` iterates it three
            //   times -- through `CreateFittingData`, then as
            //   `{(x.typeID, x.flagID): x.itemID for x in visibleFittedItems}`,
            //   then `for eachItem in visibleFittedItems`;
            // - `containerContentWindow.GetContentsList` stores it on
            //   `self.contentsList` and then runs `for c in contents`;
            // - the two info-window panels wrap the call in
            //   `except AttributeError`, which does **not** catch the
            //   `TypeError` their own callers then take -- so the guard
            //   the census reports for them is no protection at all.
            //
            // Today the answer is always empty, and that is a correct
            // answer rather than a stub: the character owns exactly one
            // item, the capsule, and its `locationID` is the station, so
            // there is nothing inside the pod. The filter is written now
            // anyway, because it is the line that stops being trivial the
            // moment the item store lands (`docs/archive/plan-docked-subsystems.md`
            // round 2, step 2) and `owned_item_rows` starts returning more
            // than one row.
            "GetContainerContents" => Ok(PyValue::List(contents_of(
                reg.owned_item_rows(req.actor),
                Args::new(req).get(0),
            ))),
            // "Split Stack" on an item in the hangar:
            // `GetInventoryMgr().SplitStack(stationID, itemID, qty, ownerID)`,
            // the answer dropped (`POP_TOP`), then the client invalidates the
            // station's items and the container reloads
            // (`invItemFunctions.SplitStack:410`, **[V]**).
            "SplitStack" => split_stack(reg, req).map(|()| PyValue::None),
            // GAP-INVPACK: the crystal tooltip's wear, `pack.rs`.
            "GetDamageForCrystals" => Ok(pack::crystal_damage(reg, req)),
            // "Trash": `errors = GetInventoryMgr().TrashItems(itemIDs,
            // locationID)`; a truthy answer is iterated into dialogs
            // (`invItemFunctions.TrashInvItems:641-645`, **[V]**).
            "TrashItems" => trash_items(reg, req),
            // #57: `menusvc.AssembleContainer:3444`, answer dropped.
            "AssembleCargoContainer" => crate::containers::assemble(reg, req),
            // NES-BACKEND: PLEX between an item stack and the vault.
            "DepositPlexToVault" => crate::nes_vault::deposit(reg, req),
            "WithdrawPlexFromVault" => crate::nes_vault::withdraw(reg, req),
            // GAPS-2: `SetLabel` (rename a ship or a container).
            _ => match super::gaps2::invbroker(reg, req) {
                Some(answer) => answer,
                None => Err(reg.no_method(req)),
            },
        },
        // The ship access object, bound on `(stationID, groupStation)`
        // while docked -- the same 2-tuple `dogmaIM` and `crimewatch`
        // use, from `eveMoniker.GetShipAccess()`.
        //
        // **Both of the calls here are on the synchronous hangar-draw
        // path**, which makes them more dangerous than they look.
        // `hangarView.LoadCamera` calls `ReplaceExistingShipModel`
        // directly rather than on a tasklet, and `_OpenView` runs
        // `LoadView` / `ShowView` / `LoadCamera` together. So leaving one
        // unanswered stalls `_OpenView`, which means `ActivateView`'s
        // `finally: HideAllLoad()` never runs and `OnClientReady` never
        // fires: a stuck loading screen. A *wrong* answer is caught by
        // that function's bare `except` and merely logs. That asymmetry
        // is section 1's corollary at its sharpest.
        "ship" => match req.method {
            // How dirty the hull looks. `CalcDirtLevelFromAge` does
            //
            //     blue.os.TimeDiffInMs(lastCleanTimeStamp, now)
            //
            // with no `None` guard, so this must be a real win64
            // filetime. "Now" means a clean pod; the in-space sibling
            // guards with `if reg._dirtTimeStamp is not None`, which is
            // good evidence this path assumes a real one.
            "GetDirtTimestamp" => Ok(PyValue::Long(crate::pyutil::filetime_now())),
            // **The return value is not the answer.**
            //
            // `_DoUndockAttempt` wraps this in
            // `PerformSessionChange('undock', shipsvc.Undock, ...)` and
            // then `POP_TOP`s the result; what actually takes the client
            // out of the station is the `SessionChangeNotification` that
            // goes out with it. See `GpsSession::session_change_for`.
            //
            // It arrives as a **fused bind**, not a call on an existing
            // object: the client releases its station `ship` object after
            // thirty seconds and re-binds on the undock click. That is
            // why the log shows `ship.Undock/2 (bound object)`.
            // SESSION-FIX2: Escape > Log Off in space. The answer is the
            // list of failed conditions (`[]` = counting down); see
            // `crate::safe_logoff`.
            "SafeLogoff" => {
                crate::safe_logoff::start(reg, req.actor, crate::pyutil::filetime_now())
            }
            "AbortSafeLogoff" => Ok(crate::safe_logoff::abort(reg, req.actor)),
            // GAP-SELFDESTRUCT: the 2-minute sequence; see `crate::self_destruct`.
            "SelfDestruct" => crate::self_destruct::start(
                reg,
                req.actor,
                Args::new(req),
                crate::pyutil::filetime_now(),
            ),
            "AbortSelfDestruct" => Ok(crate::self_destruct::abort(
                reg,
                req.actor,
                crate::pyutil::filetime_now(),
            )),
            "Undock" => {
                // T3-D: a strategic cruiser leaves only with all four
                // subsystems. A raise here also keeps the session change
                // back (`handle_bound_call` returns before it).
                let ship = req
                    .actor
                    .and_then(|id| reg.character(id))
                    .map(|c| c.ship_id);
                if ship.is_some_and(|s| !crate::subsystems::may_undock(reg.store(), s)) {
                    return Err(bare_user_error("TooFewSubSystemsToUndock"));
                }
                // `onlineModules` is not honoured
                // (`modules::sync_undock_online` says why).
                Ok(PyValue::None)
            }
            // **Boarding** (`docs/lanes/fitting.md` §1.2). The return value
            // *is* the `shipState` 4-tuple -- `_MakeShipActive` unpacks
            // `PerformSelectiveSessionChange('board', …, Board, shipID,
            // oldShipID)` unguarded (`cdl:303`) [V] -- and the
            // `{'shipid': (old, new)}` session change is what re-primes
            // godma (`godma:1384-1407`) [V]. The change is queued only once
            // the move succeeded, which is why it is not a
            // `session_change_for` arm: that one fires on the error path too.
            "Board" => board(reg, req),
            // #77 BE-2: the hull is left in space with nobody in it.
            "Eject" => board_eject::eject(reg, req),
            // The way back to the capsule (§1.4). The station path discards
            // the reply (`base:298`) and the structure path reads it as the
            // capsule's id (`structureDocking.py:91`) [V], so it is the id.
            "LeaveShip" => leave_ship(reg, req),
            // A packaged ship made boardable again (§1.5).
            "AssembleShip" => assemble_ship(reg, req),
            // FITFIX-1: the multi-fit window's "Fit N ships" (`fit_ships.rs`).
            // It was unanswered, so the window closed and nothing happened.
            "FitShips" => fit_ships::fit_ships(reg, req),
            // CAP-H: who may use the fleet hangar and the maintenance bay
            // (`shipConfigSvc.py`) [V].
            "GetShipConfiguration" => Ok(match Args::new(req).int(0) {
                Some(ship_id) => crate::ship_config::configuration_value(reg, ship_id),
                None => crate::ship_config::ShipConfig::DEFAULT.to_value(),
            }),
            "ConfigureShip" => crate::ship_config::configure(reg, req.actor, Args::new(req).get(0)),
            // One saved fitting a crate is about to hand over, and it is
            // consumed by `UNPACK_SEQUENCE 4`:
            //
            // The client (`[V]`) unpacks the answer of `ship.GetShipFittingInfo`
            // into name, icon, ship type id and insurance, four elements.
            //
            // at `eve/client/script/ui/crate/controller.py` lines 76 and
            // 252 -- both named `LoadSpecialLoot`, one on
            // `CrateController` and one on `CrateStackController`, and the
            // only two call sites in the client. Neither has a guard in
            // front of it, so `None` is
            // `TypeError: 'NoneType' object is not iterable` *before* any
            // of the four elements is looked at. That is row four of
            // `docs/guides/debugging-the-client.md` §4's table: the unpack
            // happens before the test, so testing for `None` afterwards
            // cannot save it. **The arity is the contract.**
            //
            // What each element is, from
            // `eve/client/script/ui/crate/lootItem.py` (lines 16 and 72):
            //
            // `lootItem.py:16,72` (`[V]`): the base loot item stores an icon (a
            // class default when none is passed); the special variant adds a name
            // and answers `get_name` with the label for that name.
            //
            // - **`name` is a localization label path, not a display
            //   name.** `get_name` (lootItem.py:72) hands it to
            //   `GetByLabel`, so `u''` is
            //   the empty of the right family. It cannot raise:
            //   `localization/localizationBase.Get` opens
            //   `if not messageIDorLabel: return`, and `GetByLabel` turns
            //   that `None` into `u'[no label: ]'` -- a visible
            //   placeholder, which is the right symptom for a fitting the
            //   server does not have.
            // - **`icon` is a resource path and `None` is what the client
            //   asks for**: `self.icon = icon or self.ICON_PATH` falls
            //   back to the class's own texture, so `None` selects the
            //   default rather than breaking the draw. This is an
            //   `or`-fallback and *not* a cache -- the falsy-value trap
            //   named in this round's brief is
            //   `if x is None: self._c = remote()` (and
            //   `if not x: ...; raise`), and there is none of either in
            //   this file's call path.
            // - **`ship_type_id` and `insurance` are read only under
            //   `if insurance:`**, which decides whether an
            //   `InsuranceLootItem` is appended at all. `insurance` is
            //   then formatted straight into
            //   `'UI/Insurance/QuoteWindow/%s'`, so a falsy value is
            //   exactly what stops a quote label being built out of
            //   nothing. `None` is preferred to `0` for both: "there is no
            //   insurance quote", not "quote level zero", and a `0`
            //   `ship_type_id` would be a typeID FSD does not know if the
            //   branch were ever taken.
            //
            // **Reachability, stated plainly rather than talked up.** This
            // is *not* reached merely by opening the crate window. Both
            // call sites need `getattr(self.staticData, 'fittings', [])`
            // to be non-empty for the FSD crate type, and both sit behind
            // a `crateService` call we do not answer at all --
            // `OpenCrate` -> `crateService.OpenCrate` ->
            // `LoadSpecialLoot`, and `GetSortedLoot` ->
            // `PeekInsideFixedCrate` -> `crateService
            // .PeekInsideFixedCrate` -> `LoadSpecialLoot` for a crate
            // whose `lootPresentationType` is `'fixed'`. The second path
            // *is* a window-open path, which is why this is worth closing
            // now; but a `crateService` arm has to exist before either can
            // fire, so no present traceback belongs to it.
            "GetShipFittingInfo" => Ok(PyValue::Tuple(vec![
                PyValue::Unicode(String::new()),
                PyValue::None,
                PyValue::None,
                PyValue::None,
            ])),
            // **Drones** (`docs/lanes/drones.md`), bound in space on
            // `(solarsystemid, groupSolarSystem)`: the launch
            // (`eveMisc.LaunchFromShip:79`) and the scoop
            // (`droneFunctions.RealScoopToDroneBay:214`) [V].
            "LaunchDrones" => crate::drones::launch_call(reg, req),
            "ScoopDrone" => crate::drones::scoop_call(reg, req),
            // SOC-D2: "Launch for Corporation" of a Sovereignty Hub,
            // `eveMisc.LaunchFromShip` for a non-drone stack [V].
            // #82: a Mobile Depot or an MTU is launched the same way.
            "Drop" => match crate::mtu::drop_call(reg, req) {
                Some(answer) => answer,
                // #172: a Mobile Warp Disruptor, anchored afterwards.
                None => match crate::anchor::drop_call(reg, req) {
                    Some(answer) => answer,
                    // SKY-1 (#81): a skyhook, before the hub's link.
                    None => match crate::skyhook::drop_call(reg, req) {
                        Some(answer) => answer,
                        // MERC-1 (#83): a mercenary den, before the hub's link.
                        None => match crate::merc_den::drop_call(reg, req) {
                            Some(answer) => answer,
                            None => super::sov_hub::drop_call(reg, req),
                        },
                    },
                },
            },
            // GAPS-2: `Jettison` and `Scoop`.
            _ => match super::gaps2::ship(reg, req) {
                Some(answer) => answer,
                None => Err(reg.no_method(req)),
            },
        },
        // Killmarks painted on the hull. A plain service with exactly one
        // method in the whole client.
        //
        // An int: `min(killCounter, 999)` goes straight onto a trinity
        // property, and the other consumer opens
        // `if not killmarksCount: return`, which proves falsy is
        // expected.
        "shipKillCounter" => match req.method {
            "GetItemKillCountPlayer" => Ok(crate::killmarks::count_call(reg, Args::new(req))),
            _ => Err(reg.no_method(req)),
        },
        // Fighters. **A 3-tuple, and the third element must be a dict.**
        //
        // `[V]` the client unpacks `fightersSvc.GetFightersForShip()` into
        // three parts; the first is iterated as rows of tube flag, fighter
        // item, fighter type and squadron size, and the third is walked with
        // `iteritems()` as fighter id to ability slot states.
        //
        // Unpacked with no guard in front of it, which is why `None` was
        // `TypeError: 'NoneType' object is not iterable` out of
        // `ShipFighterState.OnSessionChanged` -- it runs on any `shipid`
        // change, so giving the character a capsule is exactly what
        // started triggering it.
        //
        // There is no "this hull carries no fighters" sentinel. A capsule
        // must still be answered, and `([], [], {})` is the answer: all
        // three loops run zero times. The third element cannot be `[]`,
        // because `.iteritems()` is called on it.
        //
        // Worth recording for the day we serve real ones: the wire row is
        // `(tubeFlagID, fighterItemID, fighterTypeID, squadronSize)`, and
        // the client immediately reshuffles it into
        // `FighterInTubeTuple(fighterItemID, fighterTypeID, tubeFlagID,
        // squadronSize)`. **The namedtuple's field order is not the wire
        // order.** `tacticalNavigationService._ReloadFighters` confirms
        // the wire order independently by unpacking element 1 as the
        // fighter id.
        //
        // **Real now** (FT-B/FT-C, `crate::fighters`): the tubes' rows, the
        // squadrons in space, and load, unload, launch and recall.
        "fighterMgr" => crate::fighters::dispatch(reg, req),
        // L2 SKINS: `shipCosmeticsMgr` (the hull emblem list and the SKIN licences)
        // moved to `service/cosmetics.rs`.
        // Dogma: the character's attributes and effects.
        //
        // **The gate.** Bound on `(stationID, groupStation=15)`, called
        // first in the blocking `ProcessSessionChange` wave because
        // `invCache` puts itself at the front with `FavourMe`. Hang here
        // and the session write lock is never released.
        //
        // It cannot be answered empty, which is what makes it a data
        // structure rather than a stub. `godma.Prime` ends with an
        // **unconditional**
        //
        // `[V]` the client reads the character item and fires `OnAttribute`
        // for each of the five character attributes (memory, perception,
        // willpower, charisma, intelligence).
        //
        // so the character must really be primed: a falsy `charInfo`
        // makes `GetItem` return None and `getattr(None, 'memory')`
        // raises inside `Prime`.
        //
        // (Those attribute names are worth a note. uncompyle6 renders
        // that list as `[3, 4, 5, 6, 7]` -- it prints const-table
        // *indices* rather than values. Integer `getattr` would have
        // raised `TypeError` unconditionally, which is the tell. Read the
        // const table before trusting a list of small ints in
        // decompiled output.)
        // Repackaging, implemented from **CCP's own validation**, which ships
        // in this client: `repackaging/__init__.py` is 121 lines and is the
        // whole of it -- `CheckCanRepackage` (`:46`), `CanRepackageType`
        // (`:82`), `CanRepackageInLocation` (`:72`) and
        // `GetContentForRepackaging` (`:93`). `crate::item`'s
        // `can_repackage_type` and `is_ship_fitting_flag` are that file,
        // transcribed with its constants. Nothing here is a guess about what
        // the rule should be.
        //
        // **This is the arm where the push is the whole deliverable.**
        // `invItemFunctions.RepackageItems` discards our return value
        // (`:583`, `POP_TOP`) and then does exactly one thing:
        // `PlaySound('ship_goals_repackage_ship_play')` (`:606`). Unlike
        // "Stack All" there is **no `Refresh`**, so if we change an item and
        // say nothing the client keeps showing an assembled ship for the rest
        // of the session -- and `invCacheContainer.listed` has no TTL, so
        // closing and reopening the window does not re-ask (`item-spawn.md`
        // §2). `Registry::apply_items` queues `OnInvalidateCacheForLocations`
        // for the hangar, which drops the container so the next build does
        // re-ask, plus an `OnItemChange` carrying the new row with
        // `singleton == 0` at index 10.
        //
        // The argument is **one positional dict**, not a list:
        // `{stationID: [(itemID, locationID), ...]}` built at `:579`-`:581`
        // from `invCache.GetStationIDOfItem(item) or item.locationID`. The
        // `destroyable=True` keyword only appears on the *retry* after a
        // `Confirm*WithUpgrades` refusal, which we never raise.
        //
        // On a refusal the client compares `exception.msg` against exactly
        // two keys and **bare-re-raises anything else** (`:605`,
        // `RAISE_VARARGS 0`), so the key we pick is shown as-is. Both keys
        // used below are in `findings/user-errors.tsv` with a real FSD
        // message id, which `RepackageItemNotSingleton` is **not** -- it is
        // `pyc`-only, so raising it would hand `prompt_player_fsd_dialog` a
        // label that does not exist. "Already packaged" is therefore a
        // skip here, never a raise.
        "repackagingSvc" => match req.method {
            "RepackageItems" => repackage_items(reg, req),
            _ => Err(reg.no_method(req)),
        },
        "dogmaIM" => match req.method {
            "GetAllInfo" => Ok(reg.all_info(req.actor)),
            // The drone window's in-bay health, from
            // `InBayDroneDamageTracker.FetchInBayDroneDamageToServer:43`,
            // which iterates `.iteritems()` [V]. No drone takes damage on this
            // server yet (`drones.md` §6), so every asked drone is whole. It
            // must have an entry: the tracker re-asks for any id it holds no
            // state for (`FindDronesMissingDamageState:53`), and `{}` made it
            // ask again on every refresh, pinning the client's main thread.
            "GetLayerDamageValuesByItems" => Ok(layer_damage_whole(Args::new(req).get(0))),
            // `{attributeDroneIsAggressive: bool, attributeDroneFocusFire:
            // bool}`, sent by `godma.StateManager.SendDroneSettings` **before
            // every launch** (`droneFunctions.LaunchDrones:228`,
            // `godma:2704-2708`) and on a settings change (`:2722`); the
            // reply is discarded (`POP_TOP`) [V]. Unanswered, it raises and
            // the launch is never sent. K-DRONE: the pilot's aggressive and
            // focus-fire settings (`drones::change_settings`).
            "ChangeDroneSettings" => crate::drones::settings_call(reg, req),
            // Skillbook injection (`skillsvc.InjectSkillIntoBrain`, one list
            // of item ids) [V]. The skill lane's: `character.rs`, `Skill
            // training` section, has the shape and the rules.
            "InjectSkillIntoBrain" => character::inject_skill_into_brain(reg, req),
            // Implants (`crate::implants`): `InjectImplant(itemID)` from
            // `invItemFunctions.PlugInImplant`, `DestroyImplant(itemID)`
            // from the Implants panel's Unplug entry; both discard the
            // answer and show a `UserError`.
            "InjectImplant" => crate::implants::inject_implant(reg, req),
            "DestroyImplant" => crate::implants::destroy_implant(reg, req),
            // Boosters (`crate::boosters`): `UseBooster(itemID, locationID)`
            // from `invItemFunctions.ConsumeBooster`, result discarded.
            "UseBooster" => crate::boosters::use_booster(reg, req),
            // "Board my Corvette" (`station/base.pyc::CreateNewbieShip`,
            // `(shipID, locationID)`, reply discarded at `:723`) [V]. The
            // rookie ship lane's: `crate::starter_ship` has the rules (E7).
            "CreateNewbieShip" => crate::starter_ship::create_newbie_ship(reg, req.actor),
            // godma asks for a module fitted after the prime, from
            // `UpdateItem` (`godma:1949`), and tolerates `None`
            // (`if infoz is None: return`) [V]. The rows are the ones
            // `GetAllInfo` primes, from the same builders.
            "ItemGetInfo" => Ok(match Args::new(req).get(0) {
                Some(PyValue::Int(id)) => item_get_info(reg, req.actor, *id),
                // A loaded charge's key, `(shipID, flagID, typeID)`
                // (`godma:1949` passes whatever key it holds) [V].
                Some(PyValue::Tuple(key)) => charge_get_info(reg, req.actor, key),
                _ => PyValue::None,
            }),
            // **Module activation, the whole of it on this side.** The client
            // turns a module on *locally first* and then calls us; on any
            // error but `EffectAlreadyActive2` it calls
            // `StopEffect(effectOnline, moduleID)` and the module drops back
            // to offline (`clientDogmaLocation.py:857`). That is why this
            // raised a `CustomNotify` until now rather than answering `None`:
            // `None` left a module showing online with nothing behind it.
            // It answers the truth now. `crate::modules` has the rest;
            // `docs/re/modules.md` is the spec.
            //
            // `SetModuleOnline(shipID, moduleID)` and
            // `TakeModuleOffline(shipID, moduleID)` are the fitting window's
            // pair; the HUD's "Put Online" is `Activate(itemID, 'online',
            // ...)` instead, and both routes meet in the same state.
            "SetModuleOnline" | "TakeModuleOffline" => match Args::new(req).int(1) {
                // #41: `SetModuleOnline(structureID, moduleID)` from the
                // pilot at a structure's controls is a service module's, not
                // a ship's (`structure_services::set_online`).
                Some(module)
                    if req.actor.is_some_and(|a| {
                        Args::new(req).int(0).is_some()
                            && crate::structure_dock::control::controlling(reg, a)
                                == Args::new(req).int(0)
                    }) =>
                {
                    crate::structure_services::set_online(
                        reg,
                        req.actor.unwrap_or_default(),
                        Args::new(req).int(0).unwrap_or_default(),
                        module,
                        req.method == "SetModuleOnline",
                        crate::pyutil::filetime_now(),
                    )
                }
                Some(module) => crate::modules::set_module_online(
                    reg,
                    req.actor,
                    module,
                    req.method == "SetModuleOnline",
                    crate::pyutil::filetime_now(),
                ),
                _ => Err(Args::new(req).expected("(shipID, moduleID)")),
            },
            // `Activate(itemID, effectName, target, repeat)`, four positional
            // `[V]` (`godma:2427`, and `findings/bound-objects.json` agrees).
            // `target` is `None` or an int (`godma:2437` raises client-side
            // otherwise). **The reply is discarded** (`smb:1580`, `POP_TOP`):
            // `OnGodmaShipEffect` is the answer that matters.
            //
            // **An int second argument is an effect id** (`menusvc:2447`,
            // `Activate(itemID, effectAnchorDrop)`, two args): the anchor
            // drop and lift of a deployable (#172, `crate::anchor`) [V].
            "Activate"
                if matches!(
                    (Args::new(req).int(0), Args::new(req).int(1)),
                    (Some(_), Some(e)) if crate::anchor::is_anchor_effect(e)
                ) =>
            {
                match (Args::new(req).int(0), Args::new(req).int(1)) {
                    (Some(item), Some(effect)) => crate::anchor::activate(
                        reg,
                        req.actor,
                        item,
                        effect,
                        crate::pyutil::filetime_now(),
                    ),
                    _ => Err(Args::new(req).expected("(itemID, effectID)")),
                }
            }
            "Activate" => match (Args::new(req).int(0), Args::new(req).text(1)) {
                (Some(module), Some(name)) => {
                    let target = Args::new(req).int(2);
                    let repeat = Args::new(req).int(3).unwrap_or(0);
                    crate::weapon_bank::activate(
                        reg,
                        req.actor,
                        module,
                        &name,
                        target,
                        repeat,
                        crate::pyutil::filetime_now(),
                    )
                }
                _ => Err(Args::new(req).expected("(itemID, effectName, target, repeat)")),
            },
            // **Overload** (K-OVERHEAT, `crate::overheat`): `Overload(itemID,
            // effectID)` and `StopOverload` answer the module id, the rack
            // calls the ids they changed (`godma:2014-2067`) [V].
            "Overload" | "StopOverload" => match Args::new(req).int(0) {
                Some(module) => {
                    let effect = Args::new(req).int(1).unwrap_or(0);
                    let now = crate::pyutil::filetime_now();
                    // A bank heats through its master: each member too.
                    let slaves: Vec<i64> = crate::weapon_bank::bank_of(reg, req.actor, module)
                        .into_iter()
                        .filter(|m| *m != module)
                        .collect();
                    let reply = if req.method == "Overload" {
                        crate::overheat::overload(reg, req.actor, module, effect, now)
                    } else {
                        crate::overheat::stop_overload(reg, req.actor, module, effect, now)
                    };
                    for m in slaves {
                        let _ = if req.method == "Overload" {
                            crate::overheat::overload(reg, req.actor, m, effect, now)
                        } else {
                            crate::overheat::stop_overload(reg, req.actor, m, effect, now)
                        };
                    }
                    reply
                }
                None => Err(Args::new(req).expected("(itemID, effectID)")),
            },
            "OverloadRack" | "StopOverloadRack" => match Args::new(req).int(0) {
                Some(module) => {
                    let now = crate::pyutil::filetime_now();
                    if req.method == "OverloadRack" {
                        crate::overheat::overload_rack(reg, req.actor, module, now)
                    } else {
                        crate::overheat::stop_overload_rack(reg, req.actor, module, now)
                    }
                }
                None => Err(Args::new(req).expected("(itemID)")),
            },
            // **Module repair** with Nanite Repair Paste (`godma:2130-2190`).
            "InitiateModuleRepair" | "StopModuleRepair" => match Args::new(req).int(0) {
                Some(module) => {
                    let now = crate::pyutil::filetime_now();
                    if req.method == "InitiateModuleRepair" {
                        crate::overheat::initiate_repair(reg, req.actor, module, now)
                    } else {
                        crate::overheat::stop_repair(reg, req.actor, module, now)
                    }
                }
                None => Err(Args::new(req).expected("(itemID)")),
            },
            "InitiateModuleRepairMany" => match Args::new(req).get(0).map(id_list) {
                Some(modules) => crate::overheat::initiate_repair_many(
                    reg,
                    req.actor,
                    &modules,
                    crate::pyutil::filetime_now(),
                ),
                None => Err(Args::new(req).expected("(itemIDs)")),
            },
            // **Weapon banks** (K-OVERHEAT, `crate::weapon_bank`): the ship id
            // is the first argument of each and is ours to know
            // (`clientDogmaLocation:728-840`) [V].
            "LinkWeapons" | "MergeModuleGroups" | "PeelAndLink" => {
                match (Args::new(req).int(1), Args::new(req).int(2)) {
                    (Some(to), Some(from)) => {
                        let now = crate::pyutil::filetime_now();
                        match req.method {
                            "LinkWeapons" => {
                                crate::weapon_bank::link_weapons(reg, req.actor, to, from, now)
                            }
                            "MergeModuleGroups" => {
                                crate::weapon_bank::merge_groups(reg, req.actor, to, from, now)
                            }
                            _ => crate::weapon_bank::peel_link(reg, req.actor, to, from, now),
                        }
                    }
                    _ => Err(Args::new(req).expected("(shipID, masterID, moduleID)")),
                }
            }
            "LinkAllWeapons" => {
                crate::weapon_bank::link_all_weapons(reg, req.actor, crate::pyutil::filetime_now())
            }
            "UnlinkAllModules" => crate::weapon_bank::unlink_all(reg, req.actor),
            "UnlinkModule" | "DestroyWeaponBank" => match Args::new(req).int(1) {
                Some(module) => {
                    if req.method == "UnlinkModule" {
                        crate::weapon_bank::unlink_module(reg, req.actor, module)
                    } else {
                        crate::weapon_bank::destroy_bank(reg, req.actor, module)
                    }
                }
                None => Err(Args::new(req).expected("(shipID, masterID)")),
            },
            // `LaunchProbes(moduleID, numProbes)` -- the probe scanner
            // window's launch (`scanSvc.FindModuleAndLaunchProbes:558`)
            // [V]. Return discarded. `crate::scan` has the rest.
            "LaunchProbes" => crate::scan::launch_probes(
                reg,
                req.actor,
                Args::new(req),
                crate::pyutil::filetime_now(),
            ),
            // `Deactivate(itemID, effectName)`, two positional `[V]`
            // (`godma:2471`).
            "Deactivate" => match (Args::new(req).int(0), Args::new(req).text(1)) {
                (Some(module), Some(name)) => crate::weapon_bank::deactivate(
                    reg,
                    req.actor,
                    module,
                    &name,
                    crate::pyutil::filetime_now(),
                ),
                _ => Err(Args::new(req).expected("(itemID, effectName)")),
            },
            // **Charges (P2.3).** `LoadAmmo(shipID, moduleID(s),
            // chargeItemIDs, chargeLocationID)`, four positional, reply
            // discarded (`clientDogmaLocation.py:1201`, `:1206`) [V]: one
            // module id from every single-module route, several from "load
            // to all". `crate::modules::load_ammo` has the rest. The shipID
            // is ours to know and is not trusted.
            "LoadAmmo" => match (
                Args::new(req).get(1).map(id_list),
                Args::new(req).get(2).map(id_list),
                Args::new(req).int(3),
            ) {
                (Some(modules), Some(charges), Some(location)) if !modules.is_empty() => {
                    crate::modules::load_ammo(
                        reg,
                        req.actor,
                        &crate::weapon_bank::expand(reg, req.actor, &modules),
                        &charges,
                        location,
                        crate::pyutil::filetime_now(),
                    )
                }
                _ => Err(Args::new(req)
                    .expected("(shipID, moduleID(s), chargeItemIDs, chargeLocationID)")),
            },
            // `UnloadAmmo(shipID, moduleID(s), destination[, quantity])`:
            // three positional from the fitting window and the module menu,
            // four from a drag of the loaded charge (`clientDogmaLocation.py`
            // `:1328`, `:1330`, `:1366`, `:1377`) [V]. `destination` is a
            // container id or `(locationID, ownerID, flag)`.
            //
            // It raised `CustomNotify` until now, and deliberately not
            // `NotEnoughCargoSpace`: the handler at `:1369` swaps that key for
            // a weapon-bank message, so it would have claimed a full hold.
            "UnloadAmmo" => match (Args::new(req).get(1).map(id_list), Args::new(req).get(2)) {
                (Some(modules), Some(destination)) if !modules.is_empty() => {
                    let quantity = Args::new(req).int(3);
                    crate::modules::unload_ammo(
                        reg,
                        req.actor,
                        &crate::weapon_bank::expand(reg, req.actor, &modules),
                        destination,
                        quantity,
                        crate::pyutil::filetime_now(),
                    )
                }
                _ => Err(Args::new(req).expected("(shipID, moduleID(s), destination)")),
            },
            // Both are **iterated**, in `godma.RefreshTargets`:
            //
            // `[V]` it fires an `OnTarget` add event for each id in `GetTargets()`.
            //
            // so `None` is a `TypeError`, not an empty answer.
            //
            // **And this one is on the view-open path rather than in the
            // background**, which is what makes it load-bearing for
            // undocking. `targetMgr.Run` calls `RefreshTargets()`
            // *synchronously* when `session.shipid and not
            // session.stationid` -- i.e. exactly once we are in space --
            // and `target` is one of `SpaceView.__dependencies__`, which
            // `viewStateSvc._OpenView` starts with
            // `StartServiceAndWaitForRunningState` **outside any try
            // block**. A raise here aborts `_OpenView` before `LoadView`
            // ever runs and poisons the `target` service for the rest of
            // the session, so the space view would simply never appear.
            //
            // This arm also serves the *re-bound* `dogmaIM` on
            // `(solarsystemID, 5)`. `GetAllInfo` needs no change: the
            // client re-binds after the session change and fuses a fresh
            // `GetAllInfo` onto it, and `all_info()` ignores its
            // arguments.
            //
            // **They answer the real lists now.** `[]` for ever meant every
            // lock vanished at the next session change, because this pair is
            // the client's only way to rediscover them.
            "GetTargets" => Ok(crate::targeting::targets_of_actor(reg, req.actor)),
            "GetTargeters" => Ok(crate::targeting::targeters_of_actor(reg, req.actor)),
            // **The lock, and the whole of it on this side.** `(0, [])` --
            // a falsy flag is what makes `_LockTarget` call `OnTargetAdded`,
            // which draws the bracket, opens the target box and stops the
            // sound. No push to the locker, no destiny action.
            // `docs/re/targeting.md`.
            "AddTarget" => match Args::new(req).int(0) {
                Some(target) => match req.actor {
                    Some(actor) => crate::targeting::lock(reg, actor, target),
                    None => Ok(PyValue::Tuple(vec![PyValue::Int(0), PyValue::List(vec![])])),
                },
                // An unreadable target is not a zero: `AddTarget(0)` would
                // be a lock on a ball that does not exist. The return value
                // still has to unpack, so this is the "locked nothing"
                // answer rather than a raise.
                None => Ok(PyValue::Tuple(vec![PyValue::Int(0), PyValue::List(vec![])])),
            },
            // Return discarded (`POP_TOP` at `targetMgr.py:1613`), so the
            // push is the reply that matters -- and it is not optional:
            // `RemoveFromTeam` is a no-op for an ordinary pilot, so without
            // `OnTarget('lost', ...)` the target box stays up for ever.
            "RemoveTarget" => {
                if let (Some(actor), Some(target)) = (req.actor, Args::new(req).int(0)) {
                    crate::targeting::unlock(
                        reg,
                        actor,
                        target,
                        crate::targeting::Reason::Unlocked,
                    );
                }
                Ok(PyValue::None)
            }
            "RemoveTargets" => {
                if let (Some(actor), Some(targets)) = (req.actor, Args::new(req).ids(0)) {
                    for target in targets {
                        crate::targeting::unlock(
                            reg,
                            actor,
                            target,
                            crate::targeting::Reason::Unlocked,
                        );
                    }
                }
                Ok(PyValue::None)
            }
            "ClearTargets" => {
                if let Some(actor) = req.actor {
                    crate::targeting::clear_all(reg, actor, crate::targeting::Reason::Unlocked);
                }
                Ok(PyValue::None)
            }
            // **Drops the lock, and sends nothing.** `CancelAddTarget` does
            // the whole client side itself -- clears the state, stops the
            // sound, scatters `OnFailLockTarget` -- and *then* calls us
            // **[V]**, so anything we sent would describe something the
            // client already did.
            //
            // This was a true no-op while `AddTarget` answered "locked"
            // immediately, because there was no pending lock to cancel. Now
            // that a lock takes time there is one, and leaving it would make
            // `GetTargets` list a target the player abandoned.
            // `crate::targeting::cancel` has the one race this leaves.
            "CancelAddTarget" => {
                if let (Some(actor), Some(target)) = (req.actor, Args::new(req).int(0)) {
                    crate::targeting::cancel(reg, actor, target);
                }
                Ok(PyValue::None)
            }
            _ => Err(reg.no_method(req)),
        },
        // Saved fittings. Three calls, one shape, and it is **a plain
        // dict** -- not a rowset of either family.
        //
        // `fittingSvc.PrimeFittings` stores what we send *verbatim* into
        // `reg.fittings[ownerID]`, and the client then mutates that very
        // object: `del reg.fittings[ownerID][fittingID]`,
        // `reg.fittings[ownerID][fittingID] = fitting`,
        // `fittings.update(...)`. No rowset family has `__delitem__` or
        // `__setitem__`, so this one is settled by what the client *does*
        // to the value rather than by what it reads off it.
        //
        // Keys are int fittingIDs. Values would be `utillib.KeyVal`
        // carrying `fittingID, ownerID, name, description, shipTypeID,
        // fitData` -- the client writes attributes back onto them
        // (`...[fittingID].name = name`) and calls `.copy()`, which rules
        // out `blue.DBRow` on both counts.
        //
        // Empty is the honest answer and it is sufficient: a new
        // character has saved no fittings, and
        // `fittingBrowser.GetFittingScrolllist` has an explicit
        // `len(fittings) == 0` branch that appends a "Nothing Found" row.
        //
        // These are reached three ways, so all three must answer:
        // `GetAllFittingsForCharacter` fires char/corp/community/alliance
        // in one `uthread.parallel` and unpacks the result positionally;
        // the filter buttons' tooltips do `len()` on char and corp; and
        // `PersistFitting` does `len()` before saving. The alliance one
        // never reaches us -- `GetAllianceFittings` short-circuits to a
        // client-side `{}` when `session.allianceid` is unset, which is
        // why `allianceFittings = {}` sat in the traceback's locals next
        // to three `None`s and told us the shape.
        //
        // The mutators (`SaveFitting`, `UpdateFitting`, `DeleteFitting`, ...)
        // and the stored lists are `crate::fitting_store` (FITTING-MARKET).
        "charFittingMgr" | "corpFittingMgr" | "allianceFittingMgr" => match req.method {
            // `GetFittings(ownerID)` on charFittingMgr, and the same
            // method on corpFittingMgr: the client picks the service from
            // the ownerID and runs identical code either way.
            "GetCommunityFittings" => Ok(PyValue::Dict(vec![])),
            // FITTING-MARKET: saved fittings, `crate::fitting_store`.
            _ => {
                crate::fitting_store::dispatch(reg, req).unwrap_or_else(|| Err(reg.no_method(req)))
            }
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// The subset of `rows` that is inside the container `container`.
///
/// `container` is `GetContainerContents`' **first** argument, straight off
/// the wire; the match is against each row's `locationID`, which is how the
/// client itself decides where an item is (`StationItems.IsItemHere` tests
/// `item.locationID == session.stationid`, `GetParentShipInfo` walks
/// `locationID` upwards).
///
/// A missing or non-integer first argument answers `[]` rather than
/// everything: "you did not name a container" and "that container is empty"
/// are the same observable answer, and the alternative -- leaking the whole
/// hangar into a ship-contents panel -- would be a wrong answer that looks
/// like a right one.
fn contents_of(rows: Vec<PyValue>, container: Option<&PyValue>) -> Vec<PyValue> {
    let Some(PyValue::Int(container_id)) = container else {
        return vec![];
    };
    rows.into_iter()
        .filter(|row| row_location_id(row) == Some(*container_id))
        .collect()
}

/// A row's `locationID`, by column **name**.
///
/// By name and not by index on purpose: `item_columns()` in the parent is
/// documented as having had its order corrected once already, and a
/// positional read here would have gone on compiling and started answering
/// the wrong question. `None` means the value is not an item row of ours,
/// which `contents_of` treats as "not in this container".
fn row_location_id(row: &PyValue) -> Option<i64> {
    let PyValue::DbRow(row) = row else {
        return None;
    };
    let column = row.descriptor().index_of(b"locationID")?;
    match row.values().get(column)? {
        DbValue::Int(id) => Some(*id),
        _ => None,
    }
}

/// The address "Stack All" was asked about, or `None` if it is not one we can
/// stack.
///
/// The bound inventory names its container the same way `List` reads it --
/// `container_of(req.bind_params)`, from the `GetInventory` /
/// `GetInventoryFromId` call that minted the object -- and the only container
/// with contents today is `containerHangar`, whose real address is the
/// character's station at `flagHangar`. Anything else answers `None`: a pod's
/// own inventory is genuinely empty, so refusing to stack it leaves nothing
/// unstacked.
///
/// Undocked answers `None` too, and that is a deliberate narrow reading
/// rather than an oversight: `(containerHangar, None)` in space is the hangar
/// of a station we are not at, and this arm has no way to know which one.
/// `invCtrl.StationItems` is only reachable from a station window, so the
/// client does not ask.
fn stackable_slot(reg: &Registry, req: &Request<'_>) -> Option<crate::item::Slot> {
    if container_of(req.bind_params)? != CONTAINER_HANGAR {
        return None;
    }
    let actor = req.actor?;
    // Docked-now (`Registry::docked_station`): `Character::station_id` is
    // "my station" and stays set in space (`docs/lanes/docked.md`).
    Some(crate::item::Slot::new(
        actor,
        reg.docked_station(actor)?,
        crate::model::FLAG_HANGAR,
    ))
}

/// The item ids out of `{stationID: [(itemID, locationID), ...]}`.
///
/// Three levels, and each one is the client's:
///
/// - the outer dict is built at `invItemFunctions.py:579`-`:581`, keyed by
///   `invCache.GetStationIDOfItem(item) or item.locationID`. **A marshal dict
///   entry arrives as `(value, key)`**, so the list is element `.0` -- the
///   same inversion `reprocess::item_ids` documents, and getting it backwards
///   here would have read station ids as item ids;
/// - each list element is the 2-tuple `(item.itemID, item.locationID)` built
///   at `:580`;
/// - the station key is **discarded**. It is derived from the item's own
///   location, so it carries nothing the item id does not, and trusting a
///   client-supplied station over the store's own `location_id` is how a
///   repackage would get authorised against the wrong hangar.
///
/// Anything that does not have this shape contributes nothing, which makes
/// the result empty and the call a no-op rather than a wrong mutation.
fn repackage_item_ids(arg: Option<&PyValue>) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    let Some(PyValue::Dict(entries)) = arg else {
        return out;
    };
    for (value, _station) in entries {
        let (PyValue::List(items) | PyValue::Tuple(items)) = value else {
            continue;
        };
        for pair in items {
            let (PyValue::Tuple(parts) | PyValue::List(parts)) = pair else {
                continue;
            };
            if let Some(PyValue::Int(id)) = parts.first()
                && !out.contains(id)
            {
                out.push(*id);
            }
        }
    }
    out
}

/// One `UserError` with a bare key and no parameters.
///
/// Both keys this arm uses take no `%(name)s` substitutions
/// (`findings/user-errors.tsv` columns `params` and `dict_keys` are `-` for
/// each), so an empty dict is the whole payload.
fn bare_user_error(key: &'static str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, vec![]))
}

/// A `dogmaIM` module call whose arguments are not the shape the client
/// sends. Still a reply: a module call that is never answered leaves the
/// module locked in `invCache` for the rest of the session
/// (`docs/re/modules.md` §7.5).
///
/// Kept only as a thin wrapper over [`Args::expected`] for the sibling files
/// `fit_apply.rs` and `fit_ships.rs`, which this change does not own; delete
/// it when they migrate.
fn module_bad_args(req: &Request<'_>, want: &str) -> ServiceError {
    Args::new(req).expected(want)
}

/// `repackagingSvc.RepackageItems` -- unassemble, atomically or not at all.
///
/// **The batch is all-or-nothing**, which is the one policy decision here
/// that is ours rather than CCP's. A real cluster repackages what it can and
/// comes back with `ConfirmRepackageMultipleItemsWithUpgrades` carrying
/// `dict['itemIDs']` for the rest (`invItemFunctions.py:588`-`:603`), and
/// building that round-trip needs an upgrade model we do not have. The
/// alternative to refusing the batch was repackaging some items and saying
/// nothing about the others -- the silent half-success this packet exists to
/// remove -- so the plan is refused whole and the player is told. Marked
/// **[A]** in the lane note.
///
/// "Already packaged" is not a failure. It is excluded from both the plan and
/// the refusal set, so repackaging a mixed selection twice is idempotent and
/// the second call is a no-op instead of an error.
///
/// Contents are **moved out, not destroyed**, in the same plan as the
/// repackage -- which is what `GetContentForRepackaging` (`:93`) does: it
/// returns `(destroyed, moved)` and only two things land in `destroyed`, an
/// item at `flagHiddenModifers` and a rig on a hull that cannot unfit rigs
/// (`:112`-`:117`). Neither exists in this server, so everything moves.
fn repackage_items(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let ids = repackage_item_ids(Args::new(req).get(0));
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let Some(c) = reg.character(actor) else {
        return Ok(PyValue::None);
    };
    let (owner, active_ship) = (c.character_id, c.ship_id);
    // Docked-now, not `c.station_id` -- that is "my station", never cleared
    // on undock, so a repackage from space used to go through
    // (`docs/lanes/docked.md`).
    let Some(station) = reg.docked_station(owner) else {
        // `CanRepackageInLocation` wants the item's container to be a station,
        // a corp hangar array, a structure or an office
        // (`REPACKABLE_LOCATION_TYPES`, `repackaging/__init__.py:42`). Of
        // those we serve one, and only while docked.
        //
        // **Reachable for the first time** now that this reads docked-now.
        // Not `CanNotUnassembleInThisItemLoc`: its body is "...as it is
        // installed in a ship" (message 259995), which is the wrong story in
        // space. `CanOnlyDoInStations` takes no arguments and reads "The
        // action you are trying to perform can only be accomplished in a
        // station" (257751); the client raises it itself for station-only
        // item actions (`invItemFunctions.py:320`) **[V]**. That TQ answers
        // a repackage from space with it is `[A]`.
        return Err(bare_user_error("CanOnlyDoInStations"));
    };
    let hangar = crate::item::Slot::new(owner, station, crate::model::FLAG_HANGAR);

    let mut plan: Vec<crate::item::Edit> = Vec::new();
    let mut refused = 0usize;
    for id in ids {
        let Some(item) = reg.store().get(id) else {
            refused += 1;
            continue;
        };
        // Already packaged: nothing to do, and not a refusal.
        if !item.singleton {
            continue;
        }
        // A damaged ship, module or drone is repaired first
        // (`repackaging.CheckCanRepackage` `[V]`; GAP-REPAIR).
        if crate::repair::is_damaged(reg, id) {
            return Err(bare_user_error("CantRepackageDamagedItem"));
        }
        // **The ship the character is flying.** `CheckCanRepackage` has no
        // such test because on a real cluster the active ship is not in the
        // hangar listing to be selected; here it is -- `Registry::new` seeds
        // the capsule at `flagHangar` and `hangar_item_rows` returns it -- so
        // the test has to exist on our side. A capsule is also saved by
        // `groupCapsule` being in `REPACKABLE_CATEGORY_GROUP_EXCEPTIONS`, but
        // a Rifter the player is sitting in is not.
        //
        // The item's own address is the other half: not ours, not in this
        // station, or not loose in the hangar, and it is not repackageable
        // here. That covers "it is fitted" too, so the fitting-flag test in
        // `Store::apply` is a second line of defence rather than this one.
        if id == active_ship
            || item.owner_id != owner
            || item.location_id != station
            || item.flag_id != crate::model::FLAG_HANGAR
            || !crate::item::can_repackage_type(item.group_id, item.category_id)
            || crate::mutator_data::is_dynamic_type(reg.content(), item.type_id)
        {
            refused += 1;
            continue;
        }
        // Everything inside it comes out first, in the same plan.
        let contents: Vec<i64> = reg.store().contents_of(id).map(|i| i.item_id).collect();
        for content in contents {
            plan.push(crate::item::Edit::Move {
                item_id: content,
                to: hangar,
            });
        }
        plan.push(crate::item::Edit::Repackage { item_id: id });
    }

    if refused > 0 {
        // One item gets the specific reason; several get the plural key,
        // because `CanNotUnassembleInThisItemLoc` names *the* item's location
        // and would be a lie about a mixed batch. Both are `fsd+loc` in
        // `findings/user-errors.tsv` (message ids 279 and 3565), so both
        // render.
        return Err(bare_user_error(if refused == 1 {
            "CanNotUnassembleInThisItemLoc"
        } else {
            "MultipleItemsFailedToRepackage"
        }));
    }
    if plan.is_empty() {
        // Nothing was asked for, or everything named was already packaged.
        // Silence is right here: no item changed, so a push would be a
        // notification about nothing.
        return Ok(PyValue::None);
    }
    if reg.apply_items(&plan).is_err() {
        // Phase one refused and **nothing was applied**. The checks above
        // already cover every reason `Store::apply` can give for a
        // `Repackage`, so reaching this means the store disagreed with the
        // read we had just taken from it.
        return Err(bare_user_error("CanNotUnassembleInThisItemLoc"));
    }
    // Discarded (`invItemFunctions.py:583` is a `POP_TOP`): the client learns
    // the outcome from the two pushes `apply_items` queued.
    Ok(PyValue::None)
}

// ---------------------------------------------------------------------------
// Fitting and boarding (`docs/lanes/fitting.md`)
// ---------------------------------------------------------------------------

/// What is inside one of my items, as item rows, keeping the flags `keep`
/// accepts. Someone else's container lists as empty.
fn item_contents_rows(
    reg: &Registry,
    actor: Option<i64>,
    container: i64,
    keep: impl Fn(i64) -> bool,
) -> Vec<PyValue> {
    let Some(actor) = actor else {
        return Vec::new();
    };
    // **A wreck lists for anyone beside it**, whoever owns what is in it:
    // opening one is `invCache.GetInventoryFromId(wreckID).List()` on the
    // solar system's `invbroker` (`invControllers.py:135-163`,
    // `invCache.py:590-623`) [V], and the client has already decided the
    // player may look (`menusvc.OpenCargo:3584-3590`, range at
    // `invControllers.py:1502`) [V]. Who may *take* is
    // `combat::plan_loot`'s question.
    if wreck_open_to(reg, actor, container) {
        return reg
            .store()
            .contents_of(container)
            .map(|i| reg.item_row_value(i))
            .collect();
    }
    if reg.item(container).is_none_or(|c| c.owner_id != actor) {
        return Vec::new();
    }
    reg.store()
        .contents_of(container)
        // A loaded charge is a sublocation to the client, never an item row
        // (`crate::item::is_loaded_charge`). `UnloadAmmoToContainer` picks
        // the module out of `List(flag=slot)` as "the row that is not the
        // ammo" (`clientDogmaLocation.py:1356`) [V], so a charge listed here
        // under an int id could be picked instead.
        .filter(|i| i.owner_id == actor && keep(i.flag_id) && !crate::item::is_loaded_charge(i))
        .map(|i| reg.item_row_value(i))
        .collect()
}

/// A wreck in space in `actor`'s solar system. `[A]` no range on *looking*:
/// the client checks it before it opens the window, and a window left open
/// while its pilot drifts away re-lists on every invalidation -- a refusal
/// there would be an error dialog per take elsewhere. Taking is ranged.
/// A hacked can (HK3) is opened the same way (`invCtrl.ItemFloatingCargo`).
fn wreck_open_to(reg: &Registry, actor: i64, id: i64) -> bool {
    crate::hacking_loot::loot_source(reg, id).is_some_and(|wr| {
        reg.roster()
            .get(actor)
            .is_some_and(|p| p.solar_system_id == wr.solar_system_id)
    })
}

/// Module ids as `LoadAmmo`/`UnloadAmmo` send them: one int, a list or
/// tuple of them, or a Python `set`.
///
/// **"Load to all" sends a `set`** (`LoadChargeToAllModules` builds
/// `loadingModules = set()`, `cdl:1254`), and a set does not marshal as a
/// container: it arrives as tag 0x22 around `__builtin__.set`, which
/// `evecap` renders `<Call (<global __builtin__.set>, ([ids]))>` [V: the
/// owner's capture, `LoadAmmo(..., set([id, ...]), ...)`]. The ids are one
/// level down inside the header's argument tuple. Before this read it, every
/// set was an empty list: the call answered as if nothing was wrong and
/// loaded nothing.
fn id_list(v: &PyValue) -> Vec<i64> {
    fn ints(v: &PyValue, out: &mut Vec<i64>) {
        match v {
            PyValue::Int(n) => out.push(*n),
            PyValue::List(items) | PyValue::Tuple(items) => {
                for i in items {
                    ints(i, out);
                }
            }
            // The set wrapper: the callable is a `Global` and contributes
            // no ints; the list in its argument tuple is the set.
            PyValue::ObjectEx { header, .. } => ints(header, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    ints(v, &mut out);
    out
}

/// `dogmaIM.ItemGetInfo((shipID, flagID, typeID))`: the loaded charge's
/// row, if that key is loaded in my active ship; `None` otherwise, which
/// godma tolerates (`godma:1949`) [V].
fn charge_get_info(reg: &Registry, actor: Option<i64>, key: &[PyValue]) -> PyValue {
    let (
        Some(c),
        [
            PyValue::Int(ship),
            PyValue::Int(flag),
            PyValue::Int(type_id),
        ],
    ) = (actor.and_then(|id| reg.character(id)), key)
    else {
        return PyValue::None;
    };
    match crate::modules::loaded(reg.store(), *ship, *flag) {
        Some(i) if *ship == c.ship_id && i.type_id == *type_id && i.owner_id == c.character_id => {
            crate::modules::charge_info_in(reg.content(), i)
        }
        _ => PyValue::None,
    }
}

/// A ship's free turret hardpoints. Zero for anything that is not one of my
/// hulls -- the true answer for a capsule and for the hangar.
fn free_turrets(reg: &Registry, actor: Option<i64>, ship_id: i64) -> i64 {
    if reg.item(ship_id).is_none_or(|s| Some(s.owner_id) != actor) {
        return 0;
    }
    let Some(h) = crate::fitting::hull_of_ship(reg.content(), reg.store(), ship_id) else {
        return 0;
    };
    let used = reg
        .store()
        .contents_of(ship_id)
        .filter(|i| {
            crate::item::is_ship_fitting_flag(i.flag_id)
                && crate::modules::find_in(reg.content(), i.type_id).and_then(|t| t.hardpoint)
                    == Some(crate::fitting::Hardpoint::Turret)
        })
        .count() as i64;
    (h.turrets - used).max(0)
}

/// Our words, for a refusal that is about this server rather than EVE.
fn custom_notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

/// A plan the arm built from the store it just read was refused by the
/// store. That is our bug, not the player's, so it says so.
fn plan_refused(e: crate::item::EditError) -> ServiceError {
    crate::outln!("[inv] item plan refused: {e}");
    custom_notify("The server could not move that item.")
}

/// Fit one module and decide its online state. Returns the fitted id.
fn fit_one(
    reg: &mut Registry,
    actor: Option<i64>,
    ship_id: i64,
    module_id: i64,
    flag_id: i64,
) -> Result<i64, ServiceError> {
    let c = actor.and_then(|id| reg.character(id)).cloned();
    fit_one_as(reg, c, ship_id, module_id, flag_id)
}

/// [`fit_one`] with the pilot given. `fit_ships` passes a copy of the pilot
/// whose active ship is the hull being fitted, because every fit check reads
/// "the ship" from the pilot: a hull that is only parked in the hangar (the
/// multi-fit window's) is fitted as if it were the pilot's own, and the
/// pilot's real record is never touched.
fn fit_one_as(
    reg: &mut Registry,
    c: Option<crate::model::Character>,
    ship_id: i64,
    module_id: i64,
    flag_id: i64,
) -> Result<i64, ServiceError> {
    // **Already fitted means in this ship AND in a fitting slot**: the
    // client's rule (`fittingSlotController.py:93` `AddRealItem`:
    // `isFitted = locationID == shipID and flagID != flagCargo`) [V],
    // narrowed to fitting slots (the plan refuses every other hold). A
    // module in the ship's cargo hold is in this ship but not fitted: it is
    // skill-gated and onlined by `on_fitted` like one from the hangar (#205).
    let already_fitted = reg
        .item(module_id)
        .is_some_and(|m| m.location_id == ship_id && crate::item::is_ship_fitting_flag(m.flag_id));
    // No fitting in space: an observer's client drew this ship's guns once,
    // from the slim it was sent at arrival, and never redraws them
    // (`crate::fitting::refuse_in_space`, P2.5).
    if let Some(c) = &c {
        crate::fitting::refuse_in_space(reg, c).map_err(|r| r.to_error())?;
        // **Skill gate: fitting refuses an unskilled module** (SKILLGATE,
        // issue #36). OWNER DECISION PENDING on #36: retail may let the
        // module go on and stay offline (the client has no fit-time check);
        // the card says refuse, and a refusal leaves the module in the
        // hangar for a bot to retry. To flip: delete this one check -- the
        // online gate and `on_fitted` already keep it offline. A slot-to-slot
        // move of a fitted module is not gated; one from the cargo hold is
        // (`already_fitted` is same ship AND a fitting slot, the client's rule).
        if let Some(m) = reg.item(module_id)
            && !already_fitted
        {
            crate::skillgate::check(
                reg,
                c.character_id,
                m.type_id,
                "FittingHasSkillPrerequisites",
            )?;
        }
    }
    // X-RIG: a rig has its own rules (size, calibration, three slots).
    let is_rig = reg
        .item(module_id)
        .is_some_and(|i| crate::rigs::is_rig(reg.content(), i.type_id));
    let fit = if is_rig {
        crate::rigs::plan_fit(
            reg.content(),
            reg.store(),
            c.as_ref(),
            ship_id,
            module_id,
            flag_id,
        )
        .map_err(|r| r.to_error())?
    } else {
        crate::fitting::plan_fit_for(reg, c.as_ref(), ship_id, module_id, flag_id)
            .map_err(|r| r.to_error())?
    };
    let changes = reg.apply_items(&fit.plan).map_err(plan_refused)?;
    // From a stack the fitted module is a new id (`Edit::Assemble`).
    let fitted = changes
        .iter()
        .find_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .unwrap_or(module_id);
    // A slot-to-slot move keeps whatever online state the module had; one
    // from the cargo hold is not already fitted (same ship AND a fitting
    // slot: the client's rule, narrowed to fitting slots), so `on_fitted`
    // decides it. A subsystem is
    // passive: there is nothing to online (T3-D).
    let subsystem = reg
        .item(fitted)
        .is_some_and(|i| i.category_id == crate::subsystems::CATEGORY_SUBSYSTEM);
    if !already_fitted
        && !subsystem
        && !is_rig
        && let Some(c) = c
    {
        crate::modules::on_fitted(reg, ship_id, c.ship_type_id, fitted);
    }
    Ok(fitted)
}

/// Take one item out of the active ship into the hangar.
fn unfit_one(
    reg: &mut Registry,
    actor: Option<i64>,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<(), ServiceError> {
    // CAP-H: out of a fleet hangar or a maintenance bay.
    if let Some(r) = crate::ship_config::move_item(
        reg,
        actor,
        crate::ship_config::Dest::Hangar,
        item_id,
        quantity,
    ) {
        return r;
    }
    // Out of a drone bay is the drones module's (`docs/lanes/drones.md`).
    if crate::drones::in_a_bay(reg, item_id) {
        return crate::drones::to_hangar(reg, actor, item_id, quantity);
    }
    if crate::fighters::in_a_bay(reg, item_id) {
        return crate::fighters::to_hangar(reg, actor, item_id, quantity);
    }
    // X-RIG: unfitting a rig destroys it (a Strategic Cruiser's goes home).
    if let Some(r) = crate::rigs::unfit(reg, actor, item_id) {
        return r;
    }
    let c = actor.and_then(|id| reg.character(id)).cloned();
    let plan =
        crate::fitting::plan_unfit(reg, c.as_ref(), item_id, quantity).map_err(|r| r.to_error())?;
    reg.apply_items(&plan).map_err(plan_refused)?;
    reg.modules_mut().forget(item_id);
    Ok(())
}

/// Put an item into a ship's cargo hold (P5.12).
fn to_cargo(
    reg: &mut Registry,
    actor: Option<i64>,
    ship_id: i64,
    item_id: i64,
    quantity: Option<i64>,
    hold_flag: i64,
) -> Result<(), ServiceError> {
    // CAP-H: into a fleet hangar or a maintenance bay, or out of one.
    if let Some(r) = crate::ship_config::move_item(
        reg,
        actor,
        crate::ship_config::Dest::Hold {
            ship_id,
            flag: hold_flag,
        },
        item_id,
        quantity,
    ) {
        return r;
    }
    let was_fitted = reg
        .item(item_id)
        .is_some_and(|i| crate::item::is_ship_fitting_flag(i.flag_id));
    // **Loot** (P2.4): from a wreck, the range is measured from where the
    // looter is *now* -- so the roster is brought up to date first.
    let wreck = reg
        .item(item_id)
        .map(|i| i.location_id)
        .filter(|l| crate::hacking_loot::loot_source(reg, *l).is_some());
    if let (Some(wreck_id), Some(actor)) = (wreck, actor) {
        reg.advance_character(actor);
        // CW-1: a theft the pilot's safety forbids, in the client's own
        // words (`crate::crimewatch::theft_refusal`).
        if let Some(r) =
            crate::crimewatch::theft_refusal(reg, actor, wreck_id, crate::pyutil::filetime_now())
        {
            return Err(r.into_error());
        }
    }
    // The fleet loot log: what the move will take, read before it happens.
    let loot_taken = reg.item(item_id).map(|i| {
        (
            i.type_id,
            quantity.unwrap_or(i.quantity).clamp(1, i.quantity.max(1)),
        )
    });
    let c = actor.and_then(|id| reg.character(id)).cloned();
    let plan = crate::fitting::plan_to_hold(reg, c.as_ref(), ship_id, item_id, quantity, hold_flag)
        .map_err(|r| r.to_error())?;
    if plan.is_empty() {
        return Ok(());
    }
    reg.apply_items(&plan).map_err(plan_refused)?;
    if was_fitted {
        // A module dragged from its slot into the hold: unfitted.
        reg.modules_mut().forget(item_id);
    }
    if let (Some(wreck_id), Some(actor)) = (wreck, actor) {
        crate::combat::looted(reg, actor, wreck_id);
        crate::hacking_loot::looted(reg, actor, wreck_id);
        // `OnFleetLootEvent` to the fleet-mates in the looter's system.
        if let Some((type_id, qty)) = loot_taken {
            for p in loot_pushes(reg, actor, type_id, qty) {
                reg.push(p);
            }
        }
    }
    Ok(())
}

/// Where an `Add`/`MultiAdd` is going, from the bound inventory and `flag`.
#[derive(Clone, Copy)]
enum Target {
    /// Into the active ship at a fitting flag, or [`crate::fitting::FLAG_AUTOFIT`].
    Fit { ship_id: i64, flag_id: i64 },
    /// Out to the station hangar.
    Hangar,
    /// Into a ship's cargo hold, `flag=flagCargo` (P5.12), or its fuel bay,
    /// `flag=flagSpecializedFuelBay` (CAP-B), or any specialised hold
    /// (GAP-HOLDS): `hold_flag` says which.
    Cargo { ship_id: i64, hold_flag: i64 },
    /// Into a ship's drone bay, `flag=flagDroneBay` (`docs/lanes/drones.md`).
    DroneBay { ship_id: i64 },
    /// Into a ship's fighter bay, `flag=flagFighterBay` (FT-B).
    FighterBay { ship_id: i64 },
}

fn add_target(req: &Request<'_>) -> Result<Target, ServiceError> {
    let flag = Args::new(req).int_kw("flag");
    match container_of(req.bind_params) {
        Some(CONTAINER_HANGAR) => Ok(Target::Hangar),
        Some(ship_id)
            if flag.is_some_and(|f| {
                f == crate::fitting::FLAG_AUTOFIT || crate::item::is_ship_fitting_flag(f)
            }) =>
        {
            Ok(Target::Fit {
                ship_id,
                flag_id: flag.unwrap_or(crate::fitting::FLAG_AUTOFIT),
            })
        }
        // **The cargo hold** (P5.12): `ShipCargo.locationFlag` is
        // `const.flagCargo` (`invControllers.py:738`) and every drop on it is
        // `Add(itemID, sourceID, qty=n, flag=flagCargo)` on the ship's own
        // inventory (`:267`) [V]. Which ship, and from where, is
        // `fitting::plan_to_cargo`'s question.
        Some(ship_id)
            if flag.is_some_and(|f| {
                // GAP-HOLDS: every hold of the client's `inventoryFlagData`
                // (`crate::item::HOLDS`), the maintenance bay and the fleet
                // hangar included; the drone bay has its own arm below.
                (crate::item::hold_of(f).is_some() && f != crate::drones::FLAG_DRONE_BAY)
                    || crate::ship_config::is_bay(f)
            }) =>
        {
            Ok(Target::Cargo {
                ship_id,
                hold_flag: flag.unwrap_or(crate::fitting::FLAG_CARGO),
            })
        }
        // **The drone bay**: `invCtrl.ShipDroneBay.locationFlag` is
        // `flagDroneBay` (`invControllers.py:809`) [V]; `crate::drones::to_bay`.
        Some(ship_id) if flag == Some(crate::drones::FLAG_DRONE_BAY) => {
            Ok(Target::DroneBay { ship_id })
        }
        // **The fighter bay**: `invCtrl.ShipFighterBay.locationFlag` is
        // `flagFighterBay` (`invControllers.py:657`) [V];
        // `crate::fighters::to_bay`.
        Some(ship_id) if flag == Some(crate::fighters::FLAG_FIGHTER_BAY) => {
            Ok(Target::FighterBay { ship_id })
        }
        // Another bay (drones, a special hold), a container inside the
        // hangar: real, none modelled yet [A].
        _ => Err(custom_notify(
            "This server can only move items between the hangar, a ship's cargo hold and its fitting slots so far.",
        )),
    }
}

/// `invbroker.SplitStack(stationID, itemID, qty, ownerID)`: `qty` units off
/// a stack in the caller's own station hangar, as a new stack in the same
/// hangar.
///
/// **Refused** with `CannotSplitStackNotInHangar`, a key the client ships
/// (`findings/user-errors.tsv`, no parameters), for an item that is not the
/// caller's, not at `stationID`, not in the hangar or not a stack. Which
/// condition CCP's server raised it for is **[A]**: the key's name and the
/// client's own menu (it offers the entry for hangar stacks only) are all
/// there is to read, because the server half is not in the client. And a
/// `qty` that takes nothing or everything is a `CustomNotify`, **[A]** for
/// the same reason: the quantity popup the client opens is bounded by
/// `stacksize`, so only a modified client gets there. A corporation's
/// stack (`ownerID` is the corp) is refused too: corp hangars are the office
/// divisions of `corp_office`, not this flag.
fn split_stack(reg: &mut Registry, req: &Request<'_>) -> Result<(), ServiceError> {
    let (Some(station), Some(item_id), Some(qty)) = (
        Args::new(req).int(0),
        Args::new(req).int(1),
        Args::new(req).int(2),
    ) else {
        return Err(Args::new(req).expected("(stationID, itemID, qty, ownerID)"));
    };
    let not_in_hangar = || bare_user_error("CannotSplitStackNotInHangar");
    let actor = req.actor.ok_or_else(not_in_hangar)?;
    let hangar = crate::item::Slot::new(actor, station, crate::model::FLAG_HANGAR);
    let stack = reg
        .store()
        .get(item_id)
        .filter(|i| i.slot() == hangar && !i.singleton)
        .ok_or_else(not_in_hangar)?;
    if qty < 1 || qty >= stack.quantity {
        return Err(custom_notify(
            "That is not a quantity this stack can be split by.",
        ));
    }
    reg.apply_items(&[crate::item::Edit::Split {
        item_id,
        quantity: qty,
        to: hangar,
    }])
    .map(|_| ())
    .map_err(|_| not_in_hangar())
}

/// `invbroker.TrashItems(itemIDs, locationID)`: destroy the caller's items
/// in their hangar at `locationID`. Answers `[]`, the "no errors" the client
/// tests for.
///
/// What is skipped: another owner's items, items elsewhere than that hangar,
/// the ship the pilot sits in, and anything with something inside it (a
/// ship's modules and cargo would be orphaned). Skipped items are not
/// errors in the answer, because an error is an FSD dialog id for
/// `prompt_player_fsd_dialog` and none is in the client's tables for these
/// **[A]**; if nothing at all can be trashed the call raises a
/// `CustomNotify` instead, so a refusal is never silent.
fn trash_items(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(ids) = Args::new(req).ids(0) else {
        return Err(Args::new(req).expected("([itemIDs], locationID)"));
    };
    let Some(location) = Args::new(req).int(1) else {
        return Err(Args::new(req).expected("([itemIDs], locationID)"));
    };
    let Some(actor) = req.actor else {
        return Err(custom_notify("Nobody is logged in to trash anything."));
    };
    let hangar = crate::item::Slot::new(actor, location, crate::model::FLAG_HANGAR);
    let boarded = reg.character(actor).map(|c| c.ship_id);
    let mut edits: Vec<crate::item::Edit> = Vec::new();
    for id in ids {
        let trashable = reg.store().get(id).is_some_and(|i| i.slot() == hangar)
            && boarded != Some(id)
            && !reg.store().items().iter().any(|i| i.location_id == id)
            && !edits
                .iter()
                .any(|e| matches!(e, crate::item::Edit::Destroy { item_id } if *item_id == id));
        if trashable {
            edits.push(crate::item::Edit::Destroy { item_id: id });
        }
    }
    if edits.is_empty() {
        return Err(custom_notify(
            "Nothing there can be trashed: it is not yours, not in this hangar, boarded or not empty.",
        ));
    }
    reg.apply_items(&edits)
        .map_err(|_| custom_notify("These items could not be trashed."))?;
    Ok(PyValue::List(vec![]))
}

/// `invbroker` bound `Add(itemID, sourceID, qty=…, flag=…)`.
fn add(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(item_id) = Args::new(req).int(0) else {
        // A tuple id is a loaded charge, and the client never sends one
        // here: `invCacheContainer.Add` turns it into
        // `UnloadAmmoToContainer` -> `dogmaIM.UnloadAmmo` first
        // (`invCache.py` `Add`/`MultiAdd`/`MultiMerge`) [V].
        return Err(Args::new(req).expected("(itemID, sourceID)"));
    };
    match add_target(req)? {
        Target::Fit { ship_id, flag_id } => {
            fit_one(reg, req.actor, ship_id, item_id, flag_id)?;
        }
        Target::Hangar => unfit_one(reg, req.actor, item_id, Args::new(req).int_kw("qty"))?,
        Target::Cargo { ship_id, hold_flag } => {
            to_cargo(
                reg,
                req.actor,
                ship_id,
                item_id,
                Args::new(req).int_kw("qty"),
                hold_flag,
            )?;
        }
        Target::DroneBay { ship_id } => {
            crate::drones::to_bay(
                reg,
                req.actor,
                ship_id,
                item_id,
                Args::new(req).int_kw("qty"),
            )?;
        }
        Target::FighterBay { ship_id } => {
            crate::fighters::to_bay(
                reg,
                req.actor,
                ship_id,
                item_id,
                Args::new(req).int_kw("qty"),
            )?;
        }
    }
    Ok(PyValue::None)
}

/// `invbroker` bound `MultiAdd([itemIDs], sourceID, flag=…)`.
///
/// **One plan per item** [A]: a plan is atomic, so one refusal in a shared
/// plan would undo the modules that fit. Those that fit are fitted; if none
/// did, the first refusal is the answer, so a selection that cannot be
/// fitted at all is not met with silence.
///
/// **Into a cargo hold, any refusal is raised** [A], after the items that
/// fit have moved: a multi-drag into a hold that fills part-way would
/// otherwise leave the rest in the hangar with no word why. The client
/// sends no `qty` here -- whole stacks only (`invControllers.py:688`) [V]
/// -- and the reply is discarded (`invCache.py:1135`, `POP_TOP`) [V], so
/// the raise is the only news a refusal can be. Items are tried in the
/// order sent, against the hold as the earlier ones left it.
fn multi_add(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(ids) = Args::new(req).ids(0) else {
        return Err(Args::new(req).expected("([itemIDs], sourceID)"));
    };
    let target = add_target(req)?;
    let mut first_refusal = None;
    let mut skill_refusal = None;
    let mut done = 0usize;
    let mark = reg.pending_len();
    for id in ids {
        let r = match target {
            Target::Fit { ship_id, flag_id } => {
                fit_one(reg, req.actor, ship_id, id, flag_id).map(|_| ())
            }
            Target::Hangar => unfit_one(reg, req.actor, id, None),
            Target::Cargo { ship_id, hold_flag } => {
                to_cargo(reg, req.actor, ship_id, id, None, hold_flag)
            }
            Target::DroneBay { ship_id } => {
                crate::drones::to_bay(reg, req.actor, ship_id, id, None)
            }
            Target::FighterBay { ship_id } => {
                crate::fighters::to_bay(reg, req.actor, ship_id, id, None)
            }
        };
        match r {
            Ok(()) => done += 1,
            Err(e) => {
                // A skill refusal is the pilot's only news (the client discards
                // the reply), so it is raised even when others fitted.
                if skill_refusal.is_none() && crate::skillgate::is_refusal(&e) {
                    skill_refusal = Some(e.clone());
                }
                first_refusal.get_or_insert(e);
            }
        }
    }
    // One cache drop for the batch, after every item row (FITFIX-2).
    reg.coalesce_invalidations(mark);
    if let (Some(e), Target::Fit { .. }) = (skill_refusal, target) {
        return Err(e);
    }
    match (first_refusal, target) {
        (Some(e), Target::Cargo { .. } | Target::DroneBay { .. } | Target::FighterBay { .. }) => {
            Err(e)
        }
        (Some(e), _) if done == 0 => Err(e),
        _ => Ok(PyValue::None),
    }
}

/// The `shipState` 4-tuple for the ship being boarded: the same value
/// `Registry::all_info` sends, with its loaded charges in slot 1
/// ([`crate::modules::ship_state`], P5.12).
fn ship_state(reg: &Registry, ship_id: i64) -> PyValue {
    crate::weapon_bank::ship_state(reg, ship_id)
}

/// `{'shipid': (old, new)}` -- one key, like the death path's
/// `ShipIdToCapsule` (`docs/lanes/fitting.md` §1.2). No `locationid`: the
/// client derives it.
fn shipid_change(old: i64, new: i64) -> Vec<(PyValue, PyValue)> {
    crate::pyutil::pypairs(vec![(
        "shipid",
        PyValue::Tuple(vec![PyValue::Int(old), PyValue::Int(new)]),
    )])
}

/// Make `ship_id` the character's active ship and queue the session change.
/// Call only after the item plan applied.
fn switch_ship(reg: &mut Registry, character_id: i64, ship_id: i64, type_id: i64) {
    let Some(c) = reg.character_mut(character_id) else {
        return;
    };
    let old = c.ship_id;
    c.ship_id = ship_id;
    c.ship_type_id = type_id;
    reg.queue_session_change(character_id, shipid_change(old, ship_id));
}

/// `ship.Board(shipID, oldShipID)` (spec §1.2).
///
/// The checks and their keys are [A] -- all four exist in the dialog table,
/// none has a client raise site. Skills are re-checked here (SKILLGATE): the
/// client refuses first (`base:271`) [V], so only a bot or a forged call
/// reaches it, and the key `ShipHasSkillPrerequisites` is [V] but its use on
/// `ship.Board` is [A]. `oldShipID` is ignored; ours is the truth.
fn board(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(ship_id) = Args::new(req).int(0) else {
        return Err(Args::new(req).expected("(shipID, oldShipID)"));
    };
    let Some(c) = req.actor.and_then(|id| reg.character(id)).cloned() else {
        return Err(custom_notify("No character is selected."));
    };
    // At a structure's controls (U6C) the ship is the structure's: let go first.
    if crate::structure_dock::control::controlling(reg, c.character_id).is_some() {
        return Err(custom_notify(
            "Release control of the structure before you board a ship.",
        ));
    }
    // Docked-now (`docs/lanes/docked.md`): with `c.station_id` this never
    // fired, and a pilot in space could board a ship parked at home.
    let Some(station) = reg.docked_station(c.character_id) else {
        // #77 BE-2: in space, an abandoned ship within 6550 m.
        return board_eject::board_in_space(reg, &c, ship_id);
    };
    let Some(ship) = reg.item(ship_id).cloned() else {
        return Err(bare_user_error("ShipNotYours"));
    };
    if ship.owner_id != c.character_id {
        return Err(bare_user_error("ShipNotYours"));
    }
    if ship.location_id != station || ship.flag_id != crate::model::FLAG_HANGAR {
        return Err(bare_user_error("ShipNotHere"));
    }
    if ship.category_id != crate::model::CATEGORY_SHIP || !ship.singleton {
        return Err(bare_user_error("ShipNotBoardable"));
    }
    if ship_id == c.ship_id {
        // Already aboard: the state, and no change [A].
        return Ok(ship_state(reg, ship_id));
    }
    crate::skillgate::check(
        reg,
        c.character_id,
        ship.type_id,
        "ShipHasSkillPrerequisites",
    )?;
    // **The capsule climbs in**, from the hangar or from the old ship, to
    // `(ship, flagCapsule)` [I] -- which is what drops it out of the
    // hangar listing, and what `fitting::active_ship_of` reads back after
    // a restart.
    let pod = crate::fitting::capsule_of(reg.store(), &c).map(|p| p.item_id);
    if let Some(pod) = pod.filter(|p| *p != ship_id) {
        reg.apply_items(&[crate::item::Edit::Move {
            item_id: pod,
            to: crate::item::Slot::new(c.character_id, ship_id, crate::fitting::FLAG_CAPSULE),
        }])
        .map_err(plan_refused)?;
    }
    switch_ship(reg, c.character_id, ship_id, ship.type_id);
    // The client's embarkation leaves offline what the pilot cannot use.
    crate::modules::offline_unskilled(reg, c.character_id, ship_id);
    Ok(ship_state(reg, ship_id))
}

/// Eject a character in space from their ship, which stays there pilotless
/// (`ship.Eject`'s whole effect). `false` when the eject is refused (docked,
/// already in the capsule, in warp, a timer).
pub(crate) fn eject_character(reg: &mut Registry, character_id: i64) -> bool {
    board_eject::eject_actor(reg, character_id).is_ok()
}

/// Put a docked character into their capsule, the ship staying in the hangar:
/// what [`leave_ship`] does, for a server-side caller (the AIR tutorial's
/// `Move Player To Location` with `replace_ship` and no ship type). `false`
/// when the character is not docked, has no capsule or is already in it.
pub(crate) fn into_capsule_docked(reg: &mut Registry, character_id: i64) -> bool {
    let Some(c) = reg.character(character_id).cloned() else {
        return false;
    };
    let Some(station) = reg.docked_station(character_id) else {
        return false;
    };
    let Some(pod) = crate::fitting::capsule_of(reg.store(), &c).cloned() else {
        return false;
    };
    if pod.item_id == c.ship_id {
        return false;
    }
    if reg
        .apply_items(&[crate::item::Edit::Move {
            item_id: pod.item_id,
            to: crate::item::Slot::new(character_id, station, crate::model::FLAG_HANGAR),
        }])
        .is_err()
    {
        return false;
    }
    switch_ship(reg, character_id, pod.item_id, pod.type_id);
    true
}

/// `ship.LeaveShip(shipID)` (spec §1.4): back into the capsule, which goes
/// back to the hangar. Returns the capsule's id.
fn leave_ship(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(ship_id) = Args::new(req).int(0) else {
        return Err(Args::new(req).expected("(shipID)"));
    };
    let Some(c) = req.actor.and_then(|id| reg.character(id)).cloned() else {
        return Err(custom_notify("No character is selected."));
    };
    // Docked-now (`docs/lanes/docked.md`): with `c.station_id` this never
    // fired, and leaving a ship in space put the capsule in the home hangar.
    let Some(station) = reg.docked_station(c.character_id) else {
        return Err(custom_notify("Leaving a ship in space is not supported."));
    };
    if ship_id != c.ship_id {
        return Err(custom_notify("That is not your active ship."));
    }
    let Some(pod) = crate::fitting::capsule_of(reg.store(), &c).cloned() else {
        return Err(custom_notify("Your capsule could not be found."));
    };
    if pod.item_id == c.ship_id {
        return Err(custom_notify("You are already in your capsule."));
    }
    reg.apply_items(&[crate::item::Edit::Move {
        item_id: pod.item_id,
        to: crate::item::Slot::new(c.character_id, station, crate::model::FLAG_HANGAR),
    }])
    .map_err(plan_refused)?;
    switch_ship(reg, c.character_id, pod.item_id, pod.type_id);
    Ok(PyValue::Int(pod.item_id))
}

/// `ship.AssembleShip(itemIDs, name=…, subSystems=…)` (spec §1.5).
///
/// `invItemFunctions.py:201` passes a list (an int for a modular ship) [V];
/// the answer is a list of the resulting ids, one per input in order, or an
/// int for an int [A]. Refusal keys [A]. `name` is stored through
/// `item_names::set_label` for each hull this call assembles (the new id
/// when it came off a stack); a blank, `None` or missing name stores nothing,
/// and a ship that was already assembled is not renamed [A: retail stores the
/// assemble name as the item's name -- the client sends it
/// (`invItemFunctions.AssembleShip:201`) and this build has no other way to
/// name a fresh ship; falsifier: a retail capture where GetMultiLocationsEx
/// of a freshly named-assembled ship answers another name, or one renaming
/// an assembled ship passed to AssembleShip].
fn assemble_ship(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (ids, as_list) = match (Args::new(req).int(0), Args::new(req).ids(0)) {
        (Some(id), _) => (vec![id], false),
        (None, Some(ids)) => (ids, true),
        _ => return Err(Args::new(req).expected("(itemIDs)")),
    };
    let name = Args::new(req).text_kw("name");
    let Some(c) = req.actor.and_then(|id| reg.character(id)).cloned() else {
        return Err(custom_notify("No character is selected."));
    };
    // Docked-now (`docs/lanes/docked.md`): with `c.station_id` this never
    // fired, and a hull in the home hangar could be assembled from space.
    let Some(station) = reg.docked_station(c.character_id) else {
        return Err(bare_user_error("AssembleShipInStation"));
    };
    let mut plan = Vec::new();
    let mut modular = None;
    for &id in &ids {
        let Some(item) = reg.item(id) else {
            return Err(custom_notify("This server does not know that ship."));
        };
        if item.category_id != crate::model::CATEGORY_SHIP {
            return Err(ServiceError::Raise(eve_proto::user_error(
                "AssembleShipsOnly",
                crate::pyutil::pypairs(vec![(
                    "group",
                    // `UE_GROUPID` 7 (`eveexceptions/const.pyc:9`) [V].
                    PyValue::Tuple(vec![PyValue::Int(7), PyValue::Int(item.group_id)]),
                )]),
            )));
        }
        if item.owner_id != c.character_id {
            return Err(ServiceError::Raise(eve_proto::user_error(
                "AssembleOwnShipsOnly",
                crate::pyutil::pypairs(vec![("charID", PyValue::Int(item.owner_id))]),
            )));
        }
        if item.location_id != station || item.flag_id != crate::model::FLAG_HANGAR {
            return Err(bare_user_error("AssembleShipInStation"));
        }
        // T3-D: a strategic cruiser is assembled with its four subsystems.
        if crate::subsystems::is_modular_group(item.group_id) {
            modular = Some(item.clone());
        }
        if !item.singleton {
            plan.push(crate::item::Edit::Assemble {
                item_id: id,
                to: item.slot(),
            });
        }
    }
    if let Some(ship) = modular {
        if ids.len() != 1 {
            return Err(custom_notify(
                "A strategic cruiser is assembled on its own, with its subsystems.",
            ));
        }
        let subs = id_list(Args::new(req).kw("subSystems").unwrap_or(&PyValue::None));
        let id = assemble_modular(reg, &c, station, &ship, &subs)?;
        // The assemble already happened; a refusal changes nothing.
        if !ship.singleton
            && let Some(n) = &name
        {
            let _ = crate::item_names::set_label(reg, req.actor, id, n);
        }
        return Ok(if as_list {
            PyValue::List(vec![PyValue::Int(id)])
        } else {
            PyValue::Int(id)
        });
    }
    let changes = if plan.is_empty() {
        Vec::new()
    } else {
        reg.apply_items(&plan).map_err(plan_refused)?
    };
    // Each `Assemble` reports either `Assembled` (a stack of one, same id)
    // or `Created` (one off a stack, a new id), in plan order.
    let mut made = changes.iter().filter_map(|ch| match ch {
        crate::item::Change::Assembled { item_id, .. }
        | crate::item::Change::Created { item_id, .. } => Some(*item_id),
        _ => None,
    });
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let was_packaged = plan
            .iter()
            .any(|e| matches!(e, crate::item::Edit::Assemble { item_id, .. } if *item_id == id));
        let new_id = if was_packaged {
            made.next().unwrap_or(id)
        } else {
            id
        };
        if was_packaged && let Some(n) = &name {
            let _ = crate::item_names::set_label(reg, req.actor, new_id, n);
        }
        out.push(new_id);
    }
    Ok(if as_list {
        PyValue::List(out.into_iter().map(PyValue::Int).collect())
    } else {
        out.first().map_or(PyValue::None, |id| PyValue::Int(*id))
    })
}

/// `ship.AssembleShip(shipID, subSystems=[...])` for a strategic cruiser
/// (T3-D): the hull, packaged or a bare assembled one, and exactly four
/// subsystems, one per flag 125..128, all or nothing. Returns the ship's id
/// (new when the hull came off a stack of packaged ones).
///
/// Everything is checked before anything moves; the edits then go in one
/// plan when the hull keeps its id, and in two (hull, then subsystems) when
/// a stack of hulls gives it a new one -- the second cannot fail after the
/// checks.
fn assemble_modular(
    reg: &mut Registry,
    c: &crate::model::Character,
    station: i64,
    ship: &crate::item::Item,
    subs: &[i64],
) -> Result<i64, ServiceError> {
    let pairs =
        crate::subsystems::check_assemble(reg.content(), reg.store(), c, station, ship, subs)
            .map_err(|r| r.to_error())?;
    if reg
        .store()
        .contents_of(ship.item_id)
        .any(|i| crate::subsystems::is_subsystem_flag(i.flag_id))
    {
        return Err(crate::fitting::Refusal::Occupied.to_error());
    }
    // SKILLGATE: every subsystem is gated before anything is applied.
    for sub in subs {
        if let Some(t) = reg.item(*sub).map(|i| i.type_id) {
            crate::skillgate::check(reg, c.character_id, t, "FittingHasSkillPrerequisites")?;
        }
    }
    let owner = c.character_id;
    let mut plan = Vec::new();
    if !ship.singleton {
        plan.push(crate::item::Edit::Assemble {
            item_id: ship.item_id,
            to: ship.slot(),
        });
    }
    if ship.singleton || ship.quantity == 1 {
        plan.extend(crate::subsystems::assemble_edits(
            owner,
            ship.item_id,
            &pairs,
        ));
        reg.apply_items(&plan).map_err(plan_refused)?;
        return Ok(ship.item_id);
    }
    let changes = reg.apply_items(&plan).map_err(plan_refused)?;
    let ship_id = changes
        .iter()
        .find_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .unwrap_or(ship.item_id);
    reg.apply_items(&crate::subsystems::assemble_edits(owner, ship_id, &pairs))
        .map_err(plan_refused)?;
    Ok(ship_id)
}

/// `dogmaIM.ItemGetInfo(itemID)`: the hull's row for the active ship, a
/// fitted module's row for one of its modules, `None` for anything else.
fn item_get_info(reg: &Registry, actor: Option<i64>, item_id: i64) -> PyValue {
    let Some(c) = actor.and_then(|id| reg.character(id)) else {
        return PyValue::None;
    };
    if item_id == c.ship_id {
        return reg.capsule_dogma_row(c);
    }
    match reg.item(item_id) {
        Some(i)
            if i.location_id == c.ship_id
                && i.owner_id == c.character_id
                && i.singleton
                && crate::item::is_ship_fitting_flag(i.flag_id) =>
        {
            crate::modules::module_info(reg, c, i)
        }
        // #41: a service module fitted to the structure he has the controls of.
        _ => crate::structure_services::item_info(reg, c.character_id, item_id)
            .unwrap_or(PyValue::None),
    }
}

/// `Add`/`MultiAdd` on the inventory of the structure `req.actor` has the
/// controls of (#41): a service module into slot 164..=171 (or the autofit,
/// flag 0, for a service module) is a fit; an item fitted to the structure
/// added to the hangar (flag 4) is an unfit. `None` for every other call, which
/// carries on as the hangar's. The client discards the reply.
fn structure_module_call(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    let actor = req.actor?;
    let sid = crate::structure_dock::control::resolve_container(
        reg,
        actor,
        container_of(req.bind_params)?,
    );
    if crate::structure_dock::control::controlling(reg, actor) != Some(sid) {
        return None;
    }
    let flag = Args::new(req).int_kw("flag")?;
    let ids = if req.method == "MultiAdd" {
        Args::new(req).ids(0)?
    } else {
        vec![Args::new(req).int(0)?]
    };
    let now = crate::pyutil::filetime_now();
    let fits = |reg: &Registry, id: i64| {
        crate::structure_services::is_slot(flag)
            || (flag == crate::fitting::FLAG_AUTOFIT
                && crate::structure_services::is_service_module(reg, id))
    };
    let unfits = |reg: &Registry, id: i64| {
        flag == crate::model::FLAG_HANGAR
            && reg.item(id).is_some_and(|i| {
                i.location_id == sid && crate::structure_services::is_slot(i.flag_id)
            })
    };
    if !ids.iter().all(|id| fits(reg, *id)) && !ids.iter().all(|id| unfits(reg, *id)) {
        return None;
    }
    let mut first_refusal = None;
    let mut done = 0usize;
    let first_id = ids.first().copied().unwrap_or_default();
    for id in ids {
        let r = if fits(reg, id) {
            crate::structure_services::fit(reg, actor, sid, id, flag).map(|_| ())
        } else {
            crate::structure_services::unfit(reg, actor, sid, id, now)
        };
        match r {
            Ok(()) => done += 1,
            Err(e) => {
                first_refusal.get_or_insert(e);
            }
        }
    }
    // A fit that was refused is the pilot's only news (the reply is discarded
    // and the extra modules of a multi-drag into one slot would otherwise
    // vanish silently): raised after the ones that fit were fitted.
    Some(match first_refusal {
        Some(e) if done == 0 || fits(reg, first_id) => Err(e),
        _ => Ok(PyValue::None),
    })
}

mod board_eject;
mod fit_apply;
mod fit_ships;
mod pack;

/// The `OnFleetLootEvent` pushes for `actor` taking `qty` of `type_id` from a
/// wreck: the fleet-mates in the looter's session system
/// (`Registry::session_system`, not the home station's), and the looter.
fn loot_pushes(reg: &Registry, actor: i64, type_id: i64, qty: i64) -> Vec<crate::fleet::Push> {
    let here = reg.session_system(actor);
    reg.fleets().loot_event(actor, type_id, qty, |m| {
        here.is_some() && reg.session_system(m) == here
    })
}

/// `GetLayerDamageValuesByItems(set(droneIDs))`: `{droneID: KeyVal}` with
/// `.shieldInfo` (health, tau, time), `.armorDamage` and `.hullDamage`, the
/// fields `FetchInBayDroneDamageToServer:41-45` reads [V]. The client sends a
/// Python `set`, which arrives as `__builtin__.set(list)`; a list or tuple is
/// taken too.
fn layer_damage_whole(arg: Option<&PyValue>) -> PyValue {
    let items = match arg {
        Some(PyValue::ObjectEx { header, .. }) => match header.as_ref() {
            PyValue::Tuple(h) => match h.get(1) {
                Some(PyValue::Tuple(a)) => a.first(),
                _ => None,
            },
            _ => None,
        },
        other => other,
    };
    let ids = match items {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v.as_slice(),
        _ => &[],
    };
    PyValue::Dict(
        ids.iter()
            .filter(|i| matches!(i, PyValue::Int(_)))
            .map(|id| {
                let whole = keyval(vec![
                    // Full shield, so the tau (`[1]`) never runs; `[2]` is the
                    // client's clock, unread (see `DamageState::to_pyvalue`).
                    (
                        "shieldInfo",
                        PyValue::Tuple(vec![
                            PyValue::Real(1.0),
                            PyValue::Real(0.0),
                            PyValue::Real(0.0),
                        ]),
                    ),
                    ("armorDamage", PyValue::Real(0.0)),
                    ("hullDamage", PyValue::Real(0.0)),
                ]);
                // A marshal dict pair is (value, key) (`PyValue::Dict`).
                (whole, id.clone())
            })
            .collect(),
    )
}

#[cfg(test)]
mod subsystem_socket;

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;

    /// The one character in a single-character fixture, as an actor.
    ///
    /// Spelled out from the fixture rather than reached for inside the
    /// registry: finding the actor by taking "the first character" is the
    /// thing Round 0.3 removed, and a test that did it would hide its
    /// return.
    fn actor(reg: &Registry) -> Option<i64> {
        Some(reg.config().characters[0].character_id)
    }

    /// The same value as a constant, for the `Request` literals built where
    /// no `&Registry` is in hand. Tied to the fixture by the test below so
    /// the two cannot drift apart silently.
    const ACTOR: Option<i64> = Some(90_000_001);

    /// The fleet loot log goes to the mates in the *looter's session system*:
    /// a mate homed in A but in space in B gets it, one homed in B but in
    /// space in A does not (`docs/lanes/stale-system.md`).
    #[test]
    fn fleet_loot_reaches_the_mates_in_the_looters_system() {
        use crate::space_state::{Motion, Presence};
        const LOOTER: i64 = 90_000_811;
        const NEAR: i64 = 90_000_812;
        const AWAY: i64 = 90_000_813;
        const SYS_A: i64 = 7_001;
        const SYS_B: i64 = 7_002;
        let pilot = |id, home| crate::model::Character {
            character_id: id,
            name: format!("P{id}"),
            ship_id: id + 100_000_000,
            station_id: Some(60_707_000),
            solar_system_id: Some(home),
            ..crate::model::Character::default()
        };
        let config = SandboxConfig {
            characters: vec![pilot(LOOTER, SYS_A), pilot(NEAR, SYS_A), pilot(AWAY, SYS_B)],
            ..SandboxConfig::default()
        };

        let mut reg = Registry::new(config);
        for (who, at) in [(LOOTER, SYS_B), (NEAR, SYS_B), (AWAY, SYS_A)] {
            reg.roster_mut().arrive(Presence::arriving(
                who,
                who + 1_000,
                at,
                Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        }
        let f = reg.fleets_mut();
        let id = f.create(LOOTER).expect("create");
        f.init(LOOTER, id, Some(670), None, Some(SYS_B))
            .expect("init");
        for who in [NEAR, AWAY] {
            f.invite(LOOTER, id, who, None, None, None).expect("invite");
            f.accept_invite(who, id, Some(670), None, None)
                .expect("accept");
        }
        let pushes = loot_pushes(&reg, LOOTER, 555, 3);
        assert_eq!(pushes.len(), 1);
        let mut to = pushes[0].to.clone();
        to.sort_unstable();
        assert_eq!(to, vec![LOOTER, NEAR]);
    }

    #[test]
    fn the_fixture_character_is_the_actor_these_tests_pass() {
        assert_eq!(ACTOR, actor(&Registry::new(SandboxConfig::default())));
    }

    /// Tests for this subsystem belong here, not in the parent. That is the
    /// other half of why the split exists: two lanes adding a method each
    /// would otherwise contend for one `mod tests`.
    /// Every service this module advertises must reach an arm in *this*
    /// module's `dispatch`.
    ///
    /// It used to check the name against a hand-maintained list in the
    /// parent, which is now derived from these very lists -- so that check
    /// had become a tautology. This probes the dispatcher instead, with a
    /// method that cannot exist: `NoSuchMethod` means we got to the service,
    /// `NoSuchService` means the name is advertised and falls straight
    /// through, which is a refused call, which is a hung client.
    #[test]
    fn every_service_here_is_routed_to_this_module() {
        let mut reg = Registry::new(SandboxConfig::default());
        for s in SERVICES {
            let req = Request {
                service: s,
                method: "__no_such_method_exists__",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            };
            match dispatch(&mut reg, &req) {
                Err(ServiceError::NoSuchMethod { .. }) => {}
                other => panic!(
                    "{s} is advertised by this module but its dispatch answers {other:?} \
                     to a method that cannot exist"
                ),
            }
        }
    }

    /// **`GetCapacity` is `{capacity, used}` and the usage is real.**
    ///
    /// The shape is the client's own zero sentinel,
    /// `Row(['capacity', 'used'], [0, 0.0])`, and the consumers read both by
    /// attribute to compute `capacity - used`. Asserted as a pair, because
    /// `used` without `capacity` is worse than nothing: it makes free space
    /// negative.
    #[test]
    fn get_capacity_reports_real_usage_for_the_hangar() {
        let hangar = PyValue::Tuple(vec![PyValue::Int(CONTAINER_HANGAR)]);
        let mut reg = Registry::new(SandboxConfig::default());
        let read = |reg: &mut Registry| {
            let got = dispatch(
                reg,
                &Request {
                    service: "invbroker::inventory",
                    method: "GetCapacity",
                    args: &[],
                    kwargs: &[],
                    bind_params: Some(&hangar),
                    actor: ACTOR,
                },
            )
            .expect("GetCapacity answers");
            // A `KeyVal` is an old-style **instance**, so the attributes live
            // in its state dict rather than at the top level.
            let PyValue::Instance { class, state } = &got else {
                panic!("GetCapacity must be a KeyVal instance, got {got:?}")
            };
            assert_eq!(
                String::from_utf8_lossy(class),
                "utillib.KeyVal",
                "the consumers read .capacity and .used by attribute"
            );
            let cap = crate::pyutil::dict_get(state, b"capacity").cloned();
            let used = crate::pyutil::dict_get(state, b"used").cloned();
            match (cap, used) {
                (Some(PyValue::Real(c)), Some(PyValue::Real(u))) => (c, u),
                other => panic!("both must be reals: {other:?}"),
            }
        };

        // The capsule alone: 1,000 m3, read from `types.fsdbinary`.
        let (capacity, used) = read(&mut reg);
        assert!(capacity > used, "free space must not be negative");
        assert_eq!(used, 1000.0, "the pod's own volume");

        // A Rifter is 27,289 m3.
        reg.slash(actor(&reg), "/createitem Rifter 1")
            .expect("createitem");
        let (_, used_after) = read(&mut reg);
        assert_eq!(
            used_after,
            1000.0 + 27289.0,
            "usage is volume x quantity, summed over the container"
        );

        // A stack counts per unit: 500 EMP S at 0.0025 m3.
        reg.slash(actor(&reg), "/createitem EMP S 500")
            .expect("createitem");
        let (_, used_ammo) = read(&mut reg);
        assert!(
            (used_ammo - (used_after + 500.0 * 0.0025)).abs() < 1e-9,
            "a stack of 500 adds 500 x its unit volume, got {used_ammo}"
        );
    }

    /// A pod has no cargo hold, and the type table says so rather than us.
    #[test]
    fn the_capsules_own_inventory_has_no_capacity() {
        let mut reg = Registry::new(SandboxConfig::default());
        let pod = PyValue::Tuple(vec![PyValue::Int(
            SandboxConfig::default().characters[0].ship_id,
        )]);
        let got = dispatch(
            &mut reg,
            &Request {
                service: "invbroker::inventory",
                method: "GetCapacity",
                args: &[],
                kwargs: &[],
                bind_params: Some(&pod),
                actor: ACTOR,
            },
        )
        .expect("answers");
        let PyValue::Instance { state, .. } = &got else {
            panic!("a KeyVal instance, got {got:?}")
        };
        assert_eq!(
            crate::pyutil::dict_get(state, b"capacity"),
            Some(&PyValue::Real(0.0)),
            "typeID 670's capacity in types.fsdbinary is 0.0"
        );
    }

    /// **The hangar's other names agree with `List`.**
    ///
    /// `ListByFlags` and `ListIncludingContainers` on the inventory object
    /// are the same container read differently, so answering `[]` flatly
    /// would have made the hangar hold a pod by one call and nothing by
    /// another -- and the client merges these answers into one cache keyed by
    /// `itemID`. The sub-location reads stay empty whatever the container,
    /// because a pod has no cargo hold and a station hangar has no fighter
    /// tubes.
    #[test]
    fn the_hangars_other_list_names_agree_with_list_itself() {
        let hangar = PyValue::Tuple(vec![PyValue::Int(CONTAINER_HANGAR)]);
        let mut reg = Registry::new(SandboxConfig::default());
        let read = |reg: &mut Registry, method: &str, bind: Option<&PyValue>| {
            dispatch(
                reg,
                &Request {
                    service: "invbroker::inventory",
                    method,
                    args: &[],
                    kwargs: &[],
                    bind_params: bind,
                    actor: ACTOR,
                },
            )
            .unwrap_or_else(|e| panic!("{method}: {e}"))
        };

        let listed = read(&mut reg, "List", Some(&hangar));
        for method in ["ListByFlags", "ListIncludingContainers"] {
            assert_eq!(
                read(&mut reg, method, Some(&hangar)),
                listed,
                "{method} on the hangar must agree with List; the client \
                 merges them into one cache keyed by itemID"
            );
        }

        // A container that is not the hangar is empty, and so are the
        // sub-location reads everywhere.
        let pod = PyValue::Tuple(vec![PyValue::Int(100_000_001)]);
        assert_eq!(
            read(&mut reg, "ListByFlags", Some(&pod)),
            PyValue::List(vec![])
        );
        for method in [
            "ListCargo",
            "ListHardwareModules",
            "ListFighterBay",
            "ValidateItemListCanBeOpened",
        ] {
            assert_eq!(
                read(&mut reg, method, Some(&hangar)),
                PyValue::List(vec![]),
                "{method} is iterated; None raises, and empty is the truth"
            );
        }
        // The single-item form is truthy, or `invWindow` closes the open
        // inventory.
        assert_eq!(
            read(&mut reg, "ValidateItemCanBeOpened", Some(&hangar)),
            PyValue::Bool(true)
        );
    }

    /// One call, with a `SandboxConfig::default()` registry behind it.
    fn answer(service: &str, method: &str, args: &[PyValue]) -> PyValue {
        let mut reg = Registry::new(SandboxConfig::default());
        let req = Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        };
        dispatch(&mut reg, &req).unwrap_or_else(|e| panic!("{service}.{method} answered {e:?}"))
    }

    /// `invbroker.GetContainerContents` must be **iterable**, with exactly
    /// the three arguments the owner's capture carried.
    ///
    /// `captures/1792442372-3777106-0002.evecap`:
    ///
    /// ```text
    ///     [366] 103438.491ms OK  invbroker*  GetContainerContents
    ///           [N=1:4](100000001, 60012526, None)  -> None
    /// ```
    ///
    /// `fittingSvc.GetFittingDictFromItems` iterates the answer twice and
    /// `containerContentWindow.GetContentsList` runs `for c in contents`
    /// over it, so `None` was a `TypeError` at both. The pod holds nothing,
    /// so the list is empty -- but it is a list.
    #[test]
    fn get_container_contents_is_iterable_not_none() {
        let args = [
            PyValue::Int(100_000_001),
            PyValue::Int(60_012_526),
            PyValue::None,
        ];
        match answer("invbroker", "GetContainerContents", &args) {
            PyValue::List(rows) => assert!(
                rows.is_empty(),
                "the capsule contains nothing, so asking for its contents must not \
                 hand back the capsule itself: {rows:?}"
            ),
            other => panic!(
                "GetContainerContents must be iterable -- fittingSvc.py:173 does \
                 `{{(x.typeID, x.flagID): x.itemID for x in visibleFittedItems}}` over \
                 it -- but the arm answers {other:?}"
            ),
        }
    }

    /// **Argument 1 is the filter, and it is the only one that is.**
    ///
    /// The container is named by its own item id and the rows inside it carry
    /// `locationID == containerID`. Asked for the station the character is
    /// docked in, we must hand back the capsule that is parked there; asked
    /// for the capsule, nothing. If this ever answers the same thing to both
    /// questions, the filter has been dropped and a ship-contents panel will
    /// be showing the whole hangar.
    #[test]
    fn get_container_contents_is_filtered_by_its_first_argument() {
        let station = answer(
            "invbroker",
            "GetContainerContents",
            &[PyValue::Int(60_012_526), PyValue::None, PyValue::None],
        );
        let PyValue::List(in_station) = station else {
            panic!("expected a list");
        };
        assert_eq!(
            in_station.len(),
            1,
            "the station holds the character's one item, the capsule: {in_station:?}"
        );
        assert!(
            matches!(in_station[0], PyValue::DbRow(_)),
            "the shape is `Registry::owned_item_rows`' -- a `blue.DBRow` over \
             `item_columns()`, because `invCache.ListIncludingContainers` merges this \
             answer with `charMgr`'s into one cache keyed by `realItem.itemID`. \
             Got {:?}",
            in_station[0]
        );

        // The two-argument form, from containerContentWindow.py:78, which
        // passes `(itemID, stationID)` and no solar system.
        let ship = answer(
            "invbroker",
            "GetContainerContents",
            &[PyValue::Int(100_000_001), PyValue::Int(60_012_526)],
        );
        assert_eq!(
            ship,
            PyValue::List(vec![]),
            "nothing is fitted to or stowed in the capsule, and two arguments must \
             work as well as three"
        );
    }

    /// `ship.GetShipFittingInfo` is consumed by **`UNPACK_SEQUENCE 4`**.
    ///
    /// `eve/client/script/ui/crate/controller.py` lines 76 and 252:
    ///
    /// `crate/controller.py:76,252` (`[V]`) unpack the answer of
    /// `ship.GetShipFittingInfo` into name, icon, ship type id and insurance
    ///
    /// with no guard in front of either, so the unpack happens before
    /// anything is tested and the arity is the entire contract. Four
    /// elements, never three, never five, and never `None`.
    #[test]
    fn get_ship_fitting_info_unpacks_into_four() {
        match answer("ship", "GetShipFittingInfo", &[PyValue::Int(1)]) {
            PyValue::Tuple(parts) => {
                assert_eq!(
                    parts.len(),
                    4,
                    "controller.py:76 and :252 do `UNPACK_SEQUENCE 4` on this answer; \
                     any other length is a ValueError and `None` is a TypeError"
                );
                // `name` reaches `GetByLabel(self.name)` in
                // `lootItem.AbstractSpecialLootItem.get_name`, so it is a
                // label path -- a string, not None.
                assert!(
                    matches!(parts[0], PyValue::Unicode(_) | PyValue::Bytes(_)),
                    "element 0 is a localization label path, handed to GetByLabel: {:?}",
                    parts[0]
                );
                // `insurance` gates `if insurance:` -- the only thing that
                // reads elements 2 and 3 -- so it must be falsy while we
                // have no insurance quotes to report.
                assert_eq!(
                    parts[3],
                    PyValue::None,
                    "element 3 gates `if insurance:` in LoadSpecialLoot; a truthy value \
                     would send the client into \
                     `GetByLabel('UI/Insurance/QuoteWindow/%s' % insurance)`"
                );
            }
            other => panic!(
                "GetShipFittingInfo meets UNPACK_SEQUENCE 4 and must be a 4-sequence, \
                 not {other:?}"
            ),
        }
    }

    /// `{stationID: [(itemID, locationID), ...]}`, which is **not** a list.
    ///
    /// The shape is built at `invItemFunctions.py:579`-`:581` and the two
    /// traps are both in it: a marshal dict entry arrives `(value, key)`, so
    /// reading `.1` would give station ids; and each list element is a
    /// 2-tuple, so reading the element itself would give nothing at all.
    #[test]
    fn repackage_reads_item_ids_out_of_the_station_keyed_dict() {
        let batch = PyValue::Dict(vec![(
            PyValue::List(vec![
                PyValue::Tuple(vec![PyValue::Int(100_000_007), PyValue::Int(60_012_526)]),
                PyValue::Tuple(vec![PyValue::Int(100_000_009), PyValue::Int(60_012_526)]),
            ]),
            PyValue::Int(60_012_526),
        )]);
        assert_eq!(
            repackage_item_ids(Some(&batch)),
            vec![100_000_007, 100_000_009],
            "the item id is element 0 of each tuple inside the dict's *value*"
        );
        // Two stations, and the keys are never mistaken for item ids.
        let two = PyValue::Dict(vec![
            (
                PyValue::List(vec![PyValue::Tuple(vec![
                    PyValue::Int(100_000_007),
                    PyValue::Int(60_012_526),
                ])]),
                PyValue::Int(60_012_526),
            ),
            (
                PyValue::List(vec![PyValue::Tuple(vec![
                    PyValue::Int(100_000_008),
                    PyValue::Int(60_000_001),
                ])]),
                PyValue::Int(60_000_001),
            ),
        ]);
        assert_eq!(
            repackage_item_ids(Some(&two)),
            vec![100_000_007, 100_000_008]
        );
        // Anything else contributes nothing, so the call is a no-op rather
        // than a wrong mutation.
        assert_eq!(repackage_item_ids(None), Vec::<i64>::new());
        assert_eq!(
            repackage_item_ids(Some(&PyValue::List(vec![PyValue::Int(1)]))),
            Vec::<i64>::new()
        );
    }

    /// **"Stack All" stacks, and tells the client to re-read.**
    ///
    /// The server is only ever asked once `invCacheContainer.StackAll` has
    /// itself seen two items sharing a `typeID` in this container
    /// (`invCache.py:1155`-`:1168`), so this is the state the call arrives
    /// in. Three stacks of EMP S become one, and
    /// `OnInvalidateCacheForLocations` is the push that matters: the window's
    /// own `uthread.new(self.Refresh)` (`container.py:1083`) re-lists, and
    /// `invCacheContainer.listed` has no TTL, so without the invalidation the
    /// re-list is served from the cache and the player sees nothing change.
    #[test]
    fn stack_all_merges_the_hangar_and_invalidates_the_cache() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = actor(&reg);
        for _ in 0..3 {
            reg.slash(me, "/createitem EMP S 100").expect("createitem");
        }
        reg.take_pending();
        // EMP S is typeID 185 in `item::SPAWNABLE`, named rather than
        // inferred from the quantity: a filter on `quantity == 100` would
        // have gone on passing if `/createitem` ever stopped making three
        // stacks.
        let before: Vec<i64> = reg
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == 185)
            .map(|i| i.item_id)
            .collect();
        assert_eq!(before.len(), 3, "three separate stacks to start with");

        let hangar = PyValue::Tuple(vec![PyValue::Int(CONTAINER_HANGAR)]);
        let got = dispatch(
            &mut reg,
            &Request {
                service: "invbroker::inventory",
                method: "StackAll",
                args: &[],
                kwargs: &[],
                bind_params: Some(&hangar),
                actor: ACTOR,
            },
        )
        .expect("StackAll must not raise: the client has already counted two stacks");
        assert_eq!(
            got,
            PyValue::None,
            "every caller discards this -- `container.py:1082` is a bare POP_TOP"
        );

        let stacks: Vec<(i64, i64)> = reg
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == 185)
            .map(|i| (i.item_id, i.quantity))
            .collect();
        assert_eq!(
            stacks,
            vec![(before[0], 300)],
            "one stack of 300 survives, and it is the oldest id"
        );

        let pending = reg.take_pending();
        let events: Vec<&str> = pending.iter().map(|(_, p)| p.to_push().0).collect();
        assert!(
            events.contains(&"OnInvalidateCacheForLocations"),
            "without this the window's own Refresh is served from a cache with \
             no TTL and the stack never appears to change: {events:?}"
        );
        assert_eq!(
            events.iter().filter(|e| **e == "OnItemChange").count(),
            3,
            "one row for the surviving stack and one junkyard row for each of \
             the two folded away: {events:?}"
        );
    }

    /// Nothing to stack is not an error, and emits nothing.
    ///
    /// A push about nothing is worse than silence: `OnItemChange` makes the
    /// client rebuild a cache entry and `OnInvalidateCacheForLocations` makes
    /// it re-ask us, both for an answer that has not changed.
    #[test]
    fn stack_all_with_nothing_to_stack_changes_nothing_and_says_nothing() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = actor(&reg);
        reg.slash(me, "/createitem Rifter 2").expect("two hulls");
        reg.take_pending();
        let before = reg.store().items().len();

        let hangar = PyValue::Tuple(vec![PyValue::Int(CONTAINER_HANGAR)]);
        let got = dispatch(
            &mut reg,
            &Request {
                service: "invbroker::inventory",
                method: "StackAll",
                args: &[],
                kwargs: &[],
                bind_params: Some(&hangar),
                actor: ACTOR,
            },
        )
        .expect("answers");
        assert_eq!(got, PyValue::None);
        assert_eq!(
            reg.store().items().len(),
            before,
            "two assembled hulls do not stack -- one itemID cannot be two ships"
        );
        assert!(
            reg.take_pending().is_empty(),
            "no change, so no push: the client would re-ask for an answer that \
             has not moved"
        );
    }

    /// **Repackaging a ship clears `singleton` in the row the client reads.**
    ///
    /// Index 10, and it is what `godma.UpdateItem`'s skip3 and
    /// `undockingSvc.ShipPicker` both test. The push is the whole
    /// deliverable: `invItemFunctions.RepackageItems` discards our answer and
    /// only plays a sound (`:583`, `:606`) -- there is no `Refresh` the way
    /// "Stack All" has one -- so a repackage we do not announce stays
    /// invisible for the session.
    #[test]
    fn repackaging_a_ship_clears_singleton_and_pushes_the_new_row() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = actor(&reg);
        reg.slash(me, "/createitem Rifter 1").expect("rifter");
        let ship = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == 587)
            .map(|i| i.item_id)
            .expect("spawned");
        reg.take_pending();

        let got = dispatch(
            &mut reg,
            &Request {
                service: "repackagingSvc",
                method: "RepackageItems",
                args: &[one_station_batch(60_012_526, &[ship])],
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            },
        )
        .expect("a hull in the hangar is repackageable");
        assert_eq!(got, PyValue::None, "the caller does `POP_TOP`");
        assert_eq!(
            reg.store().get(ship).map(|i| i.singleton),
            Some(false),
            "singleton is index 10 of our row, and clearing it is the whole of \
             a repackage"
        );

        let pending = reg.take_pending();
        let events: Vec<&str> = pending.iter().map(|(_, p)| p.to_push().0).collect();
        assert_eq!(
            events,
            vec!["OnItemChange", "OnInvalidateCacheForLocations"],
            "the row first, then the invalidation -- a push that arrives before \
             the response it depends on is a change that never happened \
             (`dispatch::Reply::pushes`): {events:?}"
        );
        // And the row really carries the cleared flag, by column name,
        // because a positional read here would survive a reordering of
        // `item_columns()` and start asserting the wrong column.
        let (_, args) = pending[0].1.to_push();
        let PyValue::DbRow(row) = &args[0] else {
            panic!(
                "`invCache.OnItemChange` does `blue.DBRow(item)`, so this must be \
                    a real row, not {:?}",
                args[0]
            )
        };
        let column = row
            .descriptor()
            .index_of(b"singleton")
            .expect("item_columns() declares singleton");
        assert_eq!(row.values().get(column), Some(&DbValue::Int(0)));
    }

    /// The capsule refuses, with a key the client can actually render.
    ///
    /// Two separate findings in one test. The *rule* is CCP's:
    /// `REPACKABLE_CATEGORIES_GROUP_EXCEPTIONS = {groupCapsule}`
    /// (`repackaging/__init__.py:22`), and the pod is the one item every
    /// character here already owns. The *key* is ours to pick, and
    /// `RepackageItemNotSingleton` -- the one `CheckCanRepackage` would raise
    /// first -- is **`pyc`-only in `findings/user-errors.tsv`**, with no FSD
    /// message id, so it would hand `prompt_player_fsd_dialog` a label that
    /// does not exist. `CanNotUnassembleInThisItemLoc` has message 279.
    #[test]
    fn repackaging_the_capsule_is_refused_with_a_renderable_key() {
        let mut reg = Registry::new(SandboxConfig::default());
        let pod = SandboxConfig::default().characters[0].ship_id;
        let err = dispatch(
            &mut reg,
            &Request {
                service: "repackagingSvc",
                method: "RepackageItems",
                args: &[one_station_batch(60_012_526, &[pod])],
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            },
        )
        .expect_err("groupCapsule is carved out of categoryShip");
        assert_eq!(
            raised_key(&err).as_deref(),
            Some("CanNotUnassembleInThisItemLoc"),
            "`invItemFunctions.py:605` bare-re-raises anything that is not one \
             of its two Confirm keys, so the key is shown as-is"
        );
        assert!(
            reg.store().get(pod).expect("still there").singleton,
            "a refusal applies nothing"
        );
        assert!(reg.take_pending().is_empty(), "and pushes nothing");
    }

    /// A batch with one bad item is refused whole, and the plural key says so.
    ///
    /// **[A]**: a real cluster repackages what it can and comes back with
    /// `ConfirmRepackageMultipleItemsWithUpgrades` carrying the failures in
    /// `dict['itemIDs']`. We refuse the batch instead, because the
    /// alternative is repackaging some items and saying nothing about the
    /// rest -- which is the silent half-success this packet removed.
    #[test]
    fn a_batch_with_an_unrepackageable_item_is_refused_whole() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = actor(&reg);
        reg.slash(me, "/createitem Rifter 1").expect("rifter");
        let pod = SandboxConfig::default().characters[0].ship_id;
        let ship = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == 587)
            .map(|i| i.item_id)
            .expect("spawned");
        reg.take_pending();

        let err = dispatch(
            &mut reg,
            &Request {
                service: "repackagingSvc",
                method: "RepackageItems",
                args: &[one_station_batch(60_012_526, &[ship, pod, 999])],
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            },
        )
        .expect_err("the pod and a nonexistent id both fail");
        assert_eq!(
            raised_key(&err).as_deref(),
            Some("MultipleItemsFailedToRepackage"),
            "two failures, so the plural key (message 3565): naming *the* \
             item's location would be a lie about a mixed batch"
        );
        assert!(
            reg.store().get(ship).expect("untouched").singleton,
            "the good item in the batch is not repackaged either"
        );
        assert!(reg.take_pending().is_empty());
    }

    /// Repackaging what is already packaged is a no-op, not an error.
    ///
    /// Idempotence matters because "Repackage" is offered over a multi-select
    /// and `CheckRepackageItems` does not filter on `singleton` -- it only
    /// *warns* about assembled containers (`invItemFunctions.py:555`). So a
    /// mixed selection reaches us with packaged items in it routinely.
    #[test]
    fn repackaging_an_already_packaged_item_changes_nothing_quietly() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = actor(&reg);
        reg.slash(me, "/createitem EMP S 100").expect("ammo");
        let ammo = reg
            .store()
            .items()
            .iter()
            .find(|i| !i.singleton)
            .map(|i| i.item_id)
            .expect("a stack");
        reg.take_pending();
        let got = dispatch(
            &mut reg,
            &Request {
                service: "repackagingSvc",
                method: "RepackageItems",
                args: &[one_station_batch(60_012_526, &[ammo])],
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            },
        )
        .expect("already packaged is not a failure");
        assert_eq!(got, PyValue::None);
        assert!(
            reg.take_pending().is_empty(),
            "nothing changed, so nothing is announced"
        );
    }

    /// Repackaging a ship empties its hold into the hangar, in one plan.
    ///
    /// `GetContentForRepackaging` (`repackaging/__init__.py:93`) returns
    /// `(destroyed, moved)` and only two things are ever destroyed -- an item
    /// at `flagHiddenModifers` and a rig on a hull that cannot unfit rigs
    /// (`:112`-`:117`). Neither exists here, so everything moves.
    #[test]
    fn repackaging_moves_the_hold_out_before_packing_the_hull() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = actor(&reg);
        reg.slash(me, "/createitem Rifter 1").expect("rifter");
        reg.slash(me, "/createitem EMP S 100").expect("ammo");
        let ship = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == 587)
            .map(|i| i.item_id)
            .expect("spawned");
        let ammo = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id != 587 && !i.singleton)
            .map(|i| i.item_id)
            .expect("a stack");
        // Into the Rifter's cargo hold: `flagCargo` is 5.
        reg.apply_items(&[crate::item::Edit::Move {
            item_id: ammo,
            to: crate::item::Slot::new(90_000_001, ship, 5),
        }])
        .expect("stowed");
        reg.take_pending();

        dispatch(
            &mut reg,
            &Request {
                service: "repackagingSvc",
                method: "RepackageItems",
                args: &[one_station_batch(60_012_526, &[ship])],
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            },
        )
        .expect("a loaded hull repackages; the cargo comes out");
        assert_eq!(
            reg.store().contents_of(ship).count(),
            0,
            "the hold is empty"
        );
        let moved = reg.store().get(ammo).expect("the ammo survived");
        assert_eq!(
            (moved.location_id, moved.flag_id),
            (60_012_526, crate::model::FLAG_HANGAR),
            "and it landed loose in the station hangar"
        );
        assert!(!reg.store().get(ship).expect("still here").singleton);
    }

    /// `{stationID: [(itemID, locationID)]}` for one station, as the client
    /// builds it. A test helper, because four tests need the same three
    /// levels of nesting and a hand-built one got the `(value, key)`
    /// inversion wrong the first time.
    fn one_station_batch(station: i64, items: &[i64]) -> PyValue {
        PyValue::Dict(vec![(
            PyValue::List(
                items
                    .iter()
                    .map(|&id| PyValue::Tuple(vec![PyValue::Int(id), PyValue::Int(station)]))
                    .collect(),
            ),
            PyValue::Int(station),
        )])
    }

    /// The `UserError` key out of a raised `ServiceError`.
    ///
    /// Read the way the client reads it -- `exception.msg`, which is
    /// `args[0]` of the `eveexceptions.UserError` the `0x22` header carries --
    /// because that is the string `invItemFunctions.py:585`/`:588` compare
    /// against and `:605` bare-re-raises on. Asserts the class, so a raise
    /// that is not a `UserError` fails here rather than in a client.
    fn raised_key(err: &ServiceError) -> Option<String> {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = err else {
            panic!("not a raised exception: {err:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("a 0x22 header is a tuple")
        };
        assert_eq!(
            parts[0],
            PyValue::Global(b"eveexceptions.UserError".to_vec()),
            "`except UserError` at invItemFunctions.py:584 catches nothing else"
        );
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError's header args are a tuple")
        };
        crate::pyutil::as_bytes(&args[0]).map(|b| String::from_utf8_lossy(b).into_owned())
    }

    // ---- fitting and boarding (`docs/lanes/fitting.md` §9, tests 6-19) ----

    const STATION: i64 = 60_012_526;

    fn kw(name: &str, v: PyValue) -> (PyValue, PyValue) {
        (v, PyValue::Bytes(name.as_bytes().to_vec()))
    }

    fn ship_inv(ship: i64) -> PyValue {
        PyValue::Tuple(vec![PyValue::Int(ship), PyValue::Int(STATION)])
    }

    fn hangar_inv() -> PyValue {
        PyValue::Tuple(vec![PyValue::Int(CONTAINER_HANGAR), PyValue::None])
    }

    /// `eveMoniker.GetStationShipAccess()`: `('ship', (stationID,
    /// groupStation))` [V].
    fn ship_access() -> PyValue {
        PyValue::Tuple(vec![PyValue::Int(STATION), PyValue::Int(15)])
    }

    fn call(
        reg: &mut Registry,
        service: &str,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
        bind: &PyValue,
    ) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service,
                method,
                args,
                kwargs,
                bind_params: Some(bind),
                actor: ACTOR,
            },
        )
    }

    /// Grant `quantity` of a type and return the new item id.
    fn grant(reg: &mut Registry, type_id: i64, quantity: i64) -> i64 {
        let me = actor(reg);
        let before = reg.store().next_id();
        reg.slash(me, &format!("/createitem {type_id} {quantity}"))
            .expect("granted");
        reg.store()
            .items()
            .iter()
            .find(|i| i.type_id == type_id && i.item_id >= before)
            .map(|i| i.item_id)
            .expect("granted item")
    }

    fn pod(reg: &Registry) -> i64 {
        reg.config().characters[0].ship_id
    }

    fn me(reg: &Registry) -> &crate::model::Character {
        reg.character(ACTOR.expect("actor")).expect("character")
    }

    fn board_it(reg: &mut Registry, ship: i64) -> Result<PyValue, ServiceError> {
        let old = me(reg).ship_id;
        call(
            reg,
            "ship",
            "Board",
            &[PyValue::Int(ship), PyValue::Int(old)],
            &[],
            &ship_access(),
        )
    }

    /// A fresh world, docked in a boarded Merlin. Returns `(reg, merlin)`.
    fn in_a_merlin() -> (Registry, i64) {
        let mut reg = Registry::new(SandboxConfig::default());
        let merlin = grant(&mut reg, 603, 1);
        board_it(&mut reg, merlin).expect("boarded");
        let _ = reg.take_session_changes();
        let _ = reg.take_pending();
        (reg, merlin)
    }

    fn fit(reg: &mut Registry, ship: i64, item: i64, flag: i64) -> Result<PyValue, ServiceError> {
        call(
            reg,
            "invbroker::inventory",
            "Add",
            &[PyValue::Int(item), PyValue::Int(STATION)],
            &[kw("qty", PyValue::Int(1)), kw("flag", PyValue::Int(flag))],
            &ship_inv(ship),
        )
    }

    fn fitted_at(reg: &Registry, ship: i64, flag: i64) -> Vec<crate::item::Item> {
        reg.store()
            .contents_of(ship)
            .filter(|i| i.flag_id == flag)
            .cloned()
            .collect()
    }

    fn column(row: &PyValue, name: &str) -> DbValue {
        let PyValue::DbRow(row) = row else {
            panic!("an item row, got {row:?}");
        };
        let ix = row.descriptor().index_of(name.as_bytes()).expect("column");
        row.values()[ix].clone()
    }

    fn attr<'a>(v: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("a KeyVal, got {v:?}");
        };
        crate::pyutil::dict_get(state, name.as_bytes()).expect("attribute")
    }

    /// `flagHiSlot0` [V].
    const HI0: i64 = 27;

    /// Test 6: the drag onto a slot. The reply is `None` (discarded at
    /// `cdl:817`) [V]; one assembled unit is in the slot and the hangar
    /// stack lost one.
    #[test]
    fn a_module_dragged_onto_a_slot_is_fitted() {
        let (mut reg, merlin) = in_a_merlin();
        let stack = grant(&mut reg, 484, 3);
        assert_eq!(fit(&mut reg, merlin, stack, HI0), Ok(PyValue::None));
        let there = fitted_at(&reg, merlin, HI0);
        assert_eq!(there.len(), 1);
        assert!(there[0].singleton);
        assert_eq!(there[0].type_id, 484);
        assert_eq!(reg.store().get(stack).map(|i| i.quantity), Some(2));
    }

    /// Tests 7, 8 and 10: the wrong rack, a slot the hull does not have, and
    /// a capsule are refused with a `UserError`, and a refusal changes
    /// nothing and pushes nothing.
    #[test]
    fn a_fit_the_hull_cannot_take_is_refused_and_changes_nothing() {
        let (mut reg, merlin) = in_a_merlin();
        let extender = grant(&mut reg, 377, 1); // medPower [V-data]
        let gun = grant(&mut reg, 484, 1);
        let _ = reg.take_pending();
        let hi = crate::fitting::hull_in(reg.content(), 603)
            .expect("Merlin")
            .hi;
        let before = reg.store().items().to_vec();
        for (item, flag) in [(extender, HI0), (gun, HI0 + hi)] {
            let e = fit(&mut reg, merlin, item, flag).expect_err("refused");
            assert!(matches!(e, ServiceError::Raise(_)), "{e:?}");
        }
        assert_eq!(reg.store().items(), before.as_slice());
        assert!(reg.take_pending().is_empty(), "a refusal pushes nothing");

        // Test 10: into a capsule.
        let mut reg = Registry::new(SandboxConfig::default());
        let gun = grant(&mut reg, 484, 1);
        let pod = pod(&reg);
        let before = reg.store().items().to_vec();
        assert!(matches!(
            fit(&mut reg, pod, gun, HI0),
            Err(ServiceError::Raise(_))
        ));
        assert_eq!(reg.store().items(), before.as_slice());
    }

    /// Test 9: hardpoints. A Thrasher has more high slots than turret
    /// hardpoints [V-data]; once every hardpoint holds a turret, a turret
    /// into a free high slot is refused.
    #[test]
    fn a_turret_past_the_hardpoints_is_refused() {
        let mut reg = Registry::new(SandboxConfig::default());
        let h = *crate::fitting::hull_in(reg.content(), 16242).expect("Thrasher");
        assert!(
            h.hi > h.turrets,
            "the fixture needs a free slot past the hardpoints"
        );
        let thrasher = grant(&mut reg, 16242, 1);
        board_it(&mut reg, thrasher).expect("boarded");
        let guns = grant(&mut reg, 484, h.turrets + 1);
        for slot in 0..h.turrets {
            fit(&mut reg, thrasher, guns, HI0 + slot).expect("a hardpoint is free");
        }
        let before = reg.store().items().to_vec();
        assert!(fit(&mut reg, thrasher, guns, HI0 + h.turrets).is_err());
        assert_eq!(reg.store().items(), before.as_slice());
    }

    /// Test 11: unfit is `Add(moduleID, shipID, flag=4)` on the hangar
    /// (`fittingSlotController.py:235`) [V]; the module lands in the hangar,
    /// still assembled.
    #[test]
    fn an_unfitted_module_goes_back_to_the_hangar() {
        let (mut reg, merlin) = in_a_merlin();
        let stack = grant(&mut reg, 484, 1);
        fit(&mut reg, merlin, stack, HI0).expect("fitted");
        let module = fitted_at(&reg, merlin, HI0)[0].item_id;
        let got = call(
            &mut reg,
            "invbroker::inventory",
            "Add",
            &[PyValue::Int(module), PyValue::Int(merlin)],
            &[kw("flag", PyValue::Int(4))],
            &hangar_inv(),
        );
        assert_eq!(got, Ok(PyValue::None));
        let m = reg.store().get(module).expect("still exists");
        assert_eq!((m.location_id, m.flag_id), (STATION, 4));
        assert!(m.singleton);
    }

    /// Test 12: `List(flag=…)` on the ship is the client's own keyword
    /// (`invCache.py:1101`) [V]; `flag=None` is everything inside. Every row
    /// has the item descriptor's eleven columns.
    #[test]
    fn a_ship_lists_its_contents_by_flag() {
        let (mut reg, merlin) = in_a_merlin();
        let stack = grant(&mut reg, 484, 1);
        fit(&mut reg, merlin, stack, HI0).expect("fitted");
        let module = fitted_at(&reg, merlin, HI0)[0].item_id;
        let list = |reg: &mut Registry, flag: PyValue| match call(
            reg,
            "invbroker::inventory",
            "List",
            &[],
            &[kw("flag", flag)],
            &ship_inv(merlin),
        ) {
            Ok(PyValue::List(rows)) => rows,
            other => panic!("a list, got {other:?}"),
        };
        let slot = list(&mut reg, PyValue::Int(HI0));
        assert_eq!(slot.len(), 1);
        assert_eq!(column(&slot[0], "itemID"), DbValue::Int(module));
        let all = list(&mut reg, PyValue::None);
        assert_eq!(all.len(), reg.store().contents_of(merlin).count());
        for row in &all {
            let PyValue::DbRow(r) = row else {
                panic!("a row")
            };
            assert_eq!(r.values().len(), 11);
        }
    }

    /// Test 13: a ship I own but am not in answers `GetSelfInvItem` with its
    /// own row, not the station's.
    #[test]
    fn a_ship_that_is_not_active_is_its_own_self_item() {
        let mut reg = Registry::new(SandboxConfig::default());
        let merlin = grant(&mut reg, 603, 1);
        let row = call(
            &mut reg,
            "invbroker::inventory",
            "GetSelfInvItem",
            &[],
            &[],
            &ship_inv(merlin),
        )
        .expect("answered");
        assert_eq!(column(&row, "itemID"), DbValue::Int(merlin));
        assert_eq!(column(&row, "typeID"), DbValue::Int(603));
    }

    /// Test 14: `ItemGetInfo` for a fitted module carries a singleton
    /// `invItem` (`godma:1939`) and the duration attribute; anything else is
    /// `None`, which `godma:1949` tolerates [V].
    #[test]
    fn item_get_info_answers_a_fitted_module_and_none_otherwise() {
        let (mut reg, merlin) = in_a_merlin();
        let stack = grant(&mut reg, 484, 1);
        fit(&mut reg, merlin, stack, HI0).expect("fitted");
        let module = fitted_at(&reg, merlin, HI0)[0].item_id;
        let none = PyValue::None;
        let info = call(
            &mut reg,
            "dogmaIM",
            "ItemGetInfo",
            &[PyValue::Int(module)],
            &[],
            &none,
        )
        .expect("answered");
        assert_eq!(column(attr(&info, "invItem"), "singleton"), DbValue::Int(1));
        let speed = crate::modules::find_in(reg.content(), 484)
            .and_then(|t| t.attribute(51))
            .expect("a turret's duration is speed (51)");
        let PyValue::Dict(attributes) = attr(&info, "attributes") else {
            panic!("a dict")
        };
        assert!(attributes.contains(&(PyValue::Real(speed), PyValue::Int(51))));
        assert_eq!(
            call(
                &mut reg,
                "dogmaIM",
                "ItemGetInfo",
                &[PyValue::Int(1)],
                &[],
                &none
            ),
            Ok(PyValue::None)
        );
    }

    /// **P2.3 end to end through `dispatch`**: `LoadAmmo(shipID, moduleID,
    /// [ids], chargeLocationID)` (`cdl:1201`) [V] fills the gun from the
    /// hangar; the ship's `List(flag=slot)` still lists only the module (the
    /// charge is a sublocation -- `UnloadAmmoToContainer` takes "the row
    /// that is not the ammo" from that list, `cdl:1356`); `ItemGetInfo` on
    /// the tuple key answers the charge; `UnloadAmmo(shipID, [moduleID],
    /// stationID)` (`cdl:1330`) brings every round back. No round is made or
    /// lost on the way.
    #[test]
    fn charges_load_list_answer_and_unload_through_dispatch() {
        let (mut reg, merlin) = in_a_merlin();
        let gun = grant(&mut reg, 484, 1);
        fit(&mut reg, merlin, gun, HI0).expect("fitted");
        let module = fitted_at(&reg, merlin, HI0)[0].item_id;
        let ammo = grant(&mut reg, 185, 500);
        let none = PyValue::None;
        let total = |reg: &Registry| -> i64 {
            reg.store()
                .items()
                .iter()
                .filter(|i| i.type_id == 185)
                .map(|i| i.quantity)
                .sum()
        };
        assert_eq!(
            call(
                &mut reg,
                "dogmaIM",
                "LoadAmmo",
                &[
                    PyValue::Int(merlin),
                    PyValue::Int(module),
                    PyValue::List(vec![PyValue::Int(ammo)]),
                    PyValue::Int(STATION),
                ],
                &[],
                &none,
            ),
            Ok(PyValue::None)
        );
        assert_eq!(total(&reg), 500);
        assert!(crate::modules::loaded(reg.store(), merlin, HI0).is_some());

        let listed = call(
            &mut reg,
            "invbroker::inventory",
            "List",
            &[],
            &[kw("flag", PyValue::Int(HI0))],
            &ship_inv(merlin),
        )
        .expect("listed");
        let PyValue::List(rows) = listed else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 1, "the module and nothing else");
        assert_eq!(column(&rows[0], "itemID"), DbValue::Int(module));

        let key = crate::modules::charge_key(merlin, HI0, 185);
        let info = call(
            &mut reg,
            "dogmaIM",
            "ItemGetInfo",
            std::slice::from_ref(&key),
            &[],
            &none,
        )
        .expect("answered");
        assert_eq!(*attr(&info, "itemID"), key);

        let landed = call(
            &mut reg,
            "dogmaIM",
            "UnloadAmmo",
            &[
                PyValue::Int(merlin),
                PyValue::List(vec![PyValue::Int(module)]),
                PyValue::Int(STATION),
            ],
            &[],
            &none,
        )
        .expect("unloaded");
        let PyValue::List(ids) = landed else {
            panic!("the landed stacks")
        };
        assert_eq!(ids.len(), 1);
        assert!(crate::modules::loaded(reg.store(), merlin, HI0).is_none());
        assert_eq!(total(&reg), 500);
    }

    /// A Python `set` of ints as it arrives: tag 0x22 around
    /// `__builtin__.set`, the list one level down in the argument tuple.
    fn py_set_of(ids: &[i64]) -> PyValue {
        PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Global(b"__builtin__.set".to_vec()),
                PyValue::Tuple(vec![PyValue::List(
                    ids.iter().map(|i| PyValue::Int(*i)).collect(),
                )]),
            ])),
            list: Vec::new(),
            dict: Vec::new(),
        }
    }

    fn load_ammo(reg: &mut Registry, ship: i64, modules: PyValue, ammo: i64) {
        call(
            reg,
            "dogmaIM",
            "LoadAmmo",
            &[
                PyValue::Int(ship),
                modules,
                PyValue::List(vec![PyValue::Int(ammo)]),
                PyValue::Int(STATION),
            ],
            &[],
            &PyValue::None,
        )
        .expect("answered");
    }

    /// **"Load to all" sends a set** (`cdl:1254`, `cdl:1299`): the owner's
    /// capture has `LoadAmmo(ship, set([m1, m2, ...]), [stack], loc)` answer
    /// `None` and load nothing, because the set was read as no modules. Two
    /// guns, one stack: both fill, in order, from the same stack, and no
    /// round is made or lost.
    #[test]
    fn load_to_all_with_a_set_fills_every_module_from_the_stack() {
        let (mut reg, merlin) = in_a_merlin();
        let mut modules = Vec::new();
        for flag in [HI0, HI0 + 1] {
            let gun = grant(&mut reg, 484, 1);
            fit(&mut reg, merlin, gun, flag).expect("fitted");
            modules.push(fitted_at(&reg, merlin, flag)[0].item_id);
        }
        let ammo = grant(&mut reg, 185, 500);
        let total = |reg: &Registry| -> i64 {
            reg.store()
                .items()
                .iter()
                .filter(|i| i.type_id == 185)
                .map(|i| i.quantity)
                .sum()
        };
        load_ammo(&mut reg, merlin, py_set_of(&modules), ammo);
        let a = crate::modules::loaded(reg.store(), merlin, HI0).expect("first gun loaded");
        let b = crate::modules::loaded(reg.store(), merlin, HI0 + 1).expect("second gun loaded");
        assert!(a.quantity > 0 && b.quantity > 0);
        assert_eq!(a.quantity, b.quantity, "same gun, same capacity");
        assert_eq!(total(&reg), 500, "no round made or lost");
        let left = reg.item(ammo).map_or(0, |i| i.quantity);
        assert_eq!(
            left,
            500 - a.quantity - b.quantity,
            "the stack paid for both"
        );
    }

    /// Quantity bookkeeping across a dry gun: the loaded count is the stack
    /// the HUD reads; a gun that has fired its last round holds nothing, and
    /// a reload from what is left fills it again to its capacity (or to what
    /// is left), the stack shrinking by exactly that.
    #[test]
    fn reload_after_running_dry_refills_from_the_remaining_stack() {
        let (mut reg, merlin) = in_a_merlin();
        let gun = grant(&mut reg, 484, 1);
        fit(&mut reg, merlin, gun, HI0).expect("fitted");
        let module = fitted_at(&reg, merlin, HI0)[0].item_id;
        let ammo = grant(&mut reg, 185, 500);
        load_ammo(&mut reg, merlin, PyValue::Int(module), ammo);
        let full = crate::modules::loaded(reg.store(), merlin, HI0)
            .expect("loaded")
            .quantity;
        let after_first = reg.item(ammo).map_or(0, |i| i.quantity);
        assert_eq!(after_first, 500 - full);
        // Fire everything: the round count goes to nothing.
        let charge = crate::modules::loaded(reg.store(), merlin, HI0)
            .expect("loaded")
            .item_id;
        reg.apply_items_quiet(&[crate::item::Edit::Consume {
            item_id: charge,
            quantity: full,
        }])
        .expect("fired");
        assert!(crate::modules::loaded(reg.store(), merlin, HI0).is_none());
        // The reload, with the module as a one-element set as well.
        load_ammo(&mut reg, merlin, py_set_of(&[module]), ammo);
        let again = crate::modules::loaded(reg.store(), merlin, HI0).expect("reloaded");
        assert_eq!(again.quantity, full.min(after_first));
        assert_eq!(
            reg.item(ammo).map_or(0, |i| i.quantity),
            after_first - again.quantity
        );
    }

    /// Test 15: "Fit to Active Ship" is `MultiAdd(ids, src, flag=0)`
    /// (`fittingUtil.py:171`) [V]: two high-slot modules land in two
    /// different high slots of the hull.
    #[test]
    fn an_autofit_places_each_module_in_its_own_slot() {
        let (mut reg, merlin) = in_a_merlin();
        let gun = grant(&mut reg, 484, 1);
        let miner = grant(&mut reg, 483, 1);
        let got = call(
            &mut reg,
            "invbroker::inventory",
            "MultiAdd",
            &[
                PyValue::List(vec![PyValue::Int(gun), PyValue::Int(miner)]),
                PyValue::Int(STATION),
            ],
            &[kw("flag", PyValue::Int(0))],
            &ship_inv(merlin),
        );
        assert_eq!(got, Ok(PyValue::None));
        let hi = crate::fitting::hull_in(reg.content(), 603)
            .expect("Merlin")
            .hi;
        let mut flags: Vec<i64> = reg
            .store()
            .contents_of(merlin)
            .filter(|i| i.type_id == 484 || i.type_id == 483)
            .map(|i| i.flag_id)
            .collect();
        flags.sort_unstable();
        flags.dedup();
        assert_eq!(flags.len(), 2);
        assert!(
            flags.iter().all(|f| (HI0..HI0 + hi).contains(f)),
            "{flags:?}"
        );
    }

    type ItemPush = (i64, i64, i64, Vec<i64>);

    /// The `OnItemChange` pushes of `take_pending` as `(itemID, locationID,
    /// flagID, change keys)`, read by column name and by the change dict's
    /// integer keys (marshal dict entries are `(value, key)`).
    fn item_changes(reg: &mut Registry) -> (Vec<ItemPush>, usize, Vec<&'static str>) {
        let mut rows = Vec::new();
        let mut invalidations = 0;
        let mut names = Vec::new();
        for (_, p) in reg.take_pending() {
            let (name, args) = p.to_push();
            names.push(name);
            if name == "OnInvalidateCacheForLocations" {
                invalidations += 1;
            }
            if name != "OnItemChange" {
                continue;
            }
            let int = |col: &str| match column(&args[0], col) {
                DbValue::Int(v) => v,
                other => panic!("{col} is not an int: {other:?}"),
            };
            let PyValue::Dict(entries) = &args[1] else {
                panic!("the change is a dict, got {:?}", args[1]);
            };
            let keys = entries
                .iter()
                .filter_map(|(_, k)| match k {
                    PyValue::Int(k) => Some(*k),
                    _ => None,
                })
                .collect();
            rows.push((int("itemID"), int("locationID"), int("flagID"), keys));
        }
        (rows, invalidations, names)
    }

    /// FIT-DRAG #13: every drag-fit shape tells the client with an
    /// `OnItemChange` whose change names the old location (what
    /// `fittingController.OnDogmaItemChange` gates on, `fittingController.py:334`),
    /// and a refused drop onto a full rack or an occupied slot tells it nothing
    /// but the refusal. A regression guard for today's (correct) shape, on a
    /// hull in the fixture; it is not a red-first test.
    #[test]
    fn every_drag_fit_pushes_a_located_item_change_and_a_refused_drop_pushes_none() {
        let (mut reg, ship) = in_a_merlin();
        let hi = crate::fitting::hull_in(reg.content(), 603)
            .expect("Merlin")
            .hi;
        assert!(hi >= 3, "the fixture hull needs three high slots");
        let in_rack = |f: i64| (HI0..HI0 + hi).contains(&f);
        let moved_in = |keys: &[i64]| {
            keys.contains(&crate::item::IX_LOCATION_ID) || keys.contains(&crate::item::IX_FLAG)
        };

        // (1) a slot drop of a hangar singleton: Add(flag = HI0)
        let single = grant(&mut reg, 484, 1);
        let _ = reg.take_pending();
        assert_eq!(fit(&mut reg, ship, single, HI0), Ok(PyValue::None));
        let (rows, inval, names) = item_changes(&mut reg);
        assert!(
            names.contains(&"OnModuleAttributeChanges"),
            "a fit tells the open fitting window to re-read its slots: {names:?}"
        );
        let hit = rows
            .iter()
            .find(|r| r.0 == single)
            .expect("the fitted singleton is pushed");
        assert_eq!(
            (hit.1, hit.2),
            (ship, HI0),
            "the row is at the ship and slot"
        );
        assert!(
            moved_in(&hit.3),
            "the change names the old place: {:?}",
            hit.3
        );
        assert!(inval >= 1, "and the container is invalidated");

        // (2) a centre drop of one unit of a stack: MultiAdd([stack], flag = 0)
        let stack = grant(&mut reg, 484, 2);
        let _ = reg.take_pending();
        let got = call(
            &mut reg,
            "invbroker::inventory",
            "MultiAdd",
            &[
                PyValue::List(vec![PyValue::Int(stack)]),
                PyValue::Int(STATION),
            ],
            &[kw("flag", PyValue::Int(0))],
            &ship_inv(ship),
        );
        assert_eq!(got, Ok(PyValue::None));
        let (rows, _, _) = item_changes(&mut reg);
        let unit = rows
            .iter()
            .find(|r| r.1 == ship && in_rack(r.2))
            .expect("the unit split off the stack is pushed at the ship");
        assert!(
            moved_in(&unit.3),
            "the change names the old place: {:?}",
            unit.3
        );

        // (3) a multi-drag onto ONE slot: the second Add is refused and silent
        let a = grant(&mut reg, 484, 1);
        let b = grant(&mut reg, 484, 1);
        let _ = reg.take_pending();
        let free = (HI0..HI0 + hi)
            .find(|f| fitted_at(&reg, ship, *f).is_empty())
            .expect("a free high slot");
        assert_eq!(fit(&mut reg, ship, a, free), Ok(PyValue::None));
        let _ = reg.take_pending();
        let err = fit(&mut reg, ship, b, free).expect_err("the slot is taken");
        assert_eq!(raised_key(&err).as_deref(), Some("SlotAlreadyOccupied"));
        let (rows, inval, names) = item_changes(&mut reg);
        assert!(
            rows.is_empty() && inval == 0 && names.is_empty(),
            "a refused Add pushes nothing: {names:?}"
        );

        // (4) fill the rack, then drop one more on the centre: refused, silent
        let open = (HI0..HI0 + hi)
            .filter(|f| fitted_at(&reg, ship, *f).is_empty())
            .count();
        let fillers: Vec<PyValue> = (0..open)
            .map(|_| PyValue::Int(grant(&mut reg, 484, 1)))
            .collect();
        if !fillers.is_empty() {
            let got = call(
                &mut reg,
                "invbroker::inventory",
                "MultiAdd",
                &[PyValue::List(fillers), PyValue::Int(STATION)],
                &[kw("flag", PyValue::Int(0))],
                &ship_inv(ship),
            );
            assert_eq!(got, Ok(PyValue::None));
        }
        assert!(
            (HI0..HI0 + hi).all(|f| !fitted_at(&reg, ship, f).is_empty()),
            "the rack is full"
        );
        let extra = grant(&mut reg, 484, 1);
        let _ = reg.take_pending();
        let err = call(
            &mut reg,
            "invbroker::inventory",
            "MultiAdd",
            &[
                PyValue::List(vec![PyValue::Int(extra)]),
                PyValue::Int(STATION),
            ],
            &[kw("flag", PyValue::Int(0))],
            &ship_inv(ship),
        )
        .expect_err("no slot is free, and nothing fitted");
        assert!(matches!(err, ServiceError::Raise(_)), "{err:?}");
        let (rows, inval, names) = item_changes(&mut reg);
        assert!(
            rows.is_empty() && inval == 0 && names.is_empty(),
            "a refused MultiAdd pushes nothing: {names:?}"
        );
    }

    /// FIT-DRAG #13 cost guard: the plural slot-redraw push (`fitstate::SlotRedraw`)
    /// rides a call that changed which module sits in which slot and nothing
    /// else. A cargo `Add` and an online toggle send none; one `MultiAdd` that
    /// fits several modules sends exactly one.
    #[test]
    fn the_slot_redraw_push_rides_only_calls_that_move_a_fitted_module() {
        let count = |reg: &mut Registry| {
            reg.take_pending()
                .iter()
                .filter(|(_, p)| p.to_push().0 == "OnModuleAttributeChanges")
                .count()
        };
        let (mut reg, ship) = in_a_merlin();
        // a cargo Add: an item into the hold of the ship
        let crate_item = grant(&mut reg, 484, 1);
        let _ = reg.take_pending();
        let got = call(
            &mut reg,
            "invbroker::inventory",
            "Add",
            &[PyValue::Int(crate_item), PyValue::Int(STATION)],
            &[kw("qty", PyValue::Int(1)), kw("flag", PyValue::Int(CARGO))],
            &ship_inv(ship),
        );
        assert_eq!(got, Ok(PyValue::None));
        assert_eq!(count(&mut reg), 0, "a cargo Add moves no fitted module");

        // one MultiAdd fitting two modules: exactly one plural push
        let a = grant(&mut reg, 484, 1);
        let b = grant(&mut reg, 484, 1);
        let _ = reg.take_pending();
        let got = call(
            &mut reg,
            "invbroker::inventory",
            "MultiAdd",
            &[
                PyValue::List(vec![PyValue::Int(a), PyValue::Int(b)]),
                PyValue::Int(STATION),
            ],
            &[kw("flag", PyValue::Int(0))],
            &ship_inv(ship),
        );
        assert_eq!(got, Ok(PyValue::None));
        assert_eq!(count(&mut reg), 1, "one push for the whole MultiAdd");

        // an online toggle of the fitted module: no slot moved
        let _ = reg.take_pending();
        let _ = crate::modules::set_module_online(
            &mut reg,
            ACTOR,
            a,
            false,
            crate::pyutil::filetime_now(),
        );
        assert_eq!(count(&mut reg), 0, "an online toggle moves no module");
    }

    /// FAST-KIT: a bot fits all its guns in one call, `MultiAdd([id, id,
    /// id])` over one stack of three; each fit splits one unit off the stack,
    /// so three modules land in three different slots.
    #[test]
    fn one_multiadd_over_a_stack_fits_one_unit_per_listed_id() {
        let (mut reg, merlin) = in_a_merlin();
        let stack = grant(&mut reg, 484, 3);
        let hi = crate::fitting::hull_in(reg.content(), 603)
            .expect("Merlin")
            .hi;
        assert!(hi >= 3, "the fixture hull needs three high slots");
        let got = call(
            &mut reg,
            "invbroker::inventory",
            "MultiAdd",
            &[
                PyValue::List(vec![PyValue::Int(stack); 3]),
                PyValue::Int(STATION),
            ],
            &[kw("flag", PyValue::Int(0))],
            &ship_inv(merlin),
        );
        assert_eq!(got, Ok(PyValue::None));
        let mut flags: Vec<i64> = reg
            .store()
            .contents_of(merlin)
            .filter(|i| i.type_id == 484)
            .map(|i| i.flag_id)
            .collect();
        flags.sort_unstable();
        flags.dedup();
        assert_eq!(flags.len(), 3, "{flags:?}");
    }

    /// Test 16: `Board`'s reply is the `shipState` 4-tuple (unpacked
    /// unguarded at `cdl:303`) [V], and exactly one `{'shipid': (old, new)}`
    /// session change is queued.
    #[test]
    fn boarding_answers_the_ship_state_and_moves_shipid() {
        let mut reg = Registry::new(SandboxConfig::default());
        let merlin = grant(&mut reg, 603, 1);
        let pod = pod(&reg);
        let got = board_it(&mut reg, merlin).expect("boarded");
        let PyValue::Tuple(parts) = got else {
            panic!("a tuple, got {got:?}")
        };
        assert_eq!(parts.len(), 4);
        assert!(matches!(
            (&parts[0], &parts[1], &parts[2], &parts[3]),
            (
                PyValue::Dict(_),
                PyValue::Dict(_),
                PyValue::None,
                PyValue::Dict(_)
            )
        ));
        assert_eq!((me(&reg).ship_id, me(&reg).ship_type_id), (merlin, 603));
        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1);
        let (who, change) = &changes[0];
        assert_eq!(Some(*who), ACTOR);
        assert_eq!(change.len(), 1);
        assert_eq!(
            crate::pyutil::pairs_get(change, b"shipid"),
            Some(&PyValue::Tuple(vec![
                PyValue::Int(pod),
                PyValue::Int(merlin)
            ]))
        );
    }

    /// Test 17: a refused `Board` queues no session change and changes
    /// nothing.
    #[test]
    fn a_refused_board_changes_nothing() {
        let mut reg = Registry::new(SandboxConfig::default());
        let theirs = 100_000_500;
        let mut items = reg.store().items().to_vec();
        let t = *crate::item::find_in(reg.content(), 603).expect("Merlin");
        items.push(crate::item::Item {
            item_id: theirs,
            type_id: 603,
            owner_id: 90_000_999,
            location_id: STATION,
            flag_id: 4,
            quantity: 1,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: theirs + 1,
            ..crate::store::Snapshot::default()
        });
        let before = (reg.store().items().to_vec(), me(&reg).ship_id);
        assert!(matches!(
            board_it(&mut reg, theirs),
            Err(ServiceError::Raise(_))
        ));
        assert!(reg.take_session_changes().is_empty());
        assert_eq!((reg.store().items().to_vec(), me(&reg).ship_id), before);
    }

    /// Test 18: after boarding, godma's re-prime (`GetAllInfo`) is keyed by
    /// the new ship and its hull row is the Merlin's.
    #[test]
    fn after_boarding_get_all_info_primes_the_new_hull() {
        let (reg, merlin) = in_a_merlin();
        let info = reg.all_info(ACTOR);
        let PyValue::Dict(ship_info) = attr(&info, "shipInfo") else {
            panic!("a dict")
        };
        let (row, _) = ship_info
            .iter()
            .find(|(_, k)| *k == PyValue::Int(merlin))
            .expect("keyed by the Merlin");
        assert_eq!(column(attr(row, "invItem"), "typeID"), DbValue::Int(603));
    }

    /// Test 19: board, then leave: back in the capsule, which is back in the
    /// hangar beside the Merlin; one session change each way.
    #[test]
    fn leaving_the_ship_puts_the_capsule_back() {
        let mut reg = Registry::new(SandboxConfig::default());
        let merlin = grant(&mut reg, 603, 1);
        let pod = pod(&reg);
        board_it(&mut reg, merlin).expect("boarded");
        let got = call(
            &mut reg,
            "ship",
            "LeaveShip",
            &[PyValue::Int(merlin)],
            &[],
            &ship_access(),
        );
        assert_eq!(got, Ok(PyValue::Int(pod)));
        assert_eq!(me(&reg).ship_id, pod);
        for id in [pod, merlin] {
            let i = reg.store().get(id).expect("exists");
            assert_eq!((i.location_id, i.flag_id), (STATION, 4));
        }
        assert_eq!(reg.take_session_changes().len(), 2);
    }

    // ---- docked-now (P1.12, `docs/lanes/docked.md`) ----
    //
    // `Character::station_id` is "my station" and is never cleared on
    // undock, so every guard below that read it passed in space. Each test
    // here is the in-space half; the docked half is the test named in its
    // comment, or is in the same test.

    /// Put the fixture pilot in space the way a real undock does: the
    /// ballpark bind seeds the roster.
    fn launch(reg: &mut Registry) {
        crate::destiny::on_ballpark_bind(reg, ACTOR);
        let _ = reg.take_pending();
        let _ = reg.take_session_changes();
        assert_eq!(reg.docked_station(ACTOR.expect("actor")), None);
    }

    /// Docked half: `boarding_answers_the_ship_state_and_moves_shipid`.
    #[test]
    fn in_space_a_ship_parked_at_home_cannot_be_boarded() {
        let mut reg = Registry::new(SandboxConfig::default());
        let merlin = grant(&mut reg, 603, 1);
        launch(&mut reg);
        let before = (reg.store().items().to_vec(), me(&reg).ship_id);
        assert!(matches!(
            board_it(&mut reg, merlin),
            Err(ServiceError::Raise(_))
        ));
        assert!(reg.take_session_changes().is_empty());
        assert_eq!((reg.store().items().to_vec(), me(&reg).ship_id), before);
    }

    /// Docked half: `leaving_the_ship_puts_the_capsule_back`.
    #[test]
    fn in_space_leaving_the_ship_is_refused() {
        let (mut reg, merlin) = in_a_merlin();
        launch(&mut reg);
        let before = reg.store().items().to_vec();
        let got = call(
            &mut reg,
            "ship",
            "LeaveShip",
            &[PyValue::Int(merlin)],
            &[],
            &ship_access(),
        );
        assert!(matches!(got, Err(ServiceError::Raise(_))), "{got:?}");
        assert_eq!(me(&reg).ship_id, merlin);
        assert_eq!(reg.store().items().to_vec(), before);
        assert!(reg.take_session_changes().is_empty());
    }

    /// Docked half: `a_module_dragged_onto_a_slot_is_fitted`. P2.5 put the
    /// refusal in `fit_one`; this is its dispatch-level test.
    #[test]
    fn in_space_a_module_cannot_be_fitted_from_the_hangar() {
        let (mut reg, merlin) = in_a_merlin();
        let stack = grant(&mut reg, 484, 3);
        launch(&mut reg);
        assert!(matches!(
            fit(&mut reg, merlin, stack, HI0),
            Err(ServiceError::Raise(_))
        ));
        assert!(fitted_at(&reg, merlin, HI0).is_empty());
        assert_eq!(reg.store().get(stack).map(|i| i.quantity), Some(3));
    }

    /// Docked half: `repackaging_a_ship_clears_singleton_and_pushes_the_new_row`.
    #[test]
    fn in_space_a_hull_at_home_cannot_be_repackaged() {
        let mut reg = Registry::new(SandboxConfig::default());
        let rifter = grant(&mut reg, 587, 1);
        launch(&mut reg);
        let got = repackage(&mut reg, rifter);
        assert!(matches!(got, Err(ServiceError::Raise(_))), "{got:?}");
        assert_eq!(reg.store().get(rifter).map(|i| i.singleton), Some(true));
    }

    /// GAP-REPAIR: a damaged hull is not repackaged (`CantRepackageDamagedItem`),
    /// and is once the repair shop has mended it.
    #[test]
    fn a_damaged_hull_cannot_be_repackaged() {
        let mut reg = Registry::new(SandboxConfig::default());
        let rifter = grant(&mut reg, 587, 1);
        crate::repair::put(
            &mut reg,
            rifter,
            crate::repair::Hurt {
                armor: 10.0,
                structure: 0.0,
            },
        );
        let got = repackage(&mut reg, rifter);
        assert_eq!(
            got,
            Err(ServiceError::Raise(eve_proto::user_error(
                "CantRepackageDamagedItem",
                vec![]
            )))
        );
        assert_eq!(reg.store().get(rifter).map(|i| i.singleton), Some(true));
        crate::repair::put(&mut reg, rifter, crate::repair::Hurt::default());
        repackage(&mut reg, rifter).expect("whole, so repackaged");
    }

    fn repackage(reg: &mut Registry, item: i64) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "repackagingSvc",
                method: "RepackageItems",
                args: &[one_station_batch(STATION, &[item])],
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            },
        )
    }

    fn assemble(reg: &mut Registry, item: i64) -> Result<PyValue, ServiceError> {
        call(
            reg,
            "ship",
            "AssembleShip",
            &[PyValue::List(vec![PyValue::Int(item)])],
            &[],
            &ship_access(),
        )
    }

    fn assemble_with(
        reg: &mut Registry,
        ids: &[i64],
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<PyValue, ServiceError> {
        call(
            reg,
            "ship",
            "AssembleShip",
            &[PyValue::List(
                ids.iter().map(|i| PyValue::Int(*i)).collect(),
            )],
            kwargs,
            &ship_access(),
        )
    }

    fn name_kw(v: PyValue) -> (PyValue, PyValue) {
        (v, PyValue::Bytes(b"name".to_vec()))
    }

    /// The name the client sends with the assemble is the ship's name, and the
    /// caller's cached row is replaced by a push.
    #[test]
    fn an_assembled_hull_takes_the_name_it_was_given() {
        let mut reg = Registry::new(SandboxConfig::default());
        let hull = grant(&mut reg, 587, 1);
        repackage(&mut reg, hull).expect("repackaged");
        let _ = reg.take_pending();
        let got = assemble_with(
            &mut reg,
            &[hull],
            &[name_kw(PyValue::Unicode("Wreck - one".to_owned()))],
        )
        .expect("assembled");
        assert_eq!(got, PyValue::List(vec![PyValue::Int(hull)]));
        assert_eq!(
            crate::item_names::name_of(&reg, hull).as_deref(),
            Some("Wreck - one")
        );
        let pushed = reg.take_pending();
        let (_, args) = pushed
            .iter()
            .map(|(_, p)| p.to_push())
            .find(|(e, _)| *e == "OnCfgDataChanged")
            .expect("a cfg push");
        let PyValue::List(row) = &args[1] else {
            panic!("a row list")
        };
        assert_eq!(row[1], PyValue::Unicode("Wreck - one".to_owned()));
    }

    /// `name=None`, no kwarg and a blank name all leave the ship unnamed.
    #[test]
    fn an_assemble_without_a_name_stores_none() {
        for kwargs in [
            vec![name_kw(PyValue::None)],
            vec![],
            vec![name_kw(PyValue::Unicode("   ".to_owned()))],
        ] {
            let mut reg = Registry::new(SandboxConfig::default());
            let hull = grant(&mut reg, 587, 1);
            repackage(&mut reg, hull).expect("repackaged");
            assemble_with(&mut reg, &[hull], &kwargs).expect("assembled");
            assert_eq!(crate::item_names::name_of(&reg, hull), None);
        }
    }

    /// A ship that is already assembled is not renamed by an assemble.
    #[test]
    fn an_assembled_ship_is_not_renamed_by_assemble() {
        let mut reg = Registry::new(SandboxConfig::default());
        let hull = grant(&mut reg, 587, 1);
        assemble_with(
            &mut reg,
            &[hull],
            &[name_kw(PyValue::Unicode("Late".to_owned()))],
        )
        .expect("answered");
        assert_eq!(crate::item_names::name_of(&reg, hull), None);
    }

    /// One ship comes off a stack of two: the new id is named, the stack not.
    #[test]
    fn a_stack_assembles_a_new_named_ship() {
        let mut reg = Registry::new(SandboxConfig::default());
        let a = grant(&mut reg, 587, 1);
        let b = grant(&mut reg, 587, 1);
        repackage(&mut reg, a).expect("repackaged");
        repackage(&mut reg, b).expect("repackaged");
        reg.apply_items(&[crate::item::Edit::Merge { from: b, into: a }])
            .expect("merged");
        assert_eq!(reg.store().get(a).map(|i| i.quantity), Some(2));
        let stack = a;
        let got = assemble_with(
            &mut reg,
            &[stack],
            &[name_kw(PyValue::Unicode("Second".to_owned()))],
        )
        .expect("assembled");
        let PyValue::List(ids) = got else {
            panic!("a list")
        };
        let [PyValue::Int(new_id)] = ids[..] else {
            panic!("one id")
        };
        assert_ne!(new_id, stack, "a new id came off the stack");
        assert_eq!(
            crate::item_names::name_of(&reg, new_id).as_deref(),
            Some("Second")
        );
        assert_eq!(crate::item_names::name_of(&reg, stack), None);
    }

    /// Both halves: a packaged hull in the home hangar assembles docked and
    /// is refused, unchanged, from space.
    #[test]
    fn a_packaged_hull_assembles_docked_and_not_from_space() {
        let packaged = || {
            let mut reg = Registry::new(SandboxConfig::default());
            let rifter = grant(&mut reg, 587, 1);
            repackage(&mut reg, rifter).expect("repackaged docked");
            assert_eq!(reg.store().get(rifter).map(|i| i.singleton), Some(false));
            (reg, rifter)
        };
        let (mut reg, rifter) = packaged();
        let (mut space, _) = packaged();
        launch(&mut space);
        let got = assemble(&mut space, rifter);
        assert!(matches!(got, Err(ServiceError::Raise(_))), "{got:?}");
        assert_eq!(space.store().get(rifter).map(|i| i.singleton), Some(false));

        assemble(&mut reg, rifter).expect("assembled docked");
        assert!(
            reg.store()
                .items()
                .iter()
                .any(|i| i.type_id == 587 && i.singleton && i.location_id == STATION),
            "an assembled Rifter in the hangar"
        );
    }

    /// Docked half: `stack_all_merges_the_hangar_and_invalidates_the_cache`.
    /// In space "Stack All" on the hangar is a quiet no-op: it raises
    /// nothing, and merges nothing.
    #[test]
    fn in_space_stack_all_leaves_the_home_hangar_alone() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = actor(&reg);
        for _ in 0..3 {
            reg.slash(me, "/createitem EMP S 100").expect("createitem");
        }
        launch(&mut reg);
        let before = reg.store().items().to_vec();
        let hangar = PyValue::Tuple(vec![PyValue::Int(CONTAINER_HANGAR)]);
        let got = dispatch(
            &mut reg,
            &Request {
                service: "invbroker::inventory",
                method: "StackAll",
                args: &[],
                kwargs: &[],
                bind_params: Some(&hangar),
                actor: ACTOR,
            },
        );
        assert_eq!(got, Ok(PyValue::None));
        assert_eq!(reg.store().items().to_vec(), before);
    }

    // ---- cargo (P5.12, `docs/lanes/inventory.md`) ----

    /// `flagCargo` [V].
    const CARGO: i64 = crate::fitting::FLAG_CARGO;
    /// `flagHangar` [V].
    const HANGAR: i64 = crate::model::FLAG_HANGAR;
    const VELDSPAR: i64 = 1230;
    const EMP_S: i64 = 185;

    /// A drop on a hold: `Add(itemID, sourceID, qty=n, flag=flagCargo)` on
    /// the ship's inventory (`invControllers.py:267`) [V].
    fn into_cargo(
        reg: &mut Registry,
        ship: i64,
        item: i64,
        from: i64,
        qty: Option<i64>,
    ) -> Result<PyValue, ServiceError> {
        let qty = qty.map_or(PyValue::None, PyValue::Int);
        call(
            reg,
            "invbroker::inventory",
            "Add",
            &[PyValue::Int(item), PyValue::Int(from)],
            &[kw("qty", qty), kw("flag", PyValue::Int(CARGO))],
            &ship_inv(ship),
        )
    }

    /// A drop on the station hangar: the same call on the hangar's
    /// inventory, `flag=flagHangar`.
    fn into_hangar(
        reg: &mut Registry,
        item: i64,
        from: i64,
        qty: Option<i64>,
    ) -> Result<PyValue, ServiceError> {
        let qty = qty.map_or(PyValue::None, PyValue::Int);
        call(
            reg,
            "invbroker::inventory",
            "Add",
            &[PyValue::Int(item), PyValue::Int(from)],
            &[kw("qty", qty), kw("flag", PyValue::Int(HANGAR))],
            &hangar_inv(),
        )
    }

    /// A hangar stack of any size: `/createitem` clamps to 1,000, and a
    /// full hold of ore is more than that.
    fn hangar_stack(reg: &mut Registry, type_id: i64, quantity: i64) -> i64 {
        let owner = me(reg).character_id;
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id,
                quantity,
                to: crate::item::Slot::new(owner, STATION, HANGAR),
            }])
            .expect("created");
        changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("a stack")
    }

    /// Units of `type_id` I own at `(location, flag)`.
    fn units_at(reg: &Registry, type_id: i64, location: i64, flag: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| {
                i.type_id == type_id
                    && i.owner_id == me(reg).character_id
                    && i.location_id == location
                    && i.flag_id == flag
            })
            .map(|i| i.quantity)
            .sum()
    }

    fn units_owned(reg: &Registry, type_id: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.type_id == type_id && i.owner_id == me(reg).character_id)
            .map(|i| i.quantity)
            .sum()
    }

    /// The free m3 in a ship's hold as `fitting::plan_to_cargo` counts it:
    /// the type's unmodified capacity less what is in it, both from the
    /// registry's content.
    fn hold_free(reg: &Registry, ship: i64) -> f64 {
        let capacity = crate::fitting::base_cargo_capacity(reg.content(), reg.store(), ship);
        (capacity - reg.store().used_volume_in(reg.content(), ship, CARGO)).max(0.0)
    }

    fn hold_within_capacity(reg: &Registry, ship: i64) {
        let used = reg.store().used_volume_in(reg.content(), ship, CARGO);
        let capacity = crate::fitting::base_cargo_capacity(reg.content(), reg.store(), ship);
        assert!(capacity > 0.0, "a hull has a hold");
        assert!(
            used <= capacity + 1e-9,
            "the hold is over capacity: {used} > {capacity}"
        );
    }

    /// **Hangar -> hold -> hangar, split and whole**, and no unit is made or
    /// lost on the way. The replies are `None`: `__AddItem` returns ours
    /// untouched and its callers discard it (`invControllers.py:267`,
    /// `:317`) [V].
    #[test]
    fn a_stack_moves_into_the_hold_and_back_split_or_whole() {
        let (mut reg, merlin) = in_a_merlin();
        let stack = grant(&mut reg, EMP_S, 500);
        assert_eq!(
            into_cargo(&mut reg, merlin, stack, STATION, Some(200)),
            Ok(PyValue::None)
        );
        assert_eq!(units_at(&reg, EMP_S, merlin, CARGO), 200);
        assert_eq!(units_at(&reg, EMP_S, STATION, HANGAR), 300);
        assert_eq!(units_owned(&reg, EMP_S), 500);

        // The rest, whole: `qty` is the stack's own size.
        assert_eq!(
            into_cargo(&mut reg, merlin, stack, STATION, Some(300)),
            Ok(PyValue::None)
        );
        assert_eq!(units_at(&reg, EMP_S, STATION, HANGAR), 0);
        assert_eq!(units_at(&reg, EMP_S, merlin, CARGO), 500);
        hold_within_capacity(&reg, merlin);

        // Out again: part of one stack, then everything left.
        let in_hold: Vec<crate::item::Item> = fitted_at(&reg, merlin, CARGO);
        let first = in_hold[0].item_id;
        let part = in_hold[0].quantity - 1;
        assert_eq!(
            into_hangar(&mut reg, first, merlin, Some(part)),
            Ok(PyValue::None)
        );
        assert_eq!(units_owned(&reg, EMP_S), 500);
        for i in fitted_at(&reg, merlin, CARGO) {
            into_hangar(&mut reg, i.item_id, merlin, Some(i.quantity)).expect("moved out");
        }
        assert_eq!(units_at(&reg, EMP_S, merlin, CARGO), 0);
        assert_eq!(units_at(&reg, EMP_S, STATION, HANGAR), 500);
    }

    /// **The hold is never over capacity**: the most that fits by the
    /// client's own arithmetic (`_CalculateMaxQuantity`, `:373`) goes in;
    /// one unit more is refused with `NotEnoughCargoSpace` -- the key
    /// `invControllers._AddItem` catches to re-offer the move at the quantity
    /// that fits (`:319`-`:344`) [V] -- and a refusal changes nothing.
    #[test]
    fn a_hold_takes_what_fits_and_refuses_one_unit_more() {
        let (mut reg, merlin) = in_a_merlin();
        let free = hold_free(&reg, merlin);
        let unit = crate::item::find_in(reg.content(), VELDSPAR)
            .expect("Veldspar")
            .volume;
        let fits = crate::fitting::units_that_fit(free, unit);
        assert!(fits > 0);
        let rock = hangar_stack(&mut reg, VELDSPAR, fits + 10);
        let before = reg.store().items().to_vec();

        let whole =
            into_cargo(&mut reg, merlin, rock, STATION, Some(fits + 10)).expect_err("too much");
        assert_eq!(raised_key(&whole).as_deref(), Some("NotEnoughCargoSpace"));
        assert_eq!(
            reg.store().items(),
            &before[..],
            "a refusal changes nothing"
        );

        into_cargo(&mut reg, merlin, rock, STATION, Some(fits)).expect("exactly full");
        assert_eq!(units_at(&reg, VELDSPAR, merlin, CARGO), fits);
        hold_within_capacity(&reg, merlin);

        let one_more = into_cargo(&mut reg, merlin, rock, STATION, Some(1)).expect_err("full");
        assert_eq!(
            raised_key(&one_more).as_deref(),
            Some("NotEnoughCargoSpace")
        );
        assert_eq!(units_owned(&reg, VELDSPAR), fits + 10);
        hold_within_capacity(&reg, merlin);
    }

    /// **CAP-B, over the dispatch path**: `Add(..., flag=flagSpecializedFuelBay)`
    /// on a ship's inventory reaches the fuel-bay rule -- ore is refused with
    /// the client's `CannotPlaceTypeInFuelBay`, not the old "only the hangar,
    /// cargo and slots" notice, and a refusal changes nothing.
    #[test]
    fn a_drop_on_the_fuel_bay_reaches_the_bay_rule() {
        let (mut reg, merlin) = in_a_merlin();
        let rock = hangar_stack(&mut reg, VELDSPAR, 10);
        let before = reg.store().items().to_vec();
        let got = call(
            &mut reg,
            "invbroker::inventory",
            "Add",
            &[PyValue::Int(rock), PyValue::Int(STATION)],
            &[
                kw("qty", PyValue::Int(10)),
                kw("flag", PyValue::Int(crate::item::FLAG_FUEL_BAY)),
            ],
            &ship_inv(merlin),
        )
        .expect_err("ore is not fuel");
        assert_eq!(
            raised_key(&got).as_deref(),
            Some("CannotPlaceTypeInFuelBay")
        );
        assert_eq!(reg.store().items(), &before[..]);
    }

    /// Every push a cargo move queues is addressed to the owner, and the
    /// move is announced as item rows (`OnItemChange`) -- the only news a
    /// discarded reply leaves the client.
    #[test]
    fn a_cargo_move_tells_the_owner_and_nobody_else() {
        let (mut reg, merlin) = in_a_merlin();
        let stack = grant(&mut reg, EMP_S, 50);
        let _ = reg.take_pending();
        into_cargo(&mut reg, merlin, stack, STATION, Some(20)).expect("split in");
        let sent = reg.take_pending();
        assert!(!sent.is_empty());
        let mut item_changes = 0;
        for (_, p) in &sent {
            assert_eq!(p.audience(), Some(vec![me(&reg).character_id]), "{p:?}");
            if p.to_push().0 == "OnItemChange" {
                item_changes += 1;
            }
        }
        assert!(item_changes >= 1, "{sent:?}");
    }

    /// A multi-drag into a hold: what fits moves, in the order sent; the
    /// rest stays where it was and the refusal is raised, not swallowed.
    #[test]
    fn a_multi_drag_into_the_hold_moves_what_fits_and_says_so() {
        let (mut reg, merlin) = in_a_merlin();
        let free = hold_free(&reg, merlin);
        let unit = crate::item::find_in(reg.content(), VELDSPAR)
            .expect("Veldspar")
            .volume;
        let fits = crate::fitting::units_that_fit(free, unit);
        let a = grant(&mut reg, EMP_S, 10);
        let b = hangar_stack(&mut reg, VELDSPAR, fits + 1);
        let got = call(
            &mut reg,
            "invbroker::inventory",
            "MultiAdd",
            &[
                PyValue::List(vec![PyValue::Int(a), PyValue::Int(b)]),
                PyValue::Int(STATION),
            ],
            &[kw("flag", PyValue::Int(CARGO))],
            &ship_inv(merlin),
        );
        assert!(got.is_err(), "the stack that did not fit is reported");
        assert_eq!(units_at(&reg, EMP_S, merlin, CARGO), 10);
        assert_eq!(units_at(&reg, VELDSPAR, STATION, HANGAR), fits + 1);
        hold_within_capacity(&reg, merlin);
    }

    /// Something that cannot go in a hold is refused and nothing moves: an
    /// assembled ship, and the ship into its own hold.
    #[test]
    fn a_ship_does_not_go_into_a_hold() {
        let (mut reg, merlin) = in_a_merlin();
        let rifter = grant(&mut reg, 587, 1);
        let before = reg.store().items().to_vec();
        assert!(into_cargo(&mut reg, merlin, rifter, STATION, Some(1)).is_err());
        assert!(into_cargo(&mut reg, merlin, merlin, STATION, Some(1)).is_err());
        assert_eq!(reg.store().items(), &before[..]);
    }

    /// The hold of **another ship parked in the hangar** -- the cargo
    /// window the client opens on any ship of mine there
    /// (`invCtrl.ShipCargo(itemID)`, `invControllers.py:742`) [V] -- takes
    /// items and gives them back like the active ship's.
    #[test]
    fn a_parked_ships_hold_takes_items_and_gives_them_back() {
        let (mut reg, merlin) = in_a_merlin();
        let rifter = grant(&mut reg, 587, 1);
        let stack = grant(&mut reg, EMP_S, 40);
        into_cargo(&mut reg, rifter, stack, STATION, Some(40)).expect("in");
        assert_eq!(units_at(&reg, EMP_S, rifter, CARGO), 40);
        into_hangar(&mut reg, stack, rifter, Some(40)).expect("out");
        assert_eq!(units_at(&reg, EMP_S, STATION, HANGAR), 40);
        assert_ne!(merlin, rifter);
    }

    /// A module dragged from its slot into the hold leaves the slot, and the
    /// hold is where it is.
    #[test]
    fn a_module_dragged_from_its_slot_to_the_hold_is_unfitted_into_it() {
        let (mut reg, merlin) = in_a_merlin();
        let gun = grant(&mut reg, 484, 1);
        fit(&mut reg, merlin, gun, HI0).expect("fitted");
        let module = fitted_at(&reg, merlin, HI0)[0].item_id;
        into_cargo(&mut reg, merlin, module, merlin, None).expect("unfitted to cargo");
        assert!(fitted_at(&reg, merlin, HI0).is_empty());
        let i = reg.item(module).expect("still exists");
        assert_eq!((i.location_id, i.flag_id), (merlin, CARGO));
        hold_within_capacity(&reg, merlin);
    }

    /// **`shipState[1]` carries the loaded charges** in the shape
    /// `_MakeShipActive` stores and `LoadSublocations` /
    /// `GetQuantityFromCache` read [V]: `{shipID: {flagID: row}}`, with
    /// `row[0..2]` the sublocation key `(shipID, flagID, typeID)`
    /// (`baseDogmaLocation.py:1580`) and `row.quantity` the count (`:310`).
    #[test]
    fn boarding_answers_the_loaded_charges_in_ship_state() {
        let (mut reg, merlin) = in_a_merlin();
        let gun = grant(&mut reg, 484, 1);
        fit(&mut reg, merlin, gun, HI0).expect("fitted");
        let module = fitted_at(&reg, merlin, HI0)[0].item_id;
        let ammo = grant(&mut reg, EMP_S, 500);
        call(
            &mut reg,
            "dogmaIM",
            "LoadAmmo",
            &[
                PyValue::Int(merlin),
                PyValue::Int(module),
                PyValue::List(vec![PyValue::Int(ammo)]),
                PyValue::Int(STATION),
            ],
            &[],
            &PyValue::None,
        )
        .expect("loaded");
        let rounds = crate::modules::loaded(reg.store(), merlin, HI0)
            .expect("loaded")
            .quantity;

        let PyValue::Tuple(state) = board_it(&mut reg, merlin).expect("answered") else {
            panic!("shipState is a 4-tuple: cdl:305 UNPACK_SEQUENCE 4")
        };
        assert_eq!(state.len(), 4);
        // A marshal dict entry is `(value, key)`.
        let PyValue::Dict(by_ship) = &state[1] else {
            panic!("a dict")
        };
        let [(PyValue::Dict(by_flag), PyValue::Int(ship))] = by_ship.as_slice() else {
            panic!("one ship: {by_ship:?}")
        };
        assert_eq!(*ship, merlin);
        let [(row, PyValue::Int(flag))] = by_flag.as_slice() else {
            panic!("one loaded slot: {by_flag:?}")
        };
        assert_eq!(*flag, HI0);
        let PyValue::DbRow(r) = row else {
            panic!("indexable and with attributes: {row:?}")
        };
        assert_eq!(
            r.values()[..3],
            [DbValue::Int(merlin), DbValue::Int(HI0), DbValue::Int(EMP_S)]
        );
        assert_eq!(column(row, "quantity"), DbValue::Int(rounds));
    }

    /// Nothing loaded is `{}` -- the value that has always worked, and what
    /// `LoadSublocations`' `.get(shipID, {})` reads as nothing [V].
    #[test]
    fn with_nothing_loaded_ship_state_has_no_sublocations() {
        let (mut reg, merlin) = in_a_merlin();
        let PyValue::Tuple(state) = board_it(&mut reg, merlin).expect("answered") else {
            panic!("a 4-tuple")
        };
        assert_eq!(state[1], PyValue::Dict(vec![]));
    }

    /// Every drone the tracker asks about gets an entry, or it asks again on
    /// every refresh (the client's main thread pinned at 100%).
    #[test]
    fn layer_damage_answers_every_asked_drone_whole() {
        let set = crate::fleet_finder::py_set(&std::collections::BTreeSet::from([7_001, 7_002]));
        let PyValue::Dict(rows) = layer_damage_whole(Some(&set)) else {
            panic!("a dict")
        };
        let ids: Vec<_> = rows.iter().map(|(_, k)| k.clone()).collect();
        assert_eq!(ids, vec![PyValue::Int(7_001), PyValue::Int(7_002)]);
        let list = PyValue::List(vec![PyValue::Int(7_003)]);
        let PyValue::Dict(rows) = layer_damage_whole(Some(&list)) else {
            panic!("a dict")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(layer_damage_whole(None), PyValue::Dict(vec![]));
    }

    /// #205: a module fitted from the active ship's own cargo hold, through
    /// `fit_one`. It is in the ship but not fitted (the client's rule,
    /// `fittingSlotController.py:93` [V]), so it meets the skill gate and
    /// `on_fitted` exactly as one from the hangar does.
    mod from_cargo {
        use super::super::fit_one;
        use crate::content::Content;
        use crate::item::Item;
        use crate::model::{Character, SandboxConfig};
        use crate::service::{Registry, ServiceError};

        const ALICE: i64 = 9_505_001;
        const STATION: i64 = 9_505_002;
        const HULL: i64 = 9_505_011;
        /// Needs Small Hybrid Turret 3; the starter sheet has less.
        const LOCKED_MOD: i64 = 9_505_021;
        const FREE_MOD: i64 = 9_505_022;
        /// No skill, but a CPU no hull has.
        const HUNGRY_MOD: i64 = 9_505_023;

        fn content() -> Content {
            let module = |id: i64, cpu: &str, extra: &str| {
                format!(
                    "type\t{id}\t55\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t12,16\t50={cpu}{extra}\tFixture Module {id}\n"
                )
            };
            let rows = format!(
                "type\t{HULL}\t25\t6\t1\t1\t1000.0\t500.0\t1200000000.0\t300.0\t0\t\t\
                 9=1000000.0;11=45000.0;14=3;13=2;12=3;37=100.0;48=125000.0;552=30.0;\
                 600=2.0;1281=3.0\tFixture Hull {HULL}\n{}{}{}",
                module(LOCKED_MOD, "1.0", ";182=3301.0;277=3.0"),
                module(FREE_MOD, "1.0", ""),
                module(HUNGRY_MOD, "900000000.0", ""),
            );
            let text = format!("{}{rows}", crate::typedata::FIXTURE);
            let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
            let mut c = Content::fixture();
            c.insert("typedata", crate::typedata::Installed::build(table));
            c
        }

        /// Alice, docked, in the hull (ids[0]), with in its cargo hold: the
        /// locked module, the hungry one, a stack of three free ones and a
        /// single free one (ids[1..=4]).
        fn world() -> (Registry, Vec<i64>) {
            let alice = Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                station_id: Some(STATION),
                ..Character::default()
            };
            let mut reg = Registry::new(SandboxConfig {
                characters: vec![alice],
                ..SandboxConfig::default()
            })
            .with_content(std::sync::Arc::new(content()));
            let hull_id = 9_506_000;
            let mut items = reg.store().items().to_vec();
            let mut ids = Vec::new();
            for (n, (type_id, quantity, singleton)) in [
                (HULL, 1, true),
                (LOCKED_MOD, 1, true),
                (HUNGRY_MOD, 1, true),
                (FREE_MOD, 3, false),
                (FREE_MOD, 1, true),
            ]
            .into_iter()
            .enumerate()
            {
                let id = hull_id + n as i64;
                let t = crate::item::find_in(reg.content(), type_id).expect("a fixture type");
                let (location_id, flag_id) = if n == 0 {
                    (STATION, crate::model::FLAG_HANGAR)
                } else {
                    (hull_id, crate::fitting::FLAG_CARGO)
                };
                items.push(Item {
                    item_id: id,
                    type_id,
                    owner_id: ALICE,
                    location_id,
                    flag_id,
                    quantity,
                    group_id: t.group_id,
                    category_id: t.category_id,
                    singleton,
                });
                ids.push(id);
            }
            reg.apply_snapshot(crate::store::Snapshot {
                items,
                next_item_id: 9_507_000,
                ..crate::store::Snapshot::default()
            });
            let c = reg.character_mut(ALICE).expect("Alice");
            c.ship_id = ids[0];
            c.ship_type_id = HULL;
            (reg, ids)
        }

        fn fit(reg: &mut Registry, ids: &[i64], n: usize) -> Result<i64, ServiceError> {
            fit_one(
                reg,
                Some(ALICE),
                ids[0],
                ids[n],
                crate::fitting::FLAG_AUTOFIT,
            )
        }

        #[test]
        fn an_unskilled_module_from_cargo_is_refused_like_one_from_the_hangar() {
            let (mut reg, ids) = world();
            let r = fit(&mut reg, &ids, 1);
            assert!(r.as_ref().is_err_and(crate::skillgate::is_refusal), "{r:?}");
            assert_eq!(
                reg.store().get(ids[1]).map(|i| (i.location_id, i.flag_id)),
                Some((ids[0], crate::fitting::FLAG_CARGO)),
                "still in the cargo hold"
            );
        }

        #[test]
        fn a_module_from_cargo_over_the_cpu_goes_on_offline() {
            let (mut reg, ids) = world();
            let id = fit(&mut reg, &ids, 2).expect("fits, offline");
            assert!(
                reg.store()
                    .get(id)
                    .is_some_and(|i| crate::item::is_ship_fitting_flag(i.flag_id))
            );
            assert!(!reg.modules().is_online(id), "over the CPU: offline");
        }

        #[test]
        fn a_module_from_cargo_that_fits_the_budget_comes_up_online() {
            let (mut reg, ids) = world();
            let id = fit(&mut reg, &ids, 4).expect("fits");
            assert_eq!(id, ids[4], "a single module moves, keeping its id");
            assert!(reg.modules().is_online(id));
        }

        #[test]
        fn a_stack_in_cargo_leaves_the_rest_in_cargo() {
            let (mut reg, ids) = world();
            let id = fit(&mut reg, &ids, 3).expect("fits");
            assert_ne!(id, ids[3], "one unit assembled off the stack");
            let fitted = reg.store().get(id).expect("the fitted unit");
            assert!(fitted.singleton);
            assert_eq!(fitted.location_id, ids[0]);
            assert!(crate::item::is_ship_fitting_flag(fitted.flag_id));
            assert_eq!(
                reg.store()
                    .get(ids[3])
                    .map(|i| (i.quantity, i.location_id, i.flag_id)),
                Some((2, ids[0], crate::fitting::FLAG_CARGO))
            );
            assert!(reg.modules().is_online(id));
        }
    }
}
