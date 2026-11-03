//! **Player warp scramblers and disruptors** (W16 CP57).
//!
//! The spec is `docs/content/R-TACKLE.md`; section numbers below are its.
//! Marks as `docs/content/README.md` §2.
//!
//! **One number decides warp**: the target ship's `warpScrambleStatus`
//! (104). The ship's own value (the hull's negative warp core strength,
//! plus a running stabiliser's `modAdd`, both of which `crate::dogma`
//! already evaluates) plus the `warpScrambleStrength` (105) of every
//! disruptor (39) and scrambler (5934) running against it, added with no
//! stacking penalty (`modAdd` is not nurfable, §2) [V]. The ship may not
//! warp while that sum is **`> 0`** [I] (`autopilot.py:590`, §2).
//!
//! It needs no state of its own to *refuse*: `Modules::running_against`
//! already lists what is aimed at a ship, so [`scrammed`] is a pure sum
//! over it (§7.1). The [`SLOT`] holds only what the target's client was
//! last *told* (the buff-bar icon, the 104 it reads), so [`tick`] can tell
//! it again when that changes, whichever of `modules`' stop paths ended
//! the effect.
//!
//! The four seams this module plugs into, all coordinator rows:
//!
//! - `movement_guard::WARP_GUARDS` += [`scrammed`]: a warp under a scram
//!   raises `WarpScrambled` (§3);
//! - `movement_guard::JUMP_GUARDS` += [`gate_scrambled`], **not**
//!   [`scrammed`]: a scram does not stop a stargate jump [I] (§5). Only the
//!   HIC scripts raise `gateScrambleStatus`, and nothing we spawn runs one,
//!   so the row never fires yet;
//! - `modules::activate` asks [`activation_blocked`]: a scrambler blocks
//!   the target's microwarpdrive (§4.1) and, [A], its micro jump drive
//!   (§4.2);
//! - `tick::STAGES` += [`tick`], after `modules`: `OnEwarStart`/`OnEwarEnd`
//!   and the 104 change to the target (§6.2, §6.3), a running MWD stopped
//!   when a scram lands (§4.1 [A]), and the scrammer's aggression.
//!
//! **Not here**: NPC tackle (CP49) adds a term to [`status`], never a
//! second guard row (§7.5): two guards could each fail to beat a Venture's
//! −2 where their sum does.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use eve_marshal::PyValue;

use crate::modules::Running;
use crate::movement_guard::Refusal;
use crate::notify::Push;
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};

/// `warpDisrupt`, `effects.WarpDisrupt` [V] (`dogmaeffects[39]`, §1.2).
pub const DISRUPT: i64 = 39;
/// `warpScrambleBlockMWDWithNPCEffect`, `effects.WarpScramble` [V]
/// (`dogmaeffects[5934]`): the player scrambler, which also blocks MWDs.
pub const SCRAMBLE: i64 = 5934;
/// `shipModuleFocusedWarpScramblingScript` [V] (`typedata`): a HIC's
/// Warp Disruption Field Generator with a Focused Warp Scrambling Script
/// (type group 908) loaded. A scrambler that also blocks the MWD.
pub const FOCUSED_SCRAMBLE: i64 = 6848;
/// `shipModuleFocusedWarpDisruptionScript` [V]: the same with a Focused Warp
/// Disruption Script, an infinite-range point.
pub const FOCUSED_DISRUPT: i64 = 6849;
/// The tackle effects a player module runs (§2). CP49 owns the NPC four.
/// The two focused scripts are here too (HIC-SCRIPT): the module that runs
/// one is a 3380 aimed at a target, see [`script_effect_of`].
pub const PLAYER_EFFECTS: &[i64] = &[DISRUPT, SCRAMBLE, FOCUSED_SCRAMBLE, FOCUSED_DISRUPT];
/// The HIC focused scripts, the only effects that raise
/// `gateScrambleStatus` [V] (§1.2).
pub const GATE_EFFECTS: &[i64] = &[FOCUSED_SCRAMBLE, FOCUSED_DISRUPT];
/// The effects that also block the target's MWD and MJD (§4.1): modifier
/// 1349 += 1350 on 3454 and 4385 [V] (`typedata` modifier rows of 5934 and
/// 6848).
const BLOCKING_EFFECTS: &[i64] = &[SCRAMBLE, FOCUSED_SCRAMBLE];
/// The HIC script group [V] (the Warp Disruption Field Generator's
/// `chargeGroup1`, `typedata`).
pub const GROUP_HIC_SCRIPT: i64 = 908;
/// `warpScrambleRange` [V]: the field's radius, and the range of a focused
/// script (the effect row's `rangeAttributeID`, both 6848 and 6849). The
/// script's own `warpScrambleRangeBonus` (1327) is already in the module's
/// sheet value, `dogma` applies the charge's `otherID` modifier.
pub const ATTR_FIELD_RANGE: i64 = 103;
/// `gateScrambleStrength`'s default [V] (`dogmaattributes[1974]`): a module
/// that does not carry it still adds 1.
pub const GATE_STRENGTH_DEFAULT: f64 = 1.0;

/// `warpScrambleStatus` [V] (`dogma/const.py:1107`).
pub const ATTR_STATUS: i64 = 104;
/// `warpScrambleStrength` [V].
pub const ATTR_STRENGTH: i64 = 105;
/// `maxRange` [V]: the scrambler's and disruptor's range attribute (§1.2).
pub const ATTR_MAX_RANGE: i64 = 54;
/// `activationBlockedStrenght` (sic) [V]: what a scrambler adds to the
/// target modules' `activationBlocked` (1349).
pub const ATTR_BLOCK_STRENGTH: i64 = 1350;
/// `gateScrambleStatus` [V] (§5).
pub const ATTR_GATE_STATUS: i64 = 1973;
/// `gateScrambleStrength` [V] (§5).
pub const ATTR_GATE_STRENGTH: i64 = 1974;
/// High Speed Maneuvering [V]: every MWD requires it, and 5934's
/// `LocationRequiredSkillModifier` names it (§4.1).
pub const SKILL_HIGH_SPEED_MANEUVERING: i64 = 3454;
/// Micro Jump Drive Operation [V] (§4.2).
pub const SKILL_MICRO_JUMP_DRIVE: i64 = 4385;

