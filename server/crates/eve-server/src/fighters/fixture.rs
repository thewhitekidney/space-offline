//! The invented rows of the FT-G and FT-H tests (`fighters/skills.rs`,
//! `abandon.rs`, `strike.rs`): skills, a carrier whose hull has bonuses, the
//! fighters that need them, a bomber and a kamikaze. **Not the client's
//! numbers**: ids from 990,592,000, round values. The attribute ids are the
//! code's own constants (they are the ids the code reads); the effect shapes
//! (`modifierInfo` rows) are the client's real ones: `skillBonusFighters`,
//! `skillBonusLightFighters`, `skillBonusFighterHangarManagement` and a hull
//! bonus, with the skill's level times its bonus as in the data
//! (`docs/lanes/ft-gh.md`).

use crate::fighters_data::effect;

pub(in crate::fighters) const SK_FIGHTERS: i64 = 990_592_100;
pub(in crate::fighters) const SK_LIGHT: i64 = 990_592_101;
pub(in crate::fighters) const SK_HANGAR: i64 = 990_592_102;
/// A carrier hull with the bonuses: 100 m3 bay, three tubes (and one more by
/// a hull effect), two light, two support and no heavy slots.
pub(in crate::fighters) const SK_CARRIER: i64 = 990_592_001;
/// Needs Light Fighters I and Fighters II; an attack ability in slot 0.
pub(in crate::fighters) const SKILLED: i64 = 990_592_010;
/// Evasive manoeuvres in slot 0, resonances of one half.
pub(in crate::fighters) const EVADER: i64 = 990_592_011;
/// A bomb in slot 2.
pub(in crate::fighters) const BOMBER: i64 = 990_592_012;
/// A kamikaze in slot 2.
pub(in crate::fighters) const KAMIKAZE: i64 = 990_592_013;
/// The bomb charge the bomber throws.
pub(in crate::fighters) const BOMB: i64 = 990_592_020;
/// A hull that carries the kamikaze's resistance attribute.
pub(in crate::fighters) const CAPITAL: i64 = 990_592_030;
/// The attribute the kamikaze's resistance id names.
pub(in crate::fighters) const RESISTANCE_ATTR: i64 = 990_592_433;
/// A hull attribute no real hull has: an extra tube.
const EXTRA_TUBES_ATTR: i64 = 990_592_380;

const AB_SKILLED: i64 = 990_720;
const AB_EVASION: i64 = 990_721;
const AB_BOMB: i64 = 990_722;
const AB_KAMIKAZE: i64 = 990_723;

/// The skills' bonuses, in percent: Fighters on damage per level, Light
/// Fighters on speed per level, Fighter Hangar Management on the bay per
/// level; the hull's on damage and on shield capacity.
pub(in crate::fighters) const FIGHTERS_PCT: f64 = 5.0;
pub(in crate::fighters) const LIGHT_PCT: f64 = 10.0;
pub(in crate::fighters) const HANGAR_PCT: f64 = 5.0;
pub(in crate::fighters) const HULL_DAMAGE_PCT: f64 = 10.0;
pub(in crate::fighters) const HULL_SHIELD_PCT: f64 = 20.0;
/// What the evader's resonances are.
pub(in crate::fighters) const EVASION_RESONANCE: f64 = 0.5;
/// The bomber's, in the fixture's units.
pub(in crate::fighters) const BOMB_MPS: f64 = 1_000.0;
pub(in crate::fighters) const BOMB_FUSE_MS: f64 = 10_000.0;
pub(in crate::fighters) const BOMB_RADIUS: f64 = 3_000.0;
pub(in crate::fighters) const BOMB_EM: f64 = 5.0;
pub(in crate::fighters) const BOMB_CHARGES: i64 = 3;
pub(in crate::fighters) const BOMB_REARM_S: f64 = 20.0;
/// The kamikaze's, per damage type and per fighter.
pub(in crate::fighters) const KAMIKAZE_EACH: f64 = 2.0;
pub(in crate::fighters) const CLOUD_M: f64 = 40.0;
pub(in crate::fighters) const SQUADRON: i64 = 4;

