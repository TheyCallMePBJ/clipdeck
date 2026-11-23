//! Loading previews: decoding the highlighted entry in a background thread,
//! so the UI never waits on cliphist or on decoding a large image.
//!
//! Text is shown in full (up to a size cap). Images are shrunk to a
//! thumbnail, which is cached on disk by content hash so a big screenshot
//! is only decoded once.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;

use anyhow::{Context, Result};
use image::{DynamicImage, ImageFormat};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use crate::entry::{Entry, Kind};
use crate::event::Event;
use crate::item::ItemId;
use crate::sanitize::sanitize;

/// At most this much of an entry is shown as text.
const MAX_TEXT_BYTES: usize = 256 * 1024;
/// At most this many lines are shown.
const MAX_TEXT_LINES: usize = 5_000;
/// Thumbnails are at most this many pixels on their longest side. Large
/// enough for a sharp preview pane on a HiDPI screen.
const THUMB_MAX_PX: u32 = 1024;
/// Keep at most this many thumbnails on disk; older ones are removed.
const THUMB_CACHE_FILES: usize = 200;

/// A finished preview, ready to draw.
pub enum Content {
    Text {
        lines: Vec<String>,
        /// Size of the whole entry, which may be more than is shown.
        bytes: usize,
        truncated: bool,
    },
    /// The image, ready for the terminal's graphics protocol. Holds a
    /// thumbnail; the original size is in the entry's `Kind::Image`.
    Image(Box<StatefulProtocol>),
    Error(String),
}

/// A preview for one row, sent back from the loader thread.
pub struct Loaded {
    pub id: ItemId,
    pub content: Content,
}

/// What to load a preview for.
pub struct Request {
    pub id: ItemId,
    pub kind: Kind,
    pub source: Source,
}

/// Where a preview's bytes come from.
pub enum Source {
    /// A history entry, decoded by cliphist.
    History(Entry),
    /// A file, such as a pin's stored content.
    File(PathBuf),
}

/// Handle for sending work to the loader thread.
pub struct Loader {
    requests: Sender<Request>,
}

impl Loader {
    /// Starts the loader thread. `decode` fetches a history entry's bytes
    /// (in the app, `cliphist decode`); results are sent to `events`.
    ///
    /// `decode` is a generic closure rather than a `Cliphist` so tests can
    /// pass a fake. `Send + 'static` is what moving it into a new thread
    /// requires: it must be safe to hand to another thread, and must not
    /// borrow anything that could disappear while the thread runs.
    pub fn spawn<D>(
        decode: D,
        picker: Picker,
        thumbs_dir: Option<PathBuf>,
        events: Sender<Event>,
    ) -> Self
    where
        D: Fn(&Entry) -> Result<Vec<u8>> + Send + 'static,
    {
        let (requests, incoming) = mpsc::channel::<Request>();
        thread::spawn(move || {
            // `recv` returns an error once the `Loader` (the sender) is
            // dropped, which ends the thread.
            while let Ok(mut request) = incoming.recv() {
                // When the selection moves faster than previews load, only
                // the newest request matters; skip the stale ones.
                while let Ok(newer) = incoming.try_recv() {
                    request = newer;
                }
                let content = load(&request, &decode, &picker, thumbs_dir.as_deref());
                let loaded = Loaded {
                    id: request.id,
                    content,
                };
                if events.send(Event::Preview(loaded)).is_err() {
                    break;
                }
            }
        });
        Self { requests }
    }

    pub fn request(&self, request: Request) {
        // Can only fail if the thread has stopped, and then there's
        // nothing useful to do about it here.
        let _ = self.requests.send(request);
    }
}

