use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use crate::{cli::Args, error::VersionerError, git::Git, log, version::Version};
use serde_json::Value;

pub(crate) struct Project {
    path: PathBuf,
    source: String,
    version: Range<usize>,
    /// Tag namespace for a workspace package; `None` at the repository root,
    /// which keeps the bare `vX.Y.Z` tags.
    scope: Option<String>,
    /// Repo relative path, for logs and for matching `git status` output.
    label: String,
}

impl Project {
    pub(crate) fn parse() -> Project {
        let Ok(cwd) = std::env::current_dir() else {
            VersionerError::PackageNotFound.fatal()
        };

        let cwd = canonical(&cwd);
        let root = Git::repository_root().map(|root| canonical(&root));

        let path = locate(&cwd, root.as_deref())
            .unwrap_or_else(|| VersionerError::PackageNotFound.fatal());

        if !path.is_file() {
            VersionerError::InvalidPackage.fatal()
        }

        let Ok(source) = std::fs::read_to_string(&path) else {
            VersionerError::InvalidPackage.fatal()
        };

        let Ok(document) = serde_json::from_str::<Value>(&source) else {
            VersionerError::InvalidPackage.fatal()
        };

        let version =
            version_span(&source).unwrap_or_else(|| VersionerError::InvalidVersion.fatal());

        let at_root = root
            .as_deref()
            .is_some_and(|root| path.parent() == Some(root));
        let scope = (!at_root).then(|| tag_scope(&document, &path));

        let label = root
            .as_deref()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(path.as_path())
            .display()
            .to_string();

        Self {
            path,
            source,
            version,
            scope,
            label,
        }
    }

    /// `v1.2.3` for the repository root, `backend-v1.2.3` for a workspace
    /// package, so sibling packages can't collide on one tag.
    fn tag(&self, version: &Version) -> String {
        match &self.scope {
            Some(scope) => format!("{scope}-v{version}"),
            None => format!("v{version}"),
        }
    }

    pub(crate) fn version(&self) -> &str {
        &self.source[self.version.clone()]
    }

    /// Rewrites only the version value, leaving the rest of the file byte-for-byte intact.
    fn save(&mut self, current: &Version, next: &Version) {
        let value = next.to_string();

        self.source.replace_range(self.version.clone(), &value);
        self.version = self.version.start..self.version.start + value.len();

        if std::fs::write(&self.path, &self.source).is_err() {
            VersionerError::SaveFailed.fatal();
        }

        let arrow = if current < next { "→" } else { "←" };

        log::info(&format!("{}: {current} {arrow} {next}", self.label));
    }

    pub(crate) fn update(&mut self, args: Args) {
        if !Git::is_repository() {
            VersionerError::NotARepository.fatal()
        }

        let current = Version::parse(self.version());
        let next = current.bump(&args.command);
        let tag = self.tag(&next);

        if Git::tag_exists(&tag) {
            VersionerError::TagExists(tag).fatal()
        }

        let branch = Git::current_branch();

        warn_about_unrelated_changes(&self.label);

        self.save(&current, &next);

        if let Err(reason) = Git::commit(args.command.message(), &tag) {
            self.save(&next, &current);
            VersionerError::CommitFailed(reason).fatal()
        }

        let hint = match &branch {
            Some(branch) => {
                format!("push with: 'git push -u origin {branch} && git push origin {tag}'")
            }
            None => format!("push with: 'git push origin {tag}'"),
        };

        if !Git::has_origin_remote() {
            return log::info(&format!("no 'origin' remote configured, {hint}"));
        }

        let Some(branch) = branch else {
            return log::warn(&format!("detached HEAD, nothing to push to, {hint}"));
        };

        if let Err(reason) = Git::push(&branch, &tag) {
            log::warn(&reason);
            log::info(&format!("changes exist locally, {hint}"));
        }
    }
}

