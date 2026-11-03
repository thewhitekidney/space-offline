//! **Modules and the assembled plan** (PR-SEAM-1, `docs/lanes/pr-seam-1.md`).
//!
//! A [`Module`] is a bundle of table rows. [`build`] concatenates the
//! core's tables and every module's rows, in list order, once at startup,
//! into an [`Assembled`] plan; the call sites that used to read a `const`
//! table (`tick::STAGES`, `DOMAINS`, `slash::COMMANDS`, the hook tables ...)
//! read the plan instead. **The core tables stay where they are**; a module
//! only adds rows after them. There is no trait object and no dispatch
//! through a vtable: a row is a `fn` pointer, grep-able, and a plan field is
//! a slice read.
//!
//! [`Module::NONE`] adds nothing, so the plan of `&[]` or `&[&Module::NONE]`
//! equals the core tables, row for row (`none_is_the_core`). That is what
//! "the 1:1 server" means.
//!
//! **The plan is process-wide**, not a field of `Registry`: the content
//! loader, the delta kinds and the world file read it where no registry
//! exists. [`install`] fixes it once (`cli::run` does, before anything else
//! reads it); [`plan`] without an install is the core-only plan. Reading it
//! is an atomic load.
//!
//! **Startup refusals**, each naming the module and the row: a duplicate
//! service, command, delta kind, table, slot, stage or hook name (against
//! the core and the modules before it); a stage whose `after` names no
//! stage; two modules with one name; rows from a module with no name. A
//! slot *type* clash is caught where the slots are built
//! ([`crate::world_ext::Slots::try_build`]).
//!
//! **The world format** carries each module's `name` and `version`
//! ([`crate::store::format_id`], PR-SEAM-2): a world saved under another
//! module version is backed up before its first save. A server with no module
//! has the core's format, unchanged.
//!
//! **World-loaded hooks** ([`WorldHook`]) run after the world file is read, so
//! a module can apply settings it stored in its own delta rows.
//!
//! **Not here yet** (PR-SEAM-2): the `agent_override` and `health` hooks and
//! the kind prefix.

use crate::content::TableSpec;
pub use crate::crate_hooks::{Loot, OpenEvent, Row as CrateRow};
use crate::kill_hooks;
pub use crate::kill_hooks::Row as KillRow;
pub use crate::mining_hooks::{MinedEvent, Row as MinedRow};
use crate::mission::Change;
use crate::mission_hooks;
use crate::movement_guard;
pub use crate::rollover::Step as RolloverStep;
use crate::rules::Rules;
use crate::scan_sources;
use crate::service::DomainDispatch;
pub use crate::session_hooks::Row as SessionRow;
pub use crate::settle_hooks::{Row as SettledRow, Settled};
use crate::slash;
pub use crate::slash::Command;
use crate::slim_extra;
pub use crate::tick::Stage;
pub use crate::world_ext::SlotSpec;
// The types a module's row functions mention, which live in `pub(crate)`
// modules: re-exported so another crate can write them.
pub use crate::delta::Deltas;
pub use crate::kill_hooks::KillEvent;
pub use crate::mission_hooks::MissionEvent;
pub use crate::movement_guard::Refusal;
pub use crate::rollover::{Day, Run as RolloverRun};
pub use crate::scan::{Probe, ProbeResult};
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// A service list and the function that answers them (a row of
/// `service::DOMAINS`).
pub type Domain = (&'static [&'static str], DomainDispatch);

/// A hook that runs once the world has loaded and before the fresh roster is
/// funded (`server::Server::bind`). An `Err` refuses startup.
pub type WorldHook = fn(&mut crate::service::Registry) -> Result<(), String>;

/// A stage, and the stage it runs right after. `after` may name a core
/// stage or one an earlier row of the same or an earlier module placed.
/// Stages placed after one anchor run in list order, within and across
/// modules.
#[derive(Clone, Copy)]
pub struct Placed {
    pub stage: Stage,
    pub after: &'static str,
}

