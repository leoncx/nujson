//! Plain substring filter with smart case: case-insensitive unless the query
//! contains an uppercase character.

use std::cell::RefCell;
use std::ops::Range;

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

/// Byte ranges of the non-overlapping occurrences of `query` in `hay`, with
/// the same smart-case rule as [`TextMatcher`]. Used to highlight matches.
pub fn find_ranges(query: &str, hay: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    if query.chars().any(char::is_uppercase) {
        return hay
            .match_indices(query)
            .map(|(i, m)| i..i + m.len())
            .collect();
    }
    // Lowercasing can change byte lengths, so keep a map from each byte of
    // the lowercased text back to the start of its original char.
    let mut lower = String::with_capacity(hay.len());
    let mut origin = Vec::with_capacity(hay.len() + 1);
    for (i, ch) in hay.char_indices() {
        for lc in ch.to_lowercase() {
            lower.push(lc);
            origin.extend(std::iter::repeat_n(i, lc.len_utf8()));
        }
    }
    origin.push(hay.len());
    let needle = query.to_lowercase();
    lower
        .match_indices(&needle)
        .map(|(i, _)| origin[i]..origin[i + needle.len()])
        .filter(|r| !r.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_ranges_smart_case_and_unicode() {
        assert_eq!(find_ranges("ab", "xABxab"), vec![1..3, 4..6]);
        assert_eq!(find_ranges("AB", "xABxab"), vec![1..3]);
        // "É" is 2 bytes; ranges must point into the original string.
        let hay = "aÉtÉ été";
        let r = find_ranges("été", hay);
        assert_eq!(
            r.iter().map(|r| &hay[r.clone()]).collect::<Vec<_>>(),
            ["ÉtÉ", "été"]
        );
        assert!(find_ranges("", "abc").is_empty());
    }

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
