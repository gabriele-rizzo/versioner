use regex::Regex;
use std::{fmt, sync::LazyLock};

use crate::{cli::Commands, error::VersionerError};

static SEMVER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)\.(\d+)\.(\d+)$").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    pub(crate) fn parse(version: &str) -> Self {
        let captures = SEMVER
            .captures(version)
            .unwrap_or_else(|| VersionerError::InvalidVersion.fatal());

        let component = |index: usize| {
            captures[index]
                .parse()
                .unwrap_or_else(|_| VersionerError::InvalidVersion.fatal())
        };

        Self {
            major: component(1),
            minor: component(2),
            patch: component(3),
        }
    }

    pub(crate) fn bump(&self, command: &Commands) -> Self {
        match command {
            Commands::Major { .. } => Self {
                major: self.major + 1,
                minor: 0,
                patch: 0,
            },
            Commands::Minor { .. } => Self {
                minor: self.minor + 1,
                patch: 0,
                ..*self
            },
            Commands::Patch { .. } => Self {
                patch: self.patch + 1,
                ..*self
            },
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(major: u32, minor: u32, patch: u32) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    fn command(kind: &str) -> Commands {
        let message = "chore: bump".to_owned();

        match kind {
            "major" => Commands::Major { message },
            "minor" => Commands::Minor { message },
            _ => Commands::Patch { message },
        }
    }

    #[test]
    fn parses_a_semver_string() {
        assert_eq!(Version::parse("10.20.30"), version(10, 20, 30));
        assert_eq!(Version::parse("0.0.0"), version(0, 0, 0));
    }

    #[test]
    fn major_bump_resets_minor_and_patch() {
        assert_eq!(version(1, 2, 3).bump(&command("major")), version(2, 0, 0));
    }

    #[test]
    fn minor_bump_resets_patch_and_keeps_major() {
        assert_eq!(version(1, 2, 3).bump(&command("minor")), version(1, 3, 0));
    }

    #[test]
    fn patch_bump_keeps_major_and_minor() {
        assert_eq!(version(1, 2, 3).bump(&command("patch")), version(1, 2, 4));
    }

    #[test]
    fn a_bump_always_moves_forward() {
        let current = version(1, 2, 3);

        for kind in ["major", "minor", "patch"] {
            assert!(current < current.bump(&command(kind)));
        }
    }

    #[test]
    fn orders_by_component_not_lexically() {
        assert!(version(1, 2, 3) < version(1, 10, 0));
        assert!(version(2, 0, 0) > version(1, 99, 99));
    }

    #[test]
    fn displays_as_dotted_components() {
        assert_eq!(version(1, 20, 3).to_string(), "1.20.3");
    }
}
