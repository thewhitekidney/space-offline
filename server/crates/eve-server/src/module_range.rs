//! **A targeted module reaches only so far** (lane mine-range).
//!
//! `modules::activate` checked the range of a mining laser, a salvager, an
//! analyzer and an Entosis Link, each in its own module, and nothing for the
//! rest: a web, a damper, a remote repairer, a neutralizer could be started
//! on a ship at any distance (the client checks no range before the call,
//! `docs/re/modules.md` 7.1) and cycled for as long as it liked. This is the
//! one check for those.
//!
//! **Turrets and launchers are deliberately not here.** A gun may be started
//! at any range and then misses (`combat::Application::chance`, pinned by
//! `turret_application_socket`), and a missile is launched and flies its
//! flight time (`missile::launch`); EVE does the same, and a refusal there
//! would be the invention. Their range is enforced where it acts: the hit
//! chance, and the missile's fuse.
//!
//! - **The refusal** is `TargetTooFar` (fsd 1632), loc args `module`
//!   (`(UE_TYPEID, typeID)`), `distance`, `targetname` `[V]`
//!   (`findings/user-errors.tsv`); the same key and dict the laser's refusal
//!   uses (`crate::mining::check_activation`).
//! - **The reach** is `maxRange` (54) plus [`crate::ewar::FALLOFF_REACH`]
//!   falloffs (158 for a turret, 2044 for a projected effect): the distance
//!   past which the cycle's effect is already zero (`combat::RANGE_CUT_FALLOFFS`,
//!   `ewar::falloff`), so a module this refuses could do nothing anyway.
//!   **[A]** that EVE refuses rather than idles at that distance; falsified by
//!   a capture of a remote repairer started beyond its falloff on the real
//!   server.
//! - **Surface to surface**, as the client's range indicator measures, and
//!   **both ships brought to now first**: a roster position is true only as
//!   of its ship's last command (`Registry::advance_character`), so a target
//!   that is flying away would otherwise be measured where it last spoke.
//! - **Mid-cycle** ([`sweep`], from the world clock): a running module whose
//!   target has left its reach stops at the end of the cycle with the same
//!   `TargetTooFar` as the stop's reason (`OnGodmaShipEffect` `error`,
//!   `docs/re/modules.md` 7.3), the way a laser's cycle does.
//! - **A ship in an activated warp** may not start a module whose type says
//!   `disallowActivateOnWarp` (1245): `DeniedActivateInWarp` `[V]`.

use crate::modules::{self, Effect, ModuleType};
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// `disallowActivateOnWarp` `[V-data]`.
pub const ATTR_DISALLOW_ON_WARP: i64 = 1245;
/// `falloff` (158): a turret's.
const ATTR_FALLOFF: i64 = 158;
/// `falloffEffectiveness` (2044): a projected effect's.
const ATTR_FALLOFF_EFFECTIVENESS: i64 = 2044;

/// Whether the module is one this file checks: a projected effect on a
/// target that is not a weapon (no hardpoint) and not one of the four with a
/// check of its own.
fn is_ours(module: &ModuleType, effect: &Effect, target_id: Option<i64>) -> bool {
    target_id.is_some()
        && effect.category == modules::CATEGORY_TARGET
        && module.hardpoint.is_none()
        && !crate::mining::is_mining(effect.effect_id)
        && !crate::salvage::is_salvage_effect(effect.effect_id)
        && effect.effect_id != crate::hacking::EFFECT_DO_HACKING
        && effect.effect_id != crate::entosis::EFFECT_ENTOSIS_LINK
}

/// The warp rule: no module that forbids it starts in an activated warp.
pub fn check_warp(
    w: &mut Registry,
    character_id: i64,
    module: &ModuleType,
) -> Result<(), ServiceError> {
    if module
        .attribute(ATTR_DISALLOW_ON_WARP)
        .is_none_or(|v| v <= 0.0)
    {
        return Ok(());
    }
    w.advance_character(character_id);
    let warping = w
        .roster()
        .get(character_id)
        .is_some_and(|p| p.motion.warp_active());
    if warping {
        return Err(modules::user_error("DeniedActivateInWarp", vec![]));
    }
    Ok(())
}

