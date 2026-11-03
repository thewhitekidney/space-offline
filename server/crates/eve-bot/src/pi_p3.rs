//! **The PI farmer makes P3 and P4** (lane X-PI3, on [`crate::pi_p2`]).
//!
//! A P3 schematic takes two or three P2 (ten of each), a P4 one takes P3 and
//! often a P1 (`planetSchematicsTypeMap` [V-data]); every one runs in an
//! advanced (P3) or a high-tech (P4) industry facility the planet type
//! allows (the schematic's `pin_type_ids` and the pin's planet restriction
//! [V]). The high-tech facility stands only on some planet types, so a P4
//! colony is offered only there ([`templates`] asks [`pi::cheapest`]).
//!
//! **The tier is read off the recipes, no id in the source** ([`tiers`]): an
//! input no schematic makes is raw (tier 0); a schematic's product is one
//! tier above its highest input. P1 is 1, P2 is 2, P3 is 3, P4 is 4.
//!
//! **The colony** ([`P3Template`]) is a command centre, one factory on the
//! schematic and one launchpad: every input comes **in** through the
//! planet's customs office onto the launchpad (the carrier is
//! [`pi_p2::feed`], which pays the office's import tax) and the product goes
//! **out** the same way ([`export`]): factory -> launchpad, then
//! `ImportExportWithPlanet(pad, {}, {type: qty}, rate)` [V] and the office's
//! hold into the ship, docked and sold. The tax both ways is
//! `qty x pad rate x importTaxMultiplier x office rate`
//! (`SpaceportPin.GetImportTax`/`GetExportTax` [V]); it is part of the
//! margin a colony is chosen by ([`margin_per_hour`]). A command centre's
//! launch is free on this server, so the office is the route the packet
//! names and the player's own.
//!
//! **Where the inputs come from**: the bot's other colonies (their P2 is
//! launched to the hangar, and a type a P3 colony wants is not sold,
//! [`feeds`]) and, for what they do not make, the home station's market
//! ([`stock`]: asks only, at most [`BUY_CAP`] x `P_mkt`). A colony is
//! founded only when one of the bot's colonies makes at least one of its inputs
//! ([`templates`]: "once P2 runs") and the market pays the product over its
//! inputs and both taxes ([`choose`]).
//!
//! Every stream is dry-run through the colony code ([`pi_p2::dry_run`]), as
//! B6's and FU-PI2's are.
//!
//! **Server side** (verified, nothing added; see `docs/lanes/x-pi3.md`):
//! the factory runs any schematic whose `pin_type_ids` list its type, takes
//! one batch of each input from a launchpad route and puts the product on a
//! route into the same launchpad; the module tests run that simulation with
//! invented rows, and the lane note records the same run on the client's
//! table.

use crate::pi::{self, Layout, Site, at, cheapest, int, kv, real, temp};
use crate::pi_p2::{self, Feed, Quotes};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::planet::{self as sim, Command, Id, PinKind, Schematic, Statics};
use std::collections::{BTreeMap, BTreeSet};

/// A P3 colony is preferred to a P2 or P1 one whenever one is reachable and
/// pays. **[A]**: a player with the skill climbs the chain; falsified by P3
/// farmers earning less an hour than the P2 farmers beside them in a
/// `[soc]` session.
pub const P3_FIRST: bool = true;
/// Cycles of an input kept on the launchpad. **[A]**: a day of hourly
/// cycles, the farmer's visit rhythm; falsified by a factory idle between
/// visits (too few) or a pad full of one input (too many).
pub const STOCK_CYCLES: i64 = 24;
/// The most the bot pays an ask for an input, times `P_mkt` (never past the
/// consumer's reservation). **[A]**: ten percent is the margin a colony is
/// chosen with room for; falsified by colonies running at a loss in `[soc]`.
pub const BUY_CAP: f64 = 1.1;
/// The share of the wallet one stocking round may spend. **[A]**
pub const STOCK_SHARE: f64 = 0.25;
/// The lowest tier this module makes.
pub const FIRST_TIER: u8 = 3;

// Temporary ids of the template's pins.
const FACTORY: (i64, i64) = (1, 10);
const LAUNCHPAD: (i64, i64) = (1, 11);

// ---- the tiers, pure --------------------------------------------------------

/// The tier of every type a schematic makes or takes (see the module
/// header). A cycle in the table would leave its members out.
pub fn tiers(statics: &Statics) -> BTreeMap<i64, u8> {
    let made: BTreeSet<i64> = statics
        .schematics()
        .flat_map(|s| s.outputs.iter().map(|(t, _)| *t))
        .collect();
    let mut tier: BTreeMap<i64, u8> = statics
        .schematics()
        .flat_map(|s| s.inputs.iter().map(|(t, _)| *t))
        .filter(|t| !made.contains(t))
        .map(|t| (t, 0))
        .collect();
    loop {
        let mut grew = false;
        for s in statics.schematics() {
            let Some(top) = s
                .inputs
                .iter()
                .map(|(t, _)| tier.get(t).copied())
                .collect::<Option<Vec<u8>>>()
                .and_then(|v| v.into_iter().max())
            else {
                continue;
            };
            for (t, _) in &s.outputs {
                if !tier.contains_key(t) {
                    tier.insert(*t, top + 1);
                    grew = true;
                }
            }
        }
        if !grew {
            return tier;
        }
    }
}

