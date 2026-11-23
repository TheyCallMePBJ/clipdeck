//! Live reload: watching cliphist's database for changes.
//!
//! clipdeck never reads the database itself (its format is internal to
//! cliphist). It only watches the file to learn *when* to run
//! `cliphist list` again.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::event::Event;

/// One copy can touch the database several times in quick succession, so
/// changes are reported only after the file has been quiet this long.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// Works out which database cliphist uses, following cliphist's own rules
/// (highest priority first):
///
/// 1. the `-db-path` flag (clipdeck's `--db-path`)
/// 2. a `db-path <path>` line in cliphist's config file
/// 3. the `CLIPHIST_DB_PATH` environment variable
/// 4. `$XDG_CACHE_HOME/cliphist/db`, or `~/.cache/cliphist/db`
///
/// `env` looks up environment variables. It's a parameter (rather than
/// calling `std::env::var` directly) so tests can supply their own.
pub fn resolve_db_path(
    flag: Option<&Path>,
    env: impl Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    if let Some(path) = flag {
        return Some(path.to_owned());
    }
    let home = env("HOME").map(PathBuf::from);
    // Treat empty variables as unset, like most XDG-aware programs do.
    let var = |name: &str| env(name).filter(|v| !v.is_empty());

    let config = var("CLIPHIST_CONFIG_PATH").map(PathBuf::from).or_else(|| {
        let config_home = var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| Some(home.as_ref()?.join(".config")))?;
        Some(config_home.join("cliphist").join("config"))
    });
    if let Some(path) = config.and_then(|c| db_path_from_config(&c)) {
        return Some(path);
    }
    if let Some(path) = var("CLIPHIST_DB_PATH") {
        return Some(PathBuf::from(path));
    }
    let cache_home = var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(home?.join(".cache")))?;
    Some(cache_home.join("cliphist").join("db"))
}

/// Reads `db-path <path>` from a cliphist config file, if present.
fn db_path_from_config(config: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(config).ok()?;
    text.lines().find_map(|line| {
        let (key, value) = line.trim().split_once(char::is_whitespace)?;
        (key == "db-path").then(|| PathBuf::from(value.trim()))
    })
}

