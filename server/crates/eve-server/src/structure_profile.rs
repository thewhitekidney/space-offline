//! Structure profiles, their settings and the access rule (lane U2, issue
//! #62; spec `handback/r-upwell` §3, §4, §6, §7). The wire is
//! `service/structures.rs`; the structure itself is [`crate::structure`].
//!
//! **A profile** is a corporation's named set of settings. Each structure
//! points at one (`Structure::profile`). A setting is `(setting, group) ->
//! value`: the group is an **access group** ([`crate::owner_groups`]), and a
//! pilot has the setting when they are an effective member of one of its
//! groups. The client builds the window from `structures.settings` and
//! `structureProfileController`, which are CCP's logic on both sides **[V]**,
//! so the setting ids, their value ranges and their denial errors below are
//! the client's own.
//!
//! **Two delta kinds** (`delta_kinds::KINDS`, a format change):
//!
//! ```text
//! strprofile p <corp> <id>        -> name desc default
//! strprofile next                 -> n          (an id is never reused)
//! strprofile rf <corp>            -> weekday hour (the corp's reinforce default)
//! strsetting <profile> <setting> <group> -> value
//! ```
//!
//! **The access rule** ([`has_access`]): the owner corporation always has
//! docking and structure control; everyone else needs a group of the
//! setting in the structure's profile. A structure whose profile is gone
//! falls back to its owner's default profile; a corporation without one has
//! only itself.
//!
//! **The default profile** ([`ensure_default`]) is made the first time a
//! corporation needs one, with a corp-owned access group of the corporation
//! and its alliance on the docking setting **[A1]** (spec §9: the default
//! allows corp and alliance docking). Falsified by the first owner round
//! wanting it narrower. The group is a snapshot: a corp that joins an
//! alliance later does not widen it, and an owner can edit the profile.
//!
//! **[A]** limits that the client does not state: [`MAX_PROFILES`] a corp
//! and [`MAX_NAME_CHARS`] characters. Falsified by nothing a client can show
//! (it sends whatever the player typed).

use crate::delta::Deltas;
use crate::owner_groups as og;
use crate::service::Registry;
use crate::structure::{self, Structure};
use std::collections::BTreeMap;

/// The delta kind of profiles (and the reinforce default).
pub const KIND_PROFILE: &str = "strprofile";
/// The delta kind of settings.
pub const KIND_SETTING: &str = "strsetting";

/// `structures.SETTING_*` **[V]**.
pub const SETTING_REPROCESSING_TAX: i64 = 3;
pub const SETTING_MARKET_TAX: i64 = 4;
pub const SETTING_DEFENSE_CAN_CONTROL_STRUCTURE: i64 = 17;
pub const SETTING_HOUSING_CAN_DOCK: i64 = 19;
pub const SETTING_CORP_RENT_OFFICE: i64 = 20;
pub const SETTING_CLONINGBAY_TAX: i64 = 23;

/// `structures.SERVICE_*` **[V]**.
pub const SERVICE_DOCKING: i64 = 1;
pub const SERVICE_FITTING: i64 = 2;
pub const SERVICE_OFFICES: i64 = 3;
pub const SERVICE_REPROCESSING: i64 = 4;
pub const SERVICE_MARKET: i64 = 5;
pub const SERVICE_MEDICAL: i64 = 6;
pub const SERVICE_REPAIR: i64 = 8;
pub const SERVICE_INSURANCE: i64 = 9;
pub const SERVICE_JUMP_CLONE: i64 = 10;

/// `structures.SERVICE_STATE_*` **[V]**.
pub const SERVICE_STATE_ONLINE: i64 = 1;
pub const SERVICE_STATE_OFFLINE: i64 = 2;

/// The group id of a setting row that has no group (`NO_GROUP`), which the
/// client sends for a boolean setting **[V]**. No setting in this build is
/// boolean, so it only has to survive a round trip.
pub const NO_GROUP: i64 = 0;

