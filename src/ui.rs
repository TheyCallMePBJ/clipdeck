//! Drawing. Reads the app state and renders one frame; no logic lives here.

use image::imageops::FilterType;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph, Wrap};
use ratatui_image::{Resize, StatefulImage};

use crate::app::{App, Backend, Mode, Status};
use crate::config::Theme;
use crate::entry::Kind;
use crate::item::Item;
use crate::keys::{Command, Key, Keymap};
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

    if app.help_open() {
        draw_help(frame, app);
    }
}

/// The help screen's rows for the list: commands shown together (their
/// keys side by side, as in `j/k`) and what they do.
const HELP: &[(&[Command], &str)] = &[
    (&[Command::Down, Command::Up], "move"),
    (
        &[Command::HalfPageDown, Command::HalfPageUp],
        "half page down / up",
    ),
    (&[Command::PageDown, Command::PageUp], "page down / up"),
    (&[Command::Top, Command::Bottom], "top / bottom"),
    (
        &[Command::PreviewDown, Command::PreviewUp],
        "scroll the preview",
    ),
    (&[Command::Select], "{enter} and close"),
    (&[Command::Search], "search"),
    (&[Command::Pin], "pin / unpin"),
    (&[Command::Mark], "mark (to delete several)"),
    (
        &[Command::Delete],
        "press twice: delete (the marked, if any)",
    ),
    (&[Command::Wipe], "press twice: wipe history (pins kept)"),
    (
        &[Command::NextType, Command::PreviousType],
        "show all / text / images / URLs",
    ),
    (&[Command::Reload], "reload"),
    (&[Command::Help], "this help"),
    (&[Command::Back], "clear marks, search, type; then quit"),
    (&[Command::Quit], "quit (ctrl+c always quits)"),
];

/// Fixed keys while searching; everything else works as in the list.
const SEARCH_HELP: &[(&str, &str)] = &[
    ("typing", "filter: 'exact ^prefix suffix$ !not"),
    ("Backspace", "delete a character"),
    ("ctrl+w  ctrl+u", "delete word / clear"),
    ("Esc", "back to the list, keeping the filter"),
    (
        "other keys",
        "as in the list (Enter, arrows, Tab, ctrl+...)",
    ),
];

