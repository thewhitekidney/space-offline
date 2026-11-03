//! Mining: a laser cycle on a rock becomes ore in the hold, and the rock shrinks.
//!
//! Owned by the P3.4 lane (`docs/lanes/mining.md`). Rocks are
//! `crate::asteroid`'s; cycles are `crate::modules`'; this is the half that
//! turns one into the other and tells the clients.
//!
//! ## The loop, end to end
//!
//! 1. The pilot locks a rock (`targeting::lock`, P3.1) and clicks the Miner I.
//!    The client checks only that *something* is locked (`smb:1541-1547`)
//!    `[V]` and calls `dogmaIM.Activate(moduleID, 'miningLaser', rockID,
//!    1000)` -- mining lasers always auto-repeat
//!    (`shipmodulebutton.GROUPS_THAT_ALWAYS_AUTO_REPEAT` holds
//!    `groupMiningLaser`) `[V]`.
//! 2. `modules::activate` runs its usual checks and then [`check_activation`]:
//!    the target is a rock in this system, within the module's `maxRange`,
//!    and the hold has room for one unit of its ore.
//! 3. Every cycle that **ends** reaches [`tick`] through
//!    `Modules::take_completed` -- a queue of its own, so combat's
//!    `take_cycles` is untouched (`docs/lanes/mining.md` §2). The ore
//!    arrives at the end of the cycle **[A]**: EVE's behaviour as players
//!    know it, not read in this build.
//! 4. Per completed cycle ([`on_completed`]): `units = floor(miningAmount /
//!    volume)` **[A]** (Miner I's `miningAmount` 77 is 10 m3 `[V]`, ore
//!    volume is the type's `[V]`), cut to what the rock holds and what the
//!    hold can take. The store grows the pilot's stack of that ore in cargo
//!    (flag 5) or creates it, **then** the rock gives up exactly that many
//!    units (`asteroid::Field::mine`), so ore is conserved.
//!    **Crit and residue (MINING-CRITS).** The units are not a floor but the
//!    client's own `mining.MiningResult` (`[V]`, [`roll`]): `round_rand` of
//!    the volume, a percent residue (`3154` against `randint(0, 100)`, the
//!    rock loses it too) and a fractional crit (`5967` against `random()`,
//!    `5969` times the yield into the hold, **not** out of the rock), all
//!    from the module's **modified** attributes ([`Take`]) and a stream
//!    seeded by `(rock, module, pilot, cycle end)`; the ore's `6108` and
//!    `3236` switch them off. `docs/lanes/mining-crits.md`.
//! 5. The clients are told: the cargo row by `apply_items`' own
//!    `OnItemChange` + `OnInvalidateCacheForLocations`, addressed to the
//!    owner; the rock's new size by `SetBallRadius` to everyone in the
//!    system ([`RockRadius`]); a rock mined out by `RemoveBalls`, its locks
//!    released and every laser on it stopped with CCP's
//!    `MiningItemDepleted`; a full hold by stopping the laser with
//!    `MiningDronesDeactivatedCargoHoldNowFull` (its text is about any
//!    module: "Your {modulename} has completed operations. Ship's cargo hold
//!    is full.").
//!
//! **Everything leaves on the clock queue.** This runs on the world tick,
//! which is not anybody's payload turn: `pending` would wait for the miner's
//! next click. [`on_clock`] moves what `apply_items` and `targeting::unlock`
//! put on `pending` onto `Registry::schedule`, the same trade
//! `combat::run_death_plan` makes.
//!
//! **Gas (SOC-B2).** A gas harvester ([`EFFECT_MINING_CLOUDS`], 2726) runs
//! the same loop on a gas cloud: a site's cloud is a rock in
//! `asteroid::Field` (`crate::site` places it with its gas law), the cloud's
//! type is the gas that arrives, and `units = floor(miningAmount / volume)`
//! as for ore (a Gas Cloud Scoop I's 10 m3 on a 10 m3 cytoserocin is one
//! unit a cycle [V-data]). [`cuts`] keeps lasers off clouds and harvesters
//! off rocks. **The gas goes where ore does.**
//!
//! **The mining hold (GAPS-B).** A ship with `generalMiningHoldCapacity`
//! (1556 [V]) has a mining hold (`flagGeneralMiningHold` 134 [V]); every
//! cut -- laser, ice harvester (the laser's effect), gas harvester and
//! drone -- fills it first when it takes the type (ore, ice, gas: the
//! client's `ShouldAllowAdd` row), then the cargo hold
//! (`crate::mining_hold`). "The hold" below means both.
//!
//! **Drones (SOC-B10).** A mining drone ([`EFFECT_MINING`], 17) is a ball,
//! not a module: `crate::drones` flies it to the rock on
//! `entity.CmdMineRepeatedly` and, at the end of each of its cycles while
//! it is within its `maxRange` of the rock, calls [`drone_cycle`]. The ore
//! goes straight to the **controlling ship's cargo hold** at the end of the
//! cycle **[A]** (in EVE the drone carries it back first; here it does not
//! fly the load home). The cut is the laser's ([`cut`]): `floor(miningAmount
//! / volume)` of the drone's own attribute 77, cut to the rock and the hold.
//!
//! **Depletion is not persisted.** The field lives in `SandboxConfig` and is
//! laid out from `--belt-rocks` at every start, so a restart regrows every
//! rock. Persisting it would mean a snapshot section for rock quantities;
//! not worth it until belts respawn on a schedule anyway.

use crate::modules::{self, Cycle};
use crate::notify::{Addressed, Push};
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::time::Duration;

/// `effectMiningClouds` **[V-data]** (`dogmaeffects[2726]`: `miningClouds`,
/// category 2 "target", `durationAttributeID` 73, `rangeAttributeID` 54,
/// guid `effects.CloudMining`): the default effect of every Gas Cloud Scoop
/// and Gas Cloud Harvester (groups 737 and 4138) [V-data]. Its cycle is
/// the laser's -- `miningAmount` (77) cubic metres per completed cycle --
/// on a gas cloud instead of a rock (SOC-B2).
pub const EFFECT_MINING_CLOUDS: i64 = 2726;

/// `groupHarvestableCloud` **[V]** (`inventorycommon/const`: 711), category
/// 2 (*Celestial*) [V-data, `groups.fsdbinary`]. **The cloud is the gas**:
/// the type in space is the type that arrives in the hold, as a rock's ore
/// is (every cloud type is published, `volume` 1-10 m3 [V-data]).
pub const GROUP_HARVESTABLE_CLOUD: i64 = 711;

/// `effectMining` **[V-data]** (`dogmaeffects[17]`: `mining`, category 2
/// "target", `durationAttributeID` 73, `rangeAttributeID` 54, guid
/// `effects.Mining`): the default effect of every mining drone (group 101)
/// [V-data]. Its cycles are not module cycles: `crate::drones` runs them and
/// hands each to [`drone_cycle`] (SOC-B10).
pub const EFFECT_MINING: i64 = 17;

/// `miningAmount` (77), m3 per cycle: the drone's own attribute, as the
/// laser's (`modules::attr::MINING_AMOUNT`).
pub const ATTR_MINING_AMOUNT: i64 = modules::attr::MINING_AMOUNT;

/// The attributes of a cycle's crit and residue (`dogma/const.py`, `[V]`
/// `docs/lanes/mining-crits.md`): `miningCritChance` (a fraction),
/// `miningCritBonusYield` (a multiple of the normal yield),
/// `miningWastedVolumeMultiplier` (a multiple of the normal volume) and
/// `miningWasteProbability` (a percent, 0 to 100).
pub const ATTR_CRIT_CHANCE: i64 = 5967;
pub const ATTR_CRIT_BONUS_YIELD: i64 = 5969;
pub const ATTR_WASTE_VOLUME_MULTIPLIER: i64 = 3153;
pub const ATTR_WASTE_PROBABILITY: i64 = 3154;
/// The ore's switches (an attribute of the rock's type): non-zero turns the
/// crit, or the residue, off for that ore.
pub const ATTR_IGNORE_CRITS: i64 = 6108;
pub const ATTR_IGNORE_WASTE: i64 = 3236;

/// Marks this module's draws in `seed::Kind::Loot`'s stream, so the cycle of
/// a rock never shares a stream with a wreck's (`salvage`'s own tag).
const STREAM_TAG: u64 = 0x004d_494e;

/// What one cycle cuts with: the module's **modified** attributes
/// (`dogma::module_attribute`), or a drone's type attributes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Take {
    /// `miningAmount` (77), m3 per cycle.
    pub amount: f64,
    /// 5967, a fraction.
    pub crit_chance: f64,
    /// 5969, times the normal quantity.
    pub crit_bonus: f64,
    /// 3153, times the normal volume.
    pub waste_volume: f64,
    /// 3154, a percent.
    pub waste_percent: f64,
}

/// A module's `maxRange` (54): the **modified** sheet (a foreman burst, a
/// skill, a beacon), the type row when the sheet does not hold the module
/// (a drone).
fn reach(w: &Registry, ship_id: i64, module_id: i64, type_id: i64) -> f64 {
    crate::dogma::module_attribute(w, ship_id, module_id, 54)
        .or_else(|| modules::find_in(w.content(), type_id).and_then(|t| t.attribute(54)))
        .unwrap_or(0.0)
}

impl Take {
    /// A type's own attributes: a drone's, or a module the dogma sheet does
    /// not hold.
    fn of_type(content: &crate::content::Content, type_id: i64) -> Take {
        let a = |id| crate::typedata::attribute_in(content, type_id, id).unwrap_or(0.0);
        Take {
            amount: a(ATTR_MINING_AMOUNT),
            crit_chance: a(ATTR_CRIT_CHANCE),
            crit_bonus: a(ATTR_CRIT_BONUS_YIELD),
            waste_volume: a(ATTR_WASTE_VOLUME_MULTIPLIER),
            waste_percent: a(ATTR_WASTE_PROBABILITY),
        }
    }

    /// A fitted module's **modified** attributes: skills, crystals, rigs
    /// and a foreman burst land here (`dogma::module_attribute`). Each falls
    /// back to the type when the sheet does not hold the module; the amount
    /// also when the sheet holds none (the hand-written module rows carry
    /// `miningAmount` alone).
    fn of_module(w: &Registry, ship_id: i64, module_id: i64, type_id: i64) -> Take {
        let base = Take::of_type(w.content(), type_id);
        // One sheet for the five reads: a cache hit still builds the inputs.
        let sheet = crate::dogma::sheet(w, ship_id);
        let m = |id, fallback: f64| {
            sheet
                .as_ref()
                .and_then(|s| s.module(module_id, id))
                .unwrap_or(fallback)
        };
        let amount = sheet
            .as_ref()
            .and_then(|s| s.module(module_id, ATTR_MINING_AMOUNT))
            .filter(|v| *v != 0.0)
            .unwrap_or_else(|| {
                if base.amount != 0.0 {
                    base.amount
                } else {
                    modules::find_in(w.content(), type_id).map_or(0.0, |t| t.mining_amount())
                }
            });
        Take {
            amount,
            crit_chance: m(ATTR_CRIT_CHANCE, base.crit_chance),
            crit_bonus: m(ATTR_CRIT_BONUS_YIELD, base.crit_bonus),
            waste_volume: m(ATTR_WASTE_VOLUME_MULTIPLIER, base.waste_volume),
            waste_percent: m(ATTR_WASTE_PROBABILITY, base.waste_percent),
        }
    }
}

/// What one cycle yields, in whole units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Yield {
    /// Leaves the rock and enters the hold.
    pub normal: i64,
    /// Enters the hold and does **not** leave the rock.
    pub crit: i64,
    /// Leaves the rock and enters nothing.
    pub wasted: i64,
}

/// The dev knob (`/devminecrit`, `--dev` only): draws that always succeed,
/// so a client test sees a crit, or residue, on the first cycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Forced {
    pub crit: bool,
    pub waste: bool,
}

/// The world slot behind [`Forced`] (`world_ext::SLOTS`). Not persisted.
#[derive(Debug, Default)]
pub struct ForcedSlot(pub Forced);

/// The world slot. **Coordinator-only** registration (`world_ext::SLOTS`).
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "mining-crits",
    make: |_| Box::new(ForcedSlot::default()),
};

/// `/devminecrit crit|waste|both|off`: **`--dev` only** (a normal server
/// answers "dev only"). The next mining cycles always crit and/or always
/// leave residue (when the module's own crit bonus or waste multiplier is
/// above zero), so the client's crit line and the ledger's wasted column can
/// be seen at once (`fci_mine_crit`).
pub fn slash(
    reg: &mut Registry,
    _actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    if !reg.config().dev {
        return Ok(crate::pyutil::bstr("dev only: start the server with --dev"));
    }
    let forced = match args {
        ["crit"] => Forced {
            crit: true,
            waste: false,
        },
        ["waste"] => Forced {
            crit: false,
            waste: true,
        },
        ["both"] => Forced {
            crit: true,
            waste: true,
        },
        ["off"] => Forced::default(),
        _ => {
            return Ok(crate::pyutil::bstr(
                "usage: /devminecrit crit | waste | both | off",
            ));
        }
    };
    reg.ext_mut::<ForcedSlot>().0 = forced;
    Ok(crate::pyutil::bstr(&format!(
        "dev: forced mining crit {}, waste {}.",
        forced.crit, forced.waste
    )))
}

/// `MiningResult._get_rounded_randomized`: the whole part, plus one when the
/// remainder beats a uniform draw `[V]`. The `1e-9` is [`units_in`]'s: a
/// quotient a hair under a whole number must not lose a unit.
fn round_rand(s: &mut crate::seed::Stream, value: f64) -> i64 {
    // The client draws even for a zero (`remainder > random.random()`), so
    // the stream moves the same way; a negative or NaN value is no units.
    let draw = s.unit();
    if value.is_nan() || value <= 0.0 {
        return 0;
    }
    let v = value + 1e-9;
    let whole = v.floor();
    whole as i64 + i64::from(v - whole > draw)
}

