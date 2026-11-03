//! Player verbs: one function per row of `docs/lanes/bots.md` §3.
//!
//! **The rule, and why every doc comment below cites a client call site.**
//! Every verb is the call the real client makes for the same act -- same
//! service, same route (service, moniker or bound object), same positionals,
//! same keywords in the same wire order -- and its verdict is the evidence the
//! client waits for: a session change, a push, a listing that changed. A bot
//! that sends a call no client sends would be testing a client that does not
//! exist, and a green run would prove nothing about the one that does.
//!
//! **Verdicts are evidence, and a refusal's key is preferred when there is
//! one.** Until `430373f` a raise on a bound route read as `None`
//! (`dispatch.rs::handle_bound_call` and `handle_bind` sent it to
//! `unhandled`), so a refused `Board` or `MultiAdd` looked like a quiet
//! success *by its reply*, and the verbs were written to judge by what
//! followed. Since `430373f` every route answers a raised `UserError` with an
//! `ErrorResponse`, so the key is readable everywhere and lands in the
//! [`Error::Refused`] text; the evidence check stays, because "answered" and
//! "done" are still two different things.
//!
//! Verdicts, in one place:
//!
//! - [`Error::NotSent`]: the client's own precondition failed, so nothing
//!   was sent (not docked, not in range, nothing bound);
//! - [`Error::Refused`]: sent, answered, not done;
//! - `Error::Io` / `Error::Protocol`: the connection is in trouble.

use crate::{
    Answer, CallOutcome, Conn, Error, ObjectRef, Result, bstr, notification, station_moniker,
};
use eve_marshal::{DbValue, PyValue};
use std::time::{Duration, Instant};

/// `const.containerHangar` **[V]** -- the station hangar's magic container id
/// (`service/inventory.rs`, the `GetSelfInvItem` arm).
pub const CONTAINER_HANGAR: i64 = 10_004;
/// `const.flagHangar` **[V]**.
pub const FLAG_HANGAR: i64 = 4;
/// `const.flagCargo` **[V]**.
pub const FLAG_CARGO: i64 = 5;
/// `const.flagAutoFit`: "Fit to Active Ship" lets the server pick the slot
/// (`fittingUtil.py:171`, `fitting.md` §2.1) **[V]**.
pub const FLAG_AUTOFIT: i64 = 0;
/// The client sends `CmdDock` only inside this surface distance, metres
/// (`autopilot.py:624`, `surfaceDist < 2500`) **[V]**.
pub const DOCK_RANGE: f64 = 2_500.0;
/// `appConst.minWarpDistance`, metres: below this the client will not warp
/// (`service.rs:2337`) **[V]**.
pub const MIN_WARP_DISTANCE: f64 = 150_000.0;
/// The mining laser's effect, by the name `Activate` takes and the id
/// `OnGodmaShipEffect` reports (`mining.md` §1.1) **[V]**.
pub const MINING_LASER: (&str, i64) = ("miningLaser", eve_server::modules::EFFECT_MINING_LASER);
/// `effectOnline` **[V]**.
pub const EFFECT_ONLINE: i64 = eve_server::modules::EFFECT_ONLINE;
/// `Activate`'s `repeat` for a mining laser: run until stopped
/// (`mining.md` §1.1) **[V]**.
pub const REPEAT_UNTIL_STOPPED: i64 = 1_000;
/// `maxCargoContainerTransferDistance`, metres: the client loots a wreck
/// only inside this surface distance (`appConst.py:1726`, `IsInRange`
/// `invControllers.py:1502-1522`) **[V]**.
pub const LOOT_RANGE: f64 = 2_500.0;
/// `const.attributeQuantity`: a loaded charge's round count (P2.3) **[V]**.
pub const ATTRIBUTE_QUANTITY: i64 = 805;
/// `const.categoryCharge` **[V]**.
pub const CATEGORY_CHARGE: i64 = 8;

/// What [`Conn::loot_all`] took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Looted {
    /// Units moved into the hold.
    pub units: i64,
    /// The types that were in the wreck, in listing order.
    pub types: Vec<i64>,
    /// The refusal's key when not everything fitted (`NotEnoughCargoSpace`).
    pub refusal: Option<String>,
}

/// A gun's effect, `(name, id)`, from the module table -- the client's
/// `dogmaeffects` row **[V-data]**: the bundle's with `content`, else the
/// invented fixture (FX11, [`crate::modules::weapon_effect`]). `None` for a
/// module with no offensive target effect: not a gun.
pub fn weapon_effect(
    content: Option<&eve_server::content::Content>,
    type_id: i64,
) -> Option<(&'static str, i64)> {
    crate::modules::weapon_effect(content, type_id)
}

/// **A fitted module's dogma row**, as `dogmaIM.ItemGetInfo(moduleID)`
/// answers it (`modules.rs::module_info`): the effects running on it and
/// its type's attributes as the server serves them (FX11). The default is
/// a row that said nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModuleRow {
    /// The keys of `activeEffects`.
    pub effects: Vec<i64>,
    row: Option<PyValue>,
}

impl ModuleRow {
    /// One attribute of the row, when it carries it.
    pub fn attribute(&self, attribute: i64) -> Option<f64> {
        self.row.as_ref().and_then(|r| attribute_of(r, attribute))
    }
}

/// Attribute 805 of an `ItemGetInfo` row: `attributes` is a dict keyed by
/// attribute id (`modules.rs::charge_info_in`).
pub(crate) fn quantity_of(row: &PyValue) -> Option<i64> {
    let attrs = crate::dict_get(crate::attributes(row), "attributes")?;
    let PyValue::Dict(pairs) = attrs else {
        return None;
    };
    pairs
        .iter()
        .find(|(_, k)| *k == PyValue::Int(ATTRIBUTE_QUANTITY))
        .and_then(|(v, _)| match v {
            PyValue::Int(n) => Some(*n),
            PyValue::Real(r) => Some(r.round() as i64),
            _ => None,
        })
}

/// One attribute of an `ItemGetInfo` row, as a number: the same
/// `attributes` dict [`quantity_of`] reads, keyed by attribute id.
pub(crate) fn attribute_of(row: &PyValue, attribute: i64) -> Option<f64> {
    let attrs = crate::dict_get(crate::attributes(row), "attributes")?;
    let PyValue::Dict(pairs) = attrs else {
        return None;
    };
    pairs
        .iter()
        .find(|(_, k)| *k == PyValue::Int(attribute))
        .and_then(|(v, _)| match v {
            PyValue::Int(n) => Some(*n as f64),
            PyValue::Real(r) => Some(*r),
            _ => None,
        })
}

/// `appConst.agentDialogueButton*`, the `actionData` an agent's button is
/// drawn from (`appConst.py:193-203`, `docs/re/agents-missions.md` step 4)
/// **[V]**. The `actionID` beside it is the server's to choose and the
/// client only echoes it, so a bot finds an action by its button, as the
/// window draws it (`agentDialogueWindow.py:415`).
pub const BUTTON_VIEW_MISSION: i64 = 1;
pub const BUTTON_REQUEST_MISSION: i64 = 2;
pub const BUTTON_ACCEPT: i64 = 3;
pub const BUTTON_COMPLETE: i64 = 6;
pub const BUTTON_DECLINE: i64 = 9;
pub const BUTTON_QUIT: i64 = 11;

/// One `DoAction` answer, `((agentSaysMsg, availableActions),
/// lastActionInfo)` with `agentSaysMsg = (msgInfo, contentID)`
/// (`agentDialogueWindow.py:485`, `missioncontroller.py:318`) **[V]**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurn {
    /// The mission the agent speaks of: `agentSaysMsg[1]`.
    pub content_id: Option<i64>,
    /// `(actionID, button)` per action drawn as a button (an `int`
    /// `actionData`); links and sub-briefings are left out.
    pub actions: Vec<(i64, i64)>,
    pub completed: bool,
    pub declined: bool,
    pub quit: bool,
}

impl AgentTurn {
    pub fn from_value(v: &PyValue) -> Option<AgentTurn> {
        let [head, info] = tuple(v)? else {
            return None;
        };
        let [says, actions] = tuple(head)? else {
            return None;
        };
        let [_, content] = tuple(says)? else {
            return None;
        };
        let actions = match actions {
            PyValue::List(a) | PyValue::Tuple(a) => a
                .iter()
                .filter_map(|pair| match tuple(pair)? {
                    [PyValue::Int(id), PyValue::Int(button)] => Some((*id, *button)),
                    _ => None,
                })
                .collect(),
            _ => return None,
        };
        // Subscripted by the client, so present; 0 when not (`GetObjectiveHTML`).
        let flag = |k: &str| {
            matches!(crate::dict_get(info, k), Some(PyValue::Int(n)) if *n != 0)
                || matches!(crate::dict_get(info, k), Some(PyValue::Bool(true)))
        };
        Some(AgentTurn {
            content_id: match content {
                PyValue::Int(n) => Some(*n),
                _ => None,
            },
            actions,
            completed: flag("missionCompleted"),
            declined: flag("missionDeclined"),
            quit: flag("missionQuit"),
        })
    }

    /// The `actionID` paired with `button`, if the agent offers it.
    pub fn action_for(&self, button: i64) -> Option<i64> {
        self.actions
            .iter()
            .find(|(_, b)| *b == button)
            .map(|(id, _)| *id)
    }
}

/// A courier's `'transport'` objective, read from
/// `GetMissionObjectiveInfo()['objectives']`: `('transport', (ownerID,
/// pickupLocation, ownerID, dropoffLocation, cargo))`, each location a dict
/// of `locationID` and `solarsystemID`, the cargo `typeID` and `quantity`
/// (`agentDialogueUtil.py:190-248`; `mission.rs::objective_info_at`)
/// **[V]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CourierLeg {
    /// `contentID`: the mission.
    pub mission_id: i64,
    pub pickup_station: i64,
    pub pickup_system: i64,
    pub dropoff_station: i64,
    pub dropoff_system: i64,
    pub cargo_type: i64,
    pub quantity: i64,
}

impl CourierLeg {
    /// `None` when the document holds no transport objective: not accepted
    /// yet, or not a courier.
    pub fn from_objective(info: &PyValue) -> Option<CourierLeg> {
        let int = |v: &PyValue, k: &str| match crate::dict_get(v, k) {
            Some(PyValue::Int(n)) => Some(*n),
            _ => None,
        };
        let PyValue::List(objectives) = crate::dict_get(info, "objectives")? else {
            return None;
        };
        objectives.iter().find_map(|o| {
            let [kind, body] = tuple(o)? else {
                return None;
            };
            if crate::text(kind).as_deref() != Some("transport") {
                return None;
            }
            let [_, pickup, _, dropoff, cargo] = tuple(body)? else {
                return None;
            };
            Some(CourierLeg {
                mission_id: int(info, "contentID")?,
                pickup_station: int(pickup, "locationID")?,
                pickup_system: int(pickup, "solarsystemID")?,
                dropoff_station: int(dropoff, "locationID")?,
                dropoff_system: int(dropoff, "solarsystemID")?,
                cargo_type: int(cargo, "typeID")?,
                quantity: int(cargo, "quantity")?,
            })
        })
    }
}

/// A tuple's (or a list's) elements.
fn tuple(v: &PyValue) -> Option<&[PyValue]> {
    match v {
        PyValue::Tuple(items) | PyValue::List(items) => Some(items),
        _ => None,
    }
}

/// How long a verb waits for the evidence that follows its answer. The
/// server queues a call's pushes to drain right behind its reply, in the same
/// flush (`Reply::pushes`), so on loopback this is generous by orders of
/// magnitude; it is finite so that "no evidence" is a verdict and not a hang.
const EVIDENCE: Duration = Duration::from_millis(2_500);

/// One row of the select screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterSlot {
    pub character_id: i64,
    pub name: String,
    pub ship_type_id: Option<i64>,
    pub station_id: Option<i64>,
    pub solar_system_id: Option<i64>,
}

impl CharacterSlot {
    /// One `KeyVal` of `GetCharacterSelectionData`'s third element
    /// (`service.rs::Character::to_row`).
    pub(crate) fn from_row(row: &PyValue) -> Option<CharacterSlot> {
        let attrs = crate::attributes(row);
        let int = |k: &str| match crate::dict_get(attrs, k) {
            Some(PyValue::Int(n)) => Some(*n),
            _ => None,
        };
        Some(CharacterSlot {
            character_id: int("characterID")?,
            name: crate::dict_get(attrs, "characterName").and_then(crate::text)?,
            ship_type_id: int("shipTypeID"),
            station_id: int("stationID"),
            solar_system_id: int("solarSystemID"),
        })
    }
}

/// One item row, as an inventory's `List` returns it: a `blue.DBRow` over
/// `item_columns()` (`service.rs`), read **by column name** so a reordered
/// descriptor cannot shift a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemRow {
    pub item_id: i64,
    pub type_id: i64,
    pub owner_id: i64,
    pub location_id: i64,
    pub flag_id: i64,
    pub quantity: i64,
    pub group_id: i64,
    pub category_id: i64,
    pub singleton: bool,
}

impl ItemRow {
    pub fn from_value(v: &PyValue) -> Option<ItemRow> {
        let PyValue::DbRow(row) = v else {
            return None;
        };
        let get = |name: &str| {
            row.descriptor()
                .columns()
                .iter()
                .position(|c| c.name == name.as_bytes())
                .and_then(|i| match row.values().get(i) {
                    Some(DbValue::Int(n)) => Some(*n),
                    Some(DbValue::Bool(b)) => Some(i64::from(*b)),
                    _ => None,
                })
        };
        Some(ItemRow {
            item_id: get("itemID")?,
            type_id: get("typeID")?,
            owner_id: get("ownerID")?,
            location_id: get("locationID")?,
            flag_id: get("flagID")?,
            quantity: get("quantity")?,
            group_id: get("groupID").unwrap_or(0),
            category_id: get("categoryID").unwrap_or(0),
            singleton: get("singleton").is_some_and(|s| s != 0),
        })
    }

    /// In a module or rig slot, i.e. fitted (`item::is_ship_fitting_flag`).
    pub fn is_fitted(&self) -> bool {
        eve_server::item::is_ship_fitting_flag(self.flag_id)
    }
}

/// One `OnGodmaShipEffect`: `(itemID, effectID, t, start, active,
/// environment, startTime, duration, repeat, error, actualStopTime)`
/// (`godma:1722`, `modules.rs::ShipEffect`) **[V]**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GodmaEffect {
    pub item_id: i64,
    pub effect_id: i64,
    pub start: bool,
    pub active: bool,
    /// `environment[3]`, the target.
    pub target: Option<i64>,
    /// `error[0]`, the dialog key a stop came with -- `MiningItemDepleted`,
    /// `MiningDronesDeactivatedCargoHoldFull`, `TargetTooFar`
    /// (`mining.md` §1.4-1.5) **[V]**.
    pub error: Option<String>,
}

impl GodmaEffect {
    /// From a push's `(event, args)`, or `None` for any other push.
    pub fn from_push(event: &str, args: &[PyValue]) -> Option<GodmaEffect> {
        if event != "OnGodmaShipEffect" {
            return None;
        }
        let int = |i: usize| match args.get(i) {
            Some(PyValue::Int(n)) => Some(*n),
            Some(PyValue::Bool(b)) => Some(i64::from(*b)),
            _ => None,
        };
        let target = match args.get(5) {
            Some(PyValue::List(env) | PyValue::Tuple(env)) => match env.get(3) {
                Some(PyValue::Int(t)) => Some(*t),
                _ => None,
            },
            _ => None,
        };
        let error = match args.get(9) {
            Some(PyValue::Tuple(e) | PyValue::List(e)) => e.first().and_then(crate::text),
            _ => None,
        };
        Some(GodmaEffect {
            item_id: int(0)?,
            effect_id: int(1)?,
            start: int(3)? != 0,
            active: int(4).unwrap_or(0) != 0,
            target,
            error,
        })
    }
}

