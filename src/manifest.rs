use std::{
    fs,
    ops::Range,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{error::VersionerError, scan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Npm,
    Cargo,
    Python,
}

impl Format {
    /// Also the order manifests are listed in when several share a directory.
    pub(crate) const ALL: [Self; 3] = [Self::Npm, Self::Cargo, Self::Python];

    pub(crate) fn file_name(self) -> &'static str {
        match self {
            Self::Npm => "package.json",
            Self::Cargo => "Cargo.toml",
            Self::Python => "pyproject.toml",
        }
    }

    pub(crate) fn of(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;

        Self::ALL
            .into_iter()
            .find(|format| format.file_name() == name)
    }
}

/// A manifest file and where exactly its version sits inside it.
pub(crate) struct Manifest {
    pub(crate) format: Format,
    pub(crate) path: PathBuf,
    /// Repo relative path, for logs and git pathspecs.
    pub(crate) label: String,
    pub(crate) source: String,
    /// Where the version lives, e.g. `["package", "version"]`.
    key: &'static [&'static str],
    span: Range<usize>,
    pub(crate) name: Option<String>,
}

/// What a format-specific reader pulls out of a parsed manifest.
struct Found {
    key: &'static [&'static str],
    version: String,
    name: Option<String>,
}

impl Manifest {
    pub(crate) fn read(path: &Path, label: &str) -> Result<Self, VersionerError> {
        let invalid = |reason: String| VersionerError::InvalidManifest(label.to_owned(), reason);

        let format = Format::of(path)
            .ok_or_else(|| VersionerError::UnsupportedManifest(label.to_owned()))?;
        let source = fs::read_to_string(path).map_err(|error| invalid(error.to_string()))?;

        let found = match format {
            Format::Npm => npm(&source, label)?,
            Format::Cargo => cargo(&parse_toml(&source).map_err(invalid)?, label)?,
            Format::Python => python(&parse_toml(&source).map_err(invalid)?, label)?,
        };

        let span = match format {
            Format::Npm => scan::json::string_at(&source, found.key),
            Format::Cargo | Format::Python => scan::toml::string_at(&source, found.key),
        };

        // The raw text must be exactly what the parser decoded; an escaped or
        // oddly spelled value is refused rather than guessed at.
        let span = span
            .filter(|span| source[span.clone()] == found.version)
            .ok_or_else(|| VersionerError::Unlocatable(label.to_owned()))?;

        Ok(Self {
            format,
            path: path.to_path_buf(),
            label: label.to_owned(),
            source,
            key: found.key,
            span,
            name: found.name,
        })
    }

    pub(crate) fn version(&self) -> &str {
        &self.source[self.span.clone()]
    }

    /// Cargo only: the version sits in `[workspace.package]` and every member
    /// with `version.workspace = true` inherits it.
    pub(crate) fn is_workspace(&self) -> bool {
        self.key.first() == Some(&"workspace")
    }

    /// The file with its version set to `next`, re-parsed to prove the new
    /// value landed exactly where the parser expects it.
    pub(crate) fn rewritten(&self, next: &str) -> Result<String, VersionerError> {
        let updated = scan::replace(&self.source, std::slice::from_ref(&self.span), next);

        let reads = match self.format {
            Format::Npm => serde_json::from_str::<Value>(&updated)
                .ok()
                .is_some_and(|document| scan::json_string(&document, self.key) == Some(next)),
            Format::Cargo | Format::Python => parse_toml(&updated)
                .ok()
                .is_some_and(|document| scan::toml_string(&document, self.key) == Some(next)),
        };

        match reads {
            true => Ok(updated),
            false => Err(VersionerError::Unlocatable(self.label.clone())),
        }
    }
}

pub(crate) fn parse_toml(source: &str) -> Result<toml::Table, String> {
    source
        .parse::<toml::Table>()
        .map_err(|error| error.message().to_owned())
}

fn npm(source: &str, label: &str) -> Result<Found, VersionerError> {
    let document: Value = serde_json::from_str(source)
        .map_err(|error| VersionerError::InvalidManifest(label.to_owned(), error.to_string()))?;

    let name = document
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned);

    match document.get("version") {
        Some(Value::String(version)) => Ok(Found {
            key: &["version"],
            version: version.clone(),
            name,
        }),
        Some(other) => Err(VersionerError::InvalidVersion(other.to_string())),
        None => Err(VersionerError::MissingVersion(label.to_owned())),
    }
}

fn cargo(document: &toml::Table, label: &str) -> Result<Found, VersionerError> {
    let name = scan::toml_string(document, &["package", "name"]).map(str::to_owned);
    let own = document
        .get("package")
        .and_then(|package| package.get("version"));

    if let Some(toml::Value::String(version)) = own {
        return Ok(Found {
            key: &["package", "version"],
            version: version.clone(),
            name,
        });
    }

    // A workspace root: its members, and maybe its own package, inherit this.
    if let Some(version) = scan::toml_string(document, &["workspace", "package", "version"]) {
        return Ok(Found {
            key: &["workspace", "package", "version"],
            version: version.to_owned(),
            name,
        });
    }

    match own {
        Some(value) if inherits(value) => Err(VersionerError::UnsupportedVersion(
            label.to_owned(),
            "the version is inherited from the workspace, run versioner from the workspace root"
                .to_owned(),
        )),
        Some(other) => Err(VersionerError::InvalidVersion(other.to_string())),
        None => Err(VersionerError::MissingVersion(label.to_owned())),
    }
}

