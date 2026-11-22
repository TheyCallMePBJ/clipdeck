//! Pins: entries kept at the top of the list.
//!
//! This is the only state clipdeck owns. Each pin's content is copied into
//! clipdeck's own storage, so a pin survives cliphist trimming old entries
//! and `cliphist wipe`. Pins are keyed by a blake3 hash of the content
//! rather than by cliphist id, because cliphist gives an entry a new id
//! whenever it is copied again.
//!
//! On disk (by default under `~/.local/share/clipdeck/`):
//!
//! - `pins.toml`: the list of pins (hash, label, kind, ...)
//! - `pins/<hash>`: each pin's content, byte for byte

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::entry::{Entry, Kind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    /// blake3 hash of the content, in hex. Also the content's file name.
    pub hash: String,
    /// What the list shows and search matches.
    pub label: String,
    pub kind: Kind,
    /// cliphist's preview text for this content. Used to spot the pinned
    /// content in history cheaply: only entries with the same preview need
    /// decoding and hashing to confirm.
    pub preview: String,
    /// Seconds since the Unix epoch.
    pub pinned_at: u64,
    /// The cliphist id this content had when last seen in history. Lets
    /// clipdeck recognise it without decoding anything. Updated when the
    /// content is copied again and gets a new id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cliphist_id: Option<u64>,
}

/// The layout of `pins.toml`: a list of `[[pin]]` tables.
#[derive(Default, Serialize, Deserialize)]
struct PinsFile {
    #[serde(default, rename = "pin")]
    pins: Vec<Pin>,
}

pub struct Pins {
    dir: PathBuf,
    /// Newest pin first.
    pins: Vec<Pin>,
}

impl Pins {
    /// Loads the pins stored in `dir`. A missing `pins.toml` just means no
    /// pins yet. A file that can't be read or parsed is an error, so that
    /// clipdeck never overwrites (and loses) pins it failed to understand.
    pub fn load(dir: PathBuf) -> Result<Self> {
        let path = dir.join("pins.toml");
        let pins = match fs::read_to_string(&path) {
            Ok(text) => {
                let file: PinsFile = toml::from_str(&text)
                    .with_context(|| format!("can't parse {}", path.display()))?;
                file.pins
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).with_context(|| format!("can't read {}", path.display())),
        };
        Ok(Self { dir, pins })
    }

    pub fn all(&self) -> &[Pin] {
        &self.pins
    }

    pub fn get(&self, hash: &str) -> Option<&Pin> {
        self.pins.iter().find(|p| p.hash == hash)
    }

    pub fn content_path(&self, hash: &str) -> PathBuf {
        self.dir.join("pins").join(hash)
    }

    pub fn content(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.content_path(hash);
        fs::read(&path).with_context(|| format!("can't read pinned content {}", path.display()))
    }

    /// Pins `content`, described by the history `entry` it came from.
    /// Returns the pin's hash. Pinning the same content twice is a no-op.
    pub fn add(&mut self, content: &[u8], entry: &Entry) -> Result<String> {
        let hash = hash(content);
        if self.get(&hash).is_some() {
            return Ok(hash);
        }
        // Content first: a pin in the list must always have its file.
        write_atomically(&self.content_path(&hash), content)?;
        let pinned_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.pins.insert(
            0,
            Pin {
                hash: hash.clone(),
                label: entry.label.clone(),
                kind: entry.kind.clone(),
                preview: entry.preview.clone(),
                pinned_at,
                cliphist_id: Some(entry.id),
            },
        );
        if let Err(e) = self.save() {
            // Undo, so memory matches what's on disk.
            self.pins.remove(0);
            let _ = fs::remove_file(self.content_path(&hash));
            return Err(e);
        }
        Ok(hash)
    }

    /// Unpins and deletes the stored content. Returns the removed pin.
    pub fn remove(&mut self, hash: &str) -> Result<Pin> {
        let Some(index) = self.pins.iter().position(|p| p.hash == hash) else {
            bail!("not pinned");
        };
        let pin = self.pins.remove(index);
        if let Err(e) = self.save() {
            self.pins.insert(index, pin);
            return Err(e);
        }
        // Only after the list no longer mentions it.
        let _ = fs::remove_file(self.content_path(hash));
        Ok(pin)
    }

    /// Remembers which cliphist id the pinned content currently has.
    /// Saving is best-effort: if it fails, the id is just looked up again
    /// (by decoding) next time.
    pub fn set_cliphist_id(&mut self, hash: &str, id: u64) {
        let Some(pin) = self.pins.iter_mut().find(|p| p.hash == hash) else {
            return;
        };
        if pin.cliphist_id != Some(id) {
            pin.cliphist_id = Some(id);
            let _ = self.save();
        }
    }

