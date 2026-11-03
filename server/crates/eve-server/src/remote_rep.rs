//! **Repairs, boosts and capacitor transfers: the local reps and the four
//! remote assistance modules** (K-LOGI). Lane note `docs/lanes/k-logi.md`;
//! marks as `docs/content/README.md` §2.
//!
//! # What a fitted rep did before this lane
//!
//! Every one of them activated, paid its capacitor each cycle, drew its
//! effect and **did nothing to the target's health**: no consumer read the
//! cycle (`combat::on_cycles` throws away a cycle with no damage; only an
//! NPC's own repairer, `npc::brain` -> `combat::self_repair`, ever healed).
//! That includes the local armor repairer and shield booster of a player.
//!
//! # The effects [V]
//!
//! Rows of `dogmaeffects` (`out/typedata.tsv`): the remote four are target
//! effects (`effectCategory` 2, **not** offensive), duration attribute 73,
//! discharge 6, range 54 and falloff **2044** (`falloffEffectiveness`); the
//! client's own tooltip (`moduleButtonTooltip.py`, one `Add*Info` per module
//! group) names what each module does per cycle:
//!
//! | effect | module | per cycle | attribute |
//! |---|---|---|---|
//! | 6186 `shipModuleRemoteShieldBooster` (guid `ShieldTransfer`) | remote shield booster | shield | 68 `shieldBonus` |
//! | 6188 `shipModuleRemoteArmorRepairer` (`RemoteArmourRepair`) | remote armor repairer | armour | 84 `armorDamageAmount` |
//! | 6185 `shipModuleRemoteHullRepairer` (`RemoteHullRepair`) | remote hull repairer | hull | 83 `structureDamageAmount` |
//! | 6184 `shipModuleRemoteCapacitorTransmitter` (`EnergyTransfer`) | remote capacitor transmitter | capacitor | 90 `powerTransferAmount` |
//! | 6652, 6651 | ancillary remote shield booster, armor repairer | as 6186, 6188 | 68, 84 |
//! | 18, 592 | the legacy transporter and projector (QA rows only) | shield, armour | 68, 84; range 87 for 18 |
//! | 4, 27, 26 | local shield booster, armor repairer, hull repairer | own ship | 68, 84, 83 |
//! | 4936, 5275 | local ancillary shield booster, armor repairer | own ship | 68, 84 |
//!
//! An **ancillary armor repairer** multiplies its amount by the module's
//! attribute 1886 `chargedArmorDamageMultiplier` while it has a charge
//! (the tooltip's `chargeMultiplier`) [V]. A module that takes charges does
//! not cycle without one (`modules::advance`) -- except the **ancillary**
//! ones ([`crate::modules::is_ancillary`]), which run empty at the base
//! amount; the multiplier is scaled by the rounds the cycle took
//! (`taken / chargeRate`, [`strength`]).
//!
//! The **capacitor booster** (48) gives its ship the loaded charge's
//! attribute 67 at each cycle start; the charge is already consumed by then,
//! so the amount comes from what the cycle recorded
//! ([`crate::modules::Modules::fired_at`]).
//!
//! **The target's resistance** [V]: the module's attribute 2138
//! `remoteResistanceID` names a target attribute (2116 `remoteRepairImpedance`,
//! default 1.0) that multiplies the amount, as in `ewar`.
//!
//! # The arithmetic
//!
//! - **[A] a cycle lands when it starts**, which is when the capacitor is paid
//!   (`modules::advance`, as a gun's volley does). Falsified by a retail log
//!   whose rep arrives at the cycle's end.
//! - **[I] falloff**: full to `maxRange`, then `0.5^(((d-range)/falloff)^2)`
//!   (`ewar::falloff`, with its `FALLOFF_REACH`). Past that nothing lands and
//!   the cycle is wasted.
//! - The amount is `amount x charge x falloff x resistance`, clipped to the
//!   damage there is ([`crate::damage::ShipDamage::mend`]); a capacitor
//!   transfer clips to the target's capacity.
//!
//! # What the client is told
//!
//! The target's bars to the whole park (`OnDamageStateChange`, as a hit's) and
//! the ring attribute (264, 266 or 3) to its pilot (`combat::RingUpdate`); a
//! capacitor transfer is attribute 18 to the pilot (`modules::give_capacitor`).
//! The beam is the activation's own `OnSpecialFX` (`modules::activate`, the
//! effect's guid). Not sent: the combat-log lines `OnRemote*Source` and
//! `OnRemote*Target` (see the lane note).
//!
//! # Assistance
//!
//! Helping someone is an act (`crimewatch`): `activation_refusal` is the
//! safety's word on a flagged target, called from `modules::activate`
//! (`ModuleActivationDeniedSafetyPrevents*`, the client's own keys), and every
//! cycle that reaches the target is [`crate::crimewatch::assistance`] -- a
//! criminal or suspect target makes the helper one, and the target's weapons,
//! pvp and npc timers are inherited.
//!
//! # What this module holds
//!
//! Only which cycles it has already applied ([`SLOT`], never persisted: a
//! restart empties every running effect). The rest is a read of
//! `Modules::running_all`, as `ewar` does.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::damage::{BlueTime, Layer, attr as dmg_attr};
use crate::modules::Running;
use crate::service::Registry;
use crate::tackle::{Data, Live};
use crate::world_ext::{Boxed, SlotSpec};

/// Local `shieldBoosting`, `structureRepair`, `armorRepair` [V].
pub const SHIELD_BOOSTING: i64 = 4;
pub const STRUCTURE_REPAIR: i64 = 26;
pub const ARMOR_REPAIR: i64 = 27;
/// Local ancillary (`fueled`) shield booster and armor repairer [V].
pub const FUELED_SHIELD_BOOSTING: i64 = 4_936;
pub const FUELED_ARMOR_REPAIR: i64 = 5_275;
/// The local capacitor booster: its amount is the loaded charge's
/// `capacitorBonus` ([`attr::CAPACITOR_BONUS`]) [V: the effect has no modifier
/// rows in the client data; the charge carries the number].
pub const CAP_BOOSTER: i64 = 48;
/// The legacy remote shield transporter and armor projector [V].
pub const SHIELD_TRANSFER: i64 = 18;
pub const TARGET_ARMOR_REPAIR: i64 = 592;
/// The four remote modules and the two ancillary ones [V].
pub const REMOTE_CAPACITOR: i64 = 6_184;
pub const REMOTE_HULL: i64 = 6_185;
pub const REMOTE_SHIELD: i64 = 6_186;
pub const REMOTE_ARMOR: i64 = 6_188;
pub const ANCILLARY_REMOTE_ARMOR: i64 = 6_651;
pub const ANCILLARY_REMOTE_SHIELD: i64 = 6_652;

