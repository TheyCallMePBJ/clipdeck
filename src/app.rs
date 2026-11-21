//! Application state and key handling, independent of the terminal.
//! Keeping this free of drawing code means it can be unit-tested with a
//! fake backend.

use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::entry::Entry;

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

pub struct App<B: Backend> {
    backend: B,
    /// The last result of `backend.list()`. Never edited directly: after any
    /// change, list again so the UI matches what cliphist holds.
    entries: Vec<Entry>,
    /// Index into `entries`. Always valid when `entries` is non-empty.
    selected: usize,
    status: Option<Status>,
    /// Number of visible list rows, set by the UI each frame. Used for paging.
    page_height: usize,
    /// Scroll position of the list widget. Owned here so it persists
    /// between frames. Its selection is kept in sync with `selected`.
    list_state: ListState,
    /// Id of the entry that was armed for deletion by the previous key
    /// press. Only the very next key can confirm it.
    pending_delete: Option<u64>,
}

impl<B: Backend> App<B> {
    pub fn new(backend: B) -> Result<Self> {
        let entries = backend.list()?;
        Ok(Self {
            backend,
            entries,
            selected: 0,
            status: None,
            page_height: 10,
            list_state: ListState::default(),
            pending_delete: None,
        })
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    /// Everything the UI needs to draw the list: the entries and the list
    /// widget's state, with the selection synced.
    ///
    /// Returning both from one method lets the caller borrow two fields at
    /// once. Calling `entries()` and then using `list_state` mutably would
    /// not compile, because `entries()` borrows all of `self`.
    pub fn list_view(&mut self) -> (&[Entry], &mut ListState) {
        self.list_state.select(Some(self.selected));
        (&self.entries, &mut self.list_state)
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
        let half_page = (self.page_height / 2).max(1) as isize;
        let page = self.page_height as isize;

        // Arms are tried top to bottom, so the Ctrl combinations must come
        // before the plain letters they share a key with.
        match key.code {
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::Char('d') if ctrl => self.move_by(half_page),
            KeyCode::Char('u') if ctrl => self.move_by(-half_page),

            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.selected = self.last_index(),

            KeyCode::Enter => {
                if let Some(entry) = self.selected() {
                    return Action::Copy(entry.clone());
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => self.press_delete(pending_delete),
            KeyCode::Char('r') => match self.reload() {
                Ok(()) => self.status = Some(Status::Info("reloaded".into())),
                Err(e) => self.status = Some(Status::Error(format!("{e:#}"))),
            },
            _ => {}
        }
        Action::None
    }

    /// Moves the selection by `delta` rows, stopping at either end.
    fn move_by(&mut self, delta: isize) {
        let target = self.selected as isize + delta;
        self.selected = target.clamp(0, self.last_index() as isize) as usize;
    }

    fn last_index(&self) -> usize {
        self.entries.len().saturating_sub(1)
    }

    /// Lists again, keeping the same entry selected if it still exists
    /// (new entries may have appeared above it). Otherwise the selection
    /// stays at the same position.
    pub fn reload(&mut self) -> Result<()> {
        let selected_id = self.selected().map(|e| e.id);
        self.entries = self.backend.list()?;
        if let Some(index) = selected_id.and_then(|id| self.entries.iter().position(|e| e.id == id))
        {
            self.selected = index;
        }
        self.selected = self.selected.min(self.last_index());
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

    /// An app over entries with ids 5, 4, 3, 2, 1 (newest first, like cliphist).
    fn app() -> App<Fake> {
        let entries = (1..=5)
            .rev()
            .map(|id| entry(id, &format!("e{id}")))
            .collect();
        App::new(Fake {
            entries,
            fail_delete: false,
        })
        .unwrap()
    }

    fn press(app: &mut App<Fake>, code: KeyCode) -> Action {
        app.handle_key(KeyEvent::from(code))
    }

    fn ctrl(app: &mut App<Fake>, c: char) -> Action {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn selected_id(app: &App<Fake>) -> u64 {
        app.selected().unwrap().id
    }

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
        let mut app = App::new(Fake {
            entries: vec![],
            fail_delete: false,
        })
        .unwrap();
        assert_eq!(press(&mut app, KeyCode::Down), Action::None);
        assert_eq!(press(&mut app, KeyCode::Char('d')), Action::None);
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        assert_eq!(app.selected(), None);
    }
}
