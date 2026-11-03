//! **The PI farmer makes P2** (lane FU-PI2, on SOC-B6's [`crate::pi`]).
//!
//! B6's template extracts one resource and runs a basic factory to P1. A
//! P2 schematic takes two P1 (`planetSchematicsTypeMap`: every P2 recipe
//! has two inputs [V-data]) and runs only in an advanced industry facility
//! (the schematic's `pin_type_ids` [V]). This module adds the colony that
//! makes one:
//!
//! - **Local legs.** For every input the planet can make itself, an
//!   extraction unit on the best resource whose single-input schematic
//!   gives that P1, and a basic factory on it ([`legs_of`]).
//! - **One imported leg** ([`MAX_IMPORTS`]). An input the planet cannot
//!   make but one of the bot's **other colonies** does (B6 launches its P1
//!   to the station hangar) comes in through the planet's customs office:
//!   the template adds a launchpad, and every visit the bot carries that
//!   P1 from the hangar to the office and imports it onto the launchpad
//!   ([`feeds`], [`feed`]).
//! - **The advanced factory** on the P2 schematic, linked to every basic
//!   factory and to the launchpad, and to the command centre, with one
//!   route per leg into it and one route of P2 to the command centre,
//!   which B6's tend already launches and sells (P2 is category 43 too).
//!
//! **The P2 is chosen by the market** ([`choose`]): among the schematics
//! reachable from a planet the bot can settle ([`templates`]), the best
//! hourly margin of the P2 over its two P1 at `P_mkt` (else `P_ref`) -- the
//! signal module's prices, read with [`crate::pricing::Prices`]. A P2
//! worth less than its inputs is not made (the planet then gets B6's P1
//! colony). A recipe without prices is still a candidate [A].
//!
//! **Every stream is dry-run** through `eve_server::planet::Colonies::apply`
//! before it is sent ([`fit`], [`with_routes`]), as B6's is, so the colony
//! the bot submits is one the client's own colony code accepts.
//!
//! **Server side** (verified, nothing added): an advanced factory is a
//! process pin (group 1028 for every factory tier [V-data]), the factory
//! runs any schematic whose `pin_type_ids` list it, consumes every input of
//! a multi-input schematic per cycle (`has_enough_inputs`), and pulls from
//! a storage pin (the launchpad) through a storage -> consumer route
//! (`route_commodity_input`); the module tests run that simulation.

use crate::pi::{
    self, Layout, Site, at, cheapest, id_value, int, kv, on_planet, real, ring, seq, temp,
};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::planet::{
    self as sim, Colonies, Colony, ColonyEdit, Command, Head, Id, PinKind, ProgramQuote, Statics,
    SurfacePoint,
};
use std::collections::{BTreeMap, BTreeSet};

/// Inputs one P2 colony imports at most. **[A]**: one launchpad, one
/// customs run a visit; falsified by farmers whose P2 colonies stand idle
/// for want of an input no second launchpad would fix.
pub const MAX_IMPORTS: usize = 1;
/// A P2 is preferred to a P1 colony whenever one is reachable and not
/// priced under its inputs. **[A]**: a player with the skill climbs the
/// chain; falsified by P2 farmers earning less an hour than P1 farmers in
/// a `[soc]` session.
pub const P2_FIRST: bool = true;

// Temporary ids of the template's pins (B6 uses (1, 1) and (1, 2)).
const ADVANCED: (i64, i64) = (1, 10);
const LAUNCHPAD: (i64, i64) = (1, 11);
fn basic(i: usize) -> (i64, i64) {
    (1, 20 + i as i64)
}
fn ecu(i: usize) -> (i64, i64) {
    (1, 30 + i as i64)
}

// ---- the template, pure -----------------------------------------------------

/// One input the planet makes itself: P0 -> P1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leg {
    pub p1: i64,
    pub resource: i64,
    /// The resource's quality here (any scale: it only ranks).
    pub quality: f64,
    pub schematic_id: i64,
    pub factory_type: i64,
}

/// **A P2 colony** for one planet type.
#[derive(Debug, Clone, PartialEq)]
pub struct P2Template {
    pub planet_type: i64,
    pub cc_type: i64,
    pub ecu_type: i64,
    /// The advanced industry facility, and its schematic.
    pub factory_type: i64,
    pub schematic_id: i64,
    pub product: i64,
    /// The inputs the planet makes, in the schematic's order.
    pub legs: Vec<Leg>,
    /// The input imported through the customs office, and the launchpad.
    pub import: Option<i64>,
    pub launchpad_type: Option<i64>,
    /// The worst leg's quality: what ranks two planets for one recipe.
    pub quality: f64,
}

/// The P1 a planet type makes from `qualities`: per P1, the best resource
/// a single-input schematic turns into it in a basic factory the planet
/// allows.
pub fn legs_of(
    statics: &Statics,
    planet_type: i64,
    qualities: &BTreeMap<i64, f64>,
) -> BTreeMap<i64, Leg> {
    let mut out: BTreeMap<i64, Leg> = BTreeMap::new();
    for s in statics.schematics() {
        let (&[(input, _)], &[(p1, _)]) = (s.inputs.as_slice(), s.outputs.as_slice()) else {
            continue;
        };
        let Some(&quality) = qualities.get(&input).filter(|q| **q > 0.0) else {
            continue;
        };
        let Some(factory_type) = cheapest(
            statics,
            PinKind::Process,
            planet_type,
            Some(&s.pin_type_ids),
        ) else {
            continue;
        };
        if out.get(&p1).is_none_or(|l| quality > l.quality) {
            out.insert(
                p1,
                Leg {
                    p1,
                    resource: input,
                    quality,
                    schematic_id: s.schematic_id,
                    factory_type,
                },
            );
        }
    }
    out
}

