use std::path::PathBuf;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use ratatui::text::Line;

use crate::render::pretty::{Theme, pretty};
use crate::source::Source;
use crate::ui;

/// Parse state of a line, computed lazily and cached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validity {
    Unknown,
    Valid,
    /// Empty or whitespace-only: tolerated in NDJSON, not flagged as an error.
    Blank,
    Invalid,
}

/// Rendered detail view of one line, reused across frames.
pub struct DetailCache {
    pub line: usize,
    pub color: bool,
    pub lines: Vec<Line<'static>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

pub struct App {
    pub path: PathBuf,
    pub source: Source,
    pub color: bool,
    pub wrap: bool,
    pub focus: Focus,
    /// Line indices currently shown in the list (all lines until filters land).
    pub visible: Vec<usize>,
    /// Position in `visible` of the selected entry.
    pub selected: usize,
    /// First `visible` position drawn in the list; maintained by the UI.
    pub list_offset: usize,
    pub detail_scroll: u16,
    pub detail_hscroll: u16,
    /// Inner heights of the panes from the last draw, used for paging.
    pub list_height: usize,
    pub detail_height: usize,
    /// Max vertical scroll of the detail pane from the last draw.
    pub detail_max_scroll: u16,
    validity: Vec<Validity>,
    detail_cache: Option<DetailCache>,
    should_quit: bool,
}

impl App {
    pub fn new(path: PathBuf, source: Source, color: bool) -> Self {
        let visible = (0..source.len()).collect();
        let validity = vec![Validity::Unknown; source.len()];
        Self {
            path,
            source,
            color,
            wrap: true,
            focus: Focus::List,
            visible,
            selected: 0,
            list_offset: 0,
            detail_scroll: 0,
            detail_hscroll: 0,
            list_height: 1,
            detail_height: 1,
            detail_max_scroll: 0,
            validity,
            detail_cache: None,
            should_quit: false,
        }
    }

    /// Original (0-based) line index of the selection, if any.
    pub fn selected_line(&self) -> Option<usize> {
        self.visible.get(self.selected).copied()
    }

    /// Whether line `i` is valid JSON (cached; does not build a `Value`).
    pub fn validity(&mut self, i: usize) -> Validity {
        if self.validity[i] == Validity::Unknown {
            let raw = self.source.raw(i);
            self.validity[i] = if raw.iter().all(u8::is_ascii_whitespace) {
                Validity::Blank
            } else if serde_json::from_slice::<serde::de::IgnoredAny>(raw).is_ok() {
                Validity::Valid
            } else {
                Validity::Invalid
            };
        }
        self.validity[i]
    }

    /// Styled lines for the detail pane of line `i`, rendered once per
    /// (line, color) and cached.
    pub fn detail_lines(&mut self, i: usize) -> &[Line<'static>] {
        let fresh = matches!(&self.detail_cache, Some(c) if c.line == i && c.color == self.color);
        if !fresh {
            let lines = render_detail(&self.source, i, self.color);
            self.detail_cache = Some(DetailCache {
                line: i,
                color: self.color,
                lines,
            });
        }
        &self.detail_cache.as_ref().expect("just filled").lines
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| ui::draw(frame, self))?;
            if let Event::Key(key) = event::read()? {
                // Windows reports both press and release; only act on press.
                if key.kind == KeyEventKind::Press {
                    self.on_key(key.code, key.modifiers);
                }
            }
        }
        Ok(())
    }

    fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => self.should_quit = true,
            KeyCode::Char('c') => self.color = !self.color,
            KeyCode::Char('w') => {
                self.wrap = !self.wrap;
                self.detail_scroll = 0;
                self.detail_hscroll = 0;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::List => Focus::Detail,
                    Focus::Detail => Focus::List,
                }
            }
            _ => match self.focus {
                Focus::List => self.on_list_key(code),
                Focus::Detail => self.on_detail_key(code),
            },
        }
    }

    fn on_list_key(&mut self, code: KeyCode) {
        let page = self.list_height.max(1) as isize;
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.move_selection(page),
            KeyCode::PageUp => self.move_selection(-page),
            KeyCode::Home | KeyCode::Char('g') => self.select(0),
            KeyCode::End | KeyCode::Char('G') => self.select(usize::MAX),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Detail,
            _ => {}
        }
    }

    fn on_detail_key(&mut self, code: KeyCode) {
        let page = self.detail_height.max(1) as u16;
        let max = self.detail_max_scroll;
        let s = &mut self.detail_scroll;
        match code {
            KeyCode::Down | KeyCode::Char('j') => *s = s.saturating_add(1).min(max),
            KeyCode::Up | KeyCode::Char('k') => *s = s.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => *s = s.saturating_add(page).min(max),
            KeyCode::PageUp => *s = s.saturating_sub(page),
            KeyCode::Home | KeyCode::Char('g') => *s = 0,
            KeyCode::End | KeyCode::Char('G') => *s = max,
            KeyCode::Right | KeyCode::Char('l') if !self.wrap => {
                self.detail_hscroll = self.detail_hscroll.saturating_add(4)
            }
            KeyCode::Left | KeyCode::Char('h') if !self.wrap && self.detail_hscroll > 0 => {
                self.detail_hscroll = self.detail_hscroll.saturating_sub(4)
            }
            KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::List,
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize) {
        self.select(self.selected.saturating_add_signed(delta));
    }

    /// Select `visible[pos]`, clamped to the list; resets the detail scroll.
    fn select(&mut self, pos: usize) {
        let pos = pos.min(self.visible.len().saturating_sub(1));
        if pos != self.selected {
            self.selected = pos;
            self.detail_scroll = 0;
            self.detail_hscroll = 0;
        }
    }
}