pub(in crate::fighters) fn rows() -> String {
    let km = effect::KAMIKAZE;
    format!(
        "\
attribute\t280\t0.0\tskillLevel
attribute\t292\t0.0\tdamageMultiplierBonus
attribute\t2603\t0.0\tfighterSpeedBonus
attribute\t2340\t0.0\tskillBonusFighterHangarSize
attribute\t2226\t0.0\tfighterAbilityAttackMissileDamageMultiplier
effect\t990592200\t0\t0\t0\t0\t0\t0\t0\tfixtureLevelTimesBonus\t
effect\t990592201\t0\t0\t0\t0\t0\t0\t0\tfixtureSkillBonusFighters\t
effect\t990592202\t0\t0\t0\t0\t0\t0\t0\tfixtureSkillBonusLight\t
effect\t990592203\t0\t0\t0\t0\t0\t0\t0\tfixtureLevelTimesSpeed\t
effect\t990592204\t0\t0\t0\t0\t0\t0\t0\tfixtureLevelTimesHangar\t
effect\t990592205\t0\t0\t0\t0\t0\t0\t0\tfixtureSkillBonusHangar\t
effect\t990592206\t0\t0\t0\t0\t0\t0\t0\tfixtureHullDamage\t
effect\t990592207\t0\t0\t0\t0\t0\t0\t0\tfixtureHullShield\t
effect\t990592208\t0\t0\t0\t0\t0\t0\t0\tfixtureHullTube\t
effect\t{km}\t2\t1\t2401\t0\t2330\t0\t0\tfixtureKamikaze\teffects.FixtureKamikaze
modifier\t990592200\titemID\tItemModifier\t292\t280\t0\t\t
modifier\t990592201\tcharID\tOwnerRequiredSkillModifier\t2226\t292\t6\t{SK_FIGHTERS}\t
modifier\t990592203\titemID\tItemModifier\t2603\t280\t0\t\t
modifier\t990592202\tcharID\tOwnerRequiredSkillModifier\t37\t2603\t6\t{SK_LIGHT}\t
modifier\t990592204\titemID\tItemModifier\t2340\t280\t0\t\t
modifier\t990592205\tshipID\tItemModifier\t2055\t2340\t6\t\t
modifier\t990592206\tcharID\tOwnerRequiredSkillModifier\t2226\t2367\t6\t{SK_FIGHTERS}\t
modifier\t990592207\tcharID\tOwnerRequiredSkillModifier\t263\t2368\t6\t{SK_FIGHTERS}\t
modifier\t990592208\titemID\tItemModifier\t2216\t{EXTRA_TUBES_ATTR}\t2\t\t
type\t{SK_FIGHTERS}\t273\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t990592200,990592201\t292={FIGHTERS_PCT}\tFixture Fighters
type\t{SK_LIGHT}\t273\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t990592203,990592202\t2603={LIGHT_PCT}\tFixture Light Fighters
type\t{SK_HANGAR}\t273\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t990592204,990592205\t2340={HANGAR_PCT}\tFixture Fighter Hangar Management
type\t{SK_CARRIER}\t99590\t6\t1\t1\t50000.0\t400.0\t10000000.0\t200.0\t0\t990592206,990592207,990592208\t37=200.0;70=1.0;76=20000.0;2216=3.0;2217=2.0;2218=2.0;2055=100.0;2367={HULL_DAMAGE_PCT};2368={HULL_SHIELD_PCT};{EXTRA_TUBES_ATTR}=1.0\tFixture Skilled Carrier
type\t{SKILLED}\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;265=50.0;552=30.0;182={SK_LIGHT}.0;183={SK_FIGHTERS}.0;277=1.0;278=2.0;2212=1.0;2215={SQUADRON}.0;2223=2000.0;2426=4000.0;2226=1.5;2227=1.0;2233=2000.0;2236=10000.0;2237=5000.0\tFixture Skilled Fighter
type\t{EVADER}\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;265=50.0;552=30.0;2212=1.0;2215={SQUADRON}.0;2123=1000.0;2224=-50.0;2225=-30.0;2118={EVASION_RESONANCE};2119={EVASION_RESONANCE};2120={EVASION_RESONANCE};2121={EVASION_RESONANCE}\tFixture Evader
type\t{BOMBER}\t99593\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;265=50.0;552=30.0;2212=1.0;2215={SQUADRON}.0;2324={BOMB}.0;2349=1000.0\tFixture Bomber
type\t{KAMIKAZE}\t99593\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;265=50.0;552=30.0;2212=1.0;2215={SQUADRON}.0;2325={KAMIKAZE_EACH};2326={KAMIKAZE_EACH};2327={KAMIKAZE_EACH};2328={KAMIKAZE_EACH};2329={CLOUD_M};2330=500.0;2401=10000.0;2432={RESISTANCE_ATTR}.0\tFixture Kamikaze
type\t{BOMB}\t90\t8\t1\t1\t1.0\t0.0\t1000.0\t300.0\t0\t\t37={BOMB_MPS};281={BOMB_FUSE_MS};107={BOMB_RADIUS};114={BOMB_EM};654=10.0;653=100.0;1353=1.0\tFixture Bomb
type\t{CAPITAL}\t99594\t6\t1\t1\t20000.0\t100.0\t1000000.0\t40.0\t0\t\t9=1000.0;37=1000.0;55=1000000.0;263=1000.0;265=1000.0;552=100.0;{RESISTANCE_ATTR}=1.0\tFixture Capital Hull
"
    )
}

pub(in crate::fighters) fn abilities() -> Vec<String> {
    vec![
        format!(
            "{AB_SKILLED}\titemTargeted\t0\t0\t{}\t990650\t\tSkilled attack",
            effect::ATTACK
        ),
        format!(
            "{AB_EVASION}\tuntargeted\t0\t0\t{}\t990651\t\tEvasion",
            effect::EVASION
        ),
        format!(
            "{AB_BOMB}\tpointTargeted\t0\t0\t{}\t990652\t\tBomb",
            effect::BOMB
        ),
        format!(
            "{AB_KAMIKAZE}\titemTargeted\t0\t0\t{}\t990653\t\tKamikaze",
            effect::KAMIKAZE
        ),
    ]
}

pub(in crate::fighters) fn slots() -> Vec<String> {
    vec![
        format!("{SKILLED}\t0\t{AB_SKILLED}\t\t\t"),
        format!("{EVADER}\t0\t{AB_EVASION}\t20\t\t"),
        format!("{BOMBER}\t2\t{AB_BOMB}\t\t{BOMB_CHARGES}\t{BOMB_REARM_S}"),
        format!("{KAMIKAZE}\t2\t{AB_KAMIKAZE}\t\t\t"),
    ]
}

/// Train `character`'s skills to these levels (the invented skills are in no
/// skill table: a literal type, as `drones` does).
pub(in crate::fighters) fn train(
    reg: &mut crate::service::Registry,
    character: i64,
    trained: &[(i64, i64)],
) {
    let skills = reg.skills.entry(character).or_default();
    for &(skill, level) in trained {
        let t = crate::skill::SkillType {
            type_id: skill,
            name: "Fixture Skill",
            group_id: 273,
            rank: 1,
            primary: 166,
            secondary: 167,
        };
        skills.grant_type(&t, level);
    }
}
