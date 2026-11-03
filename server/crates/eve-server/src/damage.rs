//! Damage, its application through shield, armour and hull, and death.
//!
//! Spec: `docs/re/damage-death.md`. Owned by the P2.2a lane (`docs/lanes/damage-death.md`).
//!
//! **Pure and unwired.** Nothing in here sends anything, encodes anything, or
//! knows what a connection is. It holds one ship's health, applies a volley to
//! it, evaluates its shield at a given moment, and -- when the hull runs out --
//! says what has to happen next, as data. The wiring (the destiny entry, the
//! godma rows, `OnDamageMessage`, the removal, the wreck, the capsule, the
//! session change) is a later packet; `docs/lanes/damage-death.md` lists what
//! it needs from here.
//!
//! **The shape, and why.** The player sees health through two independent
//! pipes (§1): the *bars* (`michelle.Park.damageState`, every client, fed by
//! the destiny action `OnDamageStateChange`) and the *rings* (godma attributes
//! on the own ship, fed by `OnModuleAttributeChange`). Both run the **same
//! recharge curve on the client** (§3), so the server sends a curve, not
//! samples, and only when something other than regeneration moves a number.
//! What the server must therefore store is exactly what both producers read:
//!
//! - the shield's **absolute charge** (attribute 264) and **the moment it was
//!   last true** -- regeneration is evaluated lazily from that pair, by the
//!   client's own closed form, never by a tick loop (§3.3);
//! - **armour damage** (266) and **hull damage** (3), absolute, because
//!   neither regenerates on either side (§3.1);
//! - the capacities and the recharge rate, which come from the hull.
//!
//! `[A]` (§1.4) both families are derived from this **one** number per layer,
//! so that they cannot disagree by construction. Falsified if CCP's traffic
//! ever shows the bars and rings disagreeing by design.
//!
//! **Time.** Every `now` in this module is a blue FILETIME: 100 ns ticks, the
//! unit of `blue.os.GetSimTime()` and of `crate::pyutil::filetime_now()`. It
//! is *not* a destiny stamp. `dgmTauConstant = 10000` (`dogma/const.py:103`)
//! `[V]` is ticks per millisecond, and every `tau` here is in milliseconds.

use crate::destiny::hull::Hull;

/// A blue FILETIME: 100 ns ticks since 1601. See the module documentation.
pub type BlueTime = i64;

/// `dogma.const.dgmTauConstant` (`dogma/const.py:103`, `LOAD_CONST 10000`)
/// `[V]`: blue ticks per millisecond, the divisor that turns a stamp
/// difference into the milliseconds `tau` is measured in.
pub const DGM_TAU_CONSTANT: f64 = 10_000.0;

/// The capsule's type id, `inventorycommon/const.py:1477` `[V]` (§5.1).
pub const CAPSULE_TYPE_ID: i64 = 670;

/// The dogma attribute ids this module reads or writes.
///
/// The health ids are §0's table and `docs/port-tracker.md` §23 `[V]`; the
/// damage-type ids are §2.1 (`uilog.py:382`) `[V]`; the resonance ids were read
/// for this packet out of `dogma/const.pyc` `<module>` with `tools/dis.py`
/// `[V]` (line numbers are that module's).
pub mod attr {
    /// `hp`: the structure ring's capacity.
    pub const HP: i64 = 9;
    /// `damage`: structure damage taken, the complement of [`HP`].
    pub const DAMAGE: i64 = 3;
    /// `shieldCapacity`.
    pub const SHIELD_CAPACITY: i64 = 263;
    /// `shieldCharge`: a *charged* attribute on the client (§3.2).
    pub const SHIELD_CHARGE: i64 = 264;
    /// `armorHP`.
    pub const ARMOR_HP: i64 = 265;
    /// `armorDamage`: the complement of [`ARMOR_HP`].
    pub const ARMOR_DAMAGE: i64 = 266;
    /// `shieldRechargeRate`, ms. Sent **undivided**: the client's
    /// `CreateChargedAttribute:2061` divides it by five itself (§3.2).
    pub const SHIELD_RECHARGE_RATE: i64 = 479;

    /// `emDamage`, `explosiveDamage`, `kineticDamage`, `thermalDamage`.
    pub const EM_DAMAGE: i64 = 114;
    pub const EXPLOSIVE_DAMAGE: i64 = 116;
    pub const KINETIC_DAMAGE: i64 = 117;
    pub const THERMAL_DAMAGE: i64 = 118;
    /// `damageMultiplier`, the gun's own scale on its charge's damage.
    pub const DAMAGE_MULTIPLIER_ID: i64 = 64;

    /// `attributeShield{Em,Explosive,Kinetic,Thermal}DamageResonance`,
    /// `dogma/const.py:952,954,956,962`.
    pub const SHIELD_RESONANCE: [i64; 4] = [271, 272, 273, 274];
    /// `attributeArmor{…}DamageResonance`, `:158,160,166,168`.
    pub const ARMOR_RESONANCE: [i64; 4] = [267, 268, 269, 270];
    /// The structure's: `attribute{Em,Explosive,Kinetic,Thermal}DamageResonance`
    /// with **no** layer prefix, `:454,545,644,1067` = 113, 111, 109, 110.
    ///
    /// `[A]` that these bare names are the ship's structure resonances, as the
    /// fitting window's `GetStructure*DamageResonance` getters suggest
    /// (`fittingUtil.GetDefensiveLayersInfo:349-352`), and that
    /// `attributeHull*DamageResonance` 974-977 (`:595-598`) are something else.
    /// Falsified by a hull whose 974-977 differ from 1.0 in `typedogma`.
    pub const HULL_RESONANCE: [i64; 4] = [113, 111, 109, 110];

    /// Is `id` one of the twelve resonances?
    pub fn is_resonance(id: i64) -> bool {
        SHIELD_RESONANCE.contains(&id)
            || ARMOR_RESONANCE.contains(&id)
            || HULL_RESONANCE.contains(&id)
    }
}

/// Four damage amounts, in the order EM, explosive, kinetic, thermal.
///
/// Used twice: as a raw volley before resistances, and as a layer's
/// resonances (multipliers, `1.0` = no resistance).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Damage {
    pub em: f64,
    pub explosive: f64,
    pub kinetic: f64,
    pub thermal: f64,
}

impl Damage {
    /// No damage at all -- a miss.
    pub const ZERO: Damage = Damage {
        em: 0.0,
        explosive: 0.0,
        kinetic: 0.0,
        thermal: 0.0,
    };

    /// As resonances: no resistance in any type.
    pub const UNRESISTED: Damage = Damage {
        em: 1.0,
        explosive: 1.0,
        kinetic: 1.0,
        thermal: 1.0,
    };

    /// The four amounts in [`attr::EM_DAMAGE`].. order, paired with their
    /// attribute ids, as `OnDamageMessage`'s optional `damageAttributes`
    /// wants them (§2.1).
    pub fn by_attribute(self) -> [(i64, f64); 4] {
        [
            (attr::EM_DAMAGE, self.em),
            (attr::EXPLOSIVE_DAMAGE, self.explosive),
            (attr::KINETIC_DAMAGE, self.kinetic),
            (attr::THERMAL_DAMAGE, self.thermal),
        ]
    }

