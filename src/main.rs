mod app;
mod cliphist;
mod config;
mod entry;
mod event;
mod item;
mod keys;
mod paste;
mod pins;
mod preview;
mod sanitize;
mod search;
#[cfg(test)]
mod testutil;
mod tty;
mod ui;
mod watch;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Parser;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::KeyEventKind;
use ratatui_image::picker::Picker;

use app::{Action, App, Backend, CopySource};
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

    /// Print the chosen entry to stdout instead of copying it, for use in
    /// scripts (`clipdeck --pick | ...`). Exits with status 130 if nothing
    /// is chosen.
    #[arg(long, conflicts_with = "paste")]
    pick: bool,

    /// After copying, paste into the window underneath by pressing KEYS
    /// with `wtype`. Defaults to ctrl+v; terminals usually need
    /// ctrl+shift+v.
    #[arg(
        long,
        value_name = "KEYS",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = paste::DEFAULT_KEYS
    )]
    paste: Option<String>,

    /// Read keybinds and colors from this file instead of
    /// ~/.config/clipdeck/config.toml.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Print the default config, with every option explained, and exit.
    /// A starting point: `clipdeck --default-config > ~/.config/clipdeck/config.toml`
    #[arg(long)]
    default_config: bool,
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

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    if cli.default_config {
        print!("{}", config::DEFAULT_CONFIG);
        return Ok(ExitCode::SUCCESS);
    }
    let start = Instant::now();
    // Catch a bad shortcut now, rather than after an entry was chosen.
    if let Some(keys) = &cli.paste {
        paste::wtype_args(keys)?;
    }

    let cliphist = Cliphist::new(cli.db_path.clone());
    // If the pins file can't be read, run without pins (and say so) rather
    // than refusing to start: the popup closes on exit, so an error printed
    // here would vanish before anyone could read it.
    let (pins, pins_error) = match pins::default_dir().map(pins::Pins::load) {
        Some(Ok(pins)) => (Some(pins), None),
        Some(Err(e)) => (None, Some(format!("pins unavailable: {e:#}"))),
        None => (None, Some("pins unavailable: no home directory".to_owned())),
    };
    // A broken config falls back to the defaults, with the error shown in
    // the status bar (and printed after exit), for the same reason.
    let (config, config_error) = match cli.config.clone().or_else(config::default_path) {
        Some(path) => match config::Config::load(&path) {
            Ok(config) => (config, None),
            Err(e) => (
                config::Config::default(),
                Some(format!("config ignored: {e:#}")),
            ),
        },
        None => (config::Config::default(), None),
    };
    let image_protocol = config.image_protocol;
    let mut app = App::new(cliphist.clone(), pins)?;
    app.set_config(config);
    // The config error wins if there are both: it's the one just made.
    for message in [&pins_error, &config_error].into_iter().flatten() {
        app.show_error(message.clone());
    }
    if cli.pick {
        app.set_enter_verb("print");
    } else if cli.paste.is_some() {
        app.set_enter_verb("paste");
    }
    let listed = start.elapsed();

    // With --pick, stdout may be a pipe. Draw on the terminal instead, and
    // put stdout back (when this is dropped) before printing the entry.
    let use_terminal = tty::UseTerminal::redirect()?;
    // ratatui::init also installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();

    // Ask the terminal which image protocol it supports. This reads from
    // the terminal, so it must happen before the input thread starts.
    // Terminals without a graphics protocol get images drawn with
    // half-block characters instead.
    // The query also finds the font size, which images need for scaling,
    // so it runs even when the config names a protocol.
    let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    if let Some(protocol) = image_protocol {
        picker.set_protocol_type(protocol);
    }
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
    // Dropping the Terminal writes a "show cursor" escape, so it has to go
    // before stdout is pointed back at a possible pipe.
    drop(terminal);
    drop(use_terminal);

    let (selected, first_frame) = result?;
    if let Some(message) = &config_error {
        eprintln!("clipdeck: {message}");
    }
    if cli.timing {
        eprintln!(
            "cliphist list: {listed:?}, terminal query: {:?}, first frame: {first_frame:?}",
            queried - listed
        );
        eprintln!("image protocol: {protocol:?}");
        eprintln!("{watch_status}");
        if let Some(message) = &pins_error {
            eprintln!("{message}");
        }
    }
    // Act on the choice after the terminal is restored, so errors print
    // cleanly.
    let Some(selected) = selected else {
        // 130 is what shells report for Ctrl+c, and what fzf uses when
        // nothing was chosen. Scripts can tell "cancelled" from "picked".
        return Ok(if cli.pick {
            ExitCode::from(130)
        } else {
            ExitCode::SUCCESS
        });
    };
    let bytes = match selected {
        CopySource::Entry(entry) => cliphist.decode(&entry)?,
        CopySource::Bytes(bytes) => bytes,
    };
    if cli.pick {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&bytes)?;
        stdout.flush()?;
    } else {
        cliphist::wl_copy(&bytes)?;
        if let Some(keys) = &cli.paste {
            paste::spawn(keys)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Runs the event loop. Returns the entry to copy (if any) and the time
/// from `start` to the first frame being drawn.
fn run<B: Backend>(
    terminal: &mut DefaultTerminal,
    app: &mut App<B>,
    events: &Receiver<Event>,
    loader: &Loader,
    start: Instant,
) -> Result<(Option<CopySource>, Duration)> {
    let mut first_frame = None;
    loop {
        // Ask for the selected entry's preview if it isn't loaded or on
        // its way already. It arrives later as an `Event::Preview`.
        if let Some(request) = app.preview_request() {
            loader.request(request);
        }
        terminal.draw(|f| ui::draw(f, app))?;
        first_frame.get_or_insert_with(|| start.elapsed());

        // Blocks until any thread sends an event.
        match events.recv()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match app.handle_key(key) {
                Action::None => {}
                Action::Quit => return Ok((None, first_frame.unwrap_or_default())),
                Action::Copy(source) => return Ok((Some(source), first_frame.unwrap_or_default())),
            },
            Event::Key(_) | Event::Redraw => {}
            Event::Preview(loaded) => app.preview_loaded(loaded),
            Event::HistoryChanged => app.history_changed(),
            Event::InputError(e) => bail!("reading terminal input failed: {e}"),
        }
    }
}