/// `OnFleetInvite(fleetID, inviterID, msgName, msgDict)` (`fleet.rs`,
/// `fleetSvc.py:1326`) **[V]**. `inviter_id` is a character id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FleetInvite {
    pub fleet_id: i64,
    pub inviter_id: i64,
}

impl FleetInvite {
    pub fn from_push(event: &str, args: &[PyValue]) -> Option<FleetInvite> {
        if event != "OnFleetInvite" {
            return None;
        }
        match (args.first(), args.get(1)) {
            (Some(PyValue::Int(fleet_id)), Some(PyValue::Int(inviter_id))) => Some(FleetInvite {
                fleet_id: *fleet_id,
                inviter_id: *inviter_id,
            }),
            _ => None,
        }
    }
}

/// What `AddTarget` said: `(0, [])` locked now, `(1, [])` pending until an
/// `OnTarget('add', ...)` push (`lib.rs::Conn::lock`, `targeting.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    Locked,
    Pending,
}

impl Conn {
    fn docked_at(&self, what: &str) -> Result<i64> {
        self.session()
            .station_id()
            .ok_or_else(|| Error::NotSent(format!("{what}: not docked")))
    }

    fn active_ship(&self, what: &str) -> Result<i64> {
        self.session()
            .ship_id()
            .ok_or_else(|| Error::NotSent(format!("{what}: the session names no ship")))
    }

    /// `eveMoniker.GetInventoryMgr()` / the `dogmaIM` location moniker:
    /// `(stationID, 15)` docked, `(solarsystemID, 5)` in space.
    fn location_moniker(&self, what: &str) -> Result<PyValue> {
        self.session()
            .location_moniker()
            .ok_or_else(|| Error::NotSent(format!("{what}: the session has no location")))
    }

