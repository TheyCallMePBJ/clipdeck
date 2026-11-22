//! Everything the main loop reacts to, delivered through one channel.
//!
//! Several threads produce events: one reads the terminal, the preview
//! loader reports finished previews, and the database watcher reports
//! history changes. The main loop blocks on the channel's receiving end
//! and handles whichever event arrives first.

use std::sync::mpsc::Sender;
use std::thread;

use ratatui::crossterm::event::{self, KeyEvent};

use crate::preview::Loaded;

pub enum Event {
    Key(KeyEvent),
    /// The terminal was resized (or something else that only needs a redraw).
    Redraw,
    /// The preview loader finished an entry.
    Preview(Loaded),
    /// cliphist's database changed, e.g. something new was copied.
    HistoryChanged,
    /// Reading the terminal failed; the app can't continue.
    InputError(std::io::Error),
}

/// Starts a thread that forwards terminal input to `events`.
///
/// Must start only after anything else that reads the terminal (like the
/// image-protocol query) has finished, or the two would steal each other's
/// input.
pub fn spawn_input_thread(events: Sender<Event>) {
    thread::spawn(move || {
        loop {
            let event = match event::read() {
                Ok(event::Event::Key(key)) => Event::Key(key),
                Ok(event::Event::Resize(..)) => Event::Redraw,
                Ok(_) => continue,
                Err(e) => Event::InputError(e),
            };
            let failed = matches!(event, Event::InputError(_));
            // A send error means the main loop has exited; stop too.
            if events.send(event).is_err() || failed {
                break;
            }
        }
    });
}
