//! A pilot installs reverse engineering on an ancient relic over the wire and
//! gets a Tech 3 blueprint copy (lane `t3-a`, T3-B and T3-C).
//!
//! **Invented ids and rows**: the relic, datacores, skills, products and the
//! decryptor are made up, and the chance of the relic row is certainty, so
//! the one roll cannot fail. Only the shapes are the client's.

use super::*;
use crate::pyutil::dict_get;

const ACTOR: i64 = 90_000_001;
const RELIC: i64 = 990_901;
const CORE_A: i64 = 990_911;
const CORE_B: i64 = 990_912;
const SKILLS: [i64; 3] = [990_921, 990_922, 990_923];
const HULL_BP: i64 = 990_931;
const HULL_BP_2: i64 = 990_932;
const HULL: i64 = 990_941;
const HULL_2: i64 = 990_942;
const DECRYPTOR: i64 = 990_951;
/// The relic row's runs per copy, and the decryptor's invented attributes.
const COPY_RUNS: i64 = 20;
const DECRYPTOR_RUNS: i64 = 4;
const CORES_PER_RUN: i64 = 3;

fn content() -> crate::content::Content {
    let mut rows: Vec<String> = vec![
        format!("{RELIC}\t8\tactivity\t0\t0\t\t3600\t300"),
        format!("{RELIC}\t8\tmaterial\t{CORE_A}\t{CORES_PER_RUN}\t\t3600\t300"),
        format!("{RELIC}\t8\tmaterial\t{CORE_B}\t{CORES_PER_RUN}\t\t3600\t300"),
    ];
    for hull in [HULL_BP, HULL_BP_2] {
        rows.push(format!(
            "{RELIC}\t8\tproduct\t{hull}\t{COPY_RUNS}\t1.0\t3600\t300"
        ));
    }
    for s in SKILLS {
        rows.push(format!("{RELIC}\t8\tskill\t{s}\t1\t\t3600\t300"));
    }
    for (bp, product) in [(HULL_BP, HULL), (HULL_BP_2, HULL_2)] {
        rows.push(format!("{bp}\t1\tactivity\t0\t0\t\t900\t1"));
        rows.push(format!("{bp}\t1\tmaterial\t34\t100\t\t900\t1"));
        rows.push(format!("{bp}\t1\tproduct\t{product}\t1\t\t900\t1"));
        rows.push(format!("{bp}\t1\tskill\t3380\t1\t\t900\t1"));
    }
    let prices = crate::econ_prices::Table::parse(&["34\t4.0\tbase\t1.0\t4\t18\t\t1"])
        .expect("invented prices parse");
    let table = crate::industry::Table::parse(&rows, &prices).expect("invented rows parse");
    let ty = |id: i64, group: i64, cat: i64, attrs: &str| {
        format!(
            "type\t{id}\t{group}\t{cat}\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t{attrs}\tFixture {id}\n"
        )
    };
    let mut types = String::new();
    types += &ty(RELIC, 990_971, 34, "");
    types += &ty(CORE_A, 990_333, 17, "");
    types += &ty(CORE_B, 990_333, 17, "");
    types += &ty(HULL_BP, 990_105, 9, "");
    types += &ty(HULL_BP_2, 990_105, 9, "");
    types += &ty(
        DECRYPTOR,
        crate::industry::relic::GROUP_DECRYPTORS,
        35,
        &format!("1112=1.0;1113=1.0;1114=2.0;1124={DECRYPTOR_RUNS}.0"),
    );
    let text = format!("{}{types}", crate::typedata::FIXTURE);
    let parsed = crate::typedata::Table::parse(&text).expect("the test rows parse");
    let mut c = crate::content::Content::fixture();
    c.insert("typedata", crate::typedata::Installed::build(parsed));
    c.insert("industry", table);
    c
}

fn call_in(
    reg: &mut Registry,
    service: &str,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    dispatch(
        reg,
        &Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ACTOR),
        },
    )
}

fn location(item_id: i64) -> PyValue {
    PyValue::ObjectEx {
        form: eve_marshal::ObjectForm::New,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Tuple(vec![PyValue::Global(b"industry.job.Location".to_vec())]),
            pydict(vec![
                ("itemID", PyValue::Int(item_id)),
                ("flagID", PyValue::Int(crate::industry::FLAG_HANGAR)),
            ]),
        ])),
        list: vec![],
        dict: vec![],
    }
}

