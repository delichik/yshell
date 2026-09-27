//! YShell workspace build tool.
//!
//! Every build, check, test and packaging task the project needs - locally and
//! in CI - goes through this binary:
//!
//! ```text
//! cargo xtask <command>
//! ```
//!
//! The tool is plain Rust so it behaves identically on Windows, macOS and
//! Linux; `.github/workflows` only sets up the toolchain and then calls these
//! commands, which keeps local development and CI in sync.

mod cli;
mod context;
mod dist;
mod installers;
mod tasks;

use std::process::ExitCode;

use anyhow::Result;

use crate::cli::Command;
use crate::context::Context;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let command = cli::parse(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let ctx = Context::discover()?;
    match command {
        Command::Help => {
            print!("{}", cli::USAGE);
            Ok(())
        }
        Command::Version => {
            println!("xtask {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Fmt => tasks::fmt(&ctx),
        Command::Lint => tasks::lint(&ctx),
        Command::Check => tasks::check(&ctx),
        Command::Build(options) => tasks::build(&ctx, &options),
        Command::Test(options) => tasks::test(&ctx, &options),
        Command::Run(options) => tasks::run_app(&ctx, &options),
        Command::Deny => tasks::deny(&ctx),
        Command::Doctor => tasks::doctor(&ctx),
        Command::Clean(options) => tasks::clean(&ctx, &options),
        Command::Ci => tasks::ci(&ctx),
        Command::Dist(options) => dist::run(&ctx, &options),
        Command::Checksums(options) => dist::checksums(&ctx, &options),
    }
}
