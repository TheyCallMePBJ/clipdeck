//! Drawing. Reads the app state and renders one frame; no logic lives here.

use image::imageops::FilterType;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph, Wrap};
use ratatui_image::{Resize, StatefulImage};

use crate::app::{App, Backend, Mode, Status};
use crate::entry::{Entry, Kind};
use crate::preview::Content;
use crate::search::Match;

/// Below this many columns there's no room for a preview next to the list.
const MIN_WIDTH_FOR_PREVIEW: u16 = 70;

pub fn draw<B: Backend>(frame: &mut Frame, app: &mut App<B>) {
    let [search_area, main_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_search_line(frame, app, search_area);

    if main_area.width >= MIN_WIDTH_FOR_PREVIEW {
        let [list_area, preview_area] =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Fill(1)]).areas(main_area);
        draw_list(frame, app, list_area);
        draw_preview(frame, app, preview_area);
    } else {
        draw_list(frame, app, main_area);
    }

    frame.render_widget(status_line(app), status_area);
}

fn draw_list<B: Backend>(frame: &mut Frame, app: &mut App<B>, list_area: Rect) {
    // The border takes one row at the top and one at the bottom.
    app.set_page_height(list_area.height.saturating_sub(2) as usize);
    let block = Block::bordered().title(" clipdeck ");

    if app.match_count() == 0 {
        let message = if app.entries().is_empty() {
            "clipboard history is empty"
        } else {
            "no matches"
        };
        frame.render_widget(Paragraph::new(message.dim()).block(block), list_area);
    } else {
        let (entries, matches, state) = app.list_view();
        let items: Vec<ListItem> = matches
            .iter()
            .map(|m| entry_item(&entries[m.index], m))
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");
        frame.render_stateful_widget(list, list_area, state);
    }
}

fn draw_preview<B: Backend>(frame: &mut Frame, app: &mut App<B>, area: Rect) {
    let block = Block::bordered();
    let Some((entry, content, scroll)) = app.preview_view() else {
        // Nothing selected (empty history or no matches).
        frame.render_widget(block.title(" preview "), area);
        return;
    };
    let block = block.title(preview_title(entry, content.as_deref()));

    match content {
        None => {
            let loading = Paragraph::new("loading…".dim()).block(block);
            frame.render_widget(loading, area);
        }
        Some(Content::Text { lines, .. }) => {
            // Only hand the visible lines to the widget: wrapping a
            // 5000-line entry on every frame would be wasted work. Each
            // line takes at least one row, so this always fills the pane.
            let rows = area.height.saturating_sub(2) as usize;
            let visible: Vec<Line> = lines
                .iter()
                .skip(scroll)
                .take(rows)
                .map(|l| Line::raw(l.as_str()))
                .collect();
            let text = Paragraph::new(visible)
                .wrap(Wrap { trim: false })
                .block(block);
            frame.render_widget(text, area);
        }
        Some(Content::Image(protocol)) => {
            let inner = block.inner(area);
            frame.render_widget(block, area);
            // The thumbnail is usually bigger than the pane, so it gets
            // scaled down to fit. ratatui-image defaults to nearest-neighbour
            // scaling, which drops pixels and makes text in screenshots
            // jagged; Lanczos3 keeps it legible. The scaled result is cached
            // in the protocol state, so this runs once per image and size.
            let widget = StatefulImage::default().resize(Resize::Fit(Some(FilterType::Lanczos3)));
            frame.render_stateful_widget(widget, inner, protocol.as_mut());
        }
        Some(Content::Error(msg)) => {
            let error = Paragraph::new(msg.as_str().fg(Color::Red))
                .wrap(Wrap { trim: false })
                .block(block);
            frame.render_widget(error, area);
        }
    }
}

/// E.g. " text · 12 lines · 1.4 KiB " or " png 2560x1440 · 2 MiB ".
fn preview_title(entry: &Entry, content: Option<&Content>) -> String {
    let kind = match &entry.kind {
        Kind::Text => "text",
        Kind::Url => "url",
        Kind::Image {
            format,
            width,
            height,
            size,
        } => return format!(" {format} {width}x{height} · {size} "),
    };
    match content {
        Some(Content::Text {
            lines,
            bytes,
            truncated,
        }) => {
            let count = lines.len();
            let plural = if count == 1 { "" } else { "s" };
            let more = if *truncated { " (truncated)" } else { "" };
            format!(
                " {kind} · {count} line{plural}{more} · {} ",
                human_size(*bytes)
            )
        }
        _ => format!(" {kind} "),
    }
}

fn human_size(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KIB {
        format!("{bytes} B")
    } else if b < KIB * KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{:.1} MiB", b / KIB / KIB)
    }
}

fn draw_search_line<B: Backend>(frame: &mut Frame, app: &App<B>, area: Rect) {
    let line = match (app.mode(), app.query().is_empty()) {
        (Mode::Search, _) => Line::from(vec![" search: ".bold(), app.query().into()]),
        (Mode::Normal, false) => Line::from(vec![
            " filter: ".dim(),
            app.query().into(),
            "  (esc to clear)".dim(),
        ]),
        (Mode::Normal, true) => Line::from(" press f to search".dim()),
    };
    // Show the terminal cursor at the end of the query while typing.
    if app.mode() == Mode::Search {
        let x = area.x + line.width() as u16;
        frame.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
    }
    frame.render_widget(line, area);
}