/// The tier a schematic makes (its first output's), if known.
pub fn tier_of(tiers: &BTreeMap<i64, u8>, s: &Schematic) -> Option<u8> {
    tiers.get(&s.outputs.first()?.0).copied()
}

// ---- the template, pure -----------------------------------------------------

/// **A P3 or P4 colony** for one planet type.
#[derive(Debug, Clone, PartialEq)]
pub struct P3Template {
    pub planet_type: i64,
    pub cc_type: i64,
    /// The advanced or high-tech industry facility, and its schematic.
    pub factory_type: i64,
    pub schematic_id: i64,
    pub launchpad_type: i64,
    pub product: i64,
    pub tier: u8,
    /// `(type, per cycle)`: every one imported.
    pub inputs: Vec<(i64, i64)>,
}

/// Every P3 and P4 colony a planet of `planet_type` can run: a one-output
/// schematic of tier [`FIRST_TIER`] or more that a factory the planet
/// allows runs, with a launchpad, a command centre and the customs office
/// in the table, at least one input in `have` (what the bot's colonies
/// make), and a product not already in `made`. Sorted by schematic id.
pub fn templates(
    statics: &Statics,
    planet_type: i64,
    have: &BTreeSet<i64>,
    made: &BTreeSet<i64>,
) -> Vec<P3Template> {
    let Some(cc_type) = cheapest(statics, PinKind::Command, planet_type, None) else {
        return Vec::new();
    };
    let Some(launchpad_type) = cheapest(statics, PinKind::Spaceport, planet_type, None) else {
        return Vec::new();
    };
    if statics.customs_type().is_none() {
        return Vec::new();
    }
    let tier = tiers(statics);
    let mut out = Vec::new();
    for s in statics.schematics() {
        let &[(product, _)] = s.outputs.as_slice() else {
            continue;
        };
        let Some(level) = tier_of(&tier, s).filter(|t| *t >= FIRST_TIER) else {
            continue;
        };
        if made.contains(&product) || !s.inputs.iter().any(|(t, _)| have.contains(t)) {
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
        out.push(P3Template {
            planet_type,
            cc_type,
            factory_type,
            schematic_id: s.schematic_id,
            launchpad_type,
            product,
            tier: level,
            inputs: s.inputs.clone(),
        });
    }
    out.sort_by_key(|t| t.schematic_id);
    out
}

/// What the customs office charges for one cycle: the import tax on the
/// inputs and the export tax on the product, on the pad type.
pub fn tax_per_cycle(statics: &Statics, pad_type: i64, s: &Schematic) -> f64 {
    let rate = statics.customs_type().map_or(0.0, |t| t.tax_rate);
    let goods = |v: &[(i64, i64)]| -> BTreeMap<i64, i64> { v.iter().copied().collect() };
    statics.import_tax(pad_type, &goods(&s.inputs), rate)
        + statics.export_tax(pad_type, &goods(&s.outputs), rate)
}

/// **The market's reading of a colony**: ISK an hour the product adds over
/// its inputs and both customs taxes, at `prices` (`P_mkt` else `P_ref`).
/// `None` when a price is missing: a P3 is bought into, so it is not
/// gambled on.
pub fn margin_per_hour(
    statics: &Statics,
    t: &P3Template,
    prices: &BTreeMap<i64, f64>,
) -> Option<f64> {
    let s = statics.schematic(t.schematic_id)?;
    let value = |v: &[(i64, i64)]| -> Option<f64> {
        v.iter()
            .map(|(ty, q)| prices.get(ty).map(|p| p * *q as f64))
            .sum()
    };
    let cycle = s.cycle_time.max(1) as f64;
    let net = value(&s.outputs)? - value(&s.inputs)? - tax_per_cycle(statics, t.launchpad_type, s);
    Some(net * 3600.0 / cycle)
}

/// **Choose the colony**: the best priced margin over zero; of the planets
/// that allow that recipe, the one at `seed % n`.
pub fn choose(
    statics: &Statics,
    candidates: Vec<(i64, P3Template)>,
    prices: &BTreeMap<i64, f64>,
    seed: u64,
) -> Option<(i64, P3Template)> {
    let mut scored: Vec<(f64, i64, P3Template)> = candidates
        .into_iter()
        .filter_map(|(p, t)| Some((margin_per_hour(statics, &t, prices)?, p, t)))
        .filter(|(m, _, _)| *m > 0.0)
        .collect();
    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(a.2.schematic_id.cmp(&b.2.schematic_id))
            .then(a.1.cmp(&b.1))
    });
    let best = scored.first()?.2.schematic_id;
    scored.retain(|(_, _, t)| t.schematic_id == best);
    let i = (seed % scored.len() as u64) as usize;
    let (_, p, t) = scored.swap_remove(i);
    Some((p, t))
}

/// Every candidate on `planets` (`(planet, type)`) that is not `ours` and
/// whose type is not in `skip`.
pub fn candidates(
    statics: &Statics,
    planets: &[(i64, i64)],
    ours: &BTreeSet<i64>,
    skip: &BTreeSet<i64>,
    have: &BTreeSet<i64>,
    made: &BTreeSet<i64>,
) -> Vec<(i64, P3Template)> {
    planets
        .iter()
        .filter(|(p, t)| !ours.contains(p) && !skip.contains(t))
        .flat_map(|&(p, t)| {
            templates(statics, t, have, made)
                .into_iter()
                .map(move |x| (p, x))
        })
        .collect()
}

