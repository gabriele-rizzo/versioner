use crate::log;

pub(crate) enum VersionerError {
    PackageNotFound,
    InvalidPackage,
    InvalidVersion,
    NotARepository,
    TagExists(String),
    SaveFailed,
    CommitFailed(String),
}

impl VersionerError {
    pub(crate) fn fatal(&self) -> ! {
        let message = match self {
            Self::PackageNotFound => {
                "package.json file not found in the current directory".to_owned()
            }
            Self::InvalidPackage => "invalid package.json file".to_owned(),
            Self::InvalidVersion => "invalid version format, expected: x.x.x".to_owned(),
            Self::NotARepository => "not a git repository".to_owned(),
            Self::TagExists(tag) => format!("tag '{tag}' already exists"),
            Self::SaveFailed => "failed to update package.json".to_owned(),
            Self::CommitFailed(reason) => format!("{reason} — package.json restored"),
        };

        log::fatal(&message)
    }
}
