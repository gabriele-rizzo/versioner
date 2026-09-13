use std::{path::PathBuf, process::Command};

use crate::log;

pub(crate) struct Git;

impl Git {
    /// Runs a git command and returns its stdout verbatim. `Err` carries git's
    /// own diagnostics, whether it failed to spawn or exited non-zero.
    fn capture(args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .output()
            .map_err(|error| format!("could not run git: {error}"))?;

        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        let command = format!("git {}", args.join(" "));

        Err(match stderr.trim() {
            "" => format!("'{command}' failed"),
            detail => format!("'{command}' failed: {detail}"),
        })
    }

    fn run(args: &[&str]) -> Result<(), String> {
        Git::capture(args).map(|_| ())
    }

    fn succeeds(args: &[&str]) -> bool {
        Git::capture(args).is_ok()
    }

    pub(crate) fn is_repository() -> bool {
        Git::succeeds(&["rev-parse", "--git-dir"])
    }

    /// Top level of the worktree, or `None` outside a repository.
    pub(crate) fn repository_root() -> Option<PathBuf> {
        match Git::capture(&["rev-parse", "--show-toplevel"]) {
            Ok(root) if !root.trim().is_empty() => Some(PathBuf::from(root.trim())),
            _ => None,
        }
    }

    pub(crate) fn tag_exists(tag: &str) -> bool {
        Git::succeeds(&["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")])
    }

    pub(crate) fn has_origin_remote() -> bool {
        Git::succeeds(&["remote", "get-url", "origin"])
    }

    /// The checked out branch, or `None` when HEAD is detached.
    pub(crate) fn current_branch() -> Option<String> {
        match Git::capture(&["branch", "--show-current"]) {
            Ok(branch) if !branch.trim().is_empty() => Some(branch.trim().to_owned()),
            _ => None,
        }
    }

    /// Changed paths other than the package.json being bumped, which
    /// `git add -A` would sweep into the version commit. `bumped` is repo
    /// relative, the same form `--porcelain` reports.
    pub(crate) fn unrelated_changes(bumped: &str) -> Vec<String> {
        Git::capture(&["status", "--porcelain"])
            .map(|status| unrelated_paths(&status, bumped))
            .unwrap_or_default()
    }

    pub(crate) fn commit(message: &str, tag: &str) -> Result<(), String> {
        Git::run(&["add", "-A"])?;
        Git::run(&["commit", "-m", message])?;
        Git::run(&["tag", "-a", tag, "-m", message])?;

        log::info(&format!("successfully committed '{tag}'"));

        Ok(())
    }

    pub(crate) fn push(branch: &str, tag: &str) -> Result<(), String> {
        Git::run(&["push", "-u", "origin", branch])?;
        Git::run(&["push", "origin", tag])?;

        log::info(&format!("pushed {branch} and '{tag}' to origin"));

        Ok(())
    }
}

/// Pulls the paths out of `git status --porcelain` output, dropping the one
/// package.json being bumped. Each line is `XY <path>`, so the path starts at
/// the fourth byte.
fn unrelated_paths(status: &str, bumped: &str) -> Vec<String> {
    status
        .lines()
        .filter_map(|line| line.get(3..))
        .map(|path| path.rsplit(" -> ").next().unwrap_or(path).trim_matches('"'))
        .filter(|path| *path != bumped && !path.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::unrelated_paths;

    #[test]
    fn keeps_the_first_path_intact() {
        let status = " M before.json\n M other.txt\n";

        assert_eq!(
            unrelated_paths(status, "package.json"),
            ["before.json", "other.txt"]
        );
    }

    #[test]
    fn reads_every_status_code_column() {
        let status = "?? new.txt\nM  staged.txt\nA  added.txt\nMM both.txt\n";

        assert_eq!(
            unrelated_paths(status, "package.json"),
            ["new.txt", "staged.txt", "added.txt", "both.txt"]
        );
    }

    #[test]
    fn reports_the_destination_of_a_rename() {
        let status = "R  old.txt -> new.txt\n";

        assert_eq!(unrelated_paths(status, "package.json"), ["new.txt"]);
    }

    #[test]
    fn unquotes_paths_with_special_characters() {
        let status = "?? \"sp ace.txt\"\n";

        assert_eq!(unrelated_paths(status, "package.json"), ["sp ace.txt"]);
    }

    #[test]
    fn drops_only_the_package_json_being_bumped() {
        let status = " M package.json\n M backend/package.json\n";

        assert_eq!(
            unrelated_paths(status, "backend/package.json"),
            ["package.json"]
        );
        assert_eq!(
            unrelated_paths(status, "package.json"),
            ["backend/package.json"]
        );
    }

    #[test]
    fn a_clean_tree_has_no_paths() {
        assert_eq!(unrelated_paths("", "package.json"), [] as [String; 0]);
    }
}
