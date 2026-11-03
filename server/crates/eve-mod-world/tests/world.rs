//! The module through `eve_server`'s public face only. Invented values.

use eve_marshal::PyValue;
use eve_mod_world::settings::{self, SETTINGS};
use eve_mod_world::{KIND, MODULE, VERSION};
use eve_server::free_sp;
use eve_server::model::{Character, SandboxConfig};
use eve_server::module::{self, Module};
use eve_server::pyutil::{as_bytes, bstr, dict_get};
use eve_server::rules::{self, Rules};
use eve_server::service::{Caller, Registry, Request, ServiceError};
use std::sync::Once;

/// Fix the process's plan to the module, before anything reads it.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        module::install(&[&MODULE]).expect("the module installs");
    });
}

const FRIGATE: i64 = 3329;
const PILOT: i64 = 91_001;

fn world_with(rules: Rules) -> Registry {
    install();
    Registry::new(SandboxConfig {
        rules,
        characters: vec![Character {
            character_id: PILOT,
            name: "Pilot".to_owned(),
            ..Character::default()
        }],
        ..SandboxConfig::default()
    })
}

fn world() -> Registry {
    world_with(Rules::default())
}

fn skill_call(reg: &mut Registry, method: &str, args: &[PyValue], kw: &[(PyValue, PyValue)]) {
    reg.dispatch(&Request {
        service: "skillMgr2::skillHandler",
        method,
        args,
        kwargs: kw,
        bind_params: None,
        actor: Some(PILOT),
    })
    .expect("answered");
}

/// A pilot with one frigate level in training.
fn training_world() -> Registry {
    let mut reg = world();
    skill_call(
        &mut reg,
        "PurchaseSkills",
        &[PyValue::List(vec![PyValue::Int(FRIGATE)])],
        &[],
    );
    let info = PyValue::Dict(vec![(
        PyValue::Tuple(vec![PyValue::Int(FRIGATE), PyValue::Int(1)]),
        PyValue::Int(0),
    )]);
    let kw = [(PyValue::Bool(true), bstr("activate"))];
    skill_call(&mut reg, "SaveNewQueue", &[info], &kw);
    reg
}

/// Time left on the head of the pilot's queue, as the client reads it.
fn head_left(reg: &mut Registry) -> i64 {
    let out = reg
        .dispatch(&Request {
            service: "skillMgr2::skillHandler",
            method: "GetSkillQueueAndFreePoints",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: Some(PILOT),
        })
        .expect("answered");
    let PyValue::Tuple(t) = out else {
        panic!("a pair: {out:?}")
    };
    let PyValue::List(rows) = &t[0] else {
        panic!("rows: {:?}", t[0])
    };
    let PyValue::Instance { state, .. } = &rows[0] else {
        panic!("a row: {:?}", rows[0])
    };
    let Some(PyValue::Int(end)) = dict_get(state, b"trainingEndTime") else {
        panic!("an end time: {state:?}")
    };
    end - eve_server::pyutil::filetime_now()
}

fn slash(reg: &mut Registry, role: i64, line: &str) -> Result<PyValue, ServiceError> {
    let args = [bstr(&format!("{line} "))];
    reg.dispatch_as(
        Caller::Session { user_id: 1, role },
        &Request {
            service: "slash",
            method: "SlashCmd",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(PILOT),
        },
    )
}

const GM: i64 = eve_server::roles::ROLE_GML;

/// The `info` of a raised `CustomInfo`, or the label of what came instead.
fn info(out: Result<PyValue, ServiceError>) -> String {
    let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = &out else {
        panic!("expected a raised error, got {out:?}")
    };
    let PyValue::Tuple(h) = header.as_ref() else {
        panic!("a header")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("args")
    };
    assert_eq!(args[0], bstr("CustomInfo"), "{args:?}");
    let text = dict_get(&args[1], b"info").and_then(as_bytes);
    String::from_utf8_lossy(text.expect("an info text")).into_owned()
}

fn stored(reg: &Registry, name: &str) -> Option<String> {
    reg.deltas()
        .of(KIND)
        .get(&vec![name.to_owned()])
        .and_then(|v| v.first().cloned())
}

#[test]
fn the_module_adds_its_rows_and_only_those() {
    let core = module::build(&[]).expect("core");
    let with = module::build(&[&MODULE]).expect("assembles");
    assert_eq!(with.modules, [("world", VERSION)]);
    assert!(with.kinds.iter().any(|k| k.0 == KIND));
    assert_eq!(with.kinds.len(), core.kinds.len() + 1);
    assert_eq!(with.commands.len(), core.commands.len() + 1);
    assert_eq!(with.flags.len(), 5);
    assert_eq!(with.world_loaded.len(), 1);
    assert_eq!(with.stages.len(), core.stages.len());
    assert_eq!(with.services.len(), core.services.len());
    assert_eq!(with.on_login.len(), core.on_login.len());
    assert_eq!(with.rules.len(), core.rules.len());
}

