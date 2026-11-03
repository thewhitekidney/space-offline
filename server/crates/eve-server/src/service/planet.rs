//! `planetMgr`: the planetary-interaction window's whole remote surface.
//!
//! # The surface, counted rather than named
//!
//! `docs/port-tracker.md` §10.5 warns that service names mislead about twice
//! over in both directions, and PI is the worst case in the tree:
//!
//! - `findings/client-calls.tsv` lists **38** `planetMgr` method names.
//! - `docs/reference/bound-objects.md` §"Where this document overcounted" already
//!   corrected that to **19**, and `findings/bound-objects.json` holds those
//!   19. The other nineteen are methods of the client's **own** `ClientPlanet`
//!   / `BaseColony` / `BasePin` objects -- `GetColony`, `GetPin`,
//!   `RemovePin`, `CreateRoute`, `InstallProgram`, `SetSchematic`,
//!   `GetPlanetRadius`, `IsInEditMode`, `RevertChanges`, `StartTicking` and
//!   the rest. A server that implemented them would implement calls the
//!   client never makes.
//! - Plus **3 unbound** `sm.RemoteSvc('planetMgr')` calls:
//!   `GetPlanetsForChar`, `GetMyLaunchesDetails`, `DeleteLaunch`.
//!
//! So: 22 real entry points, 38 names, and the difference is entirely
//! client-local methods on a *bound* object -- which is exactly the failure
//! mode §10.5 describes. **Use `findings/bound-objects.json`, not the TSV**,
//! for this service.
//!
//! Of the 22, this module answers the 14 a player reaches: the reads, the
//! submit (`UserUpdateNetwork`), abandon, launch, expedited transfer, the
//! launches list and the heatmap. The eight `GM*` staff tools are left out,
//! and a test pins them. `docs/lanes/planet.md` §10 (P5.3) has what each
//! write does and which of its parts are `[A]`.
//!
//! Since W9 it also serves the **customs offices** (`docs/lanes/planet.md`
//! §11): `planetOrbitalRegistryBroker.GetTaxRate`, and -- through one hook
//! at the top of `service/inventory.rs` -- the office's own inventory and
//! `ImportExportWithPlanet` on it ([`customs_inventory_call`]).
//!
//! # A bound object, per planet
//!
//! `eveMoniker.GetPlanet(planetID)` is `Moniker('planetMgr', planetID)`
//! (`eve/common/script/net/eveMoniker.py:244`), bound once in
//! `ClientPlanet.Init`. **So `req.bind_params` is the planet id**, and it is
//! the only place most of these calls carry one -- `GetPlanetInfo` takes
//! *zero* arguments and means "the planet you bound me to".
//!
//! # Why the simulation is not here
//!
//! It is in [`crate::planet`], which knows nothing about the wire and holds
//! no `PyValue`. The split is the same one `Registry` already keeps: an arm
//! asks the world a question and shapes the answer. It matters more than
//! usual here because the simulation is the part that was ported from CCP's
//! own bytecode and the part worth testing at a dozen clock values, and
//! neither of those wants a marshal type in the way.

use super::*;

/// `groupPlanetaryLinks` (1036) holds exactly one row: typeID 2280, "Link".
///
/// Passed into the command stream rather than read from `Statics`, because a
/// `CREATELINK` does not name a type -- the client sends only the endpoints
/// and a level, so the server has to know which type a link is. **[V]**:
/// `tools/pi_statics.py` reads group 1036 out of `types.fsdbinary` through
/// its confirmed columns and it holds exactly this one id, 2280 "Link".
const TYPE_PLANETARY_LINK: i64 = 2280;

/// `appConst.MAX_DISPLAY_QUALITY = planetResourceMaxValue * 255 * 0.5`,
/// with `planetResourceMaxValue = 1.21` **[V]** (`appConst.pyc:1888-1891`).
/// `resourceController.CreateLayout` divides each `GetPlanetResourceInfo`
/// quality by this to draw its bar, so this is the top of the scale the
/// client expects those numbers on.
const MAX_DISPLAY_QUALITY: f64 = 1.21 * 255.0 * 0.5;

/// `const.flagCargo`, `flagSpecializedCommandCenterHold`,
/// `flagColonyResourcesHold` -- the three holds `ClientPlanet.CreateCommandPin`
/// searches for the command centre it is about to place, in that order of
/// listing **[V]** (`inventorycommon/const.pyc`: 5, 148, 185;
/// `clientPlanet.pyc:282-284`).
const COMMAND_CENTRE_HOLDS: [i64; 3] = [5, 148, 185];

/// `typeInterplanetaryConsolidation` 2495 and `typeCommandCenterUpgrade`
/// 2505, `inventorycommon/const.pyc` **[V]**; `typeOmnipotent` 19430 is the
/// GM skill that waives the first.
const SKILL_INTERPLANETARY_CONSOLIDATION: i64 = 2495;
const SKILL_COMMAND_CENTER_UPGRADE: i64 = 2505;
const SKILL_OMNIPOTENT: i64 = 19430;

/// `refPlanetaryConstruction`, entry type 98 "Planetary Construction" in
/// `accountingentrytypes.fsdbinary` **[V]** (`tools/fsd.py
/// accounting_entry_types`, which also confirms 96 import / 97 export tax).
const REF_PLANETARY_CONSTRUCTION: i64 = 98;

/// `refPlanetaryImportTax` 96 and `refPlanetaryExportTax` 97, the same table
/// and the same confirmation **[V]**.
const REF_PLANETARY_IMPORT_TAX: i64 = 96;
const REF_PLANETARY_EXPORT_TAX: i64 = 97;

pub(crate) const SERVICES: &[&str] = &["planetMgr", "planetOrbitalRegistryBroker"];

/// The planet this call is about.
///
/// `MachoBindObject`'s params for `planetMgr` are a bare `planetID`, so for
/// most methods this is the whole address. A few also accept it positionally
/// -- `ClientPlanet` passes it explicitly where it holds several planets --
/// so an explicit first argument wins over the binding, which is the order
/// EvEJS settled on too (`planetMgrService.js:1125 _resolvePlanetID`).
fn planet_id(req: &Request<'_>, allow_args: bool) -> Option<i64> {
    if allow_args
        && let Some(PyValue::Int(n)) = req.args.first()
        && *n > 0
    {
        return Some(*n);
    }
    match req.bind_params {
        Some(PyValue::Int(n)) if *n > 0 => Some(*n),
        // A bound object can also carry its params as a 1-tuple; accept both
        // rather than depending on which shape the resolve took.
        Some(PyValue::Tuple(parts)) => match parts.first() {
            Some(PyValue::Int(n)) if *n > 0 => Some(*n),
            _ => None,
        },
        _ => None,
    }
}

/// `eveexceptions.UserError`, with a message id the client itself raises.
///
/// Every [`crate::planet::PiError`] is named after one of `baseColony`'s own
/// `UserError` ids, and the PI window pre-validates against the same list --
/// so a refusal under the right name is a message the player can act on and
/// a refusal under the wrong one reads as the window being broken.
///
/// Returning a value where the client expects a raise is the "fifth kind of
/// empty" of `docs/port-tracker.md` §4, and
/// `a_simulation_refusal_becomes_the_user_error_the_client_names` pins it.
fn pi_error(e: crate::planet::PiError) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(e.message_id(), vec![]))
}

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

/// What the world knows about a planet, from the celestial catalogue.
struct PlanetFacts {
    solar_system_id: i64,
    type_id: i64,
    /// Metres.
    radius: f64,
    celestial_index: i64,
}

/// The planet's system, type, radius and roman numeral, read from the
/// celestial extract (`--celestials`, `crate::celestial`) **[V]** for the
/// first three -- they are CCP's own numbers out of
/// `solarsystemcontent.static`.
///
/// `celestialIndex` is **[A]**: the planet's 1-based rank among its system's
/// planets ordered by item id. CCP numbers planets outward from the star and
/// allocates their ids in the same order, so the two agree in every system
/// we have looked at, but the extract does not carry the index itself. The
/// client uses it for the numeral after the planet's name, nothing else.
fn planet_facts(reg: &Registry, planet_id: i64) -> Option<PlanetFacts> {
    let catalogue = reg.celestials();
    let c = catalogue.get(planet_id)?;
    if c.kind != crate::celestial::Kind::Planet {
        return None;
    }
    let system = catalogue.system_of(planet_id).unwrap_or(0);
    let mut planets: Vec<i64> = catalogue
        .served_in(system)
        .filter(|p| p.kind == crate::celestial::Kind::Planet)
        .map(|p| p.id)
        .collect();
    planets.sort_unstable();
    let index = planets
        .iter()
        .position(|id| *id == planet_id)
        .map_or(0, |i| i as i64 + 1);
    Some(PlanetFacts {
        solar_system_id: system,
        type_id: c.type_id,
        radius: c.radius,
        celestial_index: index,
    })
}

// --- decoding what the client sends ---------------------------------------