/// What a scrambler blocks: modules requiring one of these skills.
///
/// **[A]** 4385 is here although the player scrambler's data names only
/// 3454 (§4.2): the TQ behaviour players expect. Falsified by a recorded
/// live session where an MJD activates under a Warp Scrambler I.
pub const BLOCKED_SKILLS: &[i64] = &[SKILL_HIGH_SPEED_MANEUVERING, SKILL_MICRO_JUMP_DRIVE];

/// The warp refusal's key [V] (`findings/user-errors.tsv:4209`, no args).
pub const WARP_SCRAMBLED: &str = "WarpScrambled";
/// The gate refusal's key [V] (dialog 4657, no args, §5).
pub const GATE_SCRAMBLED: &str = "GateScrambled";
/// The blocked-module refusal's key [V] (`fittingDogmaLocation.py:613`).
pub const MODULE_IS_BLOCKED: &str = "ModuleIsBlocked";

// ---------------------------------------------------------------------------
// Where the numbers come from
// ---------------------------------------------------------------------------

/// The three reads the rule needs. [`Live`] answers them from
/// `crate::dogma` and the registry's content (`reg.content()`); a test
/// answers them from a fixture, so it states its own rows in a line each.
pub trait Data {
    /// A ship attribute as its own fitting and skills leave it (the hull's
    /// 104 plus a running stabiliser), or `None` when nothing models it.
    fn ship_attr(&self, w: &Registry, ship_id: i64, attribute_id: i64) -> Option<f64>;
    /// A running module's attribute, modified by its own ship.
    fn module_attr(&self, w: &Registry, r: &Running, attribute_id: i64) -> Option<f64>;
    /// A type's own attribute (its `requiredSkill*`), from `w`'s content.
    fn type_attr(&self, w: &Registry, type_id: i64, attribute_id: i64) -> Option<f64>;
    /// The focused script (6848 or 6849) the running module `r` aims, when
    /// it is a field generator with one loaded ([`script_effect_of`]).
    fn script_effect(&self, w: &Registry, r: &Running) -> Option<i64> {
        let flag = w.item(r.module_id).map_or(0, |i| i.flag_id);
        script_effect_of(w, r.ship_id, flag)
    }
}

/// The server's own data: the dogma sheet when `--typedata` is loaded, the
/// content's type rows otherwise.
pub struct Live;

impl Data for Live {
    fn ship_attr(&self, w: &Registry, ship_id: i64, attribute_id: i64) -> Option<f64> {
        crate::dogma::sheet(w, ship_id).map(|s| s.ship(attribute_id))
    }

    fn module_attr(&self, w: &Registry, r: &Running, attribute_id: i64) -> Option<f64> {
        // The sheet's value picks up the hull bonuses to 105 (6358, 11452,
        // 12569, §1.2); without a sheet, the type's own value [A] (§2).
        crate::dogma::module_attribute(w, r.ship_id, r.module_id, attribute_id).or_else(|| {
            crate::modules::find_in(w.content(), r.type_id).and_then(|t| t.attribute(attribute_id))
        })
    }

    fn type_attr(&self, w: &Registry, type_id: i64, attribute_id: i64) -> Option<f64> {
        crate::typedata::attribute_in(w.content(), type_id, attribute_id).or_else(|| {
            crate::modules::find_in(w.content(), type_id).and_then(|t| t.attribute(attribute_id))
        })
    }
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// Whether `effect` is a field generator's effect turned into a targeted one
/// by [`focus`]: 3380 in the target category (natively it is category 1).
pub fn is_focused(effect: &crate::modules::Effect) -> bool {
    effect.effect_id == crate::bubble::EFFECT_FIELD
        && effect.category == crate::modules::CATEGORY_TARGET
}

/// The focused script (6848 or 6849) loaded at `flag_id` of `ship_id`: the
/// loaded charge is in group 908 and carries one of the two effects.
pub fn script_effect_of(w: &Registry, ship_id: i64, flag_id: i64) -> Option<i64> {
    let charge = crate::modules::loaded(w.store(), ship_id, flag_id)?;
    if crate::scan::group_of(w.content(), charge.type_id)? != GROUP_HIC_SCRIPT {
        return None;
    }
    let row = crate::typedata::installed_in(w.content())
        .table()
        .get(charge.type_id)?;
    [FOCUSED_SCRAMBLE, FOCUSED_DISRUPT]
        .into_iter()
        .find(|e| row.effects.contains(e))
}

/// **What a field generator's activation becomes** (HIC-SCRIPT): the module
/// the client sends as `warpDisruptSphere` is a bubble with nothing loaded
/// and, with a focused script loaded, the script's effect aimed at a target
/// (`shipmodulebutton.GetRelevantEffect` asks the charge's effect, category
/// 2, and so locks first) **[V]**.
///
/// With a script: the effect becomes a target effect, its `OnSpecialFX` guid
/// the script's (`effects.WarpScramble` / `effects.WarpDisrupt`), and a
/// missing target is refused (`DeniedActivateTargetNotPresent`, the lock
/// lost in flight). Without: a target is dropped, the bubble names none. The
/// id stays 3380 so the module's own `OnGodmaShipEffect` and its
/// deactivation by name still match; [`effective`] is what the rule reads.
/// Any other effect passes through.
pub fn focus(
    w: &Registry,
    ship_id: i64,
    flag_id: i64,
    effect: crate::modules::Effect,
    target_id: Option<i64>,
) -> Result<(crate::modules::Effect, Option<i64>), crate::service::ServiceError> {
    if effect.effect_id != crate::bubble::EFFECT_FIELD {
        return Ok((effect, target_id));
    }
    let Some(script) = script_effect_of(w, ship_id, flag_id) else {
        return Ok((effect, None));
    };
    if target_id.is_none() {
        return Err(crate::modules::user_error(
            "DeniedActivateTargetNotPresent",
            vec![],
        ));
    }
    Ok((
        crate::modules::Effect {
            category: crate::modules::CATEGORY_TARGET,
            guid: if script == FOCUSED_SCRAMBLE {
                "effects.WarpScramble"
            } else {
                "effects.WarpDisrupt"
            },
            ..effect
        },
        target_id,
    ))
}

/// The effect a running module acts as: a focused field generator is its
/// script's (6848 or 6849), anything else its own.
pub fn effective(w: &Registry, r: &Running, d: &dyn Data) -> i64 {
    if is_focused(&r.effect) {
        d.script_effect(w, r).unwrap_or(r.effect.effect_id)
    } else {
        r.effect.effect_id
    }
}

/// One tackle effect that counts against a ship now, and its strength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Source {
    pub running: Running,
    /// What it acts as ([`effective`]): the id the rule and the icon read.
    pub effect_id: i64,
    /// The source module's modified 105 (0 when nothing names it).
    pub strength: f64,
}

