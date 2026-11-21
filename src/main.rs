mod app;
mod cliphist;
mod entry;
mod ui;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::{Action, App, Backend};
use cliphist::Cliphist;
use entry::Entry;

/// A terminal-styled clipboard history manager built on cliphist.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Print startup timing to stderr after exit.
    #[arg(long)]
    timing: bool,

    /// Use this cliphist database instead of cliphist's default
    /// (passed to cliphist as -db-path). Handy for testing.
    #[arg(long, value_name = "PATH")]
    db_path: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let start = Instant::now();

    let mut app = App::new(Cliphist::new(cli.db_path))?;
    let listed = start.elapsed();

    // ratatui::init also installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, start);
    // Restore before looking at the result, so an error doesn't leave the
    // terminal in raw mode.
    ratatui::restore();

    let (selected, first_frame) = result?;
    if cli.timing {
        eprintln!("cliphist list: {listed:?}, first frame: {first_frame:?}");
    }
    // Copy after the terminal is restored so errors print cleanly.
    if let Some(entry) = selected {
        app.backend().copy(&entry)?;
    }
    Ok(())
}

/// Runs the event loop. Returns the entry to copy (if any) and the time
/// from `start` to the first frame being drawn.
fn run<B: Backend>(
    terminal: &mut DefaultTerminal,
    app: &mut App<B>,
    start: Instant,
) -> Result<(Option<Entry>, Duration)> {
    terminal.draw(|f| ui::draw(f, app))?;
    let first_frame = start.elapsed();

    loop {
        let Event::Key(key) = event::read()? else {
            // Resizes and other events just trigger a redraw.
            terminal.draw(|f| ui::draw(f, app))?;
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.handle_key(key) {
            Action::None => {}
            Action::Quit => return Ok((None, first_frame)),
            Action::Copy(entry) => return Ok((Some(entry), first_frame)),
        }
        terminal.draw(|f| ui::draw(f, app))?;
    }
}
