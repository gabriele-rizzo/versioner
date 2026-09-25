use std::path::{Path, PathBuf};

use crate::{
    edit::{Edit, label},
    error::VersionerError,
    git::Git,
    lockfile,
    manifest::{Format, Manifest},
    version::Version,
};

pub(crate) struct Project {
    pub(crate) manifest: Manifest,
    root: PathBuf,
    /// Tag namespace for a workspace package; `None` at the repository root,
    /// which keeps the bare `vX.Y.Z` tags.
    scope: Option<String>,
}

impl Project {
    /// The manifest at `explicit`, or else the nearest one at or above the
    /// current directory.
    pub(crate) fn locate(explicit: Option<&Path>) -> Result<Self, VersionerError> {
        let root = Git::repository_root()
            .map(|root| canonical(&root))
            .ok_or(VersionerError::NotARepository)?;

        let manifest = match explicit {
            Some(path) => {
                let path = canonical(path);

                if !path.starts_with(&root) {
                    return Err(VersionerError::OutsideRepository(
                        path.display().to_string(),
                    ));
                }

                Manifest::read(&path, &label(&root, &path))?
            }
            None => {
                let cwd = std::env::current_dir().map_err(|_| VersionerError::ManifestNotFound)?;

                find(&canonical(&cwd), &root)?
            }
        };

        let at_root = manifest.path.parent() == Some(root.as_path());
        let scope = (!at_root).then(|| tag_scope(manifest.name.as_deref(), &manifest.path));

        Ok(Self {
            manifest,
            root,
            scope,
        })
    }

    /// `v1.2.3` for the repository root, `backend-v1.2.3` for a workspace
    /// package, so sibling packages can't collide on one tag.
    pub(crate) fn tag(&self, version: &Version) -> String {
        match &self.scope {
            Some(scope) => format!("{scope}-v{version}"),
            None => format!("v{version}"),
        }
    }

    /// The manifest and lockfile rewrites for `next`, all verified up front so
    /// nothing is written unless every file can be edited cleanly.
    pub(crate) fn edits(&self, next: &Version) -> Result<Vec<Edit>, VersionerError> {
        let next = next.to_string();

        let mut edits = vec![Edit {
            path: self.manifest.path.clone(),
            label: self.manifest.label.clone(),
            from: self.manifest.version().to_owned(),
            original: self.manifest.source.clone(),
            updated: self.manifest.rewritten(&next)?,
        }];

        edits.extend(lockfile::edits(&self.manifest, &self.root, &next)?);

        Ok(edits)
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Nearest directory at or above `from` holding a manifest, never climbing
/// past the repository root so a bump can't reach a package outside the repo
/// it tags.
fn find(from: &Path, root: &Path) -> Result<Manifest, VersionerError> {
    for directory in from.ancestors() {
        if !directory.starts_with(root) {
            break;
        }

        let candidates: Vec<PathBuf> = Format::ALL
            .iter()
            .map(|format| directory.join(format.file_name()))
            .filter(|path| path.is_file())
            .collect();

        if !candidates.is_empty() {
            return choose(root, candidates);
        }
    }

    Err(VersionerError::ManifestNotFound)
}

/// With several manifests side by side, the one that carries a version wins:
/// a package.json that only holds tooling shouldn't shadow a Cargo.toml.
fn choose(root: &Path, candidates: Vec<PathBuf>) -> Result<Manifest, VersionerError> {
    let mut read: Vec<Result<Manifest, VersionerError>> = candidates
        .iter()
        .map(|path| Manifest::read(path, &label(root, path)))
        .collect();

    if read.len() == 1 {
        return read.remove(0);
    }

    let (versioned, failed): (Vec<_>, Vec<_>) = read.into_iter().partition(Result::is_ok);
    let mut versioned: Vec<Manifest> = versioned.into_iter().filter_map(Result::ok).collect();

    match versioned.len() {
        1 => Ok(versioned.remove(0)),
        0 => {
            let mut errors: Vec<VersionerError> =
                failed.into_iter().filter_map(Result::err).collect();

            // "has no version" is the least useful thing to report when another
            // manifest failed for a real reason.
            let position = errors
                .iter()
                .position(|error| !matches!(error, VersionerError::MissingVersion(_)))
                .unwrap_or(0);

            Err(errors.swap_remove(position))
        }
        _ => Err(VersionerError::AmbiguousManifest(
            versioned
                .into_iter()
                .map(|manifest| manifest.label)
                .collect(),
        )),
    }
}

/// Git-ref-safe tag namespace for a workspace package: the unscoped half of its
/// name, falling back to the directory it lives in.
fn tag_scope(name: Option<&str>, path: &Path) -> String {
    let name = slugify(
        name.unwrap_or_default()
            .rsplit('/')
            .next()
            .unwrap_or_default(),
    );

    if !name.is_empty() {
        return name;
    }

    let directory = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|directory| directory.to_str())
        .unwrap_or_default();

    match slugify(directory) {
        directory if directory.is_empty() => "package".to_owned(),
        directory => directory,
    }
}

/// Keeps only what `git check-ref-format` is happy with, and drops the `.lock`
/// suffix and leading punctuation git rejects outright.
fn slugify(name: &str) -> String {
    let kept: String = name
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => character,
            _ => '-',
        })
        .collect();

    let trimmed = kept.trim_matches(['.', '-']);

    trimmed.strip_suffix(".lock").unwrap_or(trimmed).to_owned()
}