/// Whether `r` is aimed at `target` and holding at `now`.
///
/// `Modules` is advanced lazily, so an entry can outlive what ended it by
/// up to a tick; each test here is one `modules::advance` would end it for
/// at its next boundary, checked now so a guard never counts a dead one:
///
/// - a deactivation that has taken hold (`stop_at <= now`, §7.1);
/// - the source pilot no longer in that ship in space, or in another
///   system;
/// - the lock gone (`modules.rs`, the `target_lost` branch);
/// - **[A]** the target beyond the module's modified `maxRange` (54),
///   centre to centre (§2). Falsified by a live client whose scram holds
///   beyond its range.
fn holds(
    w: &Registry,
    r: &Running,
    target: &crate::space_state::Presence,
    now: i64,
    d: &dyn Data,
) -> bool {
    if r.stop_at.is_some_and(|s| s <= now) {
        return false;
    }
    let Some(source) = w.roster().get(r.character_id) else {
        return false;
    };
    if source.ship_id != r.ship_id || source.solar_system_id != target.solar_system_id {
        return false;
    }
    if !w.locks().targets_of(r.ship_id).contains(&target.ship_id) {
        return false;
    }
    // A focused script reaches `warpScrambleRange`, not `maxRange`.
    let range_attr = if is_focused(&r.effect) {
        ATTR_FIELD_RANGE
    } else {
        ATTR_MAX_RANGE
    };
    match d.module_attr(w, r, range_attr) {
        Some(range) if range > 0.0 => {
            crate::space_state::distance(source.motion.position, target.motion.position) <= range
        }
        _ => true,
    }
}

/// Every effect in `effects` running against `ship_id` and holding at
/// `now`, with its strength (`strength_attr` of the source module).
fn sources_of(
    w: &Registry,
    ship_id: i64,
    now: i64,
    effects: &[i64],
    strength_attr: i64,
    strength_default: f64,
    d: &dyn Data,
) -> Vec<Source> {
    let Some(target) = w.roster().by_ship(ship_id).copied() else {
        return Vec::new();
    };
    // UNDOCK-INVULN: nothing holds a protected ship.
    if crate::invuln::ship_protected(w, ship_id, now) {
        return Vec::new();
    }
    w.modules()
        .running_against(ship_id)
        .into_iter()
        .filter_map(|r| {
            let effect_id = effective(w, &r, d);
            effects.contains(&effect_id).then_some((r, effect_id))
        })
        .filter(|(r, _)| holds(w, r, &target, now, d))
        .map(|(r, effect_id)| Source {
            strength: d
                .module_attr(w, &r, strength_attr)
                .unwrap_or(strength_default),
            effect_id,
            running: r,
        })
        .collect()
}

/// The player tackle effects holding against `ship_id` at `now`.
pub fn sources(w: &Registry, ship_id: i64, now: i64, d: &dyn Data) -> Vec<Source> {
    sources_of(w, ship_id, now, PLAYER_EFFECTS, ATTR_STRENGTH, 0.0, d)
}

/// The sum of what is aimed at `ship_id`: Σ 105, no penalty, no cap (§2).
pub fn incoming(w: &Registry, ship_id: i64, now: i64, d: &dyn Data) -> f64 {
    sources(w, ship_id, now, d).iter().map(|s| s.strength).sum()
}

/// The ship's own 104: 0 when nothing models it (the attribute's default).
fn own_status(w: &Registry, ship_id: i64, d: &dyn Data) -> f64 {
    d.ship_attr(w, ship_id, ATTR_STATUS).unwrap_or(0.0)
}

/// **`warpScrambleStatus`** of `ship_id` at `now` as the players make it:
/// its own value plus [`incoming`] (§2). The NPC term is
/// `crate::npc_ewar::incoming`: its guard and stage add it to this, and
/// [`reconcile`] adds it to the 104 it sends (FX8).
pub fn status(w: &Registry, ship_id: i64, now: i64, d: &dyn Data) -> f64 {
    own_status(w, ship_id, d) + incoming(w, ship_id, now, d)
}

/// `UserError('WarpScrambled', {})` [V] (§3).
pub fn warp_refusal() -> Refusal {
    Refusal {
        user_error: WARP_SCRAMBLED,
        args: Vec::new(),
    }
}

/// [`scrammed`] at `now`, over `d`.
pub fn scrammed_by(w: &Registry, character_id: i64, now: i64, d: &dyn Data) -> Option<Refusal> {
    let ship_id = w.roster().get(character_id)?.ship_id;
    (status(w, ship_id, now, d) > 0.0).then(warp_refusal)
}

/// **The `WARP_GUARDS` row**: `WarpScrambled` while the pilot's ship's
/// [`status`] is `> 0`. A pilot not in space is never scrammed. The guard
/// carries no clock, so it reads the wall clock, as `jump.rs` does.
pub fn scrammed(w: &Registry, character_id: i64) -> Option<Refusal> {
    scrammed_by(w, character_id, crate::pyutil::filetime_now(), &Live)
}

/// The focused scripts holding against `ship_id` at `now`, each with its
/// `gateScrambleStrength` (1974).
pub fn gate_sources(w: &Registry, ship_id: i64, now: i64, d: &dyn Data) -> Vec<Source> {
    sources_of(
        w,
        ship_id,
        now,
        GATE_EFFECTS,
        ATTR_GATE_STRENGTH,
        GATE_STRENGTH_DEFAULT,
        d,
    )
}