    /// Read until a push satisfying `want` arrives or `within` elapses,
    /// keeping every notification seen in `sink`. `None` is "it did not
    /// come", a verdict for the caller.
    pub(crate) fn await_push(
        &mut self,
        within: Duration,
        sink: &mut Vec<PyValue>,
        want: impl Fn(&str, &[PyValue]) -> bool,
    ) -> Result<Option<(String, Vec<PyValue>)>> {
        let restore = self.stream.read_timeout()?;
        let deadline = Instant::now() + within;
        let verdict = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break Ok(None);
            }
            self.stream.set_read_timeout(Some(left))?;
            match self.recv() {
                Ok(frame) => {
                    if let Some((name, args)) = notification(&frame) {
                        sink.push(frame);
                        if want(&name, &args) {
                            break Ok(Some((name, args)));
                        }
                    }
                }
                Err(Error::Io(e)) if crate::is_timeout(&e) => break Ok(None),
                Err(e) => break Err(e),
            }
        };
        self.stream.set_read_timeout(restore)?;
        verdict
    }

    // -- inventory --------------------------------------------------------

    /// **Open the station hangar**: `invbroker` bound on `(stationID, 15)`,
    /// `GetInventory(containerHangar, None)`, whose answer is itself an
    /// object (`dispatch.rs:1260-1270`; `docs/reference/bound-objects.md` §4, the
    /// capture `invbroker::MachoBindObject((60000004, 15),
    /// ('GetInventoryFromId', (90000001, 1), {}))` answering two references)
    /// **[V]**.
    pub fn open_hangar(&mut self) -> Result<ObjectRef> {
        let station = self.docked_at("open_hangar")?;
        let answer = self.moniker_call(
            "invbroker",
            station_moniker(station),
            "GetInventory",
            vec![PyValue::Int(CONTAINER_HANGAR), PyValue::None],
            vec![],
        )?;
        first_object("GetInventory", answer)
    }

    /// **Open a ship's inventory**: `GetInventoryFromId(shipID, 1)` on the
    /// location's `invbroker` -- the capture above, and `PrimeLocation`'s
    /// `invCache.GetInventoryFromId(itemID, 1)` (`service.rs`
    /// `OBJECT_RETURNING`) **[V]**.
    pub fn open_ship(&mut self, ship_id: i64) -> Result<ObjectRef> {
        let params = self.location_moniker("open_ship")?;
        let answer = self.moniker_call(
            "invbroker",
            params,
            "GetInventoryFromId",
            vec![PyValue::Int(ship_id), PyValue::Int(1)],
            vec![],
        )?;
        first_object("GetInventoryFromId", answer)
    }

    /// **List an inventory**, `List(flag=flag)`: `invCacheContainer.List`
    /// always sends the keyword, `None` for "everything" (`invCache.py:1101`,
    /// quoted at `service/inventory.rs`'s `List` arm) **[V]**.
    pub fn list(&mut self, inventory: &ObjectRef, flag: Option<i64>) -> Result<Vec<ItemRow>> {
        let flag = flag.map_or(PyValue::None, PyValue::Int);
        let outcome = self.call_on(inventory, "List", vec![], vec![(flag, bstr("flag"))])?;
        match outcome.value() {
            Some(PyValue::List(rows)) => Ok(rows.iter().filter_map(ItemRow::from_value).collect()),
            other => Err(Error::Refused(format!(
                "List answered {other:?} (refusal: {:?})",
                outcome.refusal()
            ))),
        }
    }

    /// Everything in the station hangar.
    pub fn hangar_items(&mut self) -> Result<Vec<ItemRow>> {
        let hangar = self.open_hangar()?;
        self.list(&hangar, None)
    }

    /// A ship's contents, optionally one flag of them.
    pub fn ship_items(&mut self, ship_id: i64, flag: Option<i64>) -> Result<Vec<ItemRow>> {
        let ship = self.open_ship(ship_id)?;
        self.list(&ship, flag)
    }

    /// **List cargo**: the active ship's hold, `List(flag=flagCargo)` --
    /// the read the client's auto-reload makes (`cdl:1182`, P5.12 note in
    /// `docs/lanes/inventory.md`) **[V]**.
    pub fn cargo(&mut self) -> Result<Vec<ItemRow>> {
        let ship = self.active_ship("cargo")?;
        self.ship_items(ship, Some(FLAG_CARGO))
    }

    /// `ship`'s cargo hold and mining hold (`flagGeneralMiningHold` 134
    /// [V], GAPS-B): what [`Self::unload`] may move.
    pub fn holds(&mut self, ship: i64) -> Result<Vec<ItemRow>> {
        Ok(self
            .ship_items(ship, None)?
            .into_iter()
            .filter(|r| r.flag_id == FLAG_CARGO || r.flag_id == crate::brain::FLAG_MINING_HOLD)
            .collect())
    }

    /// The active ship's fitted modules.
    pub fn fitted(&mut self) -> Result<Vec<ItemRow>> {
        let ship = self.active_ship("fitted")?;
        Ok(self
            .ship_items(ship, None)?
            .into_iter()
            .filter(ItemRow::is_fitted)
            .collect())
    }

    // -- ships and fitting -------------------------------------------------

    /// **Board a ship in the hangar**: `ship.Board(shipID, oldShipID)` on the
    /// station's `ship` moniker `(stationID, 15)`, inside
    /// `PerformSelectiveSessionChange('board', ...)` (`cdl:303`,
    /// `fitting.md` §0.1, §1.2) **[V]**. `oldShipID` is `session.shipid`.
    ///
    /// Evidence: the `{'shipid': (old, new)}` session change. Boarding the
    /// ship already flown answers with no change, which is success.
    pub fn board(&mut self, ship_id: i64) -> Result<CallOutcome> {
        let station = self.docked_at("board")?;
        let old = self.active_ship("board")?;
        let mut outcome = self
            .moniker_call(
                "ship",
                station_moniker(station),
                "Board",
                vec![PyValue::Int(ship_id), PyValue::Int(old)],
                vec![],
            )?
            .outcome;
        if ship_id == old {
            return Ok(outcome);
        }
        self.await_session_change(&mut outcome, EVIDENCE)?;
        if outcome
            .session_change
            .as_ref()
            .and_then(|c| c.new_i64("shipid"))
            == Some(ship_id)
        {
            return Ok(outcome);
        }
        Err(Error::Refused(format!(
            "Board({ship_id}): no shipid session change ({})",
            refusal_or_silence(&outcome)
        )))
    }

    /// **Eject** (BE-3): `ship.Eject()` with no arguments on the solar
    /// system's `ship` moniker `(solarsystemID, groupSolarSystem)`, inside
    /// `PerformSessionChange('eject', ...)` (`menuFunctions.Eject`, reply
    /// discarded) **[V]**. The client's menu offers it only in space and not
    /// warping (`OfferEjectFromShip`) **[V]**; the bot keeps those as
    /// [`Error::NotSent`], and the server answers the rest (capsule, weapons
    /// timer, crime) with a key.
    ///
    /// Evidence: the `{'shipid': (old, capsule)}` session change. Returns the
    /// abandoned hull's id (the old `shipid`), which the pilot can board
    /// again with [`Self::board_space`].
    pub fn eject(&mut self) -> Result<i64> {
        if self.session().station_id().is_some() {
            return Err(Error::NotSent("eject: docked".to_owned()));
        }
        let old = self.active_ship("eject")?;
        if self
            .world_now()
            .ego_ball()
            .is_some_and(|b| b.motion.warp_active())
        {
            return Err(Error::NotSent("eject: warping".to_owned()));
        }
        let params = self.location_moniker("eject")?;
        let mut outcome = self
            .moniker_call("ship", params, "Eject", vec![], vec![])?
            .outcome;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!("Eject: {key}")));
        }
        self.await_session_change(&mut outcome, EVIDENCE)?;
        let new = outcome
            .session_change
            .as_ref()
            .and_then(|c| c.new_i64("shipid"));
        match new {
            Some(new) if new != old => Ok(old),
            _ => Err(Error::Refused(format!(
                "Eject: no shipid session change ({})",
                refusal_or_silence(&outcome)
            ))),
        }
    }

    /// **Board a ship in space** (BE-3): `ship.Board(shipID, oldShipID)` on
    /// the solar system's `ship` moniker, inside
    /// `PerformSelectiveSessionChange('board', ...)` **[V]**, sent only as the
    /// client's `OfferBoardShip` would offer it: the ball is in the park, has
    /// no pilot (its slim has no `charID`; the client reads
    /// `not ball.isInteractive`, which the server clears together), is not
    /// warping, and is inside [`eve_server::pilotless::BOARD_RANGE`] of the
    /// ego's surface **[V]**. Otherwise [`Error::NotSent`]: no client would
    /// ask. The server then judges ownership, locks, crime and skills.
    ///
    /// Evidence: the `{'shipid': (old, new)}` session change.
    pub fn board_space(&mut self, ship_id: i64) -> Result<CallOutcome> {
        if self.session().station_id().is_some() {
            return Err(Error::NotSent("board_space: docked".to_owned()));
        }
        let old = self.active_ship("board_space")?;
        let world = self.world_now();
        let Some(ego) = world.ego else {
            return Err(Error::NotSent(
                "board_space: no park, so no distance".to_owned(),
            ));
        };
        let Some(ball) = world.get(ship_id) else {
            return Err(Error::NotSent(format!(
                "board_space: ship {ship_id} is not in the park"
            )));
        };
        if ball.slim.is_some_and(|s| s.char_id.is_some()) {
            return Err(Error::NotSent(format!(
                "board_space: ship {ship_id} has a pilot"
            )));
        }
        if ball.motion.warp_active() {
            return Err(Error::NotSent(format!(
                "board_space: ship {ship_id} is warping"
            )));
        }
        let surface = world.surface_distance(ego, ship_id).unwrap_or(f64::MAX);
        let reach = eve_server::pilotless::BOARD_RANGE;
        if surface > reach {
            return Err(Error::NotSent(format!(
                "board_space: {surface:.0} m from the ship's surface; the client boards \
                 inside {reach} m"
            )));
        }
        let params = self.location_moniker("board_space")?;
        let mut outcome = self
            .moniker_call(
                "ship",
                params,
                "Board",
                vec![PyValue::Int(ship_id), PyValue::Int(old)],
                vec![],
            )?
            .outcome;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!("Board({ship_id}): {key}")));
        }
        self.await_session_change(&mut outcome, EVIDENCE)?;
        if outcome
            .session_change
            .as_ref()
            .and_then(|c| c.new_i64("shipid"))
            == Some(ship_id)
        {
            return Ok(outcome);
        }
        Err(Error::Refused(format!(
            "Board({ship_id}): no shipid session change ({})",
            refusal_or_silence(&outcome)
        )))
    }

    /// **"Fit to Active Ship"**: the active ship's inventory
    /// `MultiAdd([itemIDs], sourceLocationID, flag=flagAutoFit)`, where the
    /// source is `invItems[0].locationID` -- the station, for hangar items
    /// (`fittingUtil.py:171`, `fitting.md` §0.2, §2.1) **[V]**. The server
    /// picks the slot, so the bot needs no slot table.
    ///
    /// Evidence: the modules that are fitted now and were not before, read
    /// back with `List` -- **new ids**, because a module fitted off a stack
    /// is a new item (`Edit::Assemble`, `fitting.md` §0.7). Returns them.
    pub fn fit_auto(&mut self, item_ids: &[i64]) -> Result<Vec<ItemRow>> {
        let station = self.docked_at("fit_auto")?;
        let ship = self.active_ship("fit_auto")?;
        let before: Vec<i64> = self.fitted()?.iter().map(|r| r.item_id).collect();
        let inventory = self.open_ship(ship)?;
        let outcome = self.call_on(
            &inventory,
            "MultiAdd",
            vec![
                PyValue::List(item_ids.iter().copied().map(PyValue::Int).collect()),
                PyValue::Int(station),
            ],
            vec![(PyValue::Int(FLAG_AUTOFIT), bstr("flag"))],
        )?;
        let new: Vec<ItemRow> = self
            .fitted()?
            .into_iter()
            .filter(|r| !before.contains(&r.item_id))
            .collect();
        if new.is_empty() {
            return Err(Error::Refused(format!(
                "MultiAdd({item_ids:?}, flag=0): nothing new is fitted ({})",
                refusal_or_silence(&outcome)
            )));
        }
        Ok(new)
    }

    /// **Is this module online?** `dogmaIM.ItemGetInfo(itemID)` on the
    /// location's `dogmaIM` moniker, and `effectOnline` among the row's
    /// `activeEffects` -- the read godma makes for a module fitted after the
    /// prime (`godma:1949`, `fitting.md` §0.5; `modules.rs::module_info`)
    /// **[V]**. This is how the client learns that the server already
    /// onlined a module on fitting (`modules::on_fitted`), which sends no
    /// push.
    pub fn is_online(&mut self, module_id: i64) -> Result<bool> {
        Ok(self.active_effects(module_id)?.contains(&EFFECT_ONLINE))
    }

    /// **The effects running on a module**: the keys of `ItemGetInfo`'s
    /// `activeEffects` -- `effectOnline` while it is online, and its own
    /// effect (67 for a mining laser) while a cycle runs and no stop is
    /// pending (`modules.rs::module_info`). The read godma's prime makes, so
    /// a bot that reconnects learns its laser is still running as a client
    /// would (`docs/lanes/bots.md`, the P8b note).
    pub fn active_effects(&mut self, module_id: i64) -> Result<Vec<i64>> {
        Ok(self.module_row(module_id)?.effects)
    }

    /// **A module's whole dogma row** (FX11): the same `ItemGetInfo` read
    /// as [`Self::active_effects`], keeping the row's `attributes` too --
    /// the module's numbers as the server serves them to the client.
    pub fn module_row(&mut self, module_id: i64) -> Result<ModuleRow> {
        let params = self.location_moniker("active_effects")?;
        let answer = self.moniker_call(
            "dogmaIM",
            params,
            "ItemGetInfo",
            vec![PyValue::Int(module_id)],
            vec![],
        )?;
        let Some(row) = answer.value else {
            return Err(Error::Refused(format!(
                "ItemGetInfo({module_id}) ({})",
                refusal_or_silence(&answer.outcome)
            )));
        };
        let effects = match crate::dict_get(crate::attributes(&row), "activeEffects") {
            Some(PyValue::Dict(effects)) => effects
                .iter()
                .filter_map(|(_, k)| match k {
                    PyValue::Int(id) => Some(*id),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        Ok(ModuleRow {
            effects,
            row: Some(row),
        })
    }

    /// **One of the ship's own attributes, as the server serves it**:
    /// `dogmaIM.ItemGetInfo(shipID)` on the location's `dogmaIM` moniker,
    /// the row godma primes the ship from (`service.rs::capsule_dogma_row`:
    /// fitted, with the pod's targeting floor) **[V]**. `None` when the row
    /// has no such attribute (FX10: the lock range, attribute 76).
    pub fn ship_attribute(&mut self, ship_id: i64, attribute: i64) -> Result<Option<f64>> {
        let params = self.location_moniker("ship_attribute")?;
        let answer = self.moniker_call(
            "dogmaIM",
            params,
            "ItemGetInfo",
            vec![PyValue::Int(ship_id)],
            vec![],
        )?;
        let Some(row) = answer.value else {
            return Err(Error::Refused(format!(
                "ItemGetInfo({ship_id}) ({})",
                refusal_or_silence(&answer.outcome)
            )));
        };
        Ok(attribute_of(&row, attribute))
    }

    /// **Put a module online**: `dogmaIM.SetModuleOnline(shipID, moduleID)`
    /// on the location's `dogmaIM` moniker -- what the client sends itself
    /// from `cdl._OnlineModuleIfApplicable` when a module lands in a slot
    /// (`cdl:705,751,768,855`, `fitting.md` §0.3) **[V]**.
    ///
    /// **`EffectAlreadyActive2` is success**: the server onlines a module that
    /// fits the budget on fitting, and the client swallows that refusal
    /// (`clientDogmaLocation.py:857`) **[V]**. So a refusal is not the
    /// verdict: the state read back with [`Self::is_online`] is.
    pub fn set_online(&mut self, module_id: i64) -> Result<()> {
        let ship = self.active_ship("set_online")?;
        let params = self.location_moniker("set_online")?;
        // A refusal is not the verdict -- `EffectAlreadyActive2` is success
        // -- so it is kept only to say why, if the module is still offline.
        let said = match self.moniker_call(
            "dogmaIM",
            params,
            "SetModuleOnline",
            vec![PyValue::Int(ship), PyValue::Int(module_id)],
            vec![],
        ) {
            Ok(answer) => refusal_or_silence(&answer.outcome),
            Err(Error::Refused(why)) => why,
            Err(e) => return Err(e),
        };
        if self.is_online(module_id)? {
            return Ok(());
        }
        Err(Error::Refused(format!(
            "SetModuleOnline({ship}, {module_id}): still offline ({said})"
        )))
    }

    // -- in space ----------------------------------------------------------

    /// **Lock a target, the client's way**: `dogmaIM.AddTarget(targetID)` on
    /// the location's `dogmaIM` moniker. [`Conn::lock`] is the older service
    /// call, kept for its tests; this is the route the client takes.
    pub fn lock_target(&mut self, target: i64) -> Result<LockState> {
        let params = self.location_moniker("lock_target")?;
        let answer = self.moniker_call(
            "dogmaIM",
            params,
            "AddTarget",
            vec![PyValue::Int(target)],
            vec![],
        )?;
        match &answer.value {
            Some(PyValue::Tuple(t)) => match t.first() {
                Some(PyValue::Int(0)) | Some(PyValue::Bool(false)) => Ok(LockState::Locked),
                Some(_) => Ok(LockState::Pending),
                None => Err(Error::Refused(format!("AddTarget({target}): {t:?}"))),
            },
            other => Err(Error::Refused(format!(
                "AddTarget({target}) answered {other:?} ({})",
                refusal_or_silence(&answer.outcome)
            ))),
        }
    }

    /// **Start a module**: `dogmaIM.Activate(itemID, effectName, target,
    /// repeat)`, four positional, on the `dogmaIM` moniker (`godma:2427`,
    /// `service/inventory.rs`'s `Activate` arm) **[V]**. The reply is
    /// discarded by the client (`smb:1580`).
    ///
    /// Evidence: `OnGodmaShipEffect` for this module and effect with
    /// `start=1`. A stop for it instead is refused with its `error[0]` key.
    pub fn activate(
        &mut self,
        module_id: i64,
        effect: (&str, i64),
        target: Option<i64>,
        repeat: i64,
    ) -> Result<GodmaEffect> {
        let params = self.location_moniker("activate")?;
        let (name, effect_id) = effect;
        let mut outcome = self
            .moniker_call(
                "dogmaIM",
                params,
                "Activate",
                vec![
                    PyValue::Int(module_id),
                    bstr(name),
                    target.map_or(PyValue::None, PyValue::Int),
                    PyValue::Int(repeat),
                ],
                vec![],
            )?
            .outcome;
        // A refusal is the answer: no push will follow it, so do not wait
        // for one.
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!(
                "Activate({module_id}, {name:?}): {key}"
            )));
        }
        let ours = |e: &GodmaEffect| e.item_id == module_id && e.effect_id == effect_id;
        // Already in the reply's notifications?
        let mut seen = outcome
            .notifications
            .iter()
            .filter_map(notification)
            .find_map(|(n, a)| GodmaEffect::from_push(&n, &a).filter(ours));
        if seen.is_none() {
            seen = self
                .await_push(EVIDENCE, &mut outcome.notifications, |n, a| {
                    GodmaEffect::from_push(n, a).is_some_and(|e| ours(&e))
                })?
                .and_then(|(n, a)| GodmaEffect::from_push(&n, &a));
        }
        match seen {
            Some(e) if e.start => Ok(e),
            Some(e) => Err(Error::Refused(format!(
                "Activate({module_id}, {name:?}): stopped at once ({:?})",
                e.error
            ))),
            None => Err(Error::Refused(format!(
                "Activate({module_id}, {name:?}): no OnGodmaShipEffect ({})",
                refusal_or_silence(&outcome)
            ))),
        }
    }

    /// **Mine a rock**: [`Self::activate`] with `'miningLaser'` and
    /// `repeat=1000` (`mining.md` §1.1) **[V]**.
    pub fn mine(&mut self, laser_id: i64, rock_id: i64) -> Result<GodmaEffect> {
        self.activate(laser_id, MINING_LASER, Some(rock_id), REPEAT_UNTIL_STOPPED)
    }

    /// **Stop a module**: `dogmaIM.Deactivate(itemID, effectName)`, two
    /// positional (`godma:2471`) **[V]**. The stop's push is the client's
    /// evidence and is left in the outcome for the caller.
    pub fn deactivate(&mut self, module_id: i64, effect_name: &str) -> Result<CallOutcome> {
        let params = self.location_moniker("deactivate")?;
        Ok(self
            .moniker_call(
                "dogmaIM",
                params,
                "Deactivate",
                vec![PyValue::Int(module_id), bstr(effect_name)],
                vec![],
            )?
            .outcome)
    }

    /// **Warp to a fleet member**: bound `CmdWarpToStuff('char', charID,
    /// minRange=R)` -- `menusvc.WarpToMember:3265` (`docs/re/warp.md` §4) **[V]**.
    /// The id is a **character** id, not a ship id
    /// (`warping_to_a_fleet_member_takes_a_character_id`).
    ///
    /// Evidence: a `WarpTo` for this ship in a `DoDestinyUpdate`. The server
    /// answers `None` and pushes nothing for a target it cannot resolve
    /// (`service/space.rs::warp`), so silence is a refusal here.
    pub fn warp_to_member(&mut self, character_id: i64, min_range: f64) -> Result<CallOutcome> {
        let ship = self.active_ship("warp_to_member")?;
        let mut outcome = self.bound_call_kw(
            "CmdWarpToStuff",
            vec![bstr("char"), PyValue::Int(character_id)],
            vec![(PyValue::Real(min_range), bstr("minRange"))],
        )?;
        let warps = |name: &str, args: &[PyValue]| {
            name == "DoDestinyUpdate"
                && crate::read_action_list(args)
                    .iter()
                    .any(|(_, f, ops)| f == "WarpTo" && ops.first() == Some(&PyValue::Int(ship)))
        };
        let already = outcome
            .notifications
            .iter()
            .filter_map(notification)
            .any(|(n, a)| warps(&n, &a));
        if already
            || self
                .await_push(EVIDENCE, &mut outcome.notifications, warps)?
                .is_some()
        {
            return Ok(outcome);
        }
        Err(Error::Refused(format!(
            "CmdWarpToStuff('char', {character_id}): no WarpTo for this ship"
        )))
    }

    /// **Stop the ship**: bound `CmdStop()` (`space_state::COMMANDS`).
    pub fn stop(&mut self) -> Result<CallOutcome> {
        self.bound_call("CmdStop", vec![])
    }

    /// **Resync**: bound `UpdateStateRequest()`, the client's desync
    /// recovery (`michelle.py:1072`; `service/space.rs`), answered `None` with
    /// a fresh `SetState` pushed behind it. Returns once that state has been
    /// applied to [`Conn::world`].
    pub fn request_state(&mut self) -> Result<CallOutcome> {
        let states = self.world().states;
        let mut outcome = self.bound_call("UpdateStateRequest", vec![])?;
        if self.world().states > states {
            return Ok(outcome);
        }
        let deadline = Instant::now() + EVIDENCE;
        while self.world().states == states {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Refused(
                    "UpdateStateRequest: no SetState followed".to_owned(),
                ));
            }
            // Stop at the first readable SetState, not at quiet: on a busy grid
            // frames never stop and an idle wait lasts the whole fight (HANG-1).
            let more =
                self.collect_pushes_until(left.min(Duration::from_millis(250)), |frame| {
                    crate::notification(frame).is_some_and(|(event, args)| {
                        event == "DoDestinyUpdate" && crate::read_set_state(&args).is_some()
                    })
                })?;
            outcome.notifications.extend(more);
        }
        Ok(outcome)
    }

    /// **Dock**: bound `CmdDock(stationID, shipID)` (`dock-up.md` §1,
    /// `movementFunctions.RealDock`) **[V]**, sent only inside
    /// [`DOCK_RANGE`] of the station's surface, as the client's own gate
    /// requires (`autopilot.py:624`) **[V]** -- outside it this returns
    /// [`Error::NotSent`], because no client would ask.
    ///
    /// Evidence: the `stationid` session change, which also releases the
    /// park.
    pub fn dock(&mut self, station_id: i64) -> Result<CallOutcome> {
        let ship = self.active_ship("dock")?;
        let world = self.world_now();
        let Some(ego) = world.ego else {
            return Err(Error::NotSent("dock: no park, so no distance".to_owned()));
        };
        let Some(surface) = world.surface_distance(ego, station_id) else {
            return Err(Error::NotSent(format!(
                "dock: station {station_id} is not in the park"
            )));
        };
        if surface >= DOCK_RANGE {
            return Err(Error::NotSent(format!(
                "dock: {surface:.0} m from the station's surface; the client docks inside \
                 {DOCK_RANGE} m"
            )));
        }
        let mut outcome = self.bound_call(
            "CmdDock",
            vec![PyValue::Int(station_id), PyValue::Int(ship)],
        )?;
        self.await_session_change(&mut outcome, EVIDENCE)?;
        if outcome
            .session_change
            .as_ref()
            .and_then(|c| c.new_i64("stationid"))
            .is_some()
        {
            return Ok(outcome);
        }
        Err(Error::Refused(format!(
            "CmdDock({station_id}): no stationid session change ({})",
            refusal_or_silence(&outcome)
        )))
    }

    /// **Jump through a stargate** (SOC-A3): bound
    /// `CmdStargateJump(stargateID, beaconID, shipID)`, `beaconID` being the
    /// **destination** gate from the gate's slim `jumps` row
    /// (`gateJumpSvc.JumpThroughGate:131`,
    /// `eve_server::jump::cmd_stargate_jump`) **[V]**. Sent only inside
    /// [`eve_server::jump::MAX_JUMP_DISTANCE`] of the gate's surface, the
    /// client's own gate (`menusvc.py:3541`) **[V]** -- outside it this is
    /// [`Error::NotSent`]. The answer is `None`, which the client discards
    /// (`PerformSessionChange`).
    ///
    /// Evidence: the `solarsystemid` session change, which comes
    /// [`eve_server::jump::JUMP_DELAY`] after the call and releases the park
    /// ([`crate::SessionChange::releases_park`]); the next system's park is
    /// the caller's to bind.
    pub fn stargate_jump(&mut self, gate: i64, beacon: i64) -> Result<CallOutcome> {
        let ship = self.active_ship("stargate_jump")?;
        let from = self.session().solar_system_id();
        let world = self.world_now();
        let Some(ego) = world.ego else {
            return Err(Error::NotSent(
                "stargate_jump: no park, so no distance".to_owned(),
            ));
        };
        let Some(surface) = world.surface_distance(ego, gate) else {
            return Err(Error::NotSent(format!(
                "stargate_jump: gate {gate} is not in the park"
            )));
        };
        let reach = eve_server::jump::MAX_JUMP_DISTANCE;
        if surface >= reach {
            return Err(Error::NotSent(format!(
                "stargate_jump: {surface:.0} m from the gate's surface; the client jumps \
                 inside {reach} m"
            )));
        }
        self.run_pre_jump();
        let mut outcome = self.bound_call(
            "CmdStargateJump",
            vec![PyValue::Int(gate), PyValue::Int(beacon), PyValue::Int(ship)],
        )?;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!(
                "CmdStargateJump({gate}, {beacon}): {key}"
            )));
        }
        self.await_session_change(&mut outcome, eve_server::jump::JUMP_DELAY + EVIDENCE * 2)?;
        let now = self.session().solar_system_id();
        if outcome.session_changed && now.is_some() && now != from {
            return Ok(outcome);
        }
        Err(Error::Refused(format!(
            "CmdStargateJump({gate}, {beacon}): no solarsystemid session change ({})",
            refusal_or_silence(&outcome)
        )))
    }

    /// **Move cargo to the hangar**: the hangar inventory's `Add(itemID,
    /// shipID, qty=n, flag=flagHangar)` -- a drag from the hold onto the
    /// item hangar (`invControllers.py:267`; `docs/lanes/inventory.md`, the
    /// P5.12 note) **[V]**. `qty=None` moves the whole stack, as the client
    /// passes it (`clientDogmaLocation.py:1335`) **[V]**.
    ///
    /// Evidence: the hold no longer holds that many of `item_id`.
    pub fn unload(&mut self, item_id: i64, quantity: Option<i64>) -> Result<CallOutcome> {
        let ship = self.active_ship("unload")?;
        self.docked_at("unload")?;
        let had = held(&self.holds(ship)?, item_id);
        if had == 0 {
            return Err(Error::NotSent(format!(
                "unload: {item_id} is not in the hold"
            )));
        }
        let hangar = self.open_hangar()?;
        let outcome = self.call_on(
            &hangar,
            "Add",
            vec![PyValue::Int(item_id), PyValue::Int(ship)],
            vec![
                (quantity.map_or(PyValue::None, PyValue::Int), bstr("qty")),
                (PyValue::Int(FLAG_HANGAR), bstr("flag")),
            ],
        )?;
        let left = held(&self.holds(ship)?, item_id);
        let want = had - quantity.unwrap_or(had).min(had);
        if left == want {
            return Ok(outcome);
        }
        Err(Error::Refused(format!(
            "Add({item_id}, {ship}, qty={quantity:?}, flag=4): the hold still has {left} \
             ({})",
            refusal_or_silence(&outcome)
        )))
    }

    /// **Move hangar items into the hold**: the active ship's inventory
    /// `Add(itemID, stationID, qty=n, flag=flagCargo)` -- a drag from the
    /// item hangar onto the cargo hold (`invControllers.py:267`, the
    /// ship-cargo controller's `_AddItem`; `docs/lanes/inventory.md`, the
    /// P5.12 note) **[V]**. `qty=None` moves the whole stack, as for
    /// [`Self::unload`].
    ///
    /// The hold side of [`Self::unload`], which P8a left inline in its test.
    /// Evidence: the hangar holds `quantity` fewer of `item_id` (all of it
    /// for `None`) -- the hold's row may carry a **new** id when a stack is
    /// split, so the source is the side that can be counted.
    pub fn load_cargo(&mut self, item_id: i64, quantity: Option<i64>) -> Result<CallOutcome> {
        let ship = self.active_ship("load_cargo")?;
        let station = self.docked_at("load_cargo")?;
        let had = held(&self.hangar_items()?, item_id);
        if had == 0 {
            return Err(Error::NotSent(format!(
                "load_cargo: {item_id} is not in the hangar"
            )));
        }
        let inventory = self.open_ship(ship)?;
        let outcome = self.call_on(
            &inventory,
            "Add",
            vec![PyValue::Int(item_id), PyValue::Int(station)],
            vec![
                (quantity.map_or(PyValue::None, PyValue::Int), bstr("qty")),
                (PyValue::Int(FLAG_CARGO), bstr("flag")),
            ],
        )?;
        let left = held(&self.hangar_items()?, item_id);
        let want = had - quantity.unwrap_or(had).min(had);
        if left == want {
            return Ok(outcome);
        }
        Err(Error::Refused(format!(
            "Add({item_id}, {station}, qty={quantity:?}, flag=5): the hangar still has {left} \
             ({})",
            refusal_or_silence(&outcome)
        )))
    }

    /// **Unlock a target**: `dogmaIM.RemoveTarget(targetID)` on the
    /// location's `dogmaIM` moniker, the target box's "Unlock"
    /// (`targetMgr.py:1613`, return discarded) **[V]**.
    ///
    /// Evidence: `OnTarget('lost', targetID, ...)`, which the server must
    /// send because the client closes the box only on it
    /// (`service/inventory.rs`'s `RemoveTarget` arm).
    pub fn unlock_target(&mut self, target: i64) -> Result<CallOutcome> {
        let params = self.location_moniker("unlock_target")?;
        let mut outcome = self
            .moniker_call(
                "dogmaIM",
                params,
                "RemoveTarget",
                vec![PyValue::Int(target)],
                vec![],
            )?
            .outcome;
        let lost = |name: &str, args: &[PyValue]| {
            name == "OnTarget"
                && args.first() == Some(&bstr("lost"))
                && args.get(1) == Some(&PyValue::Int(target))
        };
        let already = outcome
            .notifications
            .iter()
            .filter_map(notification)
            .any(|(n, a)| lost(&n, &a));
        if already
            || self
                .await_push(EVIDENCE, &mut outcome.notifications, lost)?
                .is_some()
        {
            return Ok(outcome);
        }
        Err(Error::Refused(format!(
            "RemoveTarget({target}): no OnTarget('lost') ({})",
            refusal_or_silence(&outcome)
        )))
    }

    /// **Accept a fleet invite**: `Moniker('fleetObjectHandler',
    /// fleetID).AcceptInvite(shipTypeID)`, one bind with one fused call
    /// (`fleetSvc.OnFleetInvite:1347`, `dispatch.rs::handle_bind`) **[V]**,
    /// inside a selective session change.
    ///
    /// Evidence: the `fleetid` session change naming this fleet.
    pub fn accept_fleet_invite(&mut self, fleet_id: i64, ship_type_id: i64) -> Result<Answer> {
        let mut answer = self.moniker_call(
            "fleetObjectHandler",
            PyValue::Int(fleet_id),
            "AcceptInvite",
            vec![PyValue::Int(ship_type_id)],
            vec![],
        )?;
        self.await_session_change(&mut answer.outcome, EVIDENCE)?;
        if self.session().fleet_id() == Some(fleet_id) {
            return Ok(answer);
        }
        Err(Error::Refused(format!(
            "AcceptInvite({ship_type_id}) on fleet {fleet_id}: no fleetid session change ({})",
            refusal_or_silence(&answer.outcome)
        )))
    }
    // -- combat (P8c) ------------------------------------------------------

    /// **Rounds loaded in a gun**: `dogmaIM.ItemGetInfo((shipID, flagID,
    /// chargeTypeID))` on the location's `dogmaIM` moniker -- the key godma
    /// holds for a loaded charge, a sublocation with no item id of its own,
    /// whose count is attribute 805 `quantity` (`godma:1949` passes whatever
    /// key it holds; `docs/lanes/modules.md`, P2.3) **[V]**. `None` -- the
    /// answer for a slot holding nothing of that type -- is zero rounds.
    pub fn loaded_rounds(
        &mut self,
        ship_id: i64,
        flag_id: i64,
        charge_type_id: i64,
    ) -> Result<i64> {
        let params = self.location_moniker("loaded_rounds")?;
        let key = PyValue::Tuple(vec![
            PyValue::Int(ship_id),
            PyValue::Int(flag_id),
            PyValue::Int(charge_type_id),
        ]);
        let answer = self.moniker_call("dogmaIM", params, "ItemGetInfo", vec![key], vec![])?;
        match answer.value {
            Some(PyValue::None) => Ok(0),
            Some(row) => Ok(quantity_of(&row).unwrap_or(0)),
            None => Err(Error::Refused(format!(
                "ItemGetInfo(({ship_id}, {flag_id}, {charge_type_id})) ({})",
                refusal_or_silence(&answer.outcome)
            ))),
        }
    }

    /// **Load ammo**: `dogmaIM.LoadAmmo(shipID, moduleID, chargeItemIDs,
    /// chargeLocationID)`, four positional, reply discarded -- the HUD
    /// button's drop and the ammo menu send **one** module id, "load to all"
    /// several (`cdl:1187` `LoadChargesToModule`, `cdl:1204`
    /// `LoadAmmoToModules`, `cdl:1201/1206`) **[V]**. The location is the
    /// station when docked (the hangar), the ship in space (its hold).
    ///
    /// Evidence: the guns hold more rounds than before, read back with
    /// [`Self::loaded_rounds`]. Returns the rounds now loaded across `guns`.
    pub fn load_ammo(
        &mut self,
        guns: &[(i64, i64)],
        charge_ids: &[i64],
        charge_type_id: i64,
        location_id: i64,
    ) -> Result<i64> {
        let ship = self.active_ship("load_ammo")?;
        if guns.is_empty() || charge_ids.is_empty() {
            return Err(Error::NotSent(
                "load_ammo: no gun or no charge named".to_owned(),
            ));
        }
        let mut before = 0;
        for &(_, flag) in guns {
            before += self.loaded_rounds(ship, flag, charge_type_id)?;
        }
        let modules = match guns {
            [(one, _)] => PyValue::Int(*one),
            many => PyValue::List(many.iter().map(|(id, _)| PyValue::Int(*id)).collect()),
        };
        let params = self.location_moniker("load_ammo")?;
        let answer = self.moniker_call(
            "dogmaIM",
            params,
            "LoadAmmo",
            vec![
                PyValue::Int(ship),
                modules,
                PyValue::List(charge_ids.iter().copied().map(PyValue::Int).collect()),
                PyValue::Int(location_id),
            ],
            vec![],
        )?;
        let mut after = 0;
        for &(_, flag) in guns {
            after += self.loaded_rounds(ship, flag, charge_type_id)?;
        }
        if after > before {
            return Ok(after);
        }
        Err(Error::Refused(format!(
            "LoadAmmo({ship}, {guns:?}, {charge_ids:?}, {location_id}): still {after} rounds ({})",
            refusal_or_silence(&answer.outcome)
        )))
    }

    /// **Fire a gun at a target**: [`Self::activate`] with the gun's own
    /// effect by name -- `'projectileFired'` for an autocannon, `'targetAttack'`
    /// for a civilian turret, from the module table ([`weapon_effect`]: the
    /// bundle's with `content`), which is the client's `dogmaeffects` row
    /// **[V-data]** -- and
    /// `repeat=1000`, the auto-repeat the mining laser also sends
    /// (`mining.md` §1.1) **[I]** for a turret.
    ///
    /// Evidence: `OnGodmaShipEffect` for the gun's effect with `start=1`.
    pub fn fire(
        &mut self,
        gun_id: i64,
        gun_type_id: i64,
        content: Option<&eve_server::content::Content>,
        target: i64,
    ) -> Result<GodmaEffect> {
        let Some(effect) = weapon_effect(content, gun_type_id) else {
            return Err(Error::NotSent(format!(
                "fire: type {gun_type_id} has no weapon effect the server knows"
            )));
        };
        self.activate(gun_id, effect, Some(target), REPEAT_UNTIL_STOPPED)
    }

    /// **Open a container in space** -- a wreck: `GetInventoryFromId(itemID,
    /// 0)` on the location's `invbroker`, which is what `ItemWreck`'s
    /// controller asks `invCache` for (`invControllers.py:135-163`,
    /// `invCache.py:590-623`; `docs/lanes/damage-death.md`, P2.4) **[V]**.
    pub fn open_container(&mut self, item_id: i64) -> Result<ObjectRef> {
        let params = self.location_moniker("open_container")?;
        let answer = self.moniker_call(
            "invbroker",
            params,
            "GetInventoryFromId",
            vec![PyValue::Int(item_id), PyValue::Int(0)],
            vec![],
        )?;
        first_object("GetInventoryFromId", answer)
    }

    /// **Loot All**: list the wreck, then move everything into the active
    /// ship's hold the way `LootAll` does -- `ShipCargo().AddItems(items)`,
    /// which sends one item as `Add(itemID, wreckID, qty=None,
    /// flag=flagCargo)` and several as `MultiAdd([itemIDs], wreckID,
    /// flag=flagCargo)` (`invControllers.py:1597-1615`, `:600-693`) **[V]**.
    /// The client offers it only within `maxCargoContainerTransferDistance`,
    /// 2,500 m surface to surface (`appConst.py:1726`, `IsInRange`
    /// `:1502-1522`) **[V]**; outside that this is [`Error::NotSent`].
    ///
    /// Evidence: the wreck holds fewer units than before, read back with
    /// `List`. A full hold moves what fits and raises `NotEnoughCargoSpace`
    /// (P2.4); that key comes back in [`Looted::refusal`] beside the count.
    /// An empty wreck is `Ok` with nothing moved.
    pub fn loot_all(&mut self, wreck_id: i64) -> Result<Looted> {
        let ship = self.active_ship("loot_all")?;
        let world = self.world_now();
        let Some(ego) = world.ego else {
            return Err(Error::NotSent(
                "loot_all: no park, so no distance".to_owned(),
            ));
        };
        let Some(surface) = world.surface_distance(ego, wreck_id) else {
            return Err(Error::NotSent(format!(
                "loot_all: {wreck_id} is not in the park"
            )));
        };
        if surface >= LOOT_RANGE {
            return Err(Error::NotSent(format!(
                "loot_all: {surface:.0} m from the wreck; the client loots inside {LOOT_RANGE} m"
            )));
        }
        let wreck = self.open_container(wreck_id)?;
        let items = self.list(&wreck, None)?;
        let units = |rows: &[ItemRow]| rows.iter().map(|r| r.quantity).sum::<i64>();
        let had = units(&items);
        if items.is_empty() {
            return Ok(Looted {
                units: 0,
                types: Vec::new(),
                refusal: None,
            });
        }
        let hold = self.open_ship(ship)?;
        let flag = (PyValue::Int(FLAG_CARGO), bstr("flag"));
        let outcome = match items.as_slice() {
            [one] => self.call_on(
                &hold,
                "Add",
                vec![PyValue::Int(one.item_id), PyValue::Int(wreck_id)],
                vec![(PyValue::None, bstr("qty")), flag],
            )?,
            many => self.call_on(
                &hold,
                "MultiAdd",
                vec![
                    PyValue::List(many.iter().map(|r| PyValue::Int(r.item_id)).collect()),
                    PyValue::Int(wreck_id),
                ],
                vec![flag],
            )?,
        };
        let left = self.list(&wreck, None)?;
        let moved = had - units(&left);
        let refusal = outcome.refusal();
        if moved > 0 {
            let mut types: Vec<i64> = items.iter().map(|r| r.type_id).collect();
            types.dedup();
            return Ok(Looted {
                units: moved,
                types,
                refusal,
            });
        }
        Err(Error::Refused(format!(
            "loot {wreck_id}: nothing moved ({})",
            refusal_or_silence(&outcome)
        )))
    }
}

