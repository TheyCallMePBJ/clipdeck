mod app;
mod cliphist;
mod entry;
mod event;
mod preview;
mod sanitize;
mod search;
mod ui;
mod watch;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Parser;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::KeyEventKind;
use ratatui_image::picker::Picker;

use app::{Action, App, Backend};
use cliphist::Cliphist;
use entry::Entry;
use event::Event;
use preview::Loader;

/// A terminal-styled clipboard history manager built on cliphist.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Print startup timing (and the detected image protocol) to stderr
    /// after exit.
    #[arg(long)]
    timing: bool,

    /// Use this cliphist database instead of cliphist's default
    /// (passed to cliphist as -db-path). Handy for testing.
    #[arg(long, value_name = "PATH")]
    db_path: Option<PathBuf>,
}

/// Starts watching cliphist's database for live reload. Returns the watcher,
/// which must be kept alive, plus a line for `--timing` saying what
/// happened. Live reload is a nicety: if it can't be set up, clipdeck works
/// as before (with `r` to reload).
fn start_watching(
    db_path: Option<&std::path::Path>,
    events: mpsc::Sender<Event>,
) -> (Option<notify::RecommendedWatcher>, String) {
    let Some(path) = watch::resolve_db_path(db_path, |name| std::env::var(name).ok()) else {
        return (
            None,
            "live reload: off (can't find cliphist's database)".into(),
        );
    };
    match watch::watch(&path, events) {
        Ok(watcher) => (
            Some(watcher),
            format!("live reload: watching {}", path.display()),
        ),
        Err(e) => (None, format!("live reload: off ({e:#})")),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let start = Instant::now();

    let cliphist = Cliphist::new(cli.db_path.clone());
    let mut app = App::new(cliphist.clone())?;
    let listed = start.elapsed();

    // ratatui::init also installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();

    // Ask the terminal which image protocol it supports. This reads from
    // the terminal, so it must happen before the input thread starts.
    // Terminals without a graphics protocol get images drawn with
    // half-block characters instead.
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let protocol = picker.protocol_type();
    let queried = start.elapsed();

    let (events, incoming) = mpsc::channel();
    event::spawn_input_thread(events.clone());
    // Kept in a variable so it lives until `main` returns; dropping it
    // would stop the watch.
    let (_watcher, watch_status) = start_watching(cli.db_path.as_deref(), events.clone());
    let decoder = cliphist.clone();
    let loader = Loader::spawn(
        move |entry: &Entry| decoder.decode(entry),
        picker,
        preview::default_thumbs_dir(),
        events,
    );

    let result = run(&mut terminal, &mut app, &incoming, &loader, start);
    // Restore before looking at the result, so an error doesn't leave the
    // terminal in raw mode.
    ratatui::restore();

    let (selected, first_frame) = result?;
    if cli.timing {
        eprintln!(
            "cliphist list: {listed:?}, terminal query: {:?}, first frame: {first_frame:?}",
            queried - listed
        );
        eprintln!("image protocol: {protocol:?}");
        eprintln!("{watch_status}");
    }
    // Copy after the terminal is restored so errors print cleanly.
    if let Some(entry) = selected {
        cliphist.copy(&entry)?;
    }
    Ok(())
}

/// Runs the event loop. Returns the entry to copy (if any) and the time
/// from `start` to the first frame being drawn.
fn run<B: Backend>(
    terminal: &mut DefaultTerminal,
    app: &mut App<B>,
    events: &Receiver<Event>,
    loader: &Loader,
    start: Instant,
) -> Result<(Option<Entry>, Duration)> {
    let mut first_frame = None;
    loop {
        // Ask for the selected entry's preview if it isn't loaded or on
        // its way already. It arrives later as an `Event::Preview`.
        if let Some(entry) = app.preview_request() {
            loader.request(entry);
        }
        terminal.draw(|f| ui::draw(f, app))?;
        first_frame.get_or_insert_with(|| start.elapsed());

        // Blocks until any thread sends an event.
        match events.recv()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match app.handle_key(key) {
                Action::None => {}
                Action::Quit => return Ok((None, first_frame.unwrap_or_default())),
                Action::Copy(entry) => return Ok((Some(entry), first_frame.unwrap_or_default())),
            },
            Event::Key(_) | Event::Redraw => {}
            Event::Preview(loaded) => app.preview_loaded(loaded),
            Event::HistoryChanged => app.history_changed(),
            Event::InputError(e) => bail!("reading terminal input failed: {e}"),
        }
    }
}
