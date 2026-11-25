//! Plain substring filter with smart case: case-insensitive unless the query
//! contains an uppercase character.

use std::cell::RefCell;

use memchr::memmem::Finder;

use super::Matcher;

pub struct TextMatcher {
    finder: Finder<'static>,
    mode: Mode,
    /// Scratch buffer for ASCII-lowercased lines (avoids an alloc per line).
    buf: RefCell<Vec<u8>>,
}

enum Mode {
    CaseSensitive,
    /// Query is ASCII: lowercase line bytes in place, positions are unchanged.
    AsciiInsensitive,
    /// Query has non-ASCII chars: full Unicode lowercasing of the line.
    UnicodeInsensitive(String),
}

impl TextMatcher {
    pub fn new(query: &str) -> Self {
        let case_sensitive = query.chars().any(char::is_uppercase);
        let (needle, mode) = if case_sensitive {
            (query.to_string(), Mode::CaseSensitive)
        } else if query.is_ascii() {
            (query.to_string(), Mode::AsciiInsensitive)
        } else {
            let lower = query.to_lowercase();
            (lower.clone(), Mode::UnicodeInsensitive(lower))
        };
        Self {
            finder: Finder::new(needle.as_bytes()).into_owned(),
            mode,
            buf: RefCell::new(Vec::new()),
        }
    }
}

impl Matcher for TextMatcher {
    fn matches(&self, raw: &[u8]) -> bool {
        match &self.mode {
            Mode::CaseSensitive => self.finder.find(raw).is_some(),
            Mode::AsciiInsensitive => {
                let mut buf = self.buf.borrow_mut();
                buf.clear();
                buf.extend(raw.iter().map(u8::to_ascii_lowercase));
                self.finder.find(&buf).is_some()
            }
            Mode::UnicodeInsensitive(needle) => String::from_utf8_lossy(raw)
                .to_lowercase()
                .contains(needle.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(q: &str, line: &str) -> bool {
        TextMatcher::new(q).matches(line.as_bytes())
    }

    #[test]
    fn lowercase_query_is_case_insensitive() {
        assert!(m("error", r#"{"level":"ERROR"}"#));
        assert!(m("error", "an error"));
        assert!(!m("error", "warn"));
    }

    #[test]
    fn uppercase_query_is_case_sensitive() {
        assert!(m("Error", "Error here"));
        assert!(!m("Error", "error here"));
    }

    #[test]
    fn unicode_query() {
        assert!(m("été", r#"{"saison":"ÉTÉ"}"#));
        assert!(!m("été", "hiver"));
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(m("", "anything"));
    }
}
