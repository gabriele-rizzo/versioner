use regex::Regex;
use std::{cmp::Ordering, fmt, sync::LazyLock};

use crate::error::VersionerError;

/// The grammar from semver.org: no leading zeros, optional `-pre` and `+build`.
static SEMVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$",
    )
    .unwrap()
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Identifier {
    Numeric(u64),
    Alpha(String),
}

impl Identifier {
    /// One dot-separated pre-release identifier; numeric ones can't carry
    /// leading zeros.
    fn parse(text: &str) -> Option<Self> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return None;
        }

        if !text.bytes().all(|b| b.is_ascii_digit()) {
            return Some(Self::Alpha(text.to_owned()));
        }

        if text.len() > 1 && text.starts_with('0') {
            return None;
        }

        text.parse().ok().map(Self::Numeric)
    }

    /// `beta` or `rc.1` as given to `--pre`/`--id`.
    pub(crate) fn parse_all(text: &str) -> Result<Vec<Self>, VersionerError> {
        text.split('.')
            .map(Self::parse)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| VersionerError::InvalidPreId(text.to_owned()))
    }
}

impl Ord for Identifier {
    /// Numeric identifiers sort below alphanumeric ones, per semver §11.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Numeric(a), Self::Numeric(b)) => a.cmp(b),
            (Self::Numeric(_), Self::Alpha(_)) => Ordering::Less,
            (Self::Alpha(_), Self::Numeric(_)) => Ordering::Greater,
            (Self::Alpha(a), Self::Alpha(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Identifier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Numeric(number) => write!(f, "{number}"),
            Self::Alpha(text) => f.write_str(text),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Major,
    Minor,
    Patch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Bump {
    /// `major`/`minor`/`patch`. On a pre-release of that level this finalizes
    /// it instead of skipping ahead: 2.0.0-rc.1 → 2.0.0 for `major`.
    Release(Level),
    /// `major --pre rc`: bump the level, then open a pre-release of it.
    Start(Level, Vec<Identifier>),
    /// `pre`: the next pre-release, continuing the current one when `id`
    /// matches (or is omitted) and opening one on the next patch otherwise.
    Next(Option<Vec<Identifier>>),
}

/// Semver version. `build` is kept for display but ignored when comparing,
/// and every bump drops it.
#[derive(Debug, Clone)]
pub(crate) struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Identifier>,
    build: Option<String>,
}

impl Version {
    pub(crate) fn parse(version: &str) -> Result<Self, VersionerError> {
        let invalid = || VersionerError::InvalidVersion(version.to_owned());
        let captures = SEMVER.captures(version).ok_or_else(invalid)?;

        let component = |index: usize| captures[index].parse().map_err(|_| invalid());

        let pre = match captures.get(4) {
            Some(pre) => Identifier::parse_all(pre.as_str()).map_err(|_| invalid())?,
            None => Vec::new(),
        };

        Ok(Self {
            major: component(1)?,
            minor: component(2)?,
            patch: component(3)?,
            pre,
            build: captures.get(5).map(|build| build.as_str().to_owned()),
        })
    }

    fn release(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: Vec::new(),
            build: None,
        }
    }

    fn with_pre(self, pre: Vec<Identifier>) -> Self {
        Self { pre, ..self }
    }

    /// `level` bumped, ignoring any pre-release: always a new x.y.z.
    fn increment(&self, level: Level) -> Result<Self, VersionerError> {
        let next = |number: u64| number.checked_add(1).ok_or(VersionerError::VersionOverflow);

        Ok(match level {
            Level::Major => Self::release(next(self.major)?, 0, 0),
            Level::Minor => Self::release(self.major, next(self.minor)?, 0),
            Level::Patch => Self::release(self.major, self.minor, next(self.patch)?),
        })
    }

    pub(crate) fn bump(&self, bump: &Bump) -> Result<Self, VersionerError> {
        match bump {
            Bump::Release(level) => {
                let finalizes = !self.pre.is_empty()
                    && match level {
                        Level::Major => self.minor == 0 && self.patch == 0,
                        Level::Minor => self.patch == 0,
                        Level::Patch => true,
                    };

                match finalizes {
                    true => Ok(Self::release(self.major, self.minor, self.patch)),
                    false => self.increment(*level),
                }
            }
            Bump::Start(level, id) => Ok(self.increment(*level)?.with_pre(opened(id))),
            Bump::Next(id) => {
                let continues =
                    !self.pre.is_empty() && id.as_ref().is_none_or(|id| self.pre.starts_with(id));

                if continues {
                    let core = Self::release(self.major, self.minor, self.patch);
                    return Ok(core.with_pre(advanced(&self.pre)?));
                }

                let core = match self.pre.is_empty() {
                    true => self.increment(Level::Patch)?,
                    false => Self::release(self.major, self.minor, self.patch),
                };

                Ok(core.with_pre(opened(id.as_deref().unwrap_or_default())))
            }
        }
    }
}

/// `rc` → `rc.0`, and a bare `0` without an identifier, as npm does.
fn opened(id: &[Identifier]) -> Vec<Identifier> {
    let mut pre = id.to_vec();
    pre.push(Identifier::Numeric(0));
    pre
}

/// Increments the last numeric identifier, or appends `.0` when there is none.
fn advanced(pre: &[Identifier]) -> Result<Vec<Identifier>, VersionerError> {
    let mut pre = pre.to_vec();

    let last = pre
        .iter_mut()
        .rev()
        .find_map(|identifier| match identifier {
            Identifier::Numeric(number) => Some(number),
            Identifier::Alpha(_) => None,
        });

    match last {
        Some(number) => {
            *number = number
                .checked_add(1)
                .ok_or(VersionerError::VersionOverflow)?
        }
        None => pre.push(Identifier::Numeric(0)),
    }

    Ok(pre)
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let core =
            (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));