/// The keys of a help row: each command's keys side by side, so `j`, `k`
/// and `Down`, `Up` become `j/k  Down/Up`.
fn help_keys(keymap: &Keymap, commands: &[Command]) -> String {
    let lists: Vec<&[Key]> = commands.iter().map(|c| keymap.keys(*c)).collect();
    let longest = lists.iter().map(|l| l.len()).max().unwrap_or(0);
    (0..longest)
        .map(|i| {
            lists
                .iter()
                .filter_map(|list| list.get(i).map(Key::to_string))
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn draw_help<B: Backend>(frame: &mut Frame, app: &App<B>) {
    let theme = app.theme();
    let list_rows: Vec<(String, String)> = HELP
        .iter()
        .map(|(commands, action)| {
            (
                help_keys(app.keymap(), commands),
                action.replace("{enter}", app.enter_verb()),
            )
        })
        .filter(|(keys, _)| !keys.is_empty())
        .collect();
    let search_rows: Vec<(String, String)> = SEARCH_HELP
        .iter()
        .map(|(k, a)| ((*k).to_owned(), (*a).to_owned()))
        .collect();
    let key_width = list_rows
        .iter()
        .chain(&search_rows)
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let row = |(keys, action): &(String, String)| {
        Line::from(vec![
            format!("   {keys:<key_width$}").fg(theme.accent),
            action.clone().into(),
        ])
    };

    let mut lines = vec![Line::from(" in the list".bold())];
    lines.extend(list_rows.iter().map(row));
    lines.push(Line::raw(""));
    lines.push(Line::from(" while searching".bold()));
    lines.extend(search_rows.iter().map(row));
    lines.push(Line::raw(""));
    lines.push(Line::from(" press any key to close".dim()));

    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let width = (widest + 3).min(frame.area().width);
    let height = (lines.len() as u16 + 2).min(frame.area().height);
    let area = frame
        .area()
        .centered(Constraint::Length(width), Constraint::Length(height));
    // Clear first, so the list underneath doesn't show through.
    frame.render_widget(Clear, area);
    let help = Paragraph::new(lines).block(border(theme).title(" keys "));
    frame.render_widget(help, area);
}

fn border(theme: &Theme) -> Block<'static> {
    Block::bordered().border_style(Style::new().fg(theme.border))
}

fn draw_list<B: Backend>(frame: &mut Frame, app: &mut App<B>, list_area: Rect) {
    // The border takes one row at the top and one at the bottom.
    app.set_page_height(list_area.height.saturating_sub(2) as usize);
    let type_label = app.type_filter().label();
    let title = match type_label {
        Some(label) => format!(" clipdeck · {label} "),
        None => " clipdeck ".to_owned(),
    };
    let theme = *app.theme();
    let block = border(&theme).title(title);

    if app.match_count() == 0 {
        let message = match (type_label, app.query().is_empty()) {
            _ if app.items().is_empty() => "clipboard history is empty".to_owned(),
            (Some(label), true) => format!("no {label}"),
            _ => "no matches".to_owned(),
        };
        frame.render_widget(Paragraph::new(message.dim()).block(block), list_area);
    } else {
        let (items, matches, marked, state) = app.list_view();
        let items: Vec<ListItem> = matches
            .iter()
            .map(|m| {
                let item = &items[m.index];
                list_item(item, m, marked.contains(&item.id()), &theme)
            })
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(theme.selected())
            .highlight_symbol("> ");
        frame.render_stateful_widget(list, list_area, state);
    }
}

fn draw_preview<B: Backend>(frame: &mut Frame, app: &mut App<B>, area: Rect) {
    let theme = *app.theme();
    let block = border(&theme);
    let Some((item, content, scroll)) = app.preview_view() else {
        // Nothing selected (empty history or no matches).
        frame.render_widget(block.title(" preview "), area);
        return;
    };
    let block = block.title(preview_title(item, content.as_deref()));

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
            let error = Paragraph::new(msg.as_str().fg(theme.error))
                .wrap(Wrap { trim: false })
                .block(block);
            frame.render_widget(error, area);
        }
    }
}

/// E.g. " text · 12 lines · 1.4 KiB " or " png 2560x1440 · 2 MiB · pinned ".
fn preview_title(item: &Item, content: Option<&Content>) -> String {
    let details = match (item.kind(), content) {
        (
            Kind::Image {
                format,
                width,
                height,
                size,
            },
            _,
        ) => format!("{format} {width}x{height} · {size}"),
        (
            kind,
            Some(Content::Text {
                lines,
                bytes,
                truncated,
            }),
        ) => {
            let count = lines.len();
            let plural = if count == 1 { "" } else { "s" };
            let more = if *truncated { " (truncated)" } else { "" };
            format!(
                "{} · {count} line{plural}{more} · {}",
                kind_name(kind),
                human_size(*bytes)
            )
        }
        (kind, _) => kind_name(kind).to_owned(),
    };
    let pinned = if item.is_pinned() { " · pinned" } else { "" };
    format!(" {details}{pinned} ")
}

fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Text => "text",
        Kind::Url => "url",
        Kind::Image { .. } => "image",
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
        (Mode::Normal, false) => match app.keymap().first(Command::Back) {
            Some(back) => Line::from(vec![
                " filter: ".dim(),
                app.query().into(),
                format!("  ({back} to clear)").dim(),
            ]),
            None => Line::from(vec![" filter: ".dim(), app.query().into()]),
        },
        (Mode::Normal, true) => match app.keymap().first(Command::Search) {
            Some(search) => Line::from(format!(" press {search} to search").dim()),
            None => Line::raw(""),
        },
    };
    // Show the terminal cursor at the end of the query while typing.
    if app.mode() == Mode::Search {
        let x = area.x + line.width() as u16;
        frame.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
    }
    frame.render_widget(line, area);
}