/// `categoryStructure`, and the groups the three home hulls sit in **[V]**
/// (the spec's locator filter, §4): an Astrahus is a citadel, a Raitaru an
/// engineering complex, an Athanor a refinery.
pub const CATEGORY_STRUCTURE: i64 = 65;
const GROUP_CITADEL: i64 = 1657;
const GROUP_ENGINEERING: i64 = 1404;
const GROUP_REFINERY: i64 = 1406;

/// The inventory group of a structure hull **[A]** for any type outside the
/// three home hulls and their size siblings (they all take the citadel
/// group). Falsified by a Show Info window naming the wrong group.
pub fn group_of(type_id: i64) -> i64 {
    match type_id {
        35825..=35827 => GROUP_ENGINEERING,
        35835 | 35836 => GROUP_REFINERY,
        _ => GROUP_CITADEL,
    }
}

pub const MAX_PROFILES: usize = 50;
pub const MAX_NAME_CHARS: usize = 100;
pub const MAX_DESC_CHARS: usize = 3900;

/// One setting the client knows (`StructureSettingObject`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettingSpec {
    pub id: i64,
    /// `valueRange`; `None` is a setting with no value.
    pub range: Option<(f64, f64)>,
    /// `cantAccessError`, the UserError a refused pilot gets.
    pub denied: &'static str,
}

const fn spec(id: i64, range: Option<(f64, f64)>, denied: &'static str) -> SettingSpec {
    SettingSpec { id, range, denied }
}

/// `structures.ALL_SETTING_OBJECTS` **[V]**: every setting has groups.
pub const SETTINGS: &[SettingSpec] = &[
    spec(4, Some((0.0, 11.5)), "StructureMarketDenied"),
    spec(17, None, "StructureDefenseDenied"),
    spec(19, None, "StructureDockingDenied"),
    spec(
        20,
        Some((0.0, 1_000_000_000.0)),
        "StructureCorpOfficesDenied",
    ),
    spec(23, Some((0.0, 100_000_000.0)), "StructureCloneBayDenied"),
    spec(3, Some((0.0, 50.0)), "StructureReprocessingDenied"),
    spec(26, Some((0.0, 10.0)), "StructureReactionBiochemicalDenied"),
    spec(28, Some((0.0, 10.0)), "StructureReactionCompositeDenied"),
    spec(27, Some((0.0, 10.0)), "StructureReactionHybridDenied"),
    spec(29, Some((0.0, 10.0)), "StructureManufacturingDenied"),
    spec(30, Some((0.0, 10.0)), "StructureManufacturingCapitalDenied"),
    spec(
        31,
        Some((0.0, 10.0)),
        "StructureManufacturingSuperCapitalDenied",
    ),
    spec(32, Some((0.0, 10.0)), "StructureResearchDenied"),
    spec(33, Some((0.0, 10.0)), "StructureInventionDenied"),
    spec(34, None, "ActivateJumpBridgeDenied"),
    spec(35, None, "ConnectToCynoBeaconDenied"),
    spec(36, None, "AccessToAutoMoonMinerDenied"),
];

/// The spec of setting `id`, if the client has one.
pub fn setting_spec(id: i64) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|s| s.id == id)
}

/// `GetAccessErrorLabel`.
pub fn access_error(setting: i64) -> &'static str {
    setting_spec(setting).map_or("StructureGenericSettingDenied", |s| s.denied)
}

/// The setting that gates a service for a pilot (`SERVICES_ACCESS_SETTINGS`),
/// `None` for the ones every pilot may use once they are inside.
pub fn setting_of_service(service: i64) -> Option<i64> {
    match service {
        SERVICE_DOCKING => Some(SETTING_HOUSING_CAN_DOCK),
        SERVICE_OFFICES => Some(SETTING_CORP_RENT_OFFICE),
        SERVICE_REPROCESSING => Some(SETTING_REPROCESSING_TAX),
        SERVICE_MARKET => Some(SETTING_MARKET_TAX),
        SERVICE_MEDICAL | SERVICE_JUMP_CLONE => Some(SETTING_CLONINGBAY_TAX),
        _ => None,
    }
}

