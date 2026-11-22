//! Application state and key handling, independent of the terminal.
//! Keeping this free of drawing code means it can be unit-tested with a
//! fake backend.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::entry::Entry;
use crate::item::{Item, ItemId};
use crate::pins::{self, Pins};
use crate::preview::{Content, Loaded, Request, Source};
use crate::search::{Match, Searcher};

/// How many loaded previews to keep, so moving back and forth over the
/// same entries doesn't decode them again.
const PREVIEW_CACHE: usize = 16;

/// Where history comes from and where changes go. The real implementation
/// is `cliphist::Cliphist`; tests use an in-memory fake.
pub trait Backend {
    fn list(&self) -> Result<Vec<Entry>>;
    fn decode(&self, entry: &Entry) -> Result<Vec<u8>>;
    fn delete(&mut self, entry: &Entry) -> Result<()>;
    /// Adds `data` to history as the newest entry.
    fn store(&mut self, data: &[u8]) -> Result<()>;
}

/// What the main loop should do after a key press.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Copy(CopySource),
}

/// What to put on the clipboard.
#[derive(Debug, PartialEq, Eq)]
pub enum CopySource {
    /// A history entry, decoded by cliphist.
    Entry(Entry),
    /// Content clipdeck already has, such as a pin's.
    Bytes(Vec<u8>),
}

/// A one-line message shown in the status bar until the next key press.
#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Info(String),
    /// Waiting for the user to confirm something, e.g. a second `d`.
    Prompt(String),
    Error(String),
}

/// Which keys do what. In `Normal`, letters are commands; in `Search`,
/// they are typed into the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
}

pub struct App<B: Backend> {
    backend: B,
    /// `None` if the pins file couldn't be loaded; pinning is then disabled
    /// rather than risking overwriting pins clipdeck didn't understand.
    pins: Option<Pins>,
    /// The last result of `backend.list()`. Never edited directly: after any
    /// change, list again so the UI matches what cliphist holds.
    entries: Vec<Entry>,
    /// The rows of the list: pins first, then history entries that aren't
    /// pinned. Rebuilt from `pins` and `entries` after every change.
    items: Vec<Item>,
    /// For each pin whose content is currently in history, that history
    /// entry (by pin hash).
    pins_in_history: HashMap<String, Entry>,
    /// Content hashes of history entries that were decoded to compare them
    /// with pins, by cliphist id. Ids never change content, so this stays
    /// valid and saves decoding them again.
    hashes: HashMap<u64, String>,
    /// The items that match `query`, in display order. With an empty
    /// query this is every item.
    matches: Vec<Match>,
    /// Index into `matches`. Always valid when `matches` is non-empty.
    selected: usize,
    mode: Mode,
    query: String,
    searcher: Searcher,
    status: Option<Status>,
    /// Number of visible list rows, set by the UI each frame. Used for paging.
    page_height: usize,
    /// Scroll position of the list widget. Owned here so it persists
    /// between frames. Its selection is kept in sync with `selected`.
    list_state: ListState,
    /// The item that was armed for deletion by the previous key press.
    /// Only the very next key can confirm it.
    pending_delete: Option<ItemId>,
    /// Loaded previews, most recently loaded first.
    previews: VecDeque<(ItemId, Content)>,
    /// The item whose preview was last requested from the loader.
    preview_requested: Option<ItemId>,
    /// Preview scroll position, as (item, first visible line). Tied to the
    /// item so moving the selection starts the next preview at the top.
    preview_scroll: Option<(ItemId, usize)>,
}

impl<B: Backend> App<B> {
    pub fn new(backend: B, pins: Option<Pins>) -> Result<Self> {
        let entries = backend.list()?;
        let mut app = Self {
            backend,
            pins,
            entries,
            items: Vec::new(),
            pins_in_history: HashMap::new(),
            hashes: HashMap::new(),
            matches: Vec::new(),
            selected: 0,
            mode: Mode::Normal,
            query: String::new(),
            searcher: Searcher::new(),
            status: None,
            page_height: 10,
            list_state: ListState::default(),
            pending_delete: None,
            previews: VecDeque::new(),
            preview_requested: None,
            preview_scroll: None,
        };
        app.rebuild_items();
        app.refilter_from_top();
        Ok(app)
    }

    /// All rows, before filtering.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> Option<&Item> {
        self.matches
            .get(self.selected)
            .map(|m| &self.items[m.index])
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    pub fn show_error(&mut self, message: String) {
        self.status = Some(Status::Error(message));
    }

    /// Everything the UI needs to draw the list: the items, which of them
    /// to show (in order, with highlights), and the list widget's state with
    /// the selection synced.
    ///
    /// Returning these from one method lets the caller borrow several fields
    /// at once. Calling `items()` and then using `list_state` mutably would
    /// not compile, because `items()` borrows all of `self`.
    pub fn list_view(&mut self) -> (&[Item], &[Match], &mut ListState) {
        self.list_state.select(Some(self.selected));
        (&self.items, &self.matches, &mut self.list_state)
    }

