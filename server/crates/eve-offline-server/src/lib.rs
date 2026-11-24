//! The owner's distribution as a library, so a second binary can start the
//! same server in its own process (`eve-bots --host`, #147): the module list
//! lives here once, and `main.rs` is a call into `eve_server::cli::run`.

use eve_server::module::Module;

/// The modules this distribution adds to the core server, in plan order.
pub const MODULES: &[&Module] = &[
    &eve_mod_econ::MODULE,
    &eve_mod_world::MODULE,
    &eve_server::air_npe::MODULE,
];