/// Why a profile call was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NoSuchProfile,
    BadName,
    TooMany,
    /// The default profile cannot be deleted.
    IsDefault,
}

/// One profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub id: i64,
    pub corp: i64,
    pub name: String,
    pub desc: String,
    pub default: bool,
}

fn pkey(corp: i64, id: i64) -> Vec<String> {
    vec!["p".to_owned(), corp.to_string(), id.to_string()]
}

fn skey(profile: i64, setting: i64, group: i64) -> Vec<String> {
    vec![profile.to_string(), setting.to_string(), group.to_string()]
}

fn profile_from(k: &[String], v: &[String]) -> Option<Profile> {
    let [t, corp, id] = k else { return None };
    if t != "p" {
        return None;
    }
    Some(Profile {
        id: id.parse().ok()?,
        corp: corp.parse().ok()?,
        name: v.first().cloned().unwrap_or_default(),
        desc: v.get(1).cloned().unwrap_or_default(),
        default: v.get(2).is_some_and(|d| d == "1"),
    })
}

fn put_profile(d: &mut Deltas, p: &Profile) {
    d.put(
        KIND_PROFILE,
        pkey(p.corp, p.id),
        vec![
            p.name.clone(),
            p.desc.clone(),
            u8::from(p.default).to_string(),
        ],
    );
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Every profile, in id order.
pub fn all(d: &Deltas) -> Vec<Profile> {
    let mut out: Vec<Profile> = d
        .of(KIND_PROFILE)
        .iter()
        .filter_map(|(k, v)| profile_from(k, v))
        .collect();
    out.sort_by_key(|p| p.id);
    out
}

/// One corporation's profiles, in id order.
pub fn profiles_of(d: &Deltas, corp: i64) -> Vec<Profile> {
    all(d).into_iter().filter(|p| p.corp == corp).collect()
}

/// The profile with this id, whoever owns it.
pub fn profile(d: &Deltas, id: i64) -> Option<Profile> {
    all(d).into_iter().find(|p| p.id == id)
}

/// A corporation's default profile, if it has one.
pub fn default_of(d: &Deltas, corp: i64) -> Option<Profile> {
    profiles_of(d, corp).into_iter().find(|p| p.default)
}

fn take_id(d: &mut Deltas) -> i64 {
    let key = vec!["next".to_owned()];
    let stored = d
        .of(KIND_PROFILE)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(0);
    let used = all(d).last().map_or(0, |p| p.id);
    let id = stored.max(used) + 1;
    d.put(KIND_PROFILE, key, vec![id.to_string()]);
    id
}

fn new_profile(
    reg: &mut Registry,
    corp: i64,
    name: &str,
    desc: &str,
    default: bool,
) -> Result<Profile, Refusal> {
    let name = clip(name.trim(), MAX_NAME_CHARS);
    if name.is_empty() {
        return Err(Refusal::BadName);
    }
    if profiles_of(reg.deltas(), corp).len() >= MAX_PROFILES {
        return Err(Refusal::TooMany);
    }
    let d = reg.deltas_mut();
    let p = Profile {
        id: take_id(d),
        corp,
        name,
        desc: clip(desc, MAX_DESC_CHARS),
        default,
    };
    put_profile(d, &p);
    Ok(p)
}

/// The corporation's default profile id, making it (with its docking group)
/// if the corporation has none yet.
pub fn ensure_default(reg: &mut Registry, corp: i64) -> i64 {
    if let Some(p) = default_of(reg.deltas(), corp) {
        return p.id;
    }
    // A corporation whose default was deleted by hand-editing the world: the
    // oldest profile it still has becomes the default.
    if let Some(mut p) = profiles_of(reg.deltas(), corp).into_iter().next() {
        p.default = true;
        put_profile(reg.deltas_mut(), &p);
        return p.id;
    }
    let mut members = vec![(corp, og::MEMBER)];
    if let Some(alliance) = crate::alliance::of_corp(reg.deltas(), corp) {
        members.push((alliance, og::MEMBER));
    }
    let group = og::create_for(
        reg,
        corp,
        "Default docking",
        "The owner corporation and its alliance.",
        &members,
    );
    let p = new_profile(reg, corp, "Default", "", true).expect("a corp without profiles has room");
    reg.deltas_mut().put(
        KIND_SETTING,
        skey(p.id, SETTING_HOUSING_CAN_DOCK, group.group_id),
        vec!["0".to_owned()],
    );
    p.id
}

/// `CreateProfile`: an empty profile (nobody but the owner may dock).
pub fn create(reg: &mut Registry, corp: i64, name: &str, desc: &str) -> Result<i64, Refusal> {
    ensure_default(reg, corp);
    new_profile(reg, corp, name, desc, false).map(|p| p.id)
}

/// `DuplicateProfile`: a copy with the same settings.
pub fn duplicate(reg: &mut Registry, corp: i64, id: i64) -> Result<i64, Refusal> {
    let src = profile(reg.deltas(), id)
        .filter(|p| p.corp == corp)
        .ok_or(Refusal::NoSuchProfile)?;
    let copy = new_profile(reg, corp, &format!("{} copy", src.name), &src.desc, false)?;
    let rows = settings(reg.deltas(), id);
    save_settings(reg.deltas_mut(), copy.id, &rows);
    Ok(copy.id)
}

/// `UpdateProfile`: name and description.
pub fn update(
    reg: &mut Registry,
    corp: i64,
    id: i64,
    name: &str,
    desc: &str,
) -> Result<(), Refusal> {
    let mut p = profile(reg.deltas(), id)
        .filter(|p| p.corp == corp)
        .ok_or(Refusal::NoSuchProfile)?;
    let name = clip(name.trim(), MAX_NAME_CHARS);
    if name.is_empty() {
        return Err(Refusal::BadName);
    }
    p.name = name;
    p.desc = clip(desc, MAX_DESC_CHARS);
    put_profile(reg.deltas_mut(), &p);
    Ok(())
}

/// `SetDefaultProfile`: the one new structures get.
pub fn set_default(reg: &mut Registry, corp: i64, id: i64) -> Result<(), Refusal> {
    profile(reg.deltas(), id)
        .filter(|p| p.corp == corp)
        .ok_or(Refusal::NoSuchProfile)?;
    for mut p in profiles_of(reg.deltas(), corp) {
        let want = p.id == id;
        if p.default != want {
            p.default = want;
            put_profile(reg.deltas_mut(), &p);
        }
    }
    Ok(())
}

/// `DeleteProfile`: the structures that used it move to the default. Answers
/// their ids.
pub fn delete(reg: &mut Registry, corp: i64, id: i64) -> Result<Vec<i64>, Refusal> {
    let p = profile(reg.deltas(), id)
        .filter(|p| p.corp == corp)
        .ok_or(Refusal::NoSuchProfile)?;
    if p.default {
        return Err(Refusal::IsDefault);
    }
    let fallback = ensure_default(reg, corp);
    let moved = assign_where(reg.deltas_mut(), corp, |s| s.profile == id, fallback);
    let d = reg.deltas_mut();
    let keys: Vec<Vec<String>> = d
        .of(KIND_SETTING)
        .keys()
        .filter(|k| k.first().is_some_and(|f| *f == id.to_string()))
        .cloned()
        .collect();
    for k in keys {
        d.del(KIND_SETTING, &k);
    }
    d.del(KIND_PROFILE, &pkey(corp, id));
    Ok(moved)
}

fn assign_where(
    d: &mut Deltas,
    corp: i64,
    pick: impl Fn(&Structure) -> bool,
    profile: i64,
) -> Vec<i64> {
    let mut changed = Vec::new();
    for mut s in structure::of_corp(d, corp) {
        if pick(&s) && s.profile != profile {
            s.profile = profile;
            structure::put(d, &s);
            changed.push(s.id);
        }
    }
    changed
}

/// `ChangeProfiles(structureIDs, profileID)`: the corp's own structures move
/// to the profile. Answers the ids that changed.
pub fn assign(
    reg: &mut Registry,
    corp: i64,
    structures: &[i64],
    id: i64,
) -> Result<Vec<i64>, Refusal> {
    profile(reg.deltas(), id)
        .filter(|p| p.corp == corp)
        .ok_or(Refusal::NoSuchProfile)?;
    Ok(assign_where(
        reg.deltas_mut(),
        corp,
        |s| structures.contains(&s.id),
        id,
    ))
}

/// A profile's settings, `(setting, group, value)` in key order.
pub fn settings(d: &Deltas, id: i64) -> Vec<(i64, i64, f64)> {
    let want = id.to_string();
    d.of(KIND_SETTING)
        .iter()
        .filter(|(k, _)| k.first() == Some(&want))
        .filter_map(|(k, v)| {
            let [_, setting, group] = k.as_slice() else {
                return None;
            };
            Some((
                setting.parse().ok()?,
                group.parse().ok()?,
                v.first()?.parse().ok()?,
            ))
        })
        .collect()
}

/// A profile's settings as the window loads them: `{setting: [(group,
/// value)]}`.
pub fn settings_by_setting(d: &Deltas, id: i64) -> BTreeMap<i64, Vec<(i64, f64)>> {
    let mut out: BTreeMap<i64, Vec<(i64, f64)>> = BTreeMap::new();
    for (setting, group, value) in settings(d, id) {
        out.entry(setting).or_default().push((group, value));
    }
    out
}

/// `SaveProfileSettings`: replace every setting of the profile with `rows`
/// (`(setting, group, value)`, the shape `structureSettingsWnd.SaveProfile`
/// sends **[V]**). A setting the client does not know is dropped, a value is
/// held to the setting's range, a repeated `(setting, group)` keeps its last
/// value.
pub fn save_settings(d: &mut Deltas, id: i64, rows: &[(i64, i64, f64)]) {
    let want = id.to_string();
    let old: Vec<Vec<String>> = d
        .of(KIND_SETTING)
        .keys()
        .filter(|k| k.first() == Some(&want))
        .cloned()
        .collect();
    for k in old {
        d.del(KIND_SETTING, &k);
    }
    for &(setting, group, value) in rows {
        let Some(spec) = setting_spec(setting) else {
            continue;
        };
        let value = match spec.range {
            Some((lo, hi)) if value.is_finite() => value.clamp(lo, hi),
            Some((lo, _)) => lo,
            None => 0.0,
        };
        d.put(
            KIND_SETTING,
            skey(id, setting, group),
            vec![value.to_string()],
        );
    }
}

/// The corporation's default reinforcement `(weekday, hour)`
/// (`GetStructureReinforceDefault`).
pub fn reinforce_default(d: &Deltas, corp: i64) -> (i64, i64) {
    d.of(KIND_PROFILE)
        .get(&vec!["rf".to_owned(), corp.to_string()])
        .and_then(|v| Some((v.first()?.parse().ok()?, v.get(1)?.parse().ok()?)))
        .unwrap_or((
            structure::DEFAULT_REINFORCE_WEEKDAY,
            structure::DEFAULT_REINFORCE_HOUR,
        ))
}

/// `SetStructureReinforceDefault(hour)`: only the hour is sent **[V]**, so
/// the weekday stays as it was.
pub fn set_reinforce_default_hour(
    d: &mut Deltas,
    corp: i64,
    hour: i64,
) -> Result<(), &'static str> {
    structure::check_reinforce_hour(hour)?;
    let (weekday, _) = reinforce_default(d, corp);
    d.put(
        KIND_PROFILE,
        vec!["rf".to_owned(), corp.to_string()],
        vec![weekday.to_string(), hour.to_string()],
    );
    Ok(())
}