/// **`mining.MiningResult`, applied** (`[V]` the client's own class, which
/// ships the server's formula; `docs/lanes/mining-crits.md` §1). The draws
/// come in the client's order, from `s`:
///
/// 1. residue: `waste_percent > randint(0, 100)` (a percent, both ends in),
///    then the wasted volume is `normal_volume x waste_volume`;
/// 2. `normal = min(rock, round_rand(normal_volume / unit))`;
/// 3. `wasted = round_rand(min(rock - normal, wasted_volume / unit))`;
/// 4. crit: `normal > 0` and `crit_chance > random()`, then
///    `crit = round_rand(normal x crit_bonus)`.
///
/// `room` is the hold's room in units: **[A]** the client's `clamp_factor`
/// is the share of the cycle's volume the hold can take, and the delivered
/// units (`normal + crit`) are cut to the room at the end (a crit that does
/// not fit is lost, normal first). **[A]** efficiency is 1. `ignore_*` are
/// the ore's 6108 and 3236.
#[allow(clippy::too_many_arguments)]
fn roll(
    s: &mut crate::seed::Stream,
    t: &Take,
    unit_volume: f64,
    rock_quantity: i64,
    room: i64,
    ignore_crit: bool,
    ignore_waste: bool,
    forced: Forced,
) -> Yield {
    let positive = |v: f64| v.is_finite() && v > 0.0;
    if !positive(unit_volume) || !positive(t.amount) || rock_quantity < 1 || room < 1 {
        return Yield::default();
    }
    let clamp = ((room as f64 * unit_volume) / t.amount).min(1.0);
    let normal_volume = t.amount * clamp;
    let wasted_volume = if ignore_waste {
        0.0
    } else {
        let hit = t.waste_percent > s.range(0, 101) as f64 || forced.waste;
        if hit {
            normal_volume * t.waste_volume.max(0.0)
        } else {
            0.0
        }
    };
    let normal = round_rand(s, normal_volume / unit_volume).min(rock_quantity);
    let remaining = rock_quantity - normal;
    let wasted = round_rand(s, (wasted_volume / unit_volume).min(remaining as f64)).min(remaining);
    let crit = if !ignore_crit
        && normal > 0
        && (forced.crit || (t.crit_chance > 0.0 && t.crit_chance > s.unit()))
    {
        round_rand(s, normal as f64 * t.crit_bonus)
    } else {
        0
    };
    let normal = normal.min(room);
    Yield {
        normal,
        crit: crit.min(room - normal),
        wasted: wasted.min(rock_quantity - normal),
    }
}

/// The effects whose completed **module** cycles are mining cycles: the
/// laser and the gas harvester (SOC-B2). A drone's ([`EFFECT_MINING`]) are
/// `crate::drones`' and reach [`drone_cycle`] instead.
pub const MINING_EFFECTS: &[i64] = &[modules::EFFECT_MINING_LASER, EFFECT_MINING_CLOUDS];

pub fn is_mining(effect_id: i64) -> bool {
    MINING_EFFECTS.contains(&effect_id)
}

/// Whether `type_id` is a gas cloud: its item type is in
/// [`GROUP_HARVESTABLE_CLOUD`]. `false` for a type the item table does not
/// know -- which is every cloud until the bundle's `typedata.tsv` carries
/// group 711 (`tools/typedata.py` `GROUPS`, SOC-B2's coordinator diff).
pub fn is_gas(content: &crate::content::Content, type_id: i64) -> bool {
    crate::item::find_in(content, type_id).is_some_and(|t| t.group_id == GROUP_HARVESTABLE_CLOUD)
}

/// **Which mining effect cuts what** **[A]** (EVE as players know it; no
/// client code says it): a gas harvester only a gas cloud, a mining laser
/// anything but one. Falsified by a client that lets a Miner I start on a
/// cloud, or a scoop on a rock.
pub fn cuts(effect_id: i64, gas: bool) -> bool {
    if effect_id == EFFECT_MINING_CLOUDS {
        gas
    } else {
        !gas
    }
}

/// How many whole units `volume` cubic metres of an ore is. **[A]** the
/// floor: EVE has no fractional ore, and a cycle that cut 0.9 of a unit of
/// a 16 m3 ore yields nothing rather than a unit it did not cut.
///
/// The `1e-9` is for the float, not the rule: `10.0 / 0.1` must be 100, and
/// a quotient that lands a hair under a whole number would lose a unit.
pub fn units_in(volume: f64, unit_volume: f64) -> i64 {
    // Written so a NaN takes the early return too.
    if unit_volume.is_nan() || volume.is_nan() || unit_volume <= 0.0 || volume <= 0.0 {
        return 0;
    }
    (volume / unit_volume + 1e-9).floor() as i64
}

/// The free cubic metres in a ship's cargo hold: the hull type's capacity
/// (`types.fsdbinary`, `item::ItemType::capacity` -- a Merlin's is 150 m3
/// `[V]`) less what is at `(ship, flagCargo)`. Both from the world's
/// content (`crate::item::find_in`).
pub fn cargo_free(w: &Registry, ship_id: i64, ship_type_id: i64) -> f64 {
    let capacity = crate::item::find_in(w.content(), ship_type_id).map_or(0.0, |t| t.capacity);
    (capacity
        - w.store()
            .used_volume_in(w.content(), ship_id, crate::fitting::FLAG_CARGO))
    .max(0.0)
}

/// Surface to surface, ship to rock. **[A]** what `maxRange` is measured
/// against for a mining laser; the hull radius is the content's
/// (`destiny::hull::of_in`).
fn gap_to(
    content: &crate::content::Content,
    ship_position: [f64; 3],
    ship_type_id: i64,
    rock: &crate::asteroid::Rock,
) -> f64 {
    let hull = crate::destiny::hull::of_in(content, ship_type_id);
    crate::space_state::distance(ship_position, rock.position)
        - rock.radius
        - f64::from(hull.radius)
}

/// `(dialogKey, paramsDict)` for `OnGodmaShipEffect`'s `error`, with the one
/// argument every mining stop message takes: `modulename` (CCP's own form
/// for that name is `(UE_TYPEID, typeID)`, `bdl:926`) `[V]`.
fn module_error(key: &str, module_type_id: i64) -> PyValue {
    PyValue::Tuple(vec![
        crate::pyutil::bstr(key),
        PyValue::Dict(crate::pyutil::pypairs(vec![(
            "modulename",
            modules::type_arg(module_type_id),
        )])),
    ])
}

/// What `modules::activate` asks before a mining laser or a gas harvester
/// starts. The refusals are CCP's keys; which key for which case is **[A]**
/// (none is raised by code we can read):
///
/// - not a rock (or cloud) in this system ->
///   `CannotActivateModuleRequireAsteroidLock` (5020), "requires an active
///   target lock of an asteroid" `[V]` text;
/// - the wrong kind -- a laser on a gas cloud, a harvester on a rock
///   ([`cuts`]) -> `DeniedActivateTargetModuleDisallowed` (776);
/// - beyond `maxRange` (54) -> `TargetTooFar` (1632);
/// - no room for one unit -> `NotEnoughCargoSpace` (1336).
pub fn check_activation(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    module_id: i64,
    ship_type_id: i64,
    module: &modules::ModuleType,
    target_id: Option<i64>,
) -> Result<(), ServiceError> {
    let here = w.roster().get(character_id).copied();
    let rock = target_id
        .and_then(|t| w.rocks().get(t).copied())
        .filter(|r| here.is_some_and(|p| p.solar_system_id == r.solar_system_id));
    let Some(rock) = rock else {
        return Err(modules::user_error(
            "CannotActivateModuleRequireAsteroidLock",
            vec![("moduleTypeID", modules::type_arg(module.type_id))],
        ));
    };
    let effect_id = module
        .effect
        .map_or(modules::EFFECT_MINING_LASER, |e| e.effect_id);
    if !cuts(effect_id, is_gas(w.content(), rock.type_id)) {
        return Err(modules::user_error(
            "DeniedActivateTargetModuleDisallowed",
            vec![("moduleTypeID", modules::type_arg(module.type_id))],
        ));
    }
    w.advance_character(character_id);
    let position = w
        .roster()
        .get(character_id)
        .map_or([0.0; 3], |p| p.motion.position);
    let gap = gap_to(w.content(), position, ship_type_id, &rock);
    let range = reach(w, ship_id, module_id, module.type_id);
    if gap > range {
        let ore = crate::item::find_in(w.content(), rock.type_id).map_or("asteroid", |t| t.name);
        return Err(modules::user_error(
            "TargetTooFar",
            crate::module_range::too_far_pairs(module.type_id, gap, ore),
        ));
    }
    let ore = crate::item::find_in(w.content(), rock.type_id).copied();
    let unit = ore.map_or(0.0, |t| t.volume);
    // The mining hold counts when it takes the ore (GAPS-B).
    let (free, room) = match ore {
        Some(t) => (
            crate::mining_hold::free_for(w, ship_id, ship_type_id, &t),
            crate::mining_hold::room_for(w, ship_id, ship_type_id, &t).total(),
        ),
        None => {
            let free = cargo_free(w, ship_id, ship_type_id);
            (free, units_in(free, unit))
        }
    };
    if room < 1 {
        return Err(modules::user_error(
            "NotEnoughCargoSpace",
            vec![
                ("available", PyValue::Real(free)),
                ("volume", PyValue::Real(unit)),
            ],
        ));
    }
    Ok(())
}

/// **The world clock's hook**, after `modules::tick`: every cycle that ended
/// since the last tick, mined.
///
/// Drains `Modules::take_completed` and nothing else -- combat's
/// `take_cycles` belongs to `combat::tick` -- and drains it whole, so the
/// queue cannot grow with the completions of guns and repairers that are not
/// this module's business.
///
/// **The queue is shared** (HK2): the completions go to
/// `crate::hacking::on_completed` first -- an analyzer's first cycle ending
/// opens its game -- and then to mining, and each takes only its own
/// effects.
pub fn tick(w: &mut Registry, _now: i64) {
    let done = w.modules_mut().take_completed();
    crate::hacking::on_completed(w, &done);
    crate::salvage::on_completed(w, &done);
    crate::ship_scan::on_completed(w, &done);
    on_completed(w, done);
}

/// Mine every completed mining cycle, oldest first; ignore the rest.
pub fn on_completed(w: &mut Registry, done: Vec<Cycle>) {
    let mine: Vec<Cycle> = done
        .into_iter()
        .filter(|c| is_mining(c.effect_id))
        .collect();
    // **A system with a tethered pair is carried to now once per batch**
    // (PERF-STAGES): advancing one ship of such a system advances all of it
    // (`Registry::advance_character`), so a belt with fifty miners ending
    // their cycles in one tick advanced the whole belt fifty times. The
    // range test of each cycle then reads positions up to the batch's own
    // length old (milliseconds), and every cycle of the batch is judged at
    // the same instant, which is what `advance_system` is for.
    let mut fresh = std::collections::BTreeSet::new();
    for c in &mine {
        let Some(system) = w.roster().get(c.character_id).map(|p| p.solar_system_id) else {
            continue;
        };
        if !fresh.contains(&system) && w.roster().has_pair(system) {
            w.advance_system(system);
            fresh.insert(system);
        }
    }
    for c in mine {
        cycle(w, &c, &fresh);
    }
}

/// One completed mining cycle: a laser's on a rock, a harvester's on a
/// cloud.
fn cycle(w: &mut Registry, c: &Cycle, fresh: &std::collections::BTreeSet<i64>) {
    let Some(rock_id) = c.target_id else { return };
    let Some(here) = w.roster().get(c.character_id).copied() else {
        return;
    };
    if here.ship_id != c.ship_id {
        return;
    }
    // A rock mined out earlier in this batch is already gone, and its
    // handler has stopped every laser on it.
    let Some(rock) = w.rocks().get(rock_id).copied() else {
        return;
    };
    if rock.solar_system_id != here.solar_system_id {
        return;
    }
    let Some(ship_type_id) = w.character(c.character_id).map(|ch| ch.ship_type_id) else {
        return;
    };
    let take = Take::of_module(w, c.ship_id, c.module_id, c.type_id);
    let Some(ore) = crate::item::find_in(w.content(), rock.type_id).copied() else {
        crate::outln!(
            "[mining] rock {rock_id} is type {}, which the item table does not know; no ore",
            rock.type_id
        );
        modules::stop(w, c.module_id, c.at, None);
        return;
    };
    if !cuts(c.effect_id, ore.group_id == GROUP_HARVESTABLE_CLOUD) {
        // `check_activation` refused this pairing; a cycle that has one
        // anyway (a module swapped under it) cuts nothing.
        modules::stop(w, c.module_id, c.at, None);
        return;
    }
    let range = reach(w, c.ship_id, c.module_id, c.type_id);
    if !fresh.contains(&here.solar_system_id) {
        w.advance_character(c.character_id);
    }
    let position = w
        .roster()
        .get(c.character_id)
        .map_or(here.motion.position, |p| p.motion.position);
    let gap = gap_to(w.content(), position, ship_type_id, &rock);
    if gap > range {
        // **[A]** a cycle that ends out of range cuts nothing, and the laser
        // stops with the same dialog an out-of-range click gets.
        modules::stop(
            w,
            c.module_id,
            c.at,
            Some(PyValue::Tuple(vec![
                crate::pyutil::bstr("TargetTooFar"),
                PyValue::Dict(crate::pyutil::pypairs(crate::module_range::too_far_pairs(
                    c.type_id, gap, ore.name,
                ))),
            ])),
        );
        return;
    }
    match cut(
        w,
        c.character_id,
        c.ship_id,
        ship_type_id,
        &rock,
        &ore,
        &take,
        c.module_id,
        c.at,
    ) {
        // The hold filled from elsewhere mid-cycle: the load is lost, which
        // is what this key's own text says.
        Cut::Full => {
            modules::stop(
                w,
                c.module_id,
                c.at,
                Some(module_error(
                    "MiningDronesDeactivatedCargoHoldFull",
                    c.type_id,
                )),
            );
        }
        Cut::Mined { full: true, .. } => {
            modules::stop(
                w,
                c.module_id,
                c.at,
                Some(module_error(
                    "MiningDronesDeactivatedCargoHoldNowFull",
                    c.type_id,
                )),
            );
        }
        Cut::Mined { full: false, .. } | Cut::Nothing => {}
    }
}