/// Every type a candidate's market reading needs.
pub fn priced_types(statics: &Statics, candidates: &[(i64, P3Template)]) -> Vec<i64> {
    let mut v: BTreeSet<i64> = BTreeSet::new();
    for (_, t) in candidates {
        if let Some(s) = statics.schematic(t.schematic_id) {
            v.extend(s.inputs.iter().chain(&s.outputs).map(|(t, _)| *t));
        }
    }
    v.into_iter().collect()
}

/// **Every good the bot's colonies make**: the products of every
/// schematic a pin runs (what a P3 colony counts on, and a product it does
/// not make twice).
pub fn goods_of(views: &[pi::ColonyView], statics: &Statics) -> BTreeSet<i64> {
    views
        .iter()
        .flat_map(|v| &v.pins)
        .filter_map(|p| statics.schematic(p.schematic_id?))
        .flat_map(|s| s.outputs.iter().map(|(t, _)| *t))
        .collect()
}

// ---- the stream, pure -------------------------------------------------------

/// The stream without routes: the command centre (the hold's item) and
/// its upgrade, the factory west of it, the launchpad further west, the
/// two links and the schematic.
pub fn base_stream(t: &P3Template, cc_item: i64, site: &Site, level: i64) -> Vec<Command> {
    let cc = Id::Real(cc_item);
    let mut v = vec![Command::CreatePin {
        pin_id: cc.clone(),
        type_id: t.cc_type,
        point: at(site.theta),
    }];
    if level > 0 {
        v.push(Command::UpgradeCommandCenter {
            pin_id: cc.clone(),
            level,
        });
    }
    v.push(Command::CreatePin {
        pin_id: temp(FACTORY),
        type_id: t.factory_type,
        point: at(site.theta - pi::PIN_SPACING),
    });
    v.push(Command::CreatePin {
        pin_id: temp(LAUNCHPAD),
        type_id: t.launchpad_type,
        point: at(site.theta - 2.0 * pi::PIN_SPACING),
    });
    let link = |a: Id, b: Id| Command::CreateLink {
        endpoint1: a,
        endpoint2: b,
        level: 0,
    };
    v.push(link(temp(FACTORY), cc));
    v.push(link(temp(LAUNCHPAD), temp(FACTORY)));
    v.push(Command::SetSchematic {
        pin_id: temp(FACTORY),
        schematic_id: t.schematic_id,
    });
    v
}

/// The whole stream: [`base_stream`] and the routes -- each input from the
/// launchpad into the factory at one cycle's need, the product from the
/// factory into the launchpad.
pub fn with_routes(
    statics: &Statics,
    t: &P3Template,
    cc_item: i64,
    site: &Site,
    level: i64,
) -> std::result::Result<Vec<Command>, sim::PiError> {
    let mut v = base_stream(t, cc_item, site, level);
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
    for &(input, need) in &t.inputs {
        v.push(route(vec![temp(LAUNCHPAD), temp(FACTORY)], input, need));
    }
    let out = statics
        .schematic(t.schematic_id)
        .and_then(|s| s.outputs.first().map(|(_, q)| *q))
        .unwrap_or(1);
    v.push(route(vec![temp(FACTORY), temp(LAUNCHPAD)], t.product, out));
    pi_p2::dry_run(statics, site, &v, &Quotes::default())?;
    Ok(v)
}

/// **Fit the colony**: the lowest command-centre level up to `max_level`
/// whose whole stream the colony code accepts and whose bill fits `budget`.
pub fn fit(
    statics: &Statics,
    t: &P3Template,
    cc_item: i64,
    site: &Site,
    max_level: i64,
    budget: f64,
) -> Option<Layout> {
    let pins = statics.base_price(t.factory_type) + statics.base_price(t.launchpad_type);
    (0..=max_level.clamp(0, sim::MAX_COMMAND_CENTRE_LEVEL)).find_map(|level| {
        let cost = pins + sim::command_centre_upgrade_cost(0, level).unwrap_or(0) as f64;
        (cost <= budget && with_routes(statics, t, cc_item, site, level).is_ok()).then(|| Layout {
            level,
            heads: Vec::new(),
            head_radius: 0.0,
            cost,
        })
    })
}

// ---- what a colony wants, pure ----------------------------------------------

/// The P3 or P4 schematic a colony's factory runs.
fn running<'a>(view: &pi::ColonyView, statics: &'a Statics) -> Option<&'a Schematic> {
    let tier = tiers(statics);
    view.pins
        .iter()
        .filter_map(|p| statics.schematic(p.schematic_id?))
        .find(|s| tier_of(&tier, s).is_some_and(|t| t >= FIRST_TIER))
}

/// The launchpad pin of a colony.
fn pad<'a>(view: &'a pi::ColonyView, statics: &Statics) -> Option<&'a pi::PinView> {
    view.pins
        .iter()
        .find(|p| statics.pin_kind(p.type_id) == Some(PinKind::Spaceport))
}

