//! **The skill gate** (SKILLGATE, issue #36; `handback/r-p1-specs` SPEC 1).
//!
//! A pilot boards, fits and onlines only what their trained skills allow.
//! The requirement is the type's six `(skill, level)` pairs
//! ([`crate::drone_skills::required_skills`]), read against the trained
//! level at the moment of the call, so `/giveskill` and a finished training
//! open the gate at once. No bundle (no `typedata` table) means no
//! requirements, so test worlds without one are unaffected.
//!
//! The keys are the client's own: `ShipHasSkillPrerequisites` (258907),
//! `FittingHasSkillPrerequisites` (259603) and
//! `OnlineHasSkillPrerequisites`, all with `{item, skillCount,
//! requiredSkills}`. The client checks a board and an online itself before
//! it sends anything, so a refusal here is a forged call or a bot [V]; the
//! key on a server-side `ship.Board` is [A] (no client raise site).
//!
//! There is no GM bypass: `/giveskill` and `/devskills` are the GM way.

use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// Every `(skill, level)` of `type_id`'s requirements that `character` has
/// not reached.
pub(crate) fn missing(w: &Registry, character: i64, type_id: i64) -> Vec<(i64, i64)> {
    let table = crate::typedata::installed_in(w.content()).table();
    crate::drone_skills::required_skills(table, type_id)
        .into_iter()
        .filter(|(skill, level)| crate::drone_skills::level_of(w, character, *skill) < *level)
        .collect()
}