/// What one cut did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// `units` moved from the rock to the hold; `full`: the hold has no room
    /// for another unit now.
    Mined { units: i64, full: bool },
    /// No room for one unit: nothing cut.
    Full,
    /// Nothing cut (the cycle cut less than a unit, or the store refused).
    Nothing,
}

/// **One cycle's ore, rock to hold** -- the laser's, the harvester's and the
/// drone's alike: [`roll`]'s units (the client's own `MiningResult`), cut to
/// what the rock holds and what `ship_id`'s holds (owned by `owner`) can
/// take: its mining hold first when it takes the ore, then its cargo hold
/// (GAPS-B, `crate::mining_hold::room_for`). The store first, then the rock,
/// so ore is conserved: the hold gains `normal + crit`, the rock loses
/// `normal + wasted` (a crit is not taken from the rock, `[V]` F17); a rock
/// mined out leaves the field ([`depleted`]), one that shrank is resized.
#[allow(clippy::too_many_arguments)]
fn cut(
    w: &mut Registry,
    owner: i64,
    ship_id: i64,
    ship_type_id: i64,
    rock: &crate::asteroid::Rock,
    ore: &crate::item::ItemType,
    take: &Take,
    source_id: i64,
    at: i64,
) -> Cut {
    // The mining hold first, when the ship has one that takes the ore, then
    // the cargo hold (GAPS-B, `crate::mining_hold`).
    let room = crate::mining_hold::room_for(w, ship_id, ship_type_id, ore);
    let fits = room.total();
    if fits < 1 {
        return Cut::Full;
    }
    let flag =
        |id| crate::typedata::attribute_in(w.content(), rock.type_id, id).is_some_and(|v| v != 0.0);
    let mut stream = crate::seed::stream(
        w.world().seed,
        crate::seed::Kind::Loot,
        &[
            STREAM_TAG,
            rock.id as u64,
            source_id as u64,
            owner as u64,
            at as u64,
        ],
    );
    let got = roll(
        &mut stream,
        take,
        ore.volume,
        rock.quantity,
        fits,
        flag(ATTR_IGNORE_CRITS),
        flag(ATTR_IGNORE_WASTE),
        w.ext::<ForcedSlot>().0,
    );
    let units = got.normal + got.crit;
    if units < 1 {
        return Cut::Nothing;
    }
    let (to_hold, to_cargo) = room.split(units);
    // **The store first, then the rock**: a plan the store refuses must not
    // have cost the rock anything.
    let mut edits = Vec::new();
    for (flag, n) in [
        (crate::mining_hold::FLAG_GENERAL_MINING_HOLD, to_hold),
        (crate::fitting::FLAG_CARGO, to_cargo),
    ] {
        if n < 1 {
            continue;
        }
        let slot = crate::item::Slot::new(owner, ship_id, flag);
        let stack = w
            .store()
            .at(slot)
            .filter(|i| i.type_id == rock.type_id && !i.singleton)
            .map(|i| i.item_id)
            .min();
        edits.push(match stack {
            Some(item_id) => crate::item::Edit::Grow {
                item_id,
                quantity: n,
            },
            None => crate::item::Edit::Create {
                type_id: rock.type_id,
                quantity: n,
                to: slot,
            },
        });
    }
    if let Err(e) = on_clock(w, |w| w.apply_items(&edits)) {
        crate::outln!(
            "[mining] the store refused {units} of type {}: {e}",
            rock.type_id
        );
        return Cut::Nothing;
    }
    // The rock loses the yield and the residue; the crit comes from nowhere.
    let off_rock = got.normal + got.wasted;
    let Some(mined) = w.rocks_mut().mine(rock.id, off_rock) else {
        return Cut::Nothing;
    };
    debug_assert_eq!(mined.taken, off_rock, "roll cut it to the rock's quantity");
    crate::sov_index::on_mined(w, rock.solar_system_id, off_rock as f64 * ore.volume);
    if mined.depleted() {
        depleted(w, rock, at);
    } else if mined.shrank() {
        let to = in_system(w, rock.solar_system_id);
        let stamp = w.next_destiny_stamp();
        if let Some(p) = RockRadius::new(to, stamp, rock.id, mined.radius) {
            w.schedule(Duration::ZERO, p);
        }
    }
    announce(w, owner, source_id, rock, got, at);
    let ev = crate::mining_hooks::MinedEvent {
        miner: owner,
        ship_id,
        ship_type_id,
        rock_id: rock.id,
        rock_type: rock.type_id,
        system: rock.solar_system_id,
        normal: got.normal,
        crit: got.crit,
        at,
        source_id,
    };
    let rows = &crate::module::plan().mined;
    if !rows.is_empty() {
        on_clock(w, |w| {
            for row in rows {
                (row.run)(w, &ev);
            }
        });
    }
    Cut::Mined {
        units,
        full: crate::mining_hold::room_for(w, ship_id, ship_type_id, ore).total() < 1,
    }
}

/// **What one cut tells the world** (MINE-UI): the miner's `OnOreMined`
/// (the client's mining log line and its scanned rock's new quantity),
/// `OnOreMinedByOther` to the others who hold the rock, and the day's row in
/// the Mining Ledger. Laser, harvester and drone alike: all come through
/// [`cut`], once per cycle, so the cost is one push to the miner plus one
/// to each pilot whose park holds the rock (a handful on a grid), never
/// one per pilot in the system.
fn announce(
    w: &mut Registry,
    miner: i64,
    source_id: i64,
    rock: &crate::asteroid::Rock,
    got: Yield,
    at: i64,
) {
    let corp = w.character(miner).map_or(0, |c| c.corporation_id);
    crate::mining_ledger::record(
        w.deltas_mut(),
        miner,
        corp,
        rock.solar_system_id,
        rock.type_id,
        got.normal + got.crit,
        got.wasted,
        at,
    );
    // **A bot has no mining window** (and 250 of them mine at once): its own
    // `OnOreMined`, and an `OnOreMinedByOther` to a bot, would be tens of
    // thousands of frames a cycle round that nobody reads. A human on grid
    // still hears a bot's bite (their scanned rock shrinks).
    let is_bot = |w: &Registry, id: i64| w.character(id).is_some_and(|c| c.is_bot);
    if !is_bot(w, miner) {
        w.schedule(
            Duration::ZERO,
            OreMined {
                to: miner,
                rock_id: rock.id,
                ore_type: rock.type_id,
                // `quantity_added` is what the hold took, crit included [I];
                // `quantity_removed` is the yield (F17).
                quantity_added: got.normal + got.crit,
                quantity_removed: got.normal,
                wasted: got.wasted,
                crit: got.crit,
                source_id,
            },
        );
    }
    // Only a pilot who has the rock on grid can have scanned it, and the
    // client ignores the rest (`is_quantity_known`): so the audience is the
    // pilots whose park holds it, or whose park is not modelled.
    let parks = w.ext::<crate::grid::Parks>();
    let to: Vec<i64> = in_system(w, rock.solar_system_id)
        .into_iter()
        .filter(|c| *c != miner && parks.of(*c).is_none_or(|p| p.contains(&rock.id)))
        .filter(|c| !is_bot(w, *c))
        .collect();
    if !to.is_empty() {
        w.schedule(
            Duration::ZERO,
            OreMinedByOther {
                to,
                rock_id: rock.id,
                // What left the rock (the crit did not).
                quantity: got.normal + got.wasted,
            },
        );
    }
}

/// **One offline bite** (E1, `crate::econ_agents`): `units` of `rock`, taken
/// by a logged-off agent's settlement in a system whose rocks are laid out.
/// The rock gives the units up exactly as [`cut`] does -- the field first,
/// then the sov index's mined volume (`volume` m3 a unit), then whatever the
/// pilots in the system see: a rock mined out is removed from their parks
/// and every laser on it stopped, a shrunk one resized. The ore itself is the
/// caller's to create (the agent's hangar, not a hold). Answers the units
/// taken, `0` for a rock no longer there.
pub fn settle_bite(
    w: &mut Registry,
    rock: &crate::asteroid::Rock,
    units: i64,
    volume: f64,
    at: i64,
) -> i64 {
    let Some(mined) = w.rocks_mut().mine(rock.id, units) else {
        return 0;
    };
    crate::sov_index::on_mined(w, rock.solar_system_id, mined.taken as f64 * volume);
    if mined.depleted() {
        depleted(w, rock, at);
    } else if mined.shrank() {
        let to = in_system(w, rock.solar_system_id);
        let stamp = w.next_destiny_stamp();
        if let Some(p) = RockRadius::new(to, stamp, rock.id, mined.radius) {
            w.schedule(Duration::ZERO, p);
        }
    }
    mined.taken
}

/// What one mining drone's cycle did ([`drone_cycle`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DroneCut {
    /// `units` of the rock's ore reached the hold; `full`: no room for
    /// another unit now.
    Mined { units: i64, full: bool },
    /// The hold had no room for one unit: nothing cut.
    Full,
    /// The rock is not in the field, or not in the owner's system.
    Gone,
    /// Nothing to cut for: the owner is not in space in `ship_id`, the ore
    /// is unknown, or the rock is a gas cloud ([`cuts`]). Or the cycle cut
    /// less than a unit.
    Nothing,
}

/// **One completed mining-drone cycle** (SOC-B10), from `crate::drones`'
/// clock: `drone_type_id`'s `miningAmount` ([`ATTR_MINING_AMOUNT`]) of
/// `rock_id`'s ore into the cargo hold of `ship_id`, the drone's controller,
/// owned by `owner_id` ([`cut`]). The range is the drone's business: it
/// calls this only within its `maxRange` of the rock. **[A]** the ore
/// arrives at the end of the cycle, in the ship's hold, wherever the drone
/// is -- no trip back with the load.
pub fn drone_cycle(
    w: &mut Registry,
    owner_id: i64,
    ship_id: i64,
    drone_type_id: i64,
    rock_id: i64,
    at: i64,
) -> DroneCut {
    let Some(here) = w.roster().get(owner_id).copied() else {
        return DroneCut::Nothing;
    };
    if here.ship_id != ship_id {
        return DroneCut::Nothing;
    }
    let Some(rock) = w
        .rocks()
        .get(rock_id)
        .copied()
        .filter(|r| r.solar_system_id == here.solar_system_id)
    else {
        return DroneCut::Gone;
    };
    let Some(ship_type_id) = w.character(owner_id).map(|ch| ch.ship_type_id) else {
        return DroneCut::Nothing;
    };
    let Some(ore) = crate::item::find_in(w.content(), rock.type_id).copied() else {
        return DroneCut::Nothing;
    };
    if !cuts(EFFECT_MINING, ore.group_id == GROUP_HARVESTABLE_CLOUD) {
        return DroneCut::Nothing;
    }
    // A drone's numbers are its type's: the dogma sheet is a ship's.
    let take = Take::of_type(w.content(), drone_type_id);
    // **[A]** `moduleItemID` is 0 for a drone: `drone_cycle` is not told the
    // drone's item id. Falsified by a client log line that needs it.
    match cut(
        w,
        owner_id,
        ship_id,
        ship_type_id,
        &rock,
        &ore,
        &take,
        0,
        at,
    ) {
        Cut::Mined { units, full } => DroneCut::Mined { units, full },
        Cut::Full => DroneCut::Full,
        Cut::Nothing => DroneCut::Nothing,
    }
}

/// A rock has given up its last unit: stop every laser on it, drop it from
/// every park, release every lock on it.
///
/// In that order, so a pilot's laser button is told why it stopped before
/// the target box closes. The lock's reason is `Exploding` **[A]** -- the
/// one reason the client reads, which plays the target box's disappear
/// animation rather than closing it flat (`targeting::Reason`).
fn depleted(w: &mut Registry, rock: &crate::asteroid::Rock, at: i64) {
    for r in w.modules().running_against(rock.id) {
        if is_mining(r.effect.effect_id) {
            modules::stop(
                w,
                r.module_id,
                at,
                Some(module_error("MiningItemDepleted", r.type_id)),
            );
        }
    }
    let to = in_system(w, rock.solar_system_id);
    let stamp = w.next_destiny_stamp();
    if let Some(p) = crate::destiny::BallsRemoved::new(to, stamp, vec![rock.id]) {
        w.schedule(Duration::ZERO, p);
    }
    for locker_ship in w.locks().targeters_of(rock.id) {
        let Some(locker) = w.roster().by_ship(locker_ship).map(|p| p.character_id) else {
            w.locks_mut().remove(locker_ship, rock.id);
            continue;
        };
        on_clock(w, |w| {
            crate::targeting::unlock(w, locker, rock.id, crate::targeting::Reason::Exploding)
        });
    }
}

