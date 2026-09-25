use clap::Parser;

use crate::cli::Args;

mod cli;
mod edit;
mod error;
mod git;
mod lockfile;
mod log;
mod manifest;
mod project;
mod release;
mod scan;
mod version;

fn main() {
    release::run(Args::parse())
}
