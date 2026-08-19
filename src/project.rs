use std::{ops::Range, path::PathBuf};

use crate::{cli::Args, error::VersionerError, git::Git, log, version::Version};
use serde_json::Value;

pub(crate) struct Project {
    path: PathBuf,
    source: String,
    version: Range<usize>,
}

impl Project {
    pub(crate) fn parse() -> Project {
        let Ok(cwd) = std::env::current_dir() else {
            VersionerError::PackageNotFound.fatal()
        };

        let path = cwd.join("package.json");

        if !path.exists() {
            VersionerError::PackageNotFound.fatal()
        }

        if !path.is_file() {
            VersionerError::InvalidPackage.fatal()
        }

        let Ok(source) = std::fs::read_to_string(&path) else {
            VersionerError::InvalidPackage.fatal()
        };

        if serde_json::from_str::<Value>(&source).is_err() {
            VersionerError::InvalidPackage.fatal()
        }

        let version =
            version_span(&source).unwrap_or_else(|| VersionerError::InvalidVersion.fatal());

        Self {
            path,
            source,
            version,
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

        if current < next {
            log::info(&format!("{} → {}", current, next));
        } else {
            log::info(&format!("{} ← {}", current, next));
        }
    }

    pub(crate) fn update(&mut self, args: Args) {
        if !Git::is_repository() {
            VersionerError::NotARepository.fatal()
        }

        let current = Version::parse(self.version());
        let next = current.bump(&args.command);
        let tag = format!("v{next}");

        if Git::tag_exists(&tag) {
            VersionerError::TagExists(tag).fatal()
        }

        let branch = Git::current_branch();

        warn_about_unrelated_changes();

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
fn warn_about_unrelated_changes() {
    let changes = Git::unrelated_changes();

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
    use super::version_span;

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