/// Dogma attributes [V] (`dogmaattributes`).
pub mod attr {
    pub const SHIELD_BONUS: i64 = 68;
    pub const STRUCTURE_AMOUNT: i64 = 83;
    pub const ARMOR_AMOUNT: i64 = 84;
    pub const POWER_TRANSFER: i64 = 90;
    /// `capacitorBonus`, on a cap booster's charge.
    pub const CAPACITOR_BONUS: i64 = 67;
    pub const MAX_RANGE: i64 = 54;
    /// The legacy shield transporter's own range.
    pub const SHIELD_TRANSFER_RANGE: i64 = 87;
    pub const FALLOFF: i64 = 2044;
    /// `chargedArmorDamageMultiplier`.
    pub const CHARGED_ARMOR_MULTIPLIER: i64 = 1_886;
    /// `remoteResistanceID`: on the module, the target attribute that scales
    /// it (2116 `remoteRepairImpedance`).
    pub const RESISTANCE_ID: i64 = 2_138;
}

/// How many cycles one tick will land for one module: a world that stalled
/// for minutes must not turn a rep into a burst.
pub const MAX_CATCH_UP: i64 = 8;

/// `(source ship, module, slot)`: one assistance icon on a buff bar.
type AssistIcon = (i64, i64, &'static str);

/// Per target ship: its pilot and the icons that pilot was last shown.
type AssistIcons = BTreeMap<i64, (i64, BTreeSet<AssistIcon>)>;

/// What a cycle restores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Shield,
    Armour,
    Hull,
    Capacitor,
}

/// One effect's row: what it restores, with which attribute, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    pub kind: Kind,
    /// Aimed at another ship; `false` is the pilot's own.
    pub remote: bool,
    /// The module attribute holding the amount per cycle.
    pub amount: i64,
    /// The module's range attribute (0 for a local effect).
    pub range: i64,
    /// Takes the charge multiplier (the ancillary armor repairers).
    pub charged: bool,
    /// The amount is read off the **loaded charge**, not the module (the
    /// capacitor booster).
    pub from_charge: bool,
}

/// The row of an effect id, `None` for any other.
pub fn spec_of(effect_id: i64) -> Option<Spec> {
    let local = |kind, amount, charged| Spec {
        kind,
        remote: false,
        amount,
        range: 0,
        charged,
        from_charge: false,
    };
    let remote = |kind, amount, range, charged| Spec {
        kind,
        remote: true,
        amount,
        range,
        charged,
        from_charge: false,
    };
    Some(match effect_id {
        SHIELD_BOOSTING | FUELED_SHIELD_BOOSTING => local(Kind::Shield, attr::SHIELD_BONUS, false),
        CAP_BOOSTER => Spec {
            from_charge: true,
            ..local(Kind::Capacitor, attr::CAPACITOR_BONUS, false)
        },
        ARMOR_REPAIR => local(Kind::Armour, attr::ARMOR_AMOUNT, false),
        FUELED_ARMOR_REPAIR => local(Kind::Armour, attr::ARMOR_AMOUNT, true),
        STRUCTURE_REPAIR => local(Kind::Hull, attr::STRUCTURE_AMOUNT, false),
        REMOTE_SHIELD | ANCILLARY_REMOTE_SHIELD => {
            remote(Kind::Shield, attr::SHIELD_BONUS, attr::MAX_RANGE, false)
        }
        SHIELD_TRANSFER => remote(
            Kind::Shield,
            attr::SHIELD_BONUS,
            attr::SHIELD_TRANSFER_RANGE,
            false,
        ),
        REMOTE_ARMOR => remote(Kind::Armour, attr::ARMOR_AMOUNT, attr::MAX_RANGE, false),
        ANCILLARY_REMOTE_ARMOR => remote(Kind::Armour, attr::ARMOR_AMOUNT, attr::MAX_RANGE, true),
        TARGET_ARMOR_REPAIR => remote(Kind::Armour, attr::ARMOR_AMOUNT, attr::MAX_RANGE, false),
        REMOTE_HULL => remote(Kind::Hull, attr::STRUCTURE_AMOUNT, attr::MAX_RANGE, false),
        REMOTE_CAPACITOR => remote(
            Kind::Capacitor,
            attr::POWER_TRANSFER,
            attr::MAX_RANGE,
            false,
        ),
        _ => return None,
    })
}

/// Whether `effect_id` is a rep, boost or transfer, local or remote.
pub fn is_rep(effect_id: i64) -> bool {
    spec_of(effect_id).is_some()
}

/// Whether `effect_id` helps another ship.
pub fn is_remote(effect_id: i64) -> bool {
    spec_of(effect_id).is_some_and(|s| s.remote)
}

// ---------------------------------------------------------------------------
// The state
// ---------------------------------------------------------------------------

/// **The slot**: per module, `(started, cycles)` already applied.
#[derive(Debug, Default)]
pub struct Reps {
    done: BTreeMap<i64, (i64, i64)>,
    /// Per target ship, the `(source ship, module, slot)` icons its pilot's
    /// client was last told (EWAR-ICONS).
    icons: AssistIcons,
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "remote_rep",
    make: |_| Box::new(Reps::default()) as Boxed,
};

// ---------------------------------------------------------------------------
// The safety
// ---------------------------------------------------------------------------

