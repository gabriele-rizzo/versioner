use crate::log;

#[derive(Debug)]
pub(crate) enum VersionerError {
    NotARepository,
    ManifestNotFound,
    UnsupportedManifest(String),
    OutsideRepository(String),
    AmbiguousManifest(Vec<String>),
    InvalidManifest(String, String),
    MissingVersion(String),
    UnsupportedVersion(String, String),
    InvalidVersion(String),
    InvalidPreId(String),
    VersionOverflow,
    Backwards(String, String),
    /// The scanner's span disagrees with a real parse of the file.
    Unlocatable(String),
    TagExists(String),
    SnapshotFailed(String),
    SaveFailed(String, String),
    CommitFailed(String),
    TagFailed(String),
}

impl VersionerError {
    pub(crate) fn fatal(&self) -> ! {
        let message = match self {
            Self::NotARepository => "not a git repository".to_owned(),
            Self::ManifestNotFound => {
                "no package.json, Cargo.toml or pyproject.toml found between \
                                       the current directory and the repository root"
                    .to_owned()
            }
            Self::UnsupportedManifest(path) => format!(
                "'{path}' is not a supported manifest, expected package.json, Cargo.toml or pyproject.toml"
            ),
            Self::OutsideRepository(path) => format!("'{path}' is outside the repository"),
            Self::AmbiguousManifest(labels) => format!(
                "found several versioned manifests ({}), pick one with --manifest",
                labels.join(", ")
            ),
            Self::InvalidManifest(label, reason) => format!("invalid {label}: {reason}"),
            Self::MissingVersion(label) => format!("{label} has no version"),
            Self::UnsupportedVersion(label, reason) => format!("{label}: {reason}"),
            Self::InvalidVersion(version) => {
                format!("invalid version '{version}', expected semver: x.y.z[-pre][+build]")
            }
            Self::InvalidPreId(id) => format!(
                "invalid pre-release identifier '{id}', expected dot-separated [0-9A-Za-z-] parts"
            ),
            Self::VersionOverflow => "version component would overflow".to_owned(),
            Self::Backwards(current, next) => {
                format!("{current} → {next} would move the version backwards")
            }
            Self::Unlocatable(label) => {
                format!("could not pin down the version text in {label} to rewrite it in place")
            }
            Self::TagExists(tag) => format!("tag '{tag}' already exists"),
            Self::SnapshotFailed(reason) => format!("could not snapshot the index: {reason}"),
            Self::SaveFailed(label, reason) => {
                format!("failed to write {label}: {reason} — changes rolled back")
            }
            Self::CommitFailed(reason) | Self::TagFailed(reason) => {
                format!("{reason} — changes rolled back")
            }
        };

        log::fatal(&message)
    }
}