fn attr(row: &PyValue, key: &[u8]) -> PyValue {
    let PyValue::Instance { state, .. } = row else {
        panic!("a blueprint row is a KeyVal: {row:?}")
    };
    dict_get(state, key)
        .cloned()
        .unwrap_or_else(|| panic!("no {:?}", String::from_utf8_lossy(key)))
}

fn rows(reg: &mut Registry) -> Vec<PyValue> {
    let out = call_in(
        reg,
        "blueprintManager",
        "GetBlueprintDataByOwner",
        &[PyValue::Int(ACTOR), PyValue::None],
    )
    .expect("the blueprint list");
    let PyValue::Tuple(parts) = out else {
        panic!("a 2-tuple")
    };
    let PyValue::List(rows) = parts[0].clone() else {
        panic!("a list")
    };
    rows
}

fn held(reg: &Registry, type_id: i64) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.owner_id == ACTOR && i.type_id == type_id)
        .map(|i| i.quantity)
        .sum()
}

/// [`ACTOR`] holds `units` relics, datacores for `runs`, one decryptor, the
/// three skills and ISK, in the station they are docked in.
fn world(units: i64, runs: i64) -> (Registry, i64, i64) {
    let mut reg =
        Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(content()));
    let station = reg
        .character(ACTOR)
        .and_then(|c| c.station_id)
        .expect("the fixture character is docked");
    reg.skills.insert(
        ACTOR,
        crate::skill::Skills::restore(
            SKILLS
                .iter()
                .map(|&type_id| crate::skill::SkillEntry {
                    type_id,
                    level: 1,
                    points: 0,
                    rank: 1,
                })
                .collect(),
        ),
    );
    if let Some(c) = reg.character_mut(ACTOR) {
        c.balance = 1_000_000_000.0;
    }
    let at = crate::item::Slot::new(ACTOR, station, crate::industry::FLAG_HANGAR);
    let create = |type_id, quantity| crate::item::Edit::Create {
        type_id,
        quantity,
        to: at,
    };
    reg.apply_items(&[
        create(RELIC, units),
        create(CORE_A, CORES_PER_RUN * runs),
        create(CORE_B, CORES_PER_RUN * runs),
        create(DECRYPTOR, 1),
    ])
    .expect("the invented items apply");
    let relic_id = reg
        .store()
        .items()
        .iter()
        .find(|i| i.type_id == RELIC)
        .expect("the relic stack")
        .item_id;
    (reg, station, relic_id)
}

