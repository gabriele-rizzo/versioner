//! Runs the real binary against throwaway git repositories.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU32, Ordering},
};

/// Keeps the user's own git config (signing, hooks, default branch) out of it.
const ISOLATED: [(&str, &str); 6] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "Test"),
    ("GIT_AUTHOR_EMAIL", "test@example.com"),
    ("GIT_COMMITTER_NAME", "Test"),
    ("GIT_COMMITTER_EMAIL", "test@example.com"),
];

struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("versioner-it-{}-{unique}", std::process::id()));

        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("work")).unwrap();

        let repo = Self {
            root: fs::canonicalize(&root).unwrap(),
        };

        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.work().join(path);

        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.work().join(path)).unwrap()
    }

    fn command(program: impl AsRef<std::ffi::OsStr>, directory: &Path) -> Command {
        let mut command = Command::new(program);

        command.current_dir(directory).envs(ISOLATED);
        command
    }

    fn git_in(directory: &Path, args: &[&str]) -> String {
        let output = Repo::command("git", directory).args(args).output().unwrap();

        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn git(&self, args: &[&str]) -> String {
        Repo::git_in(&self.work(), args)
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    fn status(&self) -> String {
        self.git(&["status", "--porcelain", "--untracked-files=all"])
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn tags(&self) -> String {
        self.git(&["tag", "--list"])
    }

    fn versioner(&self, args: &[&str]) -> Run {
        self.versioner_in(".", args)
    }

    fn versioner_in(&self, directory: &str, args: &[&str]) -> Run {
        let output = Repo::command(
            env!("CARGO_BIN_EXE_versioner"),
            &self.work().join(directory),
        )
        .args(args)
        .output()
        .unwrap();

        Run(output)
    }

    /// A bare repository wired up as `origin`.
    fn with_origin(&self) -> PathBuf {
        let origin = self.root.join("origin.git");

        Repo::git_in(
            &self.root,
            &["init", "-q", "--bare", origin.to_str().unwrap()],
        );
        self.git(&["remote", "add", "origin", origin.to_str().unwrap()]);

        origin
    }

    #[cfg(unix)]
    fn hook(&self, name: &str, script: &str) {
        use std::os::unix::fs::PermissionsExt;

        let path = self.work().join(".git/hooks").join(name);

        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Run(Output);

impl Run {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.0.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.0.stderr).into_owned()
    }

    #[track_caller]
    fn succeeds(self) -> Self {
        assert!(
            self.0.status.success(),
            "versioner failed:\n{}{}",
            self.stdout(),
            self.stderr()
        );
        self
    }

    #[track_caller]
    fn fails_with(self, needle: &str) -> Self {
        assert!(
            !self.0.status.success(),
            "versioner unexpectedly succeeded:\n{}",
            self.stdout()
        );
        assert!(
            self.stderr().contains(needle),
            "stderr lacks {needle:?}:\n{}",
            self.stderr()
        );
        self
    }
}

const PACKAGE: &str =
    "{\n  \"name\": \"demo\",\n  \"version\": \"1.2.3\",\n  \"private\": true\n}\n";

const PACKAGE_LOCK: &str = r#"{
  "name": "demo",
  "version": "1.2.3",
  "lockfileVersion": 3,
  "requires": true,
  "packages": {
    "": {
      "name": "demo",
      "version": "1.2.3"
    }
  }
}
"#;

fn npm_repo() -> Repo {
    let repo = Repo::new();

    repo.write("package.json", PACKAGE);
    repo.commit_all("init");
    repo
}

#[test]
fn bumps_commits_and_tags() {
    let repo = npm_repo();

    repo.versioner(&["patch", "release 1.2.4"]).succeeds();

    assert_eq!(repo.read("package.json"), PACKAGE.replace("1.2.3", "1.2.4"));
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "release 1.2.4");
    assert_eq!(
        repo.git(&["cat-file", "-t", "v1.2.4"]),
        "tag",
        "annotated tag"
    );
    assert_eq!(repo.git(&["rev-parse", "v1.2.4^{commit}"]), repo.head());
    assert_eq!(repo.status(), "");
}

