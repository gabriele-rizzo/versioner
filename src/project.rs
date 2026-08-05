use std::path::PathBuf;

use crate::{cli::Args, error::VersionerError, git::Git, log, version::Version};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub(crate) struct Project {
    pub(crate) path: PathBuf,
    pub(crate) json: Value,
}

impl Project {
    pub(crate) fn parse() -> Project {
        if let Ok(cwd) = std::env::current_dir() {
            let path = cwd.join("package.json");

            if !path.exists() {
                VersionerError::PackageNotFound.fatal()
            }

            if !path.is_file() {
                VersionerError::InvalidPackage.fatal()
            }

            if let Ok(contents) = std::fs::read_to_string(&path) {
                let json = serde_json::from_str(&contents)
                    .unwrap_or_else(|_| VersionerError::InvalidPackage.fatal());

                return Self { path, json };
            } else {
                VersionerError::InvalidPackage.fatal()
            }
        }

        VersionerError::PackageNotFound.fatal()
    }

    fn save(&mut self, current: &Version, next: &Version) {
        self.json["version"] = Value::String(next.to_string());

        if let Err(_) = std::fs::write(&self.path, self.json.to_string()) {
            VersionerError::SaveFailed.fatal();
        }

        if current < next {
            log::info(&format!("{} → {}", current, next));
        } else {
            log::info(&format!("{} ← {}", current, next));
        }
    }

    pub(crate) fn update(&mut self, args: Args) {
        let current = Version::parse(&self);
        let next = current.bump(&args);

        if current == next {
            VersionerError::NoAction(current).fatal();
        }

        let tag = format!("v{}", next.to_string());

        if Git::tag_exists(&tag) {
            VersionerError::TagExists(tag).fatal()
        }

        self.save(&current, &next);

        if let Err(_) = Git::commit(&args.command.message(), &tag) {
            log::info("git step failed, reverting package.json");
            self.save(&next, &current);
        }

        let push_message = format!("push with: 'git push -u origin main && git push origin {tag}'");

        if Git::has_origin_remote() {
            if let Err(_) = Git::push(&tag) {
                let message = format!("push failed but changes exist locally, {}", push_message);

                log::info(&message);
            }
        } else {
            log::info(&format!("no 'origin' remote configured, {}", push_message));
        }
    }
}
