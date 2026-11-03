//! **`eve-mod-world`: the world's settings**, as one module over the core
//! (`docs/lanes/pr-seam-1.md`, issue #70). The owner chooses how fast skills
//! train, and how much ISK and how many unallocated skill points a new
//! character starts with. What the numbers do stays in `eve-server`
//! ([`eve_server::rules`]); this module only stores them and changes them.
//!
//! - [`settings`]: the table of settings, each a name, a reader and a writer
//!   on [`eve_server::rules::Rules`]; the startup flags; the `/world` command.
//!
//! **Where a value lives.** A new world seeds its settings from the flags
//! (`--world-training`, `--world-start-isk`, `--world-start-sp`) and stores
//! one row of [`KIND`] per setting. After that the stored row wins, and a flag
//! that differs only draws a log line pointing at `/world`. So an existing
//! world's live settings are changed with `/world <name> <value>`, which a GM account may run
//! (the slash console's own gate), and the change is stored at once.
//!
//! **Two kinds.** Each setting is [`settings::Kind::Live`] or
//! [`settings::Kind::FirstLaunch`]. `training` is live. `usage: "/world [training <value>]"` and
//! `startsp` are first-launch: the flags seed them when the world is made, the
//! stored row wins on reload, and `/world <name> <value>` refuses them with a
//! reply (`/world` still lists them). A later setting declares its kind.
//!
//! **A live training change** goes through `rules::set_live`, which freezes
//! every running queue at the old rate and restarts it at the new one, so time
//! already trained is not re-priced.
//!
//! The module is installed by `eve-offline-server`; the core `eve-server`
//! binary has no modules.

pub mod settings;

use eve_server::module::Module;

/// The module's delta kind: one row per setting, key `[name]`, value
/// `[number]`. Owned by this module.
pub const KIND: &str = "worldset";

/// The module's own world-format number (`Module::version`): bump it when the
/// rows of [`KIND`] change shape.
pub const VERSION: u32 = 1;

/// The module.
pub const MODULE: Module = Module {
    name: "world",
    version: VERSION,
    kinds: &[(KIND, "world")],
    commands: &[eve_server::module::Command {
        name: "world",
        usage: "/world [training|startisk|startsp|anchortime|deletetime <value>]",
        run: settings::slash,
    }],
    cli: settings::FLAGS,
    world_loaded: &[settings::startup],
    ..Module::NONE
};