#[test]
fn refuses_an_existing_tag_without_touching_anything() {
    let repo = npm_repo();
    repo.git(&["tag", "v1.3.0"]);

    repo.versioner(&["minor", "again"])
        .fails_with("tag 'v1.3.0' already exists");

    assert_eq!(repo.read("package.json"), PACKAGE);
    assert_eq!(repo.status(), "");
}

#[test]
fn keeps_the_package_lock_in_step() {
    let repo = Repo::new();
    repo.write("package.json", PACKAGE);
    repo.write("package-lock.json", PACKAGE_LOCK);
    repo.commit_all("init");

    repo.versioner(&["major", "2.0"]).succeeds();

    assert_eq!(
        repo.read("package-lock.json"),
        PACKAGE_LOCK.replace("1.2.3", "2.0.0")
    );
    assert_eq!(
        repo.git(&["show", "--name-only", "--format=", "HEAD"]),
        "package-lock.json\npackage.json"
    );
    assert_eq!(repo.status(), "");
}

#[test]
fn updates_a_workspace_member_in_the_root_lock() {
    let repo = Repo::new();
    let lock = r#"{
  "name": "mono",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "packages": {
    "": {"name": "mono", "version": "1.0.0", "workspaces": ["backend"]},
    "backend": {"name": "@mono/backend", "version": "0.4.0"},
    "node_modules/@mono/backend": {"resolved": "backend", "link": true}
  }
}
"#;

    repo.write("package.json", r#"{"name": "mono", "version": "1.0.0"}"#);
    repo.write(
        "backend/package.json",
        r#"{"name": "@mono/backend", "version": "0.4.0"}"#,
    );
    repo.write("package-lock.json", lock);
    repo.commit_all("init");

    repo.versioner_in("backend", &["minor", "backend 0.5"])
        .succeeds();

    assert_eq!(
        repo.read("package-lock.json"),
        lock.replace(r#""version": "0.4.0""#, r#""version": "0.5.0""#)
    );
    assert!(repo.read("package.json").contains("1.0.0"));
    assert_eq!(repo.tags(), "backend-v0.5.0");
    assert_eq!(repo.status(), "");
}

#[test]
fn commits_only_the_version_files() {
    let repo = npm_repo();
    repo.write("notes.txt", "draft\n");
    repo.write("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);

    let run = repo.versioner(&["patch", "just the bump"]).succeeds();

    assert!(!run.stderr().contains("will also include"));
    assert_eq!(
        repo.git(&["show", "--name-only", "--format=", "HEAD"]),
        "package.json"
    );
    assert_eq!(repo.status(), "A  staged.txt\n?? notes.txt");
}

#[test]
fn warns_when_the_manifest_already_had_changes() {
    let repo = npm_repo();
    repo.write(
        "package.json",
        &PACKAGE.replace("\"private\": true", "\"private\": false"),
    );

    let run = repo.versioner(&["patch", "bump"]).succeeds();

    assert!(
        run.stderr()
            .contains("package.json already had uncommitted changes")
    );
    assert!(
        repo.git(&["show", "HEAD:package.json"])
            .contains("\"private\": false")
    );
}

#[test]
fn all_commits_the_whole_worktree() {
    let repo = npm_repo();
    repo.write("notes.txt", "draft\n");

    let run = repo.versioner(&["patch", "everything", "--all"]).succeeds();

    assert!(
        run.stderr()
            .contains("this commit will also include notes.txt")
    );
    assert_eq!(
        repo.git(&["show", "--name-only", "--format=", "HEAD"]),
        "notes.txt\npackage.json"
    );
    assert_eq!(repo.status(), "");
}

#[cfg(unix)]
#[test]
fn restores_files_and_index_when_the_commit_fails() {
    let repo = npm_repo();
    let head = repo.head();
    repo.write("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("notes.txt", "draft\n");
    repo.hook("pre-commit", "#!/bin/sh\necho 'hook says no' >&2\nexit 1\n");

    for flags in [&[][..], &["--all"][..]] {
        let args = [&["patch", "blocked"][..], flags].concat();

        repo.versioner(&args).fails_with("hook says no");

        assert_eq!(repo.read("package.json"), PACKAGE);
        assert_eq!(repo.head(), head);
        assert_eq!(repo.tags(), "");
        assert_eq!(
            repo.status(),
            "A  staged.txt\n?? notes.txt",
            "with {flags:?}"
        );
    }
}

#[test]
fn rolls_the_commit_back_when_tagging_fails() {
    let repo = npm_repo();
    let head = repo.head();
    repo.write("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);

    // refs/tags/v1.2.4 can't be created while refs/tags/v1.2.4/x exists, yet
    // the up-front "tag exists" check still passes.
    repo.git(&["update-ref", "refs/tags/v1.2.4/x", &head]);

    repo.versioner(&["patch", "doomed"])
        .fails_with("changes rolled back");

    assert_eq!(repo.head(), head, "the version commit is undone");
    assert_eq!(repo.read("package.json"), PACKAGE);
    assert_eq!(repo.status(), "A  staged.txt");
    assert_eq!(repo.git(&["tag", "--list", "v1.2.4"]), "");
}

#[test]
fn rolls_back_to_an_unborn_branch() {
    let repo = Repo::new();
    repo.write("package.json", PACKAGE);
    repo.git(&["add", "package.json"]);

    // No commit exists yet, so any object id makes a valid conflicting ref.
    let blob = repo.git(&["hash-object", "-w", "package.json"]);
    repo.git(&["update-ref", "refs/tags/v1.2.4/x", &blob]);

    repo.versioner(&["patch", "first"])
        .fails_with("changes rolled back");

    assert!(
        !Repo::command("git", &repo.work())
            .args(["rev-parse", "-q", "--verify", "HEAD"])
            .status()
            .unwrap()
            .success(),
        "HEAD should be unborn again"
    );
    assert_eq!(repo.status(), "A  package.json");
    assert_eq!(repo.read("package.json"), PACKAGE);
}

#[test]
fn dry_run_changes_nothing() {
    let repo = Repo::new();
    repo.write("package.json", PACKAGE);
    repo.write("package-lock.json", PACKAGE_LOCK);
    repo.commit_all("init");
    let origin = repo.with_origin();
    let head = repo.head();

    let run = repo
        .versioner(&["minor", "preview", "--dry-run"])
        .succeeds();
    let stdout = run.stdout();

    assert!(
        stdout.contains("would update package.json: 1.2.3 → 1.3.0"),
        "{stdout}"
    );
    assert!(
        stdout.contains("would update package-lock.json: 1.2.3 → 1.3.0"),
        "{stdout}"
    );
    assert!(
        stdout.contains("would commit package.json, package-lock.json: 'preview'"),
        "{stdout}"
    );
    assert!(stdout.contains("would tag 'v1.3.0'"), "{stdout}");
    assert!(
        stdout.contains("would push main and 'v1.3.0' to origin"),
        "{stdout}"
    );

    assert_eq!(repo.read("package.json"), PACKAGE);
    assert_eq!(repo.head(), head);
    assert_eq!(repo.tags(), "");
    assert_eq!(repo.status(), "");
    assert_eq!(Repo::git_in(&origin, &["for-each-ref"]), "");
}

#[test]
fn dry_run_still_refuses_an_existing_tag() {
    let repo = npm_repo();
    repo.git(&["tag", "v1.2.4"]);

    repo.versioner(&["patch", "x", "--dry-run"])
        .fails_with("already exists");
}

#[test]
fn pushes_branch_and_tag_to_origin() {
    let repo = npm_repo();
    let origin = repo.with_origin();

    repo.versioner(&["patch", "ship it"]).succeeds();

    assert_eq!(Repo::git_in(&origin, &["rev-parse", "main"]), repo.head());
    assert_eq!(Repo::git_in(&origin, &["tag", "--list"]), "v1.2.4");
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "main@{upstream}"]),
        "origin/main"
    );
}

#[test]
fn no_push_keeps_everything_local() {
    let repo = npm_repo();
    let origin = repo.with_origin();

    let run = repo.versioner(&["patch", "local", "--no-push"]).succeeds();

    assert!(run.stdout().contains("not pushing (--no-push)"));
    assert!(
        run.stdout()
            .contains("git push --atomic -u origin main v1.2.4")
    );
    assert_eq!(repo.tags(), "v1.2.4");
    assert_eq!(Repo::git_in(&origin, &["for-each-ref"]), "");
}

#[test]
fn walks_through_a_pre_release_cycle() {
    let repo = npm_repo();

    repo.versioner(&["minor", "beta", "--pre", "beta"])
        .succeeds();
    assert!(repo.read("package.json").contains("\"1.3.0-beta.0\""));

    repo.versioner(&["pre", "beta 1"]).succeeds();
    assert!(repo.read("package.json").contains("\"1.3.0-beta.1\""));

    repo.versioner(&["pre", "rc", "--id", "rc"]).succeeds();
    assert!(repo.read("package.json").contains("\"1.3.0-rc.0\""));

    repo.versioner(&["pre", "back to beta", "--id", "beta"])
        .fails_with("would move the version backwards");

    repo.versioner(&["minor", "final"]).succeeds();
    assert!(repo.read("package.json").contains("\"1.3.0\""));

    assert_eq!(
        repo.tags(),
        "v1.3.0\nv1.3.0-beta.0\nv1.3.0-beta.1\nv1.3.0-rc.0"
    );
}

#[test]
fn rejects_a_malformed_pre_release_id() {
    let repo = npm_repo();

    repo.versioner(&["major", "x", "--pre", "be ta"])
        .fails_with("invalid pre-release identifier");
}

const CARGO_LOCK: &str = r#"# This file is automatically @generated by Cargo.
version = 4

[[package]]
name = "demo"
version = "0.1.0"
dependencies = [
 "itoa",
]

[[package]]
name = "itoa"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

#[test]
fn bumps_a_crate_and_its_lock_entry() {
    let repo = Repo::new();
    let manifest =
        "[package]\nname = \"demo\"\nversion = \"0.1.0\" # the crate\nedition = \"2024\"\n";

    repo.write("Cargo.toml", manifest);
    repo.write("Cargo.lock", CARGO_LOCK);
    repo.commit_all("init");

    repo.versioner(&["minor", "0.2"]).succeeds();

    assert_eq!(repo.read("Cargo.toml"), manifest.replace("0.1.0", "0.2.0"));

    // Only the local crate moves; the registry dependency at 0.1.0 stays put.
    let lock = repo.read("Cargo.lock");
    assert!(
        lock.contains("name = \"demo\"\nversion = \"0.2.0\""),
        "{lock}"
    );
    assert!(
        lock.contains("name = \"itoa\"\nversion = \"0.1.0\""),
        "{lock}"
    );
    assert_eq!(repo.tags(), "v0.2.0");
    assert_eq!(repo.status(), "");
}

#[test]
fn bumps_a_cargo_workspace_and_every_inheriting_member() {
    let repo = Repo::new();

    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/skip\"]\n\n[workspace.package]\nversion = \"1.0.0\"\n",
    );
    repo.write(
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion.workspace = true\n",
    );
    repo.write(
        "crates/cli/Cargo.toml",
        "[package]\nname = \"cli\"\nversion = { workspace = true }\n",
    );
    repo.write(
        "crates/own/Cargo.toml",
        "[package]\nname = \"own\"\nversion = \"1.0.0\"\n",
    );
    repo.write(
        "crates/skip/Cargo.toml",
        "[package]\nname = \"skip\"\nversion.workspace = true\n",
    );
    repo.write(
        "Cargo.lock",
        "version = 4\n\n[[package]]\nname = \"cli\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"core\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"own\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"skip\"\nversion = \"1.0.0\"\n",
    );
    repo.commit_all("init");

    repo.versioner(&["major", "2.0"]).succeeds();

    let lock = repo.read("Cargo.lock");
    assert!(
        lock.contains("name = \"cli\"\nversion = \"2.0.0\""),
        "{lock}"
    );
    assert!(
        lock.contains("name = \"core\"\nversion = \"2.0.0\""),
        "{lock}"
    );
    assert!(
        lock.contains("name = \"own\"\nversion = \"1.0.0\""),
        "{lock}"
    );
    assert!(
        lock.contains("name = \"skip\"\nversion = \"1.0.0\""),
        "{lock}"
    );
    assert!(repo.read("Cargo.toml").contains("version = \"2.0.0\""));
}

