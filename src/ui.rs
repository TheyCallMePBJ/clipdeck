//! Drawing. Reads the app state and renders one frame; no logic lives here.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, Paragraph};

use crate::app::{App, Backend, Status};
use crate::entry::{Entry, Kind};

pub fn draw<B: Backend>(frame: &mut Frame, app: &mut App<B>) {
    let [list_area, status_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let block = Block::bordered().title(" clipdeck ");

    // The border takes one row at the top and one at the bottom.
    app.set_page_height(list_area.height.saturating_sub(2) as usize);

    if app.entries().is_empty() {
        let empty = Paragraph::new("clipboard history is empty".dim()).block(block);
        frame.render_widget(empty, list_area);
    } else {
        let (entries, state) = app.list_view();
        let items: Vec<ListItem> = entries.iter().map(entry_item).collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");
        frame.render_stateful_widget(list, list_area, state);
    }

    frame.render_widget(status_line(app), status_area);
}

fn entry_item(e: &Entry) -> ListItem<'_> {
    match &e.kind {
        Kind::Text => ListItem::new(e.preview.as_str()),
        Kind::Url => ListItem::new(e.preview.as_str().fg(Color::Cyan).underlined()),
        Kind::Image {
            format,
            width,
            height,
            size,
        } => ListItem::new(format!("[image] {format} {width}x{height} · {size}").italic()),
    }
}

fn status_line<B: Backend>(app: &App<B>) -> Line<'static> {
    match app.status() {
        Some(Status::Info(msg)) => Line::from(format!(" {msg}")).fg(Color::Green),
        Some(Status::Prompt(msg)) => Line::from(format!(" {msg}")).fg(Color::Yellow),
        Some(Status::Error(msg)) => Line::from(format!(" error: {msg}")).fg(Color::Red),
        None => {
            let position = if app.entries().is_empty() {
                "0/0".to_owned()
            } else {
                format!("{}/{}", app.selected_index() + 1, app.entries().len())
            };
            Line::from(format!(
                " {position} · j/k move · enter copy · dd delete · r reload · q quit"
            ))
            .dim()
        }
    }
}