/// A command-line flag a module adds. `apply` gets the rules and the value
/// (`Some` when `takes_value`).
#[derive(Clone, Copy)]
pub struct Flag {
    /// With its dashes: `--world-speed`.
    pub name: &'static str,
    pub takes_value: bool,
    pub usage: &'static str,
    pub apply: fn(&mut Rules, Option<&str>) -> Result<(), String>,
}

/// What a module adds to the core. Every field is rows; `..Module::NONE`
/// fills the rest.
#[derive(Clone, Copy)]
pub struct Module {
    /// Names the module in refusals and the startup line.
    pub name: &'static str,
    /// The module's own world-format number: bump it when the module's delta
    /// rows change shape. It is part of [`crate::store::format_id`].
    pub version: u32,
    /// Edits the [`Rules`] once at startup, in list order.
    pub rules: Option<fn(&mut Rules)>,
    pub tables: &'static [TableSpec],
    pub slots: &'static [SlotSpec],
    /// `(kind, owner module)`, like `delta_kinds::KINDS`.
    pub kinds: &'static [(&'static str, &'static str)],
    pub stages: &'static [Placed],
    pub services: &'static [Domain],
    pub commands: &'static [Command],
    pub kill_player: &'static [KillRow],
    pub kill_npc: &'static [KillRow],
    pub on_login: &'static [SessionRow],
    pub on_logoff: &'static [SessionRow],
    pub mission: &'static [(Change, mission_hooks::Hook)],
    pub rollover: &'static [RolloverStep],
    pub slim: &'static [slim_extra::Source],
    pub scan: &'static [scan_sources::Source],
    pub warp_guards: &'static [movement_guard::Guard],
    pub jump_guards: &'static [movement_guard::Guard],
    pub cli: &'static [Flag],
    /// Run once after the world loads, before new characters are funded, in
    /// list order. A refusal stops startup.
    pub world_loaded: &'static [WorldHook],
    /// Crate types and what is in them (`crateService`, `crate_hooks`).
    pub crates: &'static [CrateRow],
    /// Run after every mining cut that took ore (`mining_hooks`).
    pub mined: &'static [MinedRow],
    /// Run for every kill an offline settlement books (`settle_hooks`).
    pub settled: &'static [SettledRow],
}

impl Module {
    /// A module that adds nothing.
    pub const NONE: Module = Module {
        name: "",
        version: 0,
        rules: None,
        tables: &[],
        slots: &[],
        kinds: &[],
        stages: &[],
        services: &[],
        commands: &[],
        kill_player: &[],
        kill_npc: &[],
        on_login: &[],
        on_logoff: &[],
        mission: &[],
        rollover: &[],
        slim: &[],
        scan: &[],
        warp_guards: &[],
        jump_guards: &[],
        cli: &[],
        world_loaded: &[],
        crates: &[],
        mined: &[],
        settled: &[],
    };

    fn is_empty(&self) -> bool {
        self.tables.is_empty()
            && self.slots.is_empty()
            && self.kinds.is_empty()
            && self.stages.is_empty()
            && self.services.is_empty()
            && self.commands.is_empty()
            && self.kill_player.is_empty()
            && self.kill_npc.is_empty()
            && self.on_login.is_empty()
            && self.on_logoff.is_empty()
            && self.mission.is_empty()
            && self.rollover.is_empty()
            && self.slim.is_empty()
            && self.scan.is_empty()
            && self.warp_guards.is_empty()
            && self.jump_guards.is_empty()
            && self.cli.is_empty()
            && self.world_loaded.is_empty()
            && self.crates.is_empty()
            && self.mined.is_empty()
            && self.settled.is_empty()
            && self.rules.is_none()
    }
}