#[test]
fn points_an_inheriting_member_at_the_workspace_root() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"core\"]\n");
    repo.write(
        "core/Cargo.toml",
        "[package]\nname = \"core\"\nversion.workspace = true\n",
    );
    repo.commit_all("init");

    repo.versioner_in("core", &["patch", "x"])
        .fails_with("run versioner from the workspace root");
}

#[test]
fn bumps_a_python_project_and_its_uv_lock() {
    let repo = Repo::new();
    let lock = r#"version = 1
revision = 2
requires-python = ">=3.12"

[[package]]
name = "my-lib"
version = "0.9.0"
source = { editable = "." }
dependencies = [
    { name = "requests" },
]

[[package]]
name = "requests"
version = "0.9.0"
source = { registry = "https://pypi.org/simple" }
"#;

    repo.write(
        "pyproject.toml",
        "[project]\nname = \"My_Lib\"\nversion = \"0.9.0\"\n",
    );
    repo.write("uv.lock", lock);
    repo.commit_all("init");

    repo.versioner(&["minor", "1.0 soon"]).succeeds();

    assert_eq!(
        repo.read("pyproject.toml"),
        "[project]\nname = \"My_Lib\"\nversion = \"0.10.0\"\n"
    );
    assert_eq!(
        repo.read("uv.lock"),
        lock.replacen("version = \"0.9.0\"", "version = \"0.10.0\"", 1)
    );
    assert_eq!(repo.status(), "");
}