/// How far the module reaches, metres from surface, for this ship; `None`
/// when it has no range to enforce (no `maxRange`).
pub fn reach_of(
    w: &Registry,
    ship_id: i64,
    module_id: i64,
    module: &ModuleType,
    effect: &Effect,
) -> Option<f64> {
    let get = |attr: i64| {
        crate::dogma::module_attribute(w, ship_id, module_id, attr)
            .or_else(|| module.attribute(attr))
    };
    // A scanner names its own attributes (125/510, 126) [V]; without this
    // it would have no range at all.
    if let Some((range_attr, fall_attr)) = crate::ship_scan::reach_attrs(effect.effect_id) {
        let range = get(range_attr).filter(|r| *r > 0.0)?;
        let fall = fall_attr.and_then(get).filter(|f| *f > 0.0).unwrap_or(0.0);
        return Some(range + crate::ewar::FALLOFF_REACH * fall);
    }
    // A focused HIC script reaches `warpScrambleRange` (HIC-SCRIPT).
    let range_attr = if crate::tackle::is_focused(effect) {
        crate::tackle::ATTR_FIELD_RANGE
    } else {
        54
    };
    let range = get(range_attr).filter(|r| *r > 0.0)?;
    let fall = get(ATTR_FALLOFF)
        .filter(|f| *f > 0.0)
        .or_else(|| get(ATTR_FALLOFF_EFFECTIVENESS).filter(|f| *f > 0.0))
        .unwrap_or(0.0);
    Some(range + crate::ewar::FALLOFF_REACH * fall)
}

/// Surface to surface between two ships, both carried to now. `None` when
/// either is not in the roster.
pub fn gap_between(w: &mut Registry, ship_id: i64, target_ship: i64) -> Option<f64> {
    for ship in [ship_id, target_ship] {
        let id = w.roster().by_ship(ship)?.character_id;
        w.advance_character(id);
    }
    let a = *w.roster().by_ship(ship_id)?;
    let b = *w.roster().by_ship(target_ship)?;
    let radius = |w: &Registry, character_id: i64| {
        let t = w.pilot(character_id).map_or(0, |p| p.ship_type_id);
        f64::from(crate::destiny::hull::of_in(w.content(), t).radius)
    };
    Some(
        crate::space_state::distance(a.motion.position, b.motion.position)
            - radius(w, a.character_id)
            - radius(w, b.character_id),
    )
}

/// The target ship's type name, for the dialog.
fn target_name(w: &Registry, ship: i64) -> String {
    w.roster()
        .by_ship(ship)
        .and_then(|p| w.pilot(p.character_id))
        .and_then(|p| crate::item::find_in(w.content(), p.ship_type_id))
        .map_or("ship", |t| t.name)
        .to_owned()
}

/// `TargetTooFar`'s dict, shared by the refusal and the mid-cycle stop.
fn too_far_args(
    w: &Registry,
    module_type: i64,
    gap: f64,
    target: i64,
) -> Vec<(&'static str, PyValue)> {
    too_far_pairs(module_type, gap, &target_name(w, target))
}

/// `TargetTooFar`'s loc args. The message text reads `[item]module.name`, so
/// `module` is the **plain typeID**: the client's `GetMessage` formats a
/// `(UE_TYPEID, id)` pair into the type's *name* first, and the item
/// property handler then fails `int('Multispectral ECM I')` (the `fci_ewar`
/// traceback `_GetByMessageID ... TypeNotFoundException`) `[V]`.
pub(crate) fn too_far_pairs(
    module_type: i64,
    gap: f64,
    target_name: &str,
) -> Vec<(&'static str, PyValue)> {
    vec![
        ("module", PyValue::Int(module_type)),
        ("distance", PyValue::Real(gap.max(0.0))),
        ("targetname", crate::pyutil::bstr(target_name)),
    ]
}

/// What `modules::activate` asks of every targeted module with no check of
/// its own: the target, if a ship, within the module's reach. A target that
/// is not a ship in the roster (a rock, a wreck, a can) is that kind's own
/// module's business and passes.
pub fn check_activation(
    w: &mut Registry,
    ship_id: i64,
    module_id: i64,
    module: &ModuleType,
    effect: &Effect,
    target_id: Option<i64>,
) -> Result<(), ServiceError> {
    if !is_ours(module, effect, target_id) {
        return Ok(());
    }
    let Some(target) = target_id else {
        return Ok(());
    };
    let Some(reach) = reach_of(w, ship_id, module_id, module, effect) else {
        return Ok(());
    };
    let Some(gap) = gap_between(w, ship_id, target) else {
        return Ok(());
    };
    if gap <= reach {
        return Ok(());
    }
    Err(modules::user_error(
        "TargetTooFar",
        too_far_args(w, module.type_id, gap, target),
    ))
}