fn load<D>(request: &Request, decode: &D, picker: &Picker, thumbs_dir: Option<&Path>) -> Content
where
    D: Fn(&Entry) -> Result<Vec<u8>>,
{
    let bytes = match &request.source {
        Source::History(entry) => decode(entry),
        Source::File(path) => {
            fs::read(path).with_context(|| format!("can't read {}", path.display()))
        }
    };
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(e) => return Content::Error(format!("{e:#}")),
    };
    match request.kind {
        Kind::Text | Kind::Url => text_content(&bytes),
        Kind::Image { .. } => match thumbnail(&bytes, thumbs_dir) {
            Ok(image) => Content::Image(Box::new(picker.new_resize_protocol(image))),
            Err(e) => Content::Error(format!("{e:#}")),
        },
    }
}

/// Turns an entry's bytes into displayable lines, capped in size.
pub fn text_content(bytes: &[u8]) -> Content {
    let mut truncated = bytes.len() > MAX_TEXT_BYTES;
    let shown = &bytes[..bytes.len().min(MAX_TEXT_BYTES)];
    // Not all clipboard text is valid UTF-8; invalid bytes become `�`.
    let text = sanitize(&String::from_utf8_lossy(shown), "    ");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    if lines.len() > MAX_TEXT_LINES {
        lines.truncate(MAX_TEXT_LINES);
        truncated = true;
    }
    Content::Text {
        lines,
        bytes: bytes.len(),
        truncated,
    }
}

/// Returns a thumbnail of the image in `bytes`, from the disk cache when
/// possible. The cache is best-effort: if it can't be read or written, the
/// image is simply decoded again next time.
pub fn thumbnail(bytes: &[u8], cache_dir: Option<&Path>) -> Result<DynamicImage> {
    let cache_path = cache_dir.map(|dir| dir.join(format!("{}.png", blake3::hash(bytes).to_hex())));

    if let Some(path) = &cache_path
        && let Ok(image) = image::open(path)
    {
        // Pruning removes the least recently *modified* files, so mark this
        // one as just used. Otherwise a thumbnail viewed every day would be
        // pruned as if it were old.
        let _ = fs::File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()));
        return Ok(image);
    }

    let image = image::load_from_memory(bytes).context("can't decode image")?;
    let thumb = if image.width() > THUMB_MAX_PX || image.height() > THUMB_MAX_PX {
        image.thumbnail(THUMB_MAX_PX, THUMB_MAX_PX)
    } else {
        image
    };

    if let (Some(path), Some(dir)) = (&cache_path, cache_dir) {
        // Write to a temporary name and rename, so a crash mid-write can't
        // leave a half-written file under the real name.
        let tmp = path.with_extension("png.tmp");
        if thumb.save_with_format(&tmp, ImageFormat::Png).is_ok() && fs::rename(&tmp, path).is_ok()
        {
            prune(dir, THUMB_CACHE_FILES);
        } else {
            let _ = fs::remove_file(&tmp);
        }
    }
    Ok(thumb)
}

/// Deletes the oldest files in `dir` until at most `keep` remain.
fn prune(dir: &Path, keep: usize) {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = read_dir
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort(); // oldest first
    for (_, path) in &files[..files.len() - keep] {
        let _ = fs::remove_file(path);
    }
}