fn render_detail(source: &Source, i: usize, color: bool) -> Vec<Line<'static>> {
    use ratatui::style::{Color, Modifier, Style};

    let raw = source.raw(i);
    if raw.iter().all(u8::is_ascii_whitespace) {
        return vec![Line::styled(
            "(empty line)",
            Style::new().fg(Color::DarkGray),
        )];
    }
    match serde_json::from_slice::<serde_json::Value>(raw) {
        Ok(value) => pretty(&value, &Theme::for_color(color)),
        Err(err) => {
            let err_style = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);
            vec![
                Line::styled(format!("✗ invalid JSON: {err}"), err_style),
                Line::default(),
                Line::raw(source.line(i).into_owned()),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn app_with_lines(n: usize) -> App {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for i in 0..n {
            writeln!(f, "{{\"i\":{i}}}").unwrap();
        }
        let source = Source::open(f.path()).unwrap();
        App::new(f.path().to_path_buf(), source, true)
    }

    fn press(a: &mut App, code: KeyCode) {
        a.on_key(code, KeyModifiers::NONE);
    }

    #[test]
    fn validity_and_detail_rendering() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"{\"a\":1}\nnope\n  \n").unwrap();
        let source = Source::open(f.path()).unwrap();
        let mut a = App::new(f.path().to_path_buf(), source, false);
        assert_eq!(a.validity(0), Validity::Valid);
        assert_eq!(a.validity(1), Validity::Invalid);
        assert_eq!(a.validity(2), Validity::Blank);

        let text = |lines: &[Line]| lines.iter().map(|l| l.to_string()).collect::<Vec<_>>();
        assert_eq!(text(a.detail_lines(0)), ["{", "  \"a\": 1", "}"]);
        let bad = text(a.detail_lines(1));
        assert!(bad[0].starts_with("✗ invalid JSON:"), "{bad:?}");
        assert_eq!(bad[2], "nope");
        assert_eq!(text(a.detail_lines(2)), ["(empty line)"]);
    }

    #[test]
    fn detail_cache_invalidates_on_color_toggle() {
        let mut a = app_with_lines(1);
        let colored = a.detail_lines(0).to_vec();
        press(&mut a, KeyCode::Char('c'));
        let plain = a.detail_lines(0).to_vec();
        assert_ne!(colored, plain);
    }

    #[test]
    fn tab_toggles_focus() {
        let mut a = app_with_lines(1);
        press(&mut a, KeyCode::Tab);
        assert_eq!(a.focus, Focus::Detail);
        press(&mut a, KeyCode::Tab);
        assert_eq!(a.focus, Focus::List);
    }

    #[test]
    fn c_toggles_color_and_ctrl_c_quits() {
        let mut a = app_with_lines(1);
        press(&mut a, KeyCode::Char('c'));
        assert!(!a.color);
        assert!(!a.should_quit);
        a.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(a.should_quit);
    }

    #[test]
    fn list_navigation_is_clamped() {
        let mut a = app_with_lines(10);
        a.list_height = 4;
        press(&mut a, KeyCode::Up);
        assert_eq!(a.selected, 0);
        press(&mut a, KeyCode::Char('j'));
        assert_eq!(a.selected, 1);
        press(&mut a, KeyCode::PageDown);
        assert_eq!(a.selected, 5);
        press(&mut a, KeyCode::PageDown);
        press(&mut a, KeyCode::PageDown);
        assert_eq!(a.selected, 9);
        press(&mut a, KeyCode::Char('g'));
        assert_eq!(a.selected, 0);
        press(&mut a, KeyCode::End);
        assert_eq!(a.selected_line(), Some(9));
    }

    #[test]
    fn empty_file_navigation_is_safe() {
        let mut a = app_with_lines(0);
        press(&mut a, KeyCode::Down);
        press(&mut a, KeyCode::End);
        assert_eq!(a.selected_line(), None);
    }

    #[test]
    fn changing_selection_resets_detail_scroll() {
        let mut a = app_with_lines(3);
        a.detail_max_scroll = 10;
        press(&mut a, KeyCode::Tab);
        press(&mut a, KeyCode::Down);
        press(&mut a, KeyCode::Down);
        assert_eq!(a.detail_scroll, 2);
        press(&mut a, KeyCode::Tab);
        press(&mut a, KeyCode::Down);
        assert_eq!(a.detail_scroll, 0);
    }

    #[test]
    fn detail_scroll_is_clamped() {
        let mut a = app_with_lines(1);
        a.detail_max_scroll = 1;
        press(&mut a, KeyCode::Tab);
        press(&mut a, KeyCode::PageDown);
        assert_eq!(a.detail_scroll, 1);
        press(&mut a, KeyCode::Up);
        press(&mut a, KeyCode::Up);
        assert_eq!(a.detail_scroll, 0);
    }
}