/// **The safety's word on helping**: `Some` when `actor`'s safety forbids
/// activating a remote assistance module of `effect_id` on the ball `target`
/// (a criminal or suspect pilot, or one in a limited engagement), through
/// [`crate::crimewatch::assist_refusal`]. `None` for a local effect, for
/// anything that is not a rep, and for no target.
pub fn activation_refusal(
    w: &Registry,
    actor: i64,
    module_type: i64,
    effect_id: i64,
    target: Option<i64>,
    now: BlueTime,
) -> Option<crate::crimewatch::Refusal> {
    // A remote sensor booster and a remote tracking computer are help too
    // (EWAR-2C).
    let helps = crate::ewar::Kind::of(effect_id).is_some_and(crate::ewar::Kind::is_assist);
    if !is_remote(effect_id) && !helps {
        return None;
    }
    crate::crimewatch::assist_refusal(w, actor, Some(module_type), target?, now)
}

// ---------------------------------------------------------------------------
// The reads
// ---------------------------------------------------------------------------

/// A target's attribute: its own sheet's, else its type's.
fn target_attr(w: &Registry, d: &dyn Data, ship: i64, attribute: i64) -> Option<f64> {
    d.ship_attr(w, ship, attribute).or_else(|| {
        let p = w.roster().by_ship(ship)?;
        let ty = w.pilot(p.character_id)?.ship_type_id;
        d.type_attr(w, ty, attribute)
    })
}

/// What one cycle of `r` does to `target` (a ship in the roster), before
/// the damage there is to repair: `0` out of reach, else
/// `amount x charge x falloff x resistance`. `None` when it cannot land at
/// all (the pilot left the ship or the system).
///
/// `fired` is what the cycle took out of the module
/// ([`crate::modules::Fired`]): a cap booster's amount is its charge's, and
/// an ancillary armor repairer's multiplier scales with `taken / rate`
/// (`base x (1 + (mult - 1) x taken / rate)`, so an empty one repairs the
/// base and a half-fed one in between) [A: the proportional blend; the
/// client's text only says it runs unloaded at reduced effect].
pub fn strength(
    w: &Registry,
    r: &Running,
    spec: Spec,
    target: &crate::space_state::Presence,
    d: &dyn Data,
    fired: Option<crate::modules::Fired>,
) -> Option<f64> {
    let source = w.roster().get(r.character_id)?;
    if source.ship_id != r.ship_id || source.solar_system_id != target.solar_system_id {
        return None;
    }
    let mut amount = if spec.from_charge {
        fired
            .and_then(|f| f.charge_type_id)
            .and_then(|ty| d.type_attr(w, ty, spec.amount))
            .unwrap_or(0.0)
    } else {
        d.module_attr(w, r, spec.amount).unwrap_or(0.0)
    };
    if spec.charged
        && crate::modules::find_in(w.content(), r.type_id)
            .is_some_and(crate::modules::takes_charges)
    {
        let mult = d
            .module_attr(w, r, attr::CHARGED_ARMOR_MULTIPLIER)
            .filter(|m| *m > 0.0)
            .unwrap_or(1.0);
        // No record of the fire (a module run by hand): a full charge.
        let fill = fired.map_or(1.0, |f| f.taken as f64 / f.rate.max(1) as f64);
        amount *= 1.0 + (mult - 1.0) * fill.clamp(0.0, 1.0);
    }
    if !spec.remote {
        return Some(amount);
    }
    let range = d.module_attr(w, r, spec.range).unwrap_or(0.0);
    let fall = d.module_attr(w, r, attr::FALLOFF).unwrap_or(0.0);
    let gap = crate::space_state::distance(source.motion.position, target.motion.position);
    let reach = if range > 0.0 {
        crate::ewar::falloff(gap, range, fall)
    } else {
        1.0
    };
    let resist = d
        .module_attr(w, r, attr::RESISTANCE_ID)
        .filter(|id| *id > 0.0)
        .and_then(|id| target_attr(w, d, target.ship_id, id as i64))
        .unwrap_or(1.0);
    Some(amount * reach * resist.max(0.0))
}

// ---------------------------------------------------------------------------
// The effect
// ---------------------------------------------------------------------------

/// The layer a [`Kind`] heals, `None` for the capacitor.
fn layer_of(kind: Kind) -> Option<Layer> {
    match kind {
        Kind::Shield => Some(Layer::Shield),
        Kind::Armour => Some(Layer::Armour),
        Kind::Hull => Some(Layer::Hull),
        Kind::Capacitor => None,
    }
}

/// Restore up to `amount` to `layer` of `ship`'s record and tell everyone:
/// the bars to the park, the ring to the pilot. The hit points restored; `0`
/// for a ship nothing has hurt (it has no record) or a layer already whole.
pub fn heal(w: &mut Registry, ship: i64, layer: Layer, amount: f64, now: BlueTime) -> f64 {
    let Some(record) = w.combat_mut().ship_mut(ship) else {
        return 0.0;
    };
    let before = record.rings(now);
    let healed = record.mend(layer, amount, now);
    if healed <= 0.0 {
        return 0.0;
    }
    let after = record.rings(now);
    let change = match layer {
        Layer::Shield => (
            dmg_attr::SHIELD_CHARGE,
            before.shield_charge,
            after.shield_charge,
        ),
        Layer::Armour => (
            dmg_attr::ARMOR_DAMAGE,
            before.armor_damage,
            after.armor_damage,
        ),
        Layer::Hull => (dmg_attr::DAMAGE, before.damage, after.damage),
    };
    let Some(here) = w.roster().by_ship(ship).copied() else {
        return healed;
    };
    let mut park = vec![here.character_id];
    park.extend(
        w.roster()
            .others_in_system(here.solar_system_id, here.character_id),
    );
    park.retain(|id| w.character(*id).is_some());
    let bars = crate::combat::damage_state(w, ship, now);
    let stamp = w.next_destiny_stamp();
    if let Some(p) = crate::destiny::DamageStateChange::new(park, stamp, ship, bars) {
        w.schedule(Duration::ZERO, p);
    }
    if w.character(here.character_id).is_some() {
        w.schedule(
            Duration::ZERO,
            crate::combat::RingUpdate {
                to: vec![here.character_id],
                owner_id: here.character_id,
                ship_id: ship,
                change: crate::damage::RingChange {
                    attribute_id: change.0,
                    old: change.1,
                    new: change.2,
                },
            },
        );
    }
    healed
}