    /// The sum of the four.
    pub fn total(self) -> f64 {
        self.em + self.explosive + self.kinetic + self.thermal
    }

    /// Element-wise product: a volley through a layer's resonances.
    ///
    /// `[A]` resonance is a plain multiplier on each type and the layer loses
    /// the sum. The client's own fitting window agrees in spirit -- its
    /// `GetEffectiveHp` (`fittingUtil.py:363-374`) is `hp / mean(resonance)`
    /// per layer -- but that is a display, not the server's arithmetic.
    /// Falsified by a hit whose number in the combat log is not
    /// `Σ raw × resonance`.
    fn through(self, res: Damage) -> Damage {
        Damage {
            em: self.em * res.em,
            explosive: self.explosive * res.explosive,
            kinetic: self.kinetic * res.kinetic,
            thermal: self.thermal * res.thermal,
        }
    }

    fn scaled(self, k: f64) -> Damage {
        Damage {
            em: self.em * k,
            explosive: self.explosive * k,
            kinetic: self.kinetic * k,
            thermal: self.thermal * k,
        }
    }
}

/// A ship's fixed numbers: what the hull and its fitting say, not what has
/// happened to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profile {
    /// Attribute 263.
    pub shield_capacity: f64,
    /// Attribute 479, ms, **undivided**. [`Bars`] divides it by five; the
    /// rings must not (§3.2, §8.3).
    pub shield_recharge_rate: f64,
    /// Attribute 265.
    pub armor_hp: f64,
    /// Attribute 9.
    pub hp: f64,
    pub shield_resonance: Damage,
    pub armor_resonance: Damage,
    pub hull_resonance: Damage,
}

impl Profile {
    /// From a row of the hull table, with the hull type's own resonances.
    ///
    /// **The resonances come from the world's typedata**
    /// (`typedata::resonances_in(reg.content(), ..)`, CP58c): the client's
    /// [`attr::SHIELD_RESONANCE`], [`attr::ARMOR_RESONANCE`] and
    /// [`attr::HULL_RESONANCE`] rows, with the attribute's own default (1.0)
    /// where the type carries none. For a hull the table does not hold (an
    /// NPC's: category 11 is not extracted), `[A]` **every resonance is
    /// 1.0** -- the hand hull table has no resistance columns -- and the ship
    /// takes every damage type at face value.
    pub fn from_hull_in(content: &crate::content::Content, h: &Hull) -> Profile {
        Self::with_resonances(h, crate::typedata::resonances_in(content, h.type_id))
    }

    fn with_resonances(h: &Hull, resonances: Option<[Damage; 3]>) -> Profile {
        let [shield, armor, hull] =
            resonances.unwrap_or([Damage::UNRESISTED, Damage::UNRESISTED, Damage::UNRESISTED]);
        Profile {
            shield_capacity: h.shield_capacity,
            shield_recharge_rate: h.shield_recharge_rate,
            armor_hp: h.armor_hp,
            hp: h.hp,
            shield_resonance: shield,
            armor_resonance: armor,
            hull_resonance: hull,
        }
    }

    /// The `tau` both clients run their shield curve with, in ms:
    /// `shieldRechargeRate / 5.0` (§3.1, and the client's own
    /// `CreateChargedAttribute:2061` on the godma side) `[V]`.
    pub fn shield_tau_ms(&self) -> f64 {
        self.shield_recharge_rate / 5.0
    }
}

/// The client's shield curve, `godma.GetChargeValue:2413-2422` `[V]` (§3.2):
///
/// ```text
/// 2414  if Ec == 0: return 0
/// 2418  sq = math.sqrt(max(oldVal / Ec, 0))
/// 2420  timePassed = min(oldTime - newTime, 0) / float(const.dgmTauConstant)
/// 2421  exp = math.exp(timePassed / tau)
/// 2422  return (1.0 + (sq - 1.0) * exp) ** 2 * Ec
/// ```
///
/// with one addition: a non-positive `tau` returns the value unchanged. That is
/// the bars' own guard (`if tau > 0`, `damageStateValue.py:14`) and, on the
/// godma side, the reason no charged attribute is created at all when 479 is
/// zero (`UpdateAttribute:2055`) `[V]` -- so it is the client's behaviour in
/// both families, not a defensive choice of ours.
///
/// The bars' transcription (§3.1, `:16`) divides a stamp difference by the
/// *integer* `dgmTauConstant`, which in Python 2 floors to whole milliseconds
/// unless the module imports true division `[I]`. The difference is under a
/// millisecond of `dt`, far below what the 1 Hz bar poll can show, and we
/// follow godma's float form.
pub fn charge_at(
    old_value: f64,
    old_time: BlueTime,
    capacity: f64,
    tau_ms: f64,
    now: BlueTime,
) -> f64 {
    if capacity == 0.0 {
        return 0.0;
    }
    if tau_ms <= 0.0 {
        return old_value;
    }
    let sq = (old_value / capacity).max(0.0).sqrt();
    // `min(oldTime - newTime, 0)`: a `now` before the stamp cannot drain the
    // gauge (§3.2's third point).
    let passed_ms = (old_time.saturating_sub(now)).min(0) as f64 / DGM_TAU_CONSTANT;
    let e = (passed_ms / tau_ms).exp();
    (1.0 + (sq - 1.0) * e).powi(2) * capacity
}

/// One ship's health, server-authoritative.
///
/// Stores what both gauge families are computed from and nothing they could
/// compute themselves. See the module documentation for why this is the set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShipDamage {
    profile: Profile,
    /// Attribute 264 as it was at `shield_stamp`.
    shield_charge: f64,
    shield_stamp: BlueTime,
    /// Attribute 266.
    armor_damage: f64,
    /// Attribute 3.
    hull_damage: f64,
}

/// The layers, in the order damage reaches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Shield,
    Armour,
    Hull,
}

/// What one volley did. All amounts are post-resistance hit points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Applied {
    pub shield: f64,
    pub armour: f64,
    pub hull: f64,
    /// Post-resistance damage left over after the hull reached zero.
    pub overkill: f64,
    /// `Some` exactly when this volley was the one that took the hull to zero.
    pub death: Option<Death>,
    /// The godma rows this volley makes necessary, one per layer whose number
    /// moved (§8.1). See [`RingChange`].
    pub ring_changes: RingChanges,
}

impl Applied {
    /// The hit points this volley actually removed.
    pub fn taken(&self) -> f64 {
        self.shield + self.armour + self.hull
    }

    /// The number for `OnDamageMessage`'s `damage` key.
    ///
    /// `[A]` the post-resistance damage of the whole volley, **including**
    /// overkill: what the shot did, not what the ship had left. Falsified if
    /// CCP's log shows a killing blow capped at the hull that remained.
    pub fn message_damage(&self) -> f64 {
        self.taken() + self.overkill
    }
}