/// [`gate_scrambled`] at `now`, over `d`.
pub fn gate_scrambled_by(
    w: &Registry,
    character_id: i64,
    now: i64,
    d: &dyn Data,
) -> Option<Refusal> {
    let ship_id = w.roster().get(character_id)?.ship_id;
    // 1973's default is -1000 and only capital hulls carry 0 (§5); a ship
    // nothing models reads the default.
    let own = d
        .ship_attr(w, ship_id, ATTR_GATE_STATUS)
        .unwrap_or(GATE_STATUS_DEFAULT);
    let incoming: f64 = gate_sources(w, ship_id, now, d)
        .iter()
        .map(|s| s.strength)
        .sum();
    (own + incoming > 0.0).then(|| Refusal {
        user_error: GATE_SCRAMBLED,
        args: Vec::new(),
    })
}

/// `gateScrambleStatus`'s default [V] (`dogmaattributes[1973]`, §1.1).
pub const GATE_STATUS_DEFAULT: f64 = -1000.0;

/// **The `JUMP_GUARDS` row**: `GateScrambled` while `gateScrambleStatus`
/// is `> 0` (§5). A plain scram never raises it; a HIC with a focused
/// script (HIC-SCRIPT) does, for a hull whose own value is 0 (the
/// capitals).
pub fn gate_scrambled(w: &Registry, character_id: i64) -> Option<Refusal> {
    gate_scrambled_by(w, character_id, crate::pyutil::filetime_now(), &Live)
}

/// Whether `type_id` requires one of [`BLOCKED_SKILLS`] directly.
fn is_blockable(w: &Registry, type_id: i64, d: &dyn Data) -> bool {
    crate::dogma::attr::REQUIRED_SKILLS
        .iter()
        .filter_map(|a| d.type_attr(w, type_id, *a))
        .any(|skill| BLOCKED_SKILLS.contains(&(skill as i64)))
}

/// [`activation_blocked`] over `d`.
pub fn activation_blocked_by(
    w: &Registry,
    ship_id: i64,
    type_id: i64,
    now: i64,
    d: &dyn Data,
) -> Option<Refusal> {
    if !is_blockable(w, type_id, d) {
        return None;
    }
    let blocked: f64 = sources(w, ship_id, now, d)
        .iter()
        .filter(|s| BLOCKING_EFFECTS.contains(&s.effect_id))
        .map(|s| {
            d.module_attr(w, &s.running, ATTR_BLOCK_STRENGTH)
                .unwrap_or(0.0)
        })
        .sum();
    // The client's own pre-check is `!= 0` [V] (`fittingDogmaLocation:612`).
    (blocked != 0.0).then(|| Refusal {
        user_error: MODULE_IS_BLOCKED,
        args: crate::pyutil::pypairs(vec![("moduleName", crate::modules::type_arg(type_id))]),
    })
}

/// **The activation check** `modules::activate` makes: `ModuleIsBlocked`
/// with `moduleName = (UE_TYPEID, typeID)` [V] for a module requiring
/// [`BLOCKED_SKILLS`] on a ship under a running scrambler (5934, never a
/// disruptor, §4.1).
pub fn activation_blocked(w: &Registry, ship_id: i64, type_id: i64, now: i64) -> Option<Refusal> {
    activation_blocked_by(w, ship_id, type_id, now, &Live)
}

// ---------------------------------------------------------------------------
// What the target's client is told
// ---------------------------------------------------------------------------

/// `jammingType` for an effect [V] (`dogma/effects/__init__.py:181`,
/// `:187`; §6.2). `None` is never sent: the client logs it as an error.
pub fn jamming_type(effect_id: i64) -> Option<&'static str> {
    match effect_id {
        DISRUPT | FOCUSED_DISRUPT => Some("warpScrambler"),
        SCRAMBLE | FOCUSED_SCRAMBLE => Some("warpScramblerMWD"),
        _ => None,
    }
}

/// `jammingType` of an incoming **assistance** effect [V]
/// (`dogma/effects/__init__.py:213-231`): the buff-bar icon of a remote
/// shield, armour, hull or capacitor transfer aimed at the own ship.
pub fn assist_slot(effect_id: i64) -> Option<&'static str> {
    match effect_id {
        6_186 | 6_652 => Some("shieldTransfer"),
        6_188 | 6_651 => Some("remoteArmorRepair"),
        6_185 => Some("remoteHullRepair"),
        6_184 => Some("energyTransfer"),
        _ => None,
    }
}

/// `OnEwarStart`/`OnEwarEnd(sourceBallID, moduleID, targetBallID,
/// jammingType)` [V shape] (`tactical.py:1041`, `:1063`): the buff-bar
/// icon. Server-sent [I] (no local emitter, §6.2). **[A]** to the target
/// alone, the only receiver whose handler acts (`tactical.py:1051`).
#[derive(Debug, Clone, PartialEq)]
pub struct Ewar {
    pub to: i64,
    pub source_ball: i64,
    pub module_id: i64,
    pub target_ball: i64,
    pub jamming: &'static str,
    pub start: bool,
}