/// `Git::commit` stages the whole worktree, so say what else is coming along.
fn warn_about_unrelated_changes(bumped: &str) {
    let changes = Git::unrelated_changes(bumped);

    let Some((first, rest)) = changes.split_first() else {
        return;
    };

    let listed = match rest.len() {
        0 => first.to_owned(),
        1 => format!("{first} and {}", rest[0]),
        count => format!("{first} and {count} other files"),
    };

    log::warn(&format!("this commit will also include {listed}"));
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Nearest package.json at or above `from`, never climbing past the repository
/// root so a bump can't reach a package outside the repo it tags. Without a
/// repository there is nothing to bound the walk, and `Project::update` rejects
/// that case anyway, so only `from` itself is considered.
fn locate(from: &Path, root: Option<&Path>) -> Option<PathBuf> {
    let Some(root) = root else {
        return Some(from.join("package.json")).filter(|path| path.exists());
    };

    for directory in from.ancestors() {
        let candidate = directory.join("package.json");

        if candidate.exists() {
            return Some(candidate);
        }

        if directory == root {
            break;
        }
    }

    None
}

/// Git-ref-safe tag namespace for a workspace package: the unscoped half of its
/// `"name"`, falling back to the directory it lives in.
fn tag_scope(document: &Value, path: &Path) -> String {
    let name = document
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = slugify(name.rsplit('/').next().unwrap_or_default());

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

/// Byte range of the top-level `"version"` string value inside a JSON document.
fn version_span(source: &str) -> Option<Range<usize>> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'{' | b'[' => {
                depth += 1;
                cursor += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
            }
            b'"' => {
                let (key, after) = string_span(bytes, cursor)?;
                cursor = after;

                if depth != 1 || &source[key] != "version" {
                    continue;
                }

                let colon = skip_whitespace(bytes, cursor);

                if bytes.get(colon) != Some(&b':') {
                    continue;
                }

                let value = skip_whitespace(bytes, colon + 1);

                if bytes.get(value) != Some(&b'"') {
                    return None;
                }

                return string_span(bytes, value).map(|(span, _)| span);
            }
            _ => cursor += 1,
        }
    }

    None
}

/// Span of a string's contents plus the index just past its closing quote.
fn string_span(bytes: &[u8], quote: usize) -> Option<(Range<usize>, usize)> {
    let mut cursor = quote + 1;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'"' => return Some((quote + 1..cursor, cursor + 1)),
            _ => cursor += 1,
        }
    }

    None
}

fn skip_whitespace(bytes: &[u8], from: usize) -> usize {
    let mut cursor = from;

    while matches!(bytes.get(cursor), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        cursor += 1;
    }

    cursor
}