/// One godma attribute that moved: the payload of one
/// `OnModuleAttributeChange(ownerID, shipID, attributeID, time, new, old,
/// wallclockTime)` (§1.3). The wiring supplies the rest -- see the lane note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingChange {
    /// 264, 266 or 3.
    pub attribute_id: i64,
    pub old: f64,
    pub new: f64,
}

/// Up to three [`RingChange`]s, shield first. A fixed array rather than a
/// `Vec` because there are never more than three.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RingChanges {
    pub shield: Option<RingChange>,
    pub armour: Option<RingChange>,
    pub hull: Option<RingChange>,
}

impl RingChanges {
    pub fn iter(&self) -> impl Iterator<Item = RingChange> + '_ {
        [self.shield, self.armour, self.hull].into_iter().flatten()
    }
}

/// What the **bars** need: the payload of `OnDamageStateChange` and of every
/// `damageState` slot (§1.1), before encoding.
///
/// The wire form is `((shield, tau_ms, anything), armour, hull)`, and
/// `destiny::DamageState` is the encoder -- which today hard-codes `tau = 0`,
/// freezing the shield bar. The wiring packet must give it a `tau`; see the
/// lane note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bars {
    /// `None` when the hull has no shield at all: CCP's producer then sends a
    /// bare float in the shield slot, and the client hides that bar
    /// (`GetDamageStateEx:1766`, documented on `destiny::DamageState`) `[V]`.
    pub shield: Option<ShieldBar>,
    pub armour: f64,
    pub hull: f64,
}

/// The shield slot of [`Bars`]: a point on the curve and the curve's speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShieldBar {
    /// Fraction, `1.0` full.
    pub fraction: f64,
    /// `shieldRechargeRate / 5.0`, ms -- the `/5` belongs **here** and only
    /// here (§3.1, §8.3) `[V]`.
    pub tau_ms: f64,
}

/// What the **rings** need: the own ship's absolute godma values (§1.3,
/// port-tracker §23.5). All `f64` because §23.5's float-vs-int trap is
/// already paid for once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rings {
    pub shield_capacity: f64,
    pub shield_charge: f64,
    /// Attribute 479, **undivided** (§3.2) `[V]`.
    pub shield_recharge_rate: f64,
    pub armor_hp: f64,
    pub armor_damage: f64,
    pub hp: f64,
    pub damage: f64,
}

impl Rings {
    /// `(attributeID, value)` for each of the seven, for a priming row.
    pub fn by_attribute(&self) -> [(i64, f64); 7] {
        [
            (attr::SHIELD_CAPACITY, self.shield_capacity),
            (attr::SHIELD_CHARGE, self.shield_charge),
            (attr::SHIELD_RECHARGE_RATE, self.shield_recharge_rate),
            (attr::ARMOR_HP, self.armor_hp),
            (attr::ARMOR_DAMAGE, self.armor_damage),
            (attr::HP, self.hp),
            (attr::DAMAGE, self.damage),
        ]
    }
}

/// `1 - damage / max`, CCP's producer for the armour and hull bars
/// (`GetDamageStateEx:1776`) `[V]`.
///
/// `[A]` a layer with no hit points reads as an empty bar (`0.0`). CCP's own
/// expression would divide by zero there; no hull we fly has a zero armour or
/// structure. Falsified by one that does and a client that shows it full.
fn complement_fraction(damage: f64, max: f64) -> f64 {
    if max > 0.0 { 1.0 - damage / max } else { 0.0 }
}

impl ShipDamage {
    /// An undamaged ship, its shield last true at `now`.
    pub fn full(profile: Profile, now: BlueTime) -> ShipDamage {
        ShipDamage {
            profile,
            shield_charge: profile.shield_capacity,
            shield_stamp: now,
            armor_damage: 0.0,
            hull_damage: 0.0,
        }
    }

