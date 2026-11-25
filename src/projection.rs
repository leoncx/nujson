//! jq "view": an expression applied to each line for display (list preview,
//! detail pane, copy), without filtering anything.

use ratatui::style::{Color, Style};
use ratatui::text::Line;
use serde_json::Value;

use crate::filter::jq::{Program, parse_line};
use crate::render::pretty::{Theme, pretty};

pub struct Projection {
    pub code: String,
    program: Program,
}

type Output = Result<Value, String>;

impl Projection {
    pub fn new(code: &str) -> Result<Self, String> {
        Ok(Self {
            code: code.to_string(),
            program: Program::new(code)?,
        })
    }

    /// Outputs on line `raw`, or `None` if the line is not JSON.
    fn outputs(&self, raw: &[u8]) -> Option<Vec<Output>> {
        let input = parse_line(raw)?;
        Some(
            self.program
                .run(input)
                .map(|r| r.map(|v| to_value(&v.to_string())))
                .collect(),
        )
    }

    /// One-line summary for the list: strings unquoted, other values as
    /// compact JSON, several outputs separated by two spaces.
    pub fn preview(&self, raw: &[u8]) -> Option<String> {
        let outputs = self.outputs(raw)?;
        if outputs.is_empty() {
            return Some("(no output)".into());
        }
        let parts: Vec<String> = outputs
            .into_iter()
            .map(|o| match o {
                Ok(Value::String(s)) => s,
                Ok(v) => v.to_string(),
                Err(e) => format!("✗ {e}"),
            })
            .collect();
        Some(parts.join("  "))
    }

    /// Styled detail view: each output pretty-printed, errors in red.
    pub fn render(&self, raw: &[u8], theme: &Theme) -> Option<Vec<Line<'static>>> {
        let outputs = self.outputs(raw)?;
        if outputs.is_empty() {
            return Some(vec![Line::styled(
                "(no output)",
                Style::new().fg(Color::DarkGray),
            )]);
        }
        let mut lines = Vec::new();
        for o in outputs {
            match o {
                Ok(v) => lines.extend(pretty(&v, theme)),
                Err(e) => lines.push(Line::styled(
                    format!("✗ jq error: {e}"),
                    Style::new().fg(Color::Red),
                )),
            }
        }
        Some(lines)
    }

    /// Text for the clipboard: outputs pretty-printed, one after another.
    pub fn text(&self, raw: &[u8]) -> Option<String> {
        let outputs = self.outputs(raw)?;
        let parts: Vec<String> = outputs
            .into_iter()
            .map(|o| match o {
                Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
                Err(e) => format!("error: {e}"),
            })
            .collect();
        Some(parts.join("\n"))
    }
}

/// jaq prints values as JSON; anything serde cannot read back (e.g. NaN) is
/// kept as its text.
fn to_value(json: &str) -> Value {
    serde_json::from_str(json).unwrap_or_else(|_| Value::String(json.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &[u8] = br#"{"ts":"10:00","msg":"boom","user":{"id":42,"tags":["a"]}}"#;

    fn text_of(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn preview_formats_strings_and_values() {
        let p = Projection::new(r#".ts + " " + .msg"#).unwrap();
        assert_eq!(p.preview(LINE).unwrap(), "10:00 boom");
        let p = Projection::new(".user.id, .user.tags").unwrap();
        assert_eq!(p.preview(LINE).unwrap(), r#"42  ["a"]"#);
        let p = Projection::new("empty").unwrap();
        assert_eq!(p.preview(LINE).unwrap(), "(no output)");
    }

    #[test]
    fn render_pretty_and_errors() {
        let p = Projection::new(".user").unwrap();
        let lines = p.render(LINE, &Theme::plain()).unwrap();
        assert_eq!(text_of(&lines)[..2], ["{", r#"  "id": 42,"#]);
        let p = Projection::new(".msg | .x").unwrap();
        let lines = p.render(LINE, &Theme::plain()).unwrap();
        assert!(
            text_of(&lines)[0].starts_with("✗ jq error:"),
            "{:?}",
            text_of(&lines)
        );
    }

    #[test]
    fn invalid_json_line_gives_none() {
        let p = Projection::new(".").unwrap();
        assert!(p.preview(b"nope").is_none());
        assert!(p.render(b"nope", &Theme::plain()).is_none());
    }

    #[test]
    fn copy_text_joins_pretty_outputs() {
        let p = Projection::new(".user.id, .user.tags").unwrap();
        assert_eq!(p.text(LINE).unwrap(), "42\n[\n  \"a\"\n]");
    }

    #[test]
    fn invalid_program() {
        assert!(Projection::new(".[").is_err());
    }
}