fn as_int(v: &PyValue) -> Option<i64> {
    match v {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn as_real(v: &PyValue) -> Option<f64> {
    match v {
        PyValue::Real(r) if r.is_finite() => Some(*r),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

fn as_seq(v: &PyValue) -> Option<&[PyValue]> {
    match v {
        PyValue::Tuple(items) | PyValue::List(items) => Some(items),
        _ => None,
    }
}

/// A pin or route id as the stream carries it: an int, or **any tuple** --
/// `BaseColony.IsTemporaryID` is `type(id) == types.TupleType` and nothing
/// else **[V]**. The client mints `(1, n)` for pins and `(2, n)` for routes
/// (`clientColony.GetTemporaryPinID`/`GetTemporaryRouteID`), but the shape
/// is opaque to us and is carried verbatim.
fn as_id(v: &PyValue) -> Option<crate::planet::Id> {
    match v {
        PyValue::Int(n) => Some(crate::planet::Id::Real(*n)),
        PyValue::Tuple(items) => items
            .iter()
            .map(as_int)
            .collect::<Option<Vec<_>>>()
            .map(crate::planet::Id::Temporary),
        _ => None,
    }
}

/// `{typeID: quantity}`, as the launch and transfer calls send it.
fn as_commodities(v: &PyValue) -> Option<std::collections::BTreeMap<i64, i64>> {
    let PyValue::Dict(entries) = v else {
        return None;
    };
    // `(value, key)`: the marshal dict's pair order, `pyutil::pairs_get`.
    entries
        .iter()
        .map(|(qty, type_id)| Some((as_int(type_id)?, as_int(qty)?)))
        .collect()
}

/// `pin.heads` as the client holds it: `(headID, latitude, longitude)`
/// tuples (`EcuPin.AddHead`, and `FindHead` reads `head[0]`) **[V]**.
fn as_heads(v: &PyValue) -> Option<Vec<crate::planet::Head>> {
    as_seq(v)?
        .iter()
        .map(|h| {
            let h = as_seq(h)?;
            Some(crate::planet::Head {
                head_id: as_int(h.first()?)?,
                latitude: as_real(h.get(1)?)?,
                longitude: as_real(h.get(2)?)?,
            })
        })
        .collect()
}

/// `CommandStream.Serialize`'s output: a list of `(commandID, argsTuple)`
/// pairs, one per command, with the per-opcode argument tuples
/// `docs/lanes/planet.md` §3.2 tabulates **[V]**. Any malformed entry fails
/// the whole stream: half a stream is a colony the player did not build.
fn decode_stream(v: &PyValue) -> Result<Vec<crate::planet::Command>, String> {
    use crate::planet::{Command, SurfacePoint};
    let entries = as_seq(v).ok_or("the stream is not a list")?;
    let mut out = Vec::with_capacity(entries.len());
    for (n, entry) in entries.iter().enumerate() {
        let bad = || format!("command {n} is malformed: {entry:?}");
        let pair = as_seq(entry).ok_or_else(bad)?;
        let opcode = pair.first().and_then(as_int).ok_or_else(bad)?;
        let a = pair.get(1).and_then(as_seq).ok_or_else(bad)?;
        let id = |i: usize| a.get(i).and_then(as_id).ok_or_else(bad);
        let int = |i: usize| a.get(i).and_then(as_int).ok_or_else(bad);
        let real = |i: usize| a.get(i).and_then(as_real).ok_or_else(bad);
        let point = |i: usize| -> Result<SurfacePoint, String> {
            Ok(SurfacePoint::new(real(i)?, real(i + 1)?))
        };
        out.push(match opcode {
            1 => Command::CreatePin {
                pin_id: id(0)?,
                type_id: int(1)?,
                point: point(2)?,
            },
            2 => Command::RemovePin { pin_id: id(0)? },
            3 => Command::CreateLink {
                endpoint1: id(0)?,
                endpoint2: id(1)?,
                level: int(2)?,
            },
            4 => Command::RemoveLink {
                endpoint1: id(0)?,
                endpoint2: id(1)?,
            },
            5 => Command::SetLinkLevel {
                endpoint1: id(0)?,
                endpoint2: id(1)?,
                level: int(2)?,
            },
            6 => Command::CreateRoute {
                route_id: id(0)?,
                path: a
                    .get(1)
                    .and_then(as_seq)
                    .ok_or_else(bad)?
                    .iter()
                    .map(|p| as_id(p).ok_or_else(bad))
                    .collect::<Result<Vec<_>, _>>()?,
                type_id: int(2)?,
                quantity: int(3)?,
            },
            7 => Command::RemoveRoute { route_id: id(0)? },
            8 => Command::SetSchematic {
                pin_id: id(0)?,
                schematic_id: int(1)?,
            },
            9 => Command::UpgradeCommandCenter {
                pin_id: id(0)?,
                level: int(1)?,
            },
            10 => Command::AddExtractorHead {
                pin_id: id(0)?,
                head_id: int(1)?,
                point: point(2)?,
            },
            11 => Command::KillExtractorHead {
                pin_id: id(0)?,
                head_id: int(1)?,
            },
            12 => Command::MoveExtractorHead {
                pin_id: id(0)?,
                head_id: int(1)?,
                point: point(2)?,
            },
            13 => Command::InstallProgram {
                pin_id: id(0)?,
                // `None` is Stop Program.
                program_type: match a.get(1) {
                    Some(PyValue::None) | None => None,
                    Some(v) => Some(as_int(v).ok_or_else(bad)?),
                },
                head_radius: real(2)?,
            },
            other => return Err(format!("command {n} has unknown opcode {other}")),
        });
    }
    Ok(out)
}

/// A character's trained level in one skill, `None` if untrained -- the
/// shape `skills.GetEffectiveLevel` answers in, which is what the client's
/// checks branch on.
fn skill_level(reg: &Registry, actor: i64, skill: i64) -> Option<i64> {
    reg.skills_for(Some(actor))?
        .entries()
        .find(|e| e.type_id == skill)
        .map(|e| e.level)
}

/// The reply to a submit and the colony half of `GetPlanetInfo`:
/// `colonyData.Serialize` **[V]** plus `currentSimTime`, which
/// `ClientPlanet._PrimeColony` reads off the same object.
fn colony_reply(
    statics: &crate::planet::Statics,
    owner_id: i64,
    colony: Option<&crate::planet::Colony>,
) -> PyValue {
    match colony {
        Some(c) => colony_keyval(statics, c),
        None => keyval(vec![
            ("ownerID", PyValue::Int(owner_id)),
            ("pins", PyValue::List(vec![])),
            ("links", PyValue::List(vec![])),
            ("routes", PyValue::List(vec![])),
            ("level", PyValue::Int(0)),
            (
                "currentSimTime",
                PyValue::Long(crate::pyutil::filetime_now()),
            ),
        ]),
    }
}

/// One pin, as `ClientPlanet.RestorePinFromRow` reads it.
///
/// A `KeyVal` and not a `DBRow`, because the field set **depends on the pin's
/// class**: `baseColony.Serialize` calls each pin's own `Serialize`, and
/// `BasePin`'s eight keys are extended by `ProcessPin` (3 more), `EcuPin`
/// (7 more) and `SpaceportPin` (1 more). A `DBRow` needs one column layout
/// for every row in a rowset, and there is no single layout here -- which is
/// why the client's own serialiser builds keyword-argument `KeyVal`s.
///
/// The eight base keys are `BasePin.Serialize`'s, verbatim and in its order:
/// `id, latitude, longitude, ownerID, lastRunTime, typeID, contents, state`.
fn pin_row(statics: &crate::planet::Statics, pin: &crate::planet::Pin) -> PyValue {
    use crate::planet::PinKind;
    let mut attrs: Vec<(&str, PyValue)> = vec![
        ("id", PyValue::Int(pin.pin_id)),
        ("latitude", PyValue::Real(pin.latitude)),
        ("longitude", PyValue::Real(pin.longitude)),
        ("ownerID", PyValue::Int(pin.owner_id)),
        ("lastRunTime", PyValue::Int(pin.last_run_time)),
        ("typeID", PyValue::Int(pin.type_id)),
        ("contents", contents_dict(&pin.contents)),
        ("state", PyValue::Int(pin.state)),
    ];
    match pin.kind(statics) {
        // `SpaceportPin.Serialize` adds one key, and `CommandPin` inherits
        // it. `lastLaunchTime` of `None` is "never launched", which the
        // client distinguishes from a time: `CommandCenterContainer`
        // compares `nextLaunchTime` against the wall clock only when it is
        // not `None`.
        Some(PinKind::Spaceport) | Some(PinKind::Command) => {
            attrs.push((
                "lastLaunchTime",
                if pin.last_launch_time == 0 {
                    PyValue::None
                } else {
                    PyValue::Int(pin.last_launch_time)
                },
            ));
        }
        // `ProcessPin.Serialize` adds three.
        Some(PinKind::Process) => {
            attrs.push((
                "schematicID",
                match pin.schematic_id {
                    Some(id) => PyValue::Int(id),
                    None => PyValue::None,
                },
            ));
            attrs.push(("hasReceivedInputs", PyValue::Bool(pin.has_received_inputs)));
            attrs.push((
                "receivedInputsLastCycle",
                PyValue::Bool(pin.received_inputs_last_cycle),
            ));
        }
        // `EcuPin.Serialize` adds seven, and `heads` is a list of
        // `(headID, latitude, longitude)` **tuples** -- the client's
        // `FindHead` does `head[0]` and `SetHeads` reads `head[1]` and
        // `head[2]`, so a dict or a KeyVal per head would raise in native
        // code.
        Some(PinKind::Ecu) | Some(PinKind::Extractor) => {
            attrs.push(("cycleTime", PyValue::Int(pin.cycle_time)));
            attrs.push((
                "programType",
                match pin.program_type {
                    Some(id) => PyValue::Int(id),
                    None => PyValue::None,
                },
            ));
            attrs.push(("qtyPerCycle", PyValue::Int(pin.qty_per_cycle)));
            attrs.push((
                "expiryTime",
                match pin.expiry_time {
                    Some(t) => PyValue::Int(t),
                    None => PyValue::None,
                },
            ));
            attrs.push((
                "installTime",
                match pin.install_time {
                    Some(t) => PyValue::Int(t),
                    None => PyValue::None,
                },
            ));
            attrs.push(("headRadius", PyValue::Real(pin.head_radius)));
            attrs.push((
                "heads",
                PyValue::List(
                    pin.heads
                        .iter()
                        .map(|h| {
                            PyValue::Tuple(vec![
                                PyValue::Int(h.head_id),
                                PyValue::Real(h.latitude),
                                PyValue::Real(h.longitude),
                            ])
                        })
                        .collect(),
                ),
            ));
        }
        _ => {}
    }
    keyval(attrs)
}

/// `pin.contents` -- an **int-keyed** dict of `typeID -> quantity`.
///
/// `pyutil::pydict` cannot build this: its keys are `&str`, and
/// `{'2268': 3000}` is not `{2268: 3000}` to a client that subscripts with an
/// integer typeID. It would miss every lookup and return the default, which
/// is the one failure mode with no symptom at all -- the same trap
/// `service/industry.rs::int_keyed_dict` exists for.
///
/// The `(value, key)` ordering is the marshal dict's, not a typo.
fn contents_dict(contents: &std::collections::BTreeMap<i64, i64>) -> PyValue {
    PyValue::Dict(
        contents
            .iter()
            .map(|(type_id, qty)| (PyValue::Int(*qty), PyValue::Int(*type_id)))
            .collect(),
    )
}

/// `baseColony.Serialize` -- the six keys `ClientPlanet._PrimeColony` reads.
///
/// ```text
///     ownerID, pins, links, routes, level, currentSimTime
/// ```
///
/// **`links` are `KeyVal`s** with `typeID, endpoint1, endpoint2, level`:
/// `ColonyData.RestoreLinkFromRow` reads all four **by attribute**
/// (`colonyData.pyc:620-624`) and `BaseLink.Serialize` builds exactly that
/// `KeyVal` **[V]**. This said "2-element lists, unpacked positionally" and
/// sent `[endpoint1, endpoint2, level]` -- an `AttributeError: 'list' object
/// has no attribute 'typeID'` in `_PrimeColony` the first time any colony
/// with a link was opened, found by disassembling the reader for this
/// packet. `routes` are `KeyVal`s too, because `RestoreRouteFromRow` reads
/// `.routeID`, `.path`, `.commodityTypeID` and `.commodityQuantity`.
fn colony_keyval(statics: &crate::planet::Statics, colony: &crate::planet::Colony) -> PyValue {
    keyval(vec![
        ("ownerID", PyValue::Int(colony.owner_id)),
        (
            "pins",
            PyValue::List(colony.pins.iter().map(|p| pin_row(statics, p)).collect()),
        ),
        (
            "links",
            PyValue::List(
                colony
                    .links
                    .iter()
                    .map(|l| {
                        keyval(vec![
                            ("typeID", PyValue::Int(l.type_id)),
                            ("endpoint1", PyValue::Int(l.endpoint1)),
                            ("endpoint2", PyValue::Int(l.endpoint2)),
                            ("level", PyValue::Int(l.level)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "routes",
            PyValue::List(
                colony
                    .routes
                    .iter()
                    .map(|r| {
                        keyval(vec![
                            ("routeID", PyValue::Int(r.route_id)),
                            ("charID", PyValue::Int(r.owner_id)),
                            (
                                "path",
                                PyValue::List(r.path.iter().map(|p| PyValue::Int(*p)).collect()),
                            ),
                            ("commodityTypeID", PyValue::Int(r.commodity_type_id)),
                            ("commodityQuantity", PyValue::Int(r.commodity_quantity)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("level", PyValue::Int(colony.level)),
        ("currentSimTime", PyValue::Int(colony.current_sim_time)),
    ])
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        "planetMgr" => match req.method {
            // **The unbound call, and the only one that takes no planet at
            // all.** `PlanetService.GetMyPlanets` caches it in
            // `self.colonizationData` and re-asks only while that is `None`.
            //
            // **It must be a plain list, not a `CRowset`** -- and this is a
            // disagreement with EvEJS that our client decides.
            // `planetSvc._UpdateColonyPresence` does, at
            // `planetSvc.py:172-208`:
            //
            // ```text
            //     self.colonizationData = [c for c in self.colonizationData
            //                              if c.planetID != planetID]
            //     ...
            //     newEntry = utillib.KeyVal(solarSystemID=..., planetID=...,
            //                               typeID=..., numberOfPins=...,
            //                               celestialIndex=...)
            //     self.colonizationData.append(newEntry)
            // ```
            //
            // So the container is iterated, its rows are read by attribute,
            // **and it is appended to** -- and a `dbutil.CRowset` has no
            // `.append`. EvEJS returns one
            // (`planetMgrService.js:66 PLANETS_FOR_CHAR_ROWSET_NAME =
            // 'carbon.common.script.sys.crowset.CRowset'`), which would raise
            // `AttributeError` the first time a player deploys a command
            // centre on a planet that was not already in the list -- i.e. on
            // every first colony. A list of `KeyVal`s satisfies all three
            // uses, and it is the family the client itself appends.
            //
            // The five fields are the five the client reads and the five it
            // constructs, in its own keyword order. **[V]** from the
            // `co_names` and `co_consts` of `_UpdateColonyPresence`.
            //
            // Empty is the honest answer for a character with no colonies,
            // and it caches correctly: the guard is `is None`, so `[]` is
            // stored and not re-asked.
            "GetPlanetsForChar" => {
                let Some(actor) = req.actor else {
                    // No character selected. The call is only reachable from
                    // `PlanetService`, which runs in game -- but an empty
                    // list is the same answer a character with no colonies
                    // gets, and inventing one from `characters.first()` is
                    // the bug `Request::actor` exists to prevent.
                    return Ok(PyValue::List(vec![]));
                };
                let rows = reg
                    .planet
                    .for_owner(actor)
                    .into_iter()
                    .map(|c| {
                        let facts = planet_facts(reg, c.planet_id);
                        keyval(vec![
                            (
                                "solarSystemID",
                                PyValue::Int(facts.as_ref().map_or(0, |f| f.solar_system_id)),
                            ),
                            ("planetID", PyValue::Int(c.planet_id)),
                            ("typeID", PyValue::Int(c.planet_type_id)),
                            ("numberOfPins", PyValue::Int(c.pins.len() as i64)),
                            (
                                "celestialIndex",
                                PyValue::Int(facts.as_ref().map_or(0, |f| f.celestial_index)),
                            ),
                        ])
                    })
                    .collect();
                Ok(PyValue::List(rows))
            }
            // `ClientPlanet.PreparePlanet` reads five keys off this --
            // `solarSystemID`, `planetTypeID`, `radius`, `celestialIndex` and
            // the colony's own keys when the character has one.
            //
            // **`radius` must be a Python float.** The client's template
            // writer computes `Diam = planet.radius * 2 / 1000` under Python
            // 2 integer division, and `eveplanet/verification.py::
            // _VerifyDiamater` rejects a non-float outright with
            // `PITemplateVerificationFailed`. Every SDE planet radius is
            // integer-valued, so a bare number would always marshal as
            // `PyInt` and always fail. EvEJS found this the hard way and
            // wrote it down at `planetMgrService.js:389-393`; it is one of
            // the few notes of theirs worth taking verbatim.
            //
            // `solarSystemID` and `celestialIndex` are `0` because the
            // map chain is not in the server yet (`service.rs` still flags
            // the system/constellation/region chain **[I]**). They are honest
            // zeros, not placeholders to be forgotten: the PI window uses
            // them only for the "which system" label and the roman numeral
            // after the planet's name.
            "GetPlanetInfo" => {
                let Some(planet) = planet_id(req, true) else {
                    return Err(ServiceError::BadArguments {
                        service: req.service.to_owned(),
                        method: req.method.to_owned(),
                        why: "planetMgr is bound per planet; no planetID in the \
                              bind params or the arguments"
                            .to_owned(),
                    });
                };
                let now = crate::pyutil::filetime_now();
                // From the celestial extract when the server has one, which is
                // what makes an **uncolonised** planet openable: with zeros the
                // client templates a 0 m sphere of type 0. A colony's own
                // record overrides type and radius below, so a colony stays
                // readable on a server started without `--celestials`.
                let facts = planet_facts(reg, planet);
                let mut attrs = vec![
                    ("planetID", PyValue::Int(planet)),
                    (
                        "solarSystemID",
                        PyValue::Int(facts.as_ref().map_or(0, |f| f.solar_system_id)),
                    ),
                    (
                        "planetTypeID",
                        PyValue::Int(facts.as_ref().map_or(0, |f| f.type_id)),
                    ),
                    (
                        "radius",
                        PyValue::Real(facts.as_ref().map_or(0.0, |f| f.radius)),
                    ),
                    (
                        "celestialIndex",
                        PyValue::Int(facts.as_ref().map_or(0, |f| f.celestial_index)),
                    ),
                ];
                if let Some(actor) = req.actor {
                    let statics = crate::planet::statics_in(reg.content()).clone();
                    reg.planet.advance(&statics, planet, actor, now);
                    if let Some(colony) = reg.planet.get(planet, actor) {
                        attrs[2] = ("planetTypeID", PyValue::Int(colony.planet_type_id));
                        attrs[3] = ("radius", PyValue::Real(colony.planet_radius));
                        // The colony's own six keys are merged into the same
                        // `KeyVal`, not nested: `_PrimeColony` is handed this
                        // object and reads `.pins` off it directly.
                        let PyValue::Instance { state, .. } = colony_keyval(&statics, colony)
                        else {
                            unreachable!("colony_keyval builds a KeyVal")
                        };
                        let PyValue::Dict(colony_pairs) = *state else {
                            unreachable!("a KeyVal's state is its attribute dict")
                        };
                        // `_PrimeColony` is handed this one object and reads
                        // `.pins` off it directly, so the colony's keys are
                        // merged into the planet's rather than nested.
                        let mut out = crate::pyutil::pypairs(attrs);
                        out.extend(colony_pairs);
                        return Ok(PyValue::Instance {
                            class: crate::pyutil::KEYVAL_CLASS.as_bytes().to_vec(),
                            state: Box::new(PyValue::Dict(out)),
                        });
                    }
                }
                Ok(keyval(attrs))
            }
            // `{resourceTypeID: quality}` for this planet, read by
            // `surveyUI.ApplyAttributes` and by the planetary-production
            // info panel.
            //
            // **Int-keyed**, for the reason on `contents_dict`.
            //
            // Empty without `--planets`, and empty is survivable: the survey
            // window renders a planet with no known resources rather than
            // raising.
            //
            // The qualities are `globaltypes`' per-planet-*type* distribution
            // **[V]**, scaled onto the `[0, MAX_DISPLAY_QUALITY]` range the
            // window divides by **[A]**: Tranquility answers a per-*planet*
            // figure from its resource field, which we do not have, so every
            // planet of a type reads the same. A resource the type lacks is
            // left out rather than sent as zero, which is how a planet that
            // genuinely has none of it reads.
            "GetPlanetResourceInfo" => {
                let planet = planet_id(req, true).unwrap_or(0);
                let planet_type = planet_facts(reg, planet).map(|f| f.type_id).or_else(|| {
                    reg.planet
                        .on_planet(planet)
                        .first()
                        .map(|c| c.planet_type_id)
                });
                let Some(qualities) =
                    planet_type.and_then(|t| crate::planet::statics_in(reg.content()).resources(t))
                else {
                    return Ok(PyValue::Dict(vec![]));
                };
                Ok(PyValue::Dict(
                    qualities
                        .iter()
                        .filter(|(_, q)| **q > 0.0)
                        .map(|(t, q)| (PyValue::Real(q * MAX_DISPLAY_QUALITY), PyValue::Int(*t)))
                        .collect(),
                ))
            }
            // `planetSvc.GetColonyForCharacter` unpacks **two** values:
            //
            // ```text
            //     pins, links = planet.GetFullNetworkForOwner(planetID, ownerID)
            // ```
            //
            // so arity is not negotiable, and both halves are iterated. The
            // links are bare `[endpoint1, endpoint2]` pairs here rather than
            // the 3-element form `baseColony.Serialize` uses -- the client
            // unpacks two names from each.
            "GetFullNetworkForOwner" => {
                let planet = planet_id(req, true).unwrap_or(0);
                let owner = match req.args.get(1) {
                    Some(PyValue::Int(n)) => *n,
                    // The second argument is the owner. Falling back to the
                    // actor is right: the only caller passes
                    // `session.charid`.
                    _ => req.actor.unwrap_or(0),
                };
                if owner == 0 {
                    return Ok(PyValue::Tuple(vec![
                        PyValue::List(vec![]),
                        PyValue::List(vec![]),
                    ]));
                }
                let now = crate::pyutil::filetime_now();
                let statics = crate::planet::statics_in(reg.content()).clone();
                reg.planet.advance(&statics, planet, owner, now);
                let Some(colony) = reg.planet.get(planet, owner) else {
                    return Ok(PyValue::Tuple(vec![
                        PyValue::List(vec![]),
                        PyValue::List(vec![]),
                    ]));
                };
                Ok(PyValue::Tuple(vec![
                    PyValue::List(colony.pins.iter().map(|p| pin_row(&statics, p)).collect()),
                    PyValue::List(
                        colony
                            .links
                            .iter()
                            .map(|l| {
                                PyValue::List(vec![
                                    PyValue::Int(l.endpoint1),
                                    PyValue::Int(l.endpoint2),
                                ])
                            })
                            .collect(),
                    ),
                ]))
            }
            // Every character's command centre on this planet, so the
            // planet-view can draw other people's colonies.
            //
            // **Not filtered to the actor**: that is the whole point of the
            // call, and it is the one place PI is multiplayer-visible.
            // `planetSvc.GetPlanetCommandPins` iterates it.
            "GetCommandPinsForPlanet" => {
                let planet = planet_id(req, true).unwrap_or(0);
                let statics = crate::planet::statics_in(reg.content()).clone();
                let rows = reg
                    .planet
                    .on_planet(planet)
                    .into_iter()
                    .filter_map(|c| c.command_pin(&statics).map(|p| (c.owner_id, p)))
                    .map(|(owner, pin)| {
                        keyval(vec![
                            ("pinID", PyValue::Int(pin.pin_id)),
                            ("ownerID", PyValue::Int(owner)),
                            ("typeID", PyValue::Int(pin.type_id)),
                            ("latitude", PyValue::Real(pin.latitude)),
                            ("longitude", PyValue::Real(pin.longitude)),
                        ])
                    })
                    .collect();
                Ok(PyValue::List(rows))
            }
            // Every extractor on the planet, whoever owns it, for the
            // resource-competition overlay. Iterated by
            // `planetSvc.GetExtractorsForPlanet`.
            "GetExtractorsForPlanet" => {
                let planet = planet_id(req, true).unwrap_or(0);
                let statics = crate::planet::statics_in(reg.content()).clone();
                let mut rows = Vec::new();
                for colony in reg.planet.on_planet(planet) {
                    for pin in &colony.pins {
                        if !pin
                            .kind(&statics)
                            .is_some_and(crate::planet::PinKind::is_extractor)
                        {
                            continue;
                        }
                        rows.push(keyval(vec![
                            ("pinID", PyValue::Int(pin.pin_id)),
                            ("ownerID", PyValue::Int(colony.owner_id)),
                            ("typeID", PyValue::Int(pin.type_id)),
                            ("latitude", PyValue::Real(pin.latitude)),
                            ("longitude", PyValue::Real(pin.longitude)),
                            ("headRadius", PyValue::Real(pin.head_radius)),
                            (
                                "programType",
                                match pin.program_type {
                                    Some(id) => PyValue::Int(id),
                                    None => PyValue::None,
                                },
                            ),
                        ]));
                    }
                }
                Ok(PyValue::List(rows))
            }
            // The survey window's quote, before the player commits:
            // `clientPlanet.py:719` unpacks **three** values.
            //
            // ```text
            //     qtyPerCycle, cycleTime, numCycles = \
            //         planet.GetProgramResultInfo(pinID, resourceTypeID, heads, headRadius)
            // ```
            //
            // `crate::planet::create_program` -- `baseColony.CreateProgram`
            // ported **[V]** -- over the heads the client sends, which are
            // the window's *edit-mode* heads and may belong to an ECU placed
            // in this same session (a tuple `pinID` the colony does not hold
            // yet). The per-head value comes from the assumed flat field
            // **[A]** (`crate::planet::UniformField`); the cycle time and the
            // cycle count are pure functions of the radius and agree with the
            // slider by construction. `INSTALLPROGRAM` quotes through the
            // same function, so the window's number is the number installed.
            "GetProgramResultInfo" => {
                let resource = req.args.get(1).and_then(as_int).unwrap_or(0);
                let heads = req.args.get(2).and_then(as_heads).unwrap_or_default();
                let radius = req
                    .args
                    .get(3)
                    .and_then(as_real)
                    .unwrap_or(crate::planet::RADIUS_DRILLAREAMIN)
                    .clamp(
                        crate::planet::RADIUS_DRILLAREAMIN,
                        crate::planet::RADIUS_DRILLAREAMAX,
                    );
                let planet = planet_id(req, false).unwrap_or(0);
                let owner = req.actor.unwrap_or(0);
                let statics = crate::planet::statics_in(reg.content()).clone();
                let planet_type = planet_facts(reg, planet)
                    .map(|f| f.type_id)
                    .or_else(|| reg.planet.get(planet, owner).map(|c| c.planet_type_id))
                    .unwrap_or(0);
                let empty = crate::planet::Colony::new(planet, owner, planet_type, 0);
                let colony = reg.planet.get(planet, owner).unwrap_or(&empty);
                let ecu_pin = req.args.first().and_then(as_int).unwrap_or(0);
                // A temporary ECU has no type the colony knows; every ECU
                // type carries the same tuning attributes (all defaults), so
                // the first one the table lists stands in for it.
                let ecu_type = colony
                    .pin(ecu_pin)
                    .map(|p| p.type_id)
                    .unwrap_or_else(|| first_ecu_type(&statics));
                let field = crate::planet::UniformField {
                    qualities: statics.resources(planet_type),
                };
                let quote = crate::planet::create_program(
                    &statics, colony, ecu_pin, ecu_type, resource, &heads, radius, &field,
                );
                Ok(PyValue::Tuple(vec![
                    PyValue::Int(quote.qty_per_cycle),
                    PyValue::Int(quote.cycle_time),
                    PyValue::Int(quote.num_cycles),
                ]))
            }
            // The survey window's resource heatmap: `ClientPlanet.GetResourceData`
            // hands `shData.data` to the native `CreateSHFromBuffer` **only
            // when it is not `None`**, and otherwise keeps the entry it made
            // with `builder.CreateConstantSH(0.0, 1)` **[V]**
            // (`clientPlanet.pyc:1062, :1139`). So `data = None` is the one
            // answer that cannot crash native code we have no format for: the
            // map draws flat, and the quote above is what the player reads.
            "GetResourceData" => Ok(keyval(vec![
                ("data", PyValue::None),
                ("numBands", PyValue::Int(0)),
                ("proximity", PyValue::None),
            ])),
            // **The one write the PI window makes.** Everything the player
            // does in edit mode -- placing pins, linking them, routing
            // commodities, upgrading the command centre, installing a
            // program -- is batched into one command stream and submitted
            // here by `ClientPlanet._SubmitChanges`, which then primes its
            // colony from the reply. See [`user_update_network`].
            "UserUpdateNetwork" => user_update_network(reg, req),
            // `ClientPlanet._SubmitChanges` calls this instead of submitting
            // when the edit removed the command centre, and ignores the
            // answer **[V]** (`clientPlanet.pyc:244`). The colony goes,
            // contents and all; there is no refund on Tranquility either.
            "UserAbandonPlanet" => {
                let (Some(actor), Some(planet)) = (req.actor, planet_id(req, false)) else {
                    return Err(bad_args(
                        req,
                        "abandon needs a character and a bound planet",
                    ));
                };
                if reg.planet.remove(planet, actor).is_some() {
                    reg.mark_dirty();
                }
                Ok(PyValue::None)
            }
            "UserLaunchCommodities" => user_launch_commodities(reg, req),
            "UserTransferCommodities" => user_transfer_commodities(reg, req),
            // The launches list. **Empty, truthfully**: a launch here lands
            // straight in a station hangar (see [`user_launch_commodities`])
            // rather than as a container in orbit, so there is never a launch
            // in space to list or to delete.
            "GetMyLaunchesDetails" => Ok(PyValue::List(vec![])),
            "DeleteLaunch" => Ok(PyValue::None),
            // The seven `GM*` methods are staff tools and are out of scope by
            // the brief; `the_write_path_is_left_unanswered_on_purpose` pins
            // them so serving one is a decision.
            _ => Err(reg.no_method(req)),
        },
        // **The customs offices' registry**, `Moniker('planetOrbitalRegistryBroker',
        // solarSystemID)` (`eveMoniker.GetPlanetOrbitalRegistry:253`) **[V]**.
        // The import window asks it one thing (`importExportUI.UpdateTaxRate:554`):
        // the rate it then quotes with and sends back with the transfer.
        "planetOrbitalRegistryBroker" => match req.method {
            // `GetTaxRate(customsOfficeID)` -> a float, or `None`, which the
            // window reads as "access denied" and disables the transfer
            // (`RefreshCostText:547-550`, `ConfirmCommodityTransfer:795-797`)
            // **[V]**. An id that is not an office is `None` for that reason:
            // there is nothing to quote.
            "GetTaxRate" => Ok(req
                .args
                .first()
                .and_then(as_int)
                .and_then(|id| office(reg, id))
                .map_or(PyValue::None, |o| PyValue::Real(o.tax_rate))),
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` gates the call. Spelled out rather than
        // `unreachable!()` so a drift between the two is an ordinary error
        // and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// The first ECU type the static table lists, for quoting an ECU the colony
/// does not hold yet. `0` -- every attribute at its default -- without one.
fn first_ecu_type(statics: &crate::planet::Statics) -> i64 {
    statics
        .pin_types()
        .find(|t| t.group_id == crate::planet::GROUP_ECU_PINS)
        .map_or(0, |t| t.type_id)
}

/// `planetMgr.UserUpdateNetwork(serializedChanges)`: apply one edit session.
///
/// **All or nothing, across three stores.** The stream is applied to a copy
/// of the colonies; the command centre item, the wallet and the skills are
/// checked against what the copy says happened; only then is anything
/// committed -- the item first, because it is the one step that can still
/// fail, then the colony, then the debit. A refusal anywhere leaves the
/// world exactly as it was, and is raised as the `UserError` the client
/// itself would raise, so the window says why.
///
/// What a submit costs, all **[V]** from the client's own accounting
/// (`clientColony.pyc:63-131`):
///
/// - **a command centre is paid for with the item.** `ClientPlanet.
///   CreateCommandPin` finds one of the right type in the active ship's
///   cargo, command-centre hold or colony-resources hold and uses *its item
///   id as the pin id* (`clientPlanet.pyc:279-304`). So the `CREATEPIN`
///   carries a real id, and that item is what we consume -- one unit of the
///   stack. No ISK.
/// - **every other pin costs its type's `basePrice`**, the figure
///   `PostValidateCreatePin` adds to `cumulativePinCreationCost`.
/// - **an upgrade costs `planetCommon.GetUpgradeCost(old, new)`**, which is
///   [`crate::planet::command_centre_upgrade_cost`].
///
/// Charged once, as journal entry type 98 Planetary Construction.
///
/// And two skill gates the client applies before it submits, mirrored so a
/// modified client cannot skip them: the planet count against Interplanetary
/// Consolidation (`level + 1` planets; `Omnipotent` waives it) and the
/// command-centre level against Command Center Upgrades. **Not** mirrored:
/// the per-pin required skills (`GetRequiredSkills`), which need
/// `requiredskillsfortypes` in the static file -- the client still enforces
/// them.
fn user_update_network(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    use crate::planet::{ColonyEdit, Command, FieldQuote, Id, PiError, PinKind, UniformField};
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "a submit needs a character"));
    };
    let Some(planet) = planet_id(req, false) else {
        return Err(bad_args(
            req,
            "planetMgr is bound per planet; no planetID in the bind params",
        ));
    };
    let stream = req
        .args
        .first()
        .ok_or_else(|| bad_args(req, "no command stream"))?;
    let commands = decode_stream(stream).map_err(|why| bad_args(req, &why))?;
    let now = crate::pyutil::filetime_now();
    let statics = crate::planet::statics_in(reg.content()).clone();
    let existing = reg.planet.get(planet, actor).cloned();
    let (planet_type, radius) = match (planet_facts(reg, planet), &existing) {
        (Some(f), _) => (f.type_id, f.radius),
        (None, Some(c)) => (c.planet_type_id, c.planet_radius),
        (None, None) => {
            return Err(bad_args(
                req,
                "no such planet in the celestial catalogue (start the server with --celestials)",
            ));
        }
    };
    let Some(character) = reg.character(actor) else {
        return Err(bad_args(req, "no such character"));
    };
    let (ship_id, balance, station_owner) = (
        character.ship_id,
        character.balance,
        character.station_owner_id,
    );

    // The command centres this stream deploys, and the item each one is.
    let mut consume = Vec::new();
    for command in &commands {
        let Command::CreatePin {
            pin_id, type_id, ..
        } = command
        else {
            continue;
        };
        if statics.pin_kind(*type_id) != Some(PinKind::Command) {
            continue;
        }
        let Id::Real(item_id) = pin_id else {
            return Err(pi_error(PiError::CannotBuildCommandPin));
        };
        let item = reg
            .store()
            .get(*item_id)
            .filter(|i| {
                i.owner_id == actor
                    && i.type_id == *type_id
                    && i.location_id == ship_id
                    && COMMAND_CENTRE_HOLDS.contains(&i.flag_id)
                    && i.quantity >= 1
            })
            .ok_or_else(|| pi_error(PiError::CannotBuildCommandPin))?;
        consume.push(if item.quantity > 1 {
            crate::item::Edit::Consume {
                item_id: *item_id,
                quantity: 1,
            }
        } else {
            crate::item::Edit::Destroy { item_id: *item_id }
        });
        // `ClientColony.PostValidateCreatePin:76-83`, exactly: with no other
        // colony there is no check; otherwise an untrained skill refuses and
        // a trained one must be at least the number of other colonies.
        let others = reg
            .planet
            .for_owner(actor)
            .iter()
            .filter(|c| c.planet_id != planet)
            .count() as i64;
        if others > 0 && skill_level(reg, actor, SKILL_OMNIPOTENT).is_none() {
            match skill_level(reg, actor, SKILL_INTERPLANETARY_CONSOLIDATION) {
                Some(level) if level >= others => {}
                _ => return Err(pi_error(PiError::CannotPlaceCommandCenterNotEnoughSkill)),
            }
        }
    }

    let field = UniformField {
        qualities: statics.resources(planet_type),
    };
    let quote = FieldQuote {
        statics: &statics,
        field: &field,
    };
    let mut trial = reg.planet.clone();
    let report = trial
        .apply(
            &statics,
            &ColonyEdit {
                planet_id: planet,
                owner_id: actor,
                planet_type_id: planet_type,
                planet_radius: radius,
                now,
                link_type_id: TYPE_PLANETARY_LINK,
                quote: &quote,
            },
            &commands,
        )
        .map_err(pi_error)?;

    let before_level = existing.as_ref().map_or(0, |c| c.level);
    let after = trial.get(planet, actor);
    let after_level = after.map_or(0, |c| c.level);
    // `ClientColony.PostValidateCommandCenterUpgrade:119-126`: `level >
    // skillLevel`, where an untrained skill is `None` and Python 2 orders
    // every int above `None` -- so untrained refuses every upgrade.
    if after_level > before_level
        && skill_level(reg, actor, SKILL_COMMAND_CENTER_UPGRADE).is_none_or(|l| after_level > l)
    {
        return Err(pi_error(PiError::CantUpgradeCommandCenterSkillRequired));
    }
    let pins_cost: f64 = report
        .created_pins
        .iter()
        .filter_map(|id| after.and_then(|c| c.pin(*id)))
        .filter(|p| p.kind(&statics) != Some(PinKind::Command))
        .map(|p| statics.base_price(p.type_id))
        .sum();
    let upgrade_cost = if after_level > before_level {
        crate::planet::command_centre_upgrade_cost(before_level, after_level).unwrap_or(0) as f64
    } else {
        0.0
    };
    let cost = pins_cost + upgrade_cost;
    if cost > balance {
        return Err(pi_error(PiError::CannotBuildNotEnoughCash));
    }

    if !consume.is_empty() {
        reg.apply_items(&consume)
            .map_err(|_| pi_error(PiError::CannotBuildCommandPin))?;
    }
    reg.planet = trial;
    if cost > 0.0 {
        // **[A]** on the counterparty: the station's owner, as industry's fee
        // does, so the Journal tab names an NPC corporation the client
        // already knows. Tranquility's is not on the wire we have.
        reg.charge(
            actor,
            cost,
            REF_PLANETARY_CONSTRUCTION,
            station_owner,
            planet,
        );
    }
    reg.mark_dirty();
    Ok(colony_reply(&statics, actor, reg.planet.get(planet, actor)))
}

/// `planetMgr.UserLaunchCommodities(commandPinID, {typeID: qty})` ->
/// `lastLaunchTime`, which the client stores on the pin and simulates up to
/// before removing the same commodities itself (`clientPlanet.pyc:566-571`)
/// **[V]**.
///
/// **Where the goods go is [A].** On Tranquility a launch is a cargo
/// container in orbit that the pilot flies to and empties within five days.
/// That needs a ball in space and a container type; what this sandbox has is
/// the item store, so a launch lands the commodities **in the hangar of the
/// character's station** -- the one `Character::station_id` names, which is
/// where they are docked or last docked. One new stack per type per launch.
fn user_launch_commodities(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    use crate::planet::PiError;
    let (Some(actor), Some(planet)) = (req.actor, planet_id(req, false)) else {
        return Err(bad_args(
            req,
            "a launch needs a character and a bound planet",
        ));
    };
    let pin_id = req
        .args
        .first()
        .and_then(as_int)
        .ok_or_else(|| bad_args(req, "no command pin id"))?;
    let commodities = req
        .args
        .get(1)
        .and_then(as_commodities)
        .ok_or_else(|| bad_args(req, "commodities must be {typeID: quantity}"))?;
    let now = crate::pyutil::filetime_now();
    let statics = crate::planet::statics_in(reg.content()).clone();
    reg.planet.advance(&statics, planet, actor, now);
    let mut trial = reg
        .planet
        .get(planet, actor)
        .cloned()
        .ok_or_else(|| pi_error(PiError::CannotLaunchWithoutColony))?;
    let launched = trial
        .launch(&statics, actor, pin_id, &commodities, now)
        .map_err(pi_error)?;
    let station = reg
        .character(actor)
        .and_then(|c| c.station_id)
        .ok_or_else(|| bad_args(req, "the character has no station to land a launch in"))?;
    let edits: Vec<crate::item::Edit> = launched
        .iter()
        .map(|(type_id, quantity)| crate::item::Edit::Create {
            type_id: *type_id,
            quantity: *quantity,
            to: crate::item::Slot::new(actor, station, crate::model::FLAG_HANGAR),
        })
        .collect();
    if !edits.is_empty() {
        reg.apply_items(&edits)
            .map_err(|e| bad_args(req, &format!("the hangar refused the launch: {e}")))?;
    }
    reg.planet.insert(trial);
    reg.mark_dirty();
    Ok(PyValue::Int(now))
}

/// `planetMgr.UserTransferCommodities(path, {typeID: qty})` ->
/// `(simTime, sourceRunTime)`: the client simulates to `simTime`, runs
/// `ExecuteExpeditedTransfer` itself, and sets the source's `lastRunTime` to
/// `sourceRunTime` (`clientPlanet.pyc:615-621`) **[V]**. So the server's
/// answer has to be the same move the client is about to make locally,
/// which is why both come from the same ported function.
fn user_transfer_commodities(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let (Some(actor), Some(planet)) = (req.actor, planet_id(req, false)) else {
        return Err(bad_args(
            req,
            "a transfer needs a character and a bound planet",
        ));
    };
    let path = req
        .args
        .first()
        .and_then(as_seq)
        .and_then(|p| p.iter().map(as_int).collect::<Option<Vec<_>>>())
        .ok_or_else(|| bad_args(req, "the path must be a list of pin ids"))?;
    let commodities = req
        .args
        .get(1)
        .and_then(as_commodities)
        .ok_or_else(|| bad_args(req, "commodities must be {typeID: quantity}"))?;
    let now = crate::pyutil::filetime_now();
    let statics = crate::planet::statics_in(reg.content()).clone();
    reg.planet.advance(&statics, planet, actor, now);
    let mut trial =
        reg.planet.get(planet, actor).cloned().ok_or_else(|| {
            pi_error(crate::planet::PiError::CannotManagePlanetWithoutCommandCenter)
        })?;
    trial
        .expedited_transfer(&statics, actor, &path, &commodities, now)
        .map_err(pi_error)?;
    let sim_time = trial.current_sim_time;
    let source_run_time = path
        .first()
        .and_then(|id| trial.pin(*id))
        .map_or(now, |p| p.last_run_time);
    reg.planet.insert(trial);
    reg.mark_dirty();
    Ok(PyValue::Tuple(vec![
        PyValue::Int(sim_time),
        PyValue::Int(source_run_time),
    ]))
}

// ---------------------------------------------------------------------------
// Customs offices: the office's inventory, and import/export (W9 L5)
// ---------------------------------------------------------------------------

/// The office `id` names, if it is one -- one per planet the catalogue
/// serves, once the static file names the office type
/// (`crate::planet::customs_office`).
fn office(reg: &Registry, id: i64) -> Option<crate::planet::CustomsOffice> {
    crate::planet::customs_office(
        reg.celestials(),
        crate::planet::statics_in(reg.content()),
        id,
    )
}

/// Our words, for a refusal no dialog of the client's covers.
fn custom_notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

/// A [`crate::planet::PiError`] with the loc args its dialog declares
/// (`findings/user-errors.tsv`) **[V]**: the two export refusals name the
/// commodity, and a name left out renders as the literal `{typeName}`.
fn customs_error(content: &crate::content::Content, e: crate::planet::PiError) -> ServiceError {
    use crate::planet::PiError;
    let name = |type_id: i64| {
        crate::pyutil::bstr(crate::item::find_in(content, type_id).map_or("commodity", |t| t.name))
    };
    let args = match &e {
        PiError::CannotExportCommodityNotFound { type_id } => {
            vec![("typeName", name(*type_id))]
        }
        PiError::CannotExportCommodityNotEnough {
            type_id,
            contained,
            desired,
        } => vec![
            ("typeName", name(*type_id)),
            ("contained", PyValue::Int(*contained)),
            ("desired", PyValue::Int(*desired)),
        ],
        _ => return pi_error(e),
    };
    ServiceError::Raise(eve_proto::user_error(
        e.message_id(),
        crate::pyutil::pypairs(args),
    ))
}

/// `NotEnoughSpace`, dialog 1352, with its two loc args `volume` and
/// `available` -- the same names the client raises it with
/// (`importExportUI.CheckAvailableSpaceInCustoms:447`) **[V]**.
fn not_enough_space(volume: f64, available: f64) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "NotEnoughSpace",
        crate::pyutil::pypairs(vec![
            ("volume", PyValue::Real(volume)),
            ("available", PyValue::Real(available.max(0.0))),
        ]),
    ))
}

/// One unit's volume, from the item table every hold uses.
fn unit_volume(reg: &Registry, type_id: i64) -> f64 {
    crate::item::find_in(reg.content(), type_id).map_or(0.0, |t| t.volume)
}

/// What `actor` keeps in `office`: their own items, flag hangar.
fn office_items(
    reg: &Registry,
    actor: i64,
    office: &crate::planet::CustomsOffice,
) -> Vec<crate::item::Item> {
    reg.store()
        .contents_of(office.item_id)
        .filter(|i| i.owner_id == actor && i.flag_id == crate::planet::CUSTOMS_OFFICE_FLAG)
        .cloned()
        .collect()
}

fn office_used_volume(reg: &Registry, actor: i64, office: &crate::planet::CustomsOffice) -> f64 {
    office_items(reg, actor, office)
        .iter()
        .map(|i| unit_volume(reg, i.type_id) * i.quantity as f64)
        .sum()
}

/// `actor`'s presence is in the office's solar system. An office is a
/// building in space; a docked pilot, or one in another system, cannot
/// reach it. **No range** beyond that (**[A]**): the client opens the window
/// from the planet's or the office's menu without a range test of its own
/// that we have found, and Tranquility's distance is not in the client.
fn beside(reg: &Registry, actor: i64, office: &crate::planet::CustomsOffice) -> bool {
    reg.roster()
        .get(actor)
        .is_some_and(|p| p.solar_system_id == office.solar_system_id)
}

/// The office as its own inventory's "self item": what `invCacheContainer`
/// builds its `typeID` and `item` from (`_SetItemData`). The station row
/// the generic arm answers for an unknown container would make the office a
/// station to the client.
fn office_self_item(reg: &Registry, office: &crate::planet::CustomsOffice) -> PyValue {
    reg.item_row_value(&crate::item::Item {
        item_id: office.item_id,
        type_id: office.type_id,
        owner_id: crate::planet::OWNER_INTERBUS,
        location_id: office.solar_system_id,
        flag_id: 0,
        quantity: 1,
        group_id: crate::planet::GROUP_PLANETARY_CUSTOMS_OFFICES,
        category_id: crate::planet::CATEGORY_ORBITAL,
        singleton: true,
    })
}

/// The item ids an `Add(itemID, …)` or `MultiAdd([itemIDs], …)` names.
fn add_ids(req: &Request<'_>) -> Vec<i64> {
    match (req.method, req.args.first()) {
        ("Add", Some(PyValue::Int(id))) => vec![*id],
        ("MultiAdd", Some(v)) => as_seq(v)
            .map(|ids| ids.iter().filter_map(as_int).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn int_kwarg(req: &Request<'_>, name: &str) -> Option<i64> {
    crate::pyutil::pairs_get(req.kwargs, name.as_bytes()).and_then(as_int)
}

/// **The customs offices' half of `invbroker`'s bound inventory.**
///
/// Called first by `service/inventory.rs`'s dispatch (one line, in the
/// coordinator diff), and `None` for every call that is not about an office,
/// so the inventory answers those exactly as before. Two cases:
///
/// 1. **An inventory bound on an office** --
///    `invCache.GetInventoryFromId(customsOfficeID)`, which is how the
///    import window holds the office (`importExportUI.ApplyAttributes:101`)
///    **[V]**. It lists (`List(flagHangar)`, `:351`), sizes (`GetCapacity`,
///    `:427`), takes from the hold (`Add(itemID, sourceID, qty=n)`,
///    `MoveFromInventory:756`) and transfers
///    (`ImportExportWithPlanet`, `:784`) **[V]**; the rest of the inventory
///    surface falls through.
/// 2. **An item leaving an office** for the bound inventory: a drag from the
///    window's office list onto the ship's cargo, which the cargo controller
///    sends as `Add(itemID, officeID, qty=…, flag=flagCargo)` on the
///    **ship's** inventory (`invControllers.py:267`) **[V]**.
pub(super) fn customs_inventory_call(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    if req.service != "invbroker::inventory" {
        return None;
    }
    if let Some(office) = container_of(req.bind_params).and_then(|id| office(reg, id)) {
        let actor = req.actor;
        return Some(match req.method {
            // `List(flag)` and its siblings: the caller's own goods. An
            // office is shared; nobody sees another pilot's hangar in it.
            "List" | "ListByFlags" | "ListIncludingContainers" => {
                Ok(PyValue::List(actor.map_or_else(Vec::new, |a| {
                    office_items(reg, a, &office)
                        .iter()
                        .map(|i| reg.item_row_value(i))
                        .collect()
                })))
            }
            "GetCapacity" => Ok(crate::pyutil::keyval(vec![
                ("capacity", PyValue::Real(office.capacity)),
                (
                    "used",
                    PyValue::Real(actor.map_or(0.0, |a| office_used_volume(reg, a, &office))),
                ),
            ])),
            "GetSelfInvItem" => Ok(office_self_item(reg, &office)),
            "Add" | "MultiAdd" => into_office(reg, req, &office),
            "ImportExportWithPlanet" => import_export_with_planet(reg, req, &office),
            _ => return None,
        });
    }
    let ids = add_ids(req);
    let leaving = !ids.is_empty()
        && ids.iter().all(|id| {
            reg.item(*id)
                .is_some_and(|i| crate::planet::customs_office_planet(i.location_id).is_some())
        });
    leaving.then(|| out_of_office(reg, req, &ids))
}

/// Into an office, from the active ship (`Add`/`MultiAdd` on the office's
/// inventory). Every item first, then one plan: all or nothing.
fn into_office(
    reg: &mut Registry,
    req: &Request<'_>,
    office: &crate::planet::CustomsOffice,
) -> Result<PyValue, ServiceError> {
    use crate::item::{Edit, Slot};
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "moving goods needs a character"));
    };
    if !beside(reg, actor, office) {
        return Err(pi_error(
            crate::planet::PiError::CannotImportNotInSameSolarSystem,
        ));
    }
    let ship = reg.character(actor).map(|c| c.ship_id);
    let ids = add_ids(req);
    let qty = if req.method == "Add" {
        int_kwarg(req, "qty")
    } else {
        None
    };
    let to = Slot::new(actor, office.item_id, crate::planet::CUSTOMS_OFFICE_FLAG);
    let mut edits = Vec::new();
    let mut volume = 0.0;
    for id in &ids {
        let item = reg
            .item(*id)
            .filter(|i| {
                i.owner_id == actor
                    && Some(i.location_id) == ship
                    && !crate::item::is_ship_fitting_flag(i.flag_id)
                    && !crate::item::is_loaded_charge(i)
            })
            .ok_or_else(|| {
                custom_notify("Only goods in your ship's hold can go into a customs office.")
            })?;
        // `planetCommon.CanPutTypeInCustomsOffice` **[V]**.
        if !crate::planet::can_put_type_in_customs_office(item.group_id, item.category_id) {
            return Err(pi_error(crate::planet::PiError::CannotPutInCargolink));
        }
        let n = qty.filter(|q| *q > 0 && *q < item.quantity);
        volume += unit_volume(reg, item.type_id) * n.unwrap_or(item.quantity) as f64;
        edits.push(match n {
            Some(quantity) => Edit::Split {
                item_id: *id,
                quantity,
                to,
            },
            None => Edit::Move { item_id: *id, to },
        });
    }
    if edits.is_empty() {
        return Ok(PyValue::None);
    }
    let available = office.capacity - office_used_volume(reg, actor, office);
    if volume - available > 1e-5 {
        return Err(not_enough_space(volume, available));
    }
    reg.apply_items(&edits)
        .map_err(|e| custom_notify(&format!("The customs office refused the move: {e}")))?;
    Ok(PyValue::None)
}

/// Out of an office, into the active ship's cargo hold: the inverse of
/// [`into_office`]. Only the cargo hold (**[A]** on refusing the special
/// holds: the window's drag targets the cargo, and a PI hold is one more
/// flag the morning test can add).
fn out_of_office(
    reg: &mut Registry,
    req: &Request<'_>,
    ids: &[i64],
) -> Result<PyValue, ServiceError> {
    use crate::item::{Edit, Slot};
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "moving goods needs a character"));
    };
    let ship = reg.character(actor).map(|c| c.ship_id);
    let target = container_of(req.bind_params);
    let flag = int_kwarg(req, "flag").unwrap_or(crate::fitting::FLAG_CARGO);
    if target.is_none() || target != ship || flag != crate::fitting::FLAG_CARGO {
        return Err(custom_notify(
            "Goods from a customs office can only go into your active ship's cargo hold.",
        ));
    }
    let ship = ship.unwrap_or(0);
    let qty = if req.method == "Add" {
        int_kwarg(req, "qty")
    } else {
        None
    };
    let to = Slot::new(actor, ship, crate::fitting::FLAG_CARGO);
    let mut edits = Vec::new();
    for id in ids {
        let item = reg
            .item(*id)
            .filter(|i| i.owner_id == actor)
            .cloned()
            .ok_or_else(|| custom_notify("That is not yours to take."))?;
        let office = office(reg, item.location_id)
            .ok_or_else(|| custom_notify("That customs office is not here."))?;
        if !beside(reg, actor, &office) {
            return Err(pi_error(
                crate::planet::PiError::CannotExportNotInSameSolarSystem,
            ));
        }
        let n = qty.filter(|q| *q > 0 && *q < item.quantity);
        crate::fitting::check_cargo_room_in(
            reg.content(),
            reg.store(),
            ship,
            item.type_id,
            n.unwrap_or(item.quantity),
        )
        .map_err(|r| r.to_error())?;
        edits.push(match n {
            Some(quantity) => Edit::Split {
                item_id: *id,
                quantity,
                to,
            },
            None => Edit::Move { item_id: *id, to },
        });
    }
    reg.apply_items(&edits)
        .map_err(|e| custom_notify(&format!("The cargo hold refused the move: {e}")))?;
    Ok(PyValue::None)
}

/// `customsOfficeInventory.ImportExportWithPlanet(spaceportPinID,
/// importData, exportData, taxRate)` **[V]** (`importExportUI.
/// ConfirmCommodityTransfer:777-784`):
///
/// - `importData` is `{itemID: quantity}` -- the office's own item rows,
///   keyed by `key[0]` of the window's `(itemID, typeID)`;
/// - `exportData` is `{typeID: quantity}` -- `key[1]`, because a pin holds
///   commodities, not items;
/// - `taxRate` is the rate the window quoted. A different current rate is
///   `UserError('TaxChanged')`, which the window catches by `e.msg`,
///   re-reads the rate and asks again (`:786-803`).
///
/// The reply is discarded (`POP_TOP`). What redraws the window is the
/// `OnItemChange` for the office's goods (`OnItemChange:569` resets when the
/// office is the old or new location) and `OnMajorPlanetStateUpdate`, which
/// re-reads the colony and scatters `OnPlanetPinsChanged` to it
/// (`planetSvc.pyc:149-165`) **[V]**.
///
/// One commit, in the submit's order (item, colony, wallet): everything is
/// checked against a copy of the colony and a plan of item edits before
/// either is applied, so a refusal changes nothing and **every unit that
/// leaves one side arrives on the other**.
fn import_export_with_planet(
    reg: &mut Registry,
    req: &Request<'_>,
    office: &crate::planet::CustomsOffice,
) -> Result<PyValue, ServiceError> {
    use crate::item::{Edit, Slot};
    use crate::planet::PiError;
    use std::collections::BTreeMap;
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "a transfer needs a character"));
    };
    let pin_id = req.args.first().and_then(as_int);
    let imports_by_item = req.args.get(1).and_then(as_commodities);
    let exports = req.args.get(2).and_then(as_commodities);
    let rate = req.args.get(3).and_then(as_real);
    let (Some(pin_id), Some(imports_by_item), Some(exports)) = (pin_id, imports_by_item, exports)
    else {
        return Err(bad_args(
            req,
            "(spaceportPinID, {itemID: qty}, {typeID: qty}, taxRate)",
        ));
    };
    if imports_by_item.is_empty() && exports.is_empty() {
        return Err(pi_error(PiError::PleaseSelectCommoditiesToImport));
    }
    if !beside(reg, actor, office) {
        return Err(pi_error(if imports_by_item.is_empty() {
            PiError::CannotExportNotInSameSolarSystem
        } else {
            PiError::CannotImportNotInSameSolarSystem
        }));
    }
    if rate.is_none_or(|r| (r - office.tax_rate).abs() > 1e-9) {
        return Err(pi_error(PiError::TaxChanged));
    }

    let now = crate::pyutil::filetime_now();
    let statics = crate::planet::statics_in(reg.content()).clone();
    reg.planet.advance(&statics, office.planet_id, actor, now);
    let mut trial = reg
        .planet
        .get(office.planet_id, actor)
        .cloned()
        .ok_or_else(|| {
            pi_error(if imports_by_item.is_empty() {
                PiError::CannotExportNoColony
            } else {
                PiError::CannotImportNoColony
            })
        })?;

    // The office side of an import: which items, and how much of each.
    let mut imports: BTreeMap<i64, i64> = BTreeMap::new();
    let mut edits = Vec::new();
    let mut freed = 0.0;
    for (item_id, qty) in &imports_by_item {
        let item = reg
            .item(*item_id)
            .filter(|i| {
                i.owner_id == actor
                    && i.location_id == office.item_id
                    && i.flag_id == crate::planet::CUSTOMS_OFFICE_FLAG
            })
            .cloned()
            .ok_or_else(|| custom_notify("That is not in your hangar at this customs office."))?;
        if !crate::planet::can_put_type_in_customs_office(item.group_id, item.category_id) {
            return Err(pi_error(PiError::CannotPutInCargolink));
        }
        if *qty < 1 || *qty > item.quantity {
            return Err(custom_notify(&format!(
                "Only {} of that are in the customs office.",
                item.quantity
            )));
        }
        *imports.entry(item.type_id).or_default() += qty;
        freed += unit_volume(reg, item.type_id) * *qty as f64;
        edits.push(if *qty == item.quantity {
            Edit::Destroy { item_id: *item_id }
        } else {
            Edit::Consume {
                item_id: *item_id,
                quantity: *qty,
            }
        });
    }
    if !imports.is_empty() {
        trial
            .import_commodities(&statics, actor, pin_id, &imports)
            .map_err(|e| customs_error(reg.content(), e))?;
    }
    if !exports.is_empty() {
        trial
            .export_commodities(&statics, actor, pin_id, &exports)
            .map_err(|e| customs_error(reg.content(), e))?;
    }

    // The office side of an export: onto a stack of that type the import is
    // not touching, or a new one.
    let to = Slot::new(actor, office.item_id, crate::planet::CUSTOMS_OFFICE_FLAG);
    let held = office_items(reg, actor, office);
    let mut arriving = 0.0;
    for (type_id, qty) in &exports {
        arriving += unit_volume(reg, *type_id) * *qty as f64;
        let stack = held.iter().find(|i| {
            i.type_id == *type_id && !i.singleton && !imports_by_item.contains_key(&i.item_id)
        });
        edits.push(match stack {
            Some(i) => Edit::Grow {
                item_id: i.item_id,
                quantity: *qty,
            },
            None => Edit::Create {
                type_id: *type_id,
                quantity: *qty,
                to,
            },
        });
    }
    // The office being full is `CannotExportNotEnoughSpace`, dialog 3387 --
    // a real row with no loc args **[V]**; that this is its moment is read
    // from its name **[I]**. The window checks the same sum first
    // (`CheckAvailableSpaceInCustoms:443-447`) **[V]**.
    let available = office.capacity - office_used_volume(reg, actor, office) + freed;
    if arriving - available > 1e-5 {
        return Err(pi_error(PiError::CannotExportNotEnoughSpace));
    }

    // `SpaceportPin.GetImportTax` + `GetExportTax`, exactly as the window
    // quoted them (`importExportUI.GetCost:529-535`) **[V]**.
    let pin_type = trial.pin(pin_id).map_or(0, |p| p.type_id);
    let import_tax = statics.import_tax(pin_type, &imports, office.tax_rate);
    let export_tax = statics.export_tax(pin_type, &exports, office.tax_rate);
    let balance = reg.character(actor).map_or(0.0, |c| c.balance);
    if import_tax + export_tax > balance {
        return Err(ServiceError::Raise(eve_proto::user_error(
            "NotEnoughMoney",
            crate::pyutil::pypairs(vec![
                ("amount", PyValue::Real(import_tax + export_tax)),
                ("balance", PyValue::Real(balance)),
            ]),
        )));
    }

    reg.apply_items(&edits)
        .map_err(|e| custom_notify(&format!("The customs office refused the transfer: {e}")))?;
    reg.planet.insert(trial);
    // **[A]** One journal line per direction, against the office's owner,
    // the planet as the reference -- as the construction bill does.
    if import_tax > 0.0 {
        reg.charge(
            actor,
            import_tax,
            REF_PLANETARY_IMPORT_TAX,
            crate::planet::OWNER_INTERBUS,
            office.planet_id,
        );
    }
    if export_tax > 0.0 {
        reg.charge(
            actor,
            export_tax,
            REF_PLANETARY_EXPORT_TAX,
            crate::planet::OWNER_INTERBUS,
            office.planet_id,
        );
    }
    reg.mark_dirty();
    reg.push(MajorPlanetStateUpdate {
        planet_id: office.planet_id,
    });
    Ok(PyValue::None)
}

/// `OnMajorPlanetStateUpdate(planetID, numCommandCentersChanged)` **[V]**
/// (`planetSvc.pyc:149`, argument names from the code object). The client
/// re-`Init`s its `ClientPlanet` -- a fresh `GetPlanetInfo` -- and scatters
/// `OnPlanetPinsChanged`, which is what redraws the import window's launchpad
/// list after a transfer. `0`: no command centre came or went.
#[derive(Debug)]
struct MajorPlanetStateUpdate {
    planet_id: i64,
}

impl crate::notify::Push for MajorPlanetStateUpdate {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnMajorPlanetStateUpdate",
            vec![PyValue::Int(self.planet_id), PyValue::Int(0)],
        )
    }
}

// ---- E1: a logged-off farmer's colonies ---------------------------------------

/// What one offline tend of a character's colonies did
/// ([`Registry::settle_colonies`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tend {
    /// Colonies simulated up to now.
    pub colonies: usize,
    /// Extraction programs installed again.
    pub restarted: usize,
    /// Commodities launched to the hangar, `type -> quantity`.
    pub launched: std::collections::BTreeMap<i64, i64>,
    /// Why a restart or a launch was refused, in words, one per refusal.
    pub refused: Vec<String>,
}

/// The hours of the program a restart installs when the pin's own radius is
/// out of range **[A]**: the PI bot's `PROGRAM_HOURS` (23), which
/// `eve-bot` owns and this crate cannot import. Falsified by a restarted
/// program that ends before the next settlement.
const RESTART_PROGRAM_HOURS: f64 = 23.0;

impl Registry {
    /// **Tend `actor`'s colonies as the PI bot's cycle does, away from the
    /// keyboard** (`eve-bot/src/pi.rs::run`'s tending half): each colony is
    /// simulated to `now` (it ran on its own while the pilot was away: the
    /// simulation is lazy and exact), every extraction unit with heads whose
    /// program has run out is given the same program again through the real
    /// command stream (`Colony::apply`, the quote of `INSTALLPROGRAM`), and
    /// the command centre's commodities are launched to the hangar of the
    /// character's station when its launch cooldown is over. Nothing is
    /// built: founding a colony stays the pilot's.
    pub(crate) fn settle_colonies(&mut self, actor: i64, now: i64) -> Tend {
        use crate::planet::{
            ColonyEdit, Command, FieldQuote, Id, PinKind, RADIUS_DRILLAREAMAX, RADIUS_DRILLAREAMIN,
            UniformField,
        };
        let statics = crate::planet::statics_in(self.content()).clone();
        let planets: Vec<(i64, i64, f64)> = self
            .planet
            .for_owner(actor)
            .iter()
            .map(|c| (c.planet_id, c.planet_type_id, c.planet_radius))
            .collect();
        let mut tend = Tend::default();
        let station = self.character(actor).and_then(|c| c.station_id);
        for (planet, planet_type, planet_radius) in planets {
            if self.planet.advance(&statics, planet, actor, now).is_none() {
                continue;
            }
            tend.colonies += 1;
            let Some(colony) = self.planet.get(planet, actor).cloned() else {
                continue;
            };
            let sim = colony.current_sim_time;
            // The resource a never-programmed extractor takes: the one input
            // of a single-input schematic the colony runs.
            let fallback = colony
                .pins
                .iter()
                .filter_map(|p| p.schematic_id)
                .find_map(|id| match statics.schematic(id)?.inputs.as_slice() {
                    [(t, _)] => Some(*t),
                    _ => None,
                });
            let span = RADIUS_DRILLAREAMAX - RADIUS_DRILLAREAMIN;
            let default_radius = (RADIUS_DRILLAREAMIN
                + (RESTART_PROGRAM_HOURS - 1.0) / 335.0 * span)
                .clamp(RADIUS_DRILLAREAMIN, RADIUS_DRILLAREAMAX);
            let commands: Vec<Command> = colony
                .pins
                .iter()
                .filter(|p| p.kind(&statics) == Some(PinKind::Ecu) && !p.heads.is_empty())
                .filter(|p| p.expiry_time.is_none_or(|e| e <= sim))
                .filter(|p| {
                    p.install_time
                        .is_none_or(|i| sim >= i + crate::planet::PROGRAM_INSTALLATION_COOLDOWN)
                })
                .filter_map(|p| {
                    let resource = p.program_type.or(fallback)?;
                    let radius =
                        if (RADIUS_DRILLAREAMIN..=RADIUS_DRILLAREAMAX).contains(&p.head_radius) {
                            p.head_radius
                        } else {
                            default_radius
                        };
                    Some(Command::InstallProgram {
                        pin_id: Id::Real(p.pin_id),
                        program_type: Some(resource),
                        head_radius: radius,
                    })
                })
                .collect();
            if !commands.is_empty() {
                let field = UniformField {
                    qualities: statics.resources(planet_type),
                };
                let quote = FieldQuote {
                    statics: &statics,
                    field: &field,
                };
                let mut trial = self.planet.clone();
                let applied = trial.apply(
                    &statics,
                    &ColonyEdit {
                        planet_id: planet,
                        owner_id: actor,
                        planet_type_id: planet_type,
                        planet_radius,
                        now,
                        link_type_id: TYPE_PLANETARY_LINK,
                        quote: &quote,
                    },
                    &commands,
                );
                match applied {
                    Ok(_) => {
                        self.planet = trial;
                        tend.restarted += commands.len();
                    }
                    Err(e) => tend
                        .refused
                        .push(format!("restart on planet {planet}: {e:?}")),
                }
            }
            // The launch: the command centre's commodities, when it is ready.
            let Some(station) = station else { continue };
            let Some(mut after) = self.planet.get(planet, actor).cloned() else {
                continue;
            };
            let Some(cc) = after
                .pins
                .iter()
                .find(|p| p.kind(&statics) == Some(PinKind::Command))
            else {
                continue;
            };
            let goods: std::collections::BTreeMap<i64, i64> = cc
                .contents
                .iter()
                .filter(|(t, q)| {
                    **q > 0 && unit_volume(self, **t) > 0.0 && is_commodity(&statics, **t)
                })
                .map(|(t, q)| (*t, *q))
                .collect();
            if goods.is_empty() {
                continue;
            }
            let pin = cc.pin_id;
            let launched = match after.launch(&statics, actor, pin, &goods, now) {
                Ok(l) => l,
                Err(e) => {
                    tend.refused
                        .push(format!("launch on planet {planet}: {e:?}"));
                    continue;
                }
            };
            let edits: Vec<crate::item::Edit> = launched
                .iter()
                .map(|(type_id, quantity)| crate::item::Edit::Create {
                    type_id: *type_id,
                    quantity: *quantity,
                    to: crate::item::Slot::new(actor, station, crate::model::FLAG_HANGAR),
                })
                .collect();
            if self.apply_items(&edits).is_ok() {
                self.planet.insert(after);
                for (t, q) in launched {
                    *tend.launched.entry(t).or_default() += q;
                }
            }
        }
        if tend.colonies > 0 {
            self.mark_dirty();
        }
        tend
    }
}

/// Whether `type_id` is a planetary commodity (category 43) in the statics'
/// item rows: what a launch carries.
fn is_commodity(statics: &crate::planet::Statics, type_id: i64) -> bool {
    statics
        .item_rows()
        .iter()
        .any(|r| r.type_id == type_id && r.category_id == 43)
}

/// E1's tests build a world with a wired colony (`tests::aged_colony_world`):
/// a registry whose default character owns it.
#[cfg(test)]
pub(crate) fn colony_world_for_agents(hours: i64) -> (Registry, i64) {
    (tests::aged_colony_world(hours), tests::OWNER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planet::{
        Colonies, Colony, GROUP_COMMAND_PINS, GROUP_ECU_PINS, GROUP_PROCESS_PINS,
        GROUP_SPACEPORT_PINS, Head, Link, Pin, PinType, Route, Schematic, Statics, SurfacePoint,
    };
    use crate::pyutil::dict_get;

    const PLANET: i64 = 40_000_001;
    pub(super) const OWNER: i64 = 90_000_001;
    const TYPE_CC: i64 = 2524;
    const TYPE_ECU: i64 = 2848;
    const TYPE_FACTORY: i64 = 2473;
    const TYPE_LAUNCHPAD: i64 = 2544;
    const RESOURCE: i64 = 2268;
    const PRODUCT: i64 = 3645;

    fn statics() -> Statics {
        let a = |pairs: &[(i64, f64)]| -> std::collections::BTreeMap<i64, f64> {
            pairs.iter().copied().collect()
        };
        Statics::new()
            .with_pin_type(PinType {
                type_id: TYPE_CC,
                group_id: GROUP_COMMAND_PINS,
                capacity: 500.0,
                attributes: a(&[]),
            })
            .with_pin_type(PinType {
                type_id: TYPE_ECU,
                group_id: GROUP_ECU_PINS,
                capacity: 0.0,
                attributes: a(&[
                    (crate::planet::attr::ECU_DECAY_FACTOR, 0.012),
                    (crate::planet::attr::ECU_NOISE_FACTOR, 0.8),
                ]),
            })
            .with_pin_type(PinType {
                type_id: TYPE_FACTORY,
                group_id: GROUP_PROCESS_PINS,
                capacity: 0.0,
                attributes: a(&[]),
            })
            .with_pin_type(PinType {
                type_id: TYPE_LAUNCHPAD,
                group_id: GROUP_SPACEPORT_PINS,
                capacity: 10_000.0,
                attributes: a(&[]),
            })
            .with_schematic(Schematic {
                schematic_id: 121,
                cycle_time: 1800,
                pin_type_ids: vec![TYPE_FACTORY],
                inputs: vec![(RESOURCE, 3000)],
                outputs: vec![(PRODUCT, 20)],
            })
            .with_volume(RESOURCE, 0.005)
            .with_volume(PRODUCT, 0.19)
    }

    /// A registry holding one colony, so the read arms have something true to
    /// answer. The colony has one of each pin class, because the pin row's
    /// field set depends on the class and a fixture with only one kind would
    /// not exercise that.
    fn registry_with_a_colony() -> Registry {
        let mut reg = Registry::new(SandboxConfig::default());
        reg.planet = Colonies::new();
        reg.content = std::sync::Arc::new(crate::planet::content_with(statics()));
        let now = crate::pyutil::filetime_now();
        let mut colony = Colony::new(PLANET, OWNER, 2016, now);
        colony.planet_radius = 4_000_000.0;
        colony.level = 1;
        for (id, type_id) in [
            (1_i64, TYPE_CC),
            (2, TYPE_ECU),
            (3, TYPE_FACTORY),
            (4, TYPE_LAUNCHPAD),
        ] {
            colony.pins.push(Pin::new(
                id,
                type_id,
                OWNER,
                SurfacePoint::new(1.5, 0.01 * id as f64),
                now,
            ));
        }
        colony.pin_mut(2).unwrap().heads.push(Head {
            head_id: 0,
            latitude: 1.5,
            longitude: 0.02,
        });
        colony
            .pin_mut(2)
            .unwrap()
            .install_program(&statics(), Some(RESOURCE), 0.01, 1000, now)
            .expect("the fixture's quote is positive");
        colony
            .pin_mut(3)
            .unwrap()
            .install_schematic(&statics(), 121)
            .expect("the factory may run schematic 121");
        colony.links.push(Link::new(1, 2, TYPE_PLANETARY_LINK, 0));
        colony.links.push(Link::new(2, 3, TYPE_PLANETARY_LINK, 0));
        colony.links.push(Link::new(3, 4, TYPE_PLANETARY_LINK, 0));
        colony.routes.push(Route {
            route_id: 1,
            owner_id: OWNER,
            path: vec![2, 3],
            commodity_type_id: RESOURCE,
            commodity_quantity: 1000,
        });
        reg.planet.insert(colony);
        reg
    }

    fn call_as(
        reg: &mut Registry,
        method: &str,
        args: Vec<PyValue>,
        actor: Option<i64>,
        bind: Option<PyValue>,
    ) -> Result<PyValue, ServiceError> {
        let bind_value = bind;
        dispatch(
            reg,
            &Request {
                service: "planetMgr",
                method,
                args: &args,
                kwargs: &[],
                bind_params: bind_value.as_ref(),
                actor,
            },
        )
    }

    /// Every service this module advertises must reach an arm in *this*
    /// module's `dispatch`.
    #[test]
    fn every_service_here_is_routed_to_this_module() {
        let mut reg = Registry::new(SandboxConfig::default());
        for s in SERVICES {
            match dispatch(
                &mut reg,
                &Request {
                    service: s,
                    method: "__no_such_method_exists__",
                    args: &[],
                    kwargs: &[],
                    bind_params: None,
                    actor: None,
                },
            ) {
                Err(ServiceError::NoSuchMethod { .. }) => {}
                other => panic!(
                    "{s} is advertised by this module but its dispatch answers {other:?} \
                     to a method that cannot exist"
                ),
            }
        }
    }

    /// **The no-character branch of every arm this module serves.**
    ///
    /// `Request::actor` is `Option<i64>` precisely so that no arm can quietly
    /// read a character out of `None`, and the rule is that a missing actor
    /// takes the arm's own empty branch rather than a default. Every one of
    /// these is reachable before a character is selected only by a
    /// malformed or hostile client, which is exactly why none of them may
    /// pick a character on its own.
    ///
    /// The assertion is deliberately about **family and emptiness**, not
    /// about a specific value: the point is that the answer is the same shape
    /// the client would get for a character with no colonies, so no handler
    /// can tell the two apart and none of them raises.
    #[test]
    fn every_arm_has_a_no_character_branch_that_answers_empty() {
        let mut reg = registry_with_a_colony();
        let bind = || Some(PyValue::Int(PLANET));

        // A plain list, because the client appends to it.
        let planets = call_as(&mut reg, "GetPlanetsForChar", vec![], None, None)
            .expect("GetPlanetsForChar must be answered with no character");
        assert_eq!(
            planets,
            PyValue::List(vec![]),
            "no character means no colonies, not the first character's"
        );

        // A KeyVal with the planet's own fields and no colony merged in.
        let info = call_as(&mut reg, "GetPlanetInfo", vec![], None, bind())
            .expect("GetPlanetInfo must be answered with no character");
        let PyValue::Instance { state, .. } = &info else {
            panic!("PreparePlanet reads attributes off it: {info:?}")
        };
        assert!(
            dict_get(state, b"planetID").is_some(),
            "the planet's own id is still answered: {state:?}"
        );
        assert!(
            dict_get(state, b"pins").is_none(),
            "but no colony is merged in without a character: {state:?}"
        );

        // A 2-tuple of two empty lists: the client does
        // `pins, links = ...` before looking at either.
        let network = call_as(&mut reg, "GetFullNetworkForOwner", vec![], None, bind())
            .expect("GetFullNetworkForOwner must be answered with no character");
        assert_eq!(
            network,
            PyValue::Tuple(vec![PyValue::List(vec![]), PyValue::List(vec![])]),
            "UNPACK_SEQUENCE 2 runs before either half is read"
        );

        // These three do not depend on the actor at all -- they are about the
        // planet, not about you -- so they answer the same with or without
        // one. Asserted rather than assumed, because "it does not use the
        // actor" is the kind of claim that stops being true.
        for method in ["GetCommandPinsForPlanet", "GetExtractorsForPlanet"] {
            let with = call_as(&mut reg, method, vec![], Some(OWNER), bind())
                .unwrap_or_else(|e| panic!("{method}: {e:?}"));
            let without = call_as(&mut reg, method, vec![], None, bind())
                .unwrap_or_else(|e| panic!("{method}: {e:?}"));
            assert_eq!(
                with, without,
                "{method} is about the planet, not the caller"
            );
            assert!(
                matches!(with, PyValue::List(_)),
                "{method} is iterated by the client: {with:?}"
            );
        }

        let resources = call_as(&mut reg, "GetPlanetResourceInfo", vec![], None, bind())
            .expect("GetPlanetResourceInfo must be answered");
        assert!(matches!(resources, PyValue::Dict(_)));

        let quote = call_as(
            &mut reg,
            "GetProgramResultInfo",
            vec![
                PyValue::Int(2),
                PyValue::Int(RESOURCE),
                PyValue::List(vec![]),
                PyValue::Real(0.01),
            ],
            None,
            bind(),
        )
        .expect("GetProgramResultInfo must be answered");
        let PyValue::Tuple(parts) = &quote else {
            panic!("the client unpacks three names: {quote:?}")
        };
        assert_eq!(parts.len(), 3);
    }

    /// `GetPlanetsForChar` is a **list**, not a `CRowset`, and its rows are
    /// the five fields the client reads and the five it appends.
    ///
    /// The family is the finding: `planetSvc._UpdateColonyPresence` does
    /// `self.colonizationData.append(utillib.KeyVal(...))`, and a
    /// `dbutil.CRowset` has no `.append`. EvEJS sends a `CRowset`
    /// (`planetMgrService.js:66`), which raises on the first colony a player
    /// deploys.
    #[test]
    fn the_planet_list_is_a_plain_list_the_client_can_append_to() {
        let mut reg = registry_with_a_colony();
        let out = call_as(&mut reg, "GetPlanetsForChar", vec![], Some(OWNER), None)
            .expect("GetPlanetsForChar must be answered");
        let PyValue::List(rows) = &out else {
            panic!("the client appends to this, so it must be a list: {out:?}")
        };
        assert_eq!(rows.len(), 1, "the fixture has one colony");
        let PyValue::Instance { class, state } = &rows[0] else {
            panic!("rows are read by attribute: {:?}", rows[0])
        };
        assert_eq!(
            class.as_slice(),
            crate::pyutil::KEYVAL_CLASS.as_bytes(),
            "the client appends its own utillib.KeyVal into the same list"
        );
        let row = state.as_ref();
        for field in [
            "solarSystemID",
            "planetID",
            "typeID",
            "numberOfPins",
            "celestialIndex",
        ] {
            assert!(
                dict_get(row, field.as_bytes()).is_some(),
                "_UpdateColonyPresence reads and constructs {field}"
            );
        }
        assert_eq!(
            dict_get(row, b"planetID"),
            Some(&PyValue::Int(PLANET)),
            "and the planet is the one the colony is on"
        );
        assert_eq!(
            dict_get(row, b"numberOfPins"),
            Some(&PyValue::Int(4)),
            "numberOfPins is len(colony.colonyData.pins)"
        );
    }

    /// `radius` must marshal as a Python **float**.
    ///
    /// `eveplanet/verification.py::_VerifyDiamater` rejects a non-float
    /// outright with `PITemplateVerificationFailed`, and every planet radius
    /// is integer-valued -- so an `Int` here would always be sent and always
    /// be refused.
    #[test]
    fn the_planet_radius_is_a_real_and_never_an_int() {
        let mut reg = registry_with_a_colony();
        for actor in [None, Some(OWNER)] {
            let out = call_as(
                &mut reg,
                "GetPlanetInfo",
                vec![],
                actor,
                Some(PyValue::Int(PLANET)),
            )
            .expect("GetPlanetInfo must be answered");
            let PyValue::Instance { state, .. } = &out else {
                panic!("{out:?}")
            };
            let radius = dict_get(state, b"radius").expect("PreparePlanet reads .radius");
            assert!(
                matches!(radius, PyValue::Real(_)),
                "Diam = radius * 2 / 1000 is Python 2 integer division: {radius:?}"
            );
        }
    }

    /// With a character, `GetPlanetInfo` merges the colony's six keys into
    /// the same `KeyVal` -- `_PrimeColony` is handed this object and reads
    /// `.pins` off it directly rather than off a nested field.
    #[test]
    fn a_colonised_planet_carries_its_colony_in_the_same_keyval() {
        let mut reg = registry_with_a_colony();
        let out = call_as(
            &mut reg,
            "GetPlanetInfo",
            vec![],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
        .expect("GetPlanetInfo must be answered");
        let PyValue::Instance { state, .. } = &out else {
            panic!("{out:?}")
        };
        let row = state.as_ref();
        for field in [
            "planetID",
            "solarSystemID",
            "planetTypeID",
            "radius",
            "celestialIndex",
            "ownerID",
            "pins",
            "links",
            "routes",
            "level",
            "currentSimTime",
        ] {
            assert!(
                dict_get(row, field.as_bytes()).is_some(),
                "{field} is missing: {state:?}"
            );
        }
        assert!(matches!(dict_get(row, b"pins"), Some(PyValue::List(_))));
        assert_eq!(dict_get(row, b"level"), Some(&PyValue::Int(1)));
    }

    /// A pin row's **field set depends on its class**, which is why it is a
    /// `KeyVal` and not a `DBRow`: there is no single column layout.
    ///
    /// Checked per class, because an ECU row missing `heads` and a factory
    /// row missing `schematicID` both raise inside
    /// `ClientPlanet.RestorePinFromRow` -- and the row that raises is the one
    /// the window was drawing.
    #[test]
    fn each_pin_class_carries_its_own_extra_fields() {
        let s = statics();
        let now = crate::pyutil::filetime_now();
        let base = [
            "id",
            "latitude",
            "longitude",
            "ownerID",
            "lastRunTime",
            "typeID",
            "contents",
            "state",
        ];
        let cases: Vec<(i64, Vec<&str>)> = vec![
            (TYPE_CC, vec!["lastLaunchTime"]),
            (TYPE_LAUNCHPAD, vec!["lastLaunchTime"]),
            (
                TYPE_FACTORY,
                vec![
                    "schematicID",
                    "hasReceivedInputs",
                    "receivedInputsLastCycle",
                ],
            ),
            (
                TYPE_ECU,
                vec![
                    "cycleTime",
                    "programType",
                    "qtyPerCycle",
                    "expiryTime",
                    "installTime",
                    "headRadius",
                    "heads",
                ],
            ),
        ];
        for (type_id, extra) in cases {
            let pin = Pin::new(7, type_id, OWNER, SurfacePoint::new(1.0, 0.0), now);
            let out = pin_row(&s, &pin);
            let PyValue::Instance { class, state } = &out else {
                panic!("RestorePinFromRow reads attributes: {out:?}")
            };
            assert_eq!(class.as_slice(), crate::pyutil::KEYVAL_CLASS.as_bytes());
            let row = state.as_ref();
            for field in base.iter().chain(extra.iter()) {
                assert!(
                    dict_get(row, field.as_bytes()).is_some(),
                    "type {type_id} is missing {field}: {state:?}"
                );
            }
        }
        // And `heads` is a list of 3-tuples, because `FindHead` does
        // `head[0]` and `SetHeads` reads `head[1]`/`head[2]`.
        let mut ecu = Pin::new(2, TYPE_ECU, OWNER, SurfacePoint::new(1.0, 0.0), now);
        ecu.heads.push(Head {
            head_id: 3,
            latitude: 1.0,
            longitude: 0.0,
        });
        let PyValue::Instance { state, .. } = pin_row(&s, &ecu) else {
            unreachable!()
        };
        let heads = dict_get(&state, b"heads").expect("ecu rows carry heads");
        let PyValue::List(items) = heads else {
            panic!("iterated: {heads:?}")
        };
        let PyValue::Tuple(parts) = &items[0] else {
            panic!("subscripted positionally: {:?}", items[0])
        };
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], PyValue::Int(3));
    }

    /// `contents` is keyed by **int** typeID.
    ///
    /// A string key would miss every lookup and return the default silently,
    /// which is the one failure mode with no symptom at all -- the same trap
    /// `service/industry.rs` documents for its activity-keyed dicts.
    #[test]
    fn pin_contents_are_keyed_by_integer_type_id() {
        let s = statics();
        let mut pin = Pin::new(
            4,
            TYPE_LAUNCHPAD,
            OWNER,
            SurfacePoint::new(1.0, 0.0),
            crate::pyutil::filetime_now(),
        );
        pin.contents.insert(PRODUCT, 40);
        let PyValue::Instance { state, .. } = pin_row(&s, &pin) else {
            unreachable!()
        };
        let contents = dict_get(&state, b"contents").expect("every pin row has contents");
        let PyValue::Dict(entries) = contents else {
            panic!("subscripted by typeID: {contents:?}")
        };
        assert_eq!(entries.len(), 1);
        // Wire order is (value, key); see `PyValue::Dict`.
        let (value, key) = &entries[0];
        assert_eq!(key, &PyValue::Int(PRODUCT), "an int key, not b'3645'");
        assert_eq!(value, &PyValue::Int(40));
    }

    /// `GetFullNetworkForOwner` is a 2-tuple whose links are **bare pairs**.
    ///
    /// `planetSvc.GetColonyForCharacter` does `pins, links = ...` and then
    /// unpacks two names from each link, where `baseColony.Serialize`'s links
    /// carry three. Two shapes for one concept, and the client picks by which
    /// call it made.
    #[test]
    fn the_network_is_a_pair_of_lists_whose_links_are_two_element() {
        let mut reg = registry_with_a_colony();
        let out = call_as(
            &mut reg,
            "GetFullNetworkForOwner",
            vec![PyValue::Int(PLANET), PyValue::Int(OWNER)],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
        .expect("GetFullNetworkForOwner must be answered");
        let PyValue::Tuple(parts) = &out else {
            panic!("UNPACK_SEQUENCE 2: {out:?}")
        };
        assert_eq!(parts.len(), 2);
        let PyValue::List(pins) = &parts[0] else {
            panic!("iterated: {:?}", parts[0])
        };
        assert_eq!(pins.len(), 4);
        let PyValue::List(links) = &parts[1] else {
            panic!("iterated: {:?}", parts[1])
        };
        assert_eq!(links.len(), 3);
        for link in links {
            let PyValue::List(ends) = link else {
                panic!("unpacked into two names: {link:?}")
            };
            assert_eq!(ends.len(), 2, "two, not three, for this call");
        }
    }

    /// The other characters on the planet are visible, which is the one place
    /// PI is multiplayer.
    #[test]
    fn another_characters_command_centre_is_listed_on_the_same_planet() {
        let mut reg = registry_with_a_colony();
        let now = crate::pyutil::filetime_now();
        let other = OWNER + 1;
        let mut theirs = Colony::new(PLANET, other, 2016, now);
        theirs.pins.push(Pin::new(
            50,
            TYPE_CC,
            other,
            SurfacePoint::new(2.0, 1.0),
            now,
        ));
        reg.planet.insert(theirs);
        let out = call_as(
            &mut reg,
            "GetCommandPinsForPlanet",
            vec![PyValue::Int(PLANET)],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
        .expect("GetCommandPinsForPlanet must be answered");
        let PyValue::List(rows) = &out else {
            panic!("iterated: {out:?}")
        };
        assert_eq!(rows.len(), 2, "both colonies' command centres: {rows:?}");
        let owners: Vec<PyValue> = rows
            .iter()
            .map(|r| {
                let PyValue::Instance { state, .. } = r else {
                    panic!("{r:?}")
                };
                dict_get(state, b"ownerID")
                    .expect("each row names its owner")
                    .clone()
            })
            .collect();
        assert!(owners.contains(&PyValue::Int(OWNER)));
        assert!(owners.contains(&PyValue::Int(other)));
        // But `GetPlanetsForChar` is still only yours.
        let mine =
            call_as(&mut reg, "GetPlanetsForChar", vec![], Some(other), None).expect("answered");
        let PyValue::List(rows) = &mine else {
            panic!("{mine:?}")
        };
        assert_eq!(rows.len(), 1, "one colony each, not two");
    }

    /// `GetProgramResultInfo`'s cycle half agrees with the slider's own
    /// arithmetic, and with no heads `CreateProgram`'s sum is empty.
    #[test]
    fn a_program_quote_with_no_heads_reports_the_real_cycle_and_no_yield() {
        let mut reg = registry_with_a_colony();
        for (radius, cycle_hours, cycles) in [(0.01_f64, 0.25_f64, 4_i64), (0.05, 4.0, 84)] {
            let out = call_as(
                &mut reg,
                "GetProgramResultInfo",
                vec![
                    PyValue::Int(2),
                    PyValue::Int(RESOURCE),
                    PyValue::List(vec![]),
                    PyValue::Real(radius),
                ],
                Some(OWNER),
                Some(PyValue::Int(PLANET)),
            )
            .expect("GetProgramResultInfo must be answered");
            let PyValue::Tuple(parts) = &out else {
                panic!("three names: {out:?}")
            };
            assert_eq!(
                parts[0],
                PyValue::Int(0),
                "CreateProgram sums over the heads, and there are none"
            );
            assert_eq!(
                parts[1],
                PyValue::Int((cycle_hours * crate::planet::HOUR as f64) as i64),
                "radius {radius} is a {cycle_hours}h cycle"
            );
            assert_eq!(parts[2], PyValue::Int(cycles));
        }
    }

    /// A read advances the colony, and advancing it is the only thing a read
    /// does to the world.
    ///
    /// This is what makes the simulation safe on the draw path: no store, no
    /// lock, no I/O -- `docs/reference/world-state.md` §4 rule 4.
    #[test]
    fn reading_a_colony_advances_its_simulation_and_nothing_else() {
        let mut reg = registry_with_a_colony();
        let before = reg
            .planet
            .get(PLANET, OWNER)
            .expect("the fixture colony")
            .current_sim_time;
        call_as(
            &mut reg,
            "GetFullNetworkForOwner",
            vec![PyValue::Int(PLANET), PyValue::Int(OWNER)],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
        .expect("answered");
        let after = reg
            .planet
            .get(PLANET, OWNER)
            .expect("still there")
            .current_sim_time;
        assert!(
            after >= before,
            "the clock moves forwards or not at all: {before} -> {after}"
        );
        // And nothing else in the world was touched: no notification was
        // queued by a read.
        assert!(
            reg.take_pending().is_empty(),
            "a read must not push a notification"
        );
    }

    /// A call with no planet to act on is `BadArguments`, not a guess.
    ///
    /// `planetMgr` is bound per planet, so a call that carries neither a bind
    /// param nor an explicit id is malformed. Answering about *some* planet
    /// would be the `characters.first()` mistake in a new place.
    #[test]
    fn a_call_with_no_planet_is_refused_rather_than_guessed() {
        let mut reg = registry_with_a_colony();
        match call_as(&mut reg, "GetPlanetInfo", vec![], Some(OWNER), None) {
            Err(ServiceError::BadArguments { .. }) => {}
            other => panic!("a bound call with no binding must be refused: {other:?}"),
        }
        // And a bind param carried as a 1-tuple works, because we cannot see
        // which shape the resolve chose.
        assert!(
            call_as(
                &mut reg,
                "GetPlanetInfo",
                vec![],
                Some(OWNER),
                Some(PyValue::Tuple(vec![PyValue::Int(PLANET)])),
            )
            .is_ok(),
            "a 1-tuple bind param is the same binding"
        );
    }

    /// The staff tools this module deliberately leaves alone.
    ///
    /// Pinned as a test so that serving one is a decision somebody makes on
    /// purpose rather than a line that drifts in. The player-facing write
    /// path is served now (P5.3); these eight are GM-only.
    #[test]
    fn the_gm_methods_are_left_unanswered_on_purpose() {
        let mut reg = registry_with_a_colony();
        for method in [
            "GMGetSynchedServerState",
            "GMForceInstallProgram",
            "GMConvertCommandCenter",
            "GMAddCommodity",
            "GMGetLocalDistributionReport",
            "GMRunDepletionSim",
            "GMCreateNuggetLayer",
            "GMGetCompleteResource",
        ] {
            assert!(
                matches!(
                    call_as(
                        &mut reg,
                        method,
                        vec![PyValue::Int(1)],
                        Some(OWNER),
                        Some(PyValue::Int(PLANET))
                    ),
                    Err(ServiceError::NoSuchMethod { .. })
                ),
                "{method} is unserved on purpose; see docs/lanes/planet.md"
            );
        }
    }

    /// A refusal from the simulation reaches the client as the `UserError`
    /// the client itself raises, and not as a `CallRsp`.
    ///
    /// The PI window pre-validates against the same message ids, so the name
    /// is the contract: a refusal under the wrong name is a message the
    /// player cannot act on.
    #[test]
    fn a_simulation_refusal_becomes_the_user_error_the_client_names() {
        let err = pi_error(crate::planet::PiError::CannotBuildMultipleCommandPins);
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = err else {
            panic!("the client catches UserError by name")
        };
        let PyValue::Tuple(parts) = *header else {
            panic!("a 0x22 header is a 2-tuple")
        };
        assert_eq!(
            parts[0],
            PyValue::Global(b"eveexceptions.UserError".to_vec()),
            "it must raise the class the client catches"
        );
    }

    // --- P5.3: the write path ------------------------------------------------
    //
    // The fixture below is labelled as one: its prices, planet type and
    // radius are numbers a test can reason about, not CCP's. What the tests
    // assert is the client's own contract -- the wire shapes `_PrimeColony`
    // and `LaunchCommodities` read, and the invariants of a submit (all or
    // nothing, the item consumed, the bill) -- never a fixture value as if
    // it were CCP's.

    /// Fixture prices and holds.
    const PRICE_ECU: f64 = 45_000.0;
    const PRICE_FACTORY: f64 = 75_000.0;
    const PLANET_TYPE: i64 = 2016;
    const PLANET_RADIUS: f64 = 4_000_000.0;
    const SYSTEM: i64 = 30_000_001;
    const FLAG_CARGO: i64 = 5;

    /// Item rows for the store, in [`pi_statics`] (CP60: per `Content`, no
    /// longer process-wide).
    fn fixture_item_rows() -> Vec<crate::planet::ItemRow> {
        let row = |type_id, group_id, category_id, volume, name: &str| crate::planet::ItemRow {
            type_id,
            group_id,
            category_id,
            volume,
            capacity: 0.0,
            name: name.to_owned(),
        };
        vec![
            row(
                TYPE_CC,
                GROUP_COMMAND_PINS,
                41,
                1000.0,
                "fixture command centre",
            ),
            row(RESOURCE, 1033, 42, 0.005, "fixture resource"),
            row(PRODUCT, 1042, 43, 0.19, "fixture product"),
        ]
    }

    pub(crate) fn pi_statics() -> Statics {
        fixture_item_rows().into_iter().fold(
            statics()
                .with_base_price(TYPE_ECU, PRICE_ECU)
                .with_base_price(TYPE_FACTORY, PRICE_FACTORY)
                .with_resource(PLANET_TYPE, RESOURCE, 1.0),
            Statics::with_item_row,
        )
    }

    /// An empty world that knows one planet (from a celestial extract) and
    /// the fixture statics, with the item rows registered.
    fn pi_registry() -> Registry {
        let config = SandboxConfig {
            celestials: crate::celestial::Catalogue::parse(&format!(
                "# solarSystemID\t{SYSTEM}\n\
                 planet\t{}\t{PLANET_TYPE}\t{PLANET_RADIUS}\t1.0\t0.0\t0.0\n\
                 planet\t{}\t{PLANET_TYPE}\t{PLANET_RADIUS}\t9.0\t0.0\t0.0\n",
                PLANET - 5,
                PLANET
            )),
            ..SandboxConfig::default()
        };
        let mut reg = Registry::new(config);
        reg.planet = Colonies::new();
        reg.content = std::sync::Arc::new(crate::planet::content_with(pi_statics()));
        // Enough for any fixture bill; the wallet test lowers it.
        reg.character_mut(OWNER)
            .expect("the default character")
            .balance = 10_000_000.0;
        reg
    }

    /// Put one command centre in the actor's active ship's cargo and return
    /// its item id.
    fn command_centre_in_cargo(reg: &mut Registry) -> i64 {
        let ship = reg.character(OWNER).expect("the default character").ship_id;
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: TYPE_CC,
                quantity: 1,
                to: crate::item::Slot::new(OWNER, ship, FLAG_CARGO),
            }])
            .expect("the command centre is a registered type");
        changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("one item created")
    }

    /// `(commandID, argsTuple)`, as `CommandStream.Serialize` builds it.
    fn command(op: i64, args: Vec<PyValue>) -> PyValue {
        PyValue::Tuple(vec![PyValue::Int(op), PyValue::Tuple(args)])
    }

    fn temp(n: i64) -> PyValue {
        PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(n)])
    }

    fn submit(reg: &mut Registry, stream: Vec<PyValue>) -> Result<PyValue, ServiceError> {
        call_as(
            reg,
            "UserUpdateNetwork",
            vec![PyValue::List(stream)],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
    }

    fn place_command_centre(reg: &mut Registry) -> i64 {
        let cc = command_centre_in_cargo(reg);
        submit(
            reg,
            vec![command(
                1,
                vec![
                    PyValue::Int(cc),
                    PyValue::Int(TYPE_CC),
                    PyValue::Real(1.5),
                    PyValue::Real(0.5),
                ],
            )],
        )
        .expect("a command centre from the hold is accepted");
        cc
    }

    fn attr<'a>(v: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("expected a KeyVal, got {v:?}")
        };
        dict_get(state, name.as_bytes()).unwrap_or_else(|| panic!("no .{name} on {v:?}"))
    }

    fn user_error_id(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("expected a raised UserError, got {e:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("{header:?}")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("{parts:?}")
        };
        let PyValue::Bytes(id) = &args[0] else {
            panic!("{args:?}")
        };
        String::from_utf8(id.clone()).expect("ascii")
    }

    /// **The defect this packet found by reading the client's reader.**
    /// `ColonyData.RestoreLinkFromRow` reads `.typeID`, `.endpoint1`,
    /// `.endpoint2` and `.level` **by attribute** (`colonyData.pyc:620-624`),
    /// so a link in `GetPlanetInfo`'s colony -- and in a submit's reply --
    /// must be a `KeyVal` with those four names. A list raises
    /// `AttributeError` in `_PrimeColony`.
    #[test]
    fn a_colonys_links_are_keyvals_the_client_reads_by_name() {
        let mut reg = registry_with_a_colony();
        let info = call_as(
            &mut reg,
            "GetPlanetInfo",
            vec![],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
        .expect("answered");
        let PyValue::List(links) = attr(&info, "links") else {
            panic!("links is a list")
        };
        assert!(!links.is_empty(), "the fixture colony has links");
        for link in links {
            assert_eq!(attr(link, "typeID"), &PyValue::Int(TYPE_PLANETARY_LINK));
            assert!(matches!(attr(link, "endpoint1"), PyValue::Int(_)));
            assert!(matches!(attr(link, "endpoint2"), PyValue::Int(_)));
            assert!(matches!(attr(link, "level"), PyValue::Int(_)));
        }
    }

    /// An uncolonised planet is openable: `PreparePlanet` reads its system,
    /// type and radius off `GetPlanetInfo`, and they come from the celestial
    /// extract rather than as zeros.
    #[test]
    fn an_uncolonised_planet_reads_its_type_and_radius_from_the_catalogue() {
        let mut reg = pi_registry();
        let info = call_as(
            &mut reg,
            "GetPlanetInfo",
            vec![],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
        .expect("answered");
        assert_eq!(attr(&info, "planetTypeID"), &PyValue::Int(PLANET_TYPE));
        assert_eq!(attr(&info, "radius"), &PyValue::Real(PLANET_RADIUS));
        assert_eq!(attr(&info, "solarSystemID"), &PyValue::Int(SYSTEM));
        let PyValue::Instance { state, .. } = &info else {
            unreachable!("attr() above already read it as a KeyVal")
        };
        assert!(
            dict_get(state, b"pins").is_none(),
            "no colony, so no colony keys: PreparePlanet primes one only when \
             `pins` is present"
        );
    }

    /// `GetPlanetResourceInfo` is `{resourceTypeID: quality}`, int-keyed,
    /// and a resource the planet type does not carry is absent.
    #[test]
    fn the_resource_info_is_int_keyed_and_lists_only_what_the_planet_has() {
        let mut reg = pi_registry();
        let out = call_as(
            &mut reg,
            "GetPlanetResourceInfo",
            vec![],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
        .expect("answered");
        let PyValue::Dict(entries) = &out else {
            panic!("{out:?}")
        };
        assert_eq!(entries.len(), 1, "one resource in the fixture");
        let (quality, key) = &entries[0];
        assert_eq!(key, &PyValue::Int(RESOURCE), "keyed by integer type id");
        assert!(matches!(quality, PyValue::Real(q) if *q > 0.0));
    }

    /// The first submit of a colony: the command centre comes out of the
    /// ship's hold **as the pin** -- same id -- and the item is gone.
    #[test]
    fn a_command_centre_is_deployed_from_the_ships_hold_and_consumed() {
        let mut reg = pi_registry();
        let cc = command_centre_in_cargo(&mut reg);
        let balance = reg.character(OWNER).unwrap().balance;
        let reply = submit(
            &mut reg,
            vec![command(
                1,
                vec![
                    PyValue::Int(cc),
                    PyValue::Int(TYPE_CC),
                    PyValue::Real(1.5),
                    PyValue::Real(0.5),
                ],
            )],
        )
        .expect("accepted");
        // `_PrimeColony` reads these five off the reply.
        for key in ["pins", "links", "routes", "level", "currentSimTime"] {
            attr(&reply, key);
        }
        let PyValue::List(pins) = attr(&reply, "pins") else {
            panic!("pins is a list")
        };
        assert_eq!(pins.len(), 1);
        assert_eq!(
            attr(&pins[0], "id"),
            &PyValue::Int(cc),
            "the item id is the pin id"
        );
        assert!(
            reg.store().get(cc).is_none(),
            "the command centre was consumed"
        );
        assert_eq!(
            reg.character(OWNER).unwrap().balance,
            balance,
            "a command centre is paid for with the item, not with ISK"
        );
        let planets = call_as(&mut reg, "GetPlanetsForChar", vec![], Some(OWNER), None).unwrap();
        let PyValue::List(rows) = &planets else {
            panic!("{planets:?}")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(attr(&rows[0], "solarSystemID"), &PyValue::Int(SYSTEM));
        assert_eq!(attr(&rows[0], "celestialIndex"), &PyValue::Int(2));
    }

    /// No command centre in the hold: `CannotBuildCommandPin`, and nothing
    /// changed anywhere.
    #[test]
    fn a_command_centre_that_is_not_in_the_hold_is_refused() {
        let mut reg = pi_registry();
        let err = submit(
            &mut reg,
            vec![command(
                1,
                vec![
                    PyValue::Int(123_456_789),
                    PyValue::Int(TYPE_CC),
                    PyValue::Real(1.5),
                    PyValue::Real(0.5),
                ],
            )],
        )
        .expect_err("refused");
        assert_eq!(user_error_id(&err), "CannotBuildCommandPin");
        assert!(
            reg.planet.get(PLANET, OWNER).is_none(),
            "no colony was made"
        );
    }

    /// Every other pin costs its type's base price, charged once per submit
    /// as Planetary Construction (journal type 98) -- and temporary ids come
    /// back as real ones.
    #[test]
    fn a_submit_charges_each_new_pin_its_base_price() {
        let mut reg = pi_registry();
        let cc = place_command_centre(&mut reg);
        let before = reg.character(OWNER).unwrap().balance;
        let journal_before = reg.journal().entries().len();
        let reply = submit(
            &mut reg,
            vec![
                command(
                    1,
                    vec![
                        temp(1),
                        PyValue::Int(TYPE_ECU),
                        PyValue::Real(1.5),
                        PyValue::Real(0.52),
                    ],
                ),
                command(
                    1,
                    vec![
                        temp(2),
                        PyValue::Int(TYPE_FACTORY),
                        PyValue::Real(1.5),
                        PyValue::Real(0.54),
                    ],
                ),
                command(3, vec![PyValue::Int(cc), temp(1), PyValue::Int(0)]),
                command(3, vec![temp(1), temp(2), PyValue::Int(0)]),
            ],
        )
        .expect("accepted");
        let after = reg.character(OWNER).unwrap().balance;
        assert_eq!(before - after, PRICE_ECU + PRICE_FACTORY);
        let entries = &reg.journal().entries()[journal_before..];
        assert_eq!(entries.len(), 1, "one charge per submit");
        assert_eq!(entries[0].entry_type_id, REF_PLANETARY_CONSTRUCTION);
        let PyValue::List(pins) = attr(&reply, "pins") else {
            panic!()
        };
        assert_eq!(pins.len(), 3);
        for pin in pins {
            assert!(
                matches!(attr(pin, "id"), PyValue::Int(n) if *n > 0),
                "every temporary id became a real one"
            );
        }
        let PyValue::List(links) = attr(&reply, "links") else {
            panic!()
        };
        assert_eq!(links.len(), 2);
    }

    /// A bill the wallet cannot cover is `CannotBuildNotEnoughCash`, and the
    /// colony is exactly as it was.
    #[test]
    fn a_submit_the_wallet_cannot_cover_changes_nothing() {
        let mut reg = pi_registry();
        place_command_centre(&mut reg);
        reg.character_mut(OWNER).unwrap().balance = PRICE_ECU - 1.0;
        let before = reg.planet.get(PLANET, OWNER).cloned();
        let err = submit(
            &mut reg,
            vec![command(
                1,
                vec![
                    temp(1),
                    PyValue::Int(TYPE_ECU),
                    PyValue::Real(1.5),
                    PyValue::Real(0.52),
                ],
            )],
        )
        .expect_err("refused");
        assert_eq!(user_error_id(&err), "CannotBuildNotEnoughCash");
        assert_eq!(reg.planet.get(PLANET, OWNER).cloned(), before);
        assert_eq!(reg.character(OWNER).unwrap().balance, PRICE_ECU - 1.0);
    }

    /// A colony on a second planet needs Interplanetary Consolidation, as
    /// `ClientColony.PostValidateCreatePin` says; the refused command centre
    /// stays in the hold.
    #[test]
    fn a_second_colony_needs_interplanetary_consolidation() {
        let mut reg = pi_registry();
        place_command_centre(&mut reg);
        let cc = command_centre_in_cargo(&mut reg);
        let err = call_as(
            &mut reg,
            "UserUpdateNetwork",
            vec![PyValue::List(vec![command(
                1,
                vec![
                    PyValue::Int(cc),
                    PyValue::Int(TYPE_CC),
                    PyValue::Real(1.5),
                    PyValue::Real(0.5),
                ],
            )])],
            Some(OWNER),
            Some(PyValue::Int(PLANET - 5)),
        )
        .expect_err("one colony without the skill");
        assert_eq!(
            user_error_id(&err),
            "CannotPlaceCommandCenterNotEnoughSkill"
        );
        assert!(reg.store().get(cc).is_some(), "the item was not consumed");
    }

    /// Upgrading the command centre needs Command Center Upgrades at the
    /// target level.
    #[test]
    fn an_upgrade_beyond_the_skill_is_refused() {
        let mut reg = pi_registry();
        let cc = place_command_centre(&mut reg);
        let err = submit(
            &mut reg,
            vec![command(9, vec![PyValue::Int(cc), PyValue::Int(1)])],
        )
        .expect_err("untrained");
        assert_eq!(user_error_id(&err), "CantUpgradeCommandCenterSkillRequired");
    }

    /// A malformed stream is refused whole: half a stream is a colony the
    /// player did not build.
    #[test]
    fn a_malformed_stream_is_refused_whole() {
        let mut reg = pi_registry();
        let cc = command_centre_in_cargo(&mut reg);
        let err = submit(
            &mut reg,
            vec![
                command(
                    1,
                    vec![
                        PyValue::Int(cc),
                        PyValue::Int(TYPE_CC),
                        PyValue::Real(1.5),
                        PyValue::Real(0.5),
                    ],
                ),
                command(99, vec![]),
            ],
        )
        .expect_err("unknown opcode");
        assert!(matches!(err, ServiceError::BadArguments { .. }));
        assert!(reg.store().get(cc).is_some());
        assert!(reg.planet.get(PLANET, OWNER).is_none());
    }

    /// All thirteen opcodes decode into the command the simulation applies,
    /// with the argument tuples `CommandStream.Serialize` builds.
    #[test]
    fn every_opcode_decodes() {
        use crate::planet::{Command, Id};
        let real = PyValue::Real(0.5);
        let stream = PyValue::List(vec![
            command(
                1,
                vec![temp(1), PyValue::Int(TYPE_ECU), real.clone(), real.clone()],
            ),
            command(2, vec![PyValue::Int(7)]),
            command(3, vec![PyValue::Int(7), temp(1), PyValue::Int(2)]),
            command(4, vec![PyValue::Int(7), temp(1)]),
            command(5, vec![PyValue::Int(7), temp(1), PyValue::Int(3)]),
            command(
                6,
                vec![
                    PyValue::Tuple(vec![PyValue::Int(2), PyValue::Int(1)]),
                    PyValue::List(vec![PyValue::Int(7), temp(1)]),
                    PyValue::Int(RESOURCE),
                    PyValue::Int(10),
                ],
            ),
            command(7, vec![PyValue::Int(4)]),
            command(8, vec![PyValue::Int(7), PyValue::Int(121)]),
            command(9, vec![PyValue::Int(7), PyValue::Int(2)]),
            command(
                10,
                vec![PyValue::Int(7), PyValue::Int(0), real.clone(), real.clone()],
            ),
            command(11, vec![PyValue::Int(7), PyValue::Int(0)]),
            command(
                12,
                vec![PyValue::Int(7), PyValue::Int(0), real.clone(), real.clone()],
            ),
            command(
                13,
                vec![PyValue::Int(7), PyValue::None, PyValue::Real(0.01)],
            ),
        ]);
        let commands = decode_stream(&stream).expect("well formed");
        let opcodes: Vec<i64> = commands.iter().map(Command::opcode).collect();
        assert_eq!(opcodes, (1..=13).collect::<Vec<_>>());
        assert_eq!(
            commands[0],
            Command::CreatePin {
                pin_id: Id::Temporary(vec![1, 1]),
                type_id: TYPE_ECU,
                point: SurfacePoint::new(0.5, 0.5),
            }
        );
        assert!(matches!(
            &commands[12],
            Command::InstallProgram {
                program_type: None,
                ..
            }
        ));
    }

    /// A launch takes the commodities off the command centre, lands them in
    /// the station hangar, and answers the launch time the client stores --
    /// and a second launch inside the minute is refused.
    #[test]
    fn a_launch_lands_in_the_hangar_and_starts_the_cooldown() {
        let mut reg = pi_registry();
        let cc = place_command_centre(&mut reg);
        reg.planet
            .get_mut(PLANET, OWNER)
            .unwrap()
            .pin_mut(cc)
            .unwrap()
            .contents
            .insert(PRODUCT, 30);
        let station = reg.character(OWNER).unwrap().station_id.unwrap();
        let launch = |reg: &mut Registry, qty: i64| {
            call_as(
                reg,
                "UserLaunchCommodities",
                vec![
                    PyValue::Int(cc),
                    PyValue::Dict(vec![(PyValue::Int(qty), PyValue::Int(PRODUCT))]),
                ],
                Some(OWNER),
                Some(PyValue::Int(PLANET)),
            )
        };
        let out = launch(&mut reg, 20).expect("launched");
        assert!(matches!(out, PyValue::Int(t) if t > 0), "lastLaunchTime");
        let landed: i64 = reg
            .store()
            .owned_by(OWNER)
            .filter(|i| i.type_id == PRODUCT && i.location_id == station)
            .map(|i| i.quantity)
            .sum();
        assert_eq!(landed, 20);
        let left = reg
            .planet
            .get(PLANET, OWNER)
            .unwrap()
            .pin(cc)
            .unwrap()
            .quantity(PRODUCT);
        assert_eq!(left, 10);
        let err = launch(&mut reg, 5).expect_err("inside the minute");
        assert_eq!(user_error_id(&err), "CannotLaunchCommandPinNotReady");
    }

    /// An expedited transfer answers `(simTime, sourceRunTime)`, and the
    /// source's cooldown is at least the five-minute floor.
    #[test]
    fn an_expedited_transfer_answers_the_sim_time_and_the_source_cooldown() {
        let mut reg = registry_with_a_colony();
        // Launchpad 4 -> factory 3 is one link in the fixture. Put a batch
        // of the factory's input on the launchpad.
        reg.planet
            .get_mut(PLANET, OWNER)
            .unwrap()
            .pin_mut(4)
            .unwrap()
            .contents
            .insert(RESOURCE, 3000);
        // The fixture link type needs a bandwidth.
        let mut s = crate::planet::statics_in(reg.content()).clone();
        s = s.with_pin_type(PinType {
            type_id: TYPE_PLANETARY_LINK,
            group_id: 1036,
            capacity: 0.0,
            attributes: [(crate::planet::attr::LOGISTICAL_CAPACITY, 1250.0)]
                .into_iter()
                .collect(),
        });
        reg.content = std::sync::Arc::new(crate::planet::content_with(s));
        let out = call_as(
            &mut reg,
            "UserTransferCommodities",
            vec![
                PyValue::List(vec![PyValue::Int(4), PyValue::Int(3)]),
                PyValue::Dict(vec![(PyValue::Int(3000), PyValue::Int(RESOURCE))]),
            ],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
        .expect("transferred");
        let PyValue::Tuple(parts) = &out else {
            panic!("the client unpacks two: {out:?}")
        };
        let (PyValue::Int(sim), PyValue::Int(source_run)) = (&parts[0], &parts[1]) else {
            panic!("{parts:?}")
        };
        assert!(*source_run >= *sim + 5 * crate::planet::MIN - crate::planet::SEC);
        let colony = reg.planet.get(PLANET, OWNER).unwrap();
        assert_eq!(
            colony.pin(4).unwrap().quantity(RESOURCE),
            0,
            "the batch left"
        );
    }

    /// A colony survives the world file, and so do the id allocators.
    #[test]
    fn a_colony_survives_the_world_file() {
        let mut reg = pi_registry();
        let cc = place_command_centre(&mut reg);
        submit(
            &mut reg,
            vec![
                command(
                    1,
                    vec![
                        temp(1),
                        PyValue::Int(TYPE_ECU),
                        PyValue::Real(1.5),
                        PyValue::Real(0.52),
                    ],
                ),
                command(3, vec![PyValue::Int(cc), temp(1), PyValue::Int(0)]),
            ],
        )
        .expect("accepted");
        let snap = reg.to_snapshot();
        let mut text = String::new();
        snap.planets.render(&mut text);
        let mut back = crate::planet::Saved::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            back.parse_record(&f).expect("our own record");
        }
        assert_eq!(back, snap.planets, "render and parse are inverses");
        let mut fresh = pi_registry();
        fresh.apply_snapshot(snap);
        assert_eq!(
            fresh.planet.get(PLANET, OWNER),
            reg.planet.get(PLANET, OWNER),
            "the same colony after a restart"
        );
        assert_eq!(
            fresh.planet.saved().next_pin_id,
            reg.planet.saved().next_pin_id
        );
    }

    /// Abandoning removes the colony and nothing else.
    #[test]
    fn abandoning_removes_the_colony() {
        let mut reg = pi_registry();
        place_command_centre(&mut reg);
        let out = call_as(
            &mut reg,
            "UserAbandonPlanet",
            vec![],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
        .expect("answered");
        assert_eq!(out, PyValue::None);
        assert!(reg.planet.get(PLANET, OWNER).is_none());
    }

    /// The heatmap is answered with `data = None`, the one shape the client
    /// takes without handing bytes to native code.
    #[test]
    fn the_resource_heatmap_is_answered_with_no_data() {
        let mut reg = pi_registry();
        let out = call_as(
            &mut reg,
            "GetResourceData",
            vec![PyValue::None],
            Some(OWNER),
            Some(PyValue::Int(PLANET)),
        )
        .expect("answered");
        assert_eq!(attr(&out, "data"), &PyValue::None);
    }

    // -----------------------------------------------------------------
    // Customs offices (W9 L5)
    // -----------------------------------------------------------------
    //
    // Every call below is the client's own shape **[V]** --
    // `GetTaxRate(customsOfficeID)`, `List(flagHangar)`,
    // `Add(itemID, sourceID, qty=n)`, `ImportExportWithPlanet(pinID,
    // {itemID: qty}, {typeID: qty}, taxRate)` -- and what is asserted is the
    // client's arithmetic and conservation: no unit made or lost, no ISK
    // charged that the window did not quote.

    const OFFICE_RATE: f64 = 0.17;
    const PRODUCT_MULTIPLIER: f64 = 400.0;
    const LAUNCHPAD_IMPORT_TAX: f64 = 0.5;
    const LAUNCHPAD_EXPORT_TAX: f64 = 1.0;
    const LAUNCHPAD_PIN: i64 = 950_000_000_002;

    fn office_id() -> i64 {
        crate::planet::customs_office_id(PLANET)
    }

    /// `pi_registry`, plus the office type, a taxed launchpad, a colony
    /// holding one, and the owner **in space in the planet's system**.
    fn customs_registry() -> Registry {
        let mut reg = pi_registry();
        reg.content = std::sync::Arc::new(crate::planet::content_with(
            pi_statics()
                .with_pin_type(PinType {
                    type_id: TYPE_LAUNCHPAD,
                    group_id: GROUP_SPACEPORT_PINS,
                    capacity: 10_000.0,
                    attributes: [
                        (crate::planet::attr::IMPORT_TAX, LAUNCHPAD_IMPORT_TAX),
                        (crate::planet::attr::EXPORT_TAX, LAUNCHPAD_EXPORT_TAX),
                    ]
                    .into_iter()
                    .collect(),
                })
                .with_import_tax_multiplier(PRODUCT, PRODUCT_MULTIPLIER)
                .with_customs_type(crate::planet::CustomsType {
                    type_id: 4318,
                    capacity: 35_000.0,
                    radius: 8_000.0,
                    tax_rate: OFFICE_RATE,
                }),
        ));
        let now = crate::pyutil::filetime_now();
        let mut colony = Colony::new(PLANET, OWNER, PLANET_TYPE, now);
        colony.planet_radius = PLANET_RADIUS;
        colony.pins.push(Pin::new(
            LAUNCHPAD_PIN - 1,
            TYPE_CC,
            OWNER,
            SurfacePoint::new(1.5, 0.1),
            now,
        ));
        colony.pins.push(Pin::new(
            LAUNCHPAD_PIN,
            TYPE_LAUNCHPAD,
            OWNER,
            SurfacePoint::new(1.5, 0.2),
            now,
        ));
        reg.planet.insert(colony);
        let ship = reg.character(OWNER).unwrap().ship_id;
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                OWNER,
                ship,
                SYSTEM,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        reg
    }

    fn inventory_call(
        reg: &mut Registry,
        bound_on: i64,
        method: &str,
        args: Vec<PyValue>,
        kwargs: Vec<(PyValue, PyValue)>,
    ) -> Option<Result<PyValue, ServiceError>> {
        let bind = PyValue::Tuple(vec![PyValue::Int(bound_on), PyValue::Int(0)]);
        customs_inventory_call(
            reg,
            &Request {
                service: "invbroker::inventory",
                method,
                args: &args,
                kwargs: &kwargs,
                bind_params: Some(&bind),
                actor: Some(OWNER),
            },
        )
    }

    fn product_in_cargo(reg: &mut Registry, qty: i64) -> i64 {
        let ship = reg.character(OWNER).unwrap().ship_id;
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: PRODUCT,
                quantity: qty,
                to: crate::item::Slot::new(OWNER, ship, FLAG_CARGO),
            }])
            .expect("a registered commodity");
        changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("one item")
    }

    fn in_office(reg: &Registry) -> i64 {
        reg.store()
            .contents_of(office_id())
            .filter(|i| i.type_id == PRODUCT)
            .map(|i| i.quantity)
            .sum()
    }

    fn on_launchpad(reg: &Registry) -> i64 {
        reg.planet
            .get(PLANET, OWNER)
            .and_then(|c| c.pin(LAUNCHPAD_PIN))
            .map_or(0, |p| p.quantity(PRODUCT))
    }

    fn transfer(
        reg: &mut Registry,
        import: Vec<(i64, i64)>,
        export: Vec<(i64, i64)>,
        rate: f64,
    ) -> Result<PyValue, ServiceError> {
        let dict = |pairs: Vec<(i64, i64)>| {
            PyValue::Dict(
                pairs
                    .into_iter()
                    .map(|(k, v)| (PyValue::Int(v), PyValue::Int(k)))
                    .collect(),
            )
        };
        inventory_call(
            reg,
            office_id(),
            "ImportExportWithPlanet",
            vec![
                PyValue::Int(LAUNCHPAD_PIN),
                dict(import),
                dict(export),
                PyValue::Real(rate),
            ],
            vec![],
        )
        .expect("an office's inventory is PI's")
    }

    #[test]
    fn the_registry_quotes_the_offices_rate_and_nothing_for_anything_else() {
        let mut reg = customs_registry();
        let ask = |reg: &mut Registry, id: i64| {
            dispatch(
                reg,
                &Request {
                    service: "planetOrbitalRegistryBroker",
                    method: "GetTaxRate",
                    args: &[PyValue::Int(id)],
                    kwargs: &[],
                    bind_params: Some(&PyValue::Int(SYSTEM)),
                    actor: Some(OWNER),
                },
            )
            .expect("answered")
        };
        assert_eq!(ask(&mut reg, office_id()), PyValue::Real(OFFICE_RATE));
        assert_eq!(ask(&mut reg, PLANET), PyValue::None);
    }

    /// Calls that are not about an office are left to the inventory.
    #[test]
    fn the_inventory_keeps_everything_that_is_not_an_office() {
        let mut reg = customs_registry();
        let ship = reg.character(OWNER).unwrap().ship_id;
        assert!(inventory_call(&mut reg, ship, "List", vec![], vec![]).is_none());
        assert!(inventory_call(&mut reg, office_id(), "StackAll", vec![], vec![]).is_none());
    }

    /// Hold -> office -> launchpad -> office -> hold: every unit arrives
    /// where it was sent, and the bill is the window's own quote.
    #[test]
    fn goods_round_trip_through_the_office_and_the_bill_is_the_quote() {
        let mut reg = customs_registry();
        let item = product_in_cargo(&mut reg, 100);
        // Into the office: `Add(itemID, sourceID, qty=n)`, MoveFromInventory.
        let ship = reg.character(OWNER).unwrap().ship_id;
        inventory_call(
            &mut reg,
            office_id(),
            "Add",
            vec![PyValue::Int(item), PyValue::Int(ship)],
            crate::pyutil::pypairs(vec![("qty", PyValue::Int(100))]),
        )
        .expect("PI's")
        .expect("moved");
        assert_eq!(in_office(&reg), 100);
        let PyValue::List(rows) = inventory_call(
            &mut reg,
            office_id(),
            "List",
            vec![PyValue::Int(crate::planet::CUSTOMS_OFFICE_FLAG)],
            vec![],
        )
        .expect("PI's")
        .expect("listed") else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 1);

        // Import 60 of the 100.
        let balance = reg.character(OWNER).unwrap().balance;
        let journal = reg.journal().entries().len();
        transfer(&mut reg, vec![(item, 60)], vec![], OFFICE_RATE).expect("imported");
        assert_eq!((in_office(&reg), on_launchpad(&reg)), (40, 60));
        let import_tax = 60.0 * LAUNCHPAD_IMPORT_TAX * PRODUCT_MULTIPLIER * OFFICE_RATE;
        let paid = balance - reg.character(OWNER).unwrap().balance;
        assert!((paid - import_tax).abs() < 1e-6, "{paid} != {import_tax}");
        let entries = &reg.journal().entries()[journal..];
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry_type_id, REF_PLANETARY_IMPORT_TAX);
        let pushed: Vec<_> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| p.to_push())
            .collect();
        assert!(pushed.contains(&(
            "OnMajorPlanetStateUpdate",
            vec![PyValue::Int(PLANET), PyValue::Int(0)]
        )));

        // Export 25 back, and import the other 40, in one call.
        let balance = reg.character(OWNER).unwrap().balance;
        transfer(&mut reg, vec![(item, 40)], vec![(PRODUCT, 25)], OFFICE_RATE).expect("both ways");
        assert_eq!((in_office(&reg), on_launchpad(&reg)), (25, 75));
        assert_eq!(in_office(&reg) + on_launchpad(&reg), 100, "conserved");
        let quote = 40.0 * LAUNCHPAD_IMPORT_TAX * PRODUCT_MULTIPLIER * OFFICE_RATE
            + 25.0 * LAUNCHPAD_EXPORT_TAX * PRODUCT_MULTIPLIER * OFFICE_RATE;
        let paid = balance - reg.character(OWNER).unwrap().balance;
        assert!((paid - quote).abs() < 1e-6, "{paid} != {quote}");

        // Out of the office into the hold of a ship with room for it.
        let badger =
            *crate::item::find_type_in(reg.content(), "Badger").expect("a hull with a hold");
        let station = reg.character(OWNER).unwrap().station_id.unwrap();
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: badger.type_id,
                quantity: 1,
                to: crate::item::Slot::new(OWNER, station, crate::model::FLAG_HANGAR),
            }])
            .expect("a hull");
        let hull = changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .unwrap();
        reg.character_mut(OWNER).unwrap().ship_id = hull;
        let back = reg
            .store()
            .contents_of(office_id())
            .find(|i| i.type_id == PRODUCT)
            .map(|i| i.item_id)
            .unwrap();
        inventory_call(
            &mut reg,
            hull,
            "Add",
            vec![PyValue::Int(back), PyValue::Int(office_id())],
            crate::pyutil::pypairs(vec![
                ("qty", PyValue::Int(25)),
                ("flag", PyValue::Int(FLAG_CARGO)),
            ]),
        )
        .expect("an item leaving an office is PI's")
        .expect("into the hold");
        assert_eq!(in_office(&reg), 0);
        let in_hold: i64 = reg
            .store()
            .contents_of(hull)
            .filter(|i| i.type_id == PRODUCT && i.flag_id == FLAG_CARGO)
            .map(|i| i.quantity)
            .sum();
        assert_eq!(in_hold + on_launchpad(&reg), 100, "conserved end to end");
    }

    /// The window sends the rate it quoted; a different one is `TaxChanged`,
    /// caught by name (`importExportUI.py:788`), and nothing moves.
    #[test]
    fn a_stale_rate_is_tax_changed_and_moves_nothing() {
        let mut reg = customs_registry();
        let item = product_in_cargo(&mut reg, 10);
        let ship = reg.character(OWNER).unwrap().ship_id;
        inventory_call(
            &mut reg,
            office_id(),
            "Add",
            vec![PyValue::Int(item), PyValue::Int(ship)],
            vec![],
        )
        .unwrap()
        .unwrap();
        let balance = reg.character(OWNER).unwrap().balance;
        let err =
            transfer(&mut reg, vec![(item, 10)], vec![], OFFICE_RATE + 0.01).expect_err("stale");
        assert_eq!(user_error_id(&err), "TaxChanged");
        assert_eq!((in_office(&reg), on_launchpad(&reg)), (10, 0));
        assert_eq!(reg.character(OWNER).unwrap().balance, balance);
    }

    #[test]
    fn an_empty_transfer_asks_for_commodities() {
        let mut reg = customs_registry();
        let err = transfer(&mut reg, vec![], vec![], OFFICE_RATE).expect_err("empty");
        assert_eq!(user_error_id(&err), "PleaseSelectCommoditiesToImport");
    }

    /// `planetCommon.CanPutTypeInCustomsOffice` **[V]**: a command centre is
    /// not a commodity.
    #[test]
    fn only_planetary_goods_go_into_the_office() {
        let mut reg = customs_registry();
        let cc = command_centre_in_cargo(&mut reg);
        let ship = reg.character(OWNER).unwrap().ship_id;
        let err = inventory_call(
            &mut reg,
            office_id(),
            "Add",
            vec![PyValue::Int(cc), PyValue::Int(ship)],
            vec![],
        )
        .unwrap()
        .expect_err("refused");
        assert_eq!(user_error_id(&err), "CannotPutInCargolink");
        assert_eq!(reg.item(cc).unwrap().location_id, ship, "still in the hold");
    }

    /// A pilot who is not in the office's system cannot reach it.
    #[test]
    fn a_pilot_elsewhere_cannot_use_the_office() {
        let mut reg = customs_registry();
        let item = product_in_cargo(&mut reg, 10);
        let ship = reg.character(OWNER).unwrap().ship_id;
        reg.roster_mut().leave(OWNER);
        let err = inventory_call(
            &mut reg,
            office_id(),
            "Add",
            vec![PyValue::Int(item), PyValue::Int(ship)],
            vec![],
        )
        .unwrap()
        .expect_err("refused");
        assert_eq!(user_error_id(&err), "CannotImportNotInSameSolarSystem");
    }

    /// An export of more than the launchpad holds names both figures and
    /// the commodity -- the dialog's three loc args.
    #[test]
    fn an_export_of_more_than_is_there_moves_nothing() {
        let mut reg = customs_registry();
        reg.planet
            .get_mut(PLANET, OWNER)
            .unwrap()
            .pin_mut(LAUNCHPAD_PIN)
            .unwrap()
            .contents
            .insert(PRODUCT, 5);
        let err = transfer(&mut reg, vec![], vec![(PRODUCT, 6)], OFFICE_RATE).expect_err("short");
        assert_eq!(user_error_id(&err), "CannotExportCommodityNotEnough");
        assert_eq!((in_office(&reg), on_launchpad(&reg)), (0, 5));
    }

    // ---- E1: a logged-off farmer's colonies -----------------------------------

    /// A colony `hours` game hours old, fully wired: the extractor feeds the
    /// factory, the factory feeds the command centre, and a launchpad is
    /// there too. Its program was installed `hours` ago, so a long enough
    /// age has run it out.
    pub(crate) fn aged_colony_world(hours: i64) -> Registry {
        let mut reg = Registry::new(SandboxConfig::default());
        reg.planet = Colonies::new();
        reg.content = std::sync::Arc::new(crate::planet::content_with(pi_statics()));
        let then = crate::pyutil::filetime_now() - hours * crate::seed::HOUR;
        let mut colony = Colony::new(PLANET, OWNER, PLANET_TYPE, then);
        colony.planet_radius = PLANET_RADIUS;
        colony.level = 1;
        for (id, type_id) in [(1_i64, TYPE_CC), (2, TYPE_ECU), (3, TYPE_FACTORY)] {
            colony.pins.push(Pin::new(
                id,
                type_id,
                OWNER,
                SurfacePoint::new(1.5, 0.01 * id as f64),
                then,
            ));
        }
        colony.pin_mut(2).unwrap().heads.push(Head {
            head_id: 0,
            latitude: 1.5,
            longitude: 0.02,
        });
        colony
            .pin_mut(2)
            .unwrap()
            .install_program(&pi_statics(), Some(RESOURCE), 0.01, 3000, then)
            .expect("the fixture's quote is positive");
        colony
            .pin_mut(3)
            .unwrap()
            .install_schematic(&pi_statics(), 121)
            .expect("the factory may run schematic 121");
        colony.links.push(Link::new(2, 3, TYPE_PLANETARY_LINK, 0));
        colony.links.push(Link::new(3, 1, TYPE_PLANETARY_LINK, 0));
        colony.routes.push(Route {
            route_id: 1,
            owner_id: OWNER,
            path: vec![2, 3],
            commodity_type_id: RESOURCE,
            commodity_quantity: 3000,
        });
        colony.routes.push(Route {
            route_id: 2,
            owner_id: OWNER,
            path: vec![3, 1],
            commodity_type_id: PRODUCT,
            commodity_quantity: 20,
        });
        reg.planet.insert(colony);
        reg
    }

    fn command_centre_holds(reg: &Registry) -> i64 {
        reg.planet
            .get(PLANET, OWNER)
            .and_then(|c| c.pin(1))
            .map_or(0, |p| p.quantity(PRODUCT))
    }

    #[test]
    fn an_offline_tend_launches_what_the_colony_made_while_away() {
        let mut reg = aged_colony_world(20);
        let now = crate::pyutil::filetime_now();
        // What the colony made is the simulation's, whoever asks: a clone
        // simulated to the same instant is the figure to launch.
        let statics = crate::planet::statics_in(reg.content()).clone();
        let mut probe = reg.planet.clone();
        probe.advance(&statics, PLANET, OWNER, now);
        let made = probe
            .get(PLANET, OWNER)
            .and_then(|c| c.pin(1))
            .map_or(0, |p| p.quantity(PRODUCT));
        assert!(made > 0, "twenty hours of an extractor feeding a factory");
        let hangar = |reg: &Registry| {
            reg.store()
                .items()
                .iter()
                .filter(|i| i.owner_id == OWNER && i.type_id == PRODUCT)
                .map(|i| i.quantity)
                .sum::<i64>()
        };
        assert_eq!(hangar(&reg), 0);
        let tend = reg.settle_colonies(OWNER, now);
        assert_eq!(tend.colonies, 1);
        assert_eq!(tend.launched.get(&PRODUCT), Some(&made));
        assert_eq!(hangar(&reg), made, "the launch is in the hangar");
        assert_eq!(command_centre_holds(&reg), 0, "and out of the centre");
        // A second tend at once: the launch cooldown, and nothing new.
        let again = reg.settle_colonies(OWNER, now);
        assert!(again.launched.is_empty());
    }

    #[test]
    fn an_offline_tend_installs_the_program_again_when_it_ran_out() {
        let mut reg = aged_colony_world(200);
        // Only the extractor and the centre: the restart's validation checks
        // every route against the program it installs, as the client does,
        // and this test is about the program.
        let colony = reg.planet.get_mut(PLANET, OWNER).unwrap();
        colony.routes.clear();
        colony.links.clear();
        colony.links.push(Link::new(1, 2, TYPE_PLANETARY_LINK, 0));
        colony.pins.retain(|p| p.pin_id != 3);
        let now = crate::pyutil::filetime_now();
        let expiry = |reg: &Registry| {
            reg.planet
                .get(PLANET, OWNER)
                .and_then(|c| c.pin(2))
                .and_then(|p| p.expiry_time)
        };
        let before = expiry(&reg);
        let tend = reg.settle_colonies(OWNER, now);
        assert_eq!(tend.restarted, 1, "{tend:?}");
        assert!(
            expiry(&reg) > before,
            "a later expiry: {before:?} -> {:?}",
            expiry(&reg)
        );
        // A programme still running is left alone.
        let tend = reg.settle_colonies(OWNER, now + crate::planet::MIN);
        assert_eq!(tend.restarted, 0);
    }

    #[test]
    fn a_character_with_no_colony_tends_nothing() {
        let mut reg = aged_colony_world(5);
        let tend = reg.settle_colonies(OWNER + 1, crate::pyutil::filetime_now());
        assert_eq!(tend, Tend::default());
    }
}