/// Every character in space in a system: whose park holds a rock there.
fn in_system(w: &Registry, solar_system_id: i64) -> Vec<i64> {
    w.roster()
        .in_system(solar_system_id)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

/// Run `f` and send what it put on `pending` from the clock instead.
///
/// `apply_items` and `targeting::unlock` both answer through `pending`,
/// which only a payload turn drains. Whatever was on `pending` before is
/// someone else's turn's and goes back untouched; what `f` added is ours and
/// is scheduled, keeping its own audience (`apply_items` already addresses
/// item pushes to their owners).
fn on_clock<R>(w: &mut Registry, f: impl FnOnce(&mut Registry) -> R) -> R {
    let before = w.take_pending();
    let out = f(w);
    let ours = w.take_pending();
    for (after, p) in before {
        w.push_later(after, Addressed { to: None, inner: p });
    }
    for (_, p) in ours {
        w.schedule(Duration::ZERO, Addressed { to: None, inner: p });
    }
    out
}

/// `SetBallRadius(ballID, radius)` in a `DoDestinyUpdate`: a mined rock's new
/// size.
///
/// Two arguments, `(long, double)` `[V]` (`docs/lanes/movement.md`, the
/// reference recording `76-call-SetBallRadius.bin`). It is in michelle's
/// `scatterEvents` (`michelle.pyc:1038`) `[V]`, so the native ball takes the
/// radius -- brackets, range readouts, collision -- and then
/// `spaceMgr.OnBallparkCall` calls the space object's `SetRadius` **if it has
/// one** (`spaceMgr.pyc:532-540`) `[V]`. `Asteroid` does not, so **the model
/// keeps the scale it was assembled with** until the ball is re-added (the
/// next undock or warp in): what is drawn lags, what is measured does not.
#[derive(Debug, Clone, PartialEq)]
pub struct RockRadius {
    to: Vec<i64>,
    stamp: u32,
    rock_id: i64,
    radius: f64,
}

impl RockRadius {
    /// `None` for nobody to tell or a radius the native would discard.
    pub fn new(to: Vec<i64>, stamp: u32, rock_id: i64, radius: f64) -> Option<Self> {
        if to.is_empty() || !radius.is_finite() || radius <= 0.0 {
            return None;
        }
        Some(RockRadius {
            to,
            stamp,
            rock_id,
            radius,
        })
    }
}

impl Push for RockRadius {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("SetBallRadius"),
                PyValue::Tuple(vec![PyValue::Int(self.rock_id), PyValue::Real(self.radius)]),
            ]),
        ]);
        (
            "DoDestinyUpdate",
            vec![PyValue::List(vec![entry]), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.rock_id])
    }
}

/// `attributeMiningScannerUpgrade` **[V]** (`dogma/const.py:683`): the
/// survey-scanner tier the client tests (`mining_util.py:24-31`).
const ATTR_SCANNER_TIER: i64 = 5979;

/// The reach of a survey scan: the client's own tier-1 `SCAN_EFFECT_DISTANCE`
/// (`mining_const.py:11`) **[A]**, reused for tier 2. Falsified by a ship
/// whose scan should show rocks past it.
pub const SCAN_REACH: f64 = 250_000.0;

/// `miningScanMgr.perform_scan()`: every rock of the caller's system within
/// [`SCAN_REACH`] of the ship, nearest first, as `(rock, type, quantity)`.
/// `None` is the client's cue to fall back to a tier-1 scan: the caller is
/// not in space, or the ship's scanner tier (5979) is below 2.
pub fn survey(w: &Registry, character_id: i64) -> Option<Vec<(i64, i64, i64)>> {
    let here = w.roster().get(character_id)?;
    let ship_type_id = w.character(character_id)?.ship_type_id;
    let tier = crate::typedata::attribute_in(w.content(), ship_type_id, ATTR_SCANNER_TIER)?;
    if tier < 2.0 {
        return None;
    }
    let mut found: Vec<(f64, i64, i64, i64)> = w
        .rocks()
        .in_system(here.solar_system_id)
        .map(|r| {
            (
                gap_to(w.content(), here.motion.position, ship_type_id, r),
                r,
            )
        })
        .filter(|(gap, _)| *gap <= SCAN_REACH)
        .map(|(gap, r)| (gap, r.id, r.type_id, r.quantity))
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    Some(found.into_iter().map(|(_, id, t, q)| (id, t, q)).collect())
}

/// `OnOreMined(dict)` to the miner: the client's "mined N units" log line and
/// its survey-scan quantity for the rock (`oreMinedNotificationAdapter.py`,
/// `asteroid_scanning_service.py`) **[V]**. `quantity_removed` is what left
/// the rock, residue not counted; the rock loses `removed + wasted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OreMined {
    to: i64,
    rock_id: i64,
    ore_type: i64,
    quantity_added: i64,
    quantity_removed: i64,
    wasted: i64,
    crit: i64,
    source_id: i64,
}