#[cfg(test)]
mod tests {
    use super::{find, slugify, tag_scope};
    use crate::error::VersionerError;
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU32, Ordering},
    };

    /// A throwaway `root/backend/src` tree with manifests wherever `files`
    /// says, mirroring a workspace layout.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(files: &[(&str, &str)]) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);

            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("versioner-{}-{unique}", std::process::id()));

            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("backend/src")).unwrap();

            for (path, contents) in files {
                fs::write(root.join(path), contents).unwrap();
            }

            Self {
                root: fs::canonicalize(&root).unwrap(),
            }
        }

        fn found(&self, from: &str) -> Result<String, VersionerError> {
            find(&self.root.join(from), &self.root).map(|manifest| manifest.label)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    const NPM: &str = r#"{"version": "1.0.0"}"#;
    const CARGO: &str = "[package]\nname = \"core\"\nversion = \"1.0.0\"\n";

    #[test]
    fn finds_the_manifest_in_the_current_directory() {
        let tree = Tree::new(&[("package.json", NPM), ("backend/package.json", NPM)]);

        assert_eq!(tree.found(".").unwrap(), "package.json");
        assert_eq!(tree.found("backend").unwrap(), "backend/package.json");
    }

    #[test]
    fn walks_up_to_the_nearest_manifest() {
        let tree = Tree::new(&[("package.json", NPM), ("backend/Cargo.toml", CARGO)]);

        assert_eq!(tree.found("backend/src").unwrap(), "backend/Cargo.toml");
    }

    #[test]
    fn walks_past_directories_without_a_manifest() {
        let tree = Tree::new(&[("pyproject.toml", "[project]\nversion = \"1.0.0\"\n")]);

        assert_eq!(tree.found("backend/src").unwrap(), "pyproject.toml");
    }

    #[test]
    fn never_climbs_past_the_repository_root() {
        let tree = Tree::new(&[]);

        assert!(matches!(
            tree.found("backend/src"),
            Err(VersionerError::ManifestNotFound)
        ));
    }

    #[test]
    fn prefers_the_one_manifest_with_a_version() {
        let tree = Tree::new(&[
            ("package.json", r#"{"private": true}"#),
            ("Cargo.toml", CARGO),
        ]);

        assert_eq!(tree.found(".").unwrap(), "Cargo.toml");
    }

    #[test]
    fn refuses_to_guess_between_versioned_manifests() {
        let tree = Tree::new(&[("package.json", NPM), ("Cargo.toml", CARGO)]);

        let Err(VersionerError::AmbiguousManifest(labels)) = tree.found(".") else {
            panic!("expected an ambiguity error");
        };

        assert_eq!(labels, ["package.json", "Cargo.toml"]);
    }

    #[test]
    fn reports_the_most_useful_failure() {
        let inherited = "[package]\nname = \"a\"\nversion.workspace = true\n";
        let tree = Tree::new(&[("package.json", "{}"), ("Cargo.toml", inherited)]);

        assert!(matches!(
            tree.found("."),
            Err(VersionerError::UnsupportedVersion(..))
        ));
    }

    #[test]
    fn scopes_a_tag_by_the_unscoped_package_name() {
        let path = Path::new("/repo/backend/package.json");

        assert_eq!(tag_scope(Some("@airisk/backend"), path), "backend");
        assert_eq!(tag_scope(Some("backend"), path), "backend");
    }

    #[test]
    fn falls_back_to_the_directory_without_a_usable_name() {
        let path = Path::new("/repo/backend/package.json");

        assert_eq!(tag_scope(None, path), "backend");
        assert_eq!(tag_scope(Some("@scope/"), path), "backend");
    }

    #[test]
    fn keeps_slugs_valid_as_git_refs() {
        assert_eq!(slugify("web app"), "web-app");
        assert_eq!(slugify("a:b^c~d?e*f[g"), "a-b-c-d-e-f-g");
        assert_eq!(slugify("--lead.."), "lead");
        assert_eq!(slugify("cache.lock"), "cache");
        assert_eq!(slugify("..."), "");
    }
}