/// Every P2 colony a planet of `planet_type` at `qualities` can run: a
/// schematic of two or more inputs and one output that an advanced
/// factory the planet allows runs, each input made here ([`legs_of`]) or,
/// for at most [`MAX_IMPORTS`] of them, in `made` (the P1 the bot's other
/// colonies make) through a launchpad the planet allows. At least one leg
/// is local. Sorted by schematic id.
pub fn templates(
    statics: &Statics,
    planet_type: i64,
    qualities: &BTreeMap<i64, f64>,
    made: &BTreeSet<i64>,
) -> Vec<P2Template> {
    let Some(cc_type) = cheapest(statics, PinKind::Command, planet_type, None) else {
        return Vec::new();
    };
    let Some(ecu_type) = cheapest(statics, PinKind::Ecu, planet_type, None) else {
        return Vec::new();
    };
    let local = legs_of(statics, planet_type, qualities);
    let launchpad = cheapest(statics, PinKind::Spaceport, planet_type, None);
    let mut out = Vec::new();
    for s in statics.schematics() {
        let &[(product, _)] = s.outputs.as_slice() else {
            continue;
        };
        if s.inputs.len() < 2 {
            continue;
        }
        let Some(factory_type) = cheapest(
            statics,
            PinKind::Process,
            planet_type,
            Some(&s.pin_type_ids),
        ) else {
            continue;
        };
        let mut legs = Vec::new();
        let mut imports = Vec::new();
        for &(input, _) in &s.inputs {
            match local.get(&input) {
                Some(l) => legs.push(*l),
                None if made.contains(&input) => imports.push(input),
                None => {
                    imports.clear();
                    legs.clear();
                    break;
                }
            }
        }
        if legs.is_empty() || imports.len() > MAX_IMPORTS {
            continue;
        }
        let import = imports.first().copied();
        if import.is_some() && launchpad.is_none() {
            continue;
        }
        let quality = legs.iter().map(|l| l.quality).fold(f64::INFINITY, f64::min);
        out.push(P2Template {
            planet_type,
            cc_type,
            ecu_type,
            factory_type,
            schematic_id: s.schematic_id,
            product,
            legs,
            import,
            launchpad_type: import.and(launchpad),
            quality,
        });
    }
    out.sort_by_key(|t| t.schematic_id);
    out
}

/// **The market's reading of a recipe**: ISK an hour the P2 adds over its
/// inputs, at `prices` (per type, `P_mkt` else `P_ref`). `None` when a
/// price is missing.
pub fn margin_per_hour(
    statics: &Statics,
    schematic_id: i64,
    prices: &BTreeMap<i64, f64>,
) -> Option<f64> {
    let s = statics.schematic(schematic_id)?;
    let value = |v: &[(i64, i64)]| -> Option<f64> {
        v.iter()
            .map(|(t, q)| prices.get(t).map(|p| p * *q as f64))
            .sum()
    };
    let cycle = s.cycle_time.max(1) as f64;
    Some((value(&s.outputs)? - value(&s.inputs)?) * 3600.0 / cycle)
}

/// **Choose the colony**: among `(planet, template)` candidates, drop those
/// the market prices under their inputs; the best priced margin wins (every
/// farmer chases the best margin and the price it moves sends the next one
/// elsewhere); without a priced one, the [`pi::SPREAD`] best by quality,
/// the one at `seed % n`.
pub fn choose(
    statics: &Statics,
    candidates: Vec<(i64, P2Template)>,
    prices: &BTreeMap<i64, f64>,
    seed: u64,
) -> Option<(i64, P2Template)> {
    let mut scored: Vec<(Option<f64>, i64, P2Template)> = candidates
        .into_iter()
        .map(|(p, t)| (margin_per_hour(statics, t.schematic_id, prices), p, t))
        .filter(|(m, _, _)| m.is_none_or(|m| m > 0.0))
        .collect();
    scored.sort_by(|a, b| {
        let key = |m: Option<f64>| m.unwrap_or(f64::NEG_INFINITY);
        key(b.0)
            .total_cmp(&key(a.0))
            .then(b.2.quality.total_cmp(&a.2.quality))
            .then(a.1.cmp(&b.1))
            .then(a.2.schematic_id.cmp(&b.2.schematic_id))
    });
    let first = scored.first()?;
    let i = if first.0.is_some() {
        0
    } else {
        (seed % scored.len().min(pi::SPREAD) as u64) as usize
    };
    let (_, p, t) = scored.swap_remove(i);
    Some((p, t))
}

/// Every P2 candidate on `planets` (`(planet, type)`) that are not `ours`
/// and whose type is not in `skip`, at the table's qualities.
pub fn candidates(
    statics: &Statics,
    planets: &[(i64, i64)],
    ours: &BTreeSet<i64>,
    skip: &BTreeSet<i64>,
    made: &BTreeSet<i64>,
) -> Vec<(i64, P2Template)> {
    planets
        .iter()
        .filter(|(p, t)| !ours.contains(p) && !skip.contains(t))
        .filter_map(|&(p, t)| Some((p, statics.resources(t)?, t)))
        .flat_map(|(p, q, t)| {
            templates(statics, t, q, made)
                .into_iter()
                .map(move |x| (p, x))
        })
        .collect()
}

/// Every type a candidate's market reading needs.
pub fn priced_types(statics: &Statics, candidates: &[(i64, P2Template)]) -> Vec<i64> {
    let mut v: BTreeSet<i64> = BTreeSet::new();
    for (_, t) in candidates {
        if let Some(s) = statics.schematic(t.schematic_id) {
            v.extend(s.inputs.iter().chain(&s.outputs).map(|(t, _)| *t));
        }
    }
    v.into_iter().collect()
}