/// The core tables with every module's rows after them.
pub struct Assembled {
    /// `(name, version)` of each named module, in list order.
    pub modules: Vec<(&'static str, u32)>,
    pub rules: Vec<fn(&mut Rules)>,
    pub tables: Vec<TableSpec>,
    pub slots: Vec<SlotSpec>,
    pub kinds: Vec<(&'static str, &'static str)>,
    pub stages: Vec<Stage>,
    pub services: Vec<Domain>,
    pub commands: Vec<Command>,
    pub kill_player: Vec<KillRow>,
    pub kill_npc: Vec<KillRow>,
    pub on_login: Vec<SessionRow>,
    pub on_logoff: Vec<SessionRow>,
    pub mission: Vec<(Change, mission_hooks::Hook)>,
    pub rollover: Vec<RolloverStep>,
    pub slim: Vec<slim_extra::Source>,
    pub scan: Vec<scan_sources::Source>,
    pub warp_guards: Vec<movement_guard::Guard>,
    pub jump_guards: Vec<movement_guard::Guard>,
    pub flags: Vec<Flag>,
    pub world_loaded: Vec<WorldHook>,
    pub crates: Vec<CrateRow>,
    pub mined: Vec<MinedRow>,
    pub settled: Vec<SettledRow>,
}

impl Assembled {
    /// Run every module's `rules` function on `rules`, in list order.
    pub fn apply_rules(&self, rules: &mut Rules) {
        for f in &self.rules {
            f(rules);
        }
    }

