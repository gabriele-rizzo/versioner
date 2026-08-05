use std::{
    ffi::OsStr,
    io,
    process::{Command, ExitStatus, Stdio},
};

use crate::log;

pub(crate) struct Git;

impl Git {
    fn command<I, S>(args: I, show_output: bool) -> io::Result<ExitStatus>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut base = Command::new("git");
        let command = match show_output {
            true => base.args(args),
            false => base.args(args).stdout(Stdio::null()).stderr(Stdio::null()),
        };

        command.status()
    }

    pub(crate) fn tag_exists(tag: &str) -> bool {
        let args = ["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")];

        Git::command(args, false).is_ok_and(|status| status.success())
    }

    pub(crate) fn has_origin_remote() -> bool {
        Git::command(["remote", "get-url", "origin"], false).is_ok_and(|status| status.success())
    }

    pub(crate) fn commit(message: &str, tag: &str) -> io::Result<()> {
        Git::command(["add", "-A"], false)?;
        Git::command(["commit", "-m", message], false)?;
        Git::command(["tag", "-a", tag, "-m", message], false)?;

        Ok(log::info(&format!("successfully committed '{}'", tag)))
    }

    pub(crate) fn push(tag: &str) -> io::Result<()> {
        Git::command(["push", "-u", "origin", "main"], false)?;
        Git::command(["push", "origin", tag], false)?;

        Ok(log::info(&format!("pushed main and '{}' to origin", tag)))
    }
}