    pub fn set_page_height(&mut self, rows: usize) {
        self.page_height = rows.max(1);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        self.status = None;
        // Taking it here means any key other than a second `d` cancels the
        // pending delete.
        let pending_delete = self.pending_delete.take();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if ctrl && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        // Shift+arrows scroll the preview in both modes.
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Down => return self.scroll_preview(1),
                KeyCode::Up => return self.scroll_preview(-1),
                _ => {}
            }
        }
        match self.mode {
            Mode::Normal => self.normal_key(key.code, ctrl, pending_delete),
            Mode::Search => self.search_key(key, ctrl),
        }
    }

    fn normal_key(&mut self, code: KeyCode, ctrl: bool, pending_delete: Option<ItemId>) -> Action {
        let half_page = (self.page_height / 2).max(1) as isize;

        // Arms are tried top to bottom, so the Ctrl combinations must come
        // before the plain letters they share a key with.
        match code {
            KeyCode::Char('d') if ctrl => self.move_by(half_page),
            KeyCode::Char('u') if ctrl => self.move_by(-half_page),

            KeyCode::Char('q') => return Action::Quit,
            // Esc first clears an active filter, and only quits after that.
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.refilter_keeping_selection();
            }
            KeyCode::Esc => return Action::Quit,
            KeyCode::Char('f') | KeyCode::Char('/') => self.mode = Mode::Search,
            KeyCode::Char('j') => self.move_by(1),
            KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Char('g') => self.selected = 0,
            KeyCode::Char('G') => self.selected = self.last_index(),
            KeyCode::Char('J') => return self.scroll_preview(1),
            KeyCode::Char('K') => return self.scroll_preview(-1),

            KeyCode::Enter => return self.copy_selected(),
            KeyCode::Char('p') => self.toggle_pin(),
            KeyCode::Char('d') | KeyCode::Delete => self.press_delete(pending_delete),
            KeyCode::Char('r') => match self.reload() {
                Ok(()) => self.status = Some(Status::Info("reloaded".into())),
                Err(e) => self.show_error(format!("{e:#}")),
            },
            other => self.navigate(other),
        }
        Action::None
    }

    fn search_key(&mut self, key: KeyEvent, ctrl: bool) -> Action {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // Leave search mode but keep the filter, so the results can be
            // browsed with the normal keys (and entries deleted).
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => return self.copy_selected(),
            KeyCode::Backspace => {
                if self.query.pop().is_some() {
                    self.refilter_from_top();
                } else {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.refilter_from_top();
            }
            KeyCode::Char('w') if ctrl => {
                delete_last_word(&mut self.query);
                self.refilter_from_top();
            }
            KeyCode::Char('n') if ctrl => self.move_by(1),
            KeyCode::Char('p') if ctrl => self.move_by(-1),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.query.push(c);
                self.refilter_from_top();
            }
            other => self.navigate(other),
        }
        Action::None
    }

    /// Keys that move the selection the same way in both modes.
    fn navigate(&mut self, code: KeyCode) {
        let page = self.page_height as isize;
        match code {
            KeyCode::Down => self.move_by(1),
            KeyCode::Up => self.move_by(-1),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.last_index(),
            _ => {}
        }
    }

    fn copy_selected(&mut self) -> Action {
        let source = match self.selected() {
            None => return Action::None,
            Some(Item::History(entry)) => Ok(CopySource::Entry(entry.clone())),
            // A pin's content may no longer be in cliphist at all, so it's
            // copied from clipdeck's own copy.
            Some(Item::Pinned(pin)) => self
                .pins
                .as_ref()
                .context("pins are unavailable")
                .and_then(|pins| pins.content(&pin.hash))
                .map(CopySource::Bytes),
        };
        match source {
            Ok(source) => Action::Copy(source),
            Err(e) => {
                self.show_error(format!("{e:#}"));
                Action::None
            }
        }
    }

    /// Moves the selection by `delta` rows, stopping at either end.
    fn move_by(&mut self, delta: isize) {
        let target = self.selected as isize + delta;
        self.selected = target.clamp(0, self.last_index() as isize) as usize;
    }

    fn last_index(&self) -> usize {
        self.matches.len().saturating_sub(1)
    }

    // --- building the list ------------------------------------------------

    /// Rebuilds `items` from `pins` and `entries`: pins first, then the
    /// history entries whose content isn't pinned.
    fn rebuild_items(&mut self) {
        self.pins_in_history.clear();
        let mut pinned_ids = HashSet::new();
        let mut items = Vec::new();

        // Only fields are used here (no `&self` methods), so `pins` can be
        // borrowed mutably while `entries`, `backend` and `hashes` are used.
        if let Some(pins) = self.pins.as_mut() {
            for entry in &self.entries {
                if let Some(hash) = find_pin(pins, entry, &self.backend, &mut self.hashes) {
                    pins.set_cliphist_id(&hash, entry.id);
                    pinned_ids.insert(entry.id);
                    self.pins_in_history.insert(hash, entry.clone());
                }
            }
            items.extend(pins.all().iter().cloned().map(Item::Pinned));
        }
        items.extend(
            self.entries
                .iter()
                .filter(|e| !pinned_ids.contains(&e.id))
                .cloned()
                .map(Item::History),
        );
        self.items = items;
    }

    /// Re-runs the search after the query changed, selecting the best match.
    fn refilter_from_top(&mut self) {
        self.refilter();
        self.selected = 0;
    }

    /// Re-runs the search, keeping the same item selected if it is still
    /// shown. Otherwise the selection stays at the same position.
    fn refilter_keeping_selection(&mut self) {
        let selected = self.selected().map(Item::id);
        self.refilter();
        self.reselect(selected);
    }

    fn refilter(&mut self) {
        // `self.searcher` is borrowed mutably and `self.items` immutably
        // at the same time. That's allowed because they are different
        // fields, accessed directly rather than through methods on `self`.
        let labels = self.items.iter().map(Item::label);
        self.matches = self.searcher.filter(&self.query, labels);
    }

    /// Selects the item with this id if it is shown; otherwise keeps the
    /// current position, clamped to the list.
    fn reselect(&mut self, id: Option<ItemId>) {
        let items = &self.items;
        if let Some(index) =
            id.and_then(|id| self.matches.iter().position(|m| items[m.index].id() == id))
        {
            self.selected = index;
        }
        self.selected = self.selected.min(self.last_index());
    }

    /// Lists again and re-applies the current filter, keeping the same
    /// item selected if it still exists (new entries may have appeared
    /// above it).
    pub fn reload(&mut self) -> Result<()> {
        let selected = self.selected().map(Item::id);
        self.reload_selecting(selected)
    }

    /// Lists again and selects `id` if it's shown.
    fn reload_selecting(&mut self, id: Option<ItemId>) -> Result<()> {
        self.entries = self.backend.list()?;
        self.rebuild_items();
        self.refilter();
        self.reselect(id);
        Ok(())
    }

    /// Called when cliphist's history changed outside clipdeck. Reloads
    /// quietly: unlike `r`, a successful reload shows no message, and any
    /// message already showing (like a delete prompt) stays.
    pub fn history_changed(&mut self) {
        if let Err(e) = self.reload() {
            self.show_error(format!("{e:#}"));
        }
    }

    // --- pins ---------------------------------------------------------------

    fn pins_mut(&mut self) -> Result<&mut Pins> {
        self.pins.as_mut().context("pins are unavailable")
    }

    fn toggle_pin(&mut self) {
        let result = match self.selected().cloned() {
            None => return,
            Some(Item::History(entry)) => self.pin(&entry),
            Some(Item::Pinned(pin)) => self.unpin(&pin.hash),
        };
        self.status = Some(match result {
            Ok(message) => Status::Info(message.into()),
            Err(e) => Status::Error(format!("{e:#}")),
        });
    }

    fn pin(&mut self, entry: &Entry) -> Result<&'static str> {
        let content = self.backend.decode(entry)?;
        let hash = self.pins_mut()?.add(&content, entry)?;
        self.hashes.insert(entry.id, hash.clone());
        // Follow the entry to its new place at the top.
        self.reload_selecting(Some(ItemId::Pin(hash)))?;
        Ok("pinned")
    }

    fn unpin(&mut self, hash: &str) -> Result<&'static str> {
        let in_history = self.pins_in_history.get(hash).map(|e| e.id);
        if in_history.is_none() {
            // cliphist no longer has it (trimmed or wiped), so put it back
            // rather than losing it. This happens first: if it fails, the
            // pin is kept.
            let content = self.pins_mut()?.content(hash)?;
            self.backend.store(&content)?;
        }
        self.pins_mut()?.remove(hash)?;
        // Follow the entry back into history when we know where it is.
        self.reload_selecting(in_history.map(ItemId::History))?;
        Ok(if in_history.is_some() {
            "unpinned"
        } else {
            "unpinned and put back into history"
        })
    }

    // --- delete -------------------------------------------------------------

    /// First press arms the delete; a second press on the same item,
    /// straight after, performs it.
    fn press_delete(&mut self, pending: Option<ItemId>) {
        let Some(id) = self.selected().map(Item::id) else {
            return;
        };
        // Comparing ids (not just "was the last key d") guards against the
        // list changing between the two presses, e.g. on reload.
        if pending.as_ref() == Some(&id) {
            self.delete_selected();
        } else {
            self.pending_delete = Some(id);
            self.status = Some(Status::Prompt("press d again to delete".into()));
        }
    }

    fn delete_selected(&mut self) {
        // Clone so we stop borrowing `self.items` before calling methods
        // that need `&mut self`.
        let result = match self.selected().cloned() {
            None => return,
            Some(Item::History(entry)) => self.backend.delete(&entry).map(|()| "deleted 1 entry"),
            Some(Item::Pinned(pin)) => self.delete_pin(&pin.hash).map(|()| "deleted pinned entry"),
        };
        let message = match result {
            Ok(message) => message,
            Err(e) => {
                self.show_error(format!("{e:#}"));
                return;
            }
        };
        // The deleted item is gone, so reload keeps the position and the
        // next item moves up into the selection.
        self.status = Some(match self.reload() {
            Ok(()) => Status::Info(message.into()),
            Err(e) => Status::Error(format!("{e:#}")),
        });
    }

    /// Deleting a pin removes it entirely: the pin and the history copy.
    fn delete_pin(&mut self, hash: &str) -> Result<()> {
        if let Some(entry) = self.pins_in_history.get(hash).cloned() {
            self.backend.delete(&entry)?;
        }
        self.pins_mut()?.remove(hash)?;
        Ok(())
    }

    // --- previews -----------------------------------------------------------

    /// The preview that should be loaded next, if any. The main loop calls
    /// this after every event and sends the result to the loader.
    pub fn preview_request(&mut self) -> Option<Request> {
        let item = self.selected()?;
        let id = item.id();
        let loaded = self.previews.iter().any(|(i, _)| *i == id);
        if loaded || self.preview_requested.as_ref() == Some(&id) {
            return None;
        }
        let source = match item {
            Item::History(entry) => Source::History(entry.clone()),
            Item::Pinned(pin) => Source::File(self.pins.as_ref()?.content_path(&pin.hash)),
        };
        let request = Request {
            id: id.clone(),
            kind: item.kind().clone(),
            source,
        };
        self.preview_requested = Some(id);
        Some(request)
    }

    /// Stores a preview from the loader.
    pub fn preview_loaded(&mut self, loaded: Loaded) {
        if self.preview_requested.as_ref() == Some(&loaded.id) {
            self.preview_requested = None;
        }
        self.previews.retain(|(id, _)| *id != loaded.id);
        self.previews.push_front((loaded.id, loaded.content));
        self.previews.truncate(PREVIEW_CACHE);
    }

    /// The selected item, its preview if loaded, and the scroll position.
    /// The preview is `&mut` because image previews keep encoding state
    /// that the widget updates while drawing.
    pub fn preview_view(&mut self) -> Option<(&Item, Option<&mut Content>, usize)> {
        // Direct field access (not `self.selected()`) so that `items` and
        // `previews` can be borrowed at the same time.
        let item = &self.items[self.matches.get(self.selected)?.index];
        let id = item.id();
        let content = self
            .previews
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, content)| content);
        let scroll = match &self.preview_scroll {
            Some((i, line)) if *i == id => *line,
            _ => 0,
        };
        Some((item, content, scroll))
    }

    fn scroll_preview(&mut self, delta: isize) -> Action {
        let Some((item, content, scroll)) = self.preview_view() else {
            return Action::None;
        };
        let id = item.id();
        let max = match content {
            Some(Content::Text { lines, .. }) => lines.len().saturating_sub(1),
            _ => 0,
        };
        let line = (scroll as isize + delta).clamp(0, max as isize) as usize;
        self.preview_scroll = Some((id, line));
        Action::None
    }
}

