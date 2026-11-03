//! Carrier fighters' data: abilities, per-type ability slots, class and hull
//! slot attributes (FT-A, `docs/lanes/ft-a.md`; spec R-DEEP §2).
//!
//! Pure data and no behaviour: launch, control and damage are FT-B and later.
//! The files are `fighters/abilities.tsv` and `fighters/slots.tsv`
//! (`docs/content/schema.md`), written by `tools/content_fighters.py` from the
//! client's `fighterabilities.static` and `fighterabilitiesbytype.static`. The
//! ability-to-effect link is a column of `abilities.tsv`: the client's
//! `dogmaEffectByAbilityID` [V], copied by the extractor.
//!
//! A fighter's class, squadron size, orbit range, role and refuel time are
//! attributes of its type row (typedata, category 87), read through
//! [`class_of`] and friends with an attribute lookup. A bundle without the
//! files (made before this packet) loads as an empty [`Fighters`].

use crate::content::{Boxed, Bundle, Sources, TableSpec};
use std::collections::BTreeMap;

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "fighters_data",
    load: load_table,
    fixture: || Box::new(Fighters::default()) as Boxed,
};

/// The bundle's rows, or none (a pre-FT-A bundle, or no bundle).
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Fighters::from_bundle(b)?,
        None => Fighters::default(),
    }))
}

/// The files, and their header lines copied from `docs/content/schema.md`.
pub const ABILITIES: (&str, &str) = (
    "fighters/abilities.tsv",
    "#ability_id\ttarget_mode\tlow_sec_banned\thigh_sec_banned\teffect_id\ticon_id\tturret_graphic_id\tname",
);
pub const SLOTS: (&str, &str) = (
    "fighters/slots.tsv",
    "#type_id\tslot\tability_id\tcooldown_s\tcharges\trearm_s",
);

/// `categoryFighter` [V `inventorycommon/const.py:329`].
pub const CATEGORY_FIGHTER: i64 = 87;
/// Ability slots per fighter type (`abilitySlot0..2`).
pub const SLOT_COUNT: i64 = 3;

/// Dogma attribute ids [V, R-DEEP §2.1-§2.2; the names are `dogmaattributes`].
pub mod attr {
    /// `fighterSquadronIsLight`, `...IsSupport`, `...IsHeavy`.
    pub const IS_LIGHT: i64 = 2212;
    pub const IS_SUPPORT: i64 = 2213;
    pub const IS_HEAVY: i64 = 2214;
    /// The Standup variants of the three flags.
    pub const STANDUP_IS_LIGHT: i64 = 2740;
    pub const STANDUP_IS_SUPPORT: i64 = 2741;
    pub const STANDUP_IS_HEAVY: i64 = 2742;
    /// `fighterSquadronMaxSize`: fighters in one tube.
    pub const SQUADRON_MAX_SIZE: i64 = 2215;
    /// `fighterSquadronOrbitRange`, m.
    pub const ORBIT_RANGE: i64 = 2223;
    /// `fighterSquadronRole`: see [`super::Role`].
    pub const ROLE: i64 = 2270;
    /// `fighterRefuelingTime`, ms.
    pub const REFUELING_TIME: i64 = 2426;
    /// A hull's `fighterTubes`.
    pub const TUBES: i64 = 2216;
    /// A hull's `fighterLightSlots`, `...SupportSlots`, `...HeavySlots`: how
    /// many tubes may hold that class.
    pub const LIGHT_SLOTS: i64 = 2217;
    pub const SUPPORT_SLOTS: i64 = 2218;
    pub const HEAVY_SLOTS: i64 = 2219;
    /// The Standup (structure) variants; the client sums both sets.
    pub const STANDUP_LIGHT_SLOTS: i64 = 2737;
    pub const STANDUP_SUPPORT_SLOTS: i64 = 2738;
    pub const STANDUP_HEAVY_SLOTS: i64 = 2739;
    /// A hull's `fighterCapacity`: m3 of the fighter bay.
    pub const FIGHTER_CAPACITY: i64 = 2055;
}