// -- the access rule ---------------------------------------------------------

/// The profile a structure is judged by: its own, else its owner's default.
fn profile_for(d: &Deltas, s: &Structure) -> Option<i64> {
    profile(d, s.profile)
        .filter(|p| p.corp == s.owner_corp)
        .or_else(|| default_of(d, s.owner_corp))
        .map(|p| p.id)
}

/// Whether `ch` has `setting` on `s`: the owner corporation always has
/// docking and structure control; anyone else needs a group of the setting in
/// the structure's profile, through [`og::effective`] **[A]** (the owner rule
/// is the spec's; real EVE has no owner bypass. Falsified by the first owner
/// round wanting the corp locked out of its own hull).
pub fn has_access(reg: &Registry, s: &Structure, ch: i64, setting: i64) -> bool {
    let mine = reg.character(ch).map(|c| c.corporation_id);
    if mine == Some(s.owner_corp)
        && matches!(
            setting,
            SETTING_HOUSING_CAN_DOCK | SETTING_DEFENSE_CAN_CONTROL_STRUCTURE
        )
    {
        return true;
    }
    let Some(pid) = profile_for(reg.deltas(), s) else {
        return false;
    };
    settings(reg.deltas(), pid)
        .into_iter()
        .filter(|(set, _, _)| *set == setting)
        .any(|(_, group, _)| group != NO_GROUP && og::effective(reg, group, ch) >= og::MEMBER)
}

