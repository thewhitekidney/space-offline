//! **The owner's server**: the core `eve-server` plus the modules this
//! distribution adds (PR-SEAM-1, `docs/lanes/pr-seam-1.md`). It cannot be
//! `eve-server`'s own bin: a module crate depends on `eve-server`, and a
//! package cannot depend on a crate that depends on it. The core bin
//! (`eve-server/src/bin/eve-server.rs`) stays module-free.

fn main() -> std::process::ExitCode {
    eve_server::cli::run(&[&eve_mod_econ::MODULE, &eve_mod_world::MODULE])
}