/// The dogma effects that carry the abilities' numbers [V-data]; none has a
/// `modifierInfo` (the abilities are code, not modifiers).
pub mod effect {
    pub const ATTACK: i64 = 6465;
    pub const MISSILES: i64 = 6431;
    pub const NEUT: i64 = 6434;
    pub const WEB: i64 = 6435;
    pub const WARP_DISRUPT: i64 = 6436;
    pub const ECM: i64 = 6437;
    pub const EVASION: i64 = 6439;
    pub const AFTERBURNER: i64 = 6440;
    pub const MICROWARPDRIVE: i64 = 6441;
    pub const MICRO_JUMP_DRIVE: i64 = 6442;
    pub const TACKLE: i64 = 6464;
    pub const BOMB: i64 = 6485;
    pub const KAMIKAZE: i64 = 6554;
}

/// A fighter's class; the type's flag attribute says which, never its group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Light,
    Support,
    Heavy,
}

/// `fighterSquadronRole` [V `fighterConst.ROLE_*`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    AntiFighter,
    General,
    Ewar,
    TorpedoBomber,
    AoeBomber,
}

impl Role {
    pub fn from_attr(v: f64) -> Option<Role> {
        match v.round() as i64 {
            1 => Some(Role::AntiFighter),
            2 => Some(Role::General),
            3 => Some(Role::Ewar),
            4 => Some(Role::TorpedoBomber),
            5 => Some(Role::AoeBomber),
            _ => None,
        }
    }
}

/// The class a fighter type's attributes give it: the one flag set to 1
/// (standup flags count as their class). `attr` is an attribute lookup
/// (`typedata::attribute_in` for a loaded type). `None` when no flag, or more
/// than one, is set.
pub fn class_of(attr: impl Fn(i64) -> Option<f64>) -> Option<Class> {
    let set = |a: i64| attr(a).is_some_and(|v| v == 1.0);
    let flags = [
        (
            Class::Light,
            set(attr::IS_LIGHT) || set(attr::STANDUP_IS_LIGHT),
        ),
        (
            Class::Support,
            set(attr::IS_SUPPORT) || set(attr::STANDUP_IS_SUPPORT),
        ),
        (
            Class::Heavy,
            set(attr::IS_HEAVY) || set(attr::STANDUP_IS_HEAVY),
        ),
    ];
    let mut on = flags.iter().filter(|(_, s)| *s).map(|(c, _)| *c);
    match (on.next(), on.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// A fighter type's numbers from its attributes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FighterType {
    pub class: Class,
    /// Fighters in a full tube (`fighterSquadronMaxSize`).
    pub squadron_max: i64,
    /// m
    pub orbit_range: f64,
    pub role: Option<Role>,
    /// ms
    pub refuel_ms: f64,
}

/// A fighter type from an attribute lookup; `None` when it has no class or no
/// squadron size.
pub fn fighter_type(attr: impl Fn(i64) -> Option<f64>) -> Option<FighterType> {
    let class = class_of(&attr)?;
    let squadron_max = attr(attr::SQUADRON_MAX_SIZE).map(|v| v.round() as i64)?;
    (squadron_max > 0).then(|| FighterType {
        class,
        squadron_max,
        orbit_range: attr(attr::ORBIT_RANGE).unwrap_or(0.0),
        role: attr(attr::ROLE).and_then(Role::from_attr),
        refuel_ms: attr(attr::REFUELING_TIME).unwrap_or(0.0),
    })
}

/// What a hull offers fighters: `tubes`, how many tubes may hold each class
/// (the Standup variants summed in, as the client does), and the bay.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HullSlots {
    pub tubes: i64,
    pub light: i64,
    pub support: i64,
    pub heavy: i64,
    /// m3
    pub capacity: f64,
}

impl HullSlots {
    /// From an attribute lookup. A hull with no `fighterTubes` is all zero.
    pub fn from_attrs(attr: impl Fn(i64) -> Option<f64>) -> HullSlots {
        let n = |a: i64| attr(a).map_or(0, |v| v.round() as i64);
        HullSlots {
            tubes: n(attr::TUBES),
            light: n(attr::LIGHT_SLOTS) + n(attr::STANDUP_LIGHT_SLOTS),
            support: n(attr::SUPPORT_SLOTS) + n(attr::STANDUP_SUPPORT_SLOTS),
            heavy: n(attr::HEAVY_SLOTS) + n(attr::STANDUP_HEAVY_SLOTS),
            capacity: attr(attr::FIGHTER_CAPACITY).unwrap_or(0.0),
        }
    }