    /// A ship with its shield full and the given armour and structure damage
    /// (the repair shop's stash, `crate::repair`), each clamped to the
    /// profile's current maximum in case skills or fittings changed.
    pub fn with_damage(profile: Profile, now: BlueTime, armor: f64, hull: f64) -> ShipDamage {
        let mut s = ShipDamage::full(profile, now);
        s.armor_damage = armor.clamp(0.0, profile.armor_hp.max(0.0));
        s.hull_damage = hull.clamp(0.0, profile.hp.max(0.0));
        s
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The shield's charge at `now`, by the client's own curve
    /// ([`charge_at`]).
    pub fn shield_at(&self, now: BlueTime) -> f64 {
        charge_at(
            self.shield_charge,
            self.shield_stamp,
            self.profile.shield_capacity,
            self.profile.shield_tau_ms(),
            now,
        )
    }

    /// Armour hit points remaining. Does not regenerate (§3.1).
    pub fn armour_left(&self) -> f64 {
        (self.profile.armor_hp - self.armor_damage).max(0.0)
    }

    /// Structure hit points remaining. Does not regenerate (§3.1).
    pub fn hull_left(&self) -> f64 {
        (self.profile.hp - self.hull_damage).max(0.0)
    }

    /// Whether the hull has reached zero (§0: "until the server's own hull
    /// number reaches zero").
    ///
    /// `[A]` a hull with `hp == 0` is not destroyed until something hits it.
    pub fn is_destroyed(&self) -> bool {
        self.profile.hp > 0.0 && self.hull_damage >= self.profile.hp
    }

    /// The bars at `now`: what `OnDamageStateChange` should carry if it were
    /// sent at `now`.
    ///
    /// The client stamps the entry with its **own** clock when it lands
    /// (`michelle.py:2076`) `[V]`, so the curve restarts from this point at
    /// arrival, not at `now`; the error is one network latency of
    /// regeneration.
    pub fn bars(&self, now: BlueTime) -> Bars {
        let p = &self.profile;
        let shield = (p.shield_capacity != 0.0).then(|| ShieldBar {
            fraction: self.shield_at(now) / p.shield_capacity,
            tau_ms: p.shield_tau_ms(),
        });
        Bars {
            shield,
            armour: complement_fraction(self.armor_damage, p.armor_hp),
            hull: complement_fraction(self.hull_damage, p.hp),
        }
    }

    /// The rings at `now`: the absolute godma values.
    ///
    /// The shield charge is evaluated at `now`. Its row should go out with
    /// `time = None` so the client starts the curve on arrival
    /// (`UpdateAttribute:2049`, §3.2) `[V]`, which is the same restart the
    /// bars get.
    pub fn rings(&self, now: BlueTime) -> Rings {
        let p = &self.profile;
        Rings {
            shield_capacity: p.shield_capacity,
            shield_charge: self.shield_at(now),
            shield_recharge_rate: p.shield_recharge_rate,
            armor_hp: p.armor_hp,
            armor_damage: self.armor_damage,
            hp: p.hp,
            damage: self.hull_damage,
        }
    }

    /// Restore up to `amount` hit points to the armour, or to the shield
    /// (brought up to `now` along its curve and re-stamped first), capped at
    /// the layer's maximum. Nothing for a destroyed ship. Answers the hit
    /// points restored. An NPC's own repairer (`npc::brain`, CP23).
    pub fn repair(&mut self, armour: bool, amount: f64, now: BlueTime) -> f64 {
        if self.is_destroyed() || amount <= 0.0 {
            return 0.0;
        }
        if armour {
            let healed = amount.min(self.armor_damage);
            self.armor_damage -= healed;
            healed
        } else {
            let before = self.shield_at(now);
            let after = (before + amount).min(self.profile.shield_capacity);
            self.shield_charge = after.max(before);
            self.shield_stamp = now;
            self.shield_charge - before
        }
    }

    /// Restore up to `amount` hit points to one `layer`: [`Self::repair`]
    /// for the shield and the armour, and the hull's own damage for the
    /// third (a hull repairer, K-LOGI). Answers the hit points restored.
    pub fn mend(&mut self, layer: Layer, amount: f64, now: BlueTime) -> f64 {
        match layer {
            Layer::Shield => self.repair(false, amount, now),
            Layer::Armour => self.repair(true, amount, now),
            Layer::Hull => {
                if self.is_destroyed() || amount <= 0.0 {
                    return 0.0;
                }
                let healed = amount.min(self.hull_damage);
                self.hull_damage -= healed;
                healed
            }
        }
    }

    /// **Swap the profile** at `now` (K-BOOST: a warfare buff raises or
    /// drops the ship's hit points and resonances): the shield is brought up
    /// to `now` on the old curve and re-stamped, then clamped to the new
    /// capacity. Armour and hull damage stay as the absolute numbers they
    /// are, so a larger pool shows as more left and a smaller one as less.
    pub fn set_profile(&mut self, profile: Profile, now: BlueTime) {
        self.shield_charge = self.shield_at(now).min(profile.shield_capacity);
        self.shield_stamp = now;
        self.profile = profile;
    }

    /// Apply one volley at `now`: shield, then armour, then hull.
    ///
    /// The shield is first brought up to `now` along the client's curve and
    /// re-stamped, so a shot never lands on a stale charge -- the lazy
    /// integrator of §3.3.
    ///
    /// `[A]` **layer order and overflow.** Each layer takes the volley through
    /// its own resonances. If it cannot absorb all of it, it is emptied, and
    /// the *raw* volley that remains -- scaled by the share the layer could not
    /// absorb -- moves to the next layer, where that layer's resonances apply
    /// afresh. So overflow is resisted by the layer it lands in, not by the
    /// one it came from. Falsified by a combat log in which a shot that breaks
    /// the shield of a ship with very different shield and armour resists
    /// shows a different total.
    ///
    /// `[A]` a volley against a destroyed ship does nothing and reports no
    /// death: the death was reported once, by the volley that caused it.
    pub fn apply(&mut self, volley: Damage, now: BlueTime) -> Applied {
        let shield_before = self.shield_at(now);
        self.shield_charge = shield_before;
        self.shield_stamp = now;
        let armour_before = self.armor_damage;
        let hull_before = self.hull_damage;

        let mut applied = Applied {
            shield: 0.0,
            armour: 0.0,
            hull: 0.0,
            overkill: 0.0,
            death: None,
            ring_changes: RingChanges::default(),
        };
        if self.is_destroyed() {
            return applied;
        }

        let mut raw = volley;
        for layer in [Layer::Shield, Layer::Armour, Layer::Hull] {
            let (res, left) = match layer {
                Layer::Shield => (self.profile.shield_resonance, self.shield_charge),
                Layer::Armour => (self.profile.armor_resonance, self.armour_left()),
                Layer::Hull => (self.profile.hull_resonance, self.hull_left()),
            };
            let effective = raw.through(res).total();
            if effective <= 0.0 {
                break;
            }
            let taken = effective.min(left);
            match layer {
                Layer::Shield => {
                    self.shield_charge -= taken;
                    applied.shield = taken;
                }
                // Emptying a layer sets its damage to the maximum exactly,
                // so that the client reads `0.0` rather than `1e-17` and the
                // death test below is exact.
                Layer::Armour => {
                    if taken >= left {
                        self.armor_damage = self.profile.armor_hp.max(self.armor_damage);
                    } else {
                        self.armor_damage += taken;
                    }
                    applied.armour = taken;
                }
                Layer::Hull => {
                    if taken >= left {
                        self.hull_damage = self.profile.hp.max(self.hull_damage);
                    } else {
                        self.hull_damage += taken;
                    }
                    applied.hull = taken;
                    if effective > taken {
                        applied.overkill = effective - taken;
                    }
                }
            }
            if effective <= left {
                break;
            }
            // The share of the volley this layer could not absorb.
            raw = raw.scaled(1.0 - taken / effective);
        }

        let change = |id, old: f64, new: f64| {
            (old != new).then_some(RingChange {
                attribute_id: id,
                old,
                new,
            })
        };
        applied.ring_changes = RingChanges {
            shield: change(attr::SHIELD_CHARGE, shield_before, self.shield_charge),
            armour: change(attr::ARMOR_DAMAGE, armour_before, self.armor_damage),
            hull: change(attr::DAMAGE, hull_before, self.hull_damage),
        };
        if self.is_destroyed() && hull_before < self.profile.hp {
            applied.death = Some(Death::Ship);
        }
        applied
    }
}

/// The input seam for module activation: "an effect cycled at `at` against
/// `target`", with the numbers already settled.
///
/// `modules.rs` produces one of these per weapon cycle; the caller looks up
/// the target's [`ShipDamage`], calls [`ShipDamage::apply`] with `damage` and
/// `at` (or skips it on a miss), and builds the two combat messages from the
/// result with [`Shot::attacker_message`] and [`Shot::victim_message`].
///
/// `[A]` `damage` is final pre-resistance damage: charge damage times the
/// weapon's multiplier times any hit-quality roll. How it got there --
/// tracking, falloff, wrecking hits -- is the module's business, not this
/// module's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    /// When the cycle landed, blue FILETIME.
    pub at: BlueTime,
    /// The shooter's ship (ball) id.
    pub attacker_ship_id: i64,
    /// The gun's item id -- `OnSpecialFX`'s `moduleID` (§2.4); not used here.
    pub weapon_item_id: i64,
    /// The gun's **type** id, for the message's `weapon` key (§2.1).
    pub weapon_type_id: i64,
    /// The victim's ship (ball) id.
    pub target_ship_id: i64,
    pub damage: Damage,
    /// `0` is a miss; `1..=6` names a hit quality; above 6 is a hit with no
    /// quality text (§2.2).
    pub hit_quality: u8,
}

impl Shot {
    /// §2.2: `hitQuality == 0` is a miss.
    pub fn is_miss(&self) -> bool {
        self.hit_quality == 0
    }

    /// Step 6 of §0: the shooter's line, "42 to Bob's Punisher".
    ///
    /// `[A]` `damage` is `0.0` on a miss (§9.4).
    pub fn attacker_message(&self, damage: f64) -> DamageMessage {
        DamageMessage {
            weapon: Some(self.weapon_type_id),
            ..DamageMessage::new(
                self.hit_quality,
                self.miss_or(damage),
                AttackType::Me {
                    target: self.target_ship_id,
                },
            )
        }
    }

