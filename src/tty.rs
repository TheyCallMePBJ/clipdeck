//! Pointing the TUI at the terminal when stdin or stdout is a pipe.
//!
//! With `--pick`, clipdeck prints the chosen entry to stdout, so a script
//! can run `clipdeck --pick | something`. But ratatui draws to stdout, and
//! the image-protocol query writes to stdout and reads from stdin. With
//! stdout piped, the whole UI would go down the pipe.
//!
//! [`UseTerminal`] fixes that the way fzf does: while it's alive, file
//! descriptors 0 and 1 (stdin and stdout) point at `/dev/tty`, the
//! terminal the process is running in. Dropping it puts the originals back,
//! so the picked entry goes to the real stdout.

use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};

use anyhow::{Context, Result};

pub struct UseTerminal {
    /// Copies of the original descriptors, with the descriptor number each
    /// one belongs back on. Only those that were redirected.
    saved: Vec<(OwnedFd, RawFd)>,
}

impl UseTerminal {
    /// Redirects stdin and stdout to `/dev/tty`, each only if it isn't a
    /// terminal already.
    pub fn redirect() -> Result<Self> {
        let mut guard = Self { saved: Vec::new() };
        let stdin = io::stdin();
        let stdout = io::stdout();
        if stdin.is_terminal() && stdout.is_terminal() {
            return Ok(guard);
        }
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("can't open /dev/tty to draw the UI")?;
        if !stdin.is_terminal() {
            guard.replace(stdin.as_fd().try_clone_to_owned()?, 0, &tty)?;
        }
        if !stdout.is_terminal() {
            stdout.lock().flush()?;
            guard.replace(stdout.as_fd().try_clone_to_owned()?, 1, &tty)?;
        }
        Ok(guard)
    }

    /// Keeps `original` to restore later, then points `fd` at the terminal.
    fn replace(&mut self, original: OwnedFd, fd: RawFd, tty: &File) -> Result<()> {
        dup2(tty.as_raw_fd(), fd)?;
        self.saved.push((original, fd));
        Ok(())
    }
}

impl Drop for UseTerminal {
    fn drop(&mut self) {
        // Anything still buffered was meant for the terminal.
        let _ = io::stdout().flush();
        for (original, fd) in self.saved.drain(..) {
            let _ = dup2(original.as_raw_fd(), fd);
        }
    }
}

/// Makes descriptor `to` refer to the same file as `from`.
fn dup2(from: RawFd, to: RawFd) -> io::Result<()> {
    // SAFETY: dup2 only operates on descriptor numbers and has no memory
    // safety requirements. `from` is open (we hold the file it belongs to)
    // and `to` is 0 or 1, which std's stdin/stdout use only by number, so
    // swapping what they point at doesn't invalidate anything.
    if unsafe { libc::dup2(from, to) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