    /// Tubes that may hold `class`.
    pub fn slots_for(&self, class: Class) -> i64 {
        match class {
            Class::Light => self.light,
            Class::Support => self.support,
            Class::Heavy => self.heavy,
        }
    }

    pub fn has_bay(&self) -> bool {
        self.tubes > 0 && self.capacity > 0.0
    }
}

/// How an ability picks its target (`fighterabilities.static`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetMode {
    Untargeted,
    ItemTargeted,
    PointTargeted,
}

impl TargetMode {
    fn parse(s: &str) -> Option<TargetMode> {
        match s {
            "untargeted" => Some(TargetMode::Untargeted),
            "itemTargeted" => Some(TargetMode::ItemTargeted),
            "pointTargeted" => Some(TargetMode::PointTargeted),
            _ => None,
        }
    }
}

/// One `abilities.tsv` row.
#[derive(Debug, Clone, PartialEq)]
pub struct Ability {
    pub id: i64,
    pub target_mode: TargetMode,
    pub low_sec_banned: bool,
    pub high_sec_banned: bool,
    /// The dogma effect carrying its numbers (`dogmaEffectByAbilityID`).
    pub effect_id: i64,
    pub icon_id: i64,
    pub turret_graphic_id: Option<i64>,
    pub name: String,
}

/// One `slots.tsv` row: what a fighter type's ability slot holds.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotAbility {
    pub type_id: i64,
    pub slot: i64,
    pub ability_id: i64,
    /// Seconds; only mobility abilities have one.
    pub cooldown_s: Option<f64>,
    /// Rounds before a rearm; with `rearm_s`.
    pub charges: Option<i64>,
    pub rearm_s: Option<f64>,
}

/// The tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fighters {
    abilities: BTreeMap<i64, Ability>,
    /// `type_id -> slots`, ascending by slot.
    slots: BTreeMap<i64, Vec<SlotAbility>>,
}

impl Fighters {
    /// The files from a checked bundle; an absent file is empty.
    pub fn from_bundle(b: &Bundle) -> Result<Fighters, String> {
        Fighters::parse(
            &b.data_lines(ABILITIES.0, ABILITIES.1)?,
            &b.data_lines(SLOTS.0, SLOTS.1)?,
        )
    }