/// The P1 a set of colonies makes: the products of their single-input
/// schematics (the imports a new P2 colony may count on).
pub fn made_by(views: &[pi::ColonyView], statics: &Statics) -> BTreeSet<i64> {
    views
        .iter()
        .flat_map(|v| &v.pins)
        .filter_map(|p| p.schematic_id)
        .filter_map(|s| statics.schematic(s))
        .filter(|s| s.inputs.len() == 1)
        .flat_map(|s| s.outputs.iter().map(|(t, _)| *t))
        .collect()
}

// ---- the stream, pure -------------------------------------------------------

/// Leg `i` stands north (even) or south (odd) of the advanced factory,
/// further out every second leg: its basic factory one spacing out, its
/// extraction unit two. Every link is one spacing long.
fn leg_points(site: &Site, i: usize) -> (SurfacePoint, SurfacePoint) {
    let s = pi::PIN_SPACING;
    let dir = if i.is_multiple_of(2) { -1.0 } else { 1.0 };
    let k = (i / 2) as f64;
    let lat = |n: f64| std::f64::consts::FRAC_PI_2 + dir * s * n;
    let lon = site.theta - s - 2.0 * k * s;
    (
        SurfacePoint::new(lat(1.0), lon),
        SurfacePoint::new(lat(2.0), lon),
    )
}

/// The heads of leg `i`'s extraction unit, `n` on a ring.
fn heads_of(
    statics: &Statics,
    t: &P2Template,
    site: &Site,
    i: usize,
    n: usize,
    r: f64,
) -> Vec<Head> {
    let influence = statics
        .pin_type(t.ecu_type)
        .map_or(0.0, |p| p.attr(sim::attr::ECU_AREA_OF_INFLUENCE));
    ring(leg_points(site, i).1, n, r, influence)
}

/// The stream without routes: the command centre (the hold's item) and its
/// upgrade, the advanced factory west of it, every leg's basic factory and
/// extraction unit, the launchpad further west when there is an import,
/// the links, the heads (`layout.heads.len()` per unit), the schematics and
/// the programs.
pub fn base_stream(
    statics: &Statics,
    t: &P2Template,
    cc_item: i64,
    site: &Site,
    layout: &Layout,
) -> Vec<Command> {
    let cc = Id::Real(cc_item);
    let mut v = vec![Command::CreatePin {
        pin_id: cc.clone(),
        type_id: t.cc_type,
        point: at(site.theta),
    }];
    if layout.level > 0 {
        v.push(Command::UpgradeCommandCenter {
            pin_id: cc.clone(),
            level: layout.level,
        });
    }
    v.push(Command::CreatePin {
        pin_id: temp(ADVANCED),
        type_id: t.factory_type,
        point: at(site.theta - pi::PIN_SPACING),
    });
    for (i, leg) in t.legs.iter().enumerate() {
        let (b, e) = leg_points(site, i);
        v.push(Command::CreatePin {
            pin_id: temp(basic(i)),
            type_id: leg.factory_type,
            point: b,
        });
        v.push(Command::CreatePin {
            pin_id: temp(ecu(i)),
            type_id: t.ecu_type,
            point: e,
        });
    }
    if let Some(lp) = t.launchpad_type {
        v.push(Command::CreatePin {
            pin_id: temp(LAUNCHPAD),
            type_id: lp,
            point: at(site.theta - 2.0 * pi::PIN_SPACING),
        });
    }
    let link = |a: Id, b: Id| Command::CreateLink {
        endpoint1: a,
        endpoint2: b,
        level: 0,
    };
    v.push(link(temp(ADVANCED), cc));
    for i in 0..t.legs.len() {
        v.push(link(temp(ecu(i)), temp(basic(i))));
        v.push(link(temp(basic(i)), temp(ADVANCED)));
    }
    if t.launchpad_type.is_some() {
        v.push(link(temp(LAUNCHPAD), temp(ADVANCED)));
    }
    for i in 0..t.legs.len() {
        for h in heads_of(statics, t, site, i, layout.heads.len(), layout.head_radius) {
            v.push(Command::AddExtractorHead {
                pin_id: temp(ecu(i)),
                head_id: h.head_id,
                point: h.point(),
            });
        }
    }
    for (i, leg) in t.legs.iter().enumerate() {
        v.push(Command::SetSchematic {
            pin_id: temp(basic(i)),
            schematic_id: leg.schematic_id,
        });
    }
    v.push(Command::SetSchematic {
        pin_id: temp(ADVANCED),
        schematic_id: t.schematic_id,
    });
    for (i, leg) in t.legs.iter().enumerate() {
        v.push(Command::InstallProgram {
            pin_id: temp(ecu(i)),
            program_type: Some(leg.resource),
            head_radius: layout.head_radius,
        });
    }
    v
}

/// The server's survey quote per resource, for the dry run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quotes(pub BTreeMap<i64, i64>);

impl ProgramQuote for Quotes {
    fn quote(&self, _: &Colony, _: i64, program_type: i64, _: f64) -> i64 {
        self.0.get(&program_type).copied().unwrap_or(1)
    }
}

/// [`pi::dry_run`] with a quote per resource.
pub fn dry_run(
    statics: &Statics,
    site: &Site,
    commands: &[Command],
    quotes: &Quotes,
) -> std::result::Result<(Colony, sim::ApplyReport), sim::PiError> {
    let mut colonies = Colonies::new();
    let report = colonies.apply(
        statics,
        &ColonyEdit {
            planet_id: site.planet_id,
            owner_id: site.owner_id,
            planet_type_id: site.planet_type,
            planet_radius: site.radius,
            now: eve_server::pyutil::filetime_now(),
            link_type_id: pi::LINK_TYPE,
            quote: quotes,
        },
        commands,
    )?;
    let colony = colonies
        .get(site.planet_id, site.owner_id)
        .cloned()
        .ok_or(sim::PiError::CannotManagePlanetWithoutCommandCenter)?;
    Ok((colony, report))
}