// -- agents -------------------------------------------------------------------

impl Conn {
    /// **Talk to an agent**: `DoAction(None)` on `Moniker('agentMgr',
    /// agentID)`, the window's opening call (`agentDialogueWindow.py:485`;
    /// `eveMoniker.GetAgent`, `docs/re/agents-missions.md` §3.1) **[V]**.
    /// When the first action is `RequestMission` or `ViewMission` the window
    /// sends `DoAction(thatActionID)` itself (`:493`) **[V]**, and so does
    /// this.
    ///
    /// Evidence: an answer of the conversation's shape.
    pub fn talk_to_agent(&mut self, agent: i64) -> Result<AgentTurn> {
        let turn = self.agent_do_action(agent, None)?;
        match turn.actions.first() {
            Some(&(id, BUTTON_REQUEST_MISSION | BUTTON_VIEW_MISSION)) => {
                self.agent_do_action(agent, Some(id))
            }
            _ => Ok(turn),
        }
    }

    /// **Press an agent's button**: `DoAction(actionID)` with the id the
    /// last turn paired with `button` -- a click on a drawn button, and so
    /// [`Error::NotSent`] when `turn` drew none (Accept is not drawn before
    /// an offer, Complete not before the objective is met).
    ///
    /// Evidence: for Accept, Decline, Complete and Quit, the server's
    /// `OnAgentMissionChange(change, agentID)` push, which the journal
    /// waits for (`mission.rs::tell_journal_at`); for any other button, an
    /// answer of the conversation's shape.
    pub fn agent_button(&mut self, agent: i64, turn: &AgentTurn, button: i64) -> Result<AgentTurn> {
        let Some(action) = turn.action_for(button) else {
            return Err(Error::NotSent(format!(
                "agent {agent}: no button {button} on offer ({:?})",
                turn.actions
            )));
        };
        let change = match button {
            BUTTON_ACCEPT => "accepted",
            BUTTON_DECLINE => "declined",
            BUTTON_COMPLETE => "completed",
            BUTTON_QUIT => "quit",
            _ => return self.agent_do_action(agent, Some(action)),
        };
        let after = self.agent_do_action(agent, Some(action))?;
        let mut sink = Vec::new();
        let seen = self.await_push(EVIDENCE, &mut sink, |name, args| {
            name == "OnAgentMissionChange"
                && args.first().and_then(crate::text).as_deref() == Some(change)
                && args.get(1) == Some(&PyValue::Int(agent))
        })?;
        if seen.is_some() {
            return Ok(after);
        }
        Err(Error::Refused(format!(
            "agent {agent}: DoAction({action}) brought no OnAgentMissionChange({change:?})"
        )))
    }

    /// **The mission's objective document**: `GetMissionObjectiveInfo()` on
    /// the agent's moniker, no arguments (`agentDialogueWindow.py:261`)
    /// **[V]**. The raw dict; [`CourierLeg::from_objective`] reads a
    /// courier's leg out of it.
    pub fn mission_objective(&mut self, agent: i64) -> Result<PyValue> {
        let answer = self.moniker_call(
            "agentMgr",
            PyValue::Int(agent),
            "GetMissionObjectiveInfo",
            vec![],
            vec![],
        )?;
        match answer.value {
            Some(v @ PyValue::Dict(_)) => Ok(v),
            other => Err(Error::Refused(format!(
                "GetMissionObjectiveInfo answered {other:?} (refusal: {:?})",
                answer.outcome.refusal()
            ))),
        }
    }

