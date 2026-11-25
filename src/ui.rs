use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::app::{App, Focus, Validity};
use crate::prompt::Prompt;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [main, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(main);

    draw_list(frame, app, left);
    draw_detail(frame, app, right);
    draw_footer(frame, app, footer);
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let file_name = app
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = match &app.filter {
        None => format!(" {file_name} · {} lines ", app.source.len()),
        Some(_) => format!(
            " {file_name} · {} / {} ",
            app.visible.len(),
            app.source.len()
        ),
    };
    let mut block = pane(title, app.focus == Focus::List);
    if let Some(q) = &app.filter {
        block = block.title_bottom(Line::styled(
            format!(" /{q} "),
            Style::new().fg(Color::Yellow),
        ));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let height = inner.height as usize;
    app.list_height = height;
    if app.visible.is_empty() {
        let msg = if app.filter.is_some() {
            "(no match — Esc to clear)"
        } else {
            "(empty)"
        };
        frame.render_widget(Paragraph::new(msg).style(dim()), inner);
        return;
    }

    // Keep the selection inside the viewport.
    if app.selected < app.list_offset {
        app.list_offset = app.selected;
    } else if height > 0 && app.selected >= app.list_offset + height {
        app.list_offset = app.selected + 1 - height;
    }

    let num_width = app.source.len().to_string().len();
    // "▶" + "✗" + number + " "
    let preview_width = (inner.width as usize).saturating_sub(num_width + 3);

    let end = (app.list_offset + height).min(app.visible.len());
    let mut rows = Vec::with_capacity(end - app.list_offset);
    for pos in app.list_offset..end {
        let line = app.visible[pos];
        let is_sel = pos == app.selected;
        let status = match app.validity(line) {
            Validity::Invalid => Span::styled("✗", Style::new().fg(Color::Red)),
            _ => Span::raw(" "),
        };
        let preview: String = app
            .source
            .line(line)
            .chars()
            .take(preview_width)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let row = Line::from(vec![
            Span::raw(if is_sel { "▶" } else { " " }),
            status,
            Span::styled(format!("{:>num_width$} ", line + 1), dim()),
            Span::raw(preview),
        ]);
        rows.push(if is_sel {
            row.style(Style::new().add_modifier(Modifier::REVERSED))
        } else {
            row
        });
    }
    frame.render_widget(Paragraph::new(rows), inner);
}

fn draw_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = match app.selected_line() {
        Some(i) => format!(" Line {} ", i + 1),
        None => " Detail ".to_string(),
    };
    let block = pane(title, app.focus == Focus::Detail);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.detail_height = inner.height as usize;

    let Some(line) = app.selected_line() else {
        app.detail_max_scroll = 0;
        return;
    };
    let mut para = Paragraph::new(app.detail_lines(line).to_vec());
    if app.wrap {
        para = para.wrap(Wrap { trim: false });
    }
    let total = para.line_count(inner.width.max(1));
    app.detail_max_scroll =
        u16::try_from(total.saturating_sub(inner.height as usize)).unwrap_or(u16::MAX);
    app.detail_scroll = app.detail_scroll.min(app.detail_max_scroll);

    let hscroll = if app.wrap { 0 } else { app.detail_hscroll };
    frame.render_widget(para.scroll((app.detail_scroll, hscroll)), inner);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(prompt) = &app.prompt {
        draw_prompt(frame, prompt, area);
        return;
    }
    let on_off = |b: bool| if b { "on" } else { "off" };
    let mut spans = vec![];
    let mut key = |k: &str, desc: String| {
        spans.push(Span::styled(
            format!(" {k}"),
            Style::new().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(format!(" {desc} ")));
    };
    key("↑↓", "move".into());
    key("Tab", "focus".into());
    key("/", "filter".into());
    if app.filter.is_some() {
        key("Esc", "clear".into());
    }
    if app.focus == Focus::Detail && !app.wrap {
        key("←→", "scroll".into());
    }
    key("w", format!("wrap ({})", on_off(app.wrap)));
    key("c", format!("color ({})", on_off(app.color)));
    key("q", "quit".into());
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_prompt(frame: &mut Frame, prompt: &Prompt, area: Rect) {
    let symbol = prompt.kind.symbol();
    let avail = (area.width as usize).saturating_sub(symbol.len() + 1);
    // Scroll the input horizontally so the cursor stays visible.
    let skip = prompt.cursor_col().saturating_sub(avail);
    let shown: String = prompt.text.chars().skip(skip).collect();
    let line = Line::from(vec![
        Span::styled(
            symbol,
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::raw(shown),
    ]);
    frame.render_widget(Paragraph::new(line), area);
    let x = area.x + (symbol.len() + prompt.cursor_col() - skip) as u16;
    frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
}

fn pane(title: impl Into<String>, focused: bool) -> Block<'static> {
    let border = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    Block::bordered().title(title.into()).border_style(border)
}

fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Source;
    use ratatui::{Terminal, backend::TestBackend};
    use std::path::Path;

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn fixture_app() -> App {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.ndjson");
        let source = Source::open(&path).unwrap();
        App::new(path, source, true)
    }

    #[test]
    fn renders_list_detail_and_footer() {
        let mut app = fixture_app();
        let screen = render(&mut app, 80, 10);
        assert!(screen.contains("sample.ndjson · 5 lines"), "{screen}");
        assert!(screen.contains("▶ 1 {\"ts\""), "{screen}");
        assert!(screen.contains(" ✗4 this line is not json"), "{screen}");
        assert!(screen.contains("Line 1"), "{screen}");
        assert!(screen.contains(r#""msg": "server started","#), "{screen}");
        assert!(screen.contains("wrap (on)"), "{screen}");
    }

    #[test]
    fn filter_title_prompt_and_no_match() {
        let mut app = fixture_app();
        app.apply_text_filter("error");
        let screen = render(&mut app, 80, 8);
        assert!(screen.contains("sample.ndjson · 1 / 5"), "{screen}");
        assert!(screen.contains("└ /error "), "{screen}");
        assert!(screen.contains("▶ 3 "), "{screen}");
        assert!(screen.contains("Esc clear"), "{screen}");

        app.prompt = Some(Prompt::new(crate::prompt::PromptKind::Text, "warn"));
        let screen = render(&mut app, 80, 8);
        assert!(
            screen.lines().last().unwrap().starts_with("/warn"),
            "{screen}"
        );

        app.prompt = None;
        app.apply_text_filter("nothing-matches-this");
        let screen = render(&mut app, 80, 8);
        assert!(screen.contains("(no match — Esc to clear)"), "{screen}");
        assert!(screen.contains(" Detail "), "{screen}");
    }

    #[test]
    fn list_scrolls_to_keep_selection_visible() {
        let mut app = fixture_app();
        // 6 rows total: 2 border + 1 footer => 3 visible list rows.
        app.selected = 4;
        let screen = render(&mut app, 80, 6);
        assert_eq!(app.list_offset, 2);
        assert!(screen.contains("▶ 5 "), "{screen}");
        assert!(!screen.contains(" 1 {"), "{screen}");
    }

    #[test]
    fn detail_max_scroll_reflects_wrapping() {
        let mut app = fixture_app();
        app.selected = 1; // multi-line pretty output
        // Narrow detail pane so some pretty lines must wrap.
        render(&mut app, 40, 6);
        let wrapped = app.detail_max_scroll;
        app.wrap = false;
        render(&mut app, 40, 6);
        let unwrapped = app.detail_max_scroll;
        assert!(unwrapped > 0);
        assert!(wrapped > unwrapped, "{wrapped} <= {unwrapped}");
    }
}