    /// The module flag called `name`, if one was added.
    pub fn flag(&self, name: &str) -> Option<&Flag> {
        self.flags.iter().find(|f| f.name == name)
    }
}

/// Take `rows` of one module into `out`, refusing a key `seen` already holds.
fn add_unique<T: Copy, K: Ord + std::fmt::Debug + Copy>(
    module: &str,
    what: &str,
    out: &mut Vec<T>,
    seen: &mut BTreeSet<K>,
    rows: &[T],
    key: impl Fn(&T) -> K,
) -> Result<(), String> {
    for row in rows {
        let k = key(row);
        if !seen.insert(k) {
            return Err(format!("module {module:?}: duplicate {what} {k:?}"));
        }
        out.push(*row);
    }
    Ok(())
}

/// Build the plan of `mods` over the core tables, or say what is wrong.
pub fn build(mods: &[&Module]) -> Result<Assembled, String> {
    let mut a = Assembled {
        modules: Vec::new(),
        rules: Vec::new(),
        tables: crate::content_tables::TABLES.to_vec(),
        slots: crate::world_ext::SLOTS.to_vec(),
        kinds: crate::delta_kinds::KINDS.to_vec(),
        stages: crate::tick::STAGES.to_vec(),
        services: crate::service::DOMAINS.to_vec(),
        commands: slash::COMMANDS.to_vec(),
        kill_player: kill_hooks::PLAYER.to_vec(),
        kill_npc: kill_hooks::NPC.to_vec(),
        on_login: crate::session_hooks::ON_LOGIN.to_vec(),
        on_logoff: crate::session_hooks::ON_LOGOFF.to_vec(),
        mission: mission_hooks::HOOKS.to_vec(),
        rollover: crate::rollover_steps::STEPS.to_vec(),
        slim: slim_extra::SOURCES.to_vec(),
        scan: scan_sources::SOURCES.to_vec(),
        warp_guards: movement_guard::WARP_GUARDS.to_vec(),
        jump_guards: movement_guard::JUMP_GUARDS.to_vec(),
        flags: Vec::new(),
        world_loaded: Vec::new(),
        crates: crate::crate_hooks::CRATES.to_vec(),
        mined: crate::mining_hooks::MINED.to_vec(),
        settled: crate::settle_hooks::SETTLED.to_vec(),
    };
    // What the core already owns; a module row may not repeat any of it.
    let mut names = BTreeSet::new();
    let mut tables: BTreeSet<&str> = a.tables.iter().map(|t| t.name).collect();
    let mut slots: BTreeSet<&str> = a.slots.iter().map(|s| s.name).collect();
    let mut kinds: BTreeSet<&str> = a.kinds.iter().map(|k| k.0).collect();
    let mut stages: BTreeSet<&str> = a.stages.iter().map(|s| s.name).collect();
    let mut services: BTreeSet<&str> = a
        .services
        .iter()
        .flat_map(|d| d.0.iter().copied())
        .chain(
            crate::service::Registry::OBJECT_RETURNING
                .iter()
                .map(|r| r.0),
        )
        .collect();
    let mut commands: BTreeSet<&str> = a.commands.iter().map(|c| c.name).collect();
    let mut kill: BTreeSet<&str> = a
        .kill_player
        .iter()
        .chain(&a.kill_npc)
        .map(|r| r.name)
        .collect();
    let mut login: BTreeSet<&str> = a.on_login.iter().map(|r| r.name).collect();
    let mut logoff: BTreeSet<&str> = a.on_logoff.iter().map(|r| r.name).collect();
    let mut steps: BTreeSet<&str> = a.rollover.iter().map(|r| r.name).collect();
    let mut crate_rows: BTreeSet<&str> = a.crates.iter().map(|r| r.name).collect();
    let mut mined_rows: BTreeSet<&str> = a.mined.iter().map(|r| r.name).collect();
    let mut settled_rows: BTreeSet<&str> = a.settled.iter().map(|r| r.name).collect();
    let mut placed: BTreeSet<&str> = BTreeSet::new();
    let mut flags: BTreeSet<&str> = crate::cli::CORE_FLAGS.iter().copied().collect();

    for m in mods {
        if m.name.is_empty() {
            if m.is_empty() {
                continue;
            }
            return Err("a module with rows has no name".to_owned());
        }
        if !names.insert(m.name) {
            return Err(format!("module {:?} is listed twice", m.name));
        }
        let n = m.name;
        a.modules.push((n, m.version));
        a.rules.extend(m.rules);
        add_unique(n, "table", &mut a.tables, &mut tables, m.tables, |t| t.name)?;
        add_unique(n, "slot", &mut a.slots, &mut slots, m.slots, |s| s.name)?;
        add_unique(n, "delta kind", &mut a.kinds, &mut kinds, m.kinds, |k| k.0)?;
        for d in m.services {
            for s in d.0 {
                if !services.insert(s) {
                    return Err(format!("module {n:?}: duplicate service {s:?}"));
                }
            }
        }
        a.services.extend(m.services);
        add_unique(
            n,
            "command",
            &mut a.commands,
            &mut commands,
            m.commands,
            |c| c.name,
        )?;
        add_unique(
            n,
            "kill hook",
            &mut a.kill_player,
            &mut kill,
            m.kill_player,
            |r| r.name,
        )?;
        add_unique(
            n,
            "kill hook",
            &mut a.kill_npc,
            &mut kill,
            m.kill_npc,
            |r| r.name,
        )?;
        add_unique(
            n,
            "login hook",
            &mut a.on_login,
            &mut login,
            m.on_login,
            |r| r.name,
        )?;
        add_unique(
            n,
            "logoff hook",
            &mut a.on_logoff,
            &mut logoff,
            m.on_logoff,
            |r| r.name,
        )?;
        add_unique(
            n,
            "rollover step",
            &mut a.rollover,
            &mut steps,
            m.rollover,
            |r| r.name,
        )?;
        add_unique(
            n,
            "crate row",
            &mut a.crates,
            &mut crate_rows,
            m.crates,
            |r| r.name,
        )?;
        add_unique(
            n,
            "mined row",
            &mut a.mined,
            &mut mined_rows,
            m.mined,
            |r| r.name,
        )?;
        add_unique(
            n,
            "settled row",
            &mut a.settled,
            &mut settled_rows,
            m.settled,
            |r| r.name,
        )?;
        add_unique(n, "flag", &mut a.flags, &mut flags, m.cli, |f| f.name)?;
        if !m.tables.is_empty() || !m.slots.is_empty() {
            // A table or slot whose value type another one already makes
            // would leave one unreachable. Built from this plan's own tables
            // (not `plan()`, which is not fixed yet).
            let content = crate::content::Content::try_fixture(&a.tables)
                .map_err(|e| format!("module {n:?}: {e}"))?;
            crate::world_ext::Slots::try_build(&a.slots, &content)
                .map_err(|e| format!("module {n:?}: {e}"))?;
        }
        a.world_loaded.extend(m.world_loaded);
        a.mission.extend(m.mission);
        a.slim.extend(m.slim);
        a.scan.extend(m.scan);
        a.warp_guards.extend(m.warp_guards);
        a.jump_guards.extend(m.jump_guards);
        for p in m.stages {
            if !stages.insert(p.stage.name) {
                return Err(format!("module {n:?}: duplicate stage {:?}", p.stage.name));
            }
            let Some(at) = a.stages.iter().position(|s| s.name == p.after) else {
                return Err(format!(
                    "module {n:?}: stage {:?} runs after {:?}, which is no stage",
                    p.stage.name, p.after
                ));
            };
            // After the anchor and after any module stage already placed
            // there, so stages run in list order, within and across modules.
            let mut at = at + 1;
            while a.stages.get(at).is_some_and(|s| placed.contains(s.name)) {
                at += 1;
            }
            placed.insert(p.stage.name);
            a.stages.insert(at, p.stage);
        }
    }
    Ok(a)
}

static PLAN: OnceLock<Assembled> = OnceLock::new();

/// Fix the process's plan to `mods`, once. Refuses a bad module list and a
/// second install; also refuses when [`plan`] has already been read, since
/// that fixed the core-only plan.
pub fn install(mods: &[&Module]) -> Result<&'static Assembled, String> {
    let a = build(mods)?;
    PLAN.set(a)
        .map_err(|_| "the module plan is already fixed".to_owned())?;
    Ok(plan())
}