/// **Fit the P2 template**: the highest level up to `max_level` and the
/// most heads per extraction unit (ten down to one, the same on every
/// unit) whose stream the colony code accepts and whose bill fits `budget`.
/// `layout.heads` is the first unit's ring; every unit gets as many.
pub fn fit(
    statics: &Statics,
    t: &P2Template,
    cc_item: i64,
    site: &Site,
    max_level: i64,
    budget: f64,
) -> Option<Layout> {
    let head_radius = pi::radius_for_hours(pi::PROGRAM_HOURS);
    let pins: f64 = statics.base_price(t.factory_type)
        + t.launchpad_type.map_or(0.0, |l| statics.base_price(l))
        + t.legs
            .iter()
            .map(|l| statics.base_price(l.factory_type) + statics.base_price(t.ecu_type))
            .sum::<f64>();
    for level in (0..=max_level.clamp(0, sim::MAX_COMMAND_CENTRE_LEVEL)).rev() {
        let cost = pins + sim::command_centre_upgrade_cost(0, level).unwrap_or(0) as f64;
        if cost > budget {
            continue;
        }
        for n in (1..=sim::ECU_MAX_HEADS).rev() {
            let layout = Layout {
                level,
                heads: heads_of(statics, t, site, 0, n, head_radius),
                head_radius,
                cost,
            };
            let stream = base_stream(statics, t, cc_item, site, &layout);
            if dry_run(statics, site, &stream, &Quotes::default()).is_ok() {
                return Some(layout);
            }
        }
    }
    None
}

/// **The whole stream**: [`base_stream`] and the routes -- each unit to its
/// basic factory at the unit's peak, each basic factory's cycle to the
/// advanced factory, the launchpad's import at one advanced cycle's need,
/// and the advanced factory's cycle to the command centre -- checked by a
/// last dry run at the server's `quotes`.
pub fn with_routes(
    statics: &Statics,
    t: &P2Template,
    cc_item: i64,
    site: &Site,
    layout: &Layout,
    quotes: &Quotes,
) -> std::result::Result<Vec<Command>, sim::PiError> {
    let mut v = base_stream(statics, t, cc_item, site, layout);
    let (colony, report) = dry_run(statics, site, &v, quotes)?;
    let real = |id: Id| {
        report
            .id_map
            .iter()
            .find(|(t, _)| *t == id)
            .map(|(_, r)| *r)
    };
    let out_of = |pin: (i64, i64), ty: i64| {
        real(temp(pin))
            .and_then(|p| colony.pin(p))
            .and_then(|p| p.product_max_output(statics).get(&ty).copied())
            .unwrap_or(0)
    };
    let mut k = 0;
    let mut route = |path: Vec<Id>, type_id: i64, quantity: i64| {
        k += 1;
        Command::CreateRoute {
            route_id: temp((2, k)),
            path,
            type_id,
            quantity,
        }
    };
    let mut routes = Vec::new();
    for (i, leg) in t.legs.iter().enumerate() {
        routes.push(route(
            vec![temp(ecu(i)), temp(basic(i))],
            leg.resource,
            out_of(ecu(i), leg.resource),
        ));
        routes.push(route(
            vec![temp(basic(i)), temp(ADVANCED)],
            leg.p1,
            out_of(basic(i), leg.p1),
        ));
    }
    if let Some(p1) = t.import {
        let need = statics
            .schematic(t.schematic_id)
            .and_then(|s| s.inputs.iter().find(|(x, _)| *x == p1).map(|(_, q)| *q))
            .unwrap_or(0);
        routes.push(route(vec![temp(LAUNCHPAD), temp(ADVANCED)], p1, need));
    }
    routes.push(route(
        vec![temp(ADVANCED), Id::Real(cc_item)],
        t.product,
        out_of(ADVANCED, t.product),
    ));
    v.extend(routes);
    dry_run(statics, site, &v, quotes)?;
    Ok(v)
}

// ---- feeding an import, pure ----------------------------------------------------

/// One import a colony wants: P1 `type_id` onto `launchpad`, at most
/// `room` units (what the launchpad has space for).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feed {
    pub planet: i64,
    pub launchpad: i64,
    pub type_id: i64,
    pub room: i64,
}

/// **What a colony imports**: the inputs of its factories that no pin of
/// its own makes (a program's resource, a schematic's product), onto its
/// first launchpad, sized to the launchpad's free volume.
pub fn feeds(view: &pi::ColonyView, statics: &Statics) -> Vec<Feed> {
    let Some(lp) = view
        .pins
        .iter()
        .find(|p| statics.pin_kind(p.type_id) == Some(PinKind::Spaceport))
    else {
        return Vec::new();
    };
    let schematics: Vec<&sim::Schematic> = view
        .pins
        .iter()
        .filter_map(|p| statics.schematic(p.schematic_id?))
        .collect();
    let made: BTreeSet<i64> = schematics
        .iter()
        .flat_map(|s| s.outputs.iter().map(|(t, _)| *t))
        .chain(view.pins.iter().filter_map(|p| p.program_type))
        .collect();
    let wanted: BTreeSet<i64> = schematics
        .iter()
        .flat_map(|s| s.inputs.iter().map(|(t, _)| *t))
        .filter(|t| !made.contains(t))
        .collect();
    let capacity = statics.pin_type(lp.type_id).map_or(0.0, |p| p.capacity);
    let used: f64 = lp
        .contents
        .iter()
        .map(|(t, q)| statics.volume(*t) * *q as f64)
        .sum();
    wanted
        .into_iter()
        .filter_map(|t| {
            let vol = statics.volume(t);
            let room = if vol > 0.0 {
                ((capacity - used) / vol).floor().max(0.0) as i64
            } else {
                0
            };
            (room > 0).then_some(Feed {
                planet: view.planet_id,
                launchpad: lp.id,
                type_id: t,
                room,
            })
        })
        .collect()
}