impl Push for OreMined {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let n = PyValue::Int;
        (
            "OnOreMined",
            vec![crate::pyutil::pydict(vec![
                ("itemID", n(self.rock_id)),
                ("oreType", n(self.ore_type)),
                ("quantity_added", n(self.quantity_added)),
                ("quantity_removed", n(self.quantity_removed)),
                ("amountWasted", n(self.wasted)),
                ("amountCritBonus", n(self.crit)),
                ("hasRewards", PyValue::Bool(false)),
                ("moduleItemID", n(self.source_id)),
            ])],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnOreMinedByOther(asteroidID, quantity)`: two positional arguments that
/// lower a scanned rock for the others on grid **[V]**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OreMinedByOther {
    to: Vec<i64>,
    rock_id: i64,
    quantity: i64,
}

impl Push for OreMinedByOther {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnOreMinedByOther",
            vec![PyValue::Int(self.rock_id), PyValue::Int(self.quantity)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.rock_id])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asteroid::{Field, Law, Rock};

    const ALICE: i64 = 1_000_001;
    const MERLIN: i64 = 100_000_060;
    const MINER: i64 = 100_000_061;
    const GUN: i64 = 100_000_062;
    /// `flagHiSlot0` [V].
    const HI_SLOT_0: i64 = 27;
    const ROCK: i64 = crate::asteroid::MIN_ROCK_ID;
    const VELDSPAR: i64 = 1230;
    /// Blue ticks; any fixed instant.
    const T0: i64 = 133_000_000_000_000_000;

    /// A fixture law, not a claim about any ore: tests below assert that
    /// mining follows *the rock's own* law, whatever it is.
    const LAW: Law = Law {
        growth: 0.5,
        unit_size: 90.0,
        max_radius: 16_255.0,
    };

    fn rock(quantity: i64) -> Rock {
        Rock {
            id: ROCK,
            type_id: VELDSPAR,
            belt_id: 40_000_003,
            solar_system_id: 30_000_001,
            quantity,
            radius: LAW.radius(quantity),
            position: [80_000.0, 0.0, 0.0],
            law: LAW,
        }
    }

    /// Alice in space in a Merlin (603) with a Miner I in high slot 0 and
    /// `extra` fitted beside it, stopped 2 km off one rock's surface, the
    /// rock locked.
    fn alice_mining(quantity: i64, extra: &[(i64, i64, i64)]) -> Registry {
        alice_with(None, vec![rock(quantity)], extra)
    }

    /// [`alice_mining`]'s world on `content` (the fixture's when `None`),
    /// with `rocks` in the field: Alice parks 2 km off the first one's
    /// surface and holds a lock on every one of them.
    fn alice_with(
        content: Option<crate::content::Content>,
        rocks: Vec<Rock>,
        extra: &[(i64, i64, i64)],
    ) -> Registry {
        alice_flying(content, (603, 483), rocks, extra)
    }

    /// [`alice_with`] in a `hull` with a `laser` (item ids still
    /// [`MERLIN`] and [`MINER`]).
    fn alice_flying(
        content: Option<crate::content::Content>,
        (hull, laser): (i64, i64),
        rocks: Vec<Rock>,
        extra: &[(i64, i64, i64)],
    ) -> Registry {
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ..crate::model::Character::default()
        };
        let first = rocks[0];
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice],
            rocks: Field::from_rocks(rocks.iter().copied()),
            ..crate::model::SandboxConfig::default()
        });
        if let Some(c) = content {
            reg = reg.with_content(std::sync::Arc::new(c));
        }
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let merlin = *crate::item::find_in(reg.content(), hull).expect("the hull");
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: MERLIN,
            type_id: hull,
            owner_id: ALICE,
            location_id: station,
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 1,
            group_id: merlin.group_id,
            category_id: merlin.category_id,
            singleton: true,
        });
        let mut fit = vec![(MINER, laser, HI_SLOT_0)];
        fit.extend_from_slice(extra);
        for (item_id, type_id, flag_id) in fit {
            let t = crate::item::find_in(reg.content(), type_id).expect("spawnable module");
            items.push(crate::item::Item {
                item_id,
                type_id,
                owner_id: ALICE,
                location_id: MERLIN,
                flag_id,
                quantity: 1,
                group_id: t.group_id,
                category_id: t.category_id,
                singleton: true,
            });
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: MERLIN + 100,
            ..crate::store::Snapshot::default()
        });
        let c = reg.character_mut(ALICE).expect("Alice");
        c.ship_id = MERLIN;
        c.ship_type_id = hull;
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        park(
            &mut reg,
            [first.position[0] + first.radius + 2_000.0, 0.0, 0.0],
        );
        for r in &rocks {
            reg.locks_mut().add(MERLIN, r.id);
        }
        let _ = reg.take_pending();
        let _ = reg.take_due(far());
        reg
    }

    fn park(reg: &mut Registry, at: [f64; 3]) {
        let p = reg.roster_mut().get_mut(ALICE).expect("in space");
        p.motion = crate::space_state::Motion::arrived(at, [0.0; 3]);
        p.since = crate::space_state::now_seconds();
    }

    fn far() -> std::time::Instant {
        std::time::Instant::now() + Duration::from_secs(3_600)
    }

    /// Miner I's cycle, from the fixture content every test registry holds.
    fn period() -> i64 {
        let content = crate::content::Content::fixture();
        modules::find_in(&content, 483)
            .expect("Miner I")
            .duration_ms()
            * modules::MSEC
    }

    fn start(reg: &mut Registry) {
        modules::activate(reg, Some(ALICE), MINER, "miningLaser", Some(ROCK), 1000, T0)
            .expect("the laser starts");
    }

    /// The world tick, as `push::spawn_world_ticker` runs it, at `at`.
    fn clock(reg: &mut Registry, at: i64) {
        modules::tick(reg, at);
        crate::combat::tick(reg, at);
        tick(reg, at);
    }

    fn cargo(reg: &Registry) -> Vec<crate::item::Item> {
        reg.store()
            .at(crate::item::Slot::new(
                ALICE,
                MERLIN,
                crate::fitting::FLAG_CARGO,
            ))
            .cloned()
            .collect()
    }

    fn ore_in_cargo(reg: &Registry) -> i64 {
        cargo(reg)
            .iter()
            .filter(|i| i.type_id == VELDSPAR)
            .map(|i| i.quantity)
            .sum()
    }

    /// One push as sent: `(event, args, audience)`.
    type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

    fn sent(reg: &mut Registry) -> Vec<Sent> {
        reg.take_due(far())
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (name, args, p.audience())
            })
            .collect()
    }

    /// The destiny calls in the `DoDestinyUpdate`s sent: `(name, args)`.
    fn destiny_calls(sent: &[Sent]) -> Vec<(String, Vec<PyValue>)> {
        let mut out = Vec::new();
        for (name, args, _) in sent {
            if *name != "DoDestinyUpdate" {
                continue;
            }
            let Some(PyValue::List(entries)) = args.first() else {
                continue;
            };
            for e in entries {
                let PyValue::Tuple(stamped) = e else { continue };
                let Some(PyValue::Tuple(call)) = stamped.get(1) else {
                    continue;
                };
                let (Some(PyValue::Bytes(n)), Some(PyValue::Tuple(a))) =
                    (call.first(), call.get(1))
                else {
                    continue;
                };
                out.push((String::from_utf8_lossy(n).into_owned(), a.clone()));
            }
        }
        out
    }

    /// **One completed cycle moves ore from the rock to the hold, and no more
    /// than the laser cuts.** The stack is the rock's ore in the pilot's
    /// cargo (flag 5), the rock is down by exactly what arrived, its radius
    /// is its law's for what is left, and both the owner's inventory push and
    /// the rock's new radius go out on the clock.
    #[test]
    fn a_completed_cycle_moves_ore_from_the_rock_to_the_hold() {
        let q0 = 140_000;
        let mut reg = alice_mining(q0, &[]);
        start(&mut reg);
        let _ = reg.take_pending();
        clock(&mut reg, T0 + period());
        let got = ore_in_cargo(&reg);
        assert!(got > 0, "a cycle yields ore");
        let r = *reg.rocks().get(ROCK).expect("still there");
        assert_eq!(r.quantity, q0 - got, "ore is conserved");
        assert_eq!(r.radius, LAW.radius(r.quantity), "the rock's own law");
        let unit = crate::item::find_in(reg.content(), VELDSPAR)
            .expect("Veldspar")
            .volume;
        let cut = modules::find_in(reg.content(), 483)
            .expect("Miner I")
            .mining_amount();
        assert!(got as f64 * unit <= cut + 1e-9, "no more than miningAmount");
        assert!(
            cargo(&reg)
                .iter()
                .all(|i| i.owner_id == ALICE && !i.singleton)
        );

        let out = sent(&mut reg);
        assert!(
            out.iter()
                .any(|(n, _, to)| *n == "OnItemChange" && to.as_deref() == Some(&[ALICE][..])),
            "the cargo row reaches its owner from the clock: {out:?}"
        );
        assert!(reg.take_pending().is_empty(), "nothing waits for a turn");
        let radius = destiny_calls(&out)
            .into_iter()
            .find(|(n, _)| n == "SetBallRadius")
            .expect("the rock is resized");
        assert_eq!(radius.1, vec![PyValue::Int(ROCK), PyValue::Real(r.radius)]);
    }

    /// Two cycles fill one stack, and everything that left the rock is in it.
    #[test]
    fn cycles_grow_one_stack_and_ore_is_conserved() {
        let q0 = 140_000;
        let mut reg = alice_mining(q0, &[]);
        start(&mut reg);
        clock(&mut reg, T0 + period());
        clock(&mut reg, T0 + 2 * period());
        let stacks: Vec<_> = cargo(&reg)
            .into_iter()
            .filter(|i| i.type_id == VELDSPAR)
            .collect();
        assert_eq!(stacks.len(), 1, "one stack, grown");
        let left = reg.rocks().get(ROCK).expect("still there").quantity;
        assert_eq!(stacks[0].quantity, q0 - left);
        assert!(stacks[0].quantity > ore_in_one_cycle(), "two cycles' worth");
    }

    fn ore_in_one_cycle() -> i64 {
        let mut reg = alice_mining(140_000, &[]);
        start(&mut reg);
        clock(&mut reg, T0 + period());
        ore_in_cargo(&reg)
    }

    /// **A rock mined out is gone**: every unit it held is in the hold, it is
    /// out of the field, every park is told `RemoveBalls`, the laser stops
    /// with a reason in its `error` slot, and the lock is released.
    #[test]
    fn a_rock_mined_out_leaves_the_field_and_stops_the_laser() {
        let per = ore_in_one_cycle();
        let q0 = per + per / 2;
        let mut reg = alice_mining(q0, &[]);
        start(&mut reg);
        let mut out = Vec::new();
        for n in 1..=4 {
            clock(&mut reg, T0 + n * period());
            out.extend(sent(&mut reg));
        }
        assert_eq!(ore_in_cargo(&reg), q0, "every unit, and no more");
        assert!(reg.rocks().get(ROCK).is_none());
        assert!(reg.modules().running(MINER).is_none());
        assert!(!reg.locks().targets_of(MERLIN).contains(&ROCK));
        assert!(
            destiny_calls(&out)
                .iter()
                .any(|(n, a)| n == "RemoveBalls"
                    && a == &vec![PyValue::List(vec![PyValue::Int(ROCK)])]),
            "{out:?}"
        );
        let stop = out
            .iter()
            .find(|(n, a, _)| {
                *n == "OnGodmaShipEffect" && a[0] == PyValue::Int(MINER) && a[3] == PyValue::Int(0)
            })
            .expect("the laser is told to stop");
        assert!(
            matches!(&stop.1[9], PyValue::Tuple(e) if e.len() == 2),
            "error is (key, dict): shipUI.pyc:1211 indexes [0] and [1]"
        );
        assert!(
            out.iter()
                .any(|(n, a, _)| *n == "OnTarget" && a.get(1) == Some(&PyValue::Int(ROCK))),
            "the target box closes"
        );
    }

    /// **The laser stops when the hold is full**, and the hold is never
    /// over capacity.
    #[test]
    fn the_laser_stops_when_the_hold_is_full() {
        let q0 = 140_000;
        let mut reg = alice_mining(q0, &[]);
        // Nearly full of Tritanium: room for a little Veldspar, not a cycle.
        let capacity = crate::item::find_in(reg.content(), 603)
            .expect("Merlin")
            .capacity;
        let trit = crate::item::find_in(reg.content(), 34)
            .expect("Tritanium")
            .volume;
        let unit = crate::item::find_in(reg.content(), VELDSPAR)
            .expect("Veldspar")
            .volume;
        let filler = units_in(capacity - 3.0 * unit, trit);
        reg.apply_items(&[crate::item::Edit::Create {
            type_id: 34,
            quantity: filler,
            to: crate::item::Slot::new(ALICE, MERLIN, crate::fitting::FLAG_CARGO),
        }])
        .expect("filled");
        let _ = reg.take_pending();
        start(&mut reg);
        clock(&mut reg, T0 + period());
        let got = ore_in_cargo(&reg);
        assert!(got > 0);
        assert_eq!(reg.rocks().get(ROCK).expect("rock").quantity, q0 - got);
        assert!(
            reg.store()
                .used_volume_in(reg.content(), MERLIN, crate::fitting::FLAG_CARGO)
                <= capacity + 1e-9
        );
        assert!(reg.modules().running(MINER).is_none(), "the laser stopped");
        clock(&mut reg, T0 + 2 * period());
        assert_eq!(ore_in_cargo(&reg), got, "and cut nothing more");
    }

    /// The three refusals leave nothing running: a target that is not a
    /// rock, a rock out of range, and a full hold.
    #[test]
    fn activation_wants_a_rock_in_range_and_room_in_the_hold() {
        // Not a rock (but locked).
        let mut reg = alice_mining(140_000, &[]);
        reg.locks_mut().add(MERLIN, 100_000_099);
        assert!(
            modules::activate(
                &mut reg,
                Some(ALICE),
                MINER,
                "miningLaser",
                Some(100_000_099),
                1000,
                T0
            )
            .is_err()
        );
        assert!(reg.modules().running(MINER).is_none());

        // Out of range: far beyond Miner I's maxRange from the surface.
        let mut reg = alice_mining(140_000, &[]);
        let range = modules::find_in(reg.content(), 483)
            .and_then(|t| t.attribute(54))
            .expect("maxRange");
        let r = rock(140_000);
        park(
            &mut reg,
            [r.position[0] + r.radius + 10.0 * range, 0.0, 0.0],
        );
        assert!(
            modules::activate(
                &mut reg,
                Some(ALICE),
                MINER,
                "miningLaser",
                Some(ROCK),
                1000,
                T0
            )
            .is_err()
        );
        assert!(reg.modules().running(MINER).is_none());

        // A full hold.
        let mut reg = alice_mining(140_000, &[]);
        let capacity = crate::item::find_in(reg.content(), 603)
            .expect("Merlin")
            .capacity;
        let trit = crate::item::find_in(reg.content(), 34)
            .expect("Tritanium")
            .volume;
        reg.apply_items(&[crate::item::Edit::Create {
            type_id: 34,
            quantity: units_in(capacity, trit),
            to: crate::item::Slot::new(ALICE, MERLIN, crate::fitting::FLAG_CARGO),
        }])
        .expect("filled");
        assert!(
            modules::activate(
                &mut reg,
                Some(ALICE),
                MINER,
                "miningLaser",
                Some(ROCK),
                1000,
                T0
            )
            .is_err()
        );
        assert!(reg.modules().running(MINER).is_none());
    }

    /// The `UserError` key an activation raised.
    fn key_of(r: Result<PyValue, crate::service::ServiceError>) -> String {
        let header = match r {
            Err(crate::service::ServiceError::Raise(PyValue::ObjectEx { header, .. })) => header,
            other => panic!("a raise, got {other:?}"),
        };
        let PyValue::Tuple(parts) = *header else {
            panic!("the exception header is (class, args)");
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError takes (msg, dict)");
        };
        match &args[0] {
            PyValue::Bytes(key) => String::from_utf8_lossy(key).into_owned(),
            other => panic!("the key is a str, got {other:?}"),
        }
    }

    /// Miner I's `maxRange`, from the fixture content.
    fn laser_range() -> f64 {
        modules::find_in(&crate::content::Content::fixture(), 483)
            .and_then(|t| t.attribute(54))
            .expect("maxRange")
    }

    /// **Out of range is `TargetTooFar` and nothing is mined**; inside it, a
    /// cycle mines. Both sides of the edge, measured surface to surface.
    #[test]
    fn a_laser_beyond_max_range_is_refused_with_target_too_far_and_mines_nothing() {
        let range = laser_range();
        let r = rock(140_000);
        let edge = r.position[0]
            + r.radius
            + f64::from(
                crate::destiny::hull::of_in(&crate::content::Content::fixture(), 603).radius,
            );

        let mut reg = alice_mining(140_000, &[]);
        park(&mut reg, [edge + range + 500.0, 0.0, 0.0]);
        let refused = modules::activate(
            &mut reg,
            Some(ALICE),
            MINER,
            "miningLaser",
            Some(ROCK),
            1000,
            T0,
        );
        assert_eq!(key_of(refused), "TargetTooFar");
        assert!(reg.modules().running(MINER).is_none());
        clock(&mut reg, T0 + 3 * period());
        assert_eq!(ore_in_cargo(&reg), 0, "nothing mined");
        assert_eq!(reg.rocks().get(ROCK).expect("rock").quantity, 140_000);

        let mut reg = alice_mining(140_000, &[]);
        park(&mut reg, [edge + range - 500.0, 0.0, 0.0]);
        start(&mut reg);
        clock(&mut reg, T0 + period());
        assert!(ore_in_cargo(&reg) > 0, "in range it mines");
    }

    /// **A rock that leaves reach mid-cycle stops the laser** with
    /// `TargetTooFar` on the stop push and cuts nothing that cycle.
    #[test]
    fn a_laser_whose_ship_drifts_out_of_range_mid_cycle_stops_and_cuts_nothing() {
        let range = laser_range();
        let mut reg = alice_mining(140_000, &[]);
        start(&mut reg);
        let _ = sent(&mut reg);
        let r = rock(140_000);
        park(
            &mut reg,
            [r.position[0] + r.radius + 10.0 * range, 0.0, 0.0],
        );
        clock(&mut reg, T0 + period());
        assert_eq!(ore_in_cargo(&reg), 0, "the cycle cut nothing");
        assert!(reg.modules().running(MINER).is_none(), "the laser stopped");
        let told = format!("{:?}", sent(&mut reg));
        let key = format!("{:?}", crate::pyutil::bstr("TargetTooFar"));
        assert!(told.contains(&key), "the stop names its reason");
    }

    /// **Neither consumer takes the other's cycles.** A gun and a laser
    /// cycling together: mining's tick leaves every *started* cycle on
    /// combat's queue, combat's tick leaves every *completed* one on
    /// mining's, and combat does nothing with a laser aimed at a rock.
    #[test]
    fn mining_and_combat_do_not_take_each_others_cycles() {
        // The civilian pulse laser, the only gun combat fires with no charge.
        let mut reg = alice_mining(140_000, &[(GUN, 3634, HI_SLOT_0 + 1)]);
        let victim = 100_000_099;
        reg.locks_mut().add(MERLIN, victim);
        start(&mut reg);
        modules::activate(
            &mut reg,
            Some(ALICE),
            GUN,
            "targetAttack",
            Some(victim),
            1000,
            T0,
        )
        .expect("the gun starts");
        let gun_period = modules::find_in(reg.content(), 3634)
            .expect("gun")
            .duration_ms()
            * modules::MSEC;
        let until = T0 + period().max(gun_period);

        modules::tick(&mut reg, until);
        tick(&mut reg, until);
        assert!(ore_in_cargo(&reg) > 0, "mining took its completions");
        let started = reg.modules_mut().take_cycles();
        assert!(
            started.iter().any(|c| c.module_id == GUN),
            "the gun's starts are combat's"
        );
        assert!(
            started.iter().any(|c| c.module_id == MINER),
            "mining took no start"
        );

        let later = until + period();
        modules::tick(&mut reg, later);
        crate::combat::on_cycles(&mut reg, started, later);
        assert!(reg.combat().ship(ROCK).is_none(), "a rock takes no volley");
        crate::combat::tick(&mut reg, later);
        let completed = reg.modules_mut().take_completed();
        assert!(
            completed.iter().any(|c| c.module_id == MINER),
            "combat took no completion"
        );
        assert!(completed.iter().any(|c| c.module_id == GUN));
    }

    /// `units_in` never yields more volume than it was given, nor leaves a
    /// whole unit behind.
    #[test]
    fn units_never_exceed_the_volume_cut() {
        for (v, u) in [
            (10.0, 0.1),
            (10.0, 0.15),
            (10.0, 0.3),
            (10.0, 16.0),
            (150.0, 0.35),
            (0.0, 0.1),
        ] {
            let n = units_in(v, u);
            assert!(n as f64 * u <= v + 1e-9, "{v} {u}");
            assert!((n + 1) as f64 * u > v - 1e-9, "{v} {u}");
        }
        assert_eq!(units_in(10.0, 0.0), 0);
        assert_eq!(units_in(-1.0, 0.1), 0);
    }

    /// `SetBallRadius` is one destiny call of two arguments, `(ballID,
    /// radius)` `[V]`, stamped when it leaves.
    #[test]
    fn the_radius_push_is_one_two_argument_destiny_call() {
        let mut p = RockRadius::new(vec![ALICE], 5, ROCK, 1_234.5).expect("a push");
        p.restamp(9);
        let (name, args) = p.to_push();
        assert_eq!(name, "DoDestinyUpdate");
        assert_eq!(
            args,
            vec![
                PyValue::List(vec![PyValue::Tuple(vec![
                    PyValue::Int(9),
                    PyValue::Tuple(vec![
                        crate::pyutil::bstr("SetBallRadius"),
                        PyValue::Tuple(vec![PyValue::Int(ROCK), PyValue::Real(1_234.5)]),
                    ]),
                ])]),
                PyValue::Int(0),
            ]
        );
        assert_eq!(p.audience(), Some(vec![ALICE]));
        assert!(RockRadius::new(vec![], 5, ROCK, 1.0).is_none());
        assert!(RockRadius::new(vec![ALICE], 5, ROCK, f64::NAN).is_none());
    }

    // -- Gas (SOC-B2) --------------------------------------------------------

    /// Invented: a gas cloud type and a gas harvester, neither the client's.
    /// The group (711) and the effect (2726) are the subject, so they are
    /// the consts, not copies.
    const GAS: i64 = 990_711_001;
    const HARVESTER_TYPE: i64 = 990_737_001;
    const HARVESTER: i64 = 100_000_063;
    const CLOUD: i64 = crate::asteroid::MIN_ROCK_ID + 1;
    /// The test harvester's name for its effect (the client sends the name;
    /// the row's, whatever it is).
    const CLOUD_EFFECT: &str = "fixtureClouds";

    /// `typedata::FIXTURE` plus the cloud, the harvester and its effect row,
    /// in the tool's format. **Invented numbers.**
    fn gas_content() -> crate::content::Content {
        let rows = format!(
            "type\t{GAS}\t{GROUP_HARVESTABLE_CLOUD}\t2\t1\t1\t2.5\t0.0\t0.0\t100.0\t0\t\t\tFixture Gas Cloud\n\
             type\t{HARVESTER_TYPE}\t99737\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{EFFECT_MINING_CLOUDS}\t12,16,42,{EFFECT_MINING_CLOUDS}\t6=4.0;30=1.0;50=10.0;54=9000.0;73=20000.0;77=25.0\tFixture Gas Harvester\n\
             effect\t{EFFECT_MINING_CLOUDS}\t2\t0\t73\t6\t54\t0\t0\t{CLOUD_EFFECT}\teffects.FixtureClouds\n"
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = crate::content::Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    /// A cloud of [`GAS`] at the rock's place, by the invented law.
    fn cloud(quantity: i64) -> Rock {
        Rock {
            id: CLOUD,
            type_id: GAS,
            ..rock(quantity)
        }
    }

    /// Alice beside a cloud (first, so she parks by it) and a rock, both
    /// locked, a Miner I and the harvester fitted.
    fn alice_huffing(quantity: i64) -> Registry {
        alice_with(
            Some(gas_content()),
            vec![cloud(quantity), rock(quantity)],
            &[(HARVESTER, HARVESTER_TYPE, HI_SLOT_0 + 1)],
        )
    }

    fn gas_in_cargo(reg: &Registry) -> i64 {
        cargo(reg)
            .iter()
            .filter(|i| i.type_id == GAS)
            .map(|i| i.quantity)
            .sum()
    }

    fn huff(reg: &mut Registry) -> Result<(), ServiceError> {
        modules::activate(
            reg,
            Some(ALICE),
            HARVESTER,
            CLOUD_EFFECT,
            Some(CLOUD),
            1000,
            T0,
        )
        .map(|_| ())
    }

    /// The pairing rule as a table: the harvester cuts clouds only, every
    /// other mining effect anything but a cloud.
    #[test]
    fn a_harvester_cuts_clouds_and_a_laser_cuts_rocks() {
        assert!(cuts(EFFECT_MINING_CLOUDS, true));
        assert!(!cuts(EFFECT_MINING_CLOUDS, false));
        assert!(cuts(modules::EFFECT_MINING_LASER, false));
        assert!(!cuts(modules::EFFECT_MINING_LASER, true));
        assert!(is_mining(EFFECT_MINING_CLOUDS));
        let c = gas_content();
        assert!(is_gas(&c, GAS));
        assert!(!is_gas(&c, VELDSPAR));
        assert!(!is_gas(&c, 990_799_999), "an unknown type is no cloud");
    }

    /// **One completed harvester cycle moves gas from the cloud to the
    /// hold**: the cloud's own type, `floor(miningAmount / volume)` units,
    /// and the cloud down by exactly that.
    #[test]
    fn a_completed_harvester_cycle_moves_gas_to_the_hold() {
        let q0 = 5_000;
        let mut reg = alice_huffing(q0);
        huff(&mut reg).expect("the harvester starts on the cloud");
        let t = modules::find_in(reg.content(), HARVESTER_TYPE).expect("the harvester");
        let period = t.duration_ms() * modules::MSEC;
        let unit = crate::item::find_in(reg.content(), GAS)
            .expect("the cloud is an item")
            .volume;
        let per_cycle = units_in(t.mining_amount(), unit);
        assert!(per_cycle > 0);
        clock(&mut reg, T0 + period);
        assert_eq!(gas_in_cargo(&reg), per_cycle);
        assert_eq!(
            reg.rocks().get(CLOUD).expect("still there").quantity,
            q0 - per_cycle,
            "gas is conserved"
        );
        assert_eq!(ore_in_cargo(&reg), 0, "the rock beside it is untouched");
        clock(&mut reg, T0 + 2 * period);
        assert_eq!(gas_in_cargo(&reg), 2 * per_cycle, "one stack, grown");
    }

    /// **Each module refuses the other kind**: the harvester on a rock, the
    /// Miner I on a cloud. Nothing starts, nothing is cut.
    #[test]
    fn a_harvester_refuses_a_rock_and_a_laser_refuses_a_cloud() {
        let mut reg = alice_huffing(5_000);
        assert!(
            modules::activate(
                &mut reg,
                Some(ALICE),
                HARVESTER,
                CLOUD_EFFECT,
                Some(ROCK),
                1000,
                T0
            )
            .is_err()
        );
        assert!(reg.modules().running(HARVESTER).is_none());
        assert!(
            modules::activate(
                &mut reg,
                Some(ALICE),
                MINER,
                "miningLaser",
                Some(CLOUD),
                1000,
                T0
            )
            .is_err()
        );
        assert!(reg.modules().running(MINER).is_none());
        assert_eq!(reg.rocks().get(CLOUD).expect("cloud").quantity, 5_000);
    }

    /// **A cloud harvested out leaves** like a rock: every unit in the
    /// hold, out of the field, the harvester stopped.
    #[test]
    fn a_cloud_harvested_out_leaves_the_field() {
        let mut reg = alice_huffing(2);
        huff(&mut reg).expect("starts");
        let period = modules::find_in(reg.content(), HARVESTER_TYPE)
            .expect("harvester")
            .duration_ms()
            * modules::MSEC;
        for n in 1..=3 {
            clock(&mut reg, T0 + n * period);
        }
        assert_eq!(gas_in_cargo(&reg), 2);
        assert!(reg.rocks().get(CLOUD).is_none());
        assert!(reg.modules().running(HARVESTER).is_none());
    }

    // -- Drones (SOC-B10) ----------------------------------------------------

    /// Invented: a mining drone, not the client's. Effect 17 is the subject.
    const DIGGER: i64 = 990_101_001;

    fn drone_content() -> crate::content::Content {
        let rows = format!(
            "type\t{DIGGER}\t99101\t18\t1\t1\t5.0\t0.0\t3000.0\t5.0\t{EFFECT_MINING}\t{EFFECT_MINING}\t37=400.0;54=5000.0;73=60000.0;77=1.0;1272=5.0\tFixture Digger\n"
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = crate::content::Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    /// **A drone's cycle is the laser's cut, into its controller's hold**:
    /// `floor(miningAmount / volume)` of the rock's ore, the rock down by
    /// as much. Not for a ship the owner is not in, nor a rock that is not
    /// there.
    #[test]
    fn a_drone_cycle_cuts_into_the_controllers_hold() {
        let q0 = 140_000;
        let mut reg = alice_with(Some(drone_content()), vec![rock(q0)], &[]);
        let unit = crate::item::find_in(reg.content(), VELDSPAR)
            .expect("Veldspar")
            .volume;
        let amount = crate::typedata::attribute_in(reg.content(), DIGGER, ATTR_MINING_AMOUNT)
            .expect("miningAmount");
        let per = units_in(amount, unit);
        assert!(per > 0);
        assert_eq!(
            drone_cycle(&mut reg, ALICE, MERLIN, DIGGER, ROCK, T0),
            DroneCut::Mined {
                units: per,
                full: false
            }
        );
        assert_eq!(ore_in_cargo(&reg), per);
        assert_eq!(reg.rocks().get(ROCK).expect("rock").quantity, q0 - per);
        assert_eq!(
            drone_cycle(&mut reg, ALICE, MERLIN + 1, DIGGER, ROCK, T0),
            DroneCut::Nothing,
            "not the ship she flies"
        );
        assert_eq!(
            drone_cycle(&mut reg, ALICE, MERLIN, DIGGER, ROCK + 9, T0),
            DroneCut::Gone
        );
        assert_eq!(ore_in_cargo(&reg), per, "and nothing more");
    }

    /// An invented hull with a mining hold of [`HOLD_M3`] and a cargo hold
    /// of [`HULL_CARGO_M3`] (GAPS-B), and an invented laser cutting
    /// [`LASER_M3`] every [`LASER_MS`].
    const DIGGER_HULL: i64 = 990_920_001;
    const DIGGER_LASER: i64 = 990_920_002;
    const LASER_M3: f64 = 20.0;
    const LASER_MS: i64 = 5_000;
    const HOLD_M3: f64 = 5.0;
    const HULL_CARGO_M3: f64 = 30.0;

    fn hold_content() -> crate::content::Content {
        let hold = crate::mining_hold::ATTR_GENERAL_MINING_HOLD_CAPACITY;
        let rows = format!(
            "effect\t67\t2\t0\t73\t6\t54\t0\t0\tminingLaser\teffects.Mining\n\
             type\t{DIGGER_LASER}\t54\t7\t1\t1\t5.0\t0.0\t0.0\t25.0\t67\t12,16,67\t\
             54=15000.0;73={LASER_MS}.0;77={LASER_M3}\tFixture Digger Laser\n\
             type\t{DIGGER_HULL}\t25\t6\t1\t1\t20000.0\t{HULL_CARGO_M3}\t1200000.0\t33.0\t0\t\t\
             11=45.0;14=3.0;37=355.0;48=125.0;552=40.0;{hold}={HOLD_M3}\tFixture Digger Hull\n"
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = crate::content::Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    fn ore_at(reg: &Registry, flag: i64) -> i64 {
        reg.store()
            .at(crate::item::Slot::new(ALICE, MERLIN, flag))
            .filter(|i| i.type_id == VELDSPAR)
            .map(|i| i.quantity)
            .sum()
    }

    /// **GAPS-B**: a laser cycle on a hull with a mining hold fills the
    /// hold first and spills the rest into cargo; with both full the laser
    /// stops on the full-hold key; the rock gives up exactly what arrived.
    #[test]
    fn a_laser_cycle_fills_the_mining_hold_first_then_cargo() {
        let q0 = 140_000;
        let mut reg = alice_flying(
            Some(hold_content()),
            (DIGGER_HULL, DIGGER_LASER),
            vec![rock(q0)],
            &[],
        );
        let unit = crate::item::find_in(reg.content(), VELDSPAR)
            .expect("Veldspar")
            .volume;
        let per = units_in(LASER_M3, unit);
        let period = LASER_MS * modules::MSEC;
        let (hold, cargo) = (units_in(HOLD_M3, unit), units_in(HULL_CARGO_M3, unit));
        assert!(per > hold && per < hold + cargo, "the fixture spills");
        start(&mut reg);
        let _ = reg.take_pending();
        clock(&mut reg, T0 + period);
        let flag = crate::mining_hold::FLAG_GENERAL_MINING_HOLD;
        assert_eq!(ore_at(&reg, flag), hold, "the mining hold first");
        assert_eq!(ore_at(&reg, crate::fitting::FLAG_CARGO), per - hold);
        assert_eq!(reg.rocks().get(ROCK).expect("rock").quantity, q0 - per);

        clock(&mut reg, T0 + 2 * period);
        assert_eq!(ore_at(&reg, flag), hold);
        assert_eq!(
            ore_at(&reg, crate::fitting::FLAG_CARGO),
            cargo,
            "cargo full"
        );
        assert!(
            reg.modules().running(MINER).is_none(),
            "both holds full: the laser stopped"
        );
        assert_eq!(
            reg.rocks().get(ROCK).expect("rock").quantity,
            q0 - hold - cargo
        );
    }

    /// **GAPS-B**: the mining hold refuses what its row does not allow:
    /// a mined type outside the asteroid category and its allowed groups
    /// goes to cargo ([`crate::mining_hold::accepts`]).
    #[test]
    fn a_type_the_mining_hold_refuses_goes_to_cargo() {
        let reg = alice_flying(
            Some(hold_content()),
            (DIGGER_HULL, DIGGER_LASER),
            vec![rock(1_000)],
            &[],
        );
        let mut ore = *crate::item::find_in(reg.content(), VELDSPAR).expect("Veldspar");
        let room = crate::mining_hold::room_for(&reg, MERLIN, DIGGER_HULL, &ore);
        assert_eq!(room.hold, units_in(HOLD_M3, ore.volume));
        ore.category_id = 4;
        ore.group_id = 99_004;
        let room = crate::mining_hold::room_for(&reg, MERLIN, DIGGER_HULL, &ore);
        assert_eq!(room.hold, 0, "not ore: no room in the mining hold");
        assert_eq!(room.cargo, units_in(HULL_CARGO_M3, ore.volume));
        assert_eq!(
            crate::mining_hold::room_for(&reg, MERLIN, 603, &ore).hold,
            0,
            "a hull without the attribute has none"
        );
    }

    // ---- MINE-UI -------------------------------------------------------

    const BOB: i64 = 1_000_002;
    const BOB_SHIP: i64 = 100_000_070;

    /// Bob in space in Alice's system, a second pilot to hear `OnOreMinedByOther`.
    fn add_bob(reg: &mut Registry) {
        let mut p = *reg.roster().get(ALICE).expect("Alice in space");
        p.character_id = BOB;
        p.ship_id = BOB_SHIP;
        reg.roster_mut().arrive(p);
        reg.seed_character(crate::model::Character {
            character_id: BOB,
            ship_id: BOB_SHIP,
            name: "Bob".to_owned(),
            ..crate::model::Character::default()
        });
    }

    fn pushes<'a>(out: &'a [Sent], name: &str) -> Vec<&'a Sent> {
        out.iter().filter(|(n, _, _)| *n == name).collect()
    }

    fn field<'a>(dict: &'a PyValue, key: &str) -> &'a PyValue {
        crate::pyutil::dict_get(dict, key.as_bytes()).expect("the field")
    }

    /// **A completed cycle queues one `OnOreMined` to the miner and one
    /// `OnOreMinedByOther` to the other pilot, and none to the miner.**
    #[test]
    fn a_cycle_tells_the_miner_and_the_others_on_grid() {
        let mut reg = alice_mining(140_000, &[]);
        add_bob(&mut reg);
        start(&mut reg);
        let _ = reg.take_pending();
        clock(&mut reg, T0 + period());
        let got = ore_in_cargo(&reg);
        assert!(got > 0);
        let out = sent(&mut reg);
        let mined = pushes(&out, "OnOreMined");
        assert_eq!(mined.len(), 1);
        assert_eq!(mined[0].2, Some(vec![ALICE]));
        let d = &mined[0].1[0];
        assert_eq!(field(d, "quantity_removed"), &PyValue::Int(got));
        assert_eq!(field(d, "quantity_added"), &PyValue::Int(got));
        assert_eq!(field(d, "amountWasted"), &PyValue::Int(0));
        assert_eq!(field(d, "itemID"), &PyValue::Int(ROCK));
        assert_eq!(field(d, "oreType"), &PyValue::Int(VELDSPAR));
        assert_eq!(field(d, "moduleItemID"), &PyValue::Int(MINER));
        let other = pushes(&out, "OnOreMinedByOther");
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].1, vec![PyValue::Int(ROCK), PyValue::Int(got)]);
        assert_eq!(other[0].2, Some(vec![BOB]));
    }

    /// The ledger gets the same units, under the miner's day, system and ore.
    #[test]
    fn a_cycle_is_recorded_in_the_mining_ledger() {
        let mut reg = alice_mining(140_000, &[]);
        start(&mut reg);
        clock(&mut reg, T0 + period());
        clock(&mut reg, T0 + 2 * period());
        let got = ore_in_cargo(&reg);
        let rows = crate::mining_ledger::logs(reg.deltas(), ALICE, T0 + 2 * period());
        assert_eq!(rows.len(), 1, "two cycles, one row");
        assert_eq!(rows[0].quantity, got);
        assert_eq!(rows[0].type_id, VELDSPAR);
        assert_eq!(rows[0].solar_system_id, 30_000_001);
    }

    /// A hold with no room for one unit cuts nothing: no push, no ledger row.
    #[test]
    fn a_full_hold_sends_no_ore_pushes_and_records_nothing() {
        let mut reg = alice_with(Some(drone_content()), vec![rock(140_000)], &[]);
        add_bob(&mut reg);
        let capacity = crate::item::find_in(reg.content(), 603)
            .expect("Merlin")
            .capacity;
        let trit = crate::item::find_in(reg.content(), 34)
            .expect("Tritanium")
            .volume;
        let unit = crate::item::find_in(reg.content(), VELDSPAR)
            .expect("Veldspar")
            .volume;
        reg.apply_items(&[crate::item::Edit::Create {
            type_id: 34,
            quantity: units_in(capacity - 0.5 * unit, trit),
            to: crate::item::Slot::new(ALICE, MERLIN, crate::fitting::FLAG_CARGO),
        }])
        .expect("filled");
        let _ = sent(&mut reg);
        assert_eq!(
            drone_cycle(&mut reg, ALICE, MERLIN, DIGGER, ROCK, T0),
            DroneCut::Full
        );
        let out = sent(&mut reg);
        assert!(pushes(&out, "OnOreMined").is_empty());
        assert!(pushes(&out, "OnOreMinedByOther").is_empty());
        assert!(crate::mining_ledger::logs(reg.deltas(), ALICE, T0).is_empty());
    }

    /// A drone's cut is announced like a laser's, with no module id **[A]**.
    #[test]
    fn a_drone_cut_is_announced_too() {
        let mut reg = alice_with(Some(drone_content()), vec![rock(140_000)], &[]);
        let DroneCut::Mined { units, .. } = drone_cycle(&mut reg, ALICE, MERLIN, DIGGER, ROCK, T0)
        else {
            panic!("mined")
        };
        let out = sent(&mut reg);
        let mined = pushes(&out, "OnOreMined");
        assert_eq!(mined.len(), 1);
        assert_eq!(
            field(&mined[0].1[0], "quantity_removed"),
            &PyValue::Int(units)
        );
        assert_eq!(
            crate::mining_ledger::logs(reg.deltas(), ALICE, T0)[0].quantity,
            units
        );
    }

    const SCANNER_HULL: i64 = 99_201;

    /// A hull row with survey-scanner tier `tier` (attribute 5979).
    fn scanner_content(tier: f64) -> crate::content::Content {
        let rows = format!(
            "type\t{SCANNER_HULL}\t25\t6\t1\t1\t20000.0\t100.0\t1200000.0\t33.0\t0\t\t\
             11=45.0;14=3.0;37=355.0;48=125.0;552=40.0;{ATTR_SCANNER_TIER}={tier}\tFixture Surveyor\n"
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = crate::content::Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    fn surveyor(tier: f64) -> Registry {
        let near = rock(140_000);
        let far = Rock {
            id: ROCK + 1,
            position: [1_000_000_000.0, 0.0, 0.0],
            ..rock(90_000)
        };
        let mut reg = alice_with(Some(scanner_content(tier)), vec![near, far], &[]);
        reg.character_mut(ALICE).expect("Alice").ship_type_id = SCANNER_HULL;
        reg
    }

    #[test]
    fn perform_scan_is_none_below_tier_two_and_for_a_pilot_not_in_space() {
        let reg = surveyor(1.0);
        assert_eq!(survey(&reg, ALICE), None);
        assert_eq!(survey(&reg, BOB), None);
        let plain = alice_mining(140_000, &[]);
        assert_eq!(survey(&plain, ALICE), None, "no attribute, no scanner");
    }

    #[test]
    fn perform_scan_lists_rocks_in_reach_nearest_first() {
        let reg = surveyor(2.0);
        let rows = survey(&reg, ALICE).expect("a tier-2 scan");
        assert_eq!(
            rows,
            vec![(ROCK, VELDSPAR, 140_000)],
            "2 km in, 1,000,000 km out"
        );
    }

    fn crowd(bots: bool) -> (Registry, i64) {
        let mut reg = alice_mining(140_000_000, &[]);
        for i in 0..249 {
            let mut p = *reg.roster().get(ALICE).expect("Alice in space");
            p.character_id = BOB + i;
            p.ship_id = BOB_SHIP + i;
            reg.roster_mut().arrive(p);
            reg.seed_character(crate::model::Character {
                character_id: BOB + i,
                ship_id: BOB_SHIP + i,
                is_bot: bots,
                name: format!("Bot {i}"),
                ..crate::model::Character::default()
            });
        }
        (reg, 250)
    }

    /// 250 miners on one rock, all humans (the worst case) or all bots with
    /// one human watching. Prints the push counts and the build time.
    #[test]
    fn push_count_at_two_hundred_fifty_miners() {
        for bots in [false, true] {
            let (mut reg, n) = crowd(bots);
            if bots {
                reg.character_mut(ALICE).expect("Alice").is_bot = false;
            }
            let rock = *reg.rocks().get(ROCK).expect("rock");
            let started = std::time::Instant::now();
            for i in 0..n {
                announce(
                    &mut reg,
                    if i == 0 { ALICE } else { BOB + i - 1 },
                    MINER,
                    &rock,
                    Yield {
                        normal: 100,
                        ..Yield::default()
                    },
                    T0,
                );
            }
            let took = started.elapsed();
            let out = sent(&mut reg);
            let mined = pushes(&out, "OnOreMined").len();
            let by_other = pushes(&out, "OnOreMinedByOther");
            let slots: usize = by_other
                .iter()
                .map(|p| p.2.as_ref().map_or(0, Vec::len))
                .sum();
            eprintln!(
                "[mine-ui] {n} {} miners, one round: {mined} OnOreMined, {} OnOreMinedByOther to {slots} slots, built in {took:?}",
                if bots { "bot (1 human)" } else { "human" },
                by_other.len()
            );
            if bots {
                // Alice's own bite, plus each bot's bite told to Alice alone.
                assert_eq!(mined, 1);
                assert_eq!(slots, 249);
            } else {
                assert_eq!(mined, 250);
                assert_eq!(slots, 250 * 249);
            }
        }
    }

    // ---- MINING-CRITS --------------------------------------------------
    //
    // Invented ids and rows throughout: every expectation below is the
    // client's *formula* (`mining.MiningResult`) applied by hand to the
    // numbers written here, never a value from the data.

    const CRIT_HULL: i64 = 990_310_001;
    const CRIT_LASER: i64 = 990_310_002;
    const CRIT_ORE: i64 = 990_310_003;
    const NOCRIT_ORE: i64 = 990_310_004;
    const NOWASTE_ORE: i64 = 990_310_005;
    /// The skill the laser requires, and the skill's own bonus attribute.
    const CRIT_SKILL: i64 = 990_310_006;
    const UPGRADE: i64 = 990_310_007;
    const CRIT_DRONE: i64 = 990_310_008;
    const UPGRADE_ITEM: i64 = 100_000_075;
    const LO_SLOT_0: i64 = 11;
    /// Cubic metres of one unit of the invented ores.
    const UNIT_M3: f64 = 2.0;

    /// The invented world: a hull with a roomy hold, a laser carrying
    /// `laser_attrs` (on top of 100 m3 a cycle every 5 s and the skill it
    /// requires), three ores (plain, crit-proof, waste-proof), a skill that
    /// adds `level` percent to the yield and `level` to the crit chance, an
    /// upgrade module that adds 101 to the waste percent, and a drone.
    fn crit_content(laser_attrs: &str) -> crate::content::Content {
        let rows = format!(
            "attribute\t9901\t1.0\tfixtureBonus\n\
             attribute\t182\t0.0\trequiredSkill1\n\
             attribute\t280\t0.0\tskillLevel\n\
             attribute\t9902\t0.0\tfixtureWaste\n\
             attribute\t9903\t0.0\tfixtureReach\n\
             effect\t67\t2\t0\t73\t6\t54\t0\t0\tminingLaser\teffects.Mining\n\
             effect\t9901\t0\t0\t0\t0\t0\t0\t0\tfixtureSkill\t\n\
             effect\t9902\t4\t0\t0\t0\t0\t0\t0\tfixtureUpgrade\t\n\
             effect\t9903\t4\t0\t0\t0\t0\t0\t0\tfixtureReach\t\n\
             modifier\t9901\titemID\tItemModifier\t9901\t280\t0\t\t\n\
             modifier\t9901\tshipID\tLocationRequiredSkillModifier\t77\t9901\t6\t{CRIT_SKILL}\t\n\
             modifier\t9901\tshipID\tLocationRequiredSkillModifier\t5967\t9901\t2\t{CRIT_SKILL}\t\n\
             modifier\t9902\tshipID\tLocationRequiredSkillModifier\t3154\t9902\t2\t{CRIT_SKILL}\t\n\
             modifier\t9903\tshipID\tLocationRequiredSkillModifier\t54\t9903\t2\t{CRIT_SKILL}\t\n\
             type\t{CRIT_SKILL}\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9901\t9901=1.0\tFixture Crit Skill\n\
             type\t{UPGRADE}\t1\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t11,16,9902,9903\t9902=101.0;9903=40000.0\tFixture Upgrade\n\
             type\t{CRIT_LASER}\t54\t7\t1\t1\t5.0\t0.0\t0.0\t25.0\t67\t12,16,67\t\
             54=15000.0;73=5000.0;77=100.0;182={CRIT_SKILL}.0;{laser_attrs}\tFixture Crit Laser\n\
             type\t{CRIT_HULL}\t25\t6\t1\t1\t20000.0\t5000.0\t1200000.0\t33.0\t0\t\t\
             11=45.0;14=3.0;37=355.0;48=125.0;552=40.0\tFixture Crit Hull\n\
             type\t{CRIT_ORE}\t450\t25\t1\t100\t{UNIT_M3}\t0.0\t0.0\t1.0\t0\t\t\tFixture Crit Ore\n\
             type\t{NOCRIT_ORE}\t450\t25\t1\t100\t{UNIT_M3}\t0.0\t0.0\t1.0\t0\t\t{ign_crit}=1.0\tFixture Crit-proof Ore\n\
             type\t{NOWASTE_ORE}\t450\t25\t1\t100\t{UNIT_M3}\t0.0\t0.0\t1.0\t0\t\t{ign_waste}=1.0\tFixture Waste-proof Ore\n\
             type\t{CRIT_DRONE}\t101\t18\t1\t1\t5.0\t0.0\t3000.0\t5.0\t{drone_eff}\t{drone_eff}\t\
             37=400.0;54=5000.0;73=60000.0;77=40.0;5967=5.0;5969=0.5\tFixture Crit Drone\n",
            ign_crit = ATTR_IGNORE_CRITS,
            ign_waste = ATTR_IGNORE_WASTE,
            drone_eff = EFFECT_MINING,
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = crate::content::Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    fn ore_rock(type_id: i64, quantity: i64) -> Rock {
        Rock {
            type_id,
            ..rock(quantity)
        }
    }

    /// Alice in the invented hull with the invented laser (`laser_attrs`
    /// appended to its row) on one rock of `ore`.
    fn crit_world(laser_attrs: &str, ore: i64, quantity: i64) -> Registry {
        alice_flying(
            Some(crit_content(laser_attrs)),
            (CRIT_HULL, CRIT_LASER),
            vec![ore_rock(ore, quantity)],
            &[],
        )
    }

    fn one_cycle(reg: &mut Registry) {
        start(reg);
        let _ = reg.take_pending();
        clock(reg, T0 + 5_000 * modules::MSEC);
    }

    fn held(reg: &Registry, ore: i64) -> i64 {
        cargo(reg)
            .iter()
            .filter(|i| i.type_id == ore)
            .map(|i| i.quantity)
            .sum()
    }

    fn rock_left(reg: &Registry) -> i64 {
        reg.rocks().get(ROCK).expect("the rock").quantity
    }

    fn train_crit_skill(reg: &mut Registry, level: i64) {
        let entries = vec![crate::skill::SkillEntry {
            type_id: CRIT_SKILL,
            level,
            points: 0,
            rank: 1,
        }];
        *reg.skills_mut(ALICE).expect("Alice has skills") = crate::skill::Skills::restore(entries);
    }

    /// The client's draws, replayed by hand in the client's order from a
    /// twin stream: the residue's `randint`, the yield's `round_rand`, the
    /// residue's `round_rand`, then the crit's `random()` (only for a
    /// non-zero yield and chance). Integral numbers, so each `round_rand`
    /// consumes one draw and rounds down.
    #[test]
    fn a_roll_replays_the_clients_draws_in_the_clients_order() {
        let t = Take {
            amount: 20.0,
            crit_chance: 0.5,
            crit_bonus: 0.5,
            waste_volume: 0.25,
            waste_percent: 40.0,
        };
        let (mut hits, mut crits) = (0, 0);
        for seed in 0..200u64 {
            let mut s = crate::seed::Stream::new(seed);
            let got = roll(
                &mut s,
                &t,
                UNIT_M3,
                1_000,
                1_000,
                false,
                false,
                Forced::default(),
            );
            let mut twin = crate::seed::Stream::new(seed);
            let waste_hit = 40.0 > twin.range(0, 101) as f64;
            let _normal = twin.unit();
            let _wasted = twin.unit();
            let crit_hit = 0.5 > twin.unit();
            // 20 m3 of 2 m3 ore: 10 units; 25 % of that wasted: 2.5 units,
            // so 2 or 3; a crit is 5 units, whole.
            assert_eq!(got.normal, 10, "seed {seed}");
            assert_eq!(got.crit, if crit_hit { 5 } else { 0 }, "seed {seed}");
            if waste_hit {
                assert!((2..=3).contains(&got.wasted), "seed {seed}: {got:?}");
                hits += 1;
            } else {
                assert_eq!(got.wasted, 0, "seed {seed}");
            }
            crits += i32::from(crit_hit);
            // The same stream gives the same cycle.
            let mut again = crate::seed::Stream::new(seed);
            assert_eq!(
                got,
                roll(
                    &mut again,
                    &t,
                    UNIT_M3,
                    1_000,
                    1_000,
                    false,
                    false,
                    Forced::default()
                )
            );
        }
        assert!((40..=120).contains(&hits), "waste hit {hits} of 200");
        assert!((60..=140).contains(&crits), "crit hit {crits} of 200");
    }

    /// `round_rand`: a fraction rounds up with that probability, so 2.5 is
    /// 2 or 3 and averages 2.5; a whole number never moves.
    #[test]
    fn round_rand_rounds_a_fraction_up_with_its_own_probability() {
        let mut s = crate::seed::Stream::new(7);
        let n = 4_000;
        let total: i64 = (0..n).map(|_| round_rand(&mut s, 2.5)).sum();
        let mean = total as f64 / f64::from(n);
        assert!((2.4..2.6).contains(&mean), "mean {mean}");
        for _ in 0..200 {
            assert!([2, 3].contains(&round_rand(&mut s, 2.5)));
            assert_eq!(round_rand(&mut s, 4.0), 4);
        }
        assert_eq!(round_rand(&mut s, 0.0), 0);
        assert_eq!(round_rand(&mut s, -3.0), 0);
        assert_eq!(round_rand(&mut s, f64::NAN), 0);
    }

    /// The two comparators differ: waste is a percent against `randint(0,
    /// 100)` (101 always hits, 0 never), crit a fraction against `random()`
    /// (a fraction of 1 or more always hits, 0 never).
    #[test]
    fn waste_is_a_percent_and_crit_a_fraction() {
        let base = Take {
            amount: 20.0,
            crit_chance: 0.0,
            crit_bonus: 1.0,
            waste_volume: 1.0,
            waste_percent: 0.0,
        };
        for seed in 0..100u64 {
            let at = |t: &Take| {
                roll(
                    &mut crate::seed::Stream::new(seed),
                    t,
                    UNIT_M3,
                    1_000,
                    1_000,
                    false,
                    false,
                    Forced::default(),
                )
            };
            assert_eq!(
                at(&base),
                Yield {
                    normal: 10,
                    crit: 0,
                    wasted: 0
                }
            );
            let waste = Take {
                waste_percent: 101.0,
                ..base
            };
            assert_eq!(
                at(&waste),
                Yield {
                    normal: 10,
                    crit: 0,
                    wasted: 10
                }
            );
            let crit = Take {
                crit_chance: 1.0,
                ..base
            };
            assert_eq!(
                at(&crit),
                Yield {
                    normal: 10,
                    crit: 10,
                    wasted: 0
                }
            );
            // One percent is not a fraction of one: 1 % residue is a rare hit.
            let rare = Take {
                waste_percent: 1.0,
                ..base
            };
            assert!(at(&rare).wasted <= 10);
        }
    }

    /// **A crit goes to the hold and not out of the rock** (F17: the
    /// scanning service subtracts yield + waste). `OnOreMined` says so, and
    /// so does the ledger; `hasRewards` stays false.
    #[test]
    fn a_crit_adds_to_the_hold_and_not_to_the_rock() {
        // 100 m3 of 2 m3 ore: 50 units; a certain crit of half again: 25.
        let mut reg = crit_world("5967=2.0;5969=0.5", CRIT_ORE, 1_000);
        one_cycle(&mut reg);
        assert_eq!(held(&reg, CRIT_ORE), 75, "yield plus crit in the hold");
        assert_eq!(rock_left(&reg), 950, "only the yield left the rock");
        let out = sent(&mut reg);
        let mined = pushes(&out, "OnOreMined");
        assert_eq!(mined.len(), 1);
        let d = &mined[0].1[0];
        assert_eq!(field(d, "quantity_added"), &PyValue::Int(75));
        assert_eq!(field(d, "quantity_removed"), &PyValue::Int(50));
        assert_eq!(field(d, "amountCritBonus"), &PyValue::Int(25));
        assert_eq!(field(d, "amountWasted"), &PyValue::Int(0));
        assert_eq!(field(d, "hasRewards"), &PyValue::Bool(false));
        let rows = crate::mining_ledger::logs(reg.deltas(), ALICE, T0 + 5_000 * modules::MSEC);
        assert_eq!((rows[0].quantity, rows[0].wasted), (75, 0));
    }

    /// **Waste leaves the rock and arrives nowhere**, and the ledger
    /// carries it; the others on grid are told the rock lost yield + waste.
    #[test]
    fn waste_reduces_the_rock_and_is_ledgered() {
        // 50 units, a certain residue of half the volume: 25 units.
        let mut reg = crit_world("3153=0.5;3154=101.0", CRIT_ORE, 1_000);
        add_bob(&mut reg);
        one_cycle(&mut reg);
        assert_eq!(held(&reg, CRIT_ORE), 50);
        assert_eq!(rock_left(&reg), 1_000 - 50 - 25);
        let out = sent(&mut reg);
        let d = &pushes(&out, "OnOreMined")[0].1[0];
        assert_eq!(field(d, "quantity_added"), &PyValue::Int(50));
        assert_eq!(field(d, "quantity_removed"), &PyValue::Int(50));
        assert_eq!(field(d, "amountWasted"), &PyValue::Int(25));
        assert_eq!(field(d, "amountCritBonus"), &PyValue::Int(0));
        let other = &pushes(&out, "OnOreMinedByOther")[0].1;
        assert_eq!(other[1], PyValue::Int(75), "the rock lost yield + waste");
        let rows = crate::mining_ledger::logs(reg.deltas(), ALICE, T0 + 5_000 * modules::MSEC);
        assert_eq!((rows[0].quantity, rows[0].wasted), (50, 25));
    }

    /// Waste cannot take more than the rock has left after the yield.
    #[test]
    fn waste_is_cut_to_what_the_rock_has_left() {
        let mut reg = crit_world("3153=1.0;3154=101.0", CRIT_ORE, 60);
        one_cycle(&mut reg);
        assert_eq!(held(&reg, CRIT_ORE), 50);
        assert!(reg.rocks().get(ROCK).is_none(), "the rock is gone");
        let rows = crate::mining_ledger::logs(reg.deltas(), ALICE, T0 + 5_000 * modules::MSEC);
        assert_eq!(rows[0].wasted, 10, "60 - 50, not the 50 the volume asks");
    }

    /// 6108 and 3236 on the ore switch the crit and the residue off; the
    /// same laser on an ore without them has both.
    #[test]
    fn the_ores_ignore_flags_switch_crit_and_waste_off() {
        let laser = "5967=2.0;5969=0.5;3153=0.5;3154=101.0";
        let mut plain = crit_world(laser, CRIT_ORE, 1_000);
        one_cycle(&mut plain);
        assert_eq!(held(&plain, CRIT_ORE), 50 + 25);
        assert_eq!(rock_left(&plain), 1_000 - 50 - 25);

        let mut no_crit = crit_world(laser, NOCRIT_ORE, 1_000);
        one_cycle(&mut no_crit);
        assert_eq!(held(&no_crit, NOCRIT_ORE), 50, "no crit");
        assert_eq!(rock_left(&no_crit), 1_000 - 50 - 25, "waste still");

        let mut no_waste = crit_world(laser, NOWASTE_ORE, 1_000);
        one_cycle(&mut no_waste);
        assert_eq!(held(&no_waste, NOWASTE_ORE), 50 + 25, "crit still");
        assert_eq!(rock_left(&no_waste), 1_000 - 50, "no waste");
    }

    /// **The modified attributes are the ones read**: a pilot with the skill
    /// has +5 % yield and a certain crit; without it the same laser has the
    /// type's 100 m3 and a chance of nothing.
    #[test]
    fn a_skill_modified_amount_and_chance_are_used() {
        let laser = "5967=0.0;5969=1.0";
        let mut untrained = crit_world(laser, CRIT_ORE, 1_000);
        one_cycle(&mut untrained);
        assert_eq!(held(&untrained, CRIT_ORE), 50, "the type's numbers");

        let mut trained = crit_world(laser, CRIT_ORE, 1_000);
        train_crit_skill(&mut trained, 5);
        let sheet = crate::dogma::module_attribute(&trained, MERLIN, MINER, 77).expect("sheet");
        assert!((sheet - 105.0).abs() < 1e-9, "the sheet has +5 %: {sheet}");
        one_cycle(&mut trained);
        // 105 m3 is 52.5 units: 52 or 53 whole units, and a certain crit of
        // the same size; the rock lost only the yield.
        let gone = 1_000 - rock_left(&trained);
        assert!((52..=53).contains(&gone), "yield {gone}");
        assert_eq!(held(&trained, CRIT_ORE), 2 * gone, "crit equals the yield");
    }

    /// An upgrade fitted and online (a crystal, a rig, a burst take the same
    /// road) moves the residue's chance from 0 to certain.
    #[test]
    fn a_fitted_upgrade_moves_the_waste_chance() {
        let laser = "3153=0.5;3154=0.0;5967=0.0";
        let mut reg = alice_flying(
            Some(crit_content(laser)),
            (CRIT_HULL, CRIT_LASER),
            vec![ore_rock(CRIT_ORE, 1_000)],
            &[(UPGRADE_ITEM, UPGRADE, LO_SLOT_0)],
        );
        train_crit_skill(&mut reg, 0);
        reg.modules_mut().set_online(UPGRADE_ITEM, true);
        let p = crate::dogma::module_attribute(&reg, MERLIN, MINER, 3154).expect("sheet");
        assert!((p - 101.0).abs() < 1e-9, "the upgrade is in the sheet: {p}");
        one_cycle(&mut reg);
        assert_eq!(held(&reg, CRIT_ORE), 50);
        assert_eq!(rock_left(&reg), 1_000 - 50 - 25);
    }

    /// **Mining range reads the modified sheet.** An invented burst-like
    /// module adds 40 km to `maxRange` (54); a rock past the laser's own
    /// 15 km is refused without it, accepted with it, and a cycle ending
    /// there cuts.
    #[test]
    fn a_range_modifier_on_the_sheet_reaches_the_laser() {
        let place = |reg: &mut Registry| {
            let r = reg.rocks().get(ROCK).copied().expect("the rock");
            park(reg, [r.position[0] + r.radius + 30_000.0, 0.0, 0.0]);
        };
        let fly = || {
            alice_flying(
                Some(crit_content("")),
                (CRIT_HULL, CRIT_LASER),
                vec![ore_rock(CRIT_ORE, 1_000)],
                &[(UPGRADE_ITEM, UPGRADE, LO_SLOT_0)],
            )
        };
        let mut reg = fly();
        train_crit_skill(&mut reg, 0);
        reg.modules_mut().set_online(UPGRADE_ITEM, false);
        place(&mut reg);
        let refused = modules::activate(
            &mut reg,
            Some(ALICE),
            MINER,
            "miningLaser",
            Some(ROCK),
            1000,
            T0,
        );
        assert!(refused.is_err(), "30 km is past the bare 15 km");

        let mut reg = fly();
        train_crit_skill(&mut reg, 0);
        reg.modules_mut().set_online(UPGRADE_ITEM, true);
        let r = crate::dogma::module_attribute(&reg, MERLIN, MINER, 54).expect("sheet");
        assert!(
            (r - 55_000.0).abs() < 1e-9,
            "the sheet holds the reach: {r}"
        );
        place(&mut reg);
        one_cycle(&mut reg);
        assert!(
            held(&reg, CRIT_ORE) > 0,
            "the lengthened laser mined at 30 km"
        );
    }

    /// A drone follows its type's attributes: a crit of half again.
    #[test]
    fn a_drone_follows_its_own_attributes() {
        let mut reg = crit_world("", CRIT_ORE, 1_000);
        let got = drone_cycle(&mut reg, ALICE, MERLIN, CRIT_DRONE, ROCK, T0);
        // 40 m3 of 2 m3 ore: 20 units and a certain crit of 10.
        assert_eq!(
            got,
            DroneCut::Mined {
                units: 30,
                full: false
            }
        );
        assert_eq!(rock_left(&reg), 980);
    }

    /// The dev knob is refused on a normal server, and forces the draws on
    /// a `--dev` one (only where the module has a bonus or multiplier).
    #[test]
    fn the_forced_crit_knob_is_dev_only() {
        let mut normal = crit_world("5967=0.0;5969=0.5", CRIT_ORE, 1_000);
        let said = slash(&mut normal, Some(ALICE), &["crit"]).expect("answers");
        assert!(matches!(said, PyValue::Bytes(ref b) if b.starts_with(b"dev only")));
        assert_eq!(normal.ext::<ForcedSlot>().0, Forced::default());
        one_cycle(&mut normal);
        assert_eq!(held(&normal, CRIT_ORE), 50, "no crit by chance 0");

        let mut dev = Registry::new(crate::model::SandboxConfig {
            dev: true,
            ..crate::model::SandboxConfig::default()
        });
        let _ = slash(&mut dev, Some(ALICE), &["crit"]).expect("answers");
        assert!(dev.ext::<ForcedSlot>().0.crit);
        // Forced, a module with a bonus and no chance crits; without a
        // bonus it still cannot.
        let t = Take {
            amount: 20.0,
            crit_chance: 0.0,
            crit_bonus: 0.5,
            waste_volume: 0.0,
            waste_percent: 0.0,
        };
        let forced = Forced {
            crit: true,
            waste: false,
        };
        let r = |t: &Take, f| {
            roll(
                &mut crate::seed::Stream::new(3),
                t,
                UNIT_M3,
                1_000,
                1_000,
                false,
                false,
                f,
            )
        };
        assert_eq!(r(&t, forced).crit, 5);
        assert_eq!(r(&t, Forced::default()).crit, 0);
        assert_eq!(
            r(
                &Take {
                    crit_bonus: 0.0,
                    ..t
                },
                forced
            )
            .crit,
            0
        );
        let _ = slash(&mut dev, Some(ALICE), &["off"]).expect("answers");
        assert_eq!(dev.ext::<ForcedSlot>().0, Forced::default());
    }
}