/// `no_modules_is_1to1`: no module, the core's rules and hooks are empty.
#[test]
fn no_modules_is_1to1() {
    let core = module::build(&[&Module::NONE]).expect("core");
    assert!(core.world_loaded.is_empty());
    assert!(core.flags.is_empty());
    let mut r = Rules::default();
    core.apply_rules(&mut r);
    assert_eq!(r, Rules::default());
}

#[test]
fn a_fresh_world_stores_what_it_runs_on() {
    let flagged = Rules {
        training_mult: 3.0,
        start_isk: 250.0,
        start_sp: 40,
        ..Rules::default()
    };
    let mut reg = world_with(flagged);
    settings::startup(&mut reg).expect("starts");
    assert_eq!(reg.config().rules, flagged);
    assert_eq!(stored(&reg, "training").as_deref(), Some("3"));
    assert_eq!(stored(&reg, "startisk").as_deref(), Some("250"));
    assert_eq!(stored(&reg, "startsp").as_deref(), Some("40"));
}

#[test]
fn a_stored_value_wins_over_a_differing_flag_on_reload() {
    let mut first = world_with(Rules {
        training_mult: 3.0,
        ..Rules::default()
    });
    settings::startup(&mut first).expect("starts");
    // The same world file, started with another flag.
    let mut again = world_with(Rules {
        training_mult: 7.0,
        start_sp: 9,
        ..Rules::default()
    });
    for s in SETTINGS {
        let row = first
            .deltas()
            .of(KIND)
            .get(&vec![s.name.to_owned()])
            .cloned();
        again
            .deltas_mut()
            .put(KIND, vec![s.name.to_owned()], row.expect("a row"));
    }
    settings::startup(&mut again).expect("starts");
    assert_eq!(again.config().rules.training_mult, 3.0);
    assert_eq!(again.config().rules.start_sp, 0);
}

#[test]
fn an_old_world_with_no_row_gets_one_written() {
    let mut reg = world();
    reg.deltas_mut()
        .put(KIND, vec!["startisk".to_owned()], vec!["12".to_owned()]);
    settings::startup(&mut reg).expect("starts");
    assert_eq!(reg.config().rules.start_isk, 12.0);
    assert_eq!(stored(&reg, "training").as_deref(), Some("1"));
    assert_eq!(stored(&reg, "startsp").as_deref(), Some("0"));
}

#[test]
fn a_bad_row_refuses_startup_and_an_unknown_one_is_kept() {
    let mut reg = world();
    reg.deltas_mut()
        .put(KIND, vec!["training".to_owned()], vec!["fast".to_owned()]);
    let e = settings::startup(&mut reg).expect_err("refused");
    assert!(e.contains("training"), "{e}");
    assert_eq!(reg.config().rules, Rules::default());

    let mut reg = world();
    reg.deltas_mut()
        .put(KIND, vec!["startsp".to_owned()], vec!["1.5".to_owned()]);
    assert!(settings::startup(&mut reg).is_err());

    let mut reg = world();
    reg.deltas_mut()
        .put(KIND, vec!["later".to_owned()], vec!["1".to_owned()]);
    settings::startup(&mut reg).expect("starts");
    assert_eq!(stored(&reg, "later").as_deref(), Some("1"));
}

#[test]
fn a_stored_out_of_range_value_refuses_startup() {
    let mut reg = world();
    reg.deltas_mut()
        .put(KIND, vec!["training".to_owned()], vec!["0".to_owned()]);
    assert!(settings::startup(&mut reg).is_err());
}

#[test]
fn a_live_training_change_keeps_trained_time_and_is_stored() {
    let mut reg = training_world();
    settings::startup(&mut reg).expect("starts");
    let before = head_left(&mut reg);
    let out = slash(&mut reg, GM, "/world training 4");
    assert!(info(out).contains("training"), "replies");
    assert_eq!(reg.config().rules.training_mult, 4.0);
    assert_eq!(rules::training_pace_of(&reg), 4.0);
    assert_eq!(stored(&reg, "training").as_deref(), Some("4"));
    let after = head_left(&mut reg);
    assert!(
        (after - before / 4).abs() < before / 50,
        "{after} vs {before}"
    );
}

#[test]
fn a_new_character_gets_the_new_start_values_and_old_balances_stay() {
    let mut reg = world_with(Rules {
        start_isk: 5000.0,
        start_sp: 300,
        ..Rules::default()
    });
    settings::startup(&mut reg).expect("starts");
    let old = reg.character(PILOT).expect("pilot").balance;
    assert_eq!(reg.character(PILOT).expect("pilot").balance, old);
    assert_eq!(free_sp::of(&reg, PILOT), 0);
    let id = reg.create_owned_character(7, "Newcomer".to_owned());
    assert_eq!(reg.character(id).expect("new").balance, 5000.0);
    assert_eq!(free_sp::of(&reg, id), 300);
}