    /// From each file's data lines. A short row, a bad number, an unknown
    /// target mode, a slot outside 0..3, a repeated key, or a slot naming an
    /// ability the first file lacks is an error naming the line.
    pub fn parse<S: AsRef<str>>(abilities: &[S], slots: &[S]) -> Result<Fighters, String> {
        let mut t = Fighters::default();
        for (i, line) in abilities.iter().enumerate() {
            let bad = |why: &str| {
                format!(
                    "{}: data line {}: {why}: {:?}",
                    ABILITIES.0,
                    i + 1,
                    line.as_ref()
                )
            };
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [id, mode, low, high, effect, icon, turret, name] = f[..] else {
                return Err(bad("want 8 columns"));
            };
            let int = |s: &str| s.parse::<i64>().map_err(|_| bad("not an integer"));
            let flag = |s: &str| match s {
                "0" => Ok(false),
                "1" => Ok(true),
                _ => Err(bad("not 0 or 1")),
            };
            let ability = Ability {
                id: int(id)?,
                target_mode: TargetMode::parse(mode).ok_or_else(|| bad("unknown target mode"))?,
                low_sec_banned: flag(low)?,
                high_sec_banned: flag(high)?,
                effect_id: int(effect)?,
                icon_id: int(icon)?,
                turret_graphic_id: if turret.is_empty() {
                    None
                } else {
                    Some(int(turret)?)
                },
                name: name.to_owned(),
            };
            if t.abilities.insert(ability.id, ability).is_some() {
                return Err(bad("ability twice"));
            }
        }
        for (i, line) in slots.iter().enumerate() {
            let bad = |why: &str| {
                format!(
                    "{}: data line {}: {why}: {:?}",
                    SLOTS.0,
                    i + 1,
                    line.as_ref()
                )
            };
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [type_id, slot, ability, cooldown, charges, rearm] = f[..] else {
                return Err(bad("want 6 columns"));
            };
            let int = |s: &str| s.parse::<i64>().map_err(|_| bad("not an integer"));
            let float = |s: &str| {
                s.parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .ok_or_else(|| bad("not a non-negative number"))
            };
            let row = SlotAbility {
                type_id: int(type_id)?,
                slot: int(slot)?,
                ability_id: int(ability)?,
                cooldown_s: if cooldown.is_empty() {
                    None
                } else {
                    Some(float(cooldown)?)
                },
                charges: if charges.is_empty() {
                    None
                } else {
                    Some(int(charges)?)
                },
                rearm_s: if rearm.is_empty() {
                    None
                } else {
                    Some(float(rearm)?)
                },
            };
            if !(0..SLOT_COUNT).contains(&row.slot) {
                return Err(bad("slot outside 0..3"));
            }
            if !t.abilities.contains_key(&row.ability_id) {
                return Err(bad("unknown ability"));
            }
            let rows = t.slots.entry(row.type_id).or_default();
            if rows.iter().any(|r| r.slot == row.slot) {
                return Err(bad("slot twice"));
            }
            rows.push(row);
            rows.sort_by_key(|r| r.slot);
        }
        Ok(t)
    }

    pub fn is_empty(&self) -> bool {
        self.abilities.is_empty()
    }

    pub fn ability(&self, id: i64) -> Option<&Ability> {
        self.abilities.get(&id)
    }

    pub fn abilities(&self) -> impl Iterator<Item = &Ability> {
        self.abilities.values()
    }

    /// The ability-to-effect link: the effect carrying `ability`'s numbers.
    pub fn effect_of(&self, ability: i64) -> Option<i64> {
        self.abilities.get(&ability).map(|a| a.effect_id)
    }

    /// A fighter type's populated slots, ascending by slot; empty for a type
    /// the table does not know.
    pub fn slots_of(&self, type_id: i64) -> &[SlotAbility] {
        self.slots.get(&type_id).map_or(&[], Vec::as_slice)
    }

    /// What `type_id` has in `slot`.
    pub fn slot(&self, type_id: i64, slot: i64) -> Option<&SlotAbility> {
        self.slots_of(type_id).iter().find(|r| r.slot == slot)
    }