/// One list row, with the characters that matched the query highlighted.
/// Marked rows start with a dot and pinned rows with a star.
fn list_item(item: &Item, m: &Match, marked: bool, theme: &Theme) -> ListItem<'static> {
    let base = match item.kind() {
        Kind::Text => Style::new(),
        Kind::Url => Style::new().fg(theme.url).underlined(),
        Kind::Image { .. } => Style::new().italic(),
    };
    let highlight = base.fg(theme.matched).bold();

    // Split the label into runs of highlighted and plain characters.
    // Note: nucleo counts grapheme clusters while this counts chars, so
    // highlights can drift on text with combining characters (e.g. some
    // emoji). Plain text is unaffected.
    let mut spans = Vec::new();
    if marked {
        spans.push(Span::styled("● ", Style::new().fg(theme.mark)));
    }
    if item.is_pinned() {
        spans.push(Span::styled("★ ", Style::new().fg(theme.pin)));
    }
    let mut run = String::new();
    let mut run_highlighted = false;
    let mut next_highlight = m.highlights.iter().peekable();
    for (i, ch) in item.label().chars().enumerate() {
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
    let theme = app.theme();
    match app.status() {
        Some(Status::Info(msg)) => Line::from(format!(" {msg}")).fg(theme.info),
        Some(Status::Prompt(msg)) => Line::from(format!(" {msg}")).fg(theme.prompt),
        Some(Status::Error(msg)) => Line::from(format!(" error: {msg}")).fg(theme.error),
        None => {
            let total = app.items().len();
            let shown = app.match_count();
            let mut position = if shown == 0 {
                "0/0".to_owned()
            } else {
                format!("{}/{}", app.selected_index() + 1, shown)
            };
            if shown != total {
                position.push_str(&format!(" of {total}"));
            }
            let marked = app.marked_count();
            let keys = hints(app, marked).join(" · ");
            Line::from(format!(" {position} · {keys}")).dim()
        }
    }
}

