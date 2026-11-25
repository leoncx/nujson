use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Focus};

pub fn draw(frame: &mut Frame, app: &App) {
    let [main, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(main);

    let file_name = app
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    frame.render_widget(
        Paragraph::new("(line list — M2)").block(pane(
            format!(" Lines · {file_name} "),
            app.focus == Focus::List,
        )),
        left,
    );
    frame.render_widget(
        Paragraph::new("(pretty view — M3)").block(pane(" Detail ", app.focus == Focus::Detail)),
        right,
    );

    let key = Style::new().add_modifier(Modifier::BOLD);
    let color_state = if app.color { "on" } else { "off" };
    let help = Line::from(vec![
        Span::styled(" Tab", key),
        Span::raw(" focus  "),
        Span::styled("c", key),
        Span::raw(format!(" color ({color_state})  ")),
        Span::styled("q", key),
        Span::raw(" quit"),
    ]);
    frame.render_widget(Paragraph::new(help), footer);
}

fn pane(title: impl Into<String>, focused: bool) -> Block<'static> {
    let border = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    Block::bordered().title(title.into()).border_style(border)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use std::path::PathBuf;

    #[test]
    fn renders_both_panes_and_footer() {
        let app = App::new(PathBuf::from("logs.ndjson"), true);
        let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("Lines · logs.ndjson"));
        assert!(text.contains("Detail"));
        assert!(text.contains("color (on)"));
    }
}