/// `~/.cache/clipdeck/thumbs` (respecting `XDG_CACHE_HOME`), created if
/// needed. `None` if it can't be created, which disables the cache.
pub fn default_thumbs_dir() -> Option<PathBuf> {
    let dir = directories::ProjectDirs::from("", "", "clipdeck")?
        .cache_dir()
        .join("thumbs");
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use image::{Rgb, RgbImage};
    use std::io::Cursor;
    use std::time::Duration;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_pixel(width, height, Rgb([200, 50, 50]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn text_lines(content: Content) -> (Vec<String>, bool) {
        match content {
            Content::Text {
                lines, truncated, ..
            } => (lines, truncated),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn text_is_split_into_sanitized_lines() {
        let (lines, truncated) = text_lines(text_content(b"one\r\ntwo\tx\n\x1b[0m"));
        assert_eq!(lines, ["one", "two    x", "␛[0m"]);
        assert!(!truncated);
    }

    #[test]
    fn invalid_utf8_is_shown_lossily() {
        let (lines, _) = text_lines(text_content(b"ok \xff\xfe"));
        assert_eq!(lines, ["ok ��"]);
    }

    #[test]
    fn huge_text_is_truncated() {
        let many_lines = "x\n".repeat(MAX_TEXT_LINES + 10);
        let (lines, truncated) = text_lines(text_content(many_lines.as_bytes()));
        assert_eq!(lines.len(), MAX_TEXT_LINES);
        assert!(truncated);

        let huge = vec![b'a'; MAX_TEXT_BYTES + 1];
        match text_content(&huge) {
            Content::Text {
                bytes, truncated, ..
            } => {
                assert_eq!(bytes, MAX_TEXT_BYTES + 1);
                assert!(truncated);
            }
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn large_images_are_shrunk_and_cached() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let bytes = png(2048, 1024);

        let thumb = thumbnail(&bytes, Some(&dir)).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (1024, 512));
        let cached: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(cached.len(), 1);

        // A second call is served from the cache (same result, no new file).
        let again = thumbnail(&bytes, Some(&dir)).unwrap();
        assert_eq!((again.width(), again.height()), (1024, 512));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn using_a_cached_thumbnail_keeps_it_from_being_pruned() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let bytes = png(20, 20);
        thumbnail(&bytes, Some(&dir)).unwrap();
        let path = fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        let long_ago = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();

        thumbnail(&bytes, Some(&dir)).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(
            modified > long_ago,
            "a cache hit marks the file as recently used"
        );
    }

    #[test]
    fn small_images_keep_their_size_and_cache_is_optional() {
        let thumb = thumbnail(&png(40, 30), None).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (40, 30));
    }

    #[test]
    fn undecodable_image_is_an_error() {
        assert!(thumbnail(b"not an image", None).is_err());
    }

    #[test]
    fn prune_keeps_newest_files() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        for i in 0..5 {
            fs::write(dir.join(format!("{i}.png")), b"x").unwrap();
            // Distinct modification times.
            std::thread::sleep(Duration::from_millis(10));
        }
        prune(&dir, 2);
        let mut left: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["3.png", "4.png"]);
    }

    fn history_request(line: &str) -> Request {
        let entry = Entry::parse(line).unwrap();
        Request {
            id: ItemId::History(entry.id),
            kind: entry.kind.clone(),
            source: Source::History(entry),
        }
    }

    #[test]
    fn loader_reads_files_for_pins() {
        let tmp = temp_dir();
        let dir = tmp.0.clone();
        let path = dir.join("content");
        fs::write(&path, b"pinned text").unwrap();
        let (events, received) = mpsc::channel();
        let loader = Loader::spawn(
            |_: &Entry| anyhow::bail!("cliphist isn't used for pins"),
            Picker::halfblocks(),
            None,
            events,
        );
        loader.request(Request {
            id: ItemId::Pin("abc".into()),
            kind: Kind::Text,
            source: Source::File(path),
        });
        match received.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Preview(Loaded {
                id: ItemId::Pin(hash),
                content: Content::Text { lines, .. },
            }) => {
                assert_eq!(hash, "abc");
                assert_eq!(lines, ["pinned text"]);
            }
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn loader_sends_previews_back() {
        let (events, received) = mpsc::channel();
        let loader = Loader::spawn(
            |e: &Entry| Ok(format!("full text of {}", e.id).into_bytes()),
            Picker::halfblocks(),
            None,
            events,
        );
        loader.request(history_request("7\tfull text…"));
        match received.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Preview(Loaded {
                id: ItemId::History(7),
                content: Content::Text { lines, .. },
            }) => assert_eq!(lines, ["full text of 7"]),
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn loader_reports_decode_errors() {
        let (events, received) = mpsc::channel();
        let loader = Loader::spawn(
            |_: &Entry| anyhow::bail!("decode failed"),
            Picker::halfblocks(),
            None,
            events,
        );
        loader.request(history_request("1\tx"));
        match received.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Preview(Loaded {
                content: Content::Error(msg),
                ..
            }) => assert_eq!(msg, "decode failed"),
            _ => panic!("unexpected event"),
        }
    }
}
