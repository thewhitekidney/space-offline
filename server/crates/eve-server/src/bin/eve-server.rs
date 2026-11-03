//! The core `eve-server` binary: no modules. The command line and the run
//! loop are `eve_server::cli::run` (PR-SEAM-1); a distribution that adds
//! modules is another binary calling it with them.

fn main() -> std::process::ExitCode {
    eve_server::cli::run(&[])
}
