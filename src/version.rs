use regex::Regex;
use serde::Deserialize;
use std::{cmp::Ordering, fmt, sync::LazyLock};

use crate::{
    cli::{Args, Commands},
    error::VersionerError,
    project::Project,
};

static SEMVER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)\.(\d+)\.(\d+)$").unwrap());

#[derive(Deserialize)]
pub(crate) struct Wrapper {
    version: String,
}

#[derive(Clone, PartialEq)]
pub(crate) struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    pub(crate) fn parse(package: &Project) -> Self {
        let wrapper = serde_json::from_str::<Wrapper>(&package.json.to_string())
            .unwrap_or_else(|_| VersionerError::InvalidVersion.fatal());

        let captures = SEMVER
            .captures(&wrapper.version)
            .unwrap_or_else(|| VersionerError::InvalidVersion.fatal());

        Self {
            major: captures[1]
                .parse()
                .unwrap_or_else(|_| VersionerError::InvalidVersion.fatal()),
            minor: captures[2]
                .parse()
                .unwrap_or_else(|_| VersionerError::InvalidVersion.fatal()),
            patch: captures[3]
                .parse()
                .unwrap_or_else(|_| VersionerError::InvalidVersion.fatal()),
        }
    }

    pub(crate) fn bump(&self, args: &Args) -> Self {
        let mut next = self.clone();

        match args.command {
            Commands::Major { message: _ } => next.major += 1,
            Commands::Minor { message: _ } => next.minor += 1,
            Commands::Patch { message: _ } => next.patch += 1,
        }

        next
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(
            self.major
                .cmp(&other.major)
                .then(self.minor.cmp(&other.minor))
                .then(self.patch.cmp(&other.patch)),
        )
    }
}
