//! `mdquiz` command-line entry point.

use clap::Parser;

mod cli;

/// Parse the command line and run it, reporting any error to the user.
fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    cli::run(args)
}
