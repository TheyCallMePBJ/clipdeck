//! Thin wrapper around the `cliphist` and `wl-copy` commands.
//! cliphist is the source of truth; nothing here caches history.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::app::Backend;
use crate::entry::{Entry, parse_list};

/// The real backend: runs the `cliphist` binary.
pub struct Cliphist {
    /// Passed to cliphist as `-db-path` when set; otherwise cliphist's default is used.
    db_path: Option<PathBuf>,
}

impl Cliphist {
    pub fn new(db_path: Option<PathBuf>) -> Self {
        Self { db_path }
    }

    /// Builds `cliphist [-db-path <path>] <subcommand>`.
    fn command(&self, subcommand: &str) -> Command {
        let mut cmd = Command::new("cliphist");
        if let Some(path) = &self.db_path {
            cmd.arg("-db-path").arg(path);
        }
        cmd.arg(subcommand);
        cmd
    }

    /// Runs `cliphist decode` with the entry's original line on stdin.
    pub fn decode(&self, entry: &Entry) -> Result<Vec<u8>> {
        run(self.command("decode"), Some(entry.raw.as_bytes()))
    }

    /// Decodes the entry and hands it to `wl-copy`.
    ///
    /// `wl-copy` forks a background process that serves the selection. It is
    /// put in its own process group so that closing the terminal window (which
    /// signals our process group) doesn't take the clipboard with it.
    pub fn copy(&self, entry: &Entry) -> Result<()> {
        let data = self.decode(entry)?;
        let mut child = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .context("failed to run `wl-copy` (is wl-clipboard installed?)")?;
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(&data)?;
        // The foreground wl-copy exits once it has read stdin and forked.
        let status = child.wait()?;
        if !status.success() {
            bail!("wl-copy exited with {status}");
        }
        Ok(())
    }
}

impl Backend for Cliphist {
    fn list(&self) -> Result<Vec<Entry>> {
        let out = run(self.command("list"), None)?;
        Ok(parse_list(&String::from_utf8_lossy(&out)))
    }

    fn delete(&mut self, entry: &Entry) -> Result<()> {
        // `cliphist delete` reads whole lines from stdin.
        let line = format!("{}\n", entry.raw);
        run(self.command("delete"), Some(line.as_bytes()))?;
        Ok(())
    }
}

/// Runs a command, optionally feeding it `input` on stdin, and returns its stdout.
fn run(mut cmd: Command, input: Option<&[u8]>) -> Result<Vec<u8>> {
    let name = describe(&cmd);
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to run `{name}` (is cliphist installed?)"))?;
    if let Some(input) = input {
        // The handle is dropped at the end of this statement, which closes
        // the pipe and tells the child that input is finished.
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(input)?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "`{name}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// "cliphist -db-path /x delete", for error messages.
fn describe(cmd: &Command) -> String {
    let mut parts = vec![cmd.get_program().to_string_lossy().into_owned()];
    parts.extend(cmd.get_args().map(|a| a.to_string_lossy().into_owned()));
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the real cliphist against a throwaway database.
    /// Ignored by default because it needs cliphist installed:
    /// run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs the cliphist binary"]
    fn list_and_delete_against_real_cliphist() {
        let dir = std::env::temp_dir().join(format!("clipdeck-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut backend = Cliphist::new(Some(dir.join("db")));

        for text in ["first", "second", "third"] {
            run(backend.command("store"), Some(text.as_bytes())).unwrap();
        }
        let entries = backend.list().unwrap();
        let previews: Vec<_> = entries.iter().map(|e| e.preview.as_str()).collect();
        assert_eq!(previews, ["third", "second", "first"]);

        assert_eq!(backend.decode(&entries[1]).unwrap(), b"second");

        backend.delete(&entries[1]).unwrap();
        let previews: Vec<_> = backend
            .list()
            .unwrap()
            .into_iter()
            .map(|e| e.preview)
            .collect();
        assert_eq!(previews, ["third", "first"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