/// The client's label for the lacking skills, `60519`
/// (`{skill.name} : Level {amount}`), joined by ", " [A: the English list].
pub(crate) fn text(w: &Registry, missing: &[(i64, i64)]) -> String {
    missing
        .iter()
        .map(|(skill, level)| {
            let name = crate::skill::find_in(w.content(), *skill)
                .map_or_else(|| skill.to_string(), |s| s.name.to_owned());
            format!("{name} : Level {level}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Raise `key` when `character` lacks a skill `type_id` needs.
pub(crate) fn check(
    w: &Registry,
    character: i64,
    type_id: i64,
    key: &str,
) -> Result<(), ServiceError> {
    let lacking = missing(w, character, type_id);
    if lacking.is_empty() {
        return Ok(());
    }
    Err(crate::modules::user_error(
        key,
        vec![
            ("item", PyValue::Int(type_id)),
            ("skillCount", PyValue::Int(lacking.len() as i64)),
            ("requiredSkills", PyValue::Unicode(text(w, &lacking))),
        ],
    ))
}

/// Whether `e` is one of this gate's refusals (a `*HasSkillPrerequisites`).
pub(crate) fn is_refusal(e: &ServiceError) -> bool {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        return false;
    };
    let PyValue::Tuple(h) = header.as_ref() else {
        return false;
    };
    let Some(PyValue::Tuple(args)) = h.get(1) else {
        return false;
    };
    matches!(args.first(), Some(PyValue::Bytes(k)) if k.ends_with(b"HasSkillPrerequisites"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::item::Item;
    use crate::model::{Character, SandboxConfig};
    use crate::service::Request;

    const ALICE: i64 = 9_501_001;
    const STATION: i64 = 9_501_002;
    /// Needs Gunnery 3; the starter sheet has 2.
    const LOCKED_HULL: i64 = 9_501_011;
    /// Needs nothing, as a capsule.
    const FREE_HULL: i64 = 9_501_012;
    /// Needs Spaceship Command 1, on every starter sheet.
    const CORVETTE: i64 = 9_501_013;
    /// Three pairs the sheet meets and a fourth (Small Hybrid Turret 3) it
    /// does not.
    const FOURTH_HULL: i64 = 9_501_014;
    /// Needs Small Hybrid Turret 3.
    const LOCKED_MOD: i64 = 9_501_021;
    const FREE_MOD: i64 = 9_501_022;
    /// Needs Small Hybrid Turret 3 and a CPU no hull has.
    const HUNGRY_MOD: i64 = 9_501_023;
    const GUNNERY: i64 = 3300;
    const SMALL_HYBRID: i64 = 3301;
    const HI0: i64 = 27;
    const HI1: i64 = 28;

    fn content() -> Content {
        let hull = |id: i64, extra: &str| {
            format!(
                "type\t{id}\t25\t6\t1\t1\t1000.0\t500.0\t1200000000.0\t300.0\t0\t\t\
                 9=1000000.0;11=45000.0;14=3;13=2;12=3;37=100.0;48=125000.0;552=30.0;\
                 600=2.0;1281=3.0{extra}\tFixture Hull {id}\n"
            )
        };
        let module = |id: i64, extra: &str| {
            format!(
                "type\t{id}\t55\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t12,16\t50=1.0{extra}\tFixture Module {id}\n"
            )
        };
        let rows = format!(
            "{}{}{}{}{}{}{}",
            hull(LOCKED_HULL, ";182=3300.0;277=3.0"),
            hull(FREE_HULL, ""),
            hull(CORVETTE, ";182=3327.0;277=1.0"),
            hull(
                FOURTH_HULL,
                ";182=3300.0;277=2.0;183=3327.0;278=1.0;184=3300.0;279=1.0;1285=3301.0;1286=3.0"
            ),
            module(LOCKED_MOD, ";182=3301.0;277=3.0"),
            module(FREE_MOD, ""),
            module(HUNGRY_MOD, ";182=3301.0;277=3.0;50=900000000.0"),
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    /// Alice, docked, in a free hull (item 1), with a parked hull of each
    /// kind (items 2-5, hangar) and one module of each kind in the hangar
    /// (items 6-8). Returns the ids in that order.
    fn world() -> (Registry, Vec<i64>) {
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            station_id: Some(STATION),
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![alice],
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content()));
        let mut items = reg.store().items().to_vec();
        let mut ids = Vec::new();
        for (n, type_id) in [
            FREE_HULL,
            LOCKED_HULL,
            CORVETTE,
            FOURTH_HULL,
            FREE_HULL,
            LOCKED_MOD,
            FREE_MOD,
            HUNGRY_MOD,
        ]
        .into_iter()
        .enumerate()
        {
            let id = 9_502_000 + n as i64;
            let t = crate::item::find_in(reg.content(), type_id).expect("a fixture type");
            items.push(Item {
                item_id: id,
                type_id,
                owner_id: ALICE,
                location_id: STATION,
                flag_id: crate::model::FLAG_HANGAR,
                quantity: 1,
                group_id: t.group_id,
                category_id: t.category_id,
                singleton: true,
            });
            ids.push(id);
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 9_503_000,
            ..crate::store::Snapshot::default()
        });
        let c = reg.character_mut(ALICE).expect("Alice");
        c.ship_id = ids[0];
        c.ship_type_id = FREE_HULL;
        (reg, ids)
    }

    fn int(n: i64) -> PyValue {
        PyValue::Int(n)
    }

    fn board(reg: &mut Registry, ship: i64) -> Result<PyValue, ServiceError> {
        let old = reg.character(ALICE).map_or(0, |c| c.ship_id);
        let bind = PyValue::Tuple(vec![int(STATION), int(15)]);
        reg.dispatch(&Request {
            service: "ship",
            method: "Board",
            args: &[int(ship), int(old)],
            kwargs: &[],
            bind_params: Some(&bind),
            actor: Some(ALICE),
        })
    }

    /// `invbroker.Add(item, station, qty=1, flag=flag)` on `ship`'s inventory.
    fn fit(reg: &mut Registry, ship: i64, item: i64, flag: i64) -> Result<PyValue, ServiceError> {
        let bind = PyValue::Tuple(vec![int(ship), int(STATION)]);
        let kwargs = [
            (int(1), PyValue::Bytes(b"qty".to_vec())),
            (int(flag), PyValue::Bytes(b"flag".to_vec())),
        ];
        reg.dispatch(&Request {
            service: "invbroker::inventory",
            method: "Add",
            args: &[int(item), int(STATION)],
            kwargs: &kwargs,
            bind_params: Some(&bind),
            actor: Some(ALICE),
        })
    }

    /// The key of a raised `UserError`, and its arguments by name.
    fn refusal(r: Result<PyValue, ServiceError>) -> (String, Vec<(String, PyValue)>) {
        let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = r else {
            panic!("expected a raised UserError, got {r:?}")
        };
        let PyValue::Tuple(h) = *header else {
            panic!("a call header")
        };
        let PyValue::Tuple(args) = &h[1] else {
            panic!("UserError args")
        };
        let PyValue::Bytes(key) = &args[0] else {
            panic!("the key")
        };
        let PyValue::Dict(d) = &args[1] else {
            panic!("the dict")
        };
        let named = d
            .iter()
            .map(|(v, k)| match k {
                PyValue::Bytes(k) => (String::from_utf8_lossy(k).into_owned(), v.clone()),
                other => panic!("a key, got {other:?}"),
            })
            .collect();
        (String::from_utf8_lossy(key).into_owned(), named)
    }

    fn arg(args: &[(String, PyValue)], name: &str) -> PyValue {
        args.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("no {name}"))
    }

    fn give(reg: &mut Registry, skill: i64, level: i64) {
        let content = content();
        reg.skills_mut(ALICE)
            .expect("skills")
            .grant_in(&content, skill, level);
    }

    fn take(reg: &mut Registry, skill: i64) {
        reg.skills_mut(ALICE).expect("skills").remove(skill);
    }

    #[test]
    fn a_pilot_without_the_hull_skill_cannot_board() {
        let (mut reg, ids) = world();
        let (_key, args) = refusal(board(&mut reg, ids[1]));
        // The key itself is [A] on `ship.Board` (no client raise site): left
        // to `fci_skillgate`, not asserted here.
        assert_eq!(arg(&args, "item"), int(LOCKED_HULL));
        assert_eq!(arg(&args, "skillCount"), int(1));
        assert_eq!(
            arg(&args, "requiredSkills"),
            PyValue::Unicode("Gunnery : Level 3".to_owned())
        );
        assert_eq!(reg.character(ALICE).map(|c| c.ship_id), Some(ids[0]));
    }

    #[test]
    fn the_fourth_required_skill_counts() {
        let (mut reg, ids) = world();
        let (_key, args) = refusal(board(&mut reg, ids[3]));
        assert_eq!(arg(&args, "skillCount"), int(1));
        assert_eq!(
            arg(&args, "requiredSkills"),
            PyValue::Unicode("Small Hybrid Turret : Level 3".to_owned())
        );
        give(&mut reg, SMALL_HYBRID, 3);
        board(&mut reg, ids[3]).expect("all four pairs met");
    }

    #[test]
    fn a_capsule_and_the_corvette_board_with_no_skills() {
        let (mut reg, ids) = world();
        board(&mut reg, ids[4]).expect("a hull with no requirement");
        board(&mut reg, ids[2]).expect("the corvette's skill is on every sheet");
        assert_eq!(reg.character(ALICE).map(|c| c.ship_id), Some(ids[2]));
    }

    #[test]
    fn an_unskilled_module_is_refused_at_fit_with_the_clients_key() {
        let (mut reg, ids) = world();
        let (key, args) = refusal(fit(&mut reg, ids[0], ids[5], HI0));
        assert_eq!(key, "FittingHasSkillPrerequisites");
        assert_eq!(arg(&args, "item"), int(LOCKED_MOD));
        assert_eq!(arg(&args, "skillCount"), int(1));
        assert_eq!(
            reg.store().get(ids[5]).map(|i| i.flag_id),
            Some(crate::model::FLAG_HANGAR),
            "still in the hangar"
        );
        fit(&mut reg, ids[0], ids[6], HI0).expect("a module with no requirement");
    }

    /// A selection with one skilled and one unskilled module fits the first
    /// and still raises the refusal: the client discards a plain reply, so
    /// the raise is the pilot's only news.
    #[test]
    fn multi_add_raises_a_skill_refusal_even_when_another_item_fitted() {
        let (mut reg, ids) = world();
        let bind = PyValue::Tuple(vec![int(ids[0]), int(STATION)]);
        let kwargs = [(int(HI0), PyValue::Bytes(b"flag".to_vec()))];
        let r = reg.dispatch(&Request {
            service: "invbroker::inventory",
            method: "MultiAdd",
            args: &[PyValue::List(vec![int(ids[6]), int(ids[5])]), int(STATION)],
            kwargs: &kwargs,
            bind_params: Some(&bind),
            actor: Some(ALICE),
        });
        assert!(r.as_ref().is_err_and(is_refusal), "{r:?}");
        assert_eq!(
            reg.store().get(ids[6]).map(|i| i.location_id),
            Some(ids[0]),
            "the skilled module fitted"
        );
        assert_eq!(
            reg.store().get(ids[5]).map(|i| i.location_id),
            Some(STATION),
            "the unskilled one stayed"
        );
    }

    #[test]
    fn a_slot_to_slot_move_is_not_gated() {
        let (mut reg, ids) = world();
        give(&mut reg, SMALL_HYBRID, 3);
        fit(&mut reg, ids[0], ids[5], HI0).expect("fitted while skilled");
        take(&mut reg, SMALL_HYBRID);
        fit(&mut reg, ids[0], ids[5], HI1).expect("a move, not a fit");
        assert_eq!(reg.store().get(ids[5]).map(|i| i.flag_id), Some(HI1));
    }

    #[test]
    fn set_module_online_checks_skills_before_cpu() {
        let (mut reg, ids) = world();
        give(&mut reg, SMALL_HYBRID, 3);
        fit(&mut reg, ids[0], ids[7], HI0).expect("fitted while skilled");
        take(&mut reg, SMALL_HYBRID);
        reg.modules_mut().set_online(ids[7], false);
        let r = crate::modules::set_module_online(&mut reg, Some(ALICE), ids[7], true, 1);
        let (key, _) = refusal(r);
        assert_eq!(key, "OnlineHasSkillPrerequisites");
        assert!(!reg.modules().is_online(ids[7]));
    }

    #[test]
    fn on_fitted_leaves_an_unskilled_module_offline() {
        let (mut reg, ids) = world();
        for (module, flag) in [(ids[5], HI0), (ids[6], HI1)] {
            reg.apply_items(&[crate::item::Edit::Move {
                item_id: module,
                to: crate::item::Slot::new(ALICE, ids[0], flag),
            }])
            .expect("moved");
        }
        crate::modules::on_fitted(&mut reg, ids[0], FREE_HULL, ids[5]);
        crate::modules::on_fitted(&mut reg, ids[0], FREE_HULL, ids[6]);
        assert!(!reg.modules().is_online(ids[5]), "unskilled stays offline");
        assert!(reg.modules().is_online(ids[6]), "skilled goes online");
    }

    #[test]
    fn boarding_offlines_what_the_pilot_cannot_use() {
        let (mut reg, ids) = world();
        // A hull in the hangar with an unskilled module already fitted and
        // online, as a world saved before the gate has it.
        reg.apply_items(&[crate::item::Edit::Move {
            item_id: ids[5],
            to: crate::item::Slot::new(ALICE, ids[4], HI0),
        }])
        .expect("moved");
        reg.modules_mut().set_online(ids[5], true);
        board(&mut reg, ids[4]).expect("boards");
        assert!(!reg.modules().is_online(ids[5]));
        // The login hook does the same for the active ship.
        reg.modules_mut().set_online(ids[5], true);
        crate::session_hooks::fire(crate::session_hooks::ON_LOGIN, &mut reg, ALICE);
        assert!(!reg.modules().is_online(ids[5]));
    }

    #[test]
    fn giveskill_opens_all_three_gates() {
        let (mut reg, ids) = world();
        assert!(board(&mut reg, ids[1]).is_err());
        assert!(fit(&mut reg, ids[0], ids[5], HI0).is_err());
        reg.slash(Some(ALICE), "/giveskill 3300 3")
            .expect("granted");
        reg.slash(Some(ALICE), "/giveskill 3301 3")
            .expect("granted");
        assert_eq!(crate::drone_skills::level_of(&reg, ALICE, GUNNERY), 3);
        fit(&mut reg, ids[0], ids[5], HI0).expect("the fit gate is open");
        assert!(
            reg.modules().is_online(ids[5]),
            "and it went online, the online gate is open too"
        );
        board(&mut reg, ids[1]).expect("the board gate is open");
    }

    #[test]
    fn no_table_no_gate() {
        let mut reg = Registry::new(SandboxConfig::default());
        assert!(missing(&reg, 9_501_099, 9_501_098).is_empty());
        assert!(check(&reg, 9_501_099, 9_501_098, "ShipHasSkillPrerequisites").is_ok());
        let _ = reg.take_pending();
    }
}