/// The docking half of [`Structure::can_dock`] and the access rule: what
/// `CheckMyDockingAccessToStructures` answers, and what `Dock` will check.
pub fn may_dock(reg: &Registry, s: &Structure, ch: i64) -> bool {
    s.can_dock() && has_access(reg, s, ch, SETTING_HOUSING_CAN_DOCK)
}

/// The home services (U5): what a pilot lives on in a structure.
///
/// **[A]** every online structure offers them until the upkeep lane (U6)
/// makes them service modules that are fitted and online. U6 replaces this
/// list with the fitted modules; the callers ask `services_for`, so they
/// do not change. Falsifier: a structure with no module that still repairs.
pub const HOME_SERVICES: [i64; 4] = [
    SERVICE_MEDICAL,
    SERVICE_REPAIR,
    SERVICE_INSURANCE,
    SERVICE_JUMP_CLONE,
];

/// The services a structure has, `{service: state}`. Docking and fitting are
/// hull services and are online with the hull; the [`HOME_SERVICES`] are
/// bundled until the upkeep lane (U6) fits them as modules.
pub fn services_of(s: &Structure) -> BTreeMap<i64, i64> {
    let state = if s.is_online() {
        SERVICE_STATE_ONLINE
    } else {
        SERVICE_STATE_OFFLINE
    };
    let mut out = BTreeMap::from([(SERVICE_DOCKING, state), (SERVICE_FITTING, state)]);
    for svc in HOME_SERVICES {
        out.insert(svc, state);
    }
    out
}