    fn agent_do_action(&mut self, agent: i64, action: Option<i64>) -> Result<AgentTurn> {
        let answer = self.moniker_call(
            "agentMgr",
            PyValue::Int(agent),
            "DoAction",
            vec![action.map_or(PyValue::None, PyValue::Int)],
            vec![],
        )?;
        answer
            .value
            .as_ref()
            .and_then(AgentTurn::from_value)
            .ok_or_else(|| {
                Error::Refused(format!(
                    "agent {agent}: DoAction({action:?}) answered {:?} (refusal: {:?})",
                    answer.value,
                    answer.outcome.refusal()
                ))
            })
    }
}

// -- the market (CP47) ---------------------------------------------------------

/// `rangeStation`: an order that trades only in its own station
/// (`marketutil/const.py:3`, `docs/lanes/market.md` evidence table) **[V]**.
pub const RANGE_STATION: i64 = -1;

/// `DBTYPE_CY` carries ISK x 10,000; its getter divides on read
/// (`docs/reference/dbrow.md` §3, `service/market.rs::book_row`) **[V]**.
const CY_SCALE: f64 = 10_000.0;

/// One order row, from a `GetOrders` half, a `GetCharOrders` rowset or an
/// `OnOwnOrdersChanged` push: a `blue.DBRow` over
/// `market_order_descriptor()` (own rows add columns this does not read),
/// read **by column name** (`docs/reference/market-browse.md` row 13) **[V]**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BookOrder {
    pub order_id: i64,
    pub type_id: i64,
    pub station_id: i64,
    /// ISK per unit.
    pub price: f64,
    pub vol_remaining: i64,
    pub bid: bool,
}

impl BookOrder {
    pub fn from_value(v: &PyValue) -> Option<BookOrder> {
        let PyValue::DbRow(row) = v else {
            return None;
        };
        let get = |name: &str| {
            row.descriptor()
                .columns()
                .iter()
                .position(|c| c.name == name.as_bytes())
                .and_then(|i| row.values().get(i))
        };
        let int = |name: &str| match get(name) {
            Some(DbValue::Int(n)) => Some(*n),
            Some(DbValue::Real(r)) => Some(r.round() as i64),
            Some(DbValue::Bool(b)) => Some(i64::from(*b)),
            _ => None,
        };
        let price = match get("price") {
            Some(DbValue::Int(n)) => *n as f64 / CY_SCALE,
            Some(DbValue::Real(r)) => *r,
            _ => return None,
        };
        Some(BookOrder {
            order_id: int("orderID")?,
            type_id: int("typeID")?,
            station_id: int("stationID")?,
            price,
            vol_remaining: int("volRemaining")?,
            bid: int("bid")? != 0,
        })
    }

    /// **An NPC exchange order**: its id is in the band no player order
    /// reaches (`econ_exchange::NPC_ORDER_BASE..NPC_ORDER_END`, C5 §4.1).
    /// The id is all the wire says about who is behind an order, and the
    /// band is the server's own constant, so this is the server's rule
    /// read back, not a guess.
    pub fn is_npc(&self) -> bool {
        eve_server::econ_exchange::is_npc(self.order_id)
    }
}

/// One type's order book in the pilot's region: `GetOrders`' two halves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Book {
    pub asks: Vec<BookOrder>,
    pub bids: Vec<BookOrder>,
}

impl Book {
    /// `(asks, bids)`: a 2-tuple of `CRowset`s, each an `ObjectEx` whose
    /// header args carry the descriptor and the list of rows
    /// (`marketsvc.py:863-866`; `eve-rowset` `CRowset::to_pyvalue`)
    /// **[V]**. Rows that do not read are skipped.
    pub fn from_value(v: &PyValue) -> Option<Book> {
        let PyValue::Tuple(halves) = v else {
            return None;
        };
        let [asks, bids] = &halves[..] else {
            return None;
        };
        let mut book = Book {
            asks: rows_of(asks),
            bids: rows_of(bids),
        };
        book.asks.retain(|o| !o.bid);
        book.bids.retain(|o| o.bid);
        Some(book)
    }

    /// The cheapest ask.
    pub fn best_ask(&self) -> Option<&BookOrder> {
        self.asks.iter().min_by(|a, b| a.price.total_cmp(&b.price))
    }

    /// The highest bid.
    pub fn best_bid(&self) -> Option<&BookOrder> {
        self.bids.iter().max_by(|a, b| a.price.total_cmp(&b.price))
    }

    /// **The NPC ceiling**: the cheapest NPC ask (Tier B's ceiling at a
    /// hub, C5 §4.3), if the exchange sells this type here.
    pub fn npc_ceiling(&self) -> Option<f64> {
        self.asks
            .iter()
            .filter(|o| o.is_npc())
            .map(|o| o.price)
            .min_by(f64::total_cmp)
    }

    /// **The NPC floor**: the highest NPC bid (Tier B's floor, C5 §4.3).
    pub fn npc_floor(&self) -> Option<f64> {
        self.bids
            .iter()
            .filter(|o| o.is_npc())
            .map(|o| o.price)
            .max_by(f64::total_cmp)
    }
}

/// Every order row anywhere inside `v`: a `CRowset`'s rows are a list in
/// its `ObjectEx` header, and a plain list of rows reads too.
fn rows_of(v: &PyValue) -> Vec<BookOrder> {
    let mut out = Vec::new();
    collect_rows(v, &mut out);
    out
}

fn collect_rows(v: &PyValue, out: &mut Vec<BookOrder>) {
    match v {
        PyValue::DbRow(_) => out.extend(BookOrder::from_value(v)),
        PyValue::List(items) | PyValue::Tuple(items) => {
            for i in items {
                collect_rows(i, out);
            }
        }
        PyValue::ObjectEx { header, .. } => collect_rows(header, out),
        _ => {}
    }
}

/// `CachedMethodCallResult`'s state is `(details, marshal.Save(result),
/// version)` (`objectCaching.py:860`, `docs/lanes/market.md` finding 2)
/// **[V]**: the answer is the marshalled middle. Anything else is the
/// answer itself.
fn uncached(v: PyValue) -> Option<PyValue> {
    if let PyValue::Instance { class, state } = &v
        && String::from_utf8_lossy(class).ends_with("CachedMethodCallResult")
    {
        let PyValue::Tuple(parts) = state.as_ref() else {
            return None;
        };
        let PyValue::Bytes(bytes) = parts.get(1)? else {
            return None;
        };
        return eve_server::loads(bytes).ok();
    }
    Some(v)
}

/// The bid row for `type_id` in an `OnOwnOrdersChanged(orders, reason,
/// isCorp)` push (`docs/reference/push-events.md`, the market row) **[V]**.
fn own_bid(args: &[PyValue], type_id: i64) -> Option<BookOrder> {
    rows_of(args.first()?)
        .into_iter()
        .find(|o| o.type_id == type_id && o.bid)
}

/// One stack to sell: `sellMulti.GetValidatedItem`'s line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SellLine {
    pub station_id: i64,
    pub type_id: i64,
    pub item_id: i64,
    /// The least ISK per unit the seller takes.
    pub price: f64,
    pub quantity: i64,
}

impl SellLine {
    /// `KeyVal(stationID, typeID, itemID, price, quantity, officeID, delta,
    /// rawBrokerFeePercentage)` (`sellMulti.py:541-549`) **[V]**.
    fn to_value(self) -> PyValue {
        eve_server::pyutil::keyval(vec![
            ("stationID", PyValue::Int(self.station_id)),
            ("typeID", PyValue::Int(self.type_id)),
            ("itemID", PyValue::Int(self.item_id)),
            ("price", PyValue::Real(self.price)),
            ("quantity", PyValue::Int(self.quantity)),
            ("officeID", PyValue::None),
            ("delta", PyValue::Real(0.0)),
            ("rawBrokerFeePercentage", PyValue::Real(0.0)),
        ])
    }
}

impl Conn {
    /// **The wallet**: `account.GetCashBalance(0)`, the call `WalletSvc.Run`
    /// makes (`docs/reference/push-path.md` §"wealth", `docs/reference/wallet-ledger.md` row 456)
    /// **[V]**. The client then follows `OnAccountChange`; a bot that asks
    /// again sends the same call, never a new one.
    pub fn cash_balance(&mut self) -> Result<f64> {
        let outcome = self.call("account", "GetCashBalance", vec![PyValue::Int(0)])?;
        match outcome.value() {
            Some(PyValue::Real(r)) => Ok(r),
            Some(PyValue::Int(n)) => Ok(n as f64),
            other => Err(Error::Refused(format!(
                "GetCashBalance answered {other:?} ({})",
                refusal_or_silence(&outcome)
            ))),
        }
    }

    /// **The price index**: `config.GetAverageMarketPrices()`, which the
    /// client stores as `cfg._averageMarketPrice` (`typeHelpers:106-118`,
    /// `service/base.rs`) **[V]**: `{typeID: KeyVal(adjustedPrice,
    /// averagePrice)}`. `adjustedPrice` is the server's `P_ref` (CP31).
    /// Returns `typeID -> adjustedPrice`.
    pub fn adjusted_prices(&mut self) -> Result<Vec<(i64, f64)>> {
        Ok(self
            .price_index()?
            .into_iter()
            .map(|(t, adjusted, _)| (t, adjusted))
            .collect())
    }

    /// **Both prices of the index** (FE-VALUE): `(typeID, adjustedPrice,
    /// averagePrice)` from the same `config.GetAverageMarketPrices` call.
    /// `averagePrice` is the server's trading index (`econ_prices`), the
    /// slow anchor of a bot's perceived value; absent when the row has none.
    pub fn price_index(&mut self) -> Result<Vec<(i64, f64, Option<f64>)>> {
        Ok(self.price_index_shared()?.to_vec())
    }

    /// [`Self::price_index`] without the copy, through the process-wide
    /// [`crate::pricing::PriceIndexCache`] when the driver shares one:
    /// 300 bots logging in at once make one wire call, not 300 (PERF-BURST).
    pub fn price_index_shared(&mut self) -> Result<crate::pricing::SharedPriceRows> {
        match self.price_cache.clone() {
            Some(cache) => cache.get_or_fetch(Instant::now(), |_| self.price_index_fetch()),
            None => self.price_index_fetch().map(std::sync::Arc::new),
        }
    }

    /// Share the process-wide price index cache with this connection.
    pub fn share_price_index(&mut self, cache: std::sync::Arc<crate::pricing::PriceIndexCache>) {
        self.price_cache = Some(cache);
    }

    fn price_index_fetch(&mut self) -> Result<Vec<(i64, f64, Option<f64>)>> {
        let outcome = self.call("config", "GetAverageMarketPrices", vec![])?;
        let Some(PyValue::Dict(pairs)) = outcome.value() else {
            return Err(Error::Refused(format!(
                "GetAverageMarketPrices answered {:?} ({})",
                outcome.value(),
                refusal_or_silence(&outcome)
            )));
        };
        // `(value, key)`: marshal dict entries are inverted.
        Ok(pairs
            .iter()
            .filter_map(|(kv, id)| {
                let PyValue::Int(id) = id else { return None };
                let attrs = crate::attributes(kv);
                let num = |v: &PyValue| match v {
                    PyValue::Real(p) => Some(*p),
                    PyValue::Int(p) => Some(*p as f64),
                    _ => None,
                };
                let adjusted = num(crate::dict_get(attrs, "adjustedPrice")?)?;
                let average = crate::dict_get(attrs, "averagePrice").and_then(num);
                Some((*id, adjusted, average))
            })
            .collect())
    }

    /// **One type's book**: `marketProxy.GetOrders(typeID)`, issued when a
    /// type is selected (`marketsvc.py:856`, `docs/reference/market-browse.md` row
    /// 13) **[V]**, answered as a `CachedMethodCallResult`.
    pub fn market_orders(&mut self, type_id: i64) -> Result<Book> {
        let outcome = self.call("marketProxy", "GetOrders", vec![PyValue::Int(type_id)])?;
        outcome
            .value()
            .and_then(uncached)
            .as_ref()
            .and_then(Book::from_value)
            .ok_or_else(|| {
                Error::Refused(format!(
                    "GetOrders({type_id}) answered {:?} ({})",
                    outcome.value(),
                    refusal_or_silence(&outcome)
                ))
            })
    }

    /// **My open orders**: `marketProxy.GetCharOrders()` (`orders.py:342`,
    /// the objectCaching entry in `docs/reference/market-browse.md`) **[V]**.
    pub fn my_orders(&mut self) -> Result<Vec<BookOrder>> {
        let outcome = self.call("marketProxy", "GetCharOrders", vec![])?;
        match outcome.value().and_then(uncached) {
            Some(v) => Ok(rows_of(&v)),
            None => Err(Error::Refused(format!(
                "GetCharOrders: {}",
                refusal_or_silence(&outcome)
            ))),
        }
    }

    /// **Sell stacks from the hangar**: `marketProxy.PlaceMultiSellOrder(
    /// itemList, useCorp, duration, expectedBrokersFee)`, the whole Sell
    /// window path (`sellMulti._PerformTrade`, `marketsvc.py:322`) **[V]**.
    /// `days == 0` is an immediate sell into bids at `price` or better;
    /// otherwise the remainder rests as an ask. Docked only, like the
    /// window.
    ///
    /// Evidence: the answer's truth, which is what the window reads
    /// (`if wereOrdersCreated:`, `sellMulti.py:509`) **[V]**.
    pub fn sell(&mut self, lines: &[SellLine], days: i64, broker_fee: f64) -> Result<()> {
        self.docked_at("sell")?;
        if lines.is_empty() {
            return Err(Error::NotSent("sell: nothing to sell".to_owned()));
        }
        let items = lines.iter().map(|l| l.to_value()).collect();
        let outcome = self.call(
            "marketProxy",
            "PlaceMultiSellOrder",
            vec![
                PyValue::List(items),
                PyValue::Bool(false),
                PyValue::Int(days),
                PyValue::Real(broker_fee),
            ],
        )?;
        match outcome.value() {
            Some(PyValue::Bool(true)) => Ok(()),
            other => Err(Error::Refused(format!(
                "PlaceMultiSellOrder answered {other:?} ({})",
                refusal_or_silence(&outcome)
            ))),
        }
    }

    /// **Buy**: `marketProxy.PlaceBuyOrder(stationID, typeID, price,
    /// quantity, orderRange, minVolume, duration, useCorp,
    /// expectedBrokersFee)`, nine positional, the return discarded
    /// (`marketsvc.py:313`) **[V]**. `days == 0` is the immediate buy
    /// (`buyThisTypeWindow.Buy`, `:984`) **[V]**.
    ///
    /// Evidence: the `OnOwnOrdersChanged` push naming the order, the only
    /// visible effect (`docs/reference/market-browse.md` row 3) **[V]**. The row
    /// comes back: `vol_remaining` is what did not fill.
    pub fn buy(
        &mut self,
        station_id: i64,
        type_id: i64,
        price: f64,
        quantity: i64,
        days: i64,
        broker_fee: f64,
    ) -> Result<BookOrder> {
        self.buy_with(
            false, station_id, type_id, price, quantity, days, broker_fee,
        )
    }