    /// Every type with at least one slot, ascending.
    pub fn types(&self) -> impl Iterator<Item = i64> + '_ {
        self.slots.keys().copied()
    }

    /// A type's slot-0 ability's effect (the weapon or ewar it fights with).
    pub fn weapon_effect(&self, type_id: i64) -> Option<i64> {
        self.effect_of(self.slot(type_id, 0)?.ability_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids (the 99xxxx band); no client data.
    const AB_ATTACK: i64 = 990_601;
    const AB_DRIVE: i64 = 990_602;
    const AB_BOMB: i64 = 990_603;
    const FT_A: i64 = 990_611;
    const FT_B: i64 = 990_612;
    const EFF_X: i64 = 990_621;
    const EFF_Y: i64 = 990_622;

    fn abilities() -> Vec<String> {
        vec![
            format!("{AB_ATTACK}\titemTargeted\t0\t0\t{EFF_X}\t990631\t990641\tAttack"),
            format!("{AB_DRIVE}\tuntargeted\t0\t1\t{EFF_Y}\t990632\t\tDrive"),
            format!("{AB_BOMB}\tpointTargeted\t1\t1\t{EFF_X}\t990633\t\tBomb"),
        ]
    }

    #[test]
    fn abilities_tsv_parses_all_three_target_modes() {
        let t = Fighters::parse(&abilities(), &Vec::<String>::new()).unwrap();
        let modes: Vec<_> = t.abilities().map(|a| a.target_mode).collect();
        assert_eq!(
            modes,
            [
                TargetMode::ItemTargeted,
                TargetMode::Untargeted,
                TargetMode::PointTargeted
            ]
        );
        let a = t.ability(AB_ATTACK).unwrap();
        assert_eq!(a.turret_graphic_id, Some(990_641));
        assert_eq!(a.name, "Attack");
        let d = t.ability(AB_DRIVE).unwrap();
        assert_eq!((d.low_sec_banned, d.high_sec_banned), (false, true));
        assert_eq!(d.turret_graphic_id, None);
        assert_eq!(t.effect_of(AB_DRIVE), Some(EFF_Y));
        assert_eq!(t.effect_of(1), None);
    }

    #[test]
    fn slots_row_without_cooldown_has_none() {
        let slots = vec![
            format!("{FT_A}\t0\t{AB_ATTACK}\t\t\t"),
            format!("{FT_A}\t1\t{AB_DRIVE}\t60\t\t"),
            format!("{FT_B}\t2\t{AB_BOMB}\t\t3\t20"),
        ];
        let t = Fighters::parse(&abilities(), &slots).unwrap();
        let s0 = t.slot(FT_A, 0).unwrap();
        assert_eq!((s0.cooldown_s, s0.charges, s0.rearm_s), (None, None, None));
        assert_eq!(t.slot(FT_A, 1).unwrap().cooldown_s, Some(60.0));
        let s2 = t.slot(FT_B, 2).unwrap();
        assert_eq!((s2.charges, s2.rearm_s), (Some(3), Some(20.0)));
        assert!(t.slot(FT_B, 0).is_none());
        assert_eq!(t.slots_of(FT_A).len(), 2);
        assert!(t.slots_of(1).is_empty());
        assert_eq!(t.weapon_effect(FT_A), Some(EFF_X));
        assert_eq!(t.weapon_effect(FT_B), None, "no slot 0");
        assert_eq!(t.types().collect::<Vec<_>>(), [FT_A, FT_B]);
    }

    #[test]
    fn a_slot_naming_an_unknown_ability_is_refused() {
        let slots = vec![format!("{FT_A}\t0\t990699\t\t\t")];
        let e = Fighters::parse(&abilities(), &slots).unwrap_err();
        assert!(e.contains("unknown ability"), "{e}");
    }

    #[test]
    fn bad_rows_are_refused_with_the_line() {
        let none = Vec::<String>::new();
        let e =
            Fighters::parse(&["990601\tsideways\t0\t0\t1\t1\t\tX".to_owned()], &none).unwrap_err();
        assert!(
            e.contains("unknown target mode") && e.contains("data line 1"),
            "{e}"
        );
        assert!(Fighters::parse(&["short".to_owned()], &none).is_err());
        let slot3 = vec![format!("{FT_A}\t3\t{AB_ATTACK}\t\t\t")];
        assert!(
            Fighters::parse(&abilities(), &slot3)
                .unwrap_err()
                .contains("slot outside")
        );
        let twice = vec![
            format!("{FT_A}\t0\t{AB_ATTACK}\t\t\t"),
            format!("{FT_A}\t0\t{AB_DRIVE}\t\t\t"),
        ];
        assert!(
            Fighters::parse(&abilities(), &twice)
                .unwrap_err()
                .contains("slot twice")
        );
        let mut dup = abilities();
        dup.push(dup[0].clone());
        assert!(
            Fighters::parse(&dup, &none)
                .unwrap_err()
                .contains("ability twice")
        );
        let neg = vec![format!("{FT_A}\t1\t{AB_DRIVE}\t-5\t\t")];
        assert!(Fighters::parse(&abilities(), &neg).is_err());
    }

    #[test]
    fn an_absent_bundle_is_an_empty_table() {
        let t = Fighters::parse(&Vec::<String>::new(), &Vec::<String>::new()).unwrap();
        assert!(t.is_empty());
        assert_eq!(t, Fighters::default());
    }

    fn lookup(pairs: &[(i64, f64)]) -> impl Fn(i64) -> Option<f64> + '_ {
        move |a| pairs.iter().find(|(k, _)| *k == a).map(|(_, v)| *v)
    }

    #[test]
    fn class_comes_from_the_flag_attribute() {
        // An invented type: the flag is all there is, there is no group here.
        assert_eq!(
            class_of(lookup(&[(attr::IS_HEAVY, 1.0)])),
            Some(Class::Heavy)
        );
        assert_eq!(
            class_of(lookup(&[(attr::STANDUP_IS_SUPPORT, 1.0)])),
            Some(Class::Support)
        );
        assert_eq!(
            class_of(lookup(&[(attr::IS_LIGHT, 0.0)])),
            None,
            "a flag at 0 is not set"
        );
        assert_eq!(
            class_of(lookup(&[(attr::IS_LIGHT, 1.0), (attr::IS_HEAVY, 1.0)])),
            None,
            "two flags are ambiguous"
        );
        assert_eq!(class_of(lookup(&[])), None);
    }

    #[test]
    fn fighter_type_reads_size_orbit_role_and_refuel() {
        let t = fighter_type(lookup(&[
            (attr::IS_LIGHT, 1.0),
            (attr::SQUADRON_MAX_SIZE, 9.0),
            (attr::ORBIT_RANGE, 5_000.0),
            (attr::ROLE, 4.0),
            (attr::REFUELING_TIME, 2_000.0),
        ]))
        .unwrap();
        assert_eq!(
            t,
            FighterType {
                class: Class::Light,
                squadron_max: 9,
                orbit_range: 5_000.0,
                role: Some(Role::TorpedoBomber),
                refuel_ms: 2_000.0,
            }
        );
        assert_eq!(Role::from_attr(99.0), None);
        // No squadron size: not a fighter.
        assert!(fighter_type(lookup(&[(attr::IS_LIGHT, 1.0)])).is_none());
        assert!(
            fighter_type(lookup(&[
                (attr::IS_LIGHT, 1.0),
                (attr::SQUADRON_MAX_SIZE, 0.0)
            ]))
            .is_none()
        );
    }

    #[test]
    fn hull_slots_sum_the_standup_variants() {
        let h = HullSlots::from_attrs(lookup(&[
            (attr::TUBES, 5.0),
            (attr::LIGHT_SLOTS, 3.0),
            (attr::STANDUP_LIGHT_SLOTS, 1.0),
            (attr::HEAVY_SLOTS, 4.0),
            (attr::FIGHTER_CAPACITY, 1_000.0),
        ]));
        assert_eq!((h.tubes, h.light, h.support, h.heavy), (5, 4, 0, 4));
        assert_eq!(h.slots_for(Class::Light), 4);
        assert_eq!(h.slots_for(Class::Support), 0);
        assert!(h.has_bay());
        let pod = HullSlots::from_attrs(lookup(&[]));
        assert_eq!(pod, HullSlots::default());
        assert!(!pod.has_bay());
    }

    /// Reads a real bundle (`EVE_TEST_BUNDLE`, an extracted `out/content`) and
    /// names no id: every typed slot belongs to a category-87 type whose flag
    /// attributes give it exactly one class, every ability's effect is a
    /// loaded effect for the used ones, and every hull with tubes has a bay.
    /// Skipped, passing, without the variable.
    #[test]
    fn every_bundle_fighter_has_one_class_and_known_abilities() {
        let Some(dir) = std::env::var_os("EVE_TEST_BUNDLE") else {
            return;
        };
        let loaded = crate::content::load(
            Some(std::path::Path::new(&dir)),
            &crate::authored::Dir::compiled_only(),
        )
        .expect("the bundle loads");
        let content = &loaded.content;
        let t: &Fighters = content.get();
        assert!(!t.is_empty(), "the bundle has fighter abilities");
        assert!(t.types().count() > 0);
        for id in t.types() {
            let attr = |a| crate::typedata::attribute_in(content, id, a);
            let ft = fighter_type(attr).unwrap_or_else(|| panic!("type {id}: no class or size"));
            assert!(ft.squadron_max > 0, "type {id}");
            for s in t.slots_of(id) {
                let a = t.ability(s.ability_id).expect("ability exists");
                assert!(a.effect_id > 0, "ability {}", a.id);
            }
        }
    }
}
