use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::error::VersionerError;

/// One file's planned rewrite, holding both versions of its contents so a
/// failed release can put it back exactly.
pub(crate) struct Edit {
    pub(crate) path: PathBuf,
    /// Repo relative path, for logs and git pathspecs.
    pub(crate) label: String,
    /// The version text being replaced; a lockfile can lag its manifest.
    pub(crate) from: String,
    pub(crate) original: String,
    pub(crate) updated: String,
}

impl Edit {
    /// Writes every edit, or none: a failure restores the ones already written.
    pub(crate) fn apply(edits: &[Edit]) -> Result<(), VersionerError> {
        for (index, edit) in edits.iter().enumerate() {
            if let Err(error) = fs::write(&edit.path, &edit.updated) {
                Edit::restore(&edits[..=index]);

                return Err(VersionerError::SaveFailed(
                    edit.label.clone(),
                    error.to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Puts the original contents back, returning the labels it couldn't.
    pub(crate) fn restore(edits: &[Edit]) -> Vec<String> {
        edits
            .iter()
            .filter(|edit| fs::write(&edit.path, &edit.original).is_err())
            .map(|edit| edit.label.clone())
            .collect()
    }
}

/// `path` relative to the repository root, `/`-separated as git reports it.
pub(crate) fn label(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);

    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
