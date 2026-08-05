use clap::Parser;

use crate::{cli::Args, project::Project};

mod cli;
mod error;
mod git;
mod log;
mod project;
mod version;

fn main() {
    let args = Args::parse();
    let mut project = Project::parse();

    project.update(args)
}
