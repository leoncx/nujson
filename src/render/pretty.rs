//! Pretty-print a JSON value into styled ratatui lines.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

const INDENT: &str = "  ";

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub key: Style,
    pub string: Style,
    pub number: Style,
    pub boolean: Style,
    pub null: Style,
    pub punct: Style,
}

impl Theme {
    /// Colors close to jq's defaults.
    pub fn colored() -> Self {
        Self {
            key: Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
            string: Style::new().fg(Color::Green),
            number: Style::new().fg(Color::Cyan),
            boolean: Style::new().fg(Color::Yellow),
            null: Style::new().fg(Color::Magenta),
            punct: Style::new(),
        }
    }

    pub fn plain() -> Self {
        let s = Style::new();
        Self {
            key: s,
            string: s,
            number: s,
            boolean: s,
            null: s,
            punct: s,
        }
    }

    pub fn for_color(color: bool) -> Self {
        if color {
            Self::colored()
        } else {
            Self::plain()
        }
    }
}

/// Render `value` as indented JSON, one `Line` per output line.
pub fn pretty(value: &Value, theme: &Theme) -> Vec<Line<'static>> {
    let mut w = Writer {
        theme,
        lines: Vec::new(),
        cur: Vec::new(),
    };
    w.value(value, 0);
    w.finish_line();
    w.lines
}

struct Writer<'t> {
    theme: &'t Theme,
    lines: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
}

impl Writer<'_> {
    fn push(&mut self, text: impl Into<String>, style: Style) {
        self.cur.push(Span::styled(text.into(), style));
    }

    fn finish_line(&mut self) {
        self.lines.push(Line::from(std::mem::take(&mut self.cur)));
    }

    fn newline(&mut self, depth: usize) {
        self.finish_line();
        if depth > 0 {
            self.cur.push(Span::raw(INDENT.repeat(depth)));
        }
    }

    fn value(&mut self, v: &Value, depth: usize) {
        let t = *self.theme;
        match v {
            Value::Null => self.push("null", t.null),
            Value::Bool(b) => self.push(b.to_string(), t.boolean),
            Value::Number(n) => self.push(n.to_string(), t.number),
            Value::String(s) => self.push(quote(s), t.string),
            Value::Array(items) if items.is_empty() => self.push("[]", t.punct),
            Value::Object(map) if map.is_empty() => self.push("{}", t.punct),
            Value::Array(items) => {
                self.push("[", t.punct);
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.push(",", t.punct);
                    }
                    self.newline(depth + 1);
                    self.value(item, depth + 1);
                }
                self.newline(depth);
                self.push("]", t.punct);
            }
            Value::Object(map) => {
                self.push("{", t.punct);
                for (i, (k, item)) in map.iter().enumerate() {
                    if i > 0 {
                        self.push(",", t.punct);
                    }
                    self.newline(depth + 1);
                    self.push(quote(k), t.key);
                    self.push(": ", t.punct);
                    self.value(item, depth + 1);
                }
                self.newline(depth);
                self.push("}", t.punct);
            }
        }
    }
}

/// JSON-escape a string, including the surrounding quotes.
fn quote(s: &str) -> String {
    serde_json::to_string(s).expect("serializing a str cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text(json: &str) -> String {
        let v: Value = serde_json::from_str(json).unwrap();
        pretty(&v, &Theme::plain())
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn scalars() {
        assert_eq!(plain_text("null"), "null");
        assert_eq!(plain_text("true"), "true");
        assert_eq!(plain_text("12.5"), "12.5");
        assert_eq!(plain_text(r#""a\nb""#), r#""a\nb""#);
    }

    #[test]
    fn nested_structure_keeps_key_order() {
        let out = plain_text(r#"{"z":1,"a":[true,null,{}],"m":{"x":[]}}"#);
        assert_eq!(
            out,
            r#"{
  "z": 1,
  "a": [
    true,
    null,
    {}
  ],
  "m": {
    "x": []
  }
}"#
        );
    }

    #[test]
    fn big_numbers_are_preserved_verbatim() {
        assert_eq!(
            plain_text("123456789012345678901234567890"),
            "123456789012345678901234567890"
        );
    }

    #[test]
    fn colored_theme_styles_tokens() {
        let v: Value = serde_json::from_str(r#"{"k":"v"}"#).unwrap();
        let t = Theme::colored();
        let lines = pretty(&v, &t);
        let spans = &lines[1].spans;
        assert!(
            spans
                .iter()
                .any(|s| s.content == "\"k\"" && s.style == t.key)
        );
        assert!(
            spans
                .iter()
                .any(|s| s.content == "\"v\"" && s.style == t.string)
        );
    }
}