/// The process's plan: what [`install`] fixed, else the core alone.
pub fn plan() -> &'static Assembled {
    PLAN.get_or_init(|| build(&[]).expect("the core tables assemble"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_dispatch(
        _: &mut crate::service::Registry,
        _: &crate::service::Request<'_>,
    ) -> Result<eve_marshal::PyValue, crate::service::ServiceError> {
        Ok(eve_marshal::PyValue::None)
    }
    fn noop_run(
        _: &mut crate::service::Registry,
        _: Option<i64>,
        _: &[&str],
    ) -> Result<eve_marshal::PyValue, crate::service::ServiceError> {
        Ok(eve_marshal::PyValue::None)
    }
    fn noop_stage(
        _: &mut crate::service::Registry,
        _: std::time::Instant,
        _: crate::damage::BlueTime,
    ) {
    }

    const FIRST: &str = "liveness";

    fn refused(m: Module) -> String {
        match build(&[&m]) {
            Err(e) => e,
            Ok(_) => panic!("the module was accepted"),
        }
    }

    /// Names of the core's first stage, command, service and kind, so the
    /// refusals below do not hard-code a row that may be renamed.
    fn core_command() -> &'static str {
        slash::COMMANDS[0].name
    }

    #[test]
    fn none_is_the_core() {
        let a = build(&[&Module::NONE, &Module::NONE]).unwrap();
        let names = |s: &[Stage]| s.iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names(&a.stages), names(crate::tick::STAGES));
        assert_eq!(a.tables.len(), crate::content_tables::TABLES.len());
        assert_eq!(a.slots.len(), crate::world_ext::SLOTS.len());
        assert_eq!(a.kinds, crate::delta_kinds::KINDS);
        assert_eq!(a.services.len(), crate::service::DOMAINS.len());
        assert_eq!(a.commands.len(), slash::COMMANDS.len());
        assert_eq!(a.kill_player.len(), kill_hooks::PLAYER.len());
        assert_eq!(a.kill_npc.len(), kill_hooks::NPC.len());
        assert_eq!(a.on_login.len(), crate::session_hooks::ON_LOGIN.len());
        assert_eq!(a.on_logoff.len(), crate::session_hooks::ON_LOGOFF.len());
        assert_eq!(a.mission.len(), mission_hooks::HOOKS.len());
        assert_eq!(a.rollover.len(), crate::rollover_steps::STEPS.len());
        assert_eq!(a.slim.len(), slim_extra::SOURCES.len());
        assert_eq!(a.scan.len(), scan_sources::SOURCES.len());
        assert_eq!(a.warp_guards.len(), movement_guard::WARP_GUARDS.len());
        assert_eq!(a.jump_guards.len(), movement_guard::JUMP_GUARDS.len());
        assert!(
            a.modules.is_empty()
                && a.flags.is_empty()
                && a.rules.is_empty()
                && a.world_loaded.is_empty()
                && a.crates.is_empty()
                && a.mined.is_empty()
                && a.settled.is_empty()
        );
        let mut r = Rules::default();
        a.apply_rules(&mut r);
        assert_eq!(r, Rules::default());
    }

    #[test]
    fn the_default_plan_is_the_core() {
        assert_eq!(plan().stages.len(), crate::tick::STAGES.len());
        assert_eq!(plan().commands.len(), slash::COMMANDS.len());
    }

    #[test]
    fn a_module_appends_after_the_core_and_in_place() {
        const SVC: &[&str] = &["testModSvc"];
        const CMD: &[Command] = &[Command {
            name: "testmodcmd",
            usage: "/testmodcmd",
            run: noop_run,
        }];
        const STAGE: &[Placed] = &[Placed {
            stage: Stage {
                name: "testmod_stage",
                run: noop_stage,
                every: 1,
                phase: 0,
            },
            after: FIRST,
        }];
        const KIND: &[(&str, &str)] = &[("testmodkind", "testmod")];
        let m = Module {
            name: "testmod",
            version: 3,
            services: &[(SVC, noop_dispatch)],
            commands: CMD,
            stages: STAGE,
            kinds: KIND,
            ..Module::NONE
        };
        let a = build(&[&m]).unwrap();
        assert_eq!(a.modules, [("testmod", 3)]);
        assert_eq!(a.services.last().unwrap().0, SVC);
        assert_eq!(a.commands.last().unwrap().name, "testmodcmd");
        assert_eq!(a.kinds.last().unwrap().0, "testmodkind");
        let at = a.stages.iter().position(|s| s.name == FIRST).unwrap();
        assert_eq!(a.stages[at + 1].name, "testmod_stage");
        assert_eq!(a.stages.len(), crate::tick::STAGES.len() + 1);
    }

    #[test]
    fn refuses_a_duplicate_service() {
        let core = crate::service::DOMAINS[0].0;
        let rows: &'static [Domain] =
            Box::leak(Box::new([(core, noop_dispatch as DomainDispatch)]));
        let e = refused(Module {
            name: "dup",
            services: rows,
            ..Module::NONE
        });
        assert!(
            e.contains("\"dup\"") && e.contains("duplicate service"),
            "{e}"
        );
        // Two modules, one service.
        const SVC: &[&str] = &["testTwiceSvc"];
        let m = Module {
            name: "a",
            services: &[(SVC, noop_dispatch)],
            ..Module::NONE
        };
        let m2 = Module { name: "b", ..m };
        let e = build(&[&m, &m2]).err().unwrap();
        assert!(e.contains("\"b\"") && e.contains("testTwiceSvc"), "{e}");
    }

    #[test]
    fn refuses_a_duplicate_command() {
        let core = core_command();
        let cmds: &'static [Command] = Box::leak(Box::new([Command {
            name: core,
            usage: "",
            run: noop_run,
        }]));
        let e = refused(Module {
            name: "dup",
            commands: cmds,
            ..Module::NONE
        });
        assert!(e.contains("duplicate command") && e.contains(core), "{e}");
    }

    #[test]
    fn refuses_a_duplicate_kind() {
        let core = crate::delta_kinds::KINDS[0].0;
        let kinds: &'static [(&str, &str)] = Box::leak(Box::new([(core, "dup")]));
        let e = refused(Module {
            name: "dup",
            kinds,
            ..Module::NONE
        });
        assert!(
            e.contains("duplicate delta kind") && e.contains(core),
            "{e}"
        );
    }

    #[test]
    fn refuses_a_stage_after_nothing() {
        const STAGE: &[Placed] = &[Placed {
            stage: Stage {
                name: "lost",
                run: noop_stage,
                every: 1,
                phase: 0,
            },
            after: "no_such_stage",
        }];
        let e = refused(Module {
            name: "lost",
            stages: STAGE,
            ..Module::NONE
        });
        assert!(e.contains("no_such_stage") && e.contains("no stage"), "{e}");
    }

    #[test]
    fn refuses_a_duplicate_stage_slot_and_a_nameless_module() {
        let stages: &'static [Placed] = Box::leak(Box::new([Placed {
            stage: crate::tick::STAGES[0],
            after: FIRST,
        }]));
        let e = refused(Module {
            name: "dup",
            stages,
            ..Module::NONE
        });
        assert!(e.contains("duplicate stage"), "{e}");
        let slots: &'static [SlotSpec] = Box::leak(Box::new([crate::world_ext::SLOTS[0]]));
        let e = refused(Module {
            name: "dup",
            slots,
            ..Module::NONE
        });
        assert!(e.contains("duplicate slot"), "{e}");
        let kinds: &'static [(&str, &str)] = &[("x", "x")];
        let e = refused(Module {
            kinds,
            ..Module::NONE
        });
        assert!(e.contains("no name"), "{e}");
        let a = Module {
            name: "twice",
            ..Module::NONE
        };
        assert!(build(&[&a, &a]).err().unwrap().contains("listed twice"));
    }

    #[test]
    fn rules_and_flags_reach_the_plan() {
        fn boost(r: &mut Rules) {
            r.training_mult = 8.0;
        }
        fn set(r: &mut Rules, v: Option<&str>) -> Result<(), String> {
            r.start_isk = v
                .ok_or("needs a value")?
                .parse()
                .map_err(|_| "not a number")?;
            Ok(())
        }
        const FLAGS: &[Flag] = &[Flag {
            name: "--test-start-isk",
            takes_value: true,
            usage: "",
            apply: set,
        }];
        let a = build(&[&Module {
            name: "r",
            rules: Some(boost),
            cli: FLAGS,
            ..Module::NONE
        }])
        .unwrap();
        let mut r = Rules::default();
        a.apply_rules(&mut r);
        assert_eq!(r.training_mult, 8.0);
        (a.flag("--test-start-isk").unwrap().apply)(&mut r, Some("5")).unwrap();
        assert_eq!(r.start_isk, 5.0);
        assert!(a.flag("--nope").is_none());
        // A core flag cannot be shadowed.
        let dup: &'static [Flag] = Box::leak(Box::new([Flag {
            name: "--bind",
            takes_value: true,
            usage: "",
            apply: set,
        }]));
        let e = refused(Module {
            name: "f",
            cli: dup,
            ..Module::NONE
        });
        assert!(e.contains("duplicate flag"), "{e}");
    }

    fn leak<T>(v: T) -> &'static [T] {
        Box::leak(Box::new([v]))
    }
    fn noop_kill(_: &mut crate::service::Registry, _: &KillEvent) {}
    fn noop_session(_: &mut crate::service::Registry, _: i64) {}

    #[test]
    fn refuses_a_duplicate_table_hook_and_step() {
        let t = crate::content_tables::TABLES[0];
        let e = refused(Module {
            name: "dup",
            tables: leak(t),
            ..Module::NONE
        });
        assert!(e.contains("duplicate table") && e.contains(t.name), "{e}");

        let k = KillRow {
            name: kill_hooks::PLAYER[0].name,
            run: noop_kill,
        };
        let e = refused(Module {
            name: "dup",
            kill_player: leak(k),
            ..Module::NONE
        });
        assert!(e.contains("duplicate kill hook"), "{e}");
        let k = KillRow {
            name: kill_hooks::NPC[0].name,
            run: noop_kill,
        };
        let e = refused(Module {
            name: "dup",
            kill_npc: leak(k),
            ..Module::NONE
        });
        assert!(e.contains("duplicate kill hook"), "{e}");

        let r = SessionRow {
            name: crate::session_hooks::ON_LOGIN[0].name,
            run: noop_session,
        };
        let e = refused(Module {
            name: "dup",
            on_login: leak(r),
            ..Module::NONE
        });
        assert!(e.contains("duplicate login hook"), "{e}");
        let r = SessionRow {
            name: crate::session_hooks::ON_LOGOFF[0].name,
            run: noop_session,
        };
        let e = refused(Module {
            name: "dup",
            on_logoff: leak(r),
            ..Module::NONE
        });
        assert!(e.contains("duplicate logoff hook"), "{e}");

        let st = crate::rollover_steps::STEPS[0];
        let e = refused(Module {
            name: "dup",
            rollover: leak(st),
            ..Module::NONE
        });
        assert!(
            e.contains("duplicate rollover step") && e.contains(st.name),
            "{e}"
        );
    }

    fn five() -> Box<dyn std::any::Any + Send + Sync> {
        Box::new(5u8)
    }
    fn make_five(_: &crate::content::Content) -> crate::content::Boxed {
        Box::new(5u8)
    }

    /// Two slots, or two tables, that make one value type are refused by
    /// `build` (so by `install`), naming the module, not by a panic at the
    /// first `Registry::new`.
    #[test]
    fn refuses_a_slot_or_table_type_clash_at_startup() {
        let slots: &'static [SlotSpec] = Box::leak(Box::new([
            SlotSpec {
                name: "five_a",
                make: make_five,
            },
            SlotSpec {
                name: "five_b",
                make: make_five,
            },
        ]));
        let e = refused(Module {
            name: "clash",
            slots,
            ..Module::NONE
        });
        assert!(
            e.contains("\"clash\"") && e.contains("five_a") && e.contains("five_b"),
            "{e}"
        );

        fn load(_: &crate::content::Sources) -> Result<crate::content::Boxed, String> {
            Ok(Box::new(5u8))
        }
        let spec = |name| TableSpec {
            name,
            load,
            fixture: five,
        };
        let tables: &'static [TableSpec] = Box::leak(Box::new([spec("five_a"), spec("five_b")]));
        let e = refused(Module {
            name: "clash",
            tables,
            ..Module::NONE
        });
        assert!(e.contains("\"clash\"") && e.contains("same type"), "{e}");

        // `install` refuses the same list and leaves the plan unfixed.
        assert!(
            install(&[&Module {
                name: "clash",
                tables,
                ..Module::NONE
            }])
            .is_err()
        );
        assert!(PLAN.get().is_none() || PLAN.get().unwrap().modules.is_empty());
    }

    /// Stages placed after one anchor run in list order, within a module
    /// and across modules.
    #[test]
    fn placed_stages_run_in_list_order() {
        let st = |name| Placed {
            stage: Stage {
                name,
                run: noop_stage,
                every: 1,
                phase: 0,
            },
            after: FIRST,
        };
        let one: &'static [Placed] = Box::leak(Box::new([st("s1"), st("s2")]));
        let two: &'static [Placed] = Box::leak(Box::new([st("s3"), st("s4")]));
        let a = build(&[
            &Module {
                name: "m1",
                stages: one,
                ..Module::NONE
            },
            &Module {
                name: "m2",
                stages: two,
                ..Module::NONE
            },
        ])
        .unwrap();
        let at = a.stages.iter().position(|s| s.name == FIRST).unwrap();
        let got: Vec<&str> = a.stages[at + 1..at + 5].iter().map(|s| s.name).collect();
        assert_eq!(got, ["s1", "s2", "s3", "s4"]);
        assert_eq!(a.stages[at + 5].name, crate::tick::STAGES[1].name);
    }
}