/// Starts watching `db_path` and sends `Event::HistoryChanged` to `events`
/// whenever cliphist writes to it.
///
/// Watching stops when the returned watcher is dropped, so the caller must
/// keep it alive for as long as it wants updates.
pub fn watch(db_path: &Path, events: Sender<Event>) -> Result<RecommendedWatcher> {
    // Watch the directory rather than the file itself: a file watch would
    // be lost if the database were ever replaced or created after we start.
    let dir = match db_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let file_name = db_path
        .file_name()
        .context("database path has no file name")?
        .to_owned();

    let (changes, quiet) = mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        let ours = event
            .paths
            .iter()
            .any(|p| p.file_name() == Some(file_name.as_os_str()));
        if ours && is_write(&event.kind) {
            let _ = changes.send(());
        }
    })?;
    watcher
        .watch(dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("can't watch {}", dir.display()))?;

    // Debounce: after the first change, wait until there has been no
    // change for `DEBOUNCE`, then report once.
    thread::spawn(move || {
        while quiet.recv().is_ok() {
            loop {
                match quiet.recv_timeout(DEBOUNCE) {
                    Ok(()) => continue,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            if events.send(Event::HistoryChanged).is_err() {
                return;
            }
        }
    });
    Ok(watcher)
}

/// Whether an event means the file's contents may have changed.
///
/// Opening, reading and closing are ignored on purpose: clipdeck's own
/// `cliphist list` opens the database, and reacting to that would reload
/// forever. Metadata changes (like access times) are ignored for the same
/// reason.
fn is_write(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Any)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::collections::HashMap;
    use std::sync::mpsc::Receiver;

    fn resolve(vars: &[(&str, &str)]) -> Option<PathBuf> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        resolve_db_path(None, |name| vars.get(name).cloned())
    }

    #[test]
    fn flag_wins() {
        let path = resolve_db_path(Some(Path::new("/x/db")), |_| Some("/y".into()));
        assert_eq!(path, Some(PathBuf::from("/x/db")));
    }

    #[test]
    fn default_is_in_cache_dir() {
        assert_eq!(
            resolve(&[("HOME", "/home/u")]),
            Some(PathBuf::from("/home/u/.cache/cliphist/db"))
        );
        assert_eq!(
            resolve(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "/c")]),
            Some(PathBuf::from("/c/cliphist/db"))
        );
        // Empty means unset.
        assert_eq!(
            resolve(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "")]),
            Some(PathBuf::from("/home/u/.cache/cliphist/db"))
        );
    }

    #[test]
    fn env_var_beats_default() {
        assert_eq!(
            resolve(&[("HOME", "/home/u"), ("CLIPHIST_DB_PATH", "/e/db")]),
            Some(PathBuf::from("/e/db"))
        );
    }

    #[test]
    fn config_file_beats_env_var() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let config = dir.join("config");
        fs::write(&config, "# comment\nmax-items 500\ndb-path /from/config\n").unwrap();
        let config = config.to_str().unwrap();

        assert_eq!(
            resolve(&[
                ("CLIPHIST_CONFIG_PATH", config),
                ("CLIPHIST_DB_PATH", "/e/db")
            ]),
            Some(PathBuf::from("/from/config"))
        );

        // Found via XDG_CONFIG_HOME as well.
        fs::create_dir_all(dir.join("cliphist")).unwrap();
        fs::write(dir.join("cliphist/config"), "db-path   /xdg/db  \n").unwrap();
        assert_eq!(
            resolve(&[("XDG_CONFIG_HOME", dir.to_str().unwrap())]),
            Some(PathBuf::from("/xdg/db"))
        );
    }

    #[test]
    fn config_without_db_path_is_ignored() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let config = dir.join("config");
        // cliphist doesn't accept key=value, so neither do we.
        fs::write(&config, "db-path=/nope\n").unwrap();
        assert_eq!(
            resolve(&[
                ("HOME", "/h"),
                ("CLIPHIST_CONFIG_PATH", config.to_str().unwrap())
            ]),
            Some(PathBuf::from("/h/.cache/cliphist/db"))
        );
    }

    fn changed_within(events: &Receiver<Event>, timeout: Duration) -> bool {
        matches!(events.recv_timeout(timeout), Ok(Event::HistoryChanged))
    }

    #[test]
    fn reports_writes_once_and_ignores_reads_and_other_files() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let db = dir.join("db");
        fs::write(&db, b"v1").unwrap();

        let (events, received) = mpsc::channel();
        let _watcher = watch(&db, events).unwrap();

        // Several quick writes are reported as one change.
        for i in 0..5 {
            fs::write(&db, format!("v{i}")).unwrap();
        }
        assert!(changed_within(&received, Duration::from_secs(2)));
        assert!(!changed_within(&received, Duration::from_millis(300)));

        // Reading the file, or writing a different file, is not a change.
        fs::read(&db).unwrap();
        fs::write(dir.join("other"), b"x").unwrap();
        assert!(!changed_within(&received, Duration::from_millis(300)));
    }

    /// Runs real cliphist commands against a watched database.
    #[test]
    #[ignore = "needs the cliphist binary"]
    fn cliphist_store_is_a_change_but_list_is_not() {
        use crate::app::Backend;
        use crate::cliphist::Cliphist;
        use std::io::Write;
        use std::process::{Command, Stdio};

        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let db = dir.join("db");
        let store = |text: &str| {
            let mut child = Command::new("cliphist")
                .arg("-db-path")
                .arg(&db)
                .arg("store")
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
            assert!(child.wait().unwrap().success());
        };
        store("first");

        let (events, received) = mpsc::channel();
        let _watcher = watch(&db, events).unwrap();

        let backend = Cliphist::new(Some(db.clone()));
        backend.list().unwrap();
        assert!(!changed_within(&received, Duration::from_millis(500)));

        store("second");
        assert!(changed_within(&received, Duration::from_secs(2)));
    }
}