/// **What a colony imports**: for a P3 or P4 colony, each input up to
/// [`STOCK_CYCLES`] of it on the launchpad, sized one after the other to
/// the pad's free volume; any other colony is [`pi_p2::feeds`]'s.
pub fn feeds(view: &pi::ColonyView, statics: &Statics) -> Vec<Feed> {
    let (Some(s), Some(lp)) = (running(view, statics), pad(view, statics)) else {
        return pi_p2::feeds(view, statics);
    };
    let capacity = statics.pin_type(lp.type_id).map_or(0.0, |p| p.capacity);
    let used: f64 = lp
        .contents
        .iter()
        .map(|(t, q)| statics.volume(*t) * *q as f64)
        .sum();
    let mut free = capacity - used;
    let mut out = Vec::new();
    for &(t, need) in &s.inputs {
        let want = STOCK_CYCLES * need - lp.contents.get(&t).copied().unwrap_or(0);
        let vol = statics.volume(t);
        let fits = if vol > 0.0 {
            (free / vol).floor().max(0.0) as i64
        } else {
            want
        };
        let room = want.min(fits);
        if room > 0 {
            free -= room as f64 * vol;
            out.push(Feed {
                planet: view.planet_id,
                launchpad: lp.id,
                type_id: t,
                room,
            });
        }
    }
    out
}

/// One export: `quantity` of `type_id` off `launchpad`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Export {
    pub planet: i64,
    pub launchpad: i64,
    pub type_id: i64,
    pub quantity: i64,
}

/// **What a colony exports**: the product of its P3 or P4 factory on its
/// launchpad.
pub fn exports(view: &pi::ColonyView, statics: &Statics) -> Vec<Export> {
    let (Some(s), Some(lp)) = (running(view, statics), pad(view, statics)) else {
        return Vec::new();
    };
    s.outputs
        .iter()
        .filter_map(|(t, _)| {
            let quantity = lp.contents.get(t).copied().filter(|q| *q > 0)?;
            Some(Export {
                planet: view.planet_id,
                launchpad: lp.id,
                type_id: *t,
                quantity,
            })
        })
        .collect()
}

// ---- over the wire ----------------------------------------------------------

/// **The decision point**: `Some((planet, template))` when a P3 or P4
/// colony is reachable and pays. `goods` are what the bot's colonies make
/// (the hangar is no gate: the farmer's sale lists whatever no colony wants
/// on every visit). Prices are the market signal's ([`pi_p2::prices_of`]).
pub fn pick(
    c: &mut Conn,
    statics: &Statics,
    planets: &[(i64, i64)],
    ours: &BTreeSet<i64>,
    skip: &BTreeSet<i64>,
    goods: &BTreeSet<i64>,
    seed: u64,
) -> Result<Option<(i64, P3Template)>> {
    if !P3_FIRST {
        return Ok(None);
    }
    let cands = candidates(statics, planets, ours, skip, goods, goods);
    if cands.is_empty() {
        return Ok(None);
    }
    let prices = match pi_p2::prices_of(c, &priced_types(statics, &cands)) {
        Ok(p) => p,
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(_) => return Ok(None),
    };
    Ok(choose(statics, cands, &prices, seed))
}

/// **Found the colony** on `planet`, the bot in space with the command
/// centre `cc_item` in its hold. `Ok(Ok((level, 0)))`, or the reason it was
/// not built.
#[allow(clippy::too_many_arguments)]
pub fn found(
    c: &mut Conn,
    statics: &Statics,
    planet: i64,
    t: &P3Template,
    cc_item: i64,
    owner: i64,
    budget: f64,
    max_level: i64,
    seed: u64,
) -> Result<std::result::Result<(i64, usize), String>> {
    let info = pi::planet_info(c, planet)?.unwrap_or(PyValue::None);
    let site = Site {
        planet_id: planet,
        owner_id: owner,
        planet_type: int(kv(&info, "planetTypeID")).unwrap_or(t.planet_type),
        radius: real(kv(&info, "radius")).unwrap_or(0.0),
        theta: (seed % 628) as f64 / 100.0,
    };
    let Some(layout) = fit(statics, t, cc_item, &site, max_level, budget) else {
        return Ok(Err(
            "no P3 layout fits the power, the CPU and the wallet".to_owned()
        ));
    };
    let stream = match with_routes(statics, t, cc_item, &site, layout.level) {
        Ok(s) => s,
        Err(e) => return Ok(Err(format!("the P3 routes: {}", e.message_id()))),
    };
    Ok(pi::submit(c, planet, &stream)?.map(|_| (layout.level, 0)))
}

/// What one [`tend`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tended {
    /// Units bought for the colonies' launchpads.
    pub bought: i64,
    /// Units imported onto launchpads.
    pub fed: i64,
    /// Units of product exported, the office's tax paid.
    pub exported: i64,
    pub refusals: Vec<String>,
}