/// **Mid-cycle**: every running targeted module whose target has left its
/// reach is stopped **at the end of its running cycle** (the boundary-stop path
/// of GUN-CYCLE's `target_lost`; until then an Activate is refused), with
/// `TargetTooFar` as the reason. Called from
/// `modules::tick`. Returns how many it stopped.
pub fn sweep(w: &mut Registry, now: i64) -> usize {
    let mut stopped = 0;
    for r in w.modules().running_all() {
        let Some(target) = r.target_id else { continue };
        let Some(module) = modules::find_in(w.content(), r.type_id).copied() else {
            continue;
        };
        if r.stop_at.is_some() || !is_ours(&module, &r.effect, Some(target)) {
            continue;
        }
        let Some(reach) = reach_of(w, r.ship_id, r.module_id, &module, &r.effect) else {
            continue;
        };
        let Some(gap) = gap_between(w, r.ship_id, target) else {
            continue;
        };
        if gap <= reach {
            continue;
        }
        let error = PyValue::Tuple(vec![
            crate::pyutil::bstr("TargetTooFar"),
            PyValue::Dict(crate::pyutil::pypairs(too_far_args(
                w, r.type_id, gap, target,
            ))),
        ]);
        if modules::stop_at_boundary(w, r.module_id, now, Some(error)) {
            stopped += 1;
        }
    }
    stopped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space_state::{Motion, Presence};

    const ALICE: i64 = 1_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    static CLOAK: ModuleType = ModuleType {
        type_id: 990_000_901,
        name: "Fixture Cloak",
        effect: None,
        attributes: &[(ATTR_DISALLOW_ON_WARP, 1.0)],
        rack: crate::fitting::Rack::High,
        hardpoint: None,
    };

    static SCANNER: ModuleType = ModuleType {
        type_id: 990_840_901,
        name: "Fixture Scanner",
        effect: None,
        attributes: &[(125, 20_000.0), (510, 4_000.0), (126, 15_000.0)],
        rack: crate::fitting::Rack::High,
        hardpoint: None,
    };

    fn scanner_effect(effect_id: i64) -> Effect {
        Effect {
            effect_id,
            name: "fixture",
            category: modules::CATEGORY_TARGET,
            guid: "",
            is_offensive: false,
            duration_attribute: 73,
            warp_safe: None,
        }
    }

    /// The `TargetTooFar` dialog's `module` is the plain typeID: the client
    /// turns a `(UE_TYPEID, id)` pair into a name before the `[item]module.name`
    /// handler reads it, and `int(name)` is a traceback.
    #[test]
    fn too_far_module_arg_is_a_plain_type_id() {
        let args = too_far_pairs(990_840_901, -3.0, "Fixture Hull");
        assert_eq!(args[0], ("module", PyValue::Int(990_840_901)));
        assert_eq!(args[1], ("distance", PyValue::Real(0.0)));
        assert_eq!(args.len(), 3);
    }

    /// A scanner has no attribute 54: its reach is its own 125/510 or 126,
    /// and a scanner is checked like any targeted module (`TargetTooFar`).
    #[test]
    fn scanners_reach_by_their_own_attributes_and_are_range_checked() {
        let reg = world(0);
        let ship = reg.roster().get(ALICE).expect("in space").ship_id;
        let ship_scan = scanner_effect(crate::ship_scan::SHIP_SCAN);
        let cargo_scan = scanner_effect(crate::ship_scan::CARGO_SCAN);
        assert_eq!(
            reach_of(&reg, ship, 1, &SCANNER, &ship_scan),
            Some(20_000.0 + crate::ewar::FALLOFF_REACH * 4_000.0)
        );
        assert_eq!(
            reach_of(&reg, ship, 1, &SCANNER, &cargo_scan),
            Some(15_000.0)
        );
        assert!(is_ours(&SCANNER, &ship_scan, Some(5)));
        assert!(is_ours(&SCANNER, &cargo_scan, Some(5)));
    }

    fn world(effect_stamp: i32) -> Registry {
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..crate::model::Character::default()
            }],
            ..crate::model::SandboxConfig::default()
        });
        let mut motion = Motion::arrived([0.0; 3], [0.0; 3]);
        motion.mode = crate::destiny::Mode::Warp {
            goto: [1.0e9, 0.0, 0.0],
            effect_stamp,
            total_distance: 1.0e9,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
        reg.roster_mut()
            .arrive(Presence::arriving(ALICE, ALICE_SHIP, 30_000_001, motion));
        reg
    }

    /// `DeniedActivateInWarp` applies once the warp is active, not while the
    /// ship is aligning for it.
    #[test]
    fn a_module_that_forbids_warp_activates_while_aligning_and_not_in_the_tunnel() {
        let mut aligning = world(crate::destiny::tick() as i32 + 3_600);
        assert!(check_warp(&mut aligning, ALICE, &CLOAK).is_ok());
        let mut active = world(crate::destiny::tick() as i32 - 5);
        assert!(check_warp(&mut active, ALICE, &CLOAK).is_err());
    }

    // ---- the mid-cycle stop (RANGE-STOP) ------------------------------

    const BOB: i64 = 1_000_002;
    const BOB_SHIP: i64 = 100_000_002;
    const WEB: i64 = 100_000_060;
    /// Invented rows: a medium-slot module (slot effect 13) with a targeted,
    /// offensive effect (990000010), a 5 s cycle (73) and 10 km reach (54).
    const WEB_ROWS: &str = "\
attribute\t54\t0.0\tmaxRange
attribute\t73\t0.0\tduration
effect\t990000010\t2\t1\t73\t0\t54\t0\t0\tfixtureWeb\t
type\t990000060\t65\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t990000010\t13,990000010\t54=10000.0;73=5000.0\tFixture Web
";

    /// Alice (module fitted and online) and Bob in one system, Bob `gap`
    /// metres off, Alice locking him.
    fn web_world(gap: f64) -> Registry {
        let ch = |id: i64, ship: i64, name: &str| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..crate::model::Character::default()
        };
        let reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![ch(ALICE, ALICE_SHIP, "Alice"), ch(BOB, BOB_SHIP, "Bob")],
            ..crate::model::SandboxConfig::default()
        });
        let mut content = crate::content::Content::fixture();
        content.insert(
            "typedata",
            crate::typedata::Installed::build(
                crate::typedata::Table::parse(WEB_ROWS).expect("the invented rows parse"),
            ),
        );
        let mut reg = reg.with_content(std::sync::Arc::new(content));
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: WEB,
            type_id: 990_000_060,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: 19,
            quantity: 1,
            group_id: 65,
            category_id: 7,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: WEB + 1,
            ..crate::store::Snapshot::default()
        });
        for c in [ALICE, BOB] {
            crate::destiny::on_ballpark_bind(&mut reg, Some(c));
        }
        let origin = reg.roster().get(ALICE).expect("in space").motion.position;
        for (c, dx) in [(ALICE, 0.0), (BOB, gap)] {
            let p = reg.roster_mut().get_mut(c).expect("in space");
            p.motion = Motion::arrived([origin[0] + dx, origin[1], origin[2]], [0.0; 3]);
            p.since = crate::space_state::now_seconds();
        }
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        reg.modules_mut().set_online(WEB, true);
        let _ = reg.take_pending();
        reg
    }

    /// **A target that drifts out of reach mid-cycle does not free the
    /// module** (the exploit GUN-CYCLE closed for a lost target): the module
    /// runs to the end of its cycle, an Activate before that is refused with
    /// `EffectAlreadyActive2`, and at the boundary it stops, with
    /// `TargetTooFar` as the stop's reason.
    #[test]
    fn a_target_out_of_reach_mid_cycle_stops_the_module_at_the_boundary() {
        let t0 = 1_000 * crate::modules::MSEC;
        let go = |reg: &mut Registry, now: i64| {
            modules::activate(
                reg,
                Some(ALICE),
                WEB,
                "fixtureWeb",
                Some(BOB_SHIP),
                1000,
                now,
            )
        };
        let mut reg = web_world(1_000.0);
        go(&mut reg, t0).expect("in reach: starts");
        let r = reg.modules().running(WEB).copied().expect("running");
        assert!(is_ours(
            &modules::find_in(reg.content(), r.type_id)
                .copied()
                .expect("type"),
            &r.effect,
            r.target_id
        ));
        let boundary = r.next_boundary();
        // Bob leaves, 1 000 km, a moment into the cycle.
        let p = reg.roster_mut().get_mut(BOB).expect("in space");
        p.motion.position[0] += 1.0e6;
        let mid = t0 + r.period / 2;
        assert_eq!(sweep(&mut reg, mid), 1);
        let r = reg
            .modules()
            .running(WEB)
            .copied()
            .expect("the cycle runs on");
        assert_eq!(r.stop_at, Some(boundary));
        let e = go(&mut reg, mid + 1).expect_err("still cycling");
        assert!(
            format!("{e:?}").contains(&format!("{:?}", b"EffectAlreadyActive2".to_vec())),
            "{e:?}"
        );
        // At the boundary it stops; the stop pair carries TargetTooFar.
        crate::modules::tick(&mut reg, boundary);
        assert!(reg.modules().running(WEB).is_none(), "stopped at the end");
        let told = format!(
            "{:?}",
            reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(60))
        );
        assert!(
            told.contains(&format!("{:?}", b"TargetTooFar".to_vec())),
            "{told}"
        );
    }
}