// ---- over the wire ------------------------------------------------------------------

/// `GetProgramResultInfo(pinID, typeID, heads, headRadius)` [V] for one
/// unit: the survey window's `qtyPerCycle`.
fn program_quote(
    c: &mut Conn,
    planet: i64,
    pin: (i64, i64),
    resource: i64,
    heads: &[Head],
    radius: f64,
) -> Result<i64> {
    let heads = PyValue::List(
        heads
            .iter()
            .map(|h| {
                PyValue::Tuple(vec![
                    PyValue::Int(h.head_id),
                    PyValue::Real(h.latitude),
                    PyValue::Real(h.longitude),
                ])
            })
            .collect(),
    );
    let a = on_planet(
        c,
        planet,
        "GetProgramResultInfo",
        vec![
            id_value(&temp(pin)),
            PyValue::Int(resource),
            heads,
            PyValue::Real(radius),
        ],
    )?;
    Ok(int(seq(a.value.as_ref()).first()).unwrap_or(0))
}

/// **Found the P2 colony** on `planet`, the bot in space with the command
/// centre `cc_item` in its hold: re-read the planet (the survey window's
/// qualities pick the legs), fit, quote every unit, route, submit.
/// `Ok(Ok((level, heads per unit)))`, or the reason it was not built.
#[allow(clippy::too_many_arguments)]
pub fn found(
    c: &mut Conn,
    statics: &Statics,
    planet: i64,
    t: &P2Template,
    made: &BTreeSet<i64>,
    cc_item: i64,
    owner: i64,
    budget: f64,
    max_level: i64,
    seed: u64,
) -> Result<std::result::Result<(i64, usize), String>> {
    let info = pi::planet_info(c, planet)?.unwrap_or(PyValue::None);
    let planet_type = int(kv(&info, "planetTypeID")).unwrap_or(t.planet_type);
    let radius = real(kv(&info, "radius")).unwrap_or(0.0);
    let read = pi::resource_info(c, planet)?;
    let t = templates(statics, planet_type, &read, made)
        .into_iter()
        .find(|x| x.schematic_id == t.schematic_id && x.cc_type == t.cc_type)
        .unwrap_or_else(|| t.clone());
    let site = Site {
        planet_id: planet,
        owner_id: owner,
        planet_type,
        radius,
        theta: (seed % 628) as f64 / 100.0,
    };
    let Some(layout) = fit(statics, &t, cc_item, &site, max_level, budget) else {
        return Ok(Err(
            "no P2 layout fits the power, the CPU and the wallet".to_owned()
        ));
    };
    let mut quotes = Quotes::default();
    for (i, leg) in t.legs.iter().enumerate() {
        let heads = heads_of(
            statics,
            &t,
            &site,
            i,
            layout.heads.len(),
            layout.head_radius,
        );
        let q = program_quote(c, planet, ecu(i), leg.resource, &heads, layout.head_radius)?;
        if q <= 0 {
            return Ok(Err(format!(
                "the survey quotes nothing for {}",
                leg.resource
            )));
        }
        quotes.0.insert(leg.resource, q);
    }
    let stream = match with_routes(statics, &t, cc_item, &site, &layout, &quotes) {
        Ok(s) => s,
        Err(e) => return Ok(Err(format!("the P2 routes: {}", e.message_id()))),
    };
    Ok(pi::submit(c, planet, &stream)?.map(|_| (layout.level, layout.heads.len())))
}

/// What one [`feed`] carried.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fed {
    pub units: i64,
    pub refusals: Vec<String>,
}

fn soft<T>(r: Result<T>) -> Result<std::result::Result<T, String>> {
    match r {
        Ok(v) => Ok(Ok(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(Err(e.to_string())),
    }
}

/// **Feed the imports**, docked at `home` to start and to end: load each
/// wanted P1 the hangar holds (at most the launchpad's room), undock, and
/// per colony put it into the planet's customs office from the hold
/// (`Add(itemID, shipID, qty=n)` on the office's inventory [V]), read the
/// rate (`planetOrbitalRegistryBroker.GetTaxRate(officeID)` [V]) and
/// `ImportExportWithPlanet(launchpadID, {itemID: qty}, {}, rate)` [V]; dock.
/// The office is the customs-office ball nearest the planet on the
/// overview.
pub fn feed(c: &mut Conn, statics: &Statics, home: i64, wants: &[Feed]) -> Result<Fed> {
    let mut fed = Fed::default();
    let hangar = c.hangar_items()?;
    let mut plan: Vec<(Feed, i64)> = Vec::new();
    for f in wants {
        let have: i64 = hangar
            .iter()
            .filter(|r| r.type_id == f.type_id && !r.singleton)
            .map(|r| r.quantity)
            .sum();
        let already: i64 = plan
            .iter()
            .filter(|(g, _)| g.type_id == f.type_id)
            .map(|(_, q)| *q)
            .sum();
        let q = f.room.min(have - already);
        if q > 0 {
            plan.push((*f, q));
        }
    }
    if plan.is_empty() {
        return Ok(fed);
    }
    let Some(office_type) = statics.customs_type().map(|t| t.type_id) else {
        fed.refusals
            .push("no customs office type in the PI table".to_owned());
        return Ok(fed);
    };
    // Into the hold, by type: the stacks the plan names.
    let mut to_load: BTreeMap<i64, i64> = BTreeMap::new();
    for (f, q) in &plan {
        *to_load.entry(f.type_id).or_default() += q;
    }
    for (&type_id, &want) in &to_load {
        let mut left = want;
        for r in hangar
            .iter()
            .filter(|r| r.type_id == type_id && !r.singleton)
        {
            if left <= 0 {
                break;
            }
            let q = left.min(r.quantity);
            if let Err(why) = soft(c.load_cargo(r.item_id, Some(q)))? {
                fed.refusals.push(format!("load {type_id}: {why}"));
                break;
            }
            left -= q;
        }
    }
    pi::leave(c)?;
    let done = (|| -> Result<()> {
        let ship = c
            .session()
            .ship_id()
            .ok_or_else(|| Error::NotSent("pi feed: no ship".to_owned()))?;
        let system = c.session().solar_system_id().unwrap_or(0);
        for (f, q) in &plan {
            let office = {
                let world = c.world_now();
                world
                    .balls()
                    .filter(|b| b.type_id() == Some(office_type))
                    .filter_map(|b| Some((world.surface_distance(f.planet, b.ball_id)?, b.ball_id)))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, id)| id)
            };
            let Some(office) = office else {
                fed.refusals
                    .push(format!("no customs office over {}", f.planet));
                continue;
            };
            match import_one(c, office, ship, system, f, *q)? {
                Ok(()) => fed.units += q,
                Err(why) => fed.refusals.push(why),
            }
        }
        Ok(())
    })();
    pi::fly_home(c, home)?;
    done?;
    Ok(fed)
}

