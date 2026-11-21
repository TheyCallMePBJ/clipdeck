//! Thin wrapper around the `cliphist` and `wl-copy` commands.
//! cliphist is the source of truth; nothing here caches history.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::entry::{Entry, parse_list};

/// Runs `cliphist list` and parses the result.
pub fn list() -> Result<Vec<Entry>> {
    let out = Command::new("cliphist")
        .arg("list")
        .output()
        .context("failed to run `cliphist` (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "cliphist list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(parse_list(&String::from_utf8_lossy(&out.stdout)))
}

/// Runs `cliphist decode` with the entry's original line on stdin.
pub fn decode(entry: &Entry) -> Result<Vec<u8>> {
    run_with_stdin("cliphist", &["decode"], entry.raw.as_bytes())
}

/// Decodes the entry and hands it to `wl-copy`.
///
/// `wl-copy` forks a background process that serves the selection. It is put
/// in its own process group so that closing the terminal window (which
/// signals our process group) doesn't take the clipboard with it.
pub fn copy(entry: &Entry) -> Result<()> {
    let data = decode(entry)?;
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

fn run_with_stdin(cmd: &str, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run `{cmd}`"))?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "{cmd} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}
