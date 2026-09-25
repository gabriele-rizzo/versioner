use crate::{
    cli::Args,
    edit::Edit,
    error::VersionerError,
    git::{Git, Snapshot},
    log,
    project::Project,
    version::Version,
};

/// What happens after the tag is in place.
enum Push {
    To(String),
    Skip { hint: String, reason: String },
}

pub(crate) fn run(args: Args) {
    if !Git::is_repository() {
        VersionerError::NotARepository.fatal()
    }

    let project = Project::locate(args.manifest.as_deref()).unwrap_or_else(|error| error.fatal());
    let bump = args.command.bump().unwrap_or_else(|error| error.fatal());

    let current = Version::parse(project.manifest.version()).unwrap_or_else(|error| error.fatal());
    let next = current.bump(&bump).unwrap_or_else(|error| error.fatal());

    if next <= current {
        VersionerError::Backwards(current.to_string(), next.to_string()).fatal()
    }

    let tag = project.tag(&next);

    if Git::tag_exists(&tag) {
        VersionerError::TagExists(tag).fatal()
    }

    let edits = project.edits(&next).unwrap_or_else(|error| error.fatal());
    let paths: Vec<&str> = edits.iter().map(|edit| edit.label.as_str()).collect();
    let message = args.command.message();

    warn_about_changes(&paths, args.all);

    let push = plan_push(&tag, args.no_push);

    if args.dry_run {
        return preview(&edits, &next, &tag, message, args.all, &push);
    }

    let snapshot =
        Git::snapshot().unwrap_or_else(|reason| VersionerError::SnapshotFailed(reason).fatal());

    Edit::apply(&edits).unwrap_or_else(|error| error.fatal());

    for edit in &edits {
        log::info(&format!("{}: {} → {next}", edit.label, edit.from));
    }

    if let Err(reason) = Git::commit(message, &paths, args.all) {
        abort(VersionerError::CommitFailed(reason), &snapshot, &edits)
    }

    if let Err(reason) = Git::tag(&tag, message) {
        abort(VersionerError::TagFailed(reason), &snapshot, &edits)
    }

    match push {
        Push::To(branch) => {
            if let Err(reason) = Git::push(&branch, &tag) {
                log::warn(&reason);
                log::info(&format!(
                    "changes exist locally, {}",
                    push_hint(Some(&branch), &tag)
                ));
            }
        }
        Push::Skip { hint, reason } => log::info(&format!("{reason}, {hint}")),
    }
}

/// Undoes a half-finished release: HEAD and the index go back to `snapshot`,
/// every file to its original contents, then `error` ends the run.
fn abort(error: VersionerError, snapshot: &Snapshot, edits: &[Edit]) -> ! {
    if let Err(reason) = Git::restore(snapshot) {
        log::warn(&format!("could not restore the repository: {reason}"));
    }

    let unrestored = Edit::restore(edits);

    if !unrestored.is_empty() {
        log::warn(&format!("could not restore {}", unrestored.join(", ")));
    }

    error.fatal()
}

fn plan_push(tag: &str, no_push: bool) -> Push {
    let branch = Git::current_branch();
    let hint = push_hint(branch.as_deref(), tag);

    let reason = match branch {
        _ if no_push => "not pushing (--no-push)",
        _ if !Git::has_origin_remote() => "no 'origin' remote configured",
        None => "detached HEAD, nothing to push to",
        Some(branch) => return Push::To(branch),
    };

    Push::Skip {
        hint,
        reason: reason.to_owned(),
    }
}

fn push_hint(branch: Option<&str>, tag: &str) -> String {
    match branch {
        Some(branch) => format!("push with: 'git push --atomic -u origin {branch} {tag}'"),
        None => format!("push with: 'git push origin {tag}'"),
    }
}

fn preview(edits: &[Edit], next: &Version, tag: &str, message: &str, all: bool, push: &Push) {
    log::info("dry run, nothing will be written");

    for edit in edits {
        log::info(&format!(
            "would update {}: {} → {next}",
            edit.label, edit.from
        ));
    }

    let what = match all {
        true => "every change in the worktree".to_owned(),
        false => edits
            .iter()
            .map(|edit| edit.label.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    };

    log::info(&format!("would commit {what}: '{message}'"));
    log::info(&format!("would tag '{tag}'"));

    match push {
        Push::To(branch) => log::info(&format!("would push {branch} and '{tag}' to origin")),
        Push::Skip { reason, .. } => log::info(&format!("would not push: {reason}")),
    }
}

/// Points out what the commit will carry beyond the version bump itself.
fn warn_about_changes(paths: &[&str], all: bool) {
    let changed = Git::changed_paths();

    let dirty: Vec<String> = paths
        .iter()
        .filter(|path| changed.iter().any(|changed| changed == *path))
        .map(|path| (*path).to_owned())
        .collect();

    if let Some(listed) = listed(&dirty) {
        log::warn(&format!(
            "{listed} already had uncommitted changes, the version commit will include them"
        ));
    }

    if !all {
        return;
    }

    let unrelated: Vec<String> = changed
        .into_iter()
        .filter(|changed| !paths.contains(&changed.as_str()))
        .collect();

    if let Some(listed) = listed(&unrelated) {
        log::warn(&format!("this commit will also include {listed}"));
    }
}

/// `a`, `a and b`, or `a and 3 other files`.
fn listed(paths: &[String]) -> Option<String> {
    let (first, rest) = paths.split_first()?;

    Some(match rest.len() {
        0 => first.to_owned(),
        1 => format!("{first} and {}", rest[0]),
        count => format!("{first} and {count} other files"),
    })
}
