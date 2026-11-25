use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::app::{App, Focus, Status, Validity};
use crate::prompt::Prompt;
use crate::render::highlight::highlight_line;

pub fn draw(frame: &mut Frame, app: &mut App) {
    // The footer grows by one line to show a prompt error.
    let footer_height = match &app.prompt {
        Some(p) if p.error.is_some() => 2,
        _ => 1,
    };
    let [main, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(footer_height)])
        .areas(frame.area());
    let [left, right] = Layout::horizontal([
        Constraint::Percentage(app.list_width),
        Constraint::Percentage(100 - app.list_width),
    ])
    .areas(main);

    draw_list(frame, app, left);
    draw_detail(frame, app, right);
    draw_footer(frame, app, footer);
    if app.show_help {
        draw_help(frame);
    }
}

const HELP: &[(&str, &str)] = &[
    ("↑↓ j k", "move selection / scroll detail"),
    ("PgUp PgDn Space", "page"),
    ("g G Home End", "first / last"),
    ("Tab", "switch pane (Enter/l → detail, h → list)"),
    ("← →", "scroll detail horizontally (wrap off)"),
    ("/", "text filter (smart case)"),
    ("|", "jq filter, e.g. .level == \"error\""),
    (">", "jq view, e.g. .ts + \" \" + .msg"),
    ("Esc", "cancel filter / clear filter / clear view"),
    ("w", "toggle line wrap"),
    ("c", "toggle JSON colors"),
    ("y / Y", "copy line: pretty JSON / raw"),
    ("?", "this help"),
    ("q  Ctrl-C", "quit"),
];

fn draw_help(frame: &mut Frame) {
    let key_w = HELP
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(
                    format!(" {k:<key_w$}  "),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::raw(*d),
            ])
        })
        .collect();
    let width = lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 3;
    let height = lines.len() as u16 + 2;
    let area = frame.area().centered(
        Constraint::Length(width.min(frame.area().width)),
        Constraint::Length(height.min(frame.area().height)),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" Keys ")
                .title_bottom(Line::from(" any key to close ").right_aligned())
                .border_style(Style::new().fg(Color::Cyan)),
        ),
        area,
    );
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let file_name = app
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let live = if !app.is_live() {
        ""
    } else if app.follow {
        " · following"
    } else {
        " · reading…"
    };
    let title = match &app.filter {
        None => format!(" {file_name} · {} lines{live} ", app.total),
        Some(_) => format!(
            " {file_name} · {} / {}{live} ",
            app.visible.len(),
            app.total
        ),
    };
    let mut block = pane(title, app.focus == Focus::List);
    let bottom = if let Some(job) = &app.job {
        Some(format!(
            " {}{} … {}% ",
            job.spec.kind.symbol(),
            job.spec.query,
            job.percent()
        ))
    } else {
        app.filter
            .as_ref()
            .map(|f| format!(" {}{} ", f.kind.symbol(), f.query))
    };
    if let Some(text) = bottom {
        block = block.title_bottom(Line::styled(text, Style::new().fg(Color::Yellow)));
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

    let num_width = app.total.to_string().len();
    // "▶" + "✗" + number + " "
    let preview_width = (inner.width as usize).saturating_sub(num_width + 3);

    let highlight = app.highlight_query().map(str::to_string);
    let hl = app.active_theme().matched;

    let end = (app.list_offset + height).min(app.visible.len());
    let mut rows = Vec::with_capacity(end - app.list_offset);
    for pos in app.list_offset..end {
        let line = app.visible[pos];
        let is_sel = pos == app.selected;
        let status = match app.validity(line) {
            Validity::Invalid => Span::styled("✗", Style::new().fg(Color::Red)),
            _ => Span::raw(" "),
        };
        let text = match app.view_preview(line) {
            Some(p) => p,
            None => app.source.read().line(line).into_owned(),
        };
        let preview: String = text
            .chars()
            .take(preview_width)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let row = Line::from(vec![
            Span::raw(if is_sel { "▶" } else { " " }),
            status,
            Span::styled(format!("{:>num_width$} ", line + 1), dim()),
        ]);
        let preview = Line::from(preview);
        let preview = match &highlight {
            Some(q) => highlight_line(preview, q, hl),
            None => preview,
        };
        let mut row = row;
        row.spans.extend(preview.spans);
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
    let mut block = pane(title, app.focus == Focus::Detail);
    if let Some(view) = &app.view {
        block = block.title_bottom(Line::styled(
            format!(" >{} ", view.code),
            Style::new().fg(Color::Yellow),
        ));
    }
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
        let area = if let Some(err) = &prompt.error {
            let [err_area, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
            frame.render_widget(
                Paragraph::new(format!("✗ {err}")).style(Style::new().fg(Color::Red)),
                err_area,
            );
            rest
        } else {
            area
        };
        draw_prompt(frame, prompt, area);
        return;
    }
    if let Some(status) = &app.status {
        let para = match status {
            Status::Error(msg) => {
                Paragraph::new(format!("✗ {msg}")).style(Style::new().fg(Color::Red))
            }
            Status::Info(msg) => {
                Paragraph::new(format!("✓ {msg}")).style(Style::new().fg(Color::Green))
            }
        };
        frame.render_widget(para, area);
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
    key("/", "text".into());
    key("|", "jq".into());
    key(">", "view".into());
    if app.job.is_some() {
        key("Esc", "cancel".into());
    } else if app.filter.is_some() {
        key("Esc", "clear".into());
    }
    if app.focus == Focus::Detail && !app.wrap {
        key("←→", "scroll".into());
    }
    key("?", "help".into());
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
    use crate::filter::{FilterKind, Spec};
    use crate::prompt::PromptKind;
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
        app.text_now("error");
        let screen = render(&mut app, 80, 8);
        assert!(screen.contains("sample.ndjson · 1 / 5"), "{screen}");
        assert!(screen.contains("└ /error "), "{screen}");
        assert!(screen.contains("▶ 3 "), "{screen}");
        assert!(screen.contains("Esc clear"), "{screen}");

        app.prompt = Some(Prompt::new(PromptKind::Filter(FilterKind::Text), "warn"));
        let screen = render(&mut app, 80, 8);
        assert!(
            screen.lines().last().unwrap().starts_with("/warn"),
            "{screen}"
        );

        app.prompt = None;
        app.text_now("nothing-matches-this");
        let screen = render(&mut app, 80, 8);
        assert!(screen.contains("(no match — Esc to clear)"), "{screen}");
        assert!(screen.contains(" Detail "), "{screen}");
    }

    #[test]
    fn jq_filter_and_prompt_error_rendering() {
        let mut app = fixture_app();
        app.start_filter(Spec::new(FilterKind::Jq, ".code >= 500"))
            .unwrap();
        let screen = render(&mut app, 80, 8);
        assert!(screen.contains("|.code >= 500 …"), "running: {screen}");
        app.wait_job();
        let screen = render(&mut app, 80, 8);
        assert!(screen.contains("1 / 5"), "{screen}");
        assert!(screen.contains("└ |.code >= 500 "), "{screen}");

        let mut p = Prompt::new(PromptKind::Filter(FilterKind::Jq), ".[");
        p.error = Some("expected term".into());
        app.prompt = Some(p);
        let screen = render(&mut app, 80, 8);
        let last: Vec<&str> = screen.lines().rev().take(2).collect();
        assert_eq!(last, ["|.[", "✗ expected term"], "{screen}");
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
