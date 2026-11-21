mod cliphist;
mod entry;

use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState};

use entry::{Entry, Kind};

/// A terminal-styled clipboard history manager built on cliphist.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Print startup timing to stderr after exit.
    #[arg(long)]
    timing: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let start = Instant::now();

    let entries = cliphist::list()?;
    let listed = start.elapsed();

    // ratatui::init also installs a panic hook that restores the terminal.
    let terminal = ratatui::init();
    let result = App::new(entries).run(terminal, start, cli.timing);
    ratatui::restore();

    let outcome = result?;
    if let Some(t) = outcome.first_frame {
        eprintln!("cliphist list: {listed:?}, first frame: {t:?}");
    }
    // Copy after the terminal is restored so errors print cleanly.
    if let Some(entry) = outcome.selected {
        cliphist::copy(&entry)?;
    }
    Ok(())
}

struct App {
    entries: Vec<Entry>,
    state: ListState,
}

struct Outcome {
    selected: Option<Entry>,
    first_frame: Option<std::time::Duration>,
}

impl App {
    fn new(entries: Vec<Entry>) -> Self {
        let mut state = ListState::default();
        if !entries.is_empty() {
            state.select(Some(0));
        }
        Self { entries, state }
    }

    fn run(
        mut self,
        mut terminal: DefaultTerminal,
        start: Instant,
        timing: bool,
    ) -> Result<Outcome> {
        let mut first_frame = None;
        loop {
            terminal.draw(|f| self.draw(f))?;
            if timing && first_frame.is_none() {
                first_frame = Some(start.elapsed());
            }

            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('c') if ctrl => break,
                KeyCode::Char('j') | KeyCode::Down => self.state.select_next(),
                KeyCode::Char('k') | KeyCode::Up => self.state.select_previous(),
                KeyCode::Char('g') | KeyCode::Home => self.state.select_first(),
                KeyCode::Char('G') | KeyCode::End => self.state.select_last(),
                KeyCode::Enter => {
                    let selected = self
                        .state
                        .selected()
                        .and_then(|i| self.entries.get(i))
                        .cloned();
                    return Ok(Outcome {
                        selected,
                        first_frame,
                    });
                }
                _ => {}
            }
        }
        Ok(Outcome {
            selected: None,
            first_frame,
        })
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        let [list_area, status_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());

        let items: Vec<ListItem> = self.entries.iter().map(entry_line).collect();
        let list = List::new(items)
            .block(Block::bordered().title(" clipdeck "))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");
        frame.render_stateful_widget(list, list_area, &mut self.state);

        let status = Line::from(format!(
            " {} entries · j/k move · enter copy · q quit",
            self.entries.len()
        ))
        .dim();
        frame.render_widget(status, status_area);
    }
}

fn entry_line(e: &Entry) -> ListItem<'_> {
    match &e.kind {
        Kind::Text => ListItem::new(e.preview.as_str()),
        Kind::Image {
            format,
            width,
            height,
            size,
        } => ListItem::new(format!("[image] {format} {width}x{height} · {size}").italic()),
    }
}
