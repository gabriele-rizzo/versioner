use crate::{log, version::Version};

pub(crate) enum VersionerError {
    PackageNotFound,
    InvalidPackage,
    InvalidVersion,
    TagExists(String),
    NoAction(Version),
    SaveFailed,
}

impl VersionerError {
    pub(crate) fn fatal(&self) -> ! {
        let message = match self {
            Self::PackageNotFound => "package.json file not found in the current directory",
            Self::InvalidPackage => "invalid package.json file",
            Self::InvalidVersion => "invalid version format, expected: x.x.x",
            Self::TagExists(tag) => &format!("tag '{}' already exists", tag),
            Self::NoAction(version) => &format!(
                "already on version {}, nothing to bump",
                version.to_string()
            ),
            Self::SaveFailed => "failed to update package.json",
        };

        log::fatal(&message);
    }
}