/// One import: hold -> office -> launchpad.
fn import_one(
    c: &mut Conn,
    office: i64,
    ship: i64,
    system: i64,
    f: &Feed,
    q: i64,
) -> Result<std::result::Result<(), String>> {
    let Some(stack) = c.cargo()?.into_iter().find(|r| r.type_id == f.type_id) else {
        return Ok(Err(format!("{} is not in the hold", f.type_id)));
    };
    let q = q.min(stack.quantity);
    let inv = match soft(c.open_container(office))? {
        Ok(i) => i,
        Err(why) => return Ok(Err(format!("open office {office}: {why}"))),
    };
    let added = c.call_on(
        &inv,
        "Add",
        vec![PyValue::Int(stack.item_id), PyValue::Int(ship)],
        vec![(PyValue::Int(q), PyValue::Bytes(b"qty".to_vec()))],
    );
    match added {
        Ok(a) => {
            if let Some(key) = a.refusal() {
                return Ok(Err(format!("into the office: {key}")));
            }
        }
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(e) => return Ok(Err(format!("into the office: {e}"))),
    }
    let in_office: Vec<(i64, i64)> = match soft(c.list(&inv, Some(sim::CUSTOMS_OFFICE_FLAG)))? {
        Ok(rows) => rows
            .into_iter()
            .filter(|r| r.type_id == f.type_id)
            .map(|r| (r.item_id, r.quantity))
            .collect(),
        Err(why) => return Ok(Err(format!("list the office: {why}"))),
    };
    let mut items = Vec::new();
    let mut left = q;
    for (id, n) in in_office {
        if left <= 0 {
            break;
        }
        let take = left.min(n);
        items.push((PyValue::Int(take), PyValue::Int(id)));
        left -= take;
    }
    if items.is_empty() {
        return Ok(Err(format!("{} did not reach the office", f.type_id)));
    }
    let rate = c
        .moniker_call(
            "planetOrbitalRegistryBroker",
            PyValue::Int(system),
            "GetTaxRate",
            vec![PyValue::Int(office)],
            vec![],
        )
        .map(|a| real(a.value.as_ref()));
    let rate = match rate {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(Err(format!("office {office}: access denied"))),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(e) => return Ok(Err(format!("GetTaxRate: {e}"))),
    };
    let moved = c.call_on(
        &inv,
        "ImportExportWithPlanet",
        vec![
            PyValue::Int(f.launchpad),
            PyValue::Dict(items),
            PyValue::Dict(Vec::new()),
            PyValue::Real(rate),
        ],
        vec![],
    );
    match moved {
        Ok(a) => Ok(a.refusal().map_or(Ok(()), |k| Err(format!("import: {k}")))),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(Err(format!("import: {e}"))),
    }
}