/// The services of `s` that `ch` may use now (`CharacterGetServices`): the
/// online ones whose setting, if any, the pilot holds.
pub fn services_for(reg: &Registry, s: &Structure, ch: i64) -> BTreeMap<i64, i64> {
    services_of(s)
        .into_iter()
        .filter(|(_, st)| *st == SERVICE_STATE_ONLINE)
        .filter(|(svc, _)| match *svc {
            // **[A]** the cloning bay is for whoever may dock; its tax
            // setting is an ISK matter we do not charge. Falsifier: a
            // retail structure whose bay a docked guest cannot use.
            SERVICE_MEDICAL | SERVICE_JUMP_CLONE => {
                has_access(reg, s, ch, SETTING_HOUSING_CAN_DOCK)
            }
            _ => setting_of_service(*svc).is_none_or(|set| has_access(reg, s, ch, set)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SandboxConfig;

    const OWNER: i64 = 7;
    const FRIEND: i64 = 8;
    const STRANGER: i64 = 9;
    const CORP_A: i64 = 11;
    const CORP_B: i64 = 12;
    const CORP_C: i64 = 13;

    fn world() -> Registry {
        let ch = |id: i64, corp: i64| crate::model::Character {
            character_id: id,
            corporation_id: corp,
            ship_id: 1_000 + id,
            ..crate::model::Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![ch(OWNER, CORP_A), ch(FRIEND, CORP_B), ch(STRANGER, CORP_C)],
            ..SandboxConfig::default()
        })
    }

    fn hull(reg: &mut Registry, profile: i64) -> Structure {
        let e = structure::Env::new(crate::pyutil::filetime_now());
        let mut s = Structure::anchored(7, 77, CORP_A, 22, [0.0; 3], 0.0, "Home", profile, &e);
        s.set_state(structure::State::ShieldVulnerable, &e);
        structure::put(reg.deltas_mut(), &s);
        s
    }

    #[test]
    fn the_default_profile_is_made_once_and_lets_corp_and_alliance_dock() {
        let mut reg = world();
        let id = ensure_default(&mut reg, CORP_A);
        assert_eq!(ensure_default(&mut reg, CORP_A), id);
        let p = profile(reg.deltas(), id).unwrap();
        assert!(p.default && p.corp == CORP_A);
        let s = hull(&mut reg, id);
        assert!(may_dock(&reg, &s, OWNER));
        assert!(!may_dock(&reg, &s, FRIEND));
        assert!(!may_dock(&reg, &s, STRANGER));
        // The default group is corp-owned and read-only to the client.
        let group = settings(reg.deltas(), id)[0].1;
        assert_eq!(og::group(&reg, group).unwrap().creator_id, CORP_A);
    }

    #[test]
    fn a_profile_group_opens_the_door_and_an_exclusion_closes_it() {
        let mut reg = world();
        let id = ensure_default(&mut reg, CORP_A);
        let s = hull(&mut reg, id);
        let pals = og::create_for(
            &mut reg,
            CORP_A,
            "pals",
            "",
            &[(CORP_B, og::MEMBER), (STRANGER, og::EXCLUDED)],
        )
        .group_id;
        save_settings(
            reg.deltas_mut(),
            id,
            &[(SETTING_HOUSING_CAN_DOCK, pals, 0.0)],
        );
        assert!(may_dock(&reg, &s, FRIEND));
        assert!(!may_dock(&reg, &s, STRANGER));
        // The owner never loses docking, even with an empty profile.
        save_settings(reg.deltas_mut(), id, &[]);
        assert!(may_dock(&reg, &s, OWNER));
        assert!(!may_dock(&reg, &s, FRIEND));
    }

    #[test]
    fn an_offline_hull_cannot_be_docked_whoever_you_are() {
        let mut reg = world();
        let id = ensure_default(&mut reg, CORP_A);
        let e = structure::Env::new(crate::pyutil::filetime_now());
        let s = Structure::anchored(8, 77, CORP_A, 22, [0.0; 3], 0.0, "Fresh", id, &e);
        assert!(!may_dock(&reg, &s, OWNER));
        assert!(has_access(&reg, &s, OWNER, SETTING_HOUSING_CAN_DOCK));
    }

    #[test]
    fn a_lost_profile_falls_back_to_the_owners_default() {
        let mut reg = world();
        let id = ensure_default(&mut reg, CORP_A);
        let s = hull(&mut reg, 9_999);
        assert_eq!(profile_for(reg.deltas(), &s), Some(id));
        // Another corp's profile is not the owner's.
        let other = ensure_default(&mut reg, CORP_B);
        let s = hull(&mut reg, other);
        assert_eq!(profile_for(reg.deltas(), &s), Some(id));
    }

    #[test]
    fn save_holds_values_to_the_range_and_drops_unknown_settings() {
        let mut reg = world();
        let id = ensure_default(&mut reg, CORP_A);
        save_settings(
            reg.deltas_mut(),
            id,
            &[
                (SETTING_MARKET_TAX, 5, 99.0),
                (SETTING_MARKET_TAX, 6, -3.0),
                (SETTING_HOUSING_CAN_DOCK, 5, 7.0),
                (9_999, 5, 1.0),
            ],
        );
        let rows = settings(reg.deltas(), id);
        assert_eq!(rows.len(), 3, "{rows:?}");
        let max = SETTINGS
            .iter()
            .find(|s| s.id == SETTING_MARKET_TAX)
            .and_then(|s| s.range)
            .unwrap()
            .1;
        assert!(rows.contains(&(SETTING_MARKET_TAX, 5, max)));
        assert!(rows.contains(&(SETTING_MARKET_TAX, 6, 0.0)));
        assert!(rows.contains(&(SETTING_HOUSING_CAN_DOCK, 5, 0.0)));
    }

    #[test]
    fn profile_calls_check_the_owner_and_keep_one_default() {
        let mut reg = world();
        let def = ensure_default(&mut reg, CORP_A);
        let a = create(&mut reg, CORP_A, " Allies ", "d").unwrap();
        assert_eq!(profile(reg.deltas(), a).unwrap().name, "Allies");
        assert_eq!(create(&mut reg, CORP_A, "  ", ""), Err(Refusal::BadName));
        // Another corp cannot touch it.
        assert_eq!(
            update(&mut reg, CORP_B, a, "x", ""),
            Err(Refusal::NoSuchProfile)
        );
        assert_eq!(delete(&mut reg, CORP_A, def), Err(Refusal::IsDefault));
        set_default(&mut reg, CORP_A, a).unwrap();
        assert_eq!(default_of(reg.deltas(), CORP_A).unwrap().id, a);
        assert_eq!(
            profiles_of(reg.deltas(), CORP_A)
                .iter()
                .filter(|p| p.default)
                .count(),
            1
        );
        // A structure on the deleted profile moves to the default.
        let s = hull(&mut reg, def);
        assert_eq!(delete(&mut reg, CORP_A, def), Ok(vec![s.id]));
        assert_eq!(structure::get(reg.deltas(), s.id).unwrap().profile, a);
        assert!(settings(reg.deltas(), def).is_empty());
        // An id is never reused.
        let b = create(&mut reg, CORP_A, "B", "").unwrap();
        assert!(b > a);
    }

    #[test]
    fn duplicate_copies_the_settings() {
        let mut reg = world();
        let def = ensure_default(&mut reg, CORP_A);
        let copy = duplicate(&mut reg, CORP_A, def).unwrap();
        assert_ne!(copy, def);
        assert_eq!(settings(reg.deltas(), copy), settings(reg.deltas(), def));
        assert!(!profile(reg.deltas(), copy).unwrap().default);
    }

    #[test]
    fn services_follow_the_hull_and_the_setting() {
        let mut reg = world();
        let id = ensure_default(&mut reg, CORP_A);
        let s = hull(&mut reg, id);
        assert_eq!(
            services_for(&reg, &s, OWNER)
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![
                SERVICE_DOCKING,
                SERVICE_FITTING,
                SERVICE_MEDICAL,
                SERVICE_REPAIR,
                SERVICE_INSURANCE,
                SERVICE_JUMP_CLONE
            ]
        );
        assert_eq!(
            services_for(&reg, &s, STRANGER)
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            // no dock right: no cloning bay; the shops ask no setting
            vec![SERVICE_FITTING, SERVICE_REPAIR, SERVICE_INSURANCE]
        );
    }

    #[test]
    fn reinforce_default_keeps_the_weekday_when_the_hour_is_set() {
        let mut reg = world();
        let d = reg.deltas_mut();
        assert_eq!(
            reinforce_default(d, CORP_A),
            (
                structure::DEFAULT_REINFORCE_WEEKDAY,
                structure::DEFAULT_REINFORCE_HOUR
            )
        );
        set_reinforce_default_hour(d, CORP_A, 3).unwrap();
        assert_eq!(
            reinforce_default(d, CORP_A),
            (structure::DEFAULT_REINFORCE_WEEKDAY, 3)
        );
        assert!(set_reinforce_default_hour(d, CORP_A, 24).is_err());
    }

    #[test]
    fn rows_round_trip_through_the_kinds() {
        assert!(crate::delta_kinds::is_registered(KIND_PROFILE));
        assert!(crate::delta_kinds::is_registered(KIND_SETTING));
    }
}