#[test]
fn a_bad_command_changes_nothing() {
    let mut reg = training_world();
    settings::startup(&mut reg).expect("starts");
    let before = head_left(&mut reg);
    for line in [
        "/world training 0",
        "/world training 1000000",
        "/world training fast",
        "/world startsp 1.5",
        "/world startsp -3",
        "/world startisk -1",
        "/world nonsense 3",
    ] {
        let text = info(slash(&mut reg, GM, line));
        assert!(!text.is_empty());
        assert_eq!(reg.config().rules, Rules::default(), "{line}");
    }
    assert_eq!(stored(&reg, "training").as_deref(), Some("1"));
    assert_eq!(stored(&reg, "startsp").as_deref(), Some("0"));
    assert!((head_left(&mut reg) - before).abs() < before / 50);
    // With no arguments it lists.
    assert!(info(slash(&mut reg, GM, "/world")).contains("startsp"));
}

#[test]
fn a_player_session_is_refused_by_the_slash_gate() {
    let mut reg = world();
    settings::startup(&mut reg).expect("starts");
    let out = slash(&mut reg, 0, "/world training 4");
    let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = &out else {
        panic!("refused: {out:?}")
    };
    let PyValue::Tuple(h) = header.as_ref() else {
        panic!("a header")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("args")
    };
    assert_eq!(args[0], bstr("SlashError"));
    assert_eq!(reg.config().rules, Rules::default());
}

#[test]
fn the_flags_parse_and_refuse() {
    let a = module::build(&[&MODULE]).expect("assembles");
    let mut r = Rules::default();
    (a.flag("--world-training").unwrap().apply)(&mut r, Some("2.5")).unwrap();
    (a.flag("--world-start-isk").unwrap().apply)(&mut r, Some("77")).unwrap();
    (a.flag("--world-start-sp").unwrap().apply)(&mut r, Some("12")).unwrap();
    assert_eq!((r.training_mult, r.start_isk, r.start_sp), (2.5, 77.0, 12));
    assert!((a.flag("--world-start-sp").unwrap().apply)(&mut r, Some("1.5")).is_err());
    assert!((a.flag("--world-training").unwrap().apply)(&mut r, Some("x")).is_err());
    assert!((a.flag("--world-training").unwrap().apply)(&mut r, None).is_err());
}

#[test]
fn first_launch_settings_refuse_the_command_and_still_list() {
    use eve_mod_world::settings::Kind;
    let kind = |n: &str| SETTINGS.iter().find(|s| s.name == n).expect("known").kind;
    assert_eq!(kind("training"), Kind::Live);
    assert_eq!(kind("startisk"), Kind::FirstLaunch);
    assert_eq!(kind("startsp"), Kind::FirstLaunch);

    let seeded = Rules {
        start_isk: 250.0,
        start_sp: 40,
        ..Rules::default()
    };
    let mut reg = world_with(seeded);
    settings::startup(&mut reg).expect("starts");
    for line in [
        "/world startisk 9999",
        "/world startsp 5",
        "/world StartSP 5",
    ] {
        let text = info(slash(&mut reg, GM, line));
        assert!(text.contains("first-launch"), "{line}: {text}");
        assert!(text.contains("startisk = 250"), "{text}");
        assert_eq!(reg.config().rules, seeded, "{line}");
    }
    assert_eq!(stored(&reg, "startisk").as_deref(), Some("250"));
    assert_eq!(stored(&reg, "startsp").as_deref(), Some("40"));
    // The listing shows them, and training stays live.
    assert!(info(slash(&mut reg, GM, "/world")).contains("startsp = 40"));
    info(slash(&mut reg, GM, "/world training 2"));
    assert_eq!(reg.config().rules.training_mult, 2.0);
    assert_eq!(reg.config().rules.start_isk, 250.0);
}

#[test]
fn the_anchor_time_is_live_and_bounded() {
    let mut reg = world();
    settings::startup(&mut reg).expect("starts");
    assert_eq!(reg.config().rules.upwell_anchor_secs, 3_600);
    info(slash(&mut reg, GM, "/world anchortime 600"));
    assert_eq!(reg.config().rules.upwell_anchor_secs, 600);
    assert_eq!(stored(&reg, "anchortime").as_deref(), Some("600"));
    for line in [
        "/world anchortime 5",
        "/world anchortime 1.5",
        "/world anchortime 99999999",
    ] {
        info(slash(&mut reg, GM, line));
        assert_eq!(reg.config().rules.upwell_anchor_secs, 600, "{line}");
    }
}

#[test]
fn the_delete_wait_is_live_and_bounded() {
    let mut reg = world();
    settings::startup(&mut reg).expect("starts");
    assert_eq!(
        reg.config().rules.char_delete_secs,
        rules::DEFAULT_CHAR_DELETE_SECS
    );
    info(slash(&mut reg, GM, "/world deletetime 0"));
    assert_eq!(reg.config().rules.char_delete_secs, 0);
    assert_eq!(stored(&reg, "deletetime").as_deref(), Some("0"));
    info(slash(&mut reg, GM, "/world deletetime 600"));
    let too_long = format!(
        "/world deletetime {}",
        rules::CHAR_DELETE_SECS_RANGE.end() + 1
    );
    for line in ["/world deletetime -5", "/world deletetime 1.5", &too_long] {
        info(slash(&mut reg, GM, line));
        assert_eq!(reg.config().rules.char_delete_secs, 600, "{line}");
    }
}
