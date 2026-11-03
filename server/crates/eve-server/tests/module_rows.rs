//! A module in another crate can fill every row of `Module`, using only
//! what `eve_server` exports (PR-SEAM-1). Invented names throughout.

use eve_marshal::PyValue;
use eve_server::content::{Boxed, Content, Sources, TableSpec};
use eve_server::damage::BlueTime;
use eve_server::mission::Change;
use eve_server::module::{
    self, Command, CrateRow, Day, Deltas, Flag, KillEvent, KillRow, MinedEvent, MinedRow,
    MissionEvent, Module, Placed, Probe, ProbeResult, Refusal, RolloverRun, RolloverStep,
    SessionRow, SlotSpec, Stage,
};
use eve_server::module::{Loot, OpenEvent};
use eve_server::rules::Rules;
use eve_server::service::{Registry, Request, ServiceError};
use std::time::Instant;

fn dispatch(_: &mut Registry, _: &Request<'_>) -> Result<PyValue, ServiceError> {
    Ok(PyValue::None)
}
fn slash(_: &mut Registry, _: Option<i64>, _: &[&str]) -> Result<PyValue, ServiceError> {
    Ok(PyValue::None)
}
fn stage(reg: &mut Registry, _: Instant, _: BlueTime) {
    let _deltas: &Deltas = reg.deltas();
}
fn kill(_: &mut Registry, _: &KillEvent) {}
fn mined(_: &mut Registry, _: &MinedEvent) {}
fn settled(_: &mut Registry, _: &eve_server::module::Settled) {}
fn is_crate(_: &Registry, _: i64) -> bool {
    false
}
fn open(_: &mut Registry, _: &OpenEvent) -> Vec<Loot> {
    Vec::new()
}
fn session(_: &mut Registry, _: i64) {}
fn mission(_: &mut Registry, _: &MissionEvent) {}
fn step(_: &mut Registry, _: &Day) -> usize {
    0
}
fn slim(_: &Registry, _: &[i64]) -> Vec<(i64, Vec<(&'static str, PyValue)>)> {
    Vec::new()
}
fn scan(_: &mut Registry, _: i64, _: &[Probe]) -> Vec<ProbeResult> {
    Vec::new()
}
fn guard(_: &Registry, _: i64) -> Option<Refusal> {
    None
}
fn slot(_: &Content) -> Boxed {
    Box::new(0x5eed_u32)
}
fn load(_: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(0x7ab1_u64))
}
fn fixture() -> Boxed {
    Box::new(0x7ab1_u64)
}
fn world_loaded(_: &mut Registry) -> Result<(), String> {
    Ok(())
}
fn flag(r: &mut Rules, v: Option<&str>) -> Result<(), String> {
    r.start_isk = v
        .ok_or("needs a value")?
        .parse()
        .map_err(|_| "not a number")?;
    Ok(())
}
fn rules(r: &mut Rules) {
    r.training_mult = 2.0;
}

const SERVICES: &[&str] = &["extSvc"];

#[test]
fn another_crate_fills_every_row() {
    let m = Module {
        name: "ext",
        version: 1,
        rules: Some(rules),
        tables: &[TableSpec {
            name: "ext_table",
            load,
            fixture,
        }],
        slots: &[SlotSpec {
            name: "ext_slot",
            make: slot,
        }],
        kinds: &[("extkind", "ext")],
        stages: &[Placed {
            stage: Stage {
                name: "ext_stage",
                run: stage,
                every: 1,
                phase: 0,
            },
            after: "liveness",
        }],
        services: &[(SERVICES, dispatch)],
        commands: &[Command {
            name: "extcmd",
            usage: "/extcmd",
            run: slash,
        }],
        kill_player: &[KillRow {
            name: "ext_kp",
            run: kill,
        }],
        kill_npc: &[KillRow {
            name: "ext_kn",
            run: kill,
        }],
        on_login: &[SessionRow {
            name: "ext_in",
            run: session,
        }],
        on_logoff: &[SessionRow {
            name: "ext_out",
            run: session,
        }],
        mission: &[(Change::Completed, mission)],
        rollover: &[RolloverStep {
            name: "ext_step",
            run: RolloverRun::Once(step),
        }],
        slim: &[slim],
        scan: &[scan],
        warp_guards: &[guard],
        jump_guards: &[guard],
        cli: &[Flag {
            name: "--ext-isk",
            takes_value: true,
            usage: "starting ISK",
            apply: flag,
        }],
        world_loaded: &[world_loaded],
        crates: &[CrateRow {
            name: "ext_crate",
            is_crate,
            open,
        }],
        mined: &[MinedRow {
            name: "ext_mined",
            run: mined,
        }],
        settled: &[eve_server::module::SettledRow {
            name: "ext_settled",
            run: settled,
        }],
    };
    let a = module::build(&[&m]).expect("assembles");
    assert_eq!(a.modules, [("ext", 1)]);
    assert!(a.stages.iter().any(|s| s.name == "ext_stage"));
    assert!(a.flag("--ext-isk").is_some());
    assert_eq!(a.world_loaded.len(), 1);
    assert_eq!((a.crates.len(), a.mined.len(), a.settled.len()), (1, 1, 1));
    let mut r = Rules::default();
    a.apply_rules(&mut r);
    assert_eq!(r.training_mult, 2.0);
}