impl Push for Ewar {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            if self.start {
                "OnEwarStart"
            } else {
                "OnEwarEnd"
            },
            vec![
                PyValue::Int(self.source_ball),
                PyValue::Int(self.module_id),
                PyValue::Int(self.target_ball),
                crate::pyutil::bstr(self.jamming),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// One icon on the target's buff bar: `(sourceBallID, moduleID,
/// jammingType)`, the key `tactical` files it under.
type Icon = (i64, i64, &'static str);

/// What one target's client was last told.
#[derive(Debug, Clone, Default, PartialEq)]
struct Told {
    character_id: i64,
    /// The 104 it was last sent.
    status: f64,
    icons: BTreeSet<Icon>,
    /// Source module -> the cycles already counted as aggression.
    cycles: BTreeMap<i64, i64>,
}

/// **The slot**: per target ship, what its pilot's client was last told.
/// Built empty (no deltas: none of it outlives a restart, which empties
/// every park and every running effect).
#[derive(Debug, Default)]
pub struct Tackled {
    ships: BTreeMap<i64, Told>,
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "tackle",
    make: |_| Box::new(Tackled::default()) as Boxed,
};

/// **The world clock's stage**, after `modules`: [`reconcile`] on the
/// server's own data.
pub fn tick(w: &mut Registry, _: std::time::Instant, now: crate::damage::BlueTime) {
    reconcile(w, now, &Live);
}

/// The `(key, dict)` a stopped module's `OnGodmaShipEffect` carries.
fn blocked_error(type_id: i64) -> PyValue {
    PyValue::Tuple(vec![
        crate::pyutil::bstr(MODULE_IS_BLOCKED),
        PyValue::Dict(crate::pyutil::pypairs(vec![(
            "moduleName",
            crate::modules::type_arg(type_id),
        )])),
    ])
}

/// Bring every tackled pilot's client up to `now`, over `d`.
///
/// For each ship in space with a tackle source, or that had one last time:
///
/// - `OnEwarStart` for each icon that appeared, `OnEwarEnd` for each that
///   went (§6.2);
/// - `OnModuleAttributeChange` for 104 when the status moved (§6.3: the
///   autopilot reads it off godma);
/// - under a scrambler, every running module of the ship that requires
///   [`BLOCKED_SKILLS`] stops at once with `ModuleIsBlocked` (§4.1 [A]:
///   falsified by a live client whose MWD keeps cycling under a scram);
/// - every new cycle of a source is aggression by its pilot against the
///   target's [I] (`isOffensive`, §6.1): the weapons timer that keeps a
///   tackler on the gate.
///
/// All of it on the clock: nobody's call is being answered.
pub fn reconcile(w: &mut Registry, now: i64, d: &dyn Data) {
    // TICK-1: a ship that no player tackle effect is aimed at and that was
    // not told about one last time computes nothing below (`found` empty and
    // not in `told`: the loop `continue`s), so only the ships a tackle effect
    // names, and the ones told before, are visited -- the same ships, in the
    // same order, without a roster walk and a module walk for every ship.
    let aimed: BTreeSet<i64> = w
        .modules()
        .running_all()
        .iter()
        .filter(|r| PLAYER_EFFECTS.contains(&r.effect.effect_id) || is_focused(&r.effect))
        .filter_map(|r| r.target_id)
        .collect();
    let mut told = std::mem::take(&mut w.ext_mut::<Tackled>().ships);
    // A ship that left space (docked, jumped, died, logged off) has its
    // icons ended and is forgotten. No 104: the session change re-primes
    // godma (`GetAllInfo`) with the ship as it now is.
    let gone: Vec<i64> = told
        .keys()
        .copied()
        .filter(|ship| w.roster().by_ship(*ship).is_none())
        .collect();
    for ship in gone {
        if let Some(t) = told.remove(&ship) {
            tell(w, t.character_id, ship, &t, &BTreeSet::new(), None, now);
        }
    }
    let ships: BTreeSet<(i64, i64)> = aimed
        .iter()
        .chain(told.keys())
        .filter_map(|s| w.roster().by_ship(*s))
        .map(|p| (p.ship_id, p.character_id))
        .collect();
    for (ship, character) in ships {
        let found = sources(w, ship, now, d);
        if found.is_empty() && !told.contains_key(&ship) {
            continue;
        }
        let own = own_status(w, ship, d);
        let before = told.remove(&ship).unwrap_or_else(|| Told {
            character_id: character,
            status: own,
            ..Told::default()
        });
        // The whole 104, NPC scrams included (FX8; CP49's "Not done"): the
        // `npc_ewar` stage sends the same total, so a player and an NPC on
        // one ship no longer leave the client the player-only value.
        // `status` stays player-only: `npc_ewar` adds its own term to it.
        let npc = crate::npc_ewar::incoming(w, ship, now);
        let mut after = Told {
            character_id: character,
            status: own + found.iter().map(|s| s.strength).sum::<f64>() + npc,
            ..Told::default()
        };
        for s in &found {
            let r = s.running;
            if let Some(jamming) = jamming_type(s.effect_id) {
                after.icons.insert((r.ship_id, r.module_id, jamming));
            }
            after.cycles.insert(r.module_id, r.cycles);
            if before
                .cycles
                .get(&r.module_id)
                .is_none_or(|n| *n < r.cycles)
            {
                crate::crimewatch::aggression(w, r.character_id, character, now, now);
                crate::npc_threat::on_act(w, r.character_id, character, 0.0, now);
            }
        }
        tell(
            w,
            character,
            ship,
            &before,
            &after.icons,
            Some(after.status),
            now,
        );
        if found
            .iter()
            .any(|s| BLOCKING_EFFECTS.contains(&s.effect_id))
        {
            stop_blocked(w, ship, now, d);
        }
        if !found.is_empty() {
            told.insert(ship, after);
        }
    }
    w.ext_mut::<Tackled>().ships = told;
}

/// The pushes that take `to`'s client from `before` to `icons` and, when
/// `status` is `Some`, to that 104.
fn tell(
    w: &mut Registry,
    to: i64,
    ship: i64,
    before: &Told,
    icons: &BTreeSet<Icon>,
    status: Option<f64>,
    now: i64,
) {
    let started = icons.difference(&before.icons).map(|i| (*i, true));
    let ended = before.icons.difference(icons).map(|i| (*i, false));
    let changes: Vec<(Icon, bool)> = started.chain(ended).collect();
    for ((source_ball, module_id, jamming), start) in changes {
        w.schedule(
            Duration::ZERO,
            Ewar {
                to,
                source_ball,
                module_id,
                target_ball: ship,
                jamming,
                start,
            },
        );
    }
    if let Some(new) = status.filter(|s| (s - before.status).abs() > f64::EPSILON) {
        w.schedule(
            Duration::ZERO,
            crate::modules::ModulePush::Attribute(crate::modules::AttributeChange {
                to: vec![to],
                owner_id: to,
                item_id: ship,
                attribute_id: ATTR_STATUS,
                time: now,
                new_value: new,
                old_value: before.status,
            }),
        );
    }
}

/// Stop every running module on `ship` that [`activation_blocked_by`]
/// would refuse now.
fn stop_blocked(w: &mut Registry, ship: i64, now: i64, d: &dyn Data) {
    let fitted: Vec<(i64, i64)> = w
        .store()
        .contents_of(ship)
        .map(|i| (i.item_id, i.type_id))
        .collect();
    for (module_id, type_id) in fitted {
        let running = w
            .modules()
            .running(module_id)
            .is_some_and(|r| r.stop_at.is_none());
        if running && activation_blocked_by(w, ship, type_id, now, d).is_some() {
            crate::modules::stop(w, module_id, now, Some(blocked_error(type_id)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::modules::Effect;
    use crate::service::Request;

    // Invented ids and round fixture values (lane rule): the shapes are
    // R-TACKLE §1.3's, never its numbers.
    const ALICE: i64 = 1_570_001;
    const BOB: i64 = 1_570_002;
    const CAROL: i64 = 1_570_003;
    const ALICE_SHIP: i64 = 157_000_001;
    const BOB_SHIP: i64 = 157_000_002;
    const CAROL_SHIP: i64 = 157_000_003;
    const BOB_POINT: i64 = 157_100_001;
    const CAROL_POINT: i64 = 157_100_002;
    const ALICE_MWD: i64 = 157_100_003;

    /// Fixture types: a strength-1 disruptor, a strength-2 scrambler that
    /// blocks with 1, a third source of strength 1, and a module that
    /// requires 3454 (an MWD) and one that requires something else.
    const DISRUPTOR: i64 = 990_570_001;
    const SCRAMBLER: i64 = 990_570_002;
    const WEAK: i64 = 990_570_003;
    const MWD: i64 = 990_570_004;
    const OTHER: i64 = 990_570_005;
    const MJD: i64 = 990_570_006;
    const RANGE: f64 = 10_000.0;

    /// Hull stability per ship: Alice's is a parameter of the fixture.
    struct Fixture {
        alice_status: f64,
    }

    impl Data for Fixture {
        fn ship_attr(&self, _: &Registry, ship_id: i64, attribute_id: i64) -> Option<f64> {
            (ship_id == ALICE_SHIP && attribute_id == ATTR_STATUS).then_some(self.alice_status)
        }
        fn module_attr(&self, w: &Registry, r: &Running, attribute_id: i64) -> Option<f64> {
            self.type_attr(w, r.type_id, attribute_id)
        }
        fn type_attr(&self, _: &Registry, type_id: i64, attribute_id: i64) -> Option<f64> {
            match (type_id, attribute_id) {
                (DISRUPTOR, ATTR_STRENGTH) => Some(1.0),
                (SCRAMBLER, ATTR_STRENGTH) => Some(2.0),
                (SCRAMBLER, ATTR_BLOCK_STRENGTH) => Some(1.0),
                (WEAK, ATTR_STRENGTH) => Some(1.0),
                (DISRUPTOR | SCRAMBLER | WEAK, ATTR_MAX_RANGE) => Some(RANGE),
                (MWD, 182) => Some(SKILL_HIGH_SPEED_MANEUVERING as f64),
                (MJD, 183) => Some(SKILL_MICRO_JUMP_DRIVE as f64),
                (OTHER, 182) => Some(3300.0),
                _ => None,
            }
        }
    }

    const HULL_ZERO: Fixture = Fixture { alice_status: 0.0 };

    fn effect(effect_id: i64) -> Effect {
        Effect {
            effect_id,
            name: "fixture",
            category: crate::modules::CATEGORY_TARGET,
            guid: "",
            is_offensive: true,
            duration_attribute: 73,
            warp_safe: None,
        }
    }

    /// Alice, Bob and Carol in one system, Bob and Carol 1 km from Alice,
    /// each locking Alice, Alice's MWD fitted.
    fn world() -> Registry {
        let ch = |id: i64, ship: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                ch(ALICE, ALICE_SHIP, "Alice"),
                ch(BOB, BOB_SHIP, "Bob"),
                ch(CAROL, CAROL_SHIP, "Carol"),
            ],
            ..SandboxConfig::default()
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items: vec![crate::item::Item {
                item_id: ALICE_MWD,
                type_id: MWD,
                owner_id: ALICE,
                location_id: ALICE_SHIP,
                flag_id: 19,
                quantity: 1,
                group_id: 46,
                category_id: 7,
                singleton: true,
            }],
            next_item_id: ALICE_MWD + 100,
            ..crate::store::Snapshot::default()
        });
        for c in [ALICE, BOB, CAROL] {
            crate::destiny::on_ballpark_bind(&mut reg, Some(c));
        }
        let origin = reg.roster().get(ALICE).expect("in space").motion.position;
        for (c, dx) in [(ALICE, 0.0), (BOB, 1_000.0), (CAROL, -1_000.0)] {
            let p = reg.roster_mut().get_mut(c).expect("in space");
            p.motion = crate::space_state::Motion::arrived(
                [origin[0] + dx, origin[1], origin[2]],
                [0.0; 3],
            );
            p.since = crate::space_state::now_seconds();
        }
        reg.locks_mut().add(BOB_SHIP, ALICE_SHIP);
        reg.locks_mut().add(CAROL_SHIP, ALICE_SHIP);
        let _ = reg.take_pending();
        let _ = reg.take_due(far());
        reg
    }

    fn far() -> std::time::Instant {
        std::time::Instant::now() + Duration::from_secs(3_600)
    }

    /// `(character, ship, module)` runs a module of `type_id` with
    /// `effect_id` against `target`, started a moment before `now`.
    fn run(
        reg: &mut Registry,
        who: (i64, i64, i64),
        type_id: i64,
        effect_id: i64,
        target: i64,
        now: i64,
    ) {
        let (character_id, ship_id, module_id) = who;
        reg.modules_mut().insert_running(Running {
            character_id,
            ship_id,
            module_id,
            type_id,
            effect: effect(effect_id),
            target_id: Some(target),
            started: now - crate::modules::MSEC,
            period: 5_000 * crate::modules::MSEC,
            repeat: 1000,
            cycles: 1,
            stop_at: None,
        });
    }

    fn bob_scrams(reg: &mut Registry, now: i64) {
        run(
            reg,
            (BOB, BOB_SHIP, BOB_POINT),
            SCRAMBLER,
            SCRAMBLE,
            ALICE_SHIP,
            now,
        );
    }

    fn now() -> i64 {
        crate::pyutil::filetime_now()
    }

    /// The WARP_GUARDS row over the fixture, as `movement_guard` calls it.
    fn guard(w: &Registry, c: i64) -> Option<Refusal> {
        scrammed_by(w, c, now(), &HULL_ZERO)
    }

    fn gate_guard(w: &Registry, c: i64) -> Option<Refusal> {
        gate_scrambled_by(w, c, now(), &HULL_ZERO)
    }

    fn warp_to_bob(reg: &mut Registry) -> Option<Refusal> {
        // Bob is 1 km off: far enough to be "a warp" is not the question
        // here, only whether the guard lets it start.
        let req = Request {
            service: "beyonce",
            method: "CmdWarpToStuff",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        crate::service::warp_guarded(
            reg,
            &req,
            crate::space_state::WARP_SUBJECT_CHAR.to_vec(),
            Some(&PyValue::Int(BOB)),
            0.0,
            &[guard],
        )
    }

    /// The core rule, through the warp's own guard check: a scrambler whose
    /// strength beats the hull's stability refuses the warp with
    /// `WarpScrambled` and no args [V], before anything moves; stopped, it
    /// lets the ship go.
    #[test]
    fn a_scram_stronger_than_the_hull_refuses_warp_with_warp_scrambled() {
        let mut reg = world();
        let t = now();
        assert_eq!(guard(&reg, ALICE), None, "nothing aimed at Alice yet");
        bob_scrams(&mut reg, t);
        let before = reg.roster().get(ALICE).expect("in space").motion;
        let refusal = warp_to_bob(&mut reg).expect("a scrammed warp is refused");
        assert_eq!(refusal.user_error, WARP_SCRAMBLED);
        assert!(refusal.args.is_empty(), "WarpScrambled has no args");
        // What `CmdWarpToStuff` answers with: the client's `UserError`.
        assert_eq!(
            refusal.clone().into_error(),
            crate::service::ServiceError::Raise(eve_proto::user_error(WARP_SCRAMBLED, vec![]))
        );
        assert_eq!(
            reg.roster().get(ALICE).expect("in space").motion.position,
            before.position,
            "a refused warp moved the ship"
        );
        assert!(reg.take_pending().is_empty(), "a refused warp pushed");

        // A deactivation that has taken hold counts for nothing (§7.1).
        crate::modules::stop(&mut reg, BOB_POINT, t, None);
        assert_eq!(guard(&reg, ALICE), None);
    }

    /// The sum, strictly greater than zero, no penalty: a stability the
    /// source exactly matches lets the ship warp; one more point of
    /// strength from a second pilot refuses it (§2).
    #[test]
    fn strengths_add_and_equal_to_the_stability_still_warps() {
        let mut reg = world();
        let t = now();
        let stable = Fixture { alice_status: -2.0 };
        bob_scrams(&mut reg, t);
        assert_eq!(status(&reg, ALICE_SHIP, t, &stable), 0.0);
        assert_eq!(scrammed_by(&reg, ALICE, t, &stable), None, "0 is not > 0");
        run(
            &mut reg,
            (CAROL, CAROL_SHIP, CAROL_POINT),
            WEAK,
            DISRUPT,
            ALICE_SHIP,
            t,
        );
        assert_eq!(incoming(&reg, ALICE_SHIP, t, &stable), 3.0);
        assert_eq!(
            scrammed_by(&reg, ALICE, t, &stable),
            Some(warp_refusal()),
            "two sources sum"
        );
        // A lower strength than the stability: Carol's disruptor alone.
        crate::modules::stop(&mut reg, BOB_POINT, t, None);
        assert_eq!(scrammed_by(&reg, ALICE, t, &stable), None);
    }

    /// A scram does not stop a stargate jump [I] (§5): the jump guard lets
    /// the same scrambled pilot through, while the warp guard refuses.
    #[test]
    fn a_scram_does_not_refuse_a_gate_jump() {
        let mut reg = world();
        bob_scrams(&mut reg, now());
        assert!(crate::movement_guard::check(&[guard], &reg, ALICE).is_some());
        assert_eq!(
            crate::movement_guard::check(&[gate_guard], &reg, ALICE),
            None
        );
        assert_eq!(
            crate::movement_guard::check(&crate::module::plan().jump_guards, &reg, ALICE),
            None
        );
    }

    /// A source counts only while it holds: not once its pilot has left
    /// the ship's lock, not beyond its range [A], not when aimed elsewhere.
    #[test]
    fn a_source_counts_only_while_it_holds() {
        let t = now();
        let mut reg = world();
        bob_scrams(&mut reg, t);
        reg.locks_mut().remove(BOB_SHIP, ALICE_SHIP);
        assert_eq!(incoming(&reg, ALICE_SHIP, t, &HULL_ZERO), 0.0, "lock lost");

        let mut reg = world();
        bob_scrams(&mut reg, t);
        let p = reg.roster_mut().get_mut(BOB).expect("in space");
        p.motion.position[0] += 2.0 * RANGE;
        assert_eq!(
            incoming(&reg, ALICE_SHIP, t, &HULL_ZERO),
            0.0,
            "out of range"
        );

        let mut reg = world();
        bob_scrams(&mut reg, t);
        assert_eq!(incoming(&reg, CAROL_SHIP, t, &HULL_ZERO), 0.0, "not Carol");
        assert_eq!(incoming(&reg, ALICE_SHIP, t, &HULL_ZERO), 2.0);
    }

    /// A scrambler blocks a module that requires High Speed Maneuvering,
    /// with `ModuleIsBlocked` and `moduleName = (UE_TYPEID, typeID)` [V];
    /// a disruptor blocks nothing; a module requiring another skill is
    /// never blocked.
    #[test]
    fn a_scrambler_blocks_the_mwd_and_a_disruptor_does_not() {
        let t = now();
        let mut reg = world();
        run(
            &mut reg,
            (CAROL, CAROL_SHIP, CAROL_POINT),
            DISRUPTOR,
            DISRUPT,
            ALICE_SHIP,
            t,
        );
        assert_eq!(
            activation_blocked_by(&reg, ALICE_SHIP, MWD, t, &HULL_ZERO),
            None
        );
        bob_scrams(&mut reg, t);
        let r = activation_blocked_by(&reg, ALICE_SHIP, MWD, t, &HULL_ZERO).expect("blocked");
        assert_eq!(r.user_error, MODULE_IS_BLOCKED);
        assert_eq!(
            r.args,
            crate::pyutil::pypairs(vec![("moduleName", crate::modules::type_arg(MWD))])
        );
        assert!(activation_blocked_by(&reg, ALICE_SHIP, MJD, t, &HULL_ZERO).is_some());
        assert_eq!(
            activation_blocked_by(&reg, ALICE_SHIP, OTHER, t, &HULL_ZERO),
            None
        );
    }

    type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

    fn pushes(reg: &mut Registry) -> Vec<Sent> {
        reg.take_due(far())
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (name, args, p.audience())
            })
            .collect()
    }

    /// The clock tells the target: `OnEwarStart` with the scrambler's
    /// `jammingType`, the new 104, and its running MWD stopped with
    /// `ModuleIsBlocked`; once the scram ends, `OnEwarEnd` and 104 back.
    /// A second tick with nothing changed says nothing.
    #[test]
    fn the_target_is_told_when_a_scram_starts_and_ends() {
        let t = now();
        let mut reg = world();
        run(
            &mut reg,
            (ALICE, ALICE_SHIP, ALICE_MWD),
            MWD,
            6730,
            BOB_SHIP,
            t,
        );
        bob_scrams(&mut reg, t);
        reconcile(&mut reg, t, &HULL_ZERO);
        let sent = pushes(&mut reg);
        let ewar = sent
            .iter()
            .find(|(n, ..)| *n == "OnEwarStart")
            .expect("OnEwarStart");
        assert_eq!(
            ewar.1,
            vec![
                PyValue::Int(BOB_SHIP),
                PyValue::Int(BOB_POINT),
                PyValue::Int(ALICE_SHIP),
                crate::pyutil::bstr("warpScramblerMWD"),
            ]
        );
        assert_eq!(ewar.2, Some(vec![ALICE]));
        let attr = sent
            .iter()
            .find(|(n, a, _)| {
                *n == "OnModuleAttributeChange" && a.get(2) == Some(&PyValue::Int(ATTR_STATUS))
            })
            .expect("104 told");
        assert_eq!(attr.1[4], PyValue::Real(2.0));
        assert!(
            reg.modules().running(ALICE_MWD).is_none(),
            "the MWD stopped"
        );
        assert!(
            sent.iter().any(|(n, a, _)| *n == "OnGodmaShipEffect"
                && a[0] == PyValue::Int(ALICE_MWD)
                && a[9] == blocked_error(MWD)),
            "the MWD's stop says why"
        );

        reconcile(&mut reg, t, &HULL_ZERO);
        assert!(pushes(&mut reg).is_empty(), "nothing changed, nothing said");

        crate::modules::stop(&mut reg, BOB_POINT, t, None);
        let _ = pushes(&mut reg);
        reconcile(&mut reg, t, &HULL_ZERO);
        let sent = pushes(&mut reg);
        assert!(
            sent.iter()
                .any(|(n, a, _)| *n == "OnEwarEnd"
                    && a[3] == crate::pyutil::bstr("warpScramblerMWD"))
        );
        let attr = sent
            .iter()
            .find(|(n, _, _)| *n == "OnModuleAttributeChange")
            .expect("104 told back");
        assert_eq!(attr.1[4], PyValue::Real(0.0));
        assert_eq!(attr.1[5], PyValue::Real(2.0));
    }

    /// **FX8**: a player point and an NPC scram on one ship: the 104 this
    /// stage sends is the whole sum, the NPC's strength included, as the
    /// `npc_ewar` stage's is (CP49's "Not done").
    #[test]
    fn the_104_sent_includes_an_npc_scram_on_the_same_ship() {
        let t = now();
        let mut reg = world();
        let p = reg.roster().get(ALICE).expect("in space");
        let (system, at) = (p.solar_system_id, p.motion.position);
        let id = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, system, at).expect("an id");
        assert!(reg.locks_mut().add(id, ALICE_SHIP));
        let npc = *reg.npcs().get(id).expect("spawned");
        let scram = crate::npc_ewar::Effect {
            kind: crate::npc_ewar::Kind::Scram,
            range: RANGE,
            strength: 2.0,
            period: 5_000 * crate::modules::MSEC,
            chance: 1.0,
        };
        crate::npc_ewar::cycle(&mut reg, &npc, &scram, ALICE_SHIP, t);
        let npc_term = crate::npc_ewar::incoming(&reg, ALICE_SHIP, t);
        assert_eq!(npc_term, scram.strength, "the NPC holds");
        run(
            &mut reg,
            (BOB, BOB_SHIP, BOB_POINT),
            DISRUPTOR,
            DISRUPT,
            ALICE_SHIP,
            t,
        );
        let _ = pushes(&mut reg);
        reconcile(&mut reg, t, &HULL_ZERO);
        let sent = pushes(&mut reg);
        let attr = sent
            .iter()
            .find(|(n, a, _)| {
                *n == "OnModuleAttributeChange" && a.get(2) == Some(&PyValue::Int(ATTR_STATUS))
            })
            .expect("104 told");
        let whole = status(&reg, ALICE_SHIP, t, &HULL_ZERO) + npc_term;
        assert_eq!(attr.1[4], PyValue::Real(whole));
        assert!(whole > status(&reg, ALICE_SHIP, t, &HULL_ZERO));
    }

    /// Each new cycle of a source is aggression: the scrammer, a
    /// capsuleer, gets a weapons timer against a capsuleer.
    #[test]
    fn a_scram_is_aggression() {
        let t = now();
        let mut reg = world();
        bob_scrams(&mut reg, t);
        reconcile(&mut reg, t, &HULL_ZERO);
        assert!(
            crate::crimewatch::refuse_jump(&reg, Some(BOB), t).is_err(),
            "the scrammer has a weapons timer"
        );
    }
}