    /// [`Conn::buy`] with the client's "use corporation wallet" box
    /// (`useCorp`, the eighth argument) **[V]** (CORP-BUY): the corporation's
    /// active division escrows and pays, and what fills lands in the
    /// corporation's deliveries hangar. Refused for a pilot without the
    /// accountant or trader role.
    #[allow(clippy::too_many_arguments)]
    pub fn buy_with(
        &mut self,
        use_corp: bool,
        station_id: i64,
        type_id: i64,
        price: f64,
        quantity: i64,
        days: i64,
        broker_fee: f64,
    ) -> Result<BookOrder> {
        let outcome = self.call(
            "marketProxy",
            "PlaceBuyOrder",
            vec![
                PyValue::Int(station_id),
                PyValue::Int(type_id),
                PyValue::Real(price),
                PyValue::Int(quantity),
                PyValue::Int(RANGE_STATION),
                PyValue::Int(1),
                PyValue::Int(days),
                PyValue::Bool(use_corp),
                PyValue::Real(broker_fee),
            ],
        )?;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!("PlaceBuyOrder: {key}")));
        }
        let mine = |name: &str, args: &[PyValue]| {
            (name == "OnOwnOrdersChanged").then(|| own_bid(args, type_id))?
        };
        let seen = outcome
            .notifications
            .iter()
            .filter_map(notification)
            .find_map(|(name, args)| mine(&name, &args));
        let seen = match seen {
            Some(row) => Some(row),
            None => {
                let mut sink = Vec::new();
                self.await_push(EVIDENCE, &mut sink, |n, a| mine(n, a).is_some())?
                    .and_then(|(name, args)| mine(&name, &args))
            }
        };
        seen.ok_or_else(|| {
            Error::Refused(format!(
                "PlaceBuyOrder({type_id}) brought no OnOwnOrdersChanged"
            ))
        })
    }

    /// **Refine** (FX6): `reprocessingSvc.Reprocess(itemIDs, fromLocation,
    /// ownerID, outputLocationID, outputFlagID)` on the station's
    /// `reprocessingSvc` moniker (a bare station id), the refinery window's
    /// own call (`ItemReprocessor._DoReprocess`; the wire order and the
    /// bind are `service/reprocess.rs`'s **[V]**). No output location: the
    /// station hangar.
    ///
    /// Evidence: the answer's 2-sequence, `(processedItemIDs, {typeID:
    /// quantity})`; a raise (`MrNothingToRefine`, `NotEnoughMoney`, a
    /// `CustomNotify`) is a refusal. Returns the outputs.
    pub fn reprocess(&mut self, item_ids: &[i64]) -> Result<Vec<(i64, i64)>> {
        let station = self.docked_at("reprocess")?;
        let owner = self
            .session()
            .char_id()
            .ok_or_else(|| Error::NotSent("reprocess: no character".to_owned()))?;
        if item_ids.is_empty() {
            return Err(Error::NotSent("reprocess: nothing to refine".to_owned()));
        }
        let answer = self.moniker_call(
            "reprocessingSvc",
            PyValue::Int(station),
            "Reprocess",
            vec![
                PyValue::List(item_ids.iter().copied().map(PyValue::Int).collect()),
                PyValue::Int(station),
                PyValue::Int(owner),
                PyValue::None,
                PyValue::None,
            ],
            vec![],
        )?;
        if let Some(key) = answer.refusal() {
            return Err(Error::Refused(format!("Reprocess: {key}")));
        }
        let pair = match &answer.value {
            Some(PyValue::Tuple(v) | PyValue::List(v)) if v.len() == 2 => v,
            other => {
                return Err(Error::Refused(format!("Reprocess answered {other:?}")));
            }
        };
        let processed = matches!(&pair[0], PyValue::List(v) | PyValue::Tuple(v) if !v.is_empty());
        let outputs: Vec<(i64, i64)> = match &pair[1] {
            // A marshal dict entry is `(value, key)`.
            PyValue::Dict(entries) => entries
                .iter()
                .filter_map(|(v, k)| match (k, v) {
                    (PyValue::Int(t), PyValue::Int(q)) => Some((*t, *q)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        if !processed {
            return Err(Error::Refused("Reprocess processed nothing".to_owned()));
        }
        Ok(outputs)
    }
}

// -- own orders and the modify (E9) ----------------------------------------------

/// One of my open orders, from `GetCharOrders`: the book row plus what
/// `ModifyCharOrder` sends back and the times its delay runs from. Read by
/// column name, like [`BookOrder`] (`service/market.rs`
/// `own_order_descriptor`) **[V]**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnOrder {
    pub order: BookOrder,
    pub solar_system_id: i64,
    pub range: i64,
    /// FILETIME.
    pub issue_date: i64,
    /// FILETIME of the last modify; `None` until one.
    pub last_state_change: Option<i64>,
}

impl OwnOrder {
    pub fn from_value(v: &PyValue) -> Option<OwnOrder> {
        let order = BookOrder::from_value(v)?;
        let PyValue::DbRow(row) = v else {
            return None;
        };
        let int = |name: &str| {
            row.descriptor()
                .columns()
                .iter()
                .position(|c| c.name == name.as_bytes())
                .and_then(|i| match row.values().get(i) {
                    Some(DbValue::Int(n)) => Some(*n),
                    Some(DbValue::Real(r)) => Some(r.round() as i64),
                    _ => None,
                })
        };
        Some(OwnOrder {
            order,
            solar_system_id: int("solarSystemID").unwrap_or(0),
            range: int("range").unwrap_or(RANGE_STATION),
            issue_date: int("issueDate")?,
            last_state_change: int("lastStateChange"),
        })
    }

    /// When the server takes the next modify: `mktModificationDelay` after
    /// the last operation, the last modify else the issue (E5's rule,
    /// `service/market.rs::modification_wait`), as a FILETIME.
    pub fn modifiable_from(&self) -> i64 {
        self.last_state_change.unwrap_or(self.issue_date)
            + MODIFICATION_DELAY.as_secs() as i64 * FILETIME_SECOND
    }
}

/// `appConst.mktModificationDelay`, 300 s **[V]** (E5 refuses a modify
/// inside it with `MktOrderDelay`).
pub const MODIFICATION_DELAY: Duration = Duration::from_secs(300);
/// FILETIME ticks a second.
pub const FILETIME_SECOND: i64 = 10_000_000;

// -- industry (E9) ------------------------------------------------------------------

/// `industry.job.Location`, the class `industryCommon.MatchLocation` copies
/// and `Job.dump()` sends: new-style, so the `0x23` form, header
/// `((cls,), state)` (`service/industry.rs` `location_row`) **[V]**.
pub(crate) fn job_location(item_id: i64, flag_id: i64) -> PyValue {
    PyValue::ObjectEx {
        form: eve_marshal::ObjectForm::New,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Tuple(vec![PyValue::Global(b"industry.job.Location".to_vec())]),
            eve_server::pyutil::pydict(vec![
                ("itemID", PyValue::Int(item_id)),
                ("flagID", PyValue::Int(flag_id)),
            ]),
        ])),
        list: vec![],
        dict: vec![],
    }
}

/// An attribute of a `KeyVal` row, as an integer.
fn kv_int(v: &PyValue, name: &str) -> Option<i64> {
    match crate::dict_get(crate::attributes(v), name)? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// One blueprint I own: a `GetBlueprintDataByOwner` row
/// (`service/industry.rs` `blueprint_row`, the `KeyVal` the client's
/// `BlueprintInstance` reads) **[V]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlueprintRow {
    pub item_id: i64,
    pub type_id: i64,
    pub location_id: i64,
    pub flag_id: i64,
    /// `-1` for an original (`Blueprint.__new__`'s sentinel) **[V]**.
    pub runs: i64,
    pub material_efficiency: i64,
    /// The job it is installed in.
    pub job_id: Option<i64>,
}

impl BlueprintRow {
    pub fn from_value(v: &PyValue) -> Option<BlueprintRow> {
        Some(BlueprintRow {
            item_id: kv_int(v, "itemID")?,
            type_id: kv_int(v, "typeID")?,
            location_id: kv_int(v, "locationID")?,
            flag_id: kv_int(v, "flagID")?,
            runs: kv_int(v, "runs").unwrap_or(-1),
            material_efficiency: kv_int(v, "materialEfficiency").unwrap_or(0),
            job_id: kv_int(v, "jobID"),
        })
    }

    pub fn is_original(&self) -> bool {
        self.runs < 0
    }
}

/// One of my industry jobs: a `GetJobsByOwner` row (`service/industry.rs`
/// `job_row`, what `industryCommon.JobData` reads) **[V]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobRow {
    pub job_id: i64,
    pub activity_id: i64,
    pub blueprint_id: i64,
    pub status: i64,
    pub solar_system_id: i64,
    pub station_id: i64,
    /// FILETIME.
    pub end_date: i64,
    pub runs: i64,
    pub product_type_id: i64,
}

impl JobRow {
    pub fn from_value(v: &PyValue) -> Option<JobRow> {
        Some(JobRow {
            job_id: kv_int(v, "jobID")?,
            activity_id: kv_int(v, "activityID")?,
            blueprint_id: kv_int(v, "blueprintID")?,
            status: kv_int(v, "status")?,
            solar_system_id: kv_int(v, "solarSystemID").unwrap_or(0),
            station_id: kv_int(v, "stationID").unwrap_or(0),
            end_date: kv_int(v, "endDate")?,
            runs: kv_int(v, "runs").unwrap_or(0),
            product_type_id: kv_int(v, "productTypeID").unwrap_or(0),
        })
    }

    /// `JobBase.completed`: `status >= STATUS_COMPLETED` **[V]**.
    pub fn completed(&self) -> bool {
        self.status >= eve_server::industry::STATUS_COMPLETED
    }

    /// Deliverable at `now` (a FILETIME): not collected, and its end time
    /// passed -- `industryCommon.JobStatus`'s Ready, which the client works
    /// out itself from `endDate` **[V]**.
    pub fn ready_at(&self, now: i64) -> bool {
        !self.completed() && self.end_date <= now
    }
}

impl Conn {
    /// **My open orders, with their times**: `marketProxy.GetCharOrders()`,
    /// as [`Conn::my_orders`], read as [`OwnOrder`]s.
    pub fn my_open_orders(&mut self) -> Result<Vec<OwnOrder>> {
        self.open_orders_of("GetCharOrders")
    }

    /// **The corporation's open orders** (CORP-BUY): `GetCorporationOrders`,
    /// the Corporation Orders tab's call, answered to an accountant; empty
    /// for anyone else.
    pub fn corp_open_orders(&mut self) -> Result<Vec<OwnOrder>> {
        self.open_orders_of("GetCorporationOrders")
    }

    fn open_orders_of(&mut self, method: &str) -> Result<Vec<OwnOrder>> {
        let outcome = self.call("marketProxy", method, vec![])?;
        let Some(v) = outcome.value().and_then(uncached) else {
            return Err(Error::Refused(format!(
                "{method}: {}",
                refusal_or_silence(&outcome)
            )));
        };
        let mut rows = Vec::new();
        collect_values(&v, &mut rows);
        Ok(rows.iter().filter_map(OwnOrder::from_value).collect())
    }

    /// **Reprice an order**: `marketProxy.ModifyCharOrder(orderID, newPrice,
    /// bid, stationID, solarSystemID, price, range, volRemaining,
    /// issueDate)`, nine positional, read off the client's own row of the
    /// order (`marketsvc.py:333`) **[V]**. Return discarded; a raise
    /// (`MktOrderDelay` inside the delay, `MktOrderChanged` after a fill) is
    /// a refusal.
    pub fn modify_order(&mut self, own: &OwnOrder, new_price: f64) -> Result<()> {
        let o = own.order;
        let outcome = self.call(
            "marketProxy",
            "ModifyCharOrder",
            vec![
                PyValue::Int(o.order_id),
                PyValue::Real(new_price),
                PyValue::Bool(o.bid),
                PyValue::Int(o.station_id),
                PyValue::Int(own.solar_system_id),
                PyValue::Real(o.price),
                PyValue::Int(own.range),
                PyValue::Int(o.vol_remaining),
                PyValue::Int(own.issue_date),
            ],
        )?;
        match outcome.refusal() {
            None => Ok(()),
            Some(key) => Err(Error::Refused(format!("ModifyCharOrder: {key}"))),
        }
    }

    /// **My blueprints**: `blueprintManager.GetBlueprintDataByOwner(ownerID,
    /// facilityID)` with no facility (everywhere), `blueprintSvc`'s own
    /// read; the answer is `(rows, counts)` (`service/industry.rs`) **[V]**.
    pub fn blueprints(&mut self) -> Result<Vec<BlueprintRow>> {
        let me = self
            .session()
            .char_id()
            .ok_or_else(|| Error::NotSent("blueprints: no character".to_owned()))?;
        let outcome = self.call(
            "blueprintManager",
            "GetBlueprintDataByOwner",
            vec![PyValue::Int(me), PyValue::None],
        )?;
        match outcome.value() {
            Some(PyValue::Tuple(parts)) => match parts.first() {
                Some(PyValue::List(rows)) => {
                    Ok(rows.iter().filter_map(BlueprintRow::from_value).collect())
                }
                other => Err(Error::Refused(format!(
                    "GetBlueprintDataByOwner rows: {other:?}"
                ))),
            },
            other => Err(Error::Refused(format!(
                "GetBlueprintDataByOwner answered {other:?} ({})",
                refusal_or_silence(&outcome)
            ))),
        }
    }

    /// **My running jobs**: `industryManager.GetJobsByOwner(ownerID,
    /// includeCompleted=False)` (`industrySvc.GetOwnerJobs`) **[V]**.
    pub fn industry_jobs(&mut self) -> Result<Vec<JobRow>> {
        let me = self
            .session()
            .char_id()
            .ok_or_else(|| Error::NotSent("industry jobs: no character".to_owned()))?;
        let outcome = self.call(
            "industryManager",
            "GetJobsByOwner",
            vec![PyValue::Int(me), PyValue::Bool(false)],
        )?;
        match outcome.value() {
            Some(PyValue::List(rows)) => Ok(rows.iter().filter_map(JobRow::from_value).collect()),
            other => Err(Error::Refused(format!(
                "GetJobsByOwner answered {other:?} ({})",
                refusal_or_silence(&outcome)
            ))),
        }
    }

    /// **Start a manufacturing job**: `industryManager.InstallJob(job.dump())`,
    /// one dict (`SubmitButton.ClickFunc` -> `industrySvc.InstallJob`)
    /// **[V]**. Of `Job.dump()`'s keys the server reads five (activity,
    /// blueprint, runs, input and output location) and recomputes the cost,
    /// time and materials itself (`service/industry.rs` `install_request`),
    /// so only those go. Materials come from, and the product goes to, the
    /// blueprint's own hangar. Returns the job id; an
    /// `IndustryValidationError` (no skill, materials, ISK or slot) is a
    /// refusal.
    pub fn install_job(&mut self, blueprint: &BlueprintRow, runs: i64) -> Result<i64> {
        self.docked_at("install job")?;
        let here = job_location(blueprint.location_id, blueprint.flag_id);
        let dump = eve_server::pyutil::pydict(vec![
            (
                "activityID",
                PyValue::Int(eve_server::industry::MANUFACTURING),
            ),
            ("blueprintID", PyValue::Int(blueprint.item_id)),
            ("runs", PyValue::Int(runs)),
            ("inputLocation", here.clone()),
            ("outputLocation", here),
        ]);
        let outcome = self.call("industryManager", "InstallJob", vec![dump])?;
        match (outcome.refusal(), outcome.value()) {
            (None, Some(PyValue::Int(job_id))) => Ok(job_id),
            (Some(key), _) => Err(Error::Refused(format!("InstallJob: {key}"))),
            (None, other) => Err(Error::Refused(format!("InstallJob answered {other:?}"))),
        }
    }

    /// **Deliver a finished job**: `industryManager.CompleteJob(jobID,
    /// solarSystemID)`, two positional, return discarded
    /// (`industrySvc.CompleteJob`) **[V]**. The product lands in the job's
    /// output hangar.
    pub fn complete_job(&mut self, job: &JobRow) -> Result<()> {
        let outcome = self.call(
            "industryManager",
            "CompleteJob",
            vec![PyValue::Int(job.job_id), PyValue::Int(job.solar_system_id)],
        )?;
        match outcome.refusal() {
            None => Ok(()),
            Some(key) => Err(Error::Refused(format!("CompleteJob: {key}"))),
        }
    }
}