    /// Step 7 of §0: the victim's line, "42 from Alice". Not a mirror of the
    /// shooter's: `source` and `attackerID` instead of `target`.
    ///
    /// DOC-3: it also names the `weapon` type and carries the shot's damage
    /// split as `damageAttributes`, so the victim (a bot's sense of what is
    /// hitting it) can tell the damage types and the gun. Both keys are
    /// optional in the client's reader [V]: `uilog.py` `_ReadIconsFromMessageDict`
    /// adds a damage-type icon for each positive 114/116/117/118 entry, and
    /// `AddCombatMessageFromDict` appends the weapon's name when the player's
    /// "show weapon" setting is on. `[A]` falsified by the combat log
    /// raising on a victim line (`fci_doc_3`).
    pub fn victim_message(&self, damage: f64) -> DamageMessage {
        DamageMessage {
            attacker_id: Some(self.attacker_ship_id),
            weapon: Some(self.weapon_type_id),
            damage_attributes: Some(self.damage),
            ..DamageMessage::new(
                self.hit_quality,
                self.miss_or(damage),
                AttackType::OtherPlayerWeapons {
                    source: self.attacker_ship_id,
                },
            )
        }
    }

    fn miss_or(&self, damage: f64) -> f64 {
        if self.is_miss() { 0.0 } else { damage }
    }
}

/// `attackType`, with the one id each value requires (§2.1) `[V]`: `target`
/// iff `'me'`, `source` iff `'otherPlayerWeapons'`, `owner` iff
/// `'otherPlayer'`. An enum so that a fourth value -- which aborts the message
/// (`uilog.py:414-424`) -- cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackType {
    /// Outgoing. `target` is a ball/item id.
    Me { target: i64 },
    /// Incoming from a player's weapons. `source` is a ball/item id.
    OtherPlayerWeapons { source: i64 },
    /// Incoming from something a player owns (drones). Sent as
    /// `(2, owner)`, `2` being `const.UE_OWNERID`.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "drone damage messages, docs/re/damage-death.md section 9 item 8; no packet yet"
        )
    )]
    OtherPlayer { owner: i64 },
}

impl AttackType {
    pub fn wire_name(self) -> &'static str {
        match self {
            AttackType::Me { .. } => "me",
            AttackType::OtherPlayerWeapons { .. } => "otherPlayerWeapons",
            AttackType::OtherPlayer { .. } => "otherPlayer",
        }
    }
}

/// `const.UE_OWNERID` (`eveexceptions/const.py:3`) `[V]`, the tag in an
/// `owner` value.
pub const UE_OWNERID: i64 = 2;

/// `OnDamageMessage`'s one argument, the dict of §2.1, as plain data.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageMessage {
    /// Mandatory: bare subscript at `uilog.py:395` `[V]`.
    pub hit_quality: u8,
    /// Mandatory: bare subscript at `uilog.py:396` `[V]`. `[A]` always
    /// `false` until weapons are grouped (§9.7).
    pub is_banked: bool,
    /// Always sent, because `bracketMgr.py:1382` subscripts it bare whenever
    /// `attackerID` is present `[V]` and nothing is lost by sending it
    /// otherwise.
    pub damage: f64,
    pub attack: AttackType,
    /// The attacker's ball, for the bracket blink.
    pub attacker_id: Option<i64>,
    /// A **type** id, not an item id.
    pub weapon: Option<i64>,
    /// A third party's shot; only changes whose turrets flash. Sent as `True`.
    pub observed: bool,
    /// `{attributeID: amount}` for 114/116/117/118; positive entries add an
    /// icon.
    pub damage_attributes: Option<Damage>,
    /// Added to `damage` by the client before display.
    pub deflected_dmg: Option<f64>,
    /// Name fallback when the `source` ball is not in the reader's park. A
    /// message with neither is **dropped** (`uilog.py:461-466`) `[V]`.
    pub source_char_id: Option<i64>,
    /// Name fallback when the `target` ball is not in the reader's park.
    pub target_owner_id: Option<i64>,
}

/// One value in a [`DamageMessage`], before encoding.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageValue {
    Int(i64),
    Real(f64),
    Bool(bool),
    Str(&'static str),
    /// `(UE_OWNERID, ownerID)`.
    OwnerRef(i64),
    /// `{attributeID: amount}`.
    Amounts(Vec<(i64, f64)>),
}

impl DamageMessage {
    /// The mandatory keys plus `attackType` and its id; every optional key
    /// absent.
    pub fn new(hit_quality: u8, damage: f64, attack: AttackType) -> DamageMessage {
        DamageMessage {
            hit_quality,
            is_banked: false,
            damage,
            attack,
            attacker_id: None,
            weapon: None,
            observed: false,
            damage_attributes: None,
            deflected_dmg: None,
            source_char_id: None,
            target_owner_id: None,
        }
    }

    /// The `dialogs.static` key the client will assemble from this dict,
    /// transcribed from `uilog.py:395-424` (§2.2) `[V]`. It is not sent; it
    /// says which of the eight bodies the player reads.
    #[cfg(test)]
    pub fn msg_key(&self) -> &'static str {
        let hit = self.hit_quality > 0;
        match (hit, self.attack, self.is_banked) {
            (true, AttackType::OtherPlayer { .. }, _) => "AttackHitsRD",
            (true, AttackType::OtherPlayerWeapons { .. }, _) => "AttackHitsR",
            (true, AttackType::Me { .. }, true) => "AttackHitsBanked",
            (true, AttackType::Me { .. }, false) => "AttackHits",
            (false, AttackType::OtherPlayer { .. }, _) => "AttackMissRD",
            (false, AttackType::OtherPlayerWeapons { .. }, _) => "AttackMissR",
            (false, AttackType::Me { .. }, true) => "AttackMissBanked",
            (false, AttackType::Me { .. }, false) => "AttackMiss",
        }
    }

    /// The dict's entries, in a fixed order, ready for the wiring to encode.
    pub fn entries(&self) -> Vec<(&'static str, MessageValue)> {
        let mut out = vec![
            ("hitQuality", MessageValue::Int(i64::from(self.hit_quality))),
            ("isBanked", MessageValue::Bool(self.is_banked)),
            ("damage", MessageValue::Real(self.damage)),
            ("attackType", MessageValue::Str(self.attack.wire_name())),
        ];
        out.push(match self.attack {
            AttackType::Me { target } => ("target", MessageValue::Int(target)),
            AttackType::OtherPlayerWeapons { source } => ("source", MessageValue::Int(source)),
            AttackType::OtherPlayer { owner } => ("owner", MessageValue::OwnerRef(owner)),
        });
        if let Some(id) = self.attacker_id {
            out.push(("attackerID", MessageValue::Int(id)));
        }
        if let Some(t) = self.weapon {
            out.push(("weapon", MessageValue::Int(t)));
        }
        if self.observed {
            out.push(("observed", MessageValue::Bool(true)));
        }
        if let Some(d) = self.damage_attributes {
            out.push((
                "damageAttributes",
                MessageValue::Amounts(d.by_attribute().to_vec()),
            ));
        }
        if let Some(d) = self.deflected_dmg {
            out.push(("deflectedDmg", MessageValue::Real(d)));
        }
        if let Some(c) = self.source_char_id {
            out.push(("sourceCharID", MessageValue::Int(c)));
        }
        if let Some(o) = self.target_owner_id {
            out.push(("targetOwnerID", MessageValue::Int(o)));
        }
        out
    }
}

