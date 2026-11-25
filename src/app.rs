use std::path::PathBuf;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

#[derive(Debug)]
pub struct App {
    pub path: PathBuf,
    pub color: bool,
    pub focus: Focus,
    should_quit: bool,
}

impl App {
    pub fn new(path: PathBuf, color: bool) -> Self {
        Self {
            path,
            color,
            focus: Focus::List,
            should_quit: false,
        }
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
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::List => Focus::Detail,
                    Focus::Detail => Focus::List,
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(PathBuf::from("x.ndjson"), true)
    }

    #[test]
    fn tab_toggles_focus() {
        let mut a = app();
        a.on_key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(a.focus, Focus::Detail);
        a.on_key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(a.focus, Focus::List);
    }

    #[test]
    fn c_toggles_color_and_ctrl_c_quits() {
        let mut a = app();
        a.on_key(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(!a.color);
        assert!(!a.should_quit);
        a.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(a.should_quit);
    }
}
