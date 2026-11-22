//! Application state and key handling, independent of the terminal.
//! Keeping this free of drawing code means it can be unit-tested with a
//! fake backend.

use std::collections::VecDeque;

use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::entry::Entry;
use crate::preview::{Content, Loaded};
use crate::search::{Match, Searcher};

/// How many loaded previews to keep, so moving back and forth over the
/// same entries doesn't decode them again.
const PREVIEW_CACHE: usize = 16;

/// Where history comes from and where changes go. The real implementation
/// is `cliphist::Cliphist`; tests use an in-memory fake.
pub trait Backend {
    fn list(&self) -> Result<Vec<Entry>>;
    fn delete(&mut self, entry: &Entry) -> Result<()>;
}

/// What the main loop should do after a key press.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Copy(Entry),
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
    /// The last result of `backend.list()`. Never edited directly: after any
    /// change, list again so the UI matches what cliphist holds.
    entries: Vec<Entry>,
    /// The entries that match `query`, in display order. With an empty
    /// query this is every entry.
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
    /// Id of the entry that was armed for deletion by the previous key
    /// press. Only the very next key can confirm it.
    pending_delete: Option<u64>,
    /// Loaded previews by entry id, most recently loaded first.
    previews: VecDeque<(u64, Content)>,
    /// The entry whose preview was last requested from the loader.
    preview_requested: Option<u64>,
    /// Preview scroll position, as (entry id, first visible line). Tied to
    /// the entry so moving the selection starts the next preview at the top.
    preview_scroll: (u64, usize),
}