#[test]
fn bumps_a_poetry_project() {
    let repo = Repo::new();
    repo.write(
        "pyproject.toml",
        "[tool.poetry]\nname = \"old\"\nversion = \"1.0.0\"\n",
    );
    repo.commit_all("init");

    repo.versioner(&["patch", "fix"]).succeeds();

    assert!(repo.read("pyproject.toml").contains("version = \"1.0.1\""));
}

#[test]
fn asks_which_manifest_when_several_carry_a_version() {
    let repo = Repo::new();
    repo.write("package.json", PACKAGE);
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    );
    repo.commit_all("init");

    repo.versioner(&["patch", "x"])
        .fails_with("pick one with --manifest");

    repo.versioner(&["patch", "cargo only", "--manifest", "Cargo.toml"])
        .succeeds();

    assert!(repo.read("Cargo.toml").contains("0.1.1"));
    assert_eq!(repo.read("package.json"), PACKAGE);
}

#[test]
fn skips_a_package_json_without_a_version() {
    let repo = Repo::new();
    repo.write(
        "package.json",
        r#"{"private": true, "devDependencies": {}}"#,
    );
    repo.write(
        "pyproject.toml",
        "[project]\nname = \"tool\"\nversion = \"0.1.0\"\n",
    );
    repo.commit_all("init");

    repo.versioner(&["patch", "x"]).succeeds();

    assert!(repo.read("pyproject.toml").contains("0.1.1"));
}

#[test]
fn fails_outside_a_repository() {
    let repo = Repo::new();
    let outside = repo.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("package.json"), PACKAGE).unwrap();

    let output = Repo::command(env!("CARGO_BIN_EXE_versioner"), &outside)
        .env("GIT_CEILING_DIRECTORIES", &repo.root)
        .args(["patch", "x"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a git repository"));
}