/// The key hints in the status bar, for the keys that are bound.
fn hints<B: Backend>(app: &App<B>, marked: usize) -> Vec<String> {
    let keymap = app.keymap();
    let hint = |key: Option<String>, what: &str| key.map(|k| format!("{k} {what}"));
    let wanted = match app.mode() {
        Mode::Normal if marked > 0 => vec![
            Some(format!("{marked} marked")),
            hint(keymap.first(Command::Mark), "mark"),
            hint(keymap.twice(Command::Delete), "delete marked"),
            hint(keymap.first(Command::Back), "clear marks"),
        ],
        Mode::Normal => vec![
            hint(keymap.first(Command::Select), app.enter_verb()),
            hint(keymap.first(Command::Search), "search"),
            hint(keymap.first(Command::Pin), "pin"),
            hint(keymap.first(Command::Mark), "mark"),
            hint(keymap.twice(Command::Delete), "delete"),
            hint(keymap.first(Command::Help), "help"),
            hint(keymap.first(Command::Quit), "quit"),
        ],
        Mode::Search => vec![
            Some("type to filter".to_owned()),
            hint(keymap.first(Command::Select), app.enter_verb()),
            Some("Esc done".to_owned()),
        ],
    };
    wanted.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    use crate::entry::Entry;

    struct Fixed(Vec<Entry>);

    impl Backend for Fixed {
        fn list(&self) -> Result<Vec<Entry>> {
            Ok(self.0.clone())
        }
        fn decode(&self, entry: &Entry) -> Result<Vec<u8>> {
            Ok(entry.preview.clone().into_bytes())
        }
        fn delete(&mut self, _: &[Entry]) -> Result<()> {
            Ok(())
        }
        fn store(&mut self, _: &[u8]) -> Result<()> {
            Ok(())
        }
        fn wipe(&mut self) -> Result<()> {
            Ok(())
        }
    }

    /// Draws one frame into an in-memory terminal and returns its text.
    fn render(app: &mut App<Fixed>) -> String {
        render_at(app, 60, 8)
    }

    fn render_at(app: &mut App<Fixed>, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
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
        App::new(
            Fixed(lines.iter().filter_map(|l| Entry::parse(l)).collect()),
            None,
        )
        .unwrap()
    }

    #[test]
    fn renders_list_and_status() {
        let screen = render(&mut app());
        assert!(screen.contains("press f to search"), "{screen}");
        assert!(screen.contains("> git push"), "{screen}");
        assert!(screen.contains("[image] png 2560x1440 · 2 MiB"), "{screen}");
        assert!(screen.contains("1/3 · Enter copy"), "{screen}");
    }

    #[test]
    fn enter_label_follows_the_mode() {
        let mut app = app();
        app.set_enter_verb("paste");
        assert!(render(&mut app).contains("1/3 · Enter paste"));
        app.handle_key(KeyEvent::from(KeyCode::Char('?')));
        assert!(render_at(&mut app, 100, 40).contains("paste and close"));
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
            id: crate::item::ItemId::History(3),
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
        let image = &app.items()[1];
        assert_eq!(preview_title(image, None), " png 2560x1440 · 2 MiB ");
    }

    #[test]
    fn pinned_rows_have_a_star_and_title() {
        let dir = crate::testutil::temp_dir();
        let mut pins = crate::pins::Pins::load(dir.0.clone()).unwrap();
        let pinned = Entry::parse("3\tgit push").unwrap();
        pins.add(b"git push", &pinned).unwrap();
        let mut app = App::new(Fixed(vec![pinned]), Some(pins)).unwrap();

        let screen = render_wide(&mut app);
        assert!(screen.contains("> ★ git push"), "{screen}");
        assert_eq!(app.items().len(), 1, "shown once, as a pin");
        assert_eq!(preview_title(&app.items()[0], None), " text · pinned ");
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MiB");
    }

    #[test]
    fn marked_rows_have_a_dot_and_status_counts_them() {
        let mut app = app();
        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        let screen = render_wide(&mut app);
        assert!(screen.contains("● git push"), "{screen}");
        assert!(screen.contains("1 marked"), "{screen}");
    }

    #[test]
    fn help_screen_lists_keys() {
        let mut app = app();
        app.handle_key(KeyEvent::from(KeyCode::Char('?')));
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..40)
            .map(|y| {
                (0..100)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(screen.contains(" keys "), "{screen}");
        assert!(screen.contains("j/k  Down/Up  ctrl+n/ctrl+p"), "{screen}");
        assert!(
            screen.contains("press twice: wipe history (pins kept)"),
            "{screen}"
        );
        assert!(screen.contains("press any key to close"), "{screen}");
    }

    #[test]
    fn hints_prompts_and_colors_follow_the_config() {
        let mut app = app();
        let config = crate::config::tests::user(
            "[keys]\ndelete = \"x\"\nsearch = \"s\"\n[colors]\nborder = \"blue\"",
        )
        .unwrap();
        app.set_config(config);
        let screen = render_at(&mut app, 100, 8);
        assert!(screen.contains("press s to search"), "{screen}");
        assert!(screen.contains("s search"), "{screen}");
        assert!(screen.contains("xx delete"), "{screen}");

        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert_eq!(
            terminal.backend().buffer()[(0, 1)].fg,
            ratatui::style::Color::Blue
        );

        app.handle_key(KeyEvent::from(KeyCode::Char('x')));
        assert!(render_at(&mut app, 100, 8).contains("press x again to delete"));
    }

    #[test]
    fn type_filter_shows_in_title() {
        let mut app = app();
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Tab)); // images
        let screen = render(&mut app);
        assert!(screen.contains("clipdeck · images"), "{screen}");
        assert!(screen.contains("1/1 of 3"), "{screen}");
        app.handle_key(KeyEvent::from(KeyCode::Tab)); // URLs
        app.handle_key(KeyEvent::from(KeyCode::Char('f')));
        app.handle_key(KeyEvent::from(KeyCode::Char('z')));
        assert!(render(&mut app).contains("no matches"));
    }

    #[test]
    fn empty_type_says_what_is_missing() {
        let mut app = App::new(Fixed(vec![Entry::parse("1\tjust text").unwrap()]), None).unwrap();
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Tab)); // images
        assert!(render(&mut app).contains("no images"));
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