/// What died. The wiring decides the ids; this says what must follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Death {
    /// A ship: a wreck and a capsule must follow (§0 steps 8-14).
    Ship,
    /// A capsule: no wreck; a clone jump must follow (§0 steps 15-19).
    Capsule,
}

/// Who receives a [`DeathStep`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    /// Every client whose park holds the victim's ball, victim included.
    Park,
    /// The victim's own client.
    Victim,
    /// Every client that had the victim locked.
    Lockers,
}

/// One message of the death sequence, as a decision, not a payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathAction {
    /// `AddBalls2` for the wreck, slim item with `launcherID` = the dead
    /// ship, `isEmpty`, and a 4-tuple `lootRights` (§6) `[V]`.
    AddWreck,
    /// `('RemoveBall', (victimID, True))` -- the single-entry, trap-free
    /// form (§4.1) `[V]`.
    RemoveVictimBall,
    /// `AddBalls2` for the capsule, typeID [`CAPSULE_TYPE_ID`] (§0 step 10).
    AddCapsuleBall,
    /// The capsule's godma item and its dogma attributes
    /// (`Registry::capsule_dogma_row`), so the rings are not empty (§0 step 11).
    CapsuleDogma,
    /// Session change `{'shipid': (old, capsule)}` -- the one message without
    /// which the victim stares at a dead ship (§4.2) `[V]`.
    ShipIdToCapsule,
    /// `OnTarget('lost', victimID, 'Exploding')` (§0 step 13).
    TargetLost,
    /// `OnPodOrCorvetteAssembly(capsuleID, 670)`: cosmetic skin (§5.1) `[I]`.
    PodOrCorvetteAssembly,
    /// `OnPodKilled(capsuleID)`: only labels the next session change a
    /// podding (`gameui:1203`) `[V]`.
    PodKilled,
    /// `OnPlayerPodDeath()`, **zero arguments**: the white-out
    /// (`playerdeath.py:56`) `[V]`.
    PlayerPodDeath,
    /// The docked session change: `shipid` capsule -> new ship, `stationid`
    /// None -> clone station, `solarsystemid` out (§5.2) `[V]`.
    /// `[A]` the clone station is the home station (§5.2).
    CloneJump,
    /// The frozen corpse ball, `groupBiomass` (§0 step 19) `[I]`.
    Corpse,
}

/// One step of [`Death::steps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeathStep {
    pub action: DeathAction,
    pub to: Audience,
    /// `false` when omitting it is cosmetic.
    pub required: bool,
    /// Part of the **kill batch**: one `DoDestinyUpdate`, one stamp (§8.7,
    /// `DoDestinyUpdate:660`) `[V]`.
    pub kill_batch: bool,
}

const fn step(action: DeathAction, to: Audience, required: bool, kill_batch: bool) -> DeathStep {
    DeathStep {
        action,
        to,
        required,
        kill_batch,
    }
}

const SHIP_DEATH: &[DeathStep] = &[
    // The wreck comes first so that its `launcherID` still resolves to the
    // dead ship's ball (§6, §8.8) -- §0 allows "same batch as 8 or before".
    step(DeathAction::AddWreck, Audience::Park, true, true),
    step(DeathAction::RemoveVictimBall, Audience::Park, true, true),
    step(DeathAction::AddCapsuleBall, Audience::Park, true, true),
    step(DeathAction::CapsuleDogma, Audience::Victim, true, false),
    // §4.2: free to land before or after the capsule's ball.
    step(DeathAction::ShipIdToCapsule, Audience::Victim, true, false),
    step(DeathAction::TargetLost, Audience::Lockers, true, false),
    step(
        DeathAction::PodOrCorvetteAssembly,
        Audience::Victim,
        false,
        false,
    ),
];

const CAPSULE_DEATH: &[DeathStep] = &[
    step(DeathAction::RemoveVictimBall, Audience::Park, true, true),
    // `[I]` that a capsule's lockers need the same release a ship's do.
    step(DeathAction::TargetLost, Audience::Lockers, true, false),
    // §5.2's order: `OnPodKilled`, `OnPlayerPodDeath`, then the session
    // change. The first two are "ugly, not broken" if omitted.
    step(DeathAction::PodKilled, Audience::Victim, false, false),
    step(DeathAction::PlayerPodDeath, Audience::Victim, false, false),
    step(DeathAction::CloneJump, Audience::Victim, true, false),
    step(DeathAction::Corpse, Audience::Park, false, false),
];

impl Death {
    /// Which death a hull of `type_id` dies.
    pub fn for_type(type_id: i64) -> Death {
        if type_id == CAPSULE_TYPE_ID {
            Death::Capsule
        } else {
            Death::Ship
        }
    }

