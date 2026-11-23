//! Pasting into the previous window with `wtype`.
//!
//! clipdeck can't paste while its own window has focus. So after putting the
//! entry on the clipboard it starts `wtype` in the background and exits.
//! `wtype` waits a moment (while the popup closes and focus goes back to the
//! window underneath), then presses the paste shortcut there.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// How long `wtype` waits before pressing the keys, in milliseconds. Long
/// enough for the popup to close and focus to move.
const DELAY_MS: u32 = 200;

/// The default paste shortcut. Most terminals use `ctrl+shift+v` instead.
pub const DEFAULT_KEYS: &str = "ctrl+v";

/// Turns a shortcut like `ctrl+shift+v` or `shift+Insert` into `wtype`
/// arguments: press each modifier, type the key, release the modifiers.
pub fn wtype_args(keys: &str) -> Result<Vec<String>> {
    let parts: Vec<&str> = keys.split('+').map(str::trim).collect();
    let Some((key, modifiers)) = parts.split_last() else {
        bail!("empty paste shortcut");
    };
    if key.is_empty() {
        bail!("paste shortcut {keys:?} has no key");
    }
    let mut modifier_names = Vec::new();
    for modifier in modifiers {
        let name = match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => "ctrl",
            "shift" => "shift",
            "alt" => "alt",
            "altgr" => "altgr",
            "super" | "logo" | "win" | "meta" => "logo",
            _ => bail!("unknown modifier {modifier:?} in paste shortcut {keys:?}"),
        };
        modifier_names.push(name);
    }

    let mut args = vec!["-s".to_owned(), DELAY_MS.to_string()];
    for name in &modifier_names {
        args.extend(["-M".to_owned(), (*name).to_owned()]);
    }
    if key.chars().count() == 1 {
        // A single character is typed as text (with the modifiers held).
        args.push((*key).to_owned());
    } else {
        // Anything longer is a named key, such as `Insert` or `Return`.
        args.extend(["-k".to_owned(), (*key).to_owned()]);
    }
    for name in modifier_names.iter().rev() {
        args.extend(["-m".to_owned(), (*name).to_owned()]);
    }
    Ok(args)
}

/// Starts `wtype` in the background to press `keys` once clipdeck's
/// window has closed. Doesn't wait for it.
pub fn spawn(keys: &str) -> Result<()> {
    Command::new("wtype")
        .args(wtype_args(keys)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group, like wl-copy, so it isn't killed when the
        // terminal window closes.
        .process_group(0)
        .spawn()
        .context("failed to run `wtype` (is it installed?)")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(keys: &str) -> String {
        wtype_args(keys).unwrap().join(" ")
    }

    #[test]
    fn single_key_with_modifiers() {
        assert_eq!(args("ctrl+v"), "-s 200 -M ctrl v -m ctrl");
        assert_eq!(
            args("Ctrl+Shift+v"),
            "-s 200 -M ctrl -M shift v -m shift -m ctrl"
        );
    }

    #[test]
    fn named_keys() {
        assert_eq!(args("shift+Insert"), "-s 200 -M shift -k Insert -m shift");
        assert_eq!(args("super+v"), "-s 200 -M logo v -m logo");
    }

    #[test]
    fn bad_shortcuts_are_errors() {
        assert!(wtype_args("").is_err());
        assert!(wtype_args("ctrl+").is_err());
        assert!(wtype_args("hyper+v").is_err());
    }
}
