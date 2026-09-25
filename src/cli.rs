use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{
    error::VersionerError,
    version::{Bump, Identifier, Level},
};

/// Bump the version in package.json, Cargo.toml or pyproject.toml, then
/// commit, tag and push it.
#[derive(Parser)]
#[command(author = "Gabriele Rizzo", version, long_about = None)]
pub(crate) struct Args {
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Show what would change without writing, committing or pushing anything
    #[arg(long, global = true)]
    pub(crate) dry_run: bool,

    /// Commit and tag locally, but don't push
    #[arg(long, global = true)]
    pub(crate) no_push: bool,

    /// Commit every change in the worktree, not just the version files
    #[arg(long, global = true)]
    pub(crate) all: bool,

    /// Manifest to bump, instead of the nearest one above the current directory
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) manifest: Option<PathBuf>,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum Commands {
    /// 1.4.2 → 2.0.0, or 2.0.0-rc.1 → 2.0.0
    Major(Release),
    /// 1.4.2 → 1.5.0, or 1.5.0-rc.1 → 1.5.0
    Minor(Release),
    /// 1.4.2 → 1.4.3, or 1.4.3-rc.1 → 1.4.3
    Patch(Release),
    /// Next pre-release: 1.5.0-beta.0 → 1.5.0-beta.1, or 1.4.2 → 1.4.3-<id>.0
    Pre {
        /// Commit and tag message
        message: String,

        /// Pre-release identifier; a different one than the current starts
        /// over at .0 (beta.3 → rc.0)
        #[arg(long)]
        id: Option<String>,
    },
}

#[derive(clap::Args, Debug, Clone)]
pub(crate) struct Release {
    /// Commit and tag message
    message: String,

    /// Open a pre-release of the bumped version: `minor --pre beta` gives 1.5.0-beta.0
    #[arg(long, value_name = "ID")]
    pre: Option<String>,
}

impl Commands {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Major(release) | Self::Minor(release) | Self::Patch(release) => &release.message,
            Self::Pre { message, .. } => message,
        }
    }

    pub(crate) fn bump(&self) -> Result<Bump, VersionerError> {
        let (level, release) = match self {
            Self::Major(release) => (Level::Major, release),
            Self::Minor(release) => (Level::Minor, release),
            Self::Patch(release) => (Level::Patch, release),
            Self::Pre { id, .. } => {
                return Ok(Bump::Next(
                    id.as_deref().map(Identifier::parse_all).transpose()?,
                ));
            }
        };

        Ok(match &release.pre {
            Some(id) => Bump::Start(level, Identifier::parse_all(id)?),
            None => Bump::Release(level),
        })
    }
}