/// Every `DbRow` anywhere inside `v` (the [`collect_rows`] walk, kept raw).
fn collect_values(v: &PyValue, out: &mut Vec<PyValue>) {
    match v {
        PyValue::DbRow(_) => out.push(v.clone()),
        PyValue::List(items) | PyValue::Tuple(items) => {
            for i in items {
                collect_values(i, out);
            }
        }
        PyValue::ObjectEx { header, .. } => collect_values(header, out),
        _ => {}
    }
}

fn first_object(what: &str, answer: Answer) -> Result<ObjectRef> {
    answer.objects.into_iter().next().ok_or_else(|| {
        Error::Protocol(format!(
            "{what}: no object in the answer ({:?})",
            answer.outcome.refusal()
        ))
    })
}

/// How many units of `item_id` a listing holds.
fn held(rows: &[ItemRow], item_id: i64) -> i64 {
    rows.iter()
        .filter(|r| r.item_id == item_id)
        .map(|r| r.quantity)
        .sum()
}

/// The refusal's key, or a note that there was none to read.
fn refusal_or_silence(outcome: &CallOutcome) -> String {
    match outcome.refusal() {
        Some(key) => key,
        None => "no UserError key".to_owned(),
    }
}

// -- probe scanning (SOC-B3) ---------------------------------------------------
//
// Every call below is the probe scanner window's own, as `docs/lanes/scan.md`
// §2 read it off `scanSvc.pyc`; `crate::probe` has the rules that use them.

/// The bookmark service, a plain remote service (`bookmarkSvc.py:164`,
/// `docs/lanes/bookmarks.md`) **[V]**.
pub const BOOKMARK_SERVICE: &str = "accessGroupBookmarkMgr";

/// How long a probe push may take to follow its call: the verbs' evidence
/// window. The pushes of one call are flushed together.
const PROBE_PUSH_WAIT: Duration = EVIDENCE;

/// How long [`Conn::reconnect_probes`] listens for probes that are still
/// out. **[A]**: the server answers within one flush; falsified by a bot
/// that launches a second set while its first is out.
const PROBES_QUIET: Duration = Duration::from_millis(1_000);

/// How long a probe scan runs: the launchers' `duration` of 10 s
/// **[V-data]**, which the server uses with no skill bonus
/// (`scan::SCAN_DURATION_MS`, its [A]).
pub const SCAN_DURATION: Duration = Duration::from_secs(10);

impl Conn {
    /// **The system scan manager**: `scanMgr.GetSystemScanMgr()`, the
    /// object `scanSvc.GetScanMan()` caches as `remoteObject` and every probe
    /// call goes to (`service.rs` `OBJECT_RETURNING`, `docs/lanes/scan.md`)
    /// **[V]**.
    pub fn system_scan_manager(&mut self) -> Result<ObjectRef> {
        let outcome = self.call("scanMgr", "GetSystemScanMgr", vec![])?;
        let value = outcome.value();
        value
            .as_ref()
            .map(crate::object_refs)
            .unwrap_or_default()
            .into_iter()
            .next()
            .ok_or_else(|| {
                Error::Protocol(format!(
                    "GetSystemScanMgr: no object in the answer ({})",
                    refusal_or_silence(&outcome)
                ))
            })
    }

    /// **The sensor overlay's signatures**: `GetFullState()` on the scan
    /// manager, `(anomalies, signatures, staticSites, structures)`, four
    /// dicts keyed by site id (`sensorSuiteService.OnSignalTrackerFullState`,
    /// `UNPACK_SEQUENCE 4`) **[V]**. Returns the second dict's
    /// `SignatureInfo`s: a blurred position, the target id and the
    /// deviation (`signal_tracker::signature_info`).
    pub fn sensor_overlay(
        &mut self,
        manager: &ObjectRef,
    ) -> Result<Vec<crate::probe::OverlaySite>> {
        let outcome = self.call_on(manager, "GetFullState", vec![], vec![])?;
        let Some(PyValue::Tuple(parts)) = outcome.value() else {
            return Err(Error::Refused(format!(
                "GetFullState: not a 4-tuple ({})",
                refusal_or_silence(&outcome)
            )));
        };
        let Some(PyValue::Dict(signatures)) = parts.get(1) else {
            return Ok(Vec::new());
        };
        Ok(signatures
            .iter()
            .filter_map(|(info, key)| match key {
                PyValue::Int(site_id) => crate::probe::OverlaySite::from_value(*site_id, info),
                _ => None,
            })
            .collect())
    }

    /// **Launch probes**: `dogmaIM.LaunchProbes(moduleID, numProbes)` on the
    /// location's `dogmaIM` moniker, return discarded
    /// (`scanSvc.FindModuleAndLaunchProbes:558`) **[V]**.
    ///
    /// Evidence: one `OnNewProbe(probe)` per probe launched (`scanSvc`
    /// `OnNewProbe`). Returns them; none is a refusal.
    pub fn launch_probes(
        &mut self,
        launcher: i64,
        count: i64,
    ) -> Result<Vec<crate::probe::ProbeState>> {
        let params = self.location_moniker("launch_probes")?;
        let mut outcome = self
            .moniker_call(
                "dogmaIM",
                params,
                "LaunchProbes",
                vec![PyValue::Int(launcher), PyValue::Int(count)],
                vec![],
            )?
            .outcome;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!(
                "LaunchProbes({launcher}, {count}): {key}"
            )));
        }
        let probes = self.new_probes(&mut outcome.notifications, PROBE_PUSH_WAIT)?;
        if probes.is_empty() {
            return Err(Error::Refused(format!(
                "LaunchProbes({launcher}, {count}): no OnNewProbe ({})",
                refusal_or_silence(&outcome)
            )));
        }
        Ok(probes)
    }

    /// **Reconnect to lost probes**: `ReconnectToLostProbes()` on the scan
    /// manager, return discarded (`scanSvc.ReconnectToLostProbes:331`)
    /// **[V]**; the server announces every live probe again with
    /// `OnNewProbe`. Returns them -- none is an answer, not a refusal.
    pub fn reconnect_probes(
        &mut self,
        manager: &ObjectRef,
    ) -> Result<Vec<crate::probe::ProbeState>> {
        let mut outcome = self.call_on(manager, "ReconnectToLostProbes", vec![], vec![])?;
        self.new_probes(&mut outcome.notifications, PROBES_QUIET)
    }

    /// The `OnNewProbe`s in `seen`, and those that follow within `within`
    /// of the last; every frame read is kept in `seen`.
    fn new_probes(
        &mut self,
        seen: &mut Vec<PyValue>,
        within: Duration,
    ) -> Result<Vec<crate::probe::ProbeState>> {
        let read = |frames: &[PyValue]| -> Vec<crate::probe::ProbeState> {
            frames
                .iter()
                .filter_map(notification)
                .filter(|(n, _)| n == "OnNewProbe")
                .filter_map(|(_, a)| a.first().and_then(crate::probe::ProbeState::from_value))
                .collect()
        };
        if read(seen).is_empty() {
            self.await_push(within, seen, |n, _| n == "OnNewProbe")?;
        }
        // The rest of the burst: one call's probes are flushed together.
        while self
            .await_push(Duration::from_millis(200), seen, |n, _| n == "OnNewProbe")?
            .is_some()
        {}
        let mut probes = read(seen);
        probes.sort_by_key(|p| p.probe_id);
        probes.dedup_by_key(|p| p.probe_id);
        Ok(probes)
    }

    /// **Analyze**: `RequestScans({probeID: probe})` on the scan manager,
    /// the probes being the `KeyVal`s `OnNewProbe` gave with `destination`,
    /// `scanRange` and `rangeStep` set (`scanSvc.RequestScans:251`,
    /// `probeTracker.GetProbesForScanning:306`) **[V]**. Return discarded.
    ///
    /// Evidence: `OnSystemScanStopped(probes, results, absentTargets)`,
    /// [`SCAN_DURATION`] after `OnSystemScanStarted`
    /// (`scanSvc.pyc:301`) **[V]**. Returns its results.
    pub fn request_scans(
        &mut self,
        manager: &ObjectRef,
        probes: &[crate::probe::ProbeState],
    ) -> Result<Vec<crate::probe::ScanResult>> {
        if probes.is_empty() {
            return Err(Error::NotSent("request_scans: no probe out".to_owned()));
        }
        let dict = PyValue::Dict(
            probes
                .iter()
                .map(|p| (p.to_value(), PyValue::Int(p.probe_id)))
                .collect(),
        );
        let mut outcome = self.call_on(manager, "RequestScans", vec![dict], vec![])?;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!("RequestScans: {key}")));
        }
        let scan = SCAN_DURATION;
        let stopped = |n: &str, _: &[PyValue]| n == "OnSystemScanStopped";
        let already = outcome
            .notifications
            .iter()
            .filter_map(notification)
            .find(|(n, a)| stopped(n, a));
        let stop = match already {
            Some(s) => Some(s),
            None => self.await_push(scan + EVIDENCE * 2, &mut outcome.notifications, stopped)?,
        };
        let Some((_, args)) = stop else {
            return Err(Error::Refused(format!(
                "RequestScans: no OnSystemScanStopped ({})",
                refusal_or_silence(&outcome)
            )));
        };
        Ok(match args.get(1) {
            Some(PyValue::List(rows) | PyValue::Tuple(rows)) => rows
                .iter()
                .filter_map(crate::probe::ScanResult::from_value)
                .collect(),
            _ => Vec::new(),
        })
    }

    /// **Recover probes**: `RecoverProbes(probeIDs)` on the scan manager,
    /// answered with the ids coming home (`probeTracker.RecoverProbes:231`)
    /// **[V]**; each is back in the hold as a charge.
    pub fn recover_probes(&mut self, manager: &ObjectRef, ids: &[i64]) -> Result<Vec<i64>> {
        let outcome = self.call_on(
            manager,
            "RecoverProbes",
            vec![PyValue::List(
                ids.iter().copied().map(PyValue::Int).collect(),
            )],
            vec![],
        )?;
        match outcome.value() {
            Some(PyValue::List(back) | PyValue::Tuple(back)) => Ok(back
                .iter()
                .filter_map(|v| match v {
                    PyValue::Int(i) => Some(*i),
                    _ => None,
                })
                .collect()),
            _ => Err(Error::Refused(format!(
                "RecoverProbes({ids:?}) ({})",
                refusal_or_silence(&outcome)
            ))),
        }
    }

    /// **Warp to a scan result**: bound `CmdWarpToStuff('scan', targetID,
    /// minRange=R)`, the result id as a string
    /// (`menusvc._WarpXToScanResult:1700`, `docs/lanes/scan.md` §2) **[V]**.
    /// The server answers only a result the caller's own probes made
    /// perfect (`signal_tracker::signature_target`).
    ///
    /// Evidence: a `WarpTo` for this ship in a `DoDestinyUpdate`; silence
    /// is a refusal (`service/space.rs::warp` pushes nothing for a target
    /// it cannot resolve).
    pub fn warp_to_scan_result(&mut self, target_id: &str, min_range: f64) -> Result<CallOutcome> {
        let ship = self.active_ship("warp_to_scan_result")?;
        let mut outcome = self.bound_call_kw(
            "CmdWarpToStuff",
            vec![bstr("scan"), bstr(target_id)],
            vec![(PyValue::Real(min_range), bstr("minRange"))],
        )?;
        let warps = |name: &str, args: &[PyValue]| {
            name == "DoDestinyUpdate"
                && crate::read_action_list(args)
                    .iter()
                    .any(|(_, f, ops)| f == "WarpTo" && ops.first() == Some(&PyValue::Int(ship)))
        };
        let already = outcome
            .notifications
            .iter()
            .filter_map(notification)
            .any(|(n, a)| warps(&n, &a));
        if already
            || self
                .await_push(EVIDENCE, &mut outcome.notifications, warps)?
                .is_some()
        {
            return Ok(outcome);
        }
        Err(Error::Refused(format!(
            "CmdWarpToStuff('scan', {target_id:?}): no WarpTo for this ship ({})",
            refusal_or_silence(&outcome)
        )))
    }

    /// **My personal bookmark folder**: the first personal folder of
    /// `GetMyActiveBookmarks()`, else the one the client makes itself when
    /// it has none, `AddFolder(True, name, '', None, None, None, None)`
    /// (`bookmarkSvc.py:164,175,399,407`, `docs/lanes/bookmarks.md`) **[V]**.
    pub fn personal_folder(&mut self, name: &str) -> Result<i64> {
        let outcome = self.call(BOOKMARK_SERVICE, "GetMyActiveBookmarks", vec![])?;
        if let Some(PyValue::Tuple(parts)) = outcome.value()
            && let Some(PyValue::List(folders) | PyValue::Tuple(folders)) = parts.first()
            && let Some(id) = folders
                .iter()
                .filter(|f| kv_int(f, "isPersonal") == Some(1))
                .find_map(|f| kv_int(f, "folderID"))
        {
            return Ok(id);
        }
        let outcome = self.call(
            BOOKMARK_SERVICE,
            "AddFolder",
            vec![
                PyValue::Bool(true),
                PyValue::Unicode(name.to_owned()),
                PyValue::Unicode(String::new()),
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::None,
            ],
        )?;
        outcome
            .value()
            .as_ref()
            .and_then(|f| kv_int(f, "folderID"))
            .ok_or_else(|| {
                Error::Refused(format!(
                    "AddFolder: no folder row ({})",
                    refusal_or_silence(&outcome)
                ))
            })
    }

    /// **Save Location** in space: bound `BookmarkLocation(itemID,
    /// folderID, name, comment, expiry, subfolderID=None)` with the pilot's
    /// own ship, which bookmarks the point it is at
    /// (`bookmarkSvc.py:575`, `addressbookService.py:475`) **[V]**.
    ///
    /// Evidence: the answer's 8-tuple, whose first item is the new
    /// bookmark's id. Returns it.
    pub fn save_location(&mut self, folder_id: i64, name: &str, comment: &str) -> Result<i64> {
        let ship = self.active_ship("save_location")?;
        let outcome = self.bound_call_kw(
            "BookmarkLocation",
            vec![
                PyValue::Int(ship),
                PyValue::Int(folder_id),
                PyValue::Unicode(name.to_owned()),
                PyValue::Unicode(comment.to_owned()),
                PyValue::None,
            ],
            vec![(PyValue::None, bstr("subfolderID"))],
        )?;
        match outcome.value() {
            Some(PyValue::Tuple(t)) => match t.first() {
                Some(PyValue::Int(id)) => Ok(*id),
                _ => Err(Error::Refused(format!("BookmarkLocation: answered {t:?}"))),
            },
            _ => Err(Error::Refused(format!(
                "BookmarkLocation({ship}, {folder_id}) ({})",
                refusal_or_silence(&outcome)
            ))),
        }
    }
}

// -- the repair shop (GAP-REPAIR-BOT) -----------------------------------------------

/// One row of a `repairSvc.GetRepairQuotes` answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RepairQuote {
    pub item_id: i64,
    /// Hit points of damage.
    pub damage: f64,
    /// `costToRepairOneUnitOfDamage`.
    pub unit_cost: f64,
}

impl RepairQuote {
    /// The window's own sum: `ceil(damage) x unit` (`base_repairshop.py`
    /// `RepairItems`) **[V]**.
    pub fn cost(&self) -> f64 {
        self.damage.ceil() * self.unit_cost
    }
}

/// What a docked bot does about its quote.
#[derive(Debug, Clone, PartialEq)]
pub enum RepairPlan {
    /// Nothing is damaged.
    Whole,
    /// Pay `total` for these items (the window's "Repair All").
    Pay { ids: Vec<i64>, total: f64 },
    /// The wallet does not cover `total` **[A]**: a bot that cannot pay
    /// undocks damaged and says so, rather than spending its last ISK (a
    /// replacement hull comes first). Falsified by a poor bot that dies for
    /// want of a repair it could have part-paid.
    TooPoor { total: f64, wallet: f64 },
}