fn soft<T>(r: Result<T>) -> Result<std::result::Result<T, String>> {
    match r {
        Ok(v) => Ok(Ok(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(Err(e.to_string())),
    }
}

/// **The colonies' trade on one visit**, docked at `home` to start and to
/// end: what each colony imports ([`feeds`]), the part of it the hangar
/// lacks bought from the station's asks ([`stock`]), the carrying
/// ([`pi_p2::feed`], import tax paid), and the product out ([`export`],
/// export tax paid). Returns the types the colonies import: not for sale.
pub fn tend(
    c: &mut Conn,
    statics: &Statics,
    home: i64,
    views: &[pi::ColonyView],
) -> Result<(BTreeSet<i64>, Tended)> {
    let mut done = Tended::default();
    let wants: Vec<Feed> = views.iter().flat_map(|v| feeds(v, statics)).collect();
    let keep: BTreeSet<i64> = wants.iter().map(|f| f.type_id).collect();
    if !wants.is_empty() {
        let p3_pads: BTreeSet<i64> = views
            .iter()
            .filter(|v| running(v, statics).is_some())
            .map(|v| v.planet_id)
            .collect();
        let own: Vec<Feed> = wants
            .iter()
            .filter(|f| p3_pads.contains(&f.planet))
            .copied()
            .collect();
        if !own.is_empty() {
            done.bought = stock(c, home, &own, &mut done.refusals)?;
        }
        let fed = pi_p2::feed(c, statics, home, &wants)?;
        done.fed = fed.units;
        done.refusals.extend(fed.refusals);
    }
    let out: Vec<Export> = views.iter().flat_map(|v| exports(v, statics)).collect();
    if !out.is_empty() {
        done.exported = export(c, statics, home, &out, &mut done.refusals)?;
    }
    Ok((keep, done))
}

/// **Stock the hangar** for `wants`: per type, what the colonies want less
/// what the hangar holds, bought from the home station's asks at most
/// [`BUY_CAP`] x `P_mkt` (and the consumer's reservation), within
/// [`STOCK_SHARE`] of the wallet. Returns the units bought.
pub fn stock(c: &mut Conn, home: i64, wants: &[Feed], refusals: &mut Vec<String>) -> Result<i64> {
    let mut need: BTreeMap<i64, i64> = BTreeMap::new();
    for f in wants {
        *need.entry(f.type_id).or_default() += f.room;
    }
    let hangar = c.hangar_items()?;
    let mut prices = crate::pricing::Prices::read(c)?;
    let mut isk = c.cash_balance()? * STOCK_SHARE;
    let mut bought = 0;
    for (&type_id, &want) in &need {
        let have: i64 = hangar
            .iter()
            .filter(|r| r.type_id == type_id && !r.singleton)
            .map(|r| r.quantity)
            .sum();
        let missing = want - have;
        if missing <= 0 {
            continue;
        }
        let book = match soft(c.market_orders(type_id))? {
            Ok(b) => b,
            Err(why) => {
                refusals.push(format!("book {type_id}: {why}"));
                continue;
            }
        };
        let mkt = prices.p_mkt(c, type_id)?;
        let cap = match (mkt, prices.reservation(c, type_id)?) {
            (Some(m), Some(r)) => Some((BUY_CAP * m).min(r)),
            (Some(m), None) => Some(BUY_CAP * m),
            (None, r) => r,
        };
        for (ask, k) in crate::acquire::plan(&book, home, cap, isk, missing) {
            match soft(c.buy(home, type_id, ask.price, k, 0, crate::builder::BROKER_FEE))? {
                Ok(_) => {
                    isk -= ask.price * k as f64;
                    bought += k;
                }
                Err(why) => {
                    refusals.push(format!("buy {type_id}: {why}"));
                    break;
                }
            }
        }
    }
    Ok(bought)
}

/// **Export the products**, docked at `home` to start and to end: undock,
/// per colony read the office's rate (`planetOrbitalRegistryBroker.
/// GetTaxRate` [V]), `ImportExportWithPlanet(launchpadID, {}, {typeID:
/// qty}, rate)` [V] (the export tax is paid here), take the goods from the
/// office into the hold (`Add(itemID, officeID, qty=n, flag=cargo)` on the
/// ship's inventory [V]); dock and put the hold into the hangar, where the
/// farmer's sale finds it. Returns the units exported.
pub fn export(
    c: &mut Conn,
    statics: &Statics,
    home: i64,
    wants: &[Export],
    refusals: &mut Vec<String>,
) -> Result<i64> {
    let Some(office_type) = statics.customs_type().map(|t| t.type_id) else {
        refusals.push("no customs office type in the PI table".to_owned());
        return Ok(0);
    };
    pi::leave(c)?;
    let mut units = 0;
    let mut taken: BTreeSet<i64> = BTreeSet::new();
    let done = (|| -> Result<()> {
        let ship = c
            .session()
            .ship_id()
            .ok_or_else(|| Error::NotSent("pi export: no ship".to_owned()))?;
        let system = c.session().solar_system_id().unwrap_or(0);
        for e in wants {
            let office = {
                let world = c.world_now();
                world
                    .balls()
                    .filter(|b| b.type_id() == Some(office_type))
                    .filter_map(|b| Some((world.surface_distance(e.planet, b.ball_id)?, b.ball_id)))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, id)| id)
            };
            let Some(office) = office else {
                refusals.push(format!("no customs office over {}", e.planet));
                continue;
            };
            match export_one(c, office, ship, system, e)? {
                Ok(()) => {
                    units += e.quantity;
                    taken.insert(e.type_id);
                }
                Err(why) => refusals.push(why),
            }
        }
        Ok(())
    })();
    pi::fly_home(c, home)?;
    done?;
    for row in c.cargo()? {
        if taken.contains(&row.type_id)
            && let Err(why) = soft(c.unload(row.item_id, None))?
        {
            refusals.push(format!("unload {}: {why}", row.type_id));
        }
    }
    Ok(units)
}