/// One cycle of `r`: its amount on its target, and the assistance it is.
fn cycle(w: &mut Registry, r: &Running, spec: Spec, index: i64, now: BlueTime, d: &dyn Data) {
    let ship = match (spec.remote, r.target_id) {
        (false, _) => r.ship_id,
        (true, Some(t)) if t != r.ship_id => t,
        (true, _) => return,
    };
    let Some(target) = w.roster().by_ship(ship).copied() else {
        return;
    };
    let Some(amount) = strength(
        w,
        r,
        spec,
        &target,
        d,
        w.modules().fired_at(r.module_id, index),
    ) else {
        return;
    };
    if amount <= 0.0 {
        return;
    }
    match layer_of(spec.kind) {
        Some(layer) => {
            heal(w, ship, layer, amount, now);
        }
        None => {
            if let Some(push) = crate::modules::give_capacitor(w, ship, amount, now) {
                w.schedule(Duration::ZERO, push);
            }
        }
    }
    if spec.remote {
        crate::crimewatch::assistance(w, r.character_id, ship, now, now);
        crate::npc_threat::assisted(w, r.ship_id, ship, amount, now);
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The world clock's stage**, after `modules`: [`reconcile`] on the
/// server's own data.
pub fn tick(w: &mut Registry, _: std::time::Instant, now: BlueTime) {
    reconcile(w, now, &Live);
}

/// **The incoming-assistance icons** (EWAR-ICONS): per target ship, the
/// remote reps that reach it now, diffed against what its pilot's client was
/// last told and pushed to that pilot alone (`tactical.py:883` acts only for
/// the own ship). A source out of reach or gone is an end.
fn icons(w: &mut Registry, d: &dyn Data) {
    let mut now_icons = AssistIcons::new();
    for r in w.modules().running_all() {
        let (Some(slot), Some(spec), Some(t)) = (
            crate::tackle::assist_slot(r.effect.effect_id),
            spec_of(r.effect.effect_id),
            r.target_id,
        ) else {
            continue;
        };
        if t == r.ship_id {
            continue;
        }
        let Some(target) = w.roster().by_ship(t).copied() else {
            continue;
        };
        if strength(w, &r, spec, &target, d, None).is_some_and(|a| a > 0.0) {
            now_icons
                .entry(t)
                .or_insert_with(|| (target.character_id, BTreeSet::new()))
                .1
                .insert((r.ship_id, r.module_id, slot));
        }
    }
    if now_icons.is_empty() && w.ext::<Reps>().icons.is_empty() {
        return;
    }
    let told = std::mem::take(&mut w.ext_mut::<Reps>().icons);
    let none = BTreeSet::new();
    let ships: BTreeSet<i64> = told.keys().chain(now_icons.keys()).copied().collect();
    for ship in ships {
        let old = told.get(&ship);
        let new = now_icons.get(&ship);
        let Some(to) = new.or(old).map(|(c, _)| *c) else {
            continue;
        };
        // A target that left space has no client in it to tell.
        if w.roster().by_ship(ship).is_none() {
            continue;
        }
        let (a, b) = (old.map_or(&none, |t| &t.1), new.map_or(&none, |t| &t.1));
        let changes = b
            .difference(a)
            .map(|i| (*i, true))
            .chain(a.difference(b).map(|i| (*i, false)));
        for ((source_ball, module_id, jamming), start) in changes.collect::<Vec<_>>() {
            w.schedule(
                Duration::ZERO,
                crate::tackle::Ewar {
                    to,
                    source_ball,
                    module_id,
                    target_ball: ship,
                    jamming,
                    start,
                },
            );
        }
    }
    w.ext_mut::<Reps>().icons = now_icons;
}

/// Land every cycle a rep has started since the last call, over `d`.
pub fn reconcile(w: &mut Registry, now: BlueTime, d: &dyn Data) {
    icons(w, d);
    let running: Vec<(Running, Spec)> = w
        .modules()
        .running_all()
        .into_iter()
        .filter_map(|r| Some((r, spec_of(r.effect.effect_id)?)))
        .collect();
    if running.is_empty() && w.ext::<Reps>().done.is_empty() {
        return;
    }
    let live: BTreeSet<i64> = running.iter().map(|(r, _)| r.module_id).collect();
    w.ext_mut::<Reps>().done.retain(|m, _| live.contains(m));
    for (r, spec) in running {
        let seen = w
            .ext::<Reps>()
            .done
            .get(&r.module_id)
            .filter(|(started, _)| *started == r.started)
            .map_or(0, |(_, cycles)| *cycles);
        if r.cycles <= seen {
            continue;
        }
        let fresh = (r.cycles - seen).min(MAX_CATCH_UP);
        w.ext_mut::<Reps>()
            .done
            .insert(r.module_id, (r.started, r.cycles));
        for i in 0..fresh {
            // The cycle indices run 0.. and `cycles` counts them.
            cycle(w, &r, spec, r.cycles - fresh + i, now, d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::{Damage, Profile, ShipDamage};
    use crate::model::{Character, SandboxConfig};
    use crate::modules::{Effect, MSEC};
    use eve_marshal::PyValue;

    // Invented ids and round fixture values (lane rule): the attribute and
    // effect ids are the client's own [V]; every number below is made up.
    const ALICE: i64 = 1_590_001;
    const BOB: i64 = 1_590_002;
    const CAROL: i64 = 1_590_003;
    const ALICE_SHIP: i64 = 159_000_001;
    const BOB_SHIP: i64 = 159_000_002;
    const CAROL_SHIP: i64 = 159_000_003;
    const ALICE_MOD: i64 = 159_100_001;

    /// Invented module types.
    const RARMOR_T: i64 = 990_590_001;
    const RSHIELD_T: i64 = 990_590_002;
    const RHULL_T: i64 = 990_590_003;
    const RCAP_T: i64 = 990_590_004;
    const LARMOR_T: i64 = 990_590_005;
    const LSHIELD_T: i64 = 990_590_006;

    const RANGE: f64 = 10_000.0;
    const FALL: f64 = 5_000.0;
    const ARMOR_AMOUNT: f64 = 60.0;
    const SHIELD_AMOUNT: f64 = 80.0;
    const HULL_AMOUNT: f64 = 40.0;
    const CAP_AMOUNT: f64 = 25.0;
    /// The target attribute the fixture modules name as their resistance.
    const RESIST_ATTR: i64 = 2116;
    /// The invented ship's layers.
    const SHIELD_HP: f64 = 400.0;
    const ARMOR_HP: f64 = 500.0;
    const HULL_HP: f64 = 300.0;

    struct Fixture {
        /// Bob's value of [`RESIST_ATTR`].
        resist: f64,
    }

    const PLAIN: Fixture = Fixture { resist: 1.0 };

    impl Data for Fixture {
        fn ship_attr(&self, _: &Registry, ship_id: i64, attribute_id: i64) -> Option<f64> {
            (ship_id == BOB_SHIP && attribute_id == RESIST_ATTR).then_some(self.resist)
        }
        fn module_attr(&self, w: &Registry, r: &Running, attribute_id: i64) -> Option<f64> {
            self.type_attr(w, r.type_id, attribute_id)
        }
        fn type_attr(&self, _: &Registry, type_id: i64, attribute_id: i64) -> Option<f64> {
            let remote = matches!(type_id, RARMOR_T | RSHIELD_T | RHULL_T | RCAP_T);
            match (type_id, attribute_id) {
                (_, attr::MAX_RANGE) if remote => Some(RANGE),
                (_, attr::FALLOFF) if remote => Some(FALL),
                (_, attr::RESISTANCE_ID) if remote => Some(RESIST_ATTR as f64),
                (RARMOR_T | LARMOR_T, attr::ARMOR_AMOUNT) => Some(ARMOR_AMOUNT),
                (RSHIELD_T | LSHIELD_T, attr::SHIELD_BONUS) => Some(SHIELD_AMOUNT),
                (RHULL_T, attr::STRUCTURE_AMOUNT) => Some(HULL_AMOUNT),
                (RCAP_T, attr::POWER_TRANSFER) => Some(CAP_AMOUNT),
                _ => None,
            }
        }
    }

    fn effect(effect_id: i64, category: i64) -> Effect {
        Effect {
            effect_id,
            name: "fixture",
            category,
            guid: "",
            is_offensive: false,
            duration_attribute: 73,
            warp_safe: None,
        }
    }

    fn now() -> i64 {
        crate::pyutil::filetime_now()
    }

    fn far() -> std::time::Instant {
        std::time::Instant::now() + Duration::from_secs(3_600)
    }

    /// Alice, Bob and Carol in one system; Bob 1 km from Alice, Carol 1 km
    /// the other way. Bob's ship has a record, hurt in every layer.
    fn world() -> Registry {
        world_hit(SHIELD_HP + 300.0)
    }

    /// The same, with Bob hit by one volley of `hit` (kinetic, through no
    /// resistance): the layers go in order, shield, armour, hull.
    fn world_hit(hit: f64) -> Registry {
        let ch = |id: i64, ship: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..Character::default()
        };
        // The sandbox's system, made high sec: a crime is a crime there.
        let mut content = crate::content::Content::fixture();
        content.insert(
            "universe",
            crate::celestial::Universe::parse(
                &[
                    "30000001\t19000001\t29000001\t\t\t0.75\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx"
                        .to_owned(),
                ],
                &[],
                &[],
            )
            .expect("the invented row parses"),
        );
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                ch(ALICE, ALICE_SHIP, "Alice"),
                ch(BOB, BOB_SHIP, "Bob"),
                ch(CAROL, CAROL_SHIP, "Carol"),
            ],
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
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
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        reg.locks_mut().add(CAROL_SHIP, BOB_SHIP);
        // Bob's record: the default volley takes the shield and 300 of the
        // armour.
        let profile = Profile {
            shield_capacity: SHIELD_HP,
            shield_recharge_rate: 1.0e12,
            armor_hp: ARMOR_HP,
            hp: HULL_HP,
            shield_resonance: Damage::UNRESISTED,
            armor_resonance: Damage::UNRESISTED,
            hull_resonance: Damage::UNRESISTED,
        };
        let mut record = ShipDamage::full(profile, now());
        record.apply(
            Damage {
                kinetic: hit,
                ..Damage::ZERO
            },
            now(),
        );
        reg.combat_mut().set_ship(BOB_SHIP, record);
        let _ = reg.take_pending();
        let _ = reg.take_due(far());
        reg
    }

    /// `who` runs a module of `type_id` with `effect_id` on `target`, whose
    /// first cycle started a moment before `now`.
    fn run(
        reg: &mut Registry,
        who: (i64, i64, i64),
        type_id: i64,
        effect_id: i64,
        target: Option<i64>,
    ) {
        let (character_id, ship_id, module_id) = who;
        let category = if target.is_some() {
            crate::modules::CATEGORY_TARGET
        } else {
            1
        };
        reg.modules_mut().insert_running(Running {
            character_id,
            ship_id,
            module_id,
            type_id,
            effect: effect(effect_id, category),
            target_id: target,
            started: now() - MSEC,
            period: 5_000 * MSEC,
            repeat: 1000,
            cycles: 1,
            stop_at: None,
        });
    }

    fn alice(reg: &mut Registry, type_id: i64, effect_id: i64) {
        run(
            reg,
            (ALICE, ALICE_SHIP, ALICE_MOD),
            type_id,
            effect_id,
            Some(BOB_SHIP),
        );
    }

    /// A further cycle started: what `modules::advance` does.
    fn another_cycle(reg: &mut Registry, module: i64) {
        let mut r = reg.modules().running(module).copied().expect("runs");
        r.cycles += 1;
        reg.modules_mut().insert_running(r);
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

    fn rings(reg: &Registry) -> crate::damage::Rings {
        reg.combat().ship(BOB_SHIP).expect("a record").rings(now())
    }

    fn ring_told(sent: &[Sent], id: i64) -> Option<(f64, f64)> {
        sent.iter()
            .find(|(n, a, _)| {
                *n == "OnModuleAttributeChange" && a.get(2) == Some(&PyValue::Int(id))
            })
            .map(|(_, a, _)| match (&a[4], &a[5]) {
                (PyValue::Real(n), PyValue::Real(o)) => (*n, *o),
                other => panic!("not reals: {other:?}"),
            })
    }

    /// The rows: every effect is classified as the table says, and nothing
    /// else is.
    #[test]
    fn every_effect_has_its_attribute_and_nothing_else_is_a_rep() {
        let remote = [
            (REMOTE_SHIELD, Kind::Shield, 68),
            (ANCILLARY_REMOTE_SHIELD, Kind::Shield, 68),
            (SHIELD_TRANSFER, Kind::Shield, 68),
            (REMOTE_ARMOR, Kind::Armour, 84),
            (ANCILLARY_REMOTE_ARMOR, Kind::Armour, 84),
            (TARGET_ARMOR_REPAIR, Kind::Armour, 84),
            (REMOTE_HULL, Kind::Hull, 83),
            (REMOTE_CAPACITOR, Kind::Capacitor, 90),
        ];
        for (id, kind, amount) in remote {
            let s = spec_of(id).expect("a rep");
            assert!(s.remote && is_remote(id), "{id}");
            assert_eq!((s.kind, s.amount), (kind, amount), "{id}");
        }
        for (id, kind, amount) in [
            (SHIELD_BOOSTING, Kind::Shield, 68),
            (FUELED_SHIELD_BOOSTING, Kind::Shield, 68),
            (ARMOR_REPAIR, Kind::Armour, 84),
            (FUELED_ARMOR_REPAIR, Kind::Armour, 84),
            (STRUCTURE_REPAIR, Kind::Hull, 83),
        ] {
            let s = spec_of(id).expect("a rep");
            assert!(!s.remote && !is_remote(id), "{id}");
            assert_eq!((s.kind, s.amount), (kind, amount), "{id}");
        }
        let booster = spec_of(CAP_BOOSTER).expect("a rep");
        assert!(!booster.remote && booster.from_charge && booster.kind == Kind::Capacitor);
        assert_eq!(booster.amount, 67);
        assert_eq!(spec_of(SHIELD_TRANSFER).map(|s| s.range), Some(87));
        assert!(is_rep(27) && !is_rep(6426) && !is_rep(0));
        assert!(!is_remote(6426), "a web is not assistance");
    }

    /// A remote armor repairer heals the target's armour by its amount per
    /// cycle, and the target's pilot is told the ring and the park the bars.
    #[test]
    fn a_remote_armor_repairer_heals_and_tells_the_target() {
        let mut reg = world();
        let before = rings(&reg);
        assert_eq!(before.armor_damage, 300.0);
        alice(&mut reg, RARMOR_T, REMOTE_ARMOR);
        reconcile(&mut reg, now(), &PLAIN);
        let after = rings(&reg);
        assert_eq!(after.armor_damage, 300.0 - ARMOR_AMOUNT);
        let sent = pushes(&mut reg);
        assert_eq!(
            ring_told(&sent, 266),
            Some((300.0 - ARMOR_AMOUNT, 300.0)),
            "the ring"
        );
        let ring = sent
            .iter()
            .find(|(n, a, _)| {
                *n == "OnModuleAttributeChange" && a.get(2) == Some(&PyValue::Int(266))
            })
            .expect("a ring push");
        assert_eq!(ring.2, Some(vec![BOB]), "to the target's pilot only");
        let bars = sent
            .iter()
            .find(|(n, _, _)| *n == "DoDestinyUpdate")
            .expect("the bars");
        let to = bars.2.clone().expect("an audience");
        assert!(to.contains(&BOB) && to.contains(&ALICE) && to.contains(&CAROL));
    }

    /// One cycle lands once however often the stage runs, and the next
    /// cycle lands again.
    #[test]
    fn a_cycle_lands_once() {
        let mut reg = world();
        alice(&mut reg, RARMOR_T, REMOTE_ARMOR);
        for _ in 0..3 {
            reconcile(&mut reg, now(), &PLAIN);
        }
        assert_eq!(rings(&reg).armor_damage, 300.0 - ARMOR_AMOUNT);
        another_cycle(&mut reg, ALICE_MOD);
        reconcile(&mut reg, now(), &PLAIN);
        assert_eq!(rings(&reg).armor_damage, 300.0 - 2.0 * ARMOR_AMOUNT);
        // A module restarted (a new `started`) counts from its first cycle.
        let mut r = reg.modules().running(ALICE_MOD).copied().expect("runs");
        r.started += 1;
        r.cycles = 1;
        reg.modules_mut().insert_running(r);
        reconcile(&mut reg, now(), &PLAIN);
        assert_eq!(rings(&reg).armor_damage, 300.0 - 3.0 * ARMOR_AMOUNT);
    }

    /// A rep never heals past full: the armour stops at none damaged.
    #[test]
    fn a_rep_clips_to_the_damage_there_is() {
        let mut reg = world();
        alice(&mut reg, RARMOR_T, REMOTE_ARMOR);
        reconcile(&mut reg, now(), &PLAIN);
        for _ in 0..8 {
            another_cycle(&mut reg, ALICE_MOD);
            reconcile(&mut reg, now(), &PLAIN);
        }
        assert_eq!(rings(&reg).armor_damage, 0.0, "all of it, no more");
        let _ = pushes(&mut reg);
        another_cycle(&mut reg, ALICE_MOD);
        reconcile(&mut reg, now(), &PLAIN);
        let sent = pushes(&mut reg);
        assert!(
            ring_told(&sent, 266).is_none(),
            "nothing moved, nothing told: {sent:?}"
        );
    }

    /// The shield booster fills the shield, the hull repairer the hull, and
    /// each leaves the others alone.
    #[test]
    fn each_module_heals_its_own_layer() {
        // A shield booster on a shield 100 down.
        let mut reg = world_hit(100.0);
        let shield = rings(&reg).shield_charge;
        assert!((shield - (SHIELD_HP - 100.0)).abs() < 1.0e-6, "{shield}");
        run(
            &mut reg,
            (ALICE, ALICE_SHIP, ALICE_MOD),
            RSHIELD_T,
            REMOTE_SHIELD,
            Some(BOB_SHIP),
        );
        reconcile(&mut reg, now(), &PLAIN);
        let r = rings(&reg);
        assert!((r.shield_charge - (shield + SHIELD_AMOUNT)).abs() < 1.0e-6);
        assert_eq!((r.armor_damage, r.damage), (0.0, 0.0));
        let told = ring_told(&pushes(&mut reg), 264).expect("the shield ring");
        assert!(
            (told.0 - (shield + SHIELD_AMOUNT)).abs() < 1.0e-6,
            "{told:?}"
        );

        // A hull repairer on a hull 100 down, its armour gone.
        let mut reg = world_hit(SHIELD_HP + ARMOR_HP + 100.0);
        assert!((rings(&reg).damage - 100.0).abs() < 1.0e-6);
        run(
            &mut reg,
            (ALICE, ALICE_SHIP, ALICE_MOD),
            RHULL_T,
            REMOTE_HULL,
            Some(BOB_SHIP),
        );
        reconcile(&mut reg, now(), &PLAIN);
        let r = rings(&reg);
        assert!((r.damage - (100.0 - HULL_AMOUNT)).abs() < 1.0e-6);
        assert_eq!(r.armor_damage, ARMOR_HP, "the armour is not the hull's");
        let told = ring_told(&pushes(&mut reg), 3).expect("the hull ring");
        assert!((told.0 - (100.0 - HULL_AMOUNT)).abs() < 1.0e-6, "{told:?}");
    }

    /// A local booster and a local armor repairer heal the pilot's own ship,
    /// aimed at nothing.
    #[test]
    fn a_local_rep_heals_its_own_ship() {
        let mut reg = world();
        let profile = reg.combat().ship(BOB_SHIP).copied().expect("a record");
        reg.combat_mut().set_ship(ALICE_SHIP, profile);
        run(
            &mut reg,
            (ALICE, ALICE_SHIP, ALICE_MOD),
            LARMOR_T,
            ARMOR_REPAIR,
            None,
        );
        reconcile(&mut reg, now(), &PLAIN);
        let own = reg
            .combat()
            .ship(ALICE_SHIP)
            .expect("a record")
            .rings(now());
        assert_eq!(own.armor_damage, 300.0 - ARMOR_AMOUNT);
        assert_eq!(rings(&reg).armor_damage, 300.0, "Bob is untouched");
        let sent = pushes(&mut reg);
        assert_eq!(
            sent.iter()
                .find(|(n, a, _)| *n == "OnModuleAttributeChange"
                    && a.get(1) == Some(&PyValue::Int(ALICE_SHIP)))
                .map(|(_, _, to)| to.clone()),
            Some(Some(vec![ALICE]))
        );
        // A local rep is no assistance: nobody got a timer for it.
        let key = crate::crimewatch::Timer::Weapons;
        assert!(
            reg.combat_mut()
                .crimewatch_mut()
                .expiry(ALICE, key, now())
                .is_none()
        );
    }

    /// The falloff scales the amount; past the reach nothing lands.
    #[test]
    fn range_and_falloff_scale_the_amount() {
        let mut reg = world();
        fn set(reg: &mut Registry, dx: f64) {
            let origin = reg.roster().get(ALICE).expect("in space").motion.position;
            let p = reg.roster_mut().get_mut(BOB).expect("in space");
            p.motion = crate::space_state::Motion::arrived(
                [origin[0] + dx, origin[1], origin[2]],
                [0.0; 3],
            );
        }
        // One falloff past the range: half.
        set(&mut reg, RANGE + FALL);
        alice(&mut reg, RARMOR_T, REMOTE_ARMOR);
        reconcile(&mut reg, now(), &PLAIN);
        let half = 300.0 - rings(&reg).armor_damage;
        assert!((half - ARMOR_AMOUNT * 0.5).abs() < 1.0e-6, "{half}");
        // Beyond the reach: nothing, and no assistance either.
        set(&mut reg, RANGE + (crate::ewar::FALLOFF_REACH + 1.0) * FALL);
        let before = rings(&reg).armor_damage;
        another_cycle(&mut reg, ALICE_MOD);
        reconcile(&mut reg, now(), &PLAIN);
        assert_eq!(rings(&reg).armor_damage, before);
    }

    /// The target's impedance multiplies the amount, as the module's
    /// attribute 2138 says.
    #[test]
    fn the_targets_impedance_scales_the_amount() {
        let mut reg = world();
        alice(&mut reg, RARMOR_T, REMOTE_ARMOR);
        reconcile(&mut reg, now(), &Fixture { resist: 0.5 });
        assert_eq!(rings(&reg).armor_damage, 300.0 - ARMOR_AMOUNT * 0.5);
    }

    /// A capacitor transmitter fills the target's capacitor and tells its
    /// pilot attribute 18, clipped to the capacity.
    #[test]
    fn a_capacitor_transmitter_fills_the_targets_capacitor() {
        let mut reg = world();
        // Bob's capacitor is down: his ship's charge, drained at `now`.
        let hull = reg.character(BOB).expect("Bob").ship_type_id;
        let capacity = crate::modules::capacitor_at(&reg, BOB_SHIP, hull, now()).1;
        let low = capacity - 2.0 * CAP_AMOUNT;
        reg.modules_mut().set_capacitor(BOB_SHIP, low, now());
        run(
            &mut reg,
            (ALICE, ALICE_SHIP, ALICE_MOD),
            RCAP_T,
            REMOTE_CAPACITOR,
            Some(BOB_SHIP),
        );
        reconcile(&mut reg, now(), &PLAIN);
        let told = ring_told(&pushes(&mut reg), 18).expect("attribute 18 to Bob");
        assert!((told.0 - (low + CAP_AMOUNT)).abs() < 1.0, "{told:?}");
        assert!((told.1 - low).abs() < 1.0, "{told:?}");
        // Two more cycles: it stops at the capacity.
        for _ in 0..3 {
            another_cycle(&mut reg, ALICE_MOD);
            reconcile(&mut reg, now(), &PLAIN);
        }
        let (value, capacity) = crate::modules::capacitor_at(&reg, BOB_SHIP, hull, now());
        assert!(value <= capacity && value > capacity - 1.0, "{value}");
    }

    /// A remote rep is assistance: helping a pilot with a weapons timer
    /// hands the helper that timer, inherited.
    #[test]
    fn helping_a_flagged_pilot_inherits_the_timer() {
        let mut reg = world();
        let t = now();
        crate::crimewatch::aggression(&mut reg, BOB, CAROL, t, t);
        alice(&mut reg, RARMOR_T, REMOTE_ARMOR);
        reconcile(&mut reg, t, &PLAIN);
        assert!(
            reg.combat_mut()
                .crimewatch_mut()
                .expiry(ALICE, crate::crimewatch::Timer::Weapons, t)
                .is_some(),
            "the helper has the target's weapons timer"
        );
    }

    /// The safety refuses a remote module on a flagged pilot and lets a local
    /// one and a clean target through.
    #[test]
    fn the_safety_refuses_help_to_the_flagged_only() {
        let mut reg = world();
        let t = now();
        assert!(
            activation_refusal(&reg, ALICE, RARMOR_T, REMOTE_ARMOR, Some(BOB_SHIP), t).is_none()
        );
        // Bob shoots Carol: flagged, and the client's own key refuses.
        crate::crimewatch::aggression(&mut reg, BOB, CAROL, t, t);
        let refusal = activation_refusal(&reg, ALICE, RARMOR_T, REMOTE_ARMOR, Some(BOB_SHIP), t)
            .expect("refused");
        assert!(
            refusal
                .key
                .starts_with("ModuleActivationDeniedSafetyPrevents"),
            "{}",
            refusal.key
        );
        assert!(
            activation_refusal(&reg, ALICE, LARMOR_T, ARMOR_REPAIR, None, t).is_none(),
            "a local rep needs no say"
        );
        // EWAR-2C: a remote sensor booster and a remote tracking computer are
        // help too; a disruptor is not.
        for effect in [crate::ewar::SENSOR_BOOST, crate::ewar::REMOTE_TRACKING] {
            assert!(
                activation_refusal(&reg, ALICE, RARMOR_T, effect, Some(BOB_SHIP), t).is_some(),
                "help on a flagged pilot is refused"
            );
        }
        assert!(
            activation_refusal(
                &reg,
                ALICE,
                RARMOR_T,
                crate::ewar::TRACKING,
                Some(BOB_SHIP),
                t
            )
            .is_none()
        );
    }

    /// A stage with nothing running does nothing, and a module that is no
    /// rep is ignored.
    #[test]
    fn an_idle_world_and_other_effects_do_nothing() {
        let mut reg = world();
        reconcile(&mut reg, now(), &PLAIN);
        run(
            &mut reg,
            (ALICE, ALICE_SHIP, ALICE_MOD),
            RARMOR_T,
            6426,
            Some(BOB_SHIP),
        );
        reconcile(&mut reg, now(), &PLAIN);
        assert_eq!(rings(&reg).armor_damage, 300.0);
        assert!(reg.ext::<Reps>().done.is_empty());
    }

    fn icons_of(sent: &[Sent]) -> Vec<Sent> {
        sent.iter()
            .filter(|(n, ..)| matches!(*n, "OnEwarStart" | "OnEwarEnd"))
            .cloned()
            .collect()
    }

    fn icon_args(source: i64, module: i64, target: i64, slot: &str) -> Vec<PyValue> {
        vec![
            PyValue::Int(source),
            PyValue::Int(module),
            PyValue::Int(target),
            crate::pyutil::bstr(slot),
        ]
    }

    /// EWAR-ICONS: a remote rep puts its icon on the target's pilot alone,
    /// once per source, and takes it off when the source stops or is out of
    /// reach.
    #[test]
    fn a_remote_rep_shows_its_icon_to_the_target_only() {
        let mut reg = world();
        alice(&mut reg, RARMOR_T, REMOTE_ARMOR);
        run(
            &mut reg,
            (CAROL, CAROL_SHIP, ALICE_MOD + 1),
            RSHIELD_T,
            REMOTE_SHIELD,
            Some(BOB_SHIP),
        );
        reconcile(&mut reg, now(), &PLAIN);
        let sent = icons_of(&pushes(&mut reg));
        assert_eq!(sent.len(), 2, "two reppers, two starts: {sent:?}");
        assert!(
            sent.iter()
                .all(|(n, _, a)| *n == "OnEwarStart" && *a == Some(vec![BOB]))
        );
        assert!(
            sent.iter()
                .any(|(_, a, _)| *a
                    == icon_args(ALICE_SHIP, ALICE_MOD, BOB_SHIP, "remoteArmorRepair"))
        );
        assert!(sent.iter().any(
            |(_, a, _)| *a == icon_args(CAROL_SHIP, ALICE_MOD + 1, BOB_SHIP, "shieldTransfer")
        ));
        // Nothing new on the next tick.
        reconcile(&mut reg, now(), &PLAIN);
        assert!(icons_of(&pushes(&mut reg)).is_empty());
        // Alice stops: one end, the same four arguments.
        run(
            &mut reg,
            (ALICE, ALICE_SHIP, ALICE_MOD),
            RARMOR_T,
            REMOTE_ARMOR,
            None,
        );
        reconcile(&mut reg, now(), &PLAIN);
        let sent = icons_of(&pushes(&mut reg));
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "OnEwarEnd");
        assert_eq!(
            sent[0].1,
            icon_args(ALICE_SHIP, ALICE_MOD, BOB_SHIP, "remoteArmorRepair")
        );
        // Carol's drifts out of range: an end.
        let far_off = reg.roster().get(CAROL).expect("in space").motion.position;
        let p = reg.roster_mut().get_mut(CAROL).expect("in space");
        p.motion = crate::space_state::Motion::arrived(
            [far_off[0] - 10.0 * RANGE, far_off[1], far_off[2]],
            [0.0; 3],
        );
        reconcile(&mut reg, now(), &PLAIN);
        let sent = icons_of(&pushes(&mut reg));
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].0, "OnEwarEnd");
    }
}