    /// The death sequence, in sending order.
    pub fn steps(self) -> &'static [DeathStep] {
        match self {
            Death::Ship => SHIP_DEATH,
            Death::Capsule => CAPSULE_DEATH,
        }
    }

    /// Whether a wreck must follow.
    pub fn leaves_wreck(self) -> bool {
        self.steps()
            .iter()
            .any(|s| s.action == DeathAction::AddWreck)
    }

    /// Whether a capsule must follow.
    pub fn leaves_capsule(self) -> bool {
        self.steps()
            .iter()
            .any(|s| s.action == DeathAction::AddCapsuleBall)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: BlueTime = 10_000_000;
    const T0: BlueTime = 133_000_000_000_000_000;

    /// A frigate-sized profile with a real recharge rate.
    fn profile() -> Profile {
        let content = crate::content::Content::fixture();
        Profile::from_hull_in(&content, crate::destiny::hull::of_in(&content, 587))
    }

    /// CP58c: a hull's resonances are the world's typedata row's, layer by
    /// layer, and a hull the table does not hold takes every damage type at
    /// face value. The fixture hull is read back through `typedata`'s own
    /// `Content` accessors, so nothing here is a number.
    #[test]
    fn a_profile_reads_its_resonances_from_the_worlds_typedata() {
        let content = crate::content::Content::fixture();
        let hull = crate::typedata::installed_in(&content)
            .table()
            .types()
            .find_map(|t| crate::typedata::hull_in(&content, t.type_id))
            .expect("the typedata fixture has a hull");
        let want = crate::typedata::resonances_in(&content, hull.type_id).expect("a row");
        let p = Profile::from_hull_in(&content, hull);
        assert_eq!(
            [p.shield_resonance, p.armor_resonance, p.hull_resonance],
            want
        );
        assert_ne!(want[0], Damage::UNRESISTED, "the fixture hull resists");
        assert_eq!(p.shield_capacity, hull.shield_capacity);

        let mut stranger = *hull;
        stranger.type_id = -1;
        let p = Profile::from_hull_in(&content, &stranger);
        assert_eq!(p.shield_resonance, Damage::UNRESISTED);
        assert_eq!(p.armor_resonance, Damage::UNRESISTED);
        assert_eq!(p.hull_resonance, Damage::UNRESISTED);
    }

    fn volley(n: f64) -> Damage {
        Damage {
            em: n,
            explosive: n,
            kinetic: n,
            thermal: n,
        }
    }

    /// A ship that has been hit until its armour is partly gone.
    fn damaged() -> ShipDamage {
        let mut s = ShipDamage::full(profile(), T0);
        while s.bars(T0).armour >= 1.0 && !s.is_destroyed() {
            s.apply(volley(20.0), T0);
        }
        s
    }

    /// FT-G: swapping the profile for one with other resonances keeps every
    /// number of damage taken and takes the next volley at the new
    /// resonances.
    #[test]
    fn set_profile_keeps_the_damage_taken() {
        let mut s = damaged();
        let (armour, hull, shield) = (s.armour_left(), s.hull_left(), s.shield_at(T0));
        let mut p = *s.profile();
        p.armor_resonance = Damage {
            em: 0.5,
            ..p.armor_resonance
        };
        s.set_profile(p, T0);
        assert_eq!(s.armour_left(), armour);
        assert_eq!(s.hull_left(), hull);
        assert_eq!(s.shield_at(T0), shield);
        assert_eq!(s.profile().armor_resonance.em, 0.5);
    }

    /// The bars' curve, transcribed from
    /// `CalculateCurrentDamageStateValues:14-17` (§3.1) independently of
    /// [`charge_at`].
    fn client_bar_curve(health: f64, tau_ms: f64, stored: BlueTime, now: BlueTime) -> f64 {
        if tau_ms > 0.0 {
            let e = ((stored - now) as f64 / 10_000.0 / tau_ms).exp();
            (1.0 + (health.sqrt() - 1.0) * e).powi(2)
        } else {
            health
        }
    }

    /// §8.5: the server's lazy shield equals the client's curve at
    /// t = 0, 1 s, 10 s and 10·tau, from a damaged state.
    #[test]
    fn the_shield_is_on_the_curve_the_client_draws() {
        let s = damaged();
        let bars = s.bars(T0);
        let shield = bars.shield.expect("a hull with a shield");
        let tau = shield.tau_ms;
        assert!(shield.fraction < 1.0, "the fixture must be damaged");
        for dt in [0, SECOND, 10 * SECOND, (10.0 * tau) as i64 * 10_000] {
            let ours = s.shield_at(T0 + dt) / s.profile().shield_capacity;
            let theirs = client_bar_curve(shield.fraction, tau, T0, T0 + dt);
            assert!((ours - theirs).abs() < 1e-9, "dt {dt}: {ours} vs {theirs}");
        }
    }

    /// §8.3: `tau` is divided by five on the bars and **not** on the rings.
    #[test]
    fn tau_is_divided_by_five_exactly_once() {
        let s = damaged();
        let r = s.profile().shield_recharge_rate;
        assert_eq!(s.bars(T0).shield.unwrap().tau_ms, r / 5.0);
        assert_eq!(s.rings(T0).shield_recharge_rate, r);
        let row = s.rings(T0).by_attribute();
        assert!(row.contains(&(attr::SHIELD_RECHARGE_RATE, r)));
    }

    /// §8.2: from one damaged server state, bar and ring fractions agree to
    /// 1e-6 -- now and after regeneration, the ring evaluated by a
    /// transcription of `GetChargeValue` (§3.2).
    #[test]
    fn the_two_families_agree_at_a_damaged_state() {
        let s = damaged();
        for dt in [0, 30 * SECOND] {
            let now = T0 + dt;
            let bars = s.bars(now);
            let rings = s.rings(T0);
            // `GetChargeValue:2413-2422`, from the ring's stored value at T0.
            let tau = rings.shield_recharge_rate / 5.0;
            let sq = (rings.shield_charge / rings.shield_capacity)
                .max(0.0)
                .sqrt();
            let e = (((T0 - now).min(0)) as f64 / 10_000.0 / tau).exp();
            let ring_shield = (1.0 + (sq - 1.0) * e).powi(2);
            assert!((bars.shield.unwrap().fraction - ring_shield).abs() < 1e-6);
            assert!((bars.armour - (1.0 - rings.armor_damage / rings.armor_hp)).abs() < 1e-6);
            assert!((bars.hull - (1.0 - rings.damage / rings.hp)).abs() < 1e-6);
        }
        assert!(
            s.bars(T0).armour < 1.0,
            "a full-health fixture cannot fail this"
        );
    }

    /// §3.1: armour and hull are bare floats with no extrapolation -- they
    /// never regenerate.
    #[test]
    fn armour_and_hull_do_not_regenerate() {
        let s = damaged();
        let a = s.bars(T0);
        let b = s.bars(T0 + 3_600 * SECOND);
        assert_eq!((a.armour, a.hull), (b.armour, b.hull));
    }

    /// §3.1: the shield climbs monotonically toward 1.0 and never overshoots.
    #[test]
    fn the_shield_climbs_and_never_overshoots() {
        let s = damaged();
        let mut last = s.shield_at(T0);
        for k in 1..200 {
            let v = s.shield_at(T0 + k * 60 * SECOND);
            assert!(v >= last && v <= s.profile().shield_capacity + 1e-9);
            last = v;
        }
    }

    /// §3.2, `GetChargeValue:2420`'s `min(…, 0)`: a stamp from the future
    /// cannot drain the gauge.
    #[test]
    fn a_time_before_the_stamp_does_not_drain() {
        let s = damaged();
        assert_eq!(s.shield_at(T0 - 100 * SECOND), s.shield_at(T0));
    }

    /// §3.1 (`if tau > 0`) and §3.2 (`UpdateAttribute:2055`): a zero recharge
    /// rate freezes the shield.
    #[test]
    fn a_zero_recharge_rate_freezes_the_shield() {
        let mut p = profile();
        p.shield_recharge_rate = 0.0;
        let mut s = ShipDamage::full(p, T0);
        s.apply(volley(1.0), T0);
        assert_eq!(s.shield_at(T0 + 3_600 * SECOND), s.shield_at(T0));
    }

    /// `GetDamageStateEx:1766` via `destiny::DamageState`: a hull with no
    /// shield gets a bare float in the shield slot -- a hidden bar.
    #[test]
    fn no_shield_capacity_hides_the_shield_bar() {
        let mut p = profile();
        p.shield_capacity = 0.0;
        assert_eq!(ShipDamage::full(p, T0).bars(T0).shield, None);
    }

    /// §8.1: one volley moves both families together. Every layer whose bar
    /// moved has a ring row, every ring row's layer moved, and nothing went up.
    #[test]
    fn a_volley_moves_both_families_together() {
        let mut s = ShipDamage::full(profile(), T0);
        for _ in 0..200 {
            let before = s.bars(T0);
            let applied = s.apply(volley(15.0), T0);
            let after = s.bars(T0);
            let pairs = [
                (
                    before.shield.unwrap().fraction,
                    after.shield.unwrap().fraction,
                    applied.ring_changes.shield,
                ),
                (before.armour, after.armour, applied.ring_changes.armour),
                (before.hull, after.hull, applied.ring_changes.hull),
            ];
            for (b, a, row) in pairs {
                assert!(a <= b);
                assert_eq!(a < b, row.is_some(), "bar {b}->{a}, ring {row:?}");
            }
            if s.is_destroyed() {
                break;
            }
        }
    }

    /// §0: the ship dies when the server's own hull number reaches zero --
    /// the death is reported exactly once, by the volley that zeroed the
    /// structure bar, and a ship death leaves a wreck and a capsule.
    #[test]
    fn death_is_the_hull_reaching_zero_and_is_reported_once() {
        let mut s = ShipDamage::full(profile(), T0);
        let mut deaths = 0;
        for _ in 0..1_000 {
            let applied = s.apply(volley(25.0), T0);
            assert_eq!(s.is_destroyed(), s.bars(T0).hull == 0.0);
            if let Some(d) = applied.death {
                deaths += 1;
                assert_eq!(s.bars(T0).hull, 0.0);
                assert!(d.leaves_wreck() && d.leaves_capsule());
            }
        }
        assert_eq!(deaths, 1);
    }

    /// §5.1: type 670 is the capsule, and a capsule's death leaves no
    /// capsule and no wreck but must end in a clone jump (§5.2).
    #[test]
    fn a_capsule_dies_into_a_clone_jump() {
        let d = Death::for_type(CAPSULE_TYPE_ID);
        assert_eq!(d, Death::Capsule);
        assert!(!d.leaves_wreck() && !d.leaves_capsule());
        let jump = d
            .steps()
            .iter()
            .find(|s| s.action == DeathAction::CloneJump)
            .unwrap();
        assert!(jump.required && jump.to == Audience::Victim);
    }

    /// §4.2: the `shipid` session change is required and goes to the victim.
    /// §8.7: the wreck, the removal and the capsule travel in one batch.
    /// §8.8: the wreck's add is never after the removal of its `launcherID`.
    #[test]
    fn the_ship_death_sequence_is_ordered_and_batched() {
        let steps = Death::Ship.steps();
        let at = |a| steps.iter().position(|s| s.action == a).unwrap();
        assert!(at(DeathAction::AddWreck) < at(DeathAction::RemoveVictimBall));
        for a in [
            DeathAction::AddWreck,
            DeathAction::RemoveVictimBall,
            DeathAction::AddCapsuleBall,
        ] {
            assert!(steps[at(a)].kill_batch && steps[at(a)].to == Audience::Park);
        }
        let sc = steps[at(DeathAction::ShipIdToCapsule)];
        assert!(sc.required && sc.to == Audience::Victim);
    }

    fn shot(hit_quality: u8) -> Shot {
        Shot {
            at: T0,
            attacker_ship_id: 9_000_001,
            weapon_item_id: 9_000_002,
            weapon_type_id: 2873,
            target_ship_id: 9_000_003,
            damage: volley(10.0),
            hit_quality,
        }
    }

    fn keys(m: &DamageMessage) -> Vec<&'static str> {
        m.entries().into_iter().map(|(k, _)| k).collect()
    }

    /// §2.2: the message key the client assembles, for every combination,
    /// lands on one of the eight `dialogs.static` keys.
    #[test]
    fn every_message_lands_on_one_of_the_eight_keys() {
        let cases = [
            (0, AttackType::Me { target: 1 }, false, "AttackMiss"),
            (0, AttackType::Me { target: 1 }, true, "AttackMissBanked"),
            (
                0,
                AttackType::OtherPlayerWeapons { source: 1 },
                false,
                "AttackMissR",
            ),
            (
                0,
                AttackType::OtherPlayer { owner: 1 },
                false,
                "AttackMissRD",
            ),
            (4, AttackType::Me { target: 1 }, false, "AttackHits"),
            (6, AttackType::Me { target: 1 }, true, "AttackHitsBanked"),
            (
                1,
                AttackType::OtherPlayerWeapons { source: 1 },
                false,
                "AttackHitsR",
            ),
            (
                7,
                AttackType::OtherPlayer { owner: 1 },
                false,
                "AttackHitsRD",
            ),
        ];
        for (q, attack, banked, key) in cases {
            let mut m = DamageMessage::new(q, 1.0, attack);
            m.is_banked = banked;
            assert_eq!(m.msg_key(), key);
        }
    }

    /// §2.1: `hitQuality` and `isBanked` are always keys (bare subscripts);
    /// `damage` is present whenever `attackerID` is; exactly one of
    /// `target`/`source`/`owner`, matching `attackType`; `owner` is
    /// `(2, ownerID)`.
    #[test]
    fn the_dict_carries_what_the_client_subscripts() {
        let mut drone = DamageMessage::new(3, 5.0, AttackType::OtherPlayer { owner: 42 });
        drone.attacker_id = Some(7);
        let msgs = [
            shot(4).attacker_message(40.0),
            shot(4).victim_message(40.0),
            shot(0).victim_message(0.0),
            drone,
        ];
        for m in &msgs {
            let k = keys(m);
            assert!(k.contains(&"hitQuality") && k.contains(&"isBanked"));
            if k.contains(&"attackerID") {
                assert!(k.contains(&"damage"));
            }
            let ids: Vec<_> = k
                .iter()
                .filter(|k| ["target", "source", "owner"].contains(k))
                .collect();
            let want = match m.attack {
                AttackType::Me { .. } => "target",
                AttackType::OtherPlayerWeapons { .. } => "source",
                AttackType::OtherPlayer { .. } => "owner",
            };
            assert_eq!(ids, vec![&want]);
            assert!(["me", "otherPlayerWeapons", "otherPlayer"].contains(&m.attack.wire_name()));
        }
        let owner = msgs[3]
            .entries()
            .into_iter()
            .find(|(k, _)| *k == "owner")
            .unwrap()
            .1;
        assert_eq!(owner, MessageValue::OwnerRef(42));
        assert_eq!(UE_OWNERID, 2);
    }

    /// §0 steps 6 and 7: the shooter's and the victim's dicts are different
    /// key sets, not mirrors.
    #[test]
    fn shooter_and_victim_get_different_dicts() {
        let s = shot(4);
        let mut a = keys(&s.attacker_message(42.0));
        let mut v = keys(&s.victim_message(42.0));
        a.sort_unstable();
        v.sort_unstable();
        assert_eq!(
            a,
            [
                "attackType",
                "damage",
                "hitQuality",
                "isBanked",
                "target",
                "weapon"
            ]
        );
        assert_eq!(
            v,
            [
                "attackType",
                "attackerID",
                "damage",
                "damageAttributes",
                "hitQuality",
                "isBanked",
                "source",
                "weapon"
            ]
        );
        assert_eq!(s.victim_message(42.0).attacker_id, Some(s.attacker_ship_id));
        assert_eq!(s.attacker_message(42.0).weapon, Some(s.weapon_type_id));
    }
}