/// A bot that cannot pay the whole quote part-pays the hull (armour first,
/// the server's order) from `wallet`, keeping half back **[A]**; a module
/// needs its full price, so only the active ship's own line is part-paid.
/// Falsified by a bot that is stranded broke after a part-payment. `None`
/// when the hull is whole or the share is under one point of damage.
pub fn plan_part_pay(quotes: &[RepairQuote], ship: i64, wallet: f64) -> Option<f64> {
    let q = quotes
        .iter()
        .find(|q| q.item_id == ship && q.damage.ceil() > 0.0)?;
    let pay = (wallet * 0.5).min(q.cost());
    (pay >= q.unit_cost).then_some(pay)
}

/// The decision: repair everything the quote lists when the wallet covers it.
pub fn plan_repair(quotes: &[RepairQuote], wallet: f64) -> RepairPlan {
    let hurt: Vec<&RepairQuote> = quotes.iter().filter(|q| q.damage.ceil() > 0.0).collect();
    if hurt.is_empty() {
        return RepairPlan::Whole;
    }
    let total: f64 = hurt.iter().map(|q| q.cost()).sum();
    if total > wallet {
        return RepairPlan::TooPoor { total, wallet };
    }
    RepairPlan::Pay {
        ids: hurt.iter().map(|q| q.item_id).collect(),
        total,
    }
}

/// A `GetRepairQuotes` answer, `{itemID: [KeyVal]}`, as quotes.
pub fn repair_quotes_of(answer: &PyValue) -> Vec<RepairQuote> {
    let PyValue::Dict(entries) = crate::attributes(answer) else {
        return Vec::new();
    };
    let num = |row: &PyValue, key: &str| -> Option<f64> {
        let PyValue::Dict(attrs) = crate::attributes(row) else {
            return None;
        };
        attrs
            .iter()
            .find(|(_, k)| crate::text(k).as_deref() == Some(key))
            .and_then(|(v, _)| match v {
                PyValue::Real(r) => Some(*r),
                PyValue::Int(n) => Some(*n as f64),
                _ => None,
            })
    };
    let mut out = Vec::new();
    for (rows, _) in entries {
        let (PyValue::List(rows) | PyValue::Tuple(rows)) = rows else {
            continue;
        };
        for row in rows {
            if let (Some(id), Some(damage), Some(unit)) = (
                num(row, "itemID"),
                num(row, "damage"),
                num(row, "costToRepairOneUnitOfDamage"),
            ) {
                out.push(RepairQuote {
                    item_id: id as i64,
                    damage,
                    unit_cost: unit,
                });
            }
        }
    }
    out
}

impl Conn {
    /// **Repair the flown ship at the station's repair shop** (the Repair
    /// Shop window's two calls, `base_repairshop.py` **[V]**):
    /// `GetRepairQuotes([ship])` on the `repairSvc` moniker
    /// `(station, station, groupStation)`, then `RepairItemsInStation(ids,
    /// total)` when [`plan_repair`] says the wallet covers it. Returns a note
    /// for the act's log line, empty when nothing was damaged. A refusal is a
    /// note, not an error: no undock waits on the repair shop.
    pub fn repair_ship(&mut self) -> Result<String> {
        let station = self.docked_at("repair")?;
        let ship = self.active_ship("repair")?;
        let params = PyValue::Tuple(vec![
            PyValue::Int(station),
            PyValue::Int(station),
            PyValue::Int(crate::GROUP_STATION),
        ]);
        let asked = self.moniker_call(
            "repairSvc",
            params.clone(),
            "GetRepairQuotes",
            vec![PyValue::List(vec![PyValue::Int(ship)])],
            vec![],
        )?;
        if let Some(key) = asked.refusal() {
            return Ok(format!("repair quote refused: {key}"));
        }
        let quotes = asked
            .value
            .as_ref()
            .map(repair_quotes_of)
            .unwrap_or_default();
        let wallet = self.cash_balance()?;
        match plan_repair(&quotes, wallet) {
            RepairPlan::Whole => Ok(String::new()),
            RepairPlan::TooPoor { total, wallet } => {
                let Some(pay) = plan_part_pay(&quotes, ship, wallet) else {
                    return Ok(format!(
                        "repair: undocking damaged, {total:.0} ISK needed, {wallet:.0} held"
                    ));
                };
                let done = self.moniker_call(
                    "repairSvc",
                    params,
                    "RepairItemsInStation",
                    vec![PyValue::List(vec![PyValue::Int(ship)]), PyValue::Real(pay)],
                    vec![],
                )?;
                Ok(match done.refusal() {
                    None => format!("part-repaired the hull for {pay:.0} of {total:.0} ISK"),
                    Some(key) => format!("repair refused: {key}"),
                })
            }
            RepairPlan::Pay { ids, total } => {
                let done = self.moniker_call(
                    "repairSvc",
                    params,
                    "RepairItemsInStation",
                    vec![
                        PyValue::List(ids.into_iter().map(PyValue::Int).collect()),
                        PyValue::Real(total),
                    ],
                    vec![],
                )?;
                Ok(match done.refusal() {
                    None => format!("repaired for {total:.0} ISK"),
                    Some(key) => format!("repair refused: {key}"),
                })
            }
        }
    }

    /// [`Self::repair_ship`] then [`Self::undock`], with the repair's note.
    /// Every bot undock goes through here (a source scan test holds that):
    /// docking no longer mends a hull, so a bot pays the repair shop first.
    /// A repair that fails for any reason but a dead socket is a note, never
    /// a stuck undock. The note is empty when nothing was damaged.
    pub fn repair_then_undock(&mut self) -> Result<(CallOutcome, String)> {
        let note = match self.repair_ship() {
            Ok(note) => note,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(e) => format!("repair failed: {e}"),
        };
        Ok((self.undock()?, note))
    }

    /// [`Self::repair_then_undock`] for the roles that keep no act log: the
    /// one line, with its cost, goes to stderr (the bots log).
    pub fn undock_repaired(&mut self) -> Result<CallOutcome> {
        let (outcome, note) = self.repair_then_undock()?;
        if !note.is_empty() {
            eprintln!("bot undock: {note}");
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An order row reads by column name: `price` is `DBTYPE_CY` (ISK x
    /// 10,000), `volRemaining` a real; an id in the exchange band is an NPC.
    #[test]
    fn an_order_row_reads_by_name_and_knows_an_npc_by_its_id() {
        use eve_marshal::{Column, DbRow, DbRowDescriptor, DbType};
        let columns = [
            ("bid", DbType::Bool),
            ("volRemaining", DbType::R5),
            ("price", DbType::Cy),
            ("stationID", DbType::I8),
            ("typeID", DbType::I4),
            ("orderID", DbType::I8),
        ];
        let descriptor = DbRowDescriptor::new(
            columns
                .iter()
                .map(|&(n, t)| Column {
                    name: n.as_bytes().to_vec(),
                    db_type: t,
                })
                .collect(),
        )
        .expect("a layout");
        let row = |id: i64| {
            PyValue::DbRow(Box::new(
                DbRow::new(
                    descriptor.clone(),
                    vec![
                        DbValue::Bool(true),
                        DbValue::Real(7.0),
                        DbValue::Int(125_000),
                        DbValue::Int(69_000_001),
                        DbValue::Int(990_001),
                        DbValue::Int(id),
                    ],
                )
                .expect("a row"),
            ))
        };
        let player = BookOrder::from_value(&row(5_000_000_001)).expect("reads");
        assert_eq!(
            player,
            BookOrder {
                order_id: 5_000_000_001,
                type_id: 990_001,
                station_id: 69_000_001,
                price: 12.5,
                vol_remaining: 7,
                bid: true,
            }
        );
        assert!(!player.is_npc());
        let npc = eve_server::econ_exchange::NPC_ORDER_BASE + 3;
        assert!(BookOrder::from_value(&row(npc)).expect("reads").is_npc());
        let book = Book::from_value(&PyValue::Tuple(vec![
            PyValue::List(vec![]),
            PyValue::List(vec![row(npc), row(5_000_000_001)]),
        ]))
        .expect("two halves");
        assert_eq!(book.bids.len(), 2);
        assert_eq!(book.npc_floor(), Some(12.5));
        assert_eq!(book.npc_ceiling(), None);
    }

    #[test]
    fn a_selection_row_reads_by_attribute() {
        let row = eve_server::pyutil::keyval(vec![
            ("characterID", PyValue::Int(90_000_002)),
            ("characterName", PyValue::Unicode("Miner1".to_owned())),
            ("shipTypeID", PyValue::Int(670)),
            ("stationID", PyValue::Int(60_012_526)),
            ("solarSystemID", PyValue::None),
        ]);
        assert_eq!(
            CharacterSlot::from_row(&row),
            Some(CharacterSlot {
                character_id: 90_000_002,
                name: "Miner1".to_owned(),
                ship_type_id: Some(670),
                station_id: Some(60_012_526),
                solar_system_id: None,
            })
        );
    }

    #[test]
    fn a_mining_stop_carries_its_reason() {
        let args = vec![
            PyValue::Int(100_000_061),
            PyValue::Int(67),
            PyValue::Int(0),
            PyValue::Int(0),
            PyValue::Int(0),
            PyValue::List(vec![
                PyValue::Int(100_000_061),
                PyValue::Int(90_000_001),
                PyValue::Int(100_000_001),
                PyValue::Int(70_000_000),
                PyValue::None,
                PyValue::List(vec![]),
                PyValue::Int(67),
            ]),
            PyValue::None,
            PyValue::Int(60_000),
            PyValue::Int(1000),
            PyValue::Tuple(vec![bstr("MiningItemDepleted"), PyValue::Dict(vec![])]),
            PyValue::None,
        ];
        let e = GodmaEffect::from_push("OnGodmaShipEffect", &args).expect("reads");
        assert!(!e.start);
        assert_eq!(e.target, Some(70_000_000));
        assert_eq!(e.error.as_deref(), Some("MiningItemDepleted"));
        assert_eq!(GodmaEffect::from_push("OnTarget", &args), None);
    }

    #[test]
    fn an_invite_names_the_fleet_and_the_inviter() {
        let args = vec![
            PyValue::Int(5),
            PyValue::Int(90_000_001),
            bstr("AskJoinFleet"),
            PyValue::Dict(vec![]),
        ];
        assert_eq!(
            FleetInvite::from_push("OnFleetInvite", &args),
            Some(FleetInvite {
                fleet_id: 5,
                inviter_id: 90_000_001
            })
        );
    }

    #[test]
    fn an_agent_turn_reads_its_buttons_and_flags() {
        let info = eve_server::pyutil::pydict(vec![
            ("missionCompleted", PyValue::Int(1)),
            ("missionDeclined", PyValue::Int(0)),
            ("missionQuit", PyValue::Int(0)),
            ("missionCantReplay", PyValue::Int(0)),
            ("loyaltyPoints", PyValue::Int(0)),
        ]);
        let v = PyValue::Tuple(vec![
            PyValue::Tuple(vec![
                PyValue::Tuple(vec![PyValue::Int(5), PyValue::Int(999_001)]),
                PyValue::List(vec![
                    PyValue::Tuple(vec![PyValue::Int(4), PyValue::Int(BUTTON_COMPLETE)]),
                    PyValue::Tuple(vec![PyValue::Int(5), bstr("a link")]),
                ]),
            ]),
            info,
        ]);
        let t = AgentTurn::from_value(&v).expect("reads");
        assert_eq!(t.content_id, Some(999_001));
        assert_eq!(
            t.actions,
            vec![(4, BUTTON_COMPLETE)],
            "links are not buttons"
        );
        assert_eq!(t.action_for(BUTTON_COMPLETE), Some(4));
        assert_eq!(t.action_for(BUTTON_ACCEPT), None);
        assert!(t.completed && !t.declined && !t.quit);
        assert_eq!(AgentTurn::from_value(&PyValue::None), None);
    }

    #[test]
    fn a_transport_objective_reads_as_a_courier_leg() {
        use eve_server::pyutil::pydict;
        let place = |station, system| {
            pydict(vec![
                ("solarsystemID", PyValue::Int(system)),
                ("typeID", PyValue::Int(1)),
                ("locationID", PyValue::Int(station)),
            ])
        };
        let transport = PyValue::Tuple(vec![
            bstr("transport"),
            PyValue::Tuple(vec![
                PyValue::Int(7),
                place(61, 31),
                PyValue::Int(8),
                place(62, 32),
                pydict(vec![
                    ("hasCargo", PyValue::Int(1)),
                    ("quantity", PyValue::Int(3)),
                    ("typeID", PyValue::Int(44)),
                    ("volume", PyValue::Real(1.0)),
                ]),
            ]),
        ]);
        let info = |objectives| {
            pydict(vec![
                ("objectives", PyValue::List(objectives)),
                ("contentID", PyValue::Int(999_002)),
            ])
        };
        assert_eq!(
            CourierLeg::from_objective(&info(vec![transport])),
            Some(CourierLeg {
                mission_id: 999_002,
                pickup_station: 61,
                pickup_system: 31,
                dropoff_station: 62,
                dropoff_system: 32,
                cargo_type: 44,
                quantity: 3,
            })
        );
        assert_eq!(
            CourierLeg::from_objective(&info(vec![])),
            None,
            "offered only"
        );
    }

    #[test]
    fn a_damaged_quote_is_paid_when_the_wallet_covers_it() {
        let q = |id, damage, unit| RepairQuote {
            item_id: id,
            damage,
            unit_cost: unit,
        };
        let quotes = [q(11, 10.2, 2.0), q(12, 0.0, 2.0), q(13, 5.0, 1.0)];
        assert_eq!(
            plan_repair(&quotes, 1_000.0),
            RepairPlan::Pay {
                ids: vec![11, 13],
                total: 22.0 + 5.0
            }
        );
        assert_eq!(
            plan_repair(&quotes, 10.0),
            RepairPlan::TooPoor {
                total: 27.0,
                wallet: 10.0
            }
        );
        assert_eq!(plan_repair(&quotes[1..2], 0.0), RepairPlan::Whole);
        assert_eq!(plan_repair(&[], 0.0), RepairPlan::Whole);
        // Part-pay: half the wallet, capped at the hull's own bill.
        assert_eq!(plan_part_pay(&quotes, 11, 10.0), Some(5.0));
        assert_eq!(plan_part_pay(&quotes, 11, 1_000.0), Some(22.0));
        assert_eq!(plan_part_pay(&quotes, 11, 1.0), None);
        assert_eq!(plan_part_pay(&quotes, 12, 100.0), None);
    }

    /// Every bot undock repairs first: outside the helper's own file, the
    /// act driver (which repairs, then undocks with its modules online) and
    /// the arena harness (fresh hulls), no non-test source calls the bare
    /// `.undock()`.
    #[test]
    fn every_bot_undock_goes_through_the_repairing_helper() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut bare = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("src") {
            let path = entry.expect("entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".rs")
                || ["lib.rs", "verbs.rs", "arena.rs", "driver.rs"].contains(&name)
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("reads");
            let live = text.split("#[cfg(test)]").next().unwrap_or("");
            for (n, line) in live.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if code.contains(".undock()") || code.contains(".undock_with(") {
                    bare.push(format!("{name}:{}", n + 1));
                }
            }
        }
        // The cyno alt (capital.rs) names its online module and repairs first.
        let cap = std::fs::read_to_string(dir.join("capital.rs")).expect("capital");
        let at = cap.find("c.undock_with(").expect("the cyno undock");
        assert!(
            cap[..at]
                .rfind("c.repair_ship()")
                .is_some_and(|r| at - r < 400)
        );
        bare.retain(|b| !b.starts_with("capital.rs:"));
        assert!(
            bare.is_empty(),
            "bare undock, use undock_repaired: {bare:?}"
        );
        let driver = std::fs::read_to_string(dir.join("driver.rs")).expect("driver");
        let at = driver.find("Act::Undock =>").expect("the undock arm");
        let arm = &driver[at..];
        assert!(
            arm.find("repair_ship").expect("repairs") < arm.find("undock_with").expect("undocks"),
            "the driver repairs before it undocks"
        );
    }
}
