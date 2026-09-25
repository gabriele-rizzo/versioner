//! Keeps a lockfile's record of the project's own version in step with the
//! manifest, so the version commit doesn't leave `npm install` or
//! `cargo build` a dirty tree to clean up.

use std::{
    fs,
    ops::Range,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    edit::{Edit, label},
    error::VersionerError,
    manifest::{self, Format, Manifest},
    scan,
};

pub(crate) fn edits(
    manifest: &Manifest,
    root: &Path,
    next: &str,
) -> Result<Vec<Edit>, VersionerError> {
    let edit = match manifest.format {
        Format::Npm => npm(manifest, root, next)?,
        Format::Cargo => cargo(manifest, root, next)?,
        Format::Python => uv(manifest, root, next)?,
    };

    Ok(edit.into_iter().collect())
}

/// `package-lock.json` records the version at the top and under `packages[""]`
/// for its own package, and under `packages["<dir>"]` for a workspace member.
fn npm(manifest: &Manifest, root: &Path, next: &str) -> Result<Option<Edit>, VersionerError> {
    let directory = parent(&manifest.path);

    let Some(lock) = nearest(
        directory,
        root,
        &["npm-shrinkwrap.json", "package-lock.json"],
    ) else {
        return Ok(None);
    };

    let key = label(parent(&lock), directory);
    let lock_label = label(root, &lock);
    let source = read(&lock, &lock_label)?;

    let document: Value = serde_json::from_str(&source)
        .map_err(|error| VersionerError::InvalidManifest(lock_label.clone(), error.to_string()))?;

    let mut paths = vec![vec!["packages", key.as_str(), "version"]];

    if key.is_empty() {
        paths.push(vec!["version"]);
    }

    // Only entries that exist; each one's span must hold what the parser read.
    let mut kept = Vec::new();
    let mut spans = Vec::new();

    for path in paths {
        let Some(expected) = scan::json_string(&document, &path) else {
            continue;
        };

        let span = scan::json::string_at(&source, &path)
            .filter(|span| &source[span.clone()] == expected)
            .ok_or_else(|| VersionerError::Unlocatable(lock_label.clone()))?;

        kept.push(path);
        spans.push(span);
    }

    let Some(first) = spans.first() else {
        return Ok(None);
    };

    let from = source[first.clone()].to_owned();
    let updated = scan::replace(&source, &spans, next);

    let lands = serde_json::from_str::<Value>(&updated).is_ok_and(|document| {
        kept.iter()
            .all(|path| scan::json_string(&document, path) == Some(next))
    });

    if !lands {
        return Err(VersionerError::Unlocatable(lock_label));
    }

    Ok(Some(Edit {
        path: lock,
        label: lock_label,
        from,
        original: source,
        updated,
    }))
}

/// Cargo.lock lists local crates as `[[package]]` blocks without a `source`.
/// A workspace version moves every member that inherits it.
fn cargo(manifest: &Manifest, root: &Path, next: &str) -> Result<Option<Edit>, VersionerError> {
    let Some(lock) = nearest(parent(&manifest.path), root, &["Cargo.lock"]) else {
        return Ok(None);
    };

    let names = match manifest.is_workspace() {
        true => inheriting_members(manifest),
        false => manifest.name.clone().into_iter().collect(),
    };

    locked_packages(&lock, root, next, |name, source| {
        source.is_none() && names.iter().any(|member| member == name)
    })
}

/// uv.lock marks the project itself with an `editable` or `virtual` source.
fn uv(manifest: &Manifest, root: &Path, next: &str) -> Result<Option<Edit>, VersionerError> {
    let Some(name) = manifest.name.as_deref().map(normalize_python) else {
        return Ok(None);
    };

    let Some(lock) = nearest(parent(&manifest.path), root, &["uv.lock"]) else {
        return Ok(None);
    };

    locked_packages(&lock, root, next, |locked, source| {
        normalize_python(locked) == name
            && source
                .is_some_and(|source| source.contains("editable") || source.contains("virtual"))
    })
}

/// Rewrites the `version` of every `[[package]]` that `matches` its name and
/// raw `source`, cross-checked against a real parse before and after.
fn locked_packages(
    lock: &Path,
    root: &Path,
    next: &str,
    matches: impl Fn(&str, Option<&str>) -> bool,
) -> Result<Option<Edit>, VersionerError> {
    let lock_label = label(root, lock);
    let source = read(lock, &lock_label)?;
    let unlocatable = || VersionerError::Unlocatable(lock_label.clone());

    let scanned: Vec<(String, Range<usize>)> = scan::toml::packages(&source)
        .into_iter()
        .filter(|package| matches(&package.name, package.source.as_deref()))
        .filter_map(|package| Some((package.name, package.version?)))
        .collect();

    if scanned.is_empty() {
        return Ok(None);
    }

    let parsed = |source: &str| -> Option<Vec<(String, String)>> {
        let document = manifest::parse_toml(source).ok()?;
        let mut found = Vec::new();

        for package in document.get("package")?.as_array()? {
            let name = package.get("name")?.as_str()?;
            let origin = package.get("source").map(toml::Value::to_string);

            if matches(name, origin.as_deref()) {
                found.push((
                    name.to_owned(),
                    package.get("version")?.as_str()?.to_owned(),
                ));
            }
        }

        found.sort();
        Some(found)
    };

    let mut expected: Vec<(String, String)> = scanned
        .iter()
        .map(|(name, span)| (name.clone(), source[span.clone()].to_owned()))
        .collect();
    expected.sort();

    if parsed(&source).as_ref() != Some(&expected) {
        return Err(unlocatable());
    }

    let spans: Vec<Range<usize>> = scanned.iter().map(|(_, span)| span.clone()).collect();
    let updated = scan::replace(&source, &spans, next);

    let lands = parsed(&updated).is_some_and(|found| {
        found.len() == spans.len() && found.iter().all(|(_, version)| version == next)
    });

    if !lands {
        return Err(unlocatable());
    }

    Ok(Some(Edit {
        path: lock.to_path_buf(),
        label: lock_label,
        from: source[spans[0].clone()].to_owned(),
        original: source,
        updated,
    }))
}