/// One list row, with the characters that matched the query highlighted.
fn entry_item(e: &Entry, m: &Match) -> ListItem<'static> {
    let base = match e.kind {
        Kind::Text => Style::new(),
        Kind::Url => Style::new().fg(Color::Cyan).underlined(),
        Kind::Image { .. } => Style::new().italic(),
    };
    let highlight = base.fg(Color::Yellow).bold();

    // Split the label into runs of highlighted and plain characters.
    // Note: nucleo counts grapheme clusters while this counts chars, so
    // highlights can drift on text with combining characters (e.g. some
    // emoji). Plain text is unaffected.
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_highlighted = false;
    let mut next_highlight = m.highlights.iter().peekable();
    for (i, ch) in e.label.chars().enumerate() {
        let highlighted = next_highlight.next_if_eq(&&(i as u32)).is_some();
        if highlighted != run_highlighted && !run.is_empty() {
            let style = if run_highlighted { highlight } else { base };
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        run_highlighted = highlighted;
        run.push(ch);
    }
    let style = if run_highlighted { highlight } else { base };
    spans.push(Span::styled(run, style));

    ListItem::new(Line::from(spans))
}

fn status_line<B: Backend>(app: &App<B>) -> Line<'static> {
    match app.status() {
        Some(Status::Info(msg)) => Line::from(format!(" {msg}")).fg(Color::Green),
        Some(Status::Prompt(msg)) => Line::from(format!(" {msg}")).fg(Color::Yellow),
        Some(Status::Error(msg)) => Line::from(format!(" error: {msg}")).fg(Color::Red),
        None => {
            let total = app.entries().len();
            let shown = app.match_count();
            let mut position = if shown == 0 {
                "0/0".to_owned()
            } else {
                format!("{}/{}", app.selected_index() + 1, shown)
            };
            if shown != total {
                position.push_str(&format!(" of {total}"));
            }
            let keys = match app.mode() {
                Mode::Normal => {
                    "j/k move · J/K scroll · enter copy · f search · dd delete · q quit"
                }
                Mode::Search => "type to filter · ↑/↓ move · enter copy · esc done",
            };
            Line::from(format!(" {position} · {keys}")).dim()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    struct Fixed(Vec<Entry>);

    impl Backend for Fixed {
        fn list(&self) -> Result<Vec<Entry>> {
            Ok(self.0.clone())
        }
        fn delete(&mut self, _: &Entry) -> Result<()> {
            Ok(())
        }
    }

    /// Draws one frame into an in-memory terminal and returns its text.
    fn render(app: &mut App<Fixed>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app() -> App<Fixed> {
        let lines = [
            "3\tgit push",
            "2\t[[ binary data 2 MiB png 2560x1440 ]]",
            "1\thttps://example.com",
        ];
        App::new(Fixed(
            lines.iter().filter_map(|l| Entry::parse(l)).collect(),
        ))
        .unwrap()
    }

    #[test]
    fn renders_list_and_status() {
        let screen = render(&mut app());
        assert!(screen.contains("press f to search"), "{screen}");
        assert!(screen.contains("> git push"), "{screen}");
        assert!(screen.contains("[image] png 2560x1440 · 2 MiB"), "{screen}");
        assert!(screen.contains("1/3 · j/k move"), "{screen}");
    }

    #[test]
    fn renders_search_and_filtered_count() {
        let mut app = app();
        for c in "fexa".chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        let screen = render(&mut app);
        assert!(screen.contains("search: exa"), "{screen}");
        assert!(screen.contains("> https://example.com"), "{screen}");
        assert!(!screen.contains("git push"), "{screen}");
        assert!(screen.contains("1/1 of 3"), "{screen}");
    }

    fn render_wide(app: &mut App<Fixed>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 8)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn narrow_terminal_has_no_preview() {
        let screen = render(&mut app());
        assert!(!screen.contains("loading"), "{screen}");
    }

    #[test]
    fn wide_terminal_shows_loading_then_text() {
        let mut app = app();
        assert!(render_wide(&mut app).contains("loading…"));
        app.preview_loaded(crate::preview::Loaded {
            id: 3,
            content: crate::preview::text_content(b"git push --force-with-lease\nsecond line"),
        });
        let screen = render_wide(&mut app);
        assert!(screen.contains("text · 2 lines · 39 B"), "{screen}");
        assert!(screen.contains("git push --force-with-lease"), "{screen}");
        assert!(screen.contains("second line"), "{screen}");
    }

    #[test]
    fn preview_title_for_images_uses_entry_info() {
        let app = app();
        let image = &app.entries()[1];
        assert_eq!(preview_title(image, None), " png 2560x1440 · 2 MiB ");
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MiB");
    }

    #[test]
    fn renders_no_matches() {
        let mut app = app();
        for c in "fzzz".chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert!(render(&mut app).contains("no matches"));
    }
}