    fn save(&self) -> Result<()> {
        let file = PinsFile {
            pins: self.pins.clone(),
        };
        let text = toml::to_string_pretty(&file).context("can't serialize pins")?;
        write_atomically(&self.dir.join("pins.toml"), text.as_bytes())
    }
}

/// blake3 hash of `content`, in hex.
pub fn hash(content: &[u8]) -> String {
    blake3::hash(content).to_hex().to_string()
}

/// Writes via a temporary file and a rename, so a crash can never leave a
/// half-written file under the real name.
fn write_atomically(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("can't create {}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, data).with_context(|| format!("can't write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("can't write {}", path.display()))
}

/// `~/.local/share/clipdeck` (respecting `XDG_DATA_HOME`).
pub fn default_dir() -> Option<PathBuf> {
    Some(
        directories::ProjectDirs::from("", "", "clipdeck")?
            .data_dir()
            .to_owned(),
    )
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A temporary directory, deleted when this value is dropped.
    pub struct TempDir(pub PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A fresh, empty directory for one test.
    pub fn temp_dir() -> TempDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("clipdeck-pins-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn entry(id: u64, text: &str) -> Entry {
        Entry::parse(&format!("{id}\t{text}")).unwrap()
    }

    #[test]
    fn add_stores_content_and_survives_reload() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let mut pins = Pins::load(dir.clone()).unwrap();
        let hash = pins.add(b"hello world", &entry(7, "hello world")).unwrap();

        assert_eq!(pins.content(&hash).unwrap(), b"hello world");
        let reloaded = Pins::load(dir.clone()).unwrap();
        assert_eq!(reloaded.all(), pins.all());
        let pin = &reloaded.all()[0];
        assert_eq!(pin.label, "hello world");
        assert_eq!(pin.cliphist_id, Some(7));
    }

    #[test]
    fn newest_pin_first_and_no_duplicates() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let mut pins = Pins::load(dir.clone()).unwrap();
        pins.add(b"a", &entry(1, "a")).unwrap();
        pins.add(b"b", &entry(2, "b")).unwrap();
        pins.add(b"a", &entry(3, "a")).unwrap();
        let labels: Vec<_> = pins.all().iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["b", "a"]);
    }

    #[test]
    fn remove_deletes_content() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let mut pins = Pins::load(dir.clone()).unwrap();
        let hash = pins.add(b"bytes", &entry(1, "bytes")).unwrap();
        let removed = pins.remove(&hash).unwrap();
        assert_eq!(removed.hash, hash);
        assert!(pins.all().is_empty());
        assert!(!pins.content_path(&hash).exists());
        assert!(Pins::load(dir.clone()).unwrap().all().is_empty());
    }

    #[test]
    fn binary_content_round_trips() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let mut pins = Pins::load(dir.clone()).unwrap();
        let image = Entry::parse("3\t[[ binary data 4 B png 1x1 ]]").unwrap();
        let bytes = [0u8, 255, 10, 13];
        let hash = pins.add(&bytes, &image).unwrap();
        assert_eq!(pins.content(&hash).unwrap(), bytes);
        let reloaded = Pins::load(dir.clone()).unwrap();
        assert!(matches!(
            reloaded.all()[0].kind,
            Kind::Image { width: 1, .. }
        ));
    }

    #[test]
    fn cliphist_id_updates_are_saved() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let mut pins = Pins::load(dir.clone()).unwrap();
        let hash = pins.add(b"x", &entry(1, "x")).unwrap();
        pins.set_cliphist_id(&hash, 42);
        assert_eq!(
            Pins::load(dir.clone()).unwrap().all()[0].cliphist_id,
            Some(42)
        );
    }

    #[test]
    fn unreadable_file_is_an_error_not_an_empty_list() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        fs::write(dir.join("pins.toml"), "this is [not toml").unwrap();
        assert!(Pins::load(dir.clone()).is_err());
    }

    #[test]
    fn file_format_is_readable() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let mut pins = Pins::load(dir.clone()).unwrap();
        pins.add(b"https://example.com", &entry(1, "https://example.com"))
            .unwrap();
        let text = fs::read_to_string(dir.join("pins.toml")).unwrap();
        assert!(text.contains("[[pin]]"), "{text}");
        assert!(text.contains("label = \"https://example.com\""), "{text}");
        assert!(text.contains("type = \"url\""), "{text}");
    }
}