#[cfg(test)]
mod tests {
    use super::{locate, slugify, tag_scope, version_span};
    use serde_json::json;
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU32, Ordering},
    };

    /// A throwaway `root/backend/src` tree with package.json files wherever
    /// `packages` says, mirroring a workspace layout.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(packages: &[&str]) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);

            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("versioner-{}-{unique}", std::process::id()));

            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("backend/src")).unwrap();

            for package in packages {
                fs::write(root.join(package).join("package.json"), "{}").unwrap();
            }

            Self {
                root: fs::canonicalize(&root).unwrap(),
            }
        }

        fn at(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }

        fn found(&self, from: &str) -> Option<String> {
            locate(&self.at(from), Some(&self.root)).map(|path| {
                path.strip_prefix(&self.root)
                    .unwrap_or(&path)
                    .display()
                    .to_string()
            })
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn finds_the_package_json_in_the_current_directory() {
        let tree = Tree::new(&[".", "backend"]);

        assert_eq!(tree.found("."), Some("package.json".to_owned()));
        assert_eq!(
            tree.found("backend"),
            Some("backend/package.json".to_owned())
        );
    }

    #[test]
    fn walks_up_to_the_nearest_package_json() {
        let tree = Tree::new(&[".", "backend"]);

        assert_eq!(
            tree.found("backend/src"),
            Some("backend/package.json".to_owned())
        );
    }

    #[test]
    fn walks_past_directories_without_a_package_json() {
        let tree = Tree::new(&["."]);

        assert_eq!(tree.found("backend/src"), Some("package.json".to_owned()));
    }

    #[test]
    fn never_climbs_past_the_repository_root() {
        let tree = Tree::new(&[]);

        assert_eq!(tree.found("backend/src"), None);
        assert_eq!(tree.found("."), None);
    }

    #[test]
    fn outside_a_repository_only_the_current_directory_counts() {
        let tree = Tree::new(&["."]);

        assert_eq!(locate(&tree.at("."), None), Some(tree.at("package.json")));
        assert_eq!(locate(&tree.at("backend/src"), None), None);
    }

    #[test]
    fn scopes_a_tag_by_the_unscoped_package_name() {
        let path = Path::new("/repo/backend/package.json");

        assert_eq!(
            tag_scope(&json!({"name": "@airisk/backend"}), path),
            "backend"
        );
        assert_eq!(tag_scope(&json!({"name": "backend"}), path), "backend");
    }

    #[test]
    fn falls_back_to_the_directory_without_a_usable_name() {
        let path = Path::new("/repo/backend/package.json");

        assert_eq!(tag_scope(&json!({}), path), "backend");
        assert_eq!(tag_scope(&json!({"name": "@scope/"}), path), "backend");
        assert_eq!(tag_scope(&json!({"name": 3}), path), "backend");
    }

    #[test]
    fn keeps_slugs_valid_as_git_refs() {
        assert_eq!(slugify("web app"), "web-app");
        assert_eq!(slugify("a:b^c~d?e*f[g"), "a-b-c-d-e-f-g");
        assert_eq!(slugify("--lead.."), "lead");
        assert_eq!(slugify("cache.lock"), "cache");
        assert_eq!(slugify("..."), "");
    }

    fn version_of(source: &str) -> Option<&str> {
        version_span(source).map(|span| &source[span])
    }

    #[test]
    fn finds_the_top_level_version() {
        assert_eq!(version_of(r#"{"version": "1.2.3"}"#), Some("1.2.3"));
        assert_eq!(
            version_of("{\n\t\"version\": \"1.2.3\"\n}\n"),
            Some("1.2.3")
        );
    }

    #[test]
    fn ignores_a_nested_version() {
        let source = r#"{"engines": {"version": "nested"}, "version": "1.2.3"}"#;

        assert_eq!(version_of(source), Some("1.2.3"));
    }

    #[test]
    fn ignores_version_used_as_a_value() {
        let source = r#"{"type": "version", "version": "1.2.3"}"#;

        assert_eq!(version_of(source), Some("1.2.3"));
    }

    #[test]
    fn ignores_version_inside_an_array() {
        let source = r#"{"keywords": ["version"], "version": "1.2.3"}"#;

        assert_eq!(version_of(source), Some("1.2.3"));
    }

    #[test]
    fn tolerates_whitespace_around_the_colon() {
        assert_eq!(version_of("{\"version\"\n  :\t\"1.2.3\"}"), Some("1.2.3"));
    }

    #[test]
    fn skips_escaped_quotes_and_multibyte_text() {
        let source = r#"{"description": "héllo \"version\": ✓", "version": "1.2.3"}"#;

        assert_eq!(version_of(source), Some("1.2.3"));
    }

    #[test]
    fn spans_only_the_value() {
        let source = r#"{"version": "1.2.3"}"#;
        let span = version_span(source).unwrap();

        assert_eq!(&source[span.start - 1..span.end + 1], r#""1.2.3""#);
    }

    #[test]
    fn finds_nothing_without_a_top_level_version() {
        assert_eq!(version_of(r#"{"name": "demo"}"#), None);
        assert_eq!(version_of(r#"{"a": {"version": "1.2.3"}}"#), None);
    }

    #[test]
    fn finds_nothing_when_the_version_is_not_a_string() {
        assert_eq!(version_of(r#"{"version": 3}"#), None);
        assert_eq!(version_of(r#"{"version": null}"#), None);
    }
}