/// Returns the hash of the pin whose content `entry` holds, if any.
///
/// Cheap checks first, so that normally nothing is decoded: the cliphist
/// id remembered for the pin, then the preview text. Only an entry whose
/// preview equals a pin's is decoded and hashed, to be sure (different
/// content can share a preview, e.g. long text with the same beginning).
fn find_pin<B: Backend>(
    pins: &Pins,
    entry: &Entry,
    backend: &B,
    hashes: &mut HashMap<u64, String>,
) -> Option<String> {
    // The preview is compared too, in case the id came from a different
    // cliphist database (e.g. one used with --db-path).
    let by_id = pins
        .all()
        .iter()
        .find(|p| p.cliphist_id == Some(entry.id) && p.preview == entry.preview);
    if let Some(pin) = by_id {
        return Some(pin.hash.clone());
    }
    if !pins.all().iter().any(|p| p.preview == entry.preview) {
        return None;
    }
    let hash = match hashes.get(&entry.id) {
        Some(hash) => hash.clone(),
        None => {
            let hash = pins::hash(&backend.decode(entry).ok()?);
            hashes.insert(entry.id, hash.clone());
            hash
        }
    };
    pins.get(&hash).map(|pin| pin.hash.clone())
}

/// Removes the last word and any spaces after it, like Ctrl+W in a shell.
fn delete_last_word(query: &mut String) {
    let trimmed = query.trim_end().len();
    query.truncate(trimmed);
    let cut = query.rfind(' ').map_or(0, |i| i + 1);
    query.truncate(cut);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pins::tests::{TempDir, temp_dir};
    use anyhow::bail;

    /// In-memory backend. Tests edit `entries` directly to simulate
    /// cliphist changing underneath the app.
    struct Fake {
        entries: Vec<Entry>,
        /// Content for an id. Without an override, an entry's content is
        /// its preview text.
        contents: HashMap<u64, Vec<u8>>,
        fail_delete: bool,
        /// Keeps the pins directory alive (and deletes it afterwards).
        _pins_dir: Option<TempDir>,
    }

    impl Fake {
        fn new(entries: Vec<Entry>) -> Self {
            Self {
                entries,
                contents: HashMap::new(),
                fail_delete: false,
                _pins_dir: None,
            }
        }
    }

    impl Backend for Fake {
        fn list(&self) -> Result<Vec<Entry>> {
            Ok(self.entries.clone())
        }

        fn decode(&self, entry: &Entry) -> Result<Vec<u8>> {
            Ok(self
                .contents
                .get(&entry.id)
                .cloned()
                .unwrap_or_else(|| entry.preview.clone().into_bytes()))
        }

        fn delete(&mut self, entry: &Entry) -> Result<()> {
            if self.fail_delete {
                bail!("delete failed");
            }
            self.entries.retain(|e| e.raw != entry.raw);
            Ok(())
        }

        fn store(&mut self, data: &[u8]) -> Result<()> {
            let id = self.entries.iter().map(|e| e.id).max().unwrap_or(0) + 1;
            let text = String::from_utf8_lossy(data);
            self.entries.insert(0, entry(id, &text));
            Ok(())
        }
    }

    fn entry(id: u64, text: &str) -> Entry {
        Entry::parse(&format!("{id}\t{text}")).unwrap()
    }

    /// Entries newest first, like cliphist: the first text gets the highest id.
    fn entries(texts: &[&str]) -> Vec<Entry> {
        let n = texts.len() as u64;
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| entry(n - i as u64, t))
            .collect()
    }

    /// An app without pins (as if the pins file couldn't be loaded).
    fn app_with(texts: &[&str]) -> App<Fake> {
        App::new(Fake::new(entries(texts)), None).unwrap()
    }

    /// An app with an empty pin store in a temporary directory.
    fn pinning_app(texts: &[&str]) -> App<Fake> {
        let dir = temp_dir();
        let pins = Pins::load(dir.0.clone()).unwrap();
        let mut fake = Fake::new(entries(texts));
        fake._pins_dir = Some(dir);
        App::new(fake, Some(pins)).unwrap()
    }

    /// An app over entries with ids 5, 4, 3, 2, 1 (newest first, like cliphist).
    fn app() -> App<Fake> {
        app_with(&["e5", "e4", "e3", "e2", "e1"])
    }

    fn press(app: &mut App<Fake>, code: KeyCode) -> Action {
        app.handle_key(KeyEvent::from(code))
    }

    fn ctrl(app: &mut App<Fake>, c: char) -> Action {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn type_str(app: &mut App<Fake>, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn selected_id(app: &App<Fake>) -> u64 {
        match app.selected() {
            Some(Item::History(entry)) => entry.id,
            other => panic!("expected a history entry, got {other:?}"),
        }
    }

    fn history_len(app: &App<Fake>) -> usize {
        app.entries.len()
    }

    /// Labels of the visible rows, pinned ones marked with "*".
    fn shown(app: &mut App<Fake>) -> Vec<String> {
        let (items, matches, _) = app.list_view();
        matches
            .iter()
            .map(|m| {
                let item = &items[m.index];
                let mark = if item.is_pinned() { "*" } else { "" };
                format!("{mark}{}", item.label())
            })
            .collect()
    }

    // --- navigation -------------------------------------------------------

    #[test]
    fn movement_stops_at_both_ends() {
        let mut app = app();
        press(&mut app, KeyCode::Up);
        assert_eq!(app.selected_index(), 0);
        for _ in 0..10 {
            press(&mut app, KeyCode::Char('j'));
        }
        assert_eq!(app.selected_index(), 4);
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.selected_index(), 0);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.selected_index(), 4);
    }

    #[test]
    fn paging_uses_page_height() {
        let mut app = app();
        app.set_page_height(2);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.selected_index(), 2);
        ctrl(&mut app, 'u');
        assert_eq!(app.selected_index(), 1);
    }

    #[test]
    fn ctrl_d_pages_instead_of_deleting() {
        let mut app = app();
        app.set_page_height(4);
        ctrl(&mut app, 'd');
        assert_eq!(history_len(&app), 5);
        assert_eq!(app.selected_index(), 2);
    }

    #[test]
    fn enter_copies_selected_entry() {
        let mut app = app();
        press(&mut app, KeyCode::Down);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Action::Copy(CopySource::Entry(entry(4, "e4")))
        );
    }

    // --- delete -----------------------------------------------------------

    #[test]
    fn delete_selects_next_entry() {
        let mut app = app();
        press(&mut app, KeyCode::Down); // id 4
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        let ids: Vec<_> = app.entries.iter().map(|e| e.id).collect();
        assert_eq!(ids, [5, 3, 2, 1]);
        assert_eq!(selected_id(&app), 3);
        assert_eq!(app.status(), Some(&Status::Info("deleted 1 entry".into())));
    }

    #[test]
    fn single_press_only_asks_for_confirmation() {
        let mut app = app();
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(history_len(&app), 5);
        assert_eq!(
            app.status(),
            Some(&Status::Prompt("press d again to delete".into()))
        );
    }

    #[test]
    fn d_and_delete_can_be_mixed() {
        let mut app = app();
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Delete);
        assert_eq!(history_len(&app), 4);
    }

    #[test]
    fn other_key_cancels_pending_delete() {
        let mut app = app();
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(history_len(&app), 5);
        // Moving back doesn't revive the old pending delete either.
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(history_len(&app), 5);
    }

    #[test]
    fn pending_delete_is_tied_to_the_entry() {
        let mut app = app();
        press(&mut app, KeyCode::Char('d')); // arms id 5
        // Simulate the selected entry vanishing between the two presses.
        app.backend.entries.remove(0);
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(history_len(&app), 4);
        assert!(matches!(app.status(), Some(Status::Prompt(_))));
    }

    #[test]
    fn deleting_last_entry_moves_selection_up() {
        let mut app = app();
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Delete);
        assert_eq!(selected_id(&app), 2);
    }

    #[test]
    fn failed_delete_reports_error_and_keeps_list() {
        let mut app = app();
        app.backend.fail_delete = true;
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(history_len(&app), 5);
        assert!(matches!(app.status(), Some(Status::Error(_))));
        // The message is cleared by the next key press.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.status(), None);
    }

    // --- reload -----------------------------------------------------------

    #[test]
    fn reload_follows_selected_entry_when_new_entries_arrive() {
        let mut app = app();
        press(&mut app, KeyCode::Down); // id 4, index 1
        app.backend.entries.insert(0, entry(6, "new"));
        app.reload().unwrap();
        assert_eq!(selected_id(&app), 4);
        assert_eq!(app.selected_index(), 2);
    }

    #[test]
    fn history_change_reloads_quietly_and_keeps_prompt() {
        let mut app = app();
        press(&mut app, KeyCode::Char('d')); // arms id 5
        app.backend.entries.insert(0, entry(6, "copied elsewhere"));
        app.history_changed();

        assert_eq!(history_len(&app), 6);
        assert_eq!(selected_id(&app), 5, "selection stays on the same entry");
        assert!(matches!(app.status(), Some(Status::Prompt(_))));
        // The armed delete still applies to the same entry.
        press(&mut app, KeyCode::Char('d'));
        assert!(!app.entries.iter().any(|e| e.id == 5));
    }

    #[test]
    fn history_change_keeps_search() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "git");
        app.backend.entries.insert(0, entry(9, "git stash"));
        app.history_changed();
        assert_eq!(app.mode(), Mode::Search);
        assert_eq!(app.query(), "git");
        assert_eq!(app.match_count(), 3);
    }

    #[test]
    fn empty_history_is_harmless() {
        let mut app = app_with(&[]);
        assert_eq!(press(&mut app, KeyCode::Down), Action::None);
        assert_eq!(press(&mut app, KeyCode::Char('d')), Action::None);
        assert_eq!(press(&mut app, KeyCode::Char('p')), Action::None);
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        assert_eq!(app.selected(), None);
    }

    // --- search -----------------------------------------------------------

    fn history() -> App<Fake> {
        app_with(&["git push", "hello world", "git pull", "cargo test"])
    }

    #[test]
    fn f_starts_search_and_typing_filters() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.mode(), Mode::Search);
        type_str(&mut app, "git");
        assert_eq!(app.query(), "git");
        assert_eq!(shown(&mut app), ["git push", "git pull"]);
    }

    #[test]
    fn slash_also_starts_search() {
        let mut app = history();
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.mode(), Mode::Search);
    }

    #[test]
    fn command_letters_are_typed_in_search_mode() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        // q, d, j, f, p would be commands in normal mode.
        assert_eq!(press(&mut app, KeyCode::Char('q')), Action::None);
        type_str(&mut app, "djfp");
        assert_eq!(app.query(), "qdjfp");
        assert_eq!(history_len(&app), 4);
    }

    #[test]
    fn enter_in_search_copies_best_match() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "pull");
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Action::Copy(CopySource::Entry(entry(2, "git pull")))
        );
    }

    #[test]
    fn typing_resets_selection_to_top() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "g");
        press(&mut app, KeyCode::Down);
        type_str(&mut app, "i");
        assert_eq!(app.selected_index(), 0);
    }

    #[test]
    fn arrows_and_ctrl_n_p_move_in_search_mode() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        press(&mut app, KeyCode::Down);
        ctrl(&mut app, 'n');
        assert_eq!(app.selected_index(), 2);
        ctrl(&mut app, 'p');
        assert_eq!(app.selected_index(), 1);
    }

    #[test]
    fn esc_leaves_search_then_clears_filter_then_quits() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "git");
        press(&mut app, KeyCode::Down); // "git pull"

        // 1st Esc: back to normal mode, filter still applied.
        assert_eq!(press(&mut app, KeyCode::Esc), Action::None);
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.match_count(), 2);

        // 2nd Esc: filter cleared, still on the same entry.
        assert_eq!(press(&mut app, KeyCode::Esc), Action::None);
        assert_eq!(app.query(), "");
        assert_eq!(app.match_count(), 4);
        assert_eq!(app.selected().unwrap().label(), "git pull");

        // 3rd Esc: quit.
        assert_eq!(press(&mut app, KeyCode::Esc), Action::Quit);
    }

    #[test]
    fn backspace_edits_query_and_exits_when_empty() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "gx");
        assert_eq!(app.match_count(), 0);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.query(), "g");
        assert_eq!(app.match_count(), 3);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.mode(), Mode::Search);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.mode(), Mode::Normal);
    }

    #[test]
    fn ctrl_w_and_ctrl_u_edit_query() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "git pu");
        ctrl(&mut app, 'w');
        assert_eq!(app.query(), "git ");
        ctrl(&mut app, 'u');
        assert_eq!(app.query(), "");
        assert_eq!(app.mode(), Mode::Search);
    }

    #[test]
    fn delete_inside_filtered_view_keeps_filter() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "git");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.query(), "git");
        assert_eq!(shown(&mut app), ["git pull"]);
        assert_eq!(history_len(&app), 3);
    }

    #[test]
    fn reload_reapplies_filter() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "git");
        app.backend.entries.insert(0, entry(9, "git stash"));
        app.reload().unwrap();
        assert_eq!(shown(&mut app).len(), 3);
    }

    #[test]
    fn no_matches_is_harmless() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "zzz");
        assert_eq!(app.selected(), None);
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_index(), 0);
    }

    // --- pins ---------------------------------------------------------------

    /// Pins the entry with this label.
    fn pin_label(app: &mut App<Fake>, label: &str) {
        let index = app
            .matches
            .iter()
            .position(|m| app.items[m.index].label() == label)
            .unwrap();
        app.selected = index;
        press(app, KeyCode::Char('p'));
        assert_eq!(app.status(), Some(&Status::Info("pinned".into())));
    }

    #[test]
    fn pinning_moves_entry_to_top_without_duplicate() {
        let mut app = pinning_app(&["a", "b", "c"]);
        press(&mut app, KeyCode::Down); // "b"
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(shown(&mut app), ["*b", "a", "c"]);
        assert_eq!(app.selected_index(), 0, "selection follows the pin");
        assert_eq!(app.status(), Some(&Status::Info("pinned".into())));
        // cliphist still has it; it's just shown once, as a pin.
        assert_eq!(history_len(&app), 3);
    }

    #[test]
    fn newest_pin_is_first() {
        let mut app = pinning_app(&["a", "b", "c"]);
        pin_label(&mut app, "c");
        pin_label(&mut app, "a");
        assert_eq!(shown(&mut app), ["*a", "*c", "b"]);
    }

    #[test]
    fn pins_survive_wipe_and_restart() {
        let mut app = pinning_app(&["a", "b", "c"]);
        pin_label(&mut app, "b");

        app.backend.entries.clear(); // cliphist wipe
        app.reload().unwrap();
        assert_eq!(shown(&mut app), ["*b"]);

        // A new App with the same pins directory still has the pin.
        let pins = Pins::load(app.backend._pins_dir.as_ref().unwrap().0.clone()).unwrap();
        let mut restarted = App::new(Fake::new(vec![]), Some(pins)).unwrap();
        assert_eq!(shown(&mut restarted), ["*b"]);
    }

    #[test]
    fn recopied_pin_is_recognised_by_content() {
        let mut app = pinning_app(&["a", "b", "c"]);
        pin_label(&mut app, "b"); // id 2
        // Copying "b" again: cliphist moves it to the top with a new id.
        app.backend.entries.retain(|e| e.id != 2);
        app.backend.entries.insert(0, entry(10, "b"));
        app.history_changed();
        assert_eq!(shown(&mut app), ["*b", "a", "c"]);
        // The new id is remembered, so it won't need decoding next time.
        let pin = &app.pins.as_ref().unwrap().all()[0];
        assert_eq!(pin.cliphist_id, Some(10));
    }

    #[test]
    fn same_preview_but_different_content_is_not_hidden() {
        let mut app = pinning_app(&["same", "x"]);
        pin_label(&mut app, "same");
        // A new entry whose preview matches the pin, but whose full
        // content differs (previews are cut off at 100 characters).
        app.backend.entries.insert(0, entry(7, "same"));
        app.backend.contents.insert(7, b"same, but longer".to_vec());
        app.history_changed();
        assert_eq!(shown(&mut app), ["*same", "same", "x"]);
    }

    #[test]
    fn remembered_id_from_another_database_is_not_trusted() {
        let mut app = pinning_app(&["a", "b"]); // b is id 1
        pin_label(&mut app, "b");
        // Same id, different entry (as with a different --db-path).
        app.backend.entries = vec![entry(1, "zzz")];
        app.history_changed();
        assert_eq!(shown(&mut app), ["*b", "zzz"]);
    }

    #[test]
    fn unpinning_returns_entry_to_history() {
        let mut app = pinning_app(&["a", "b", "c"]);
        pin_label(&mut app, "b");
        press(&mut app, KeyCode::Char('p')); // on the pin
        assert_eq!(app.status(), Some(&Status::Info("unpinned".into())));
        assert_eq!(shown(&mut app), ["a", "b", "c"]);
        assert_eq!(selected_id(&app), 2, "selection follows it back");
        assert_eq!(history_len(&app), 3, "nothing was stored");
    }

    #[test]
    fn unpinning_after_wipe_puts_content_back_into_history() {
        let mut app = pinning_app(&["a", "b"]);
        pin_label(&mut app, "b");
        app.backend.entries.clear();
        app.history_changed();
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(
            app.status(),
            Some(&Status::Info("unpinned and put back into history".into()))
        );
        assert_eq!(shown(&mut app), ["b"]);
        assert!(app.pins.as_ref().unwrap().all().is_empty());
    }

    #[test]
    fn deleting_a_pin_removes_it_and_its_history_copy() {
        let mut app = pinning_app(&["a", "b", "c"]);
        pin_label(&mut app, "b");
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(
            app.status(),
            Some(&Status::Info("deleted pinned entry".into()))
        );
        assert_eq!(shown(&mut app), ["a", "c"]);
        assert!(app.pins.as_ref().unwrap().all().is_empty());
    }

    #[test]
    fn enter_on_pin_copies_stored_content() {
        let mut app = pinning_app(&["a", "b"]);
        pin_label(&mut app, "b");
        app.backend.entries.clear(); // not in cliphist any more
        app.history_changed();
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Action::Copy(CopySource::Bytes(b"b".to_vec()))
        );
    }

    #[test]
    fn search_includes_pins() {
        let mut app = pinning_app(&["git push", "hello", "git pull"]);
        pin_label(&mut app, "git pull");
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "git");
        assert_eq!(shown(&mut app), ["*git pull", "git push"]);
    }

    #[test]
    fn pinning_without_pin_store_shows_error() {
        let mut app = app();
        press(&mut app, KeyCode::Char('p'));
        assert!(
            matches!(app.status(), Some(Status::Error(msg)) if msg.contains("unavailable")),
            "{:?}",
            app.status()
        );
        assert_eq!(app.match_count(), 5);
    }

    #[test]
    fn pin_preview_is_loaded_from_its_file() {
        let mut app = pinning_app(&["a", "b"]);
        pin_label(&mut app, "b");
        let request = app.preview_request().unwrap();
        assert!(matches!(request.id, ItemId::Pin(_)));
        match request.source {
            Source::File(path) => assert_eq!(std::fs::read(path).unwrap(), b"b"),
            Source::History(_) => panic!("expected the pin's file"),
        }
    }

    // --- previews ---------------------------------------------------------

    fn text_preview(app: &mut App<Fake>, id: u64, text: &str) {
        app.preview_loaded(Loaded {
            id: ItemId::History(id),
            content: crate::preview::text_content(text.as_bytes()),
        });
    }

    fn shift(app: &mut App<Fake>, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::SHIFT));
    }

    fn preview_scroll(app: &mut App<Fake>) -> usize {
        app.preview_view().unwrap().2
    }

    fn requested(app: &mut App<Fake>) -> Option<ItemId> {
        app.preview_request().map(|r| r.id)
    }

    #[test]
    fn preview_is_requested_once_per_entry() {
        let mut app = app();
        assert_eq!(requested(&mut app), Some(ItemId::History(5)));
        // Already requested: don't ask again while it loads.
        assert_eq!(requested(&mut app), None);
        press(&mut app, KeyCode::Down);
        assert_eq!(requested(&mut app), Some(ItemId::History(4)));
    }

    #[test]
    fn loaded_preview_is_shown_for_selected_entry() {
        let mut app = app();
        app.preview_request();
        assert!(app.preview_view().unwrap().1.is_none(), "not loaded yet");
        text_preview(&mut app, 5, "full text");
        let (item, content, _) = app.preview_view().unwrap();
        assert_eq!(item.id(), ItemId::History(5));
        assert!(matches!(content, Some(Content::Text { .. })));
        // Loaded previews aren't requested again.
        assert_eq!(requested(&mut app), None);
    }

    #[test]
    fn cached_previews_are_reused_when_moving_back() {
        let mut app = app();
        text_preview(&mut app, 5, "five");
        text_preview(&mut app, 4, "four");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Up);
        assert_eq!(requested(&mut app), None);
        assert!(app.preview_view().unwrap().1.is_some());
    }

    #[test]
    fn preview_cache_is_bounded() {
        let mut app = app();
        for id in 100..100 + PREVIEW_CACHE as u64 {
            text_preview(&mut app, id, "x");
        }
        text_preview(&mut app, 5, "x");
        assert_eq!(app.previews.len(), PREVIEW_CACHE);
        // The oldest one was evicted.
        assert!(
            !app.previews
                .iter()
                .any(|(id, _)| *id == ItemId::History(100))
        );
    }

    #[test]
    fn preview_scrolls_within_bounds_and_resets_on_move() {
        let mut app = app();
        text_preview(&mut app, 5, "a\nb\nc");
        press(&mut app, KeyCode::Char('J'));
        shift(&mut app, KeyCode::Down);
        shift(&mut app, KeyCode::Down);
        assert_eq!(preview_scroll(&mut app), 2, "stops at the last line");
        press(&mut app, KeyCode::Char('K'));
        assert_eq!(preview_scroll(&mut app), 1);

        press(&mut app, KeyCode::Down);
        assert_eq!(preview_scroll(&mut app), 0, "next entry starts at the top");
    }

    #[test]
    fn shift_arrows_scroll_in_search_mode_too() {
        let mut app = app();
        text_preview(&mut app, 5, "a\nb");
        press(&mut app, KeyCode::Char('f'));
        shift(&mut app, KeyCode::Down);
        assert_eq!(preview_scroll(&mut app), 1);
        assert_eq!(app.selected_index(), 0, "selection didn't move");
    }

    #[test]
    fn delete_last_word_works_like_a_shell() {
        let cases = [
            ("git push", "git "),
            ("git push  ", "git "),
            ("git", ""),
            ("", ""),
        ];
        for (input, expected) in cases {
            let mut s = input.to_owned();
            delete_last_word(&mut s);
            assert_eq!(s, expected, "input: {input:?}");
        }
    }
}