/// Names of the workspace crates whose version is `version.workspace = true`,
/// found through `workspace.members` (literal paths and `*`/`?` wildcards).
fn inheriting_members(manifest: &Manifest) -> Vec<String> {
    let Ok(document) = manifest::parse_toml(&manifest.source) else {
        return Vec::new();
    };

    let directory = parent(&manifest.path);
    let workspace = document.get("workspace");
    let patterns = |key: &str| -> Vec<String> {
        workspace
            .and_then(|workspace| workspace.get(key))
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|pattern| pattern.as_str().map(str::to_owned))
            .collect()
    };

    let excluded: Vec<PathBuf> = patterns("exclude")
        .iter()
        .flat_map(|pattern| expand(directory, pattern))
        .collect();

    let mut members: Vec<PathBuf> = patterns("members")
        .iter()
        .flat_map(|pattern| expand(directory, pattern))
        .filter(|member| !excluded.contains(member))
        .collect();

    // The root package, when there is one, can inherit from its own workspace.
    members.insert(0, directory.to_path_buf());

    let mut names: Vec<String> = members
        .iter()
        .filter_map(|member| {
            let source = fs::read_to_string(member.join("Cargo.toml")).ok()?;
            let document = manifest::parse_toml(&source).ok()?;
            let package = document.get("package")?;

            manifest::inherits(package.get("version")?)
                .then(|| package.get("name")?.as_str().map(str::to_owned))?
        })
        .collect();

    names.sort();
    names.dedup();
    names
}

/// Directories matching a `/`-separated pattern whose segments may use `*`
/// and `?`, sorted for a stable result.
fn expand(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut found = vec![base.to_path_buf()];

    for segment in pattern
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
    {
        found = found
            .into_iter()
            .flat_map(|directory| {
                if !segment.contains(['*', '?']) {
                    return vec![directory.join(segment)];
                }

                let mut matched: Vec<PathBuf> = fs::read_dir(&directory)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| wildcard(segment.as_bytes(), name.as_bytes()))
                    })
                    .collect();

                matched.sort();
                matched
            })
            .collect();
    }

    found
}

fn wildcard(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.first(), name.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            wildcard(&pattern[1..], name) || (!name.is_empty() && wildcard(pattern, &name[1..]))
        }
        (Some(b'?'), Some(_)) => wildcard(&pattern[1..], &name[1..]),
        (Some(expected), Some(actual)) if expected == actual => wildcard(&pattern[1..], &name[1..]),
        _ => false,
    }
}

/// PEP 503: case-insensitive, with runs of `-`, `_` and `.` all equal to `-`.
fn normalize_python(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());

    for character in name.chars() {
        match character {
            '-' | '_' | '.' => {
                if !normalized.ends_with('-') {
                    normalized.push('-');
                }
            }
            character => normalized.extend(character.to_lowercase()),
        }
    }

    normalized
}

/// Nearest of `names` at or above `from`, checked in order in each directory
/// and never looking past the repository root.
fn nearest(from: &Path, root: &Path, names: &[&str]) -> Option<PathBuf> {
    for directory in from.ancestors() {
        if !directory.starts_with(root) {
            break;
        }

        let found = names
            .iter()
            .map(|name| directory.join(name))
            .find(|path| path.is_file());

        if found.is_some() {
            return found;
        }
    }

    None
}

fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new(""))
}

fn read(path: &Path, label: &str) -> Result<String, VersionerError> {
    fs::read_to_string(path)
        .map_err(|error| VersionerError::InvalidManifest(label.to_owned(), error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{normalize_python, wildcard};

    #[test]
    fn matches_wildcards() {
        assert!(wildcard(b"*", b"core"));
        assert!(wildcard(b"crate-*", b"crate-cli"));
        assert!(wildcard(b"c?re", b"core"));
        assert!(!wildcard(b"crate-*", b"core"));
        assert!(!wildcard(b"c?re", b"cre"));
    }

    #[test]
    fn normalizes_python_names() {
        assert_eq!(normalize_python("My_Package.Name"), "my-package-name");
        assert_eq!(normalize_python("a--_b"), "a-b");
    }
}
