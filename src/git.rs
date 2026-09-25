use std::{path::PathBuf, process::Command};

use crate::log;

pub(crate) struct Git;

/// Where HEAD and the index stood before a release touched them.
pub(crate) struct Snapshot {
    /// `None` on a branch with no commits yet.
    head: Option<String>,
    /// Tree object holding the index exactly as it was, staged changes included.
    index: String,
}

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

    fn head() -> Option<String> {
        Git::capture(&["rev-parse", "-q", "--verify", "HEAD"])
            .ok()
            .map(|head| head.trim().to_owned())
    }

    /// Every changed path in the worktree, repo relative, untracked files
    /// listed one by one rather than collapsed into their directory.
    pub(crate) fn changed_paths() -> Vec<String> {
        Git::capture(&["status", "--porcelain", "-z", "--untracked-files=all"])
            .map(|status| changed_paths(&status))
            .unwrap_or_default()
    }

    /// Fails during an unresolved merge, which is no time to cut a release.
    pub(crate) fn snapshot() -> Result<Snapshot, String> {
        Ok(Snapshot {
            head: Git::head(),
            index: Git::capture(&["write-tree"])?.trim().to_owned(),
        })
    }

    /// Moves HEAD back to where `snapshot` found it (only if a commit was made
    /// on top) and reloads the index it saved.
    pub(crate) fn restore(snapshot: &Snapshot) -> Result<(), String> {
        let current = Git::head();

        if let Some(commit) = current.as_deref().filter(|_| current != snapshot.head) {
            match &snapshot.head {
                Some(head) => Git::run(&[
                    "update-ref",
                    "-m",
                    "versioner: roll back failed release",
                    "HEAD",
                    head,
                    commit,
                ])?,
                // First commit on the branch: back to an unborn branch.
                None => Git::run(&["update-ref", "-d", "HEAD", commit])?,
            }
        }

        Git::run(&["read-tree", &snapshot.index])
    }

    /// Commits `paths` (repo relative) and nothing else, leaving whatever else
    /// is staged still staged. With `all`, commits the whole worktree.
    pub(crate) fn commit(message: &str, paths: &[&str], all: bool) -> Result<(), String> {
        if all {
            Git::run(&["add", "-A"])?;
            return Git::run(&["commit", "-m", message]);
        }

        let specs: Vec<String> = paths
            .iter()
            .map(|path| format!(":(top,literal){path}"))
            .collect();
        let specs: Vec<&str> = specs.iter().map(String::as_str).collect();

        Git::run(&[&["add", "--"], specs.as_slice()].concat())?;
        Git::run(&[&["commit", "-m", message, "--only", "--"], specs.as_slice()].concat())
    }

    pub(crate) fn tag(tag: &str, message: &str) -> Result<(), String> {
        Git::run(&["tag", "-a", tag, "-m", message])?;

        log::info(&format!("successfully committed '{tag}'"));

        Ok(())
    }

    /// Pushes the branch and the tag together, so the remote never ends up with
    /// one and not the other. Falls back to a plain push for servers that don't
    /// support `--atomic`.
    pub(crate) fn push(branch: &str, tag: &str) -> Result<(), String> {
        let tag_ref = format!("refs/tags/{tag}");

        match Git::run(&["push", "--atomic", "-u", "origin", branch, &tag_ref]) {
            Err(reason) if reason.contains("does not support --atomic") => {
                Git::run(&["push", "-u", "origin", branch, &tag_ref])?;
            }
            other => other?,
        }

        log::info(&format!("pushed {branch} and '{tag}' to origin"));

        Ok(())
    }
}

/// Paths out of `git status --porcelain -z`: NUL-separated `XY <path>` records,
/// where a rename or copy is followed by one extra record holding its source.
fn changed_paths(status: &str) -> Vec<String> {
    let mut records = status.split('\0');
    let mut paths = Vec::new();

    while let Some(record) = records.next() {
        let Some(path) = record.get(3..).filter(|path| !path.is_empty()) else {
            continue;
        };

        if matches!(record.as_bytes().first(), Some(b'R' | b'C')) {
            records.next();
        }

        paths.push(path.to_owned());
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::changed_paths;

    #[test]
    fn keeps_the_first_path_intact() {
        let status = " M before.json\0 M other.txt\0";

        assert_eq!(changed_paths(status), ["before.json", "other.txt"]);
    }

    #[test]
    fn reads_every_status_code_column() {
        let status = "?? new.txt\0M  staged.txt\0A  added.txt\0MM both.txt\0";

        assert_eq!(
            changed_paths(status),
            ["new.txt", "staged.txt", "added.txt", "both.txt"]
        );
    }

    #[test]
    fn reports_the_destination_of_a_rename() {
        let status = "R  new.txt\0old.txt\0 M after.txt\0";

        assert_eq!(changed_paths(status), ["new.txt", "after.txt"]);
    }

    #[test]
    fn keeps_special_characters_verbatim() {
        let status = "?? sp ace \"q\".txt\0";

        assert_eq!(changed_paths(status), ["sp ace \"q\".txt"]);
    }

    #[test]
    fn a_clean_tree_has_no_paths() {
        assert_eq!(changed_paths(""), [] as [String; 0]);
    }
}