#[test]
fn a_pilot_installs_relic_invention_with_a_decryptor_and_gets_a_t3_copy() {
    let (units, runs) = (3, 2);
    let (mut reg, station, relic_id) = world(units, runs);

    // 1. The window lists the stack: runs = quantity, a positive quantity, not
    //    in a job. (`BlueprintInstance`: category 34 is never an original.)
    let listed = rows(&mut reg);
    let stack = listed
        .iter()
        .find(|r| attr(r, b"typeID") == PyValue::Int(RELIC))
        .expect("the relic stack is listed");
    assert_eq!(attr(stack, b"itemID"), PyValue::Int(relic_id));
    assert_eq!(attr(stack, b"runs"), PyValue::Int(units));
    assert_eq!(attr(stack, b"quantity"), PyValue::Int(units));
    assert_eq!(attr(stack, b"jobID"), PyValue::None);
    assert_eq!(attr(stack, b"materialEfficiency"), PyValue::Int(0));
    // And by id.
    let one = call_in(
        &mut reg,
        "blueprintManager",
        "GetBlueprintData",
        &[PyValue::Int(relic_id)],
    )
    .expect("by id");
    assert_eq!(attr(&one, b"runs"), PyValue::Int(units));

    // 2. Install `runs` runs of the first product, with the decryptor among
    //    the materials the way `Job.dump()` sends them.
    let materials = int_keyed_dict(vec![
        (CORE_A, PyValue::Int(CORES_PER_RUN * runs)),
        (CORE_B, PyValue::Int(CORES_PER_RUN * runs)),
        (DECRYPTOR, PyValue::Int(1)),
    ]);
    let dump = pydict(vec![
        ("activityID", PyValue::Int(crate::industry::INVENTION)),
        ("blueprintID", PyValue::Int(relic_id)),
        ("runs", PyValue::Int(runs)),
        ("licensedRuns", PyValue::None),
        ("productTypeID", PyValue::Int(HULL_BP)),
        ("materials", materials),
        ("inputLocation", location(station)),
        ("outputLocation", location(station)),
    ]);
    let answer =
        call_in(&mut reg, "industryManager", "InstallJob", &[dump]).expect("a relic job installs");
    let PyValue::Int(job_id) = answer else {
        panic!("the job id: {answer:?}")
    };
    assert_eq!(held(&reg, RELIC), units - runs, "one unit per run");
    assert_eq!(held(&reg, CORE_A), 0);
    assert_eq!(held(&reg, CORE_B), 0);
    assert_eq!(held(&reg, DECRYPTOR), 0, "the decryptor is consumed");
    let job = *reg.jobs().get(job_id).expect("the job");
    assert_eq!(job.activity_id, crate::industry::INVENTION);
    assert_eq!(job.optional_type_id, Some(DECRYPTOR));
    assert_eq!(job.product_type_id, HULL_BP);
    let pushed: Vec<&str> = reg
        .take_pending()
        .iter()
        .map(|(_, n)| n.to_push().0)
        .collect();
    assert!(pushed.contains(&"OnIndustryJob"), "{pushed:?}");
    assert!(
        pushed.contains(&"OnBlueprintsUpdated"),
        "the stack lost units: {pushed:?}"
    );
    // What is left of the stack is listed with its new size and no job.
    let listed = rows(&mut reg);
    let left = listed
        .iter()
        .find(|r| attr(r, b"typeID") == PyValue::Int(RELIC))
        .expect("the rest of the stack");
    assert_eq!(attr(left, b"runs"), PyValue::Int(units - runs));
    assert_eq!(attr(left, b"jobID"), PyValue::None);

    // 3. The clock runs out; deliver.
    let mut done = job;
    done.end_date = 1;
    reg.jobs_mut().commit(done);
    call_in(
        &mut reg,
        "industryManager",
        "CompleteJob",
        &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
    )
    .expect("a ready job delivers");
    assert_eq!(
        held(&reg, HULL_BP),
        runs,
        "a certain chance: one copy per run, as a stack of `successfulRuns`"
    );
    let listed = rows(&mut reg);
    let copies: Vec<&PyValue> = listed
        .iter()
        .filter(|r| attr(r, b"typeID") == PyValue::Int(HULL_BP))
        .collect();
    assert!(!copies.is_empty(), "the T3 copies are listed");
    for c in copies {
        assert_eq!(attr(c, b"quantity"), PyValue::Int(-2), "the mark of a copy");
        assert_eq!(
            attr(c, b"runs"),
            PyValue::Int(COPY_RUNS + DECRYPTOR_RUNS),
            "the row's runs plus the decryptor's"
        );
        assert_eq!(
            attr(c, b"materialEfficiency"),
            PyValue::Int(crate::industry::INVENTION_MATERIAL_EFFICIENCY + 1)
        );
        assert_eq!(
            attr(c, b"timeEfficiency"),
            PyValue::Int(crate::industry::INVENTION_TIME_EFFICIENCY + 2)
        );
    }
}

#[test]
fn a_relic_without_its_datacores_refuses_and_changes_nothing() {
    let (mut reg, station, relic_id) = world(2, 1);
    // One datacore short: ask for two runs' worth.
    let dump = pydict(vec![
        ("activityID", PyValue::Int(crate::industry::INVENTION)),
        ("blueprintID", PyValue::Int(relic_id)),
        ("runs", PyValue::Int(2)),
        ("productTypeID", PyValue::Int(HULL_BP_2)),
        ("inputLocation", location(station)),
        ("outputLocation", location(station)),
    ]);
    let err = call_in(&mut reg, "industryManager", "InstallJob", &[dump])
        .expect_err("short of datacores");
    assert_eq!(user_error_msg(&err), "IndustryValidationError");
    assert_eq!(held(&reg, RELIC), 2, "nothing was taken");
    assert_eq!(held(&reg, CORE_A), CORES_PER_RUN);
}

/// The `msg` of a raised `UserError`.
fn user_error_msg(e: &ServiceError) -> String {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("not a raised exception: {e:?}")
    };
    let PyValue::Tuple(parts) = header.as_ref() else {
        panic!("a 0x22 header is a tuple")
    };
    let PyValue::Tuple(args) = &parts[1] else {
        panic!("UserError's header args are a tuple")
    };
    match &args[0] {
        PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        other => panic!("the message id is a str: {other:?}"),
    }
}