impl<B: Backend> App<B> {
    pub fn new(backend: B) -> Result<Self> {
        let entries = backend.list()?;
        let mut app = Self {
            backend,
            entries,
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
            preview_scroll: (0, 0),
        };
        app.refilter_from_top();
        Ok(app)
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.matches
            .get(self.selected)
            .map(|m| &self.entries[m.index])
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

    /// Everything the UI needs to draw the list: the entries, which of them
    /// to show (in order, with highlights), and the list widget's state with
    /// the selection synced.
    ///
    /// Returning these from one method lets the caller borrow several fields
    /// at once. Calling `entries()` and then using `list_state` mutably would
    /// not compile, because `entries()` borrows all of `self`.
    pub fn list_view(&mut self) -> (&[Entry], &[Match], &mut ListState) {
        self.list_state.select(Some(self.selected));
        (&self.entries, &self.matches, &mut self.list_state)
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

    fn normal_key(&mut self, code: KeyCode, ctrl: bool, pending_delete: Option<u64>) -> Action {
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
            KeyCode::Char('d') | KeyCode::Delete => self.press_delete(pending_delete),
            KeyCode::Char('r') => match self.reload() {
                Ok(()) => self.status = Some(Status::Info("reloaded".into())),
                Err(e) => self.status = Some(Status::Error(format!("{e:#}"))),
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

    fn copy_selected(&self) -> Action {
        match self.selected() {
            Some(entry) => Action::Copy(entry.clone()),
            None => Action::None,
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

    /// Re-runs the search after the query changed, selecting the best match.
    fn refilter_from_top(&mut self) {
        self.refilter();
        self.selected = 0;
    }

    /// Re-runs the search, keeping the same entry selected if it is still
    /// shown. Otherwise the selection stays at the same position.
    fn refilter_keeping_selection(&mut self) {
        let selected_id = self.selected().map(|e| e.id);
        self.refilter();
        self.reselect(selected_id);
    }

    fn refilter(&mut self) {
        // `self.searcher` is borrowed mutably and `self.entries` immutably
        // at the same time. That's allowed because they are different
        // fields, accessed directly rather than through methods on `self`.
        let labels = self.entries.iter().map(|e| e.label.as_str());
        self.matches = self.searcher.filter(&self.query, labels);
    }

    /// Selects the entry with this id if it is shown; otherwise keeps the
    /// current position, clamped to the list.
    fn reselect(&mut self, id: Option<u64>) {
        let entries = &self.entries;
        if let Some(index) =
            id.and_then(|id| self.matches.iter().position(|m| entries[m.index].id == id))
        {
            self.selected = index;
        }
        self.selected = self.selected.min(self.last_index());
    }

    /// Lists again and re-applies the current filter, keeping the same
    /// entry selected if it still exists (new entries may have appeared
    /// above it).
    pub fn reload(&mut self) -> Result<()> {
        let selected_id = self.selected().map(|e| e.id);
        self.entries = self.backend.list()?;
        self.refilter();
        self.reselect(selected_id);
        Ok(())
    }

    /// First press arms the delete; a second press on the same entry,
    /// straight after, performs it.
    fn press_delete(&mut self, pending: Option<u64>) {
        let Some(id) = self.selected().map(|e| e.id) else {
            return;
        };
        // Comparing ids (not just "was the last key d") guards against the
        // list changing between the two presses, e.g. on reload.
        if pending == Some(id) {
            self.delete_selected();
        } else {
            self.pending_delete = Some(id);
            self.status = Some(Status::Prompt("press d again to delete".into()));
        }
    }

    fn delete_selected(&mut self) {
        // Clone so we stop borrowing `self.entries` before calling
        // `self.backend.delete`, which needs `&mut self.backend`.
        let Some(entry) = self.selected().cloned() else {
            return;
        };
        if let Err(e) = self.backend.delete(&entry) {
            self.status = Some(Status::Error(format!("{e:#}")));
            return;
        }
        // The deleted entry is gone, so reload keeps the position and the
        // next entry moves up into the selection.
        self.status = Some(match self.reload() {
            Ok(()) => Status::Info("deleted 1 entry".into()),
            Err(e) => Status::Error(format!("{e:#}")),
        });
    }

    // --- previews ---------------------------------------------------------

    /// The entry whose preview should be loaded next, if any. The main loop
    /// calls this after every event and sends the result to the loader.
    pub fn preview_request(&mut self) -> Option<Entry> {
        let entry = self.selected()?;
        let id = entry.id;
        let loaded = self.previews.iter().any(|(i, _)| *i == id);
        if loaded || self.preview_requested == Some(id) {
            return None;
        }
        let entry = entry.clone();
        self.preview_requested = Some(id);
        Some(entry)
    }

    /// Stores a preview from the loader.
    pub fn preview_loaded(&mut self, loaded: Loaded) {
        if self.preview_requested == Some(loaded.id) {
            self.preview_requested = None;
        }
        self.previews.retain(|(id, _)| *id != loaded.id);
        self.previews.push_front((loaded.id, loaded.content));
        self.previews.truncate(PREVIEW_CACHE);
    }

    /// The selected entry, its preview if loaded, and the scroll position.
    /// The preview is `&mut` because image previews keep encoding state
    /// that the widget updates while drawing.
    pub fn preview_view(&mut self) -> Option<(&Entry, Option<&mut Content>, usize)> {
        // Direct field access (not `self.selected()`) so that `entries` and
        // `previews` can be borrowed at the same time.
        let entry = &self.entries[self.matches.get(self.selected)?.index];
        let content = self
            .previews
            .iter_mut()
            .find(|(id, _)| *id == entry.id)
            .map(|(_, content)| content);
        let scroll = match self.preview_scroll {
            (id, line) if id == entry.id => line,
            _ => 0,
        };
        Some((entry, content, scroll))
    }

    fn scroll_preview(&mut self, delta: isize) -> Action {
        let Some((entry, content, scroll)) = self.preview_view() else {
            return Action::None;
        };
        let max = match content {
            Some(Content::Text { lines, .. }) => lines.len().saturating_sub(1),
            _ => 0,
        };
        let line = (scroll as isize + delta).clamp(0, max as isize) as usize;
        self.preview_scroll = (entry.id, line);
        Action::None
    }
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
    use anyhow::bail;

    /// In-memory backend. Tests edit `entries` directly to simulate
    /// cliphist changing underneath the app.
    struct Fake {
        entries: Vec<Entry>,
        fail_delete: bool,
    }

    impl Backend for Fake {
        fn list(&self) -> Result<Vec<Entry>> {
            Ok(self.entries.clone())
        }

        fn delete(&mut self, entry: &Entry) -> Result<()> {
            if self.fail_delete {
                bail!("delete failed");
            }
            self.entries.retain(|e| e.raw != entry.raw);
            Ok(())
        }
    }

    fn entry(id: u64, text: &str) -> Entry {
        Entry::parse(&format!("{id}\t{text}")).unwrap()
    }

    fn app_with(texts: &[&str]) -> App<Fake> {
        // Newest first, like cliphist: the first text gets the highest id.
        let n = texts.len() as u64;
        let entries = texts
            .iter()
            .enumerate()
            .map(|(i, t)| entry(n - i as u64, t))
            .collect();
        App::new(Fake {
            entries,
            fail_delete: false,
        })
        .unwrap()
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
        app.selected().unwrap().id
    }

    fn shown(app: &mut App<Fake>) -> Vec<String> {
        let (entries, matches, _) = app.list_view();
        matches
            .iter()
            .map(|m| entries[m.index].label.clone())
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
        assert_eq!(app.entries().len(), 5);
        assert_eq!(app.selected_index(), 2);
    }

    #[test]
    fn enter_copies_selected_entry() {
        let mut app = app();
        press(&mut app, KeyCode::Down);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Action::Copy(entry(4, "e4"))
        );
    }

    // --- delete -----------------------------------------------------------

    #[test]
    fn delete_selects_next_entry() {
        let mut app = app();
        press(&mut app, KeyCode::Down); // id 4
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        let ids: Vec<_> = app.entries().iter().map(|e| e.id).collect();
        assert_eq!(ids, [5, 3, 2, 1]);
        assert_eq!(selected_id(&app), 3);
        assert_eq!(app.status(), Some(&Status::Info("deleted 1 entry".into())));
    }

    #[test]
    fn single_press_only_asks_for_confirmation() {
        let mut app = app();
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.entries().len(), 5);
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
        assert_eq!(app.entries().len(), 4);
    }

    #[test]
    fn other_key_cancels_pending_delete() {
        let mut app = app();
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.entries().len(), 5);
        // Moving back doesn't revive the old pending delete either.
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.entries().len(), 5);
    }

    #[test]
    fn pending_delete_is_tied_to_the_entry() {
        let mut app = app();
        press(&mut app, KeyCode::Char('d')); // arms id 5
        // Simulate the selected entry vanishing between the two presses.
        app.backend.entries.remove(0);
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.entries().len(), 4);
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
        assert_eq!(app.entries().len(), 5);
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
    fn empty_history_is_harmless() {
        let mut app = app_with(&[]);
        assert_eq!(press(&mut app, KeyCode::Down), Action::None);
        assert_eq!(press(&mut app, KeyCode::Char('d')), Action::None);
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
        // q, d, j, f would be commands in normal mode.
        assert_eq!(press(&mut app, KeyCode::Char('q')), Action::None);
        type_str(&mut app, "djf");
        assert_eq!(app.query(), "qdjf");
        assert_eq!(app.entries().len(), 4);
    }

    #[test]
    fn enter_in_search_copies_best_match() {
        let mut app = history();
        press(&mut app, KeyCode::Char('f'));
        type_str(&mut app, "pull");
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Action::Copy(entry(2, "git pull"))
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
        assert_eq!(app.selected().unwrap().label, "git pull");

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
        assert_eq!(app.entries().len(), 3);
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

    // --- previews ---------------------------------------------------------

    fn text_preview(app: &mut App<Fake>, id: u64, text: &str) {
        app.preview_loaded(Loaded {
            id,
            content: crate::preview::text_content(text.as_bytes()),
        });
    }

    fn shift(app: &mut App<Fake>, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::SHIFT));
    }

    fn preview_scroll(app: &mut App<Fake>) -> usize {
        app.preview_view().unwrap().2
    }

    #[test]
    fn preview_is_requested_once_per_entry() {
        let mut app = app();
        assert_eq!(app.preview_request().map(|e| e.id), Some(5));
        // Already requested: don't ask again while it loads.
        assert_eq!(app.preview_request(), None);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.preview_request().map(|e| e.id), Some(4));
    }

    #[test]
    fn loaded_preview_is_shown_for_selected_entry() {
        let mut app = app();
        app.preview_request();
        assert!(app.preview_view().unwrap().1.is_none(), "not loaded yet");
        text_preview(&mut app, 5, "full text");
        let (entry, content, _) = app.preview_view().unwrap();
        assert_eq!(entry.id, 5);
        assert!(matches!(content, Some(Content::Text { .. })));
        // Loaded previews aren't requested again.
        assert_eq!(app.preview_request(), None);
    }

    #[test]
    fn cached_previews_are_reused_when_moving_back() {
        let mut app = app();
        text_preview(&mut app, 5, "five");
        text_preview(&mut app, 4, "four");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.preview_request(), None);
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
        assert!(!app.previews.iter().any(|(id, _)| *id == 100));
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