/// `P_mkt` (else `P_ref`) of each of `types`: the market signal's prices.
pub fn prices_of(c: &mut Conn, types: &[i64]) -> Result<BTreeMap<i64, f64>> {
    let mut prices = crate::pricing::Prices::read(c)?;
    let mut out = BTreeMap::new();
    for &t in types {
        if let Some(p) = prices.p_mkt(c, t)?.or_else(|| prices.p_ref(t)) {
            out.insert(t, p);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids and rows only.
    const PT: i64 = 990_670_001;
    const PT_B: i64 = 990_670_002;
    const CC: i64 = 990_670_011;
    const ECU_T: i64 = 990_670_012;
    const BASIC_T: i64 = 990_670_015;
    const ADV_T: i64 = 990_670_016;
    const PAD_T: i64 = 990_670_017;
    const RES_A: i64 = 990_670_021;
    const RES_B: i64 = 990_670_022;
    const P1_A: i64 = 990_670_031;
    const P1_B: i64 = 990_670_032;
    const P1_C: i64 = 990_670_033;
    const P2_AB: i64 = 990_670_041;
    const P2_AC: i64 = 990_670_042;
    const S_A: i64 = 990_671;
    const S_B: i64 = 990_672;
    const S_AB: i64 = 990_675;
    const S_AC: i64 = 990_676;
    const PLANET: i64 = 40_967_001;
    const PLANET_B: i64 = 40_967_002;
    const OWNER: i64 = 90_000_671;
    const CC_ITEM: i64 = 100_000_671;

    /// Two planet types: `PT` has both resources, `PT_B` only the second.
    /// P2_AB needs P1_A and P1_B; P2_AC needs P1_A and P1_C, which no
    /// planet makes.
    fn statics() -> Statics {
        let link = pi::LINK_TYPE;
        Statics::parse(&format!(
            "pintype\t{CC}\t1027\t500.0\t0.0\t0\t48=1675.0;11=6000.0\n\
             pintype\t{ECU_T}\t1063\t0.0\t1000.0\t0\t49=100.0;15=500.0;1690=10.0;1691=50.0;1689=0.3\n\
             pintype\t{BASIC_T}\t1028\t0.0\t2000.0\t0\t49=100.0;15=100.0\n\
             pintype\t{ADV_T}\t1028\t0.0\t5000.0\t0\t49=150.0;15=150.0\n\
             pintype\t{PAD_T}\t1030\t10000.0\t9000.0\t0\t49=300.0;15=200.0;1638=0.5\n\
             pintype\t{link}\t1036\t0.0\t0.0\t0\t49=15.0;15=10.0;1631=1250.0;1633=0.15;1634=0.2;1635=1.4;1636=1.2\n\
             schematic\t{S_A}\t60\t{BASIC_T}\t{RES_A}=10\t{P1_A}=5\n\
             schematic\t{S_B}\t60\t{BASIC_T}\t{RES_B}=10\t{P1_B}=5\n\
             schematic\t{S_AB}\t60\t{ADV_T}\t{P1_A}=5;{P1_B}=5\t{P2_AB}=1\n\
             schematic\t{S_AC}\t60\t{ADV_T}\t{P1_A}=5;{P1_C}=5\t{P2_AC}=1\n\
             resource\t{PT}\t{RES_A}=0.6;{RES_B}=0.8\n\
             resource\t{PT_B}\t{RES_B}=0.9\n\
             itemtype\t{CC}\t1027\t41\t10.0\t500.0\tFixture Centre\n\
             itemtype\t{P1_A}\t1042\t43\t0.5\t0.0\tFixture P1 A\n\
             itemtype\t{P1_B}\t1042\t43\t0.5\t0.0\tFixture P1 B\n\
             itemtype\t{P2_AB}\t1034\t43\t1.0\t0.0\tFixture P2\n"
        ))
        .expect("the invented rows parse")
    }

    fn site(planet_type: i64) -> Site {
        Site {
            planet_id: PLANET,
            owner_id: OWNER,
            planet_type,
            radius: 3_000_000.0,
            theta: 1.0,
        }
    }

    fn quotes() -> Quotes {
        Quotes(BTreeMap::from([(RES_A, 400), (RES_B, 400)]))
    }

    /// A planet with both resources makes P2_AB on its own; P2_AC needs a
    /// P1 nobody makes, so it is not offered until another colony makes
    /// it -- and then as an import.
    #[test]
    fn a_p2_is_reachable_from_the_planets_legs_or_one_import() {
        let s = statics();
        let t = templates(&s, PT, s.resources(PT).unwrap(), &BTreeSet::new());
        assert_eq!(t.len(), 1, "{t:?}");
        let t = &t[0];
        assert_eq!(
            (t.schematic_id, t.product, t.factory_type),
            (S_AB, P2_AB, ADV_T)
        );
        let legs: Vec<(i64, i64)> = t.legs.iter().map(|l| (l.resource, l.p1)).collect();
        assert_eq!(legs, vec![(RES_A, P1_A), (RES_B, P1_B)]);
        assert_eq!((t.import, t.launchpad_type), (None, None));

        // The B planet makes only P1_B: nothing, until a colony makes P1_A.
        let only_b = s.resources(PT_B).unwrap();
        assert!(templates(&s, PT_B, only_b, &BTreeSet::new()).is_empty());
        let fed = templates(&s, PT_B, only_b, &BTreeSet::from([P1_A]));
        assert_eq!(fed.len(), 1, "{fed:?}");
        assert_eq!(fed[0].schematic_id, S_AB);
        assert_eq!(
            (fed[0].import, fed[0].launchpad_type),
            (Some(P1_A), Some(PAD_T))
        );
        assert_eq!(fed[0].legs.len(), 1);

        // A colony that would only import is not offered.
        let none = BTreeMap::from([(RES_B, 0.0)]);
        assert!(templates(&s, PT_B, &none, &BTreeSet::from([P1_A, P1_B])).is_empty());
    }

    /// The market picks: the best priced margin wins; a P2 priced under its
    /// inputs is dropped; with no prices the seed spreads by quality.
    #[test]
    fn the_market_chooses_the_p2_and_refuses_one_worth_less_than_its_inputs() {
        let s = statics();
        let made = BTreeSet::from([P1_A]);
        let planets = [(PLANET, PT), (PLANET_B, PT_B)];
        let c = candidates(&s, &planets, &BTreeSet::new(), &BTreeSet::new(), &made);
        assert_eq!(c.len(), 2, "both planets make P2_AB");
        assert_eq!(priced_types(&s, &c), {
            let mut v = vec![P1_A, P1_B, P2_AB];
            v.sort();
            v
        });

        let rich = BTreeMap::from([(P1_A, 10.0), (P1_B, 10.0), (P2_AB, 500.0)]);
        let m = margin_per_hour(&s, S_AB, &rich).unwrap();
        assert!((m - (500.0 - 100.0) * 60.0).abs() < 1e-9, "{m}");
        let (p, t) = choose(&s, c.clone(), &rich, 7).expect("a choice");
        assert_eq!(t.schematic_id, S_AB);
        assert!([PLANET, PLANET_B].contains(&p));

        let poor = BTreeMap::from([(P1_A, 10.0), (P1_B, 10.0), (P2_AB, 50.0)]);
        assert!(
            choose(&s, c.clone(), &poor, 0).is_none(),
            "under its inputs"
        );

        // Unpriced: by quality (the B planet's worst leg is the better), and
        // the seed walks the spread.
        let a = choose(&s, c.clone(), &BTreeMap::new(), 0).unwrap().0;
        let b = choose(&s, c, &BTreeMap::new(), 1).unwrap().0;
        assert_eq!(a, PLANET_B);
        assert_ne!(a, b);
    }

    /// The fitted stream is what the colony code accepts, its routes carry
    /// each leg into the advanced factory and the P2 to the centre, and the
    /// server's simulation turns it into a P2 stack in the command centre.
    #[test]
    fn the_p2_colony_dry_runs_and_its_simulation_fills_the_centre_with_p2() {
        let s = statics();
        let t = templates(&s, PT, s.resources(PT).unwrap(), &BTreeSet::new()).remove(0);
        let layout = fit(&s, &t, CC_ITEM, &site(PT), 0, f64::INFINITY).expect("a layout");
        assert!(!layout.heads.is_empty());
        let stream = with_routes(&s, &t, CC_ITEM, &site(PT), &layout, &quotes()).expect("routes");
        let routes: Vec<(i64, i64)> = stream
            .iter()
            .filter_map(|c| match c {
                Command::CreateRoute {
                    type_id, quantity, ..
                } => Some((*type_id, *quantity)),
                _ => None,
            })
            .collect();
        assert_eq!(routes.len(), 5, "{routes:?}");
        assert!(routes.contains(&(P1_A, 5)) && routes.contains(&(P1_B, 5)));
        assert_eq!(routes.last(), Some(&(P2_AB, 1)));
        let (mut colony, _) = dry_run(&s, &site(PT), &stream, &quotes()).unwrap();
        assert_eq!(colony.pins.len(), 6);
        let adv = colony
            .pins
            .iter()
            .find(|p| p.type_id == ADV_T)
            .expect("the advanced factory");
        assert_eq!(adv.schematic_id, Some(S_AB));

        // Six hours of the server's own simulation.
        let end = colony.current_sim_time + 6 * sim::HOUR;
        colony.run_simulation(&s, end);
        let cc = colony.command_pin(&s).expect("the centre");
        assert!(cc.quantity(P2_AB) > 0, "{:?}", cc.contents);
        assert_eq!(
            pi::launch_of(&view_of(&colony), &s).map(|(_, g)| g.contains_key(&P2_AB)),
            Some(true)
        );

        // The encoded stream is one wire pair per command.
        let PyValue::List(wire) = pi::encode(&stream) else {
            panic!("a list")
        };
        assert_eq!(wire.len(), stream.len());
    }

    /// The colony as `read_colony` would give it back.
    fn view_of(colony: &Colony) -> pi::ColonyView {
        pi::ColonyView {
            planet_id: colony.planet_id,
            planet_type_id: colony.planet_type_id,
            radius: colony.planet_radius,
            sim_time: colony.current_sim_time,
            level: colony.level,
            pins: colony
                .pins
                .iter()
                .map(|p| pi::PinView {
                    id: p.pin_id,
                    type_id: p.type_id,
                    contents: p.contents.clone(),
                    program_type: p.program_type,
                    expiry_time: p.expiry_time,
                    install_time: p.install_time,
                    head_radius: p.head_radius,
                    heads: p.heads.len(),
                    schematic_id: p.schematic_id,
                    last_launch_time: (p.last_launch_time > 0).then_some(p.last_launch_time),
                })
                .collect(),
        }
    }

    /// The import-fed colony: a launchpad, a route from it into the
    /// advanced factory, a feed for the missing P1 sized to the pad's room,
    /// and -- once the P1 is imported -- P2 in the centre.
    #[test]
    fn an_import_fed_colony_wants_its_missing_p1_and_makes_p2_once_fed() {
        let s = statics();
        let made = BTreeSet::from([P1_A]);
        let t = templates(&s, PT_B, s.resources(PT_B).unwrap(), &made).remove(0);
        let site = Site {
            planet_type: PT_B,
            ..site(PT_B)
        };
        let layout = fit(&s, &t, CC_ITEM, &site, 0, f64::INFINITY).expect("a layout");
        let stream = with_routes(&s, &t, CC_ITEM, &site, &layout, &quotes()).expect("routes");
        let (mut colony, _) = dry_run(&s, &site, &stream, &quotes()).unwrap();
        assert_eq!(colony.pins.len(), 5, "centre, unit, basic, advanced, pad");
        assert!(colony.routes.iter().any(|r| r.commodity_type_id == P1_A));

        let wants = feeds(&view_of(&colony), &s);
        let pad = colony
            .pins
            .iter()
            .find(|p| p.type_id == PAD_T)
            .unwrap()
            .pin_id;
        assert_eq!(
            wants,
            vec![Feed {
                planet: PLANET,
                launchpad: pad,
                type_id: P1_A,
                room: 20_000,
            }]
        );
        // Unfed, no P2; fed, P2.
        let mut unfed = colony.clone();
        unfed.run_simulation(&s, unfed.current_sim_time + 6 * sim::HOUR);
        assert_eq!(unfed.command_pin(&s).unwrap().quantity(P2_AB), 0);
        colony
            .import_commodities(&s, OWNER, pad, &BTreeMap::from([(P1_A, 500)]))
            .expect("the pad takes it");
        colony.run_simulation(&s, colony.current_sim_time + 6 * sim::HOUR);
        assert!(colony.command_pin(&s).unwrap().quantity(P2_AB) > 0);
        // A B6 colony (no pad) wants nothing.
        let mut plain = view_of(&colony);
        plain.pins.retain(|p| p.type_id != PAD_T);
        assert!(feeds(&plain, &s).is_empty());
    }

    /// What the bot's colonies make: their single-input schematics' P1.
    #[test]
    fn the_p1_made_counts_basic_factories_only() {
        let s = statics();
        let pin = |schematic: i64| pi::PinView {
            schematic_id: Some(schematic),
            ..pi::PinView::default()
        };
        let view = pi::ColonyView {
            pins: vec![pin(S_A), pin(S_AB)],
            ..pi::ColonyView::default()
        };
        assert_eq!(made_by(&[view], &s), BTreeSet::from([P1_A]));
    }
}