/// `version.workspace = true`
pub(crate) fn inherits(value: &toml::Value) -> bool {
    value.get("workspace").and_then(toml::Value::as_bool) == Some(true)
}

fn python(document: &toml::Table, label: &str) -> Result<Found, VersionerError> {
    let name = scan::toml_string(document, &["project", "name"])
        .or_else(|| scan::toml_string(document, &["tool", "poetry", "name"]))
        .map(str::to_owned);

    match document
        .get("project")
        .and_then(|project| project.get("version"))
    {
        Some(toml::Value::String(version)) => {
            return Ok(Found {
                key: &["project", "version"],
                version: version.clone(),
                name,
            });
        }
        Some(other) => return Err(VersionerError::InvalidVersion(other.to_string())),
        None => {}
    }

    if let Some(version) = scan::toml_string(document, &["tool", "poetry", "version"]) {
        return Ok(Found {
            key: &["tool", "poetry", "version"],
            version: version.to_owned(),
            name,
        });
    }

    let dynamic = document
        .get("project")
        .and_then(|project| project.get("dynamic"))
        .and_then(toml::Value::as_array)
        .is_some_and(|fields| fields.iter().any(|field| field.as_str() == Some("version")));

    match dynamic {
        true => Err(VersionerError::UnsupportedVersion(
            label.to_owned(),
            "the version is dynamic, bump it wherever your build backend reads it from".to_owned(),
        )),
        false => Err(VersionerError::MissingVersion(label.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Writes `source` under the manifest's real file name and reads it back.
    fn read(file_name: &str, source: &str) -> Result<Manifest, VersionerError> {
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "versioner-manifest-{}-{unique}",
            std::process::id()
        ));

        fs::create_dir_all(&directory).unwrap();

        let path = directory.join(file_name);
        fs::write(&path, source).unwrap();

        let manifest = Manifest::read(&path, file_name);
        let _ = fs::remove_dir_all(&directory);

        manifest
    }

    #[test]
    fn reads_each_format() {
        let npm = read("package.json", r#"{"name": "web", "version": "1.0.0"}"#).unwrap();
        assert_eq!((npm.version(), npm.name.as_deref()), ("1.0.0", Some("web")));

        let cargo = read(
            "Cargo.toml",
            "[package]\nname = \"cli\"\nversion = \"2.0.0\"\n",
        )
        .unwrap();
        assert_eq!(
            (cargo.version(), cargo.name.as_deref()),
            ("2.0.0", Some("cli"))
        );
        assert!(!cargo.is_workspace());

        let python = read(
            "pyproject.toml",
            "[project]\nname = \"lib\"\nversion = \"3.0.0\"\n",
        )
        .unwrap();
        assert_eq!(
            (python.version(), python.name.as_deref()),
            ("3.0.0", Some("lib"))
        );

        let poetry = read(
            "pyproject.toml",
            "[tool.poetry]\nname = \"old\"\nversion = \"4.0.0\"\n",
        )
        .unwrap();
        assert_eq!(
            (poetry.version(), poetry.name.as_deref()),
            ("4.0.0", Some("old"))
        );
    }

    #[test]
    fn reads_a_cargo_workspace_version() {
        let source =
            "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"0.3.0\"\n";
        let manifest = read("Cargo.toml", source).unwrap();

        assert_eq!(manifest.version(), "0.3.0");
        assert!(manifest.is_workspace());

        let source = "[package]\nname = \"root\"\nversion.workspace = true\n\n[workspace.package]\nversion = \"0.3.0\"\n";
        assert!(read("Cargo.toml", source).unwrap().is_workspace());
    }

    #[test]
    fn explains_versions_it_cannot_bump() {
        let inherited = read(
            "Cargo.toml",
            "[package]\nname = \"a\"\nversion.workspace = true\n",
        );
        assert!(matches!(
            inherited,
            Err(VersionerError::UnsupportedVersion(..))
        ));

        let dynamic = read(
            "pyproject.toml",
            "[project]\nname = \"a\"\ndynamic = [\"version\"]\n",
        );
        assert!(matches!(
            dynamic,
            Err(VersionerError::UnsupportedVersion(..))
        ));

        let missing = read("package.json", r#"{"name": "a"}"#);
        assert!(matches!(missing, Err(VersionerError::MissingVersion(_))));

        let number = read("package.json", r#"{"version": 1}"#);
        assert!(matches!(number, Err(VersionerError::InvalidVersion(_))));
    }

    #[test]
    fn refuses_a_version_whose_text_differs_from_its_value() {
        let escaped = read("package.json", r#"{"version": "1.0.\u0030"}"#);

        assert!(
            matches!(escaped, Err(VersionerError::Unlocatable(_))),
            "{:?}",
            escaped.err()
        );
    }

    #[test]
    fn rewrites_only_the_version() {
        let source = "[package]\nname = \"cli\"  # keep me\nversion = \"2.0.0\" # and me\n";
        let manifest = read("Cargo.toml", source).unwrap();

        assert_eq!(
            manifest.rewritten("2.1.0").unwrap(),
            "[package]\nname = \"cli\"  # keep me\nversion = \"2.1.0\" # and me\n"
        );
    }
}
