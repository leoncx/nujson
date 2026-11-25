//! Minimal single-line text input used by the search/filter prompt.

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use crate::filter::FilterKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// Text (`/`) or jq (`|`) filter.
    Filter(FilterKind),
    /// jq view expression (`>`).
    View,
}

impl PromptKind {
    pub fn symbol(self) -> &'static str {
        match self {
            PromptKind::Filter(k) => k.symbol(),
            PromptKind::View => ">",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Editing,
    Submit,
    Cancel,
}

#[derive(Debug)]
pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    /// Error from the last submit (e.g. invalid jq); cleared on edit.
    pub error: Option<String>,
    /// Cursor position as a byte index into `text` (always on a char boundary).
    pub cursor: usize,
}

impl Prompt {
    pub fn new(kind: PromptKind, initial: &str) -> Self {
        Self {
            kind,
            error: None,
            text: initial.to_string(),
            cursor: initial.len(),
        }
    }

    /// Cursor position in chars, for placing the terminal cursor.
    pub fn cursor_col(&self) -> usize {
        self.text[..self.cursor].chars().count()
    }

    pub fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Outcome {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        if !matches!(code, KeyCode::Enter | KeyCode::Esc) {
            self.error = None;
        }
        match code {
            KeyCode::Enter => return Outcome::Submit,
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Char('u') if ctrl => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.text.len(),
            KeyCode::Char(c) if !ctrl => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace => {
                if let Some(c) = self.text[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                    self.text.remove(self.cursor);
                }
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            KeyCode::Left => {
                if let Some(c) = self.text[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                }
            }
            KeyCode::Right => {
                if let Some(c) = self.text[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            _ => {}
        }
        Outcome::Editing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(p: &mut Prompt, s: &str) {
        for c in s.chars() {
            p.on_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    #[test]
    fn edit_with_unicode_and_cursor_moves() {
        let mut p = Prompt::new(PromptKind::Filter(FilterKind::Text), "");
        typed(&mut p, "été");
        assert_eq!(p.cursor_col(), 3);
        p.on_key(KeyCode::Left, KeyModifiers::NONE);
        p.on_key(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(p.text, "éé");
        assert_eq!(p.cursor_col(), 1);
        typed(&mut p, "x");
        assert_eq!(p.text, "éxé");
        p.on_key(KeyCode::Home, KeyModifiers::NONE);
        p.on_key(KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(p.text, "xé");
    }

    #[test]
    fn ctrl_u_clears_before_cursor_and_outcomes() {
        let mut p = Prompt::new(PromptKind::Filter(FilterKind::Text), "abc");
        p.on_key(KeyCode::Left, KeyModifiers::NONE);
        p.on_key(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(p.text, "c");
        assert_eq!(p.cursor, 0);
        assert_eq!(
            p.on_key(KeyCode::Enter, KeyModifiers::NONE),
            Outcome::Submit
        );
        assert_eq!(p.on_key(KeyCode::Esc, KeyModifiers::NONE), Outcome::Cancel);
    }
}