        // A pre-release sorts below the release it leads up to.
        let pre = match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => self.pre.cmp(&other.pre),
        };

        core.then(pre)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;

        if let Some((first, rest)) = self.pre.split_first() {
            write!(f, "-{first}")?;

            for identifier in rest {
                write!(f, ".{identifier}")?;
            }
        }

        if let Some(build) = &self.build {
            write!(f, "+{build}")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn bumped(from: &str, bump: Bump) -> String {
        v(from).bump(&bump).unwrap().to_string()
    }

    fn id(text: &str) -> Vec<Identifier> {
        Identifier::parse_all(text).unwrap()
    }

    #[test]
    fn parses_a_semver_string() {
        assert_eq!(v("10.20.30").to_string(), "10.20.30");
        assert_eq!(v("0.0.0").to_string(), "0.0.0");
    }

    #[test]
    fn parses_pre_release_and_build_metadata() {
        assert_eq!(v("1.0.0-beta.2").to_string(), "1.0.0-beta.2");
        assert_eq!(
            v("1.0.0-rc.1+sha.5114f85").to_string(),
            "1.0.0-rc.1+sha.5114f85"
        );
        assert_eq!(v("1.0.0+20260925").to_string(), "1.0.0+20260925");
    }

    #[test]
    fn rejects_what_semver_rejects() {
        for text in [
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.3-",
            "1.2.3-01",
            "1.2.3-a..b",
            "v1.2.3",
            "",
        ] {
            assert!(Version::parse(text).is_err(), "{text} should be rejected");
        }
    }

    #[test]
    fn rejects_components_that_overflow() {
        assert!(Version::parse("99999999999999999999.0.0").is_err());
    }

    #[test]
    fn major_bump_resets_minor_and_patch() {
        assert_eq!(bumped("1.2.3", Bump::Release(Level::Major)), "2.0.0");
    }

    #[test]
    fn minor_bump_resets_patch_and_keeps_major() {
        assert_eq!(bumped("1.2.3", Bump::Release(Level::Minor)), "1.3.0");
    }

    #[test]
    fn patch_bump_keeps_major_and_minor() {
        assert_eq!(bumped("1.2.3", Bump::Release(Level::Patch)), "1.2.4");
    }

    #[test]
    fn a_bump_drops_build_metadata() {
        assert_eq!(
            bumped("1.2.3+build.7", Bump::Release(Level::Patch)),
            "1.2.4"
        );
    }

    #[test]
    fn a_release_bump_finalizes_a_matching_pre_release() {
        assert_eq!(bumped("2.0.0-rc.1", Bump::Release(Level::Major)), "2.0.0");
        assert_eq!(bumped("1.3.0-beta.0", Bump::Release(Level::Minor)), "1.3.0");
        assert_eq!(bumped("1.2.4-0", Bump::Release(Level::Patch)), "1.2.4");
    }

    #[test]
    fn a_release_bump_moves_past_a_lower_level_pre_release() {
        assert_eq!(bumped("1.3.1-rc.0", Bump::Release(Level::Minor)), "1.4.0");
        assert_eq!(bumped("1.3.0-rc.0", Bump::Release(Level::Major)), "2.0.0");
    }

    #[test]
    fn starting_a_pre_release_bumps_first() {
        assert_eq!(
            bumped("1.2.3", Bump::Start(Level::Major, id("rc"))),
            "2.0.0-rc.0"
        );
        assert_eq!(
            bumped("1.2.3", Bump::Start(Level::Minor, id("beta"))),
            "1.3.0-beta.0"
        );
        assert_eq!(
            bumped("1.2.3", Bump::Start(Level::Patch, vec![])),
            "1.2.4-0"
        );
    }

    #[test]
    fn next_continues_the_current_pre_release() {
        assert_eq!(bumped("1.3.0-beta.0", Bump::Next(None)), "1.3.0-beta.1");
        assert_eq!(
            bumped("1.3.0-beta.9", Bump::Next(Some(id("beta")))),
            "1.3.0-beta.10"
        );
        assert_eq!(bumped("1.3.0-beta", Bump::Next(None)), "1.3.0-beta.0");
        assert_eq!(bumped("1.3.0-beta.1.x", Bump::Next(None)), "1.3.0-beta.2.x");
    }

    #[test]
    fn next_switches_to_a_new_identifier_on_the_same_release() {
        assert_eq!(
            bumped("1.3.0-beta.4", Bump::Next(Some(id("rc")))),
            "1.3.0-rc.0"
        );
    }

    #[test]
    fn next_opens_a_pre_release_on_the_next_patch() {
        assert_eq!(
            bumped("1.2.3", Bump::Next(Some(id("alpha")))),
            "1.2.4-alpha.0"
        );
        assert_eq!(bumped("1.2.3", Bump::Next(None)), "1.2.4-0");
    }

    #[test]
    fn release_bumps_always_move_forward() {
        for from in ["1.2.3", "1.3.0-beta.1", "2.0.0-rc.0", "0.0.0"] {
            for level in [Level::Major, Level::Minor, Level::Patch] {
                let current = v(from);
                assert!(current < current.bump(&Bump::Release(level)).unwrap());
            }
        }
    }

    #[test]
    fn detects_overflow_instead_of_wrapping() {
        let max = format!("{}.0.0", u64::MAX);

        assert!(matches!(
            v(&max).bump(&Bump::Release(Level::Major)),
            Err(VersionerError::VersionOverflow)
        ));
    }

    #[test]
    fn orders_by_component_not_lexically() {
        assert!(v("1.2.3") < v("1.10.0"));
        assert!(v("2.0.0") > v("1.99.99"));
    }

    #[test]
    fn orders_pre_releases_as_semver_specifies() {
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
        ];

        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn ignores_build_metadata_when_comparing() {
        assert_eq!(v("1.0.0+a"), v("1.0.0+b"));
    }

    #[test]
    fn validates_pre_release_identifiers() {
        assert_eq!(
            id("rc.1"),
            [Identifier::Alpha("rc".into()), Identifier::Numeric(1)]
        );

        for text in ["", "beta..1", "be ta", "01", "ß"] {
            assert!(
                Identifier::parse_all(text).is_err(),
                "{text:?} should be rejected"
            );
        }
    }
}
