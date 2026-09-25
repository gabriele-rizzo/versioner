# versioner

Bump a project's version, commit it, tag it and push it — in one command, and
without leaving a mess behind when something fails halfway.

```sh
versioner minor "feat: search"
# info: package.json: 1.4.2 → 1.5.0
# info: package-lock.json: 1.4.2 → 1.5.0
# info: successfully committed 'v1.5.0'
# info: pushed main and 'v1.5.0' to origin
```

## Install

```sh
cargo install --git https://github.com/gabriele-rizzo/versioner
```

## Usage

```
versioner <major|minor|patch> <message> [--pre <id>]
versioner pre <message> [--id <id>]
```

| Command                 | Effect                                                             |
| ----------------------- | ------------------------------------------------------------------ |
| `major`                 | `1.4.2 → 2.0.0`                                                    |
| `minor`                 | `1.4.2 → 1.5.0`                                                    |
| `patch`                 | `1.4.2 → 1.4.3`                                                    |
| `minor --pre beta`      | `1.4.2 → 1.5.0-beta.0`: bump, then open a pre-release of it        |
| `pre`                   | `1.5.0-beta.0 → 1.5.0-beta.1`, or `1.4.2 → 1.4.3-0` from a release |
| `pre --id rc`           | `1.5.0-beta.3 → 1.5.0-rc.0`, or `1.4.2 → 1.4.3-rc.0`               |
| `minor` on a pre-release | `1.5.0-rc.0 → 1.5.0`: finalizes it                                |

These follow npm's `semver.inc` rules. A bump that would move the version
backwards (`pre --id alpha` on `1.5.0-beta.3`) is refused.

The message is used for both the commit and the annotated tag.

### Options

| Flag              | Effect                                                                        |
| ----------------- | ----------------------------------------------------------------------------- |
| `--dry-run`       | Print every file change, the commit, the tag and the push, then stop.        |
| `--no-push`       | Commit and tag locally only.                                                  |
| `--all`           | Commit the whole worktree, not just the version files.                       |
| `--manifest PATH` | Bump this manifest instead of the nearest one above the current directory.   |

## What gets bumped

versioner walks up from the current directory to the nearest manifest, never
past the repository root:

| Manifest         | Version read from                                  | Lockfile kept in step                                   |
| ---------------- | -------------------------------------------------- | ------------------------------------------------------- |
| `package.json`   | `version`                                          | `package-lock.json` / `npm-shrinkwrap.json`             |
| `Cargo.toml`     | `[package] version`, else `[workspace.package]`    | `Cargo.lock` (every member inheriting a workspace version) |
| `pyproject.toml` | `[project] version`, else `[tool.poetry] version`  | `uv.lock`                                               |

Lockfiles are searched upwards too, so bumping `backend/package.json` in an npm
workspace updates `packages["backend"]` in the root `package-lock.json`.

When a directory holds several manifests, the one with a version wins (a
`package.json` that only lists dev tools doesn't shadow `pyproject.toml`). If
more than one has a version, versioner asks you to choose with `--manifest`.

Only the version text is rewritten; formatting, comments and key order stay
byte-for-byte intact. Each edit is re-parsed before anything is written, to
confirm the new value is exactly where the file's parser expects it.

## Tags

The manifest at the repository root gets `vX.Y.Z` tags. Any other manifest
gets a prefix from its package name (or its directory), e.g.
`backend-v0.5.0`, so sibling packages never collide.

## Commits and failures

By default only the files versioner changed go into the commit. Anything else
you had staged stays staged, and untracked files stay untracked. If a version
file already had uncommitted edits, you're warned that they'll be included.

If the commit or the tag fails (a hook rejects it, the tag ref can't be
created, …), versioner rolls everything back. HEAD returns to where it was
(even on a branch with no commits yet), the index is restored exactly, and
every file gets its original contents. You end up where you started.

The branch and tag are pushed together with `git push --atomic`, so the
remote never gets one without the other. A failed push leaves the release in
place locally and prints the command to retry.

## Limitations

- Versions must be semver (`x.y.z[-pre][+build]`). PEP 440 spellings like
  `1.0.0rc1` are rejected, and so are dynamic `pyproject.toml` versions.
- A Cargo workspace member with `version.workspace = true` has to be bumped
  from the workspace root.
- Cargo `path` dependencies that pin a member's version (`version = "1.0"`)
  aren't updated. After a major bump, update those yourself.
- Workspace `members` globs support `*` and `?` only.

## Development

```sh
cargo test                          # unit tests + end-to-end runs against temp repos
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```