/// One export: pad -> office -> hold.
fn export_one(
    c: &mut Conn,
    office: i64,
    ship: i64,
    system: i64,
    e: &Export,
) -> Result<std::result::Result<(), String>> {
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
        Err(err @ (Error::Io(_) | Error::Protocol(_))) => return Err(err),
        Err(err) => return Ok(Err(format!("GetTaxRate: {err}"))),
    };
    let inv = match soft(c.open_container(office))? {
        Ok(i) => i,
        Err(why) => return Ok(Err(format!("open office {office}: {why}"))),
    };
    let moved = c.call_on(
        &inv,
        "ImportExportWithPlanet",
        vec![
            PyValue::Int(e.launchpad),
            PyValue::Dict(Vec::new()),
            PyValue::Dict(vec![(PyValue::Int(e.quantity), PyValue::Int(e.type_id))]),
            PyValue::Real(rate),
        ],
        vec![],
    );
    match moved {
        Ok(a) => {
            if let Some(k) = a.refusal() {
                return Ok(Err(format!("export: {k}")));
            }
        }
        Err(err @ (Error::Io(_) | Error::Protocol(_))) => return Err(err),
        Err(err) => return Ok(Err(format!("export: {err}"))),
    }
    let rows = match soft(c.list(&inv, Some(sim::CUSTOMS_OFFICE_FLAG)))? {
        Ok(r) => r,
        Err(why) => return Ok(Err(format!("list the office: {why}"))),
    };
    let hold = match soft(c.open_ship(ship))? {
        Ok(i) => i,
        Err(why) => return Ok(Err(format!("open the ship: {why}"))),
    };
    let mut left = e.quantity;
    for r in rows.into_iter().filter(|r| r.type_id == e.type_id) {
        if left <= 0 {
            break;
        }
        let take = left.min(r.quantity);
        let added = c.call_on(
            &hold,
            "Add",
            vec![PyValue::Int(r.item_id), PyValue::Int(office)],
            vec![
                (PyValue::Int(take), PyValue::Bytes(b"qty".to_vec())),
                (
                    PyValue::Int(crate::FLAG_CARGO),
                    PyValue::Bytes(b"flag".to_vec()),
                ),
            ],
        );
        match added {
            Ok(a) => {
                if let Some(k) = a.refusal() {
                    return Ok(Err(format!("into the hold: {k}")));
                }
            }
            Err(err @ (Error::Io(_) | Error::Protocol(_))) => return Err(err),
            Err(err) => return Ok(Err(format!("into the hold: {err}"))),
        }
        left -= take;
    }
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids and rows only.
    const PT: i64 = 990_700_001;
    const PT_LOW: i64 = 990_700_002;
    const CC: i64 = 990_700_011;
    const BASIC_T: i64 = 990_700_015;
    const ADV_T: i64 = 990_700_016;
    const HT_ONLY: i64 = 990_700_018;
    const PAD_T: i64 = 990_700_017;
    const OFFICE_T: i64 = 990_700_019;
    const RES_A: i64 = 990_700_021;
    const RES_B: i64 = 990_700_022;
    const P1_A: i64 = 990_700_031;
    const P1_B: i64 = 990_700_032;
    const P2_A: i64 = 990_700_041;
    const P2_B: i64 = 990_700_042;
    const P3_X: i64 = 990_700_051;
    const P4_Y: i64 = 990_700_061;
    const S_A: i64 = 990_701;
    const S_B: i64 = 990_702;
    const S_P2A: i64 = 990_703;
    const S_P2B: i64 = 990_704;
    const S_P3: i64 = 990_705;
    const S_P4: i64 = 990_706;
    const PLANET: i64 = 40_970_001;
    const OWNER: i64 = 90_000_700;
    const CC_ITEM: i64 = 100_000_700;

    /// A recipe chain: P0 -> P1 (two), P1+P1 -> P2 (two), P2+P2 -> P3, and
    /// P3 + P1 -> P4 in a facility only `PT` allows. `PT_LOW` allows the
    /// advanced factory, not the high-tech one.
    fn statics() -> Statics {
        let link = pi::LINK_TYPE;
        Statics::parse(&format!(
            "pintype\t{CC}\t1027\t500.0\t0.0\t0\t48=1675.0;11=6000.0\n\
             pintype\t{BASIC_T}\t1028\t0.0\t2000.0\t0\t49=100.0;15=100.0\n\
             pintype\t{ADV_T}\t1028\t0.0\t5000.0\t0\t49=150.0;15=150.0\n\
             pintype\t{HT_ONLY}\t1028\t0.0\t9000.0\t0\t49=200.0;15=200.0;1632={PT}.0\n\
             pintype\t{PAD_T}\t1030\t10000.0\t9000.0\t0\t49=300.0;15=200.0;1638=0.5;1639=1.0\n\
             pintype\t{link}\t1036\t0.0\t0.0\t0\t49=15.0;15=10.0;1631=1250.0;1633=0.15;1634=0.2;1635=1.4;1636=1.2\n\
             schematic\t{S_A}\t60\t{BASIC_T}\t{RES_A}=10\t{P1_A}=5\n\
             schematic\t{S_B}\t60\t{BASIC_T}\t{RES_B}=10\t{P1_B}=5\n\
             schematic\t{S_P2A}\t60\t{ADV_T}\t{P1_A}=5;{P1_B}=5\t{P2_A}=3\n\
             schematic\t{S_P2B}\t60\t{ADV_T}\t{P1_A}=6;{P1_B}=4\t{P2_B}=3\n\
             schematic\t{S_P3}\t3600\t{ADV_T}\t{P2_A}=10;{P2_B}=10\t{P3_X}=3\n\
             schematic\t{S_P4}\t3600\t{HT_ONLY}\t{P3_X}=6;{P1_A}=40\t{P4_Y}=1\n\
             itemtype\t{CC}\t1027\t41\t10.0\t500.0\tFixture Centre\n\
             itemtype\t{P1_A}\t1042\t43\t0.5\t0.0\tFixture P1 A\n\
             itemtype\t{P1_B}\t1042\t43\t0.5\t0.0\tFixture P1 B\n\
             itemtype\t{P2_A}\t1034\t43\t1.0\t0.0\tFixture P2 A\n\
             itemtype\t{P2_B}\t1034\t43\t1.0\t0.0\tFixture P2 B\n\
             itemtype\t{P3_X}\t1040\t43\t3.0\t0.0\tFixture P3\n\
             itemtype\t{P4_Y}\t1041\t43\t50.0\t0.0\tFixture P4\n\
             taxmult\t{P2_A}\t100.0\n\
             taxmult\t{P3_X}\t1000.0\n\
             taxmult\t{P4_Y}\t10000.0\n\
             customsoffice\t{OFFICE_T}\t35000.0\t8000.0\t0.1\n\
             resource\t{PT}\t{RES_A}=0.6;{RES_B}=0.8\n\
             resource\t{PT_LOW}\t{RES_B}=0.9\n"
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

    fn set(v: &[i64]) -> BTreeSet<i64> {
        v.iter().copied().collect()
    }

    /// The tiers come off the recipes.
    #[test]
    fn the_tiers_are_read_off_the_recipes() {
        let t = tiers(&statics());
        let of = |x: i64| t.get(&x).copied();
        assert_eq!(of(RES_A), Some(0));
        assert_eq!(of(P1_A), Some(1));
        assert_eq!(of(P2_B), Some(2));
        assert_eq!(of(P3_X), Some(3));
        assert_eq!(of(P4_Y), Some(4), "a P3 and a P1 make a P4");
    }

    /// A colony needs one input held or made; a P4 only where the high-tech
    /// facility stands; a product already made is not made twice.
    #[test]
    fn a_colony_needs_an_input_in_hand_and_a_facility_the_planet_allows() {
        let s = statics();
        assert!(templates(&s, PT, &set(&[]), &set(&[])).is_empty());
        let t = templates(&s, PT, &set(&[P2_A]), &set(&[]));
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!(
            (t[0].schematic_id, t[0].factory_type, t[0].tier),
            (S_P3, ADV_T, 3)
        );
        assert_eq!(t[0].inputs, vec![(P2_A, 10), (P2_B, 10)]);
        // A P3 in hand opens the P4, on the planet type that allows it.
        let both = templates(&s, PT, &set(&[P2_A, P3_X]), &set(&[]));
        let ids: Vec<i64> = both.iter().map(|t| t.schematic_id).collect();
        assert_eq!(ids, vec![S_P3, S_P4]);
        assert_eq!(both[1].factory_type, HT_ONLY);
        let low = templates(&s, PT_LOW, &set(&[P2_A, P3_X]), &set(&[]));
        assert_eq!(
            low.iter().map(|t| t.schematic_id).collect::<Vec<_>>(),
            vec![S_P3],
            "no high-tech facility on this planet type"
        );
        assert!(templates(&s, PT, &set(&[P2_A]), &set(&[P3_X])).is_empty());
    }

    /// Both taxes are in the margin: the market pays a P3 over its inputs
    /// and the office's cut, or the colony is not chosen.
    #[test]
    fn the_margin_pays_the_customs_office_both_ways() {
        let s = statics();
        let t = templates(&s, PT, &set(&[P2_A]), &set(&[])).remove(0);
        let sch = s.schematic(S_P3).unwrap();
        let tax = tax_per_cycle(&s, PAD_T, sch);
        // In: 10 x 0.5 x 100 x 0.1 for P2_A, the default multiplier for
        // P2_B; out: 3 x 1.0 x 1000 x 0.1.
        let default = s.import_tax_multiplier(P2_B);
        let expect = 10.0 * 0.5 * 100.0 * 0.1 + 10.0 * 0.5 * default * 0.1 + 3.0 * 1000.0 * 0.1;
        assert!((tax - expect).abs() < 1e-9, "{tax} {expect}");

        let prices = |p3: f64| BTreeMap::from([(P2_A, 10.0), (P2_B, 10.0), (P3_X, p3)]);
        let m = margin_per_hour(&s, &t, &prices(200.0)).unwrap();
        assert!((m - (600.0 - 200.0 - tax)).abs() < 1e-6, "{m}");
        let c = vec![(PLANET, t.clone())];
        assert!(choose(&s, c.clone(), &prices(200.0), 3).is_some());
        assert!(choose(&s, c.clone(), &prices(60.0), 3).is_none());
        // No price for an input: not gambled on.
        let part = BTreeMap::from([(P2_A, 10.0), (P3_X, 900.0)]);
        assert!(margin_per_hour(&s, &t, &part).is_none());
    }

    /// The colony dry-runs at the lowest level, its routes bring each
    /// input from the pad and the product back to it, and the server's
    /// simulation turns imported inputs into product on the pad.
    #[test]
    fn the_colony_makes_p3_from_imported_inputs_and_parks_it_on_the_pad() {
        let s = statics();
        let t = templates(&s, PT, &set(&[P2_A]), &set(&[])).remove(0);
        let layout = fit(&s, &t, CC_ITEM, &site(PT), 2, f64::INFINITY).expect("a layout");
        assert_eq!(layout.level, 0, "the cheapest level that fits");
        assert!(fit(&s, &t, CC_ITEM, &site(PT), 2, 1.0).is_none(), "wallet");
        let stream = with_routes(&s, &t, CC_ITEM, &site(PT), 0).expect("routes");
        let routes: Vec<(i64, i64)> = stream
            .iter()
            .filter_map(|c| match c {
                Command::CreateRoute {
                    type_id, quantity, ..
                } => Some((*type_id, *quantity)),
                _ => None,
            })
            .collect();
        assert_eq!(routes, vec![(P2_A, 10), (P2_B, 10), (P3_X, 3)]);

        let (mut colony, _) =
            pi_p2::dry_run(&s, &site(PT), &stream, &Quotes::default()).expect("dry run");
        assert_eq!(colony.pins.len(), 3);
        let pad_id = colony
            .pins
            .iter()
            .find(|p| p.type_id == PAD_T)
            .unwrap()
            .pin_id;
        // Unfed: nothing. Fed one day: P3 on the pad, none in the centre.
        let mut unfed = colony.clone();
        unfed.run_simulation(&s, unfed.current_sim_time + 6 * sim::HOUR);
        assert_eq!(unfed.pin(pad_id).unwrap().quantity(P3_X), 0);
        colony
            .import_commodities(
                &s,
                OWNER,
                pad_id,
                &BTreeMap::from([(P2_A, 240), (P2_B, 240)]),
            )
            .expect("the pad takes both");
        colony.run_simulation(&s, colony.current_sim_time + 6 * sim::HOUR);
        assert!(colony.pin(pad_id).unwrap().quantity(P3_X) > 0);
        assert_eq!(colony.command_pin(&s).unwrap().quantity(P3_X), 0);
        // And the P3 leaves through the office, as the export asks.
        let made = colony.pin(pad_id).unwrap().quantity(P3_X);
        colony
            .export_commodities(&s, OWNER, pad_id, &BTreeMap::from([(P3_X, made)]))
            .expect("the pad gives it up");
        assert_eq!(colony.pin(pad_id).unwrap().quantity(P3_X), 0);
    }

    /// A P4 colony on the planet type with the high-tech facility.
    #[test]
    fn a_p4_colony_runs_on_the_high_tech_facility() {
        let s = statics();
        let t = templates(&s, PT, &set(&[P3_X]), &set(&[]))
            .into_iter()
            .find(|t| t.tier == 4)
            .expect("a P4 colony");
        let stream = with_routes(&s, &t, CC_ITEM, &site(PT), 0).expect("routes");
        let (mut colony, _) =
            pi_p2::dry_run(&s, &site(PT), &stream, &Quotes::default()).expect("dry run");
        let pad_id = colony
            .pins
            .iter()
            .find(|p| p.type_id == PAD_T)
            .unwrap()
            .pin_id;
        colony
            .import_commodities(&s, OWNER, pad_id, &BTreeMap::from([(P3_X, 12), (P1_A, 80)]))
            .expect("the pad takes both");
        colony.run_simulation(&s, colony.current_sim_time + 6 * sim::HOUR);
        assert!(colony.pin(pad_id).unwrap().quantity(P4_Y) > 0);
    }

    fn view_of(colony: &sim::Colony) -> pi::ColonyView {
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

    /// What the colony wants imported: a day of each input, sized one after
    /// the other to the pad; and what it exports: the product on the pad.
    #[test]
    fn the_colony_wants_a_day_of_inputs_and_exports_what_the_pad_holds() {
        let s = statics();
        let t = templates(&s, PT, &set(&[P2_A]), &set(&[])).remove(0);
        let stream = with_routes(&s, &t, CC_ITEM, &site(PT), 0).unwrap();
        let (mut colony, _) = pi_p2::dry_run(&s, &site(PT), &stream, &Quotes::default()).unwrap();
        let pad_id = colony
            .pins
            .iter()
            .find(|p| p.type_id == PAD_T)
            .unwrap()
            .pin_id;
        let wants = feeds(&view_of(&colony), &s);
        let day = STOCK_CYCLES * 10;
        assert_eq!(
            wants,
            vec![
                Feed {
                    planet: PLANET,
                    launchpad: pad_id,
                    type_id: P2_A,
                    room: day
                },
                Feed {
                    planet: PLANET,
                    launchpad: pad_id,
                    type_id: P2_B,
                    room: day
                },
            ]
        );
        assert!(exports(&view_of(&colony), &s).is_empty());
        colony
            .import_commodities(&s, OWNER, pad_id, &BTreeMap::from([(P2_A, 100)]))
            .unwrap();
        let wants = feeds(&view_of(&colony), &s);
        assert_eq!(wants[0].room, day - 100, "what the pad holds counts");
        colony.run_simulation(&s, colony.current_sim_time + sim::HOUR);
        // A colony that is not P3 is pi_p2's: this one has no import.
        let mut plain = view_of(&colony);
        plain.pins.retain(|p| p.type_id != ADV_T);
        assert!(feeds(&plain, &s).is_empty());
        assert!(goods_of(&[view_of(&colony)], &s).contains(&P3_X));
    }
}
