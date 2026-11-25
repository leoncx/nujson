//! Highlight text-filter matches inside already-styled lines.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::filter::text::find_ranges;

pub fn match_style(color: bool) -> Style {
    if color {
        Style::new().bg(Color::Yellow).fg(Color::Black)
    } else {
        Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }
}

/// Split the spans of `line` so occurrences of `query` (smart case) are
/// patched with `hl`, keeping the original styles elsewhere. Matches may
/// cross span boundaries.
pub fn highlight_line(line: Line<'static>, query: &str, hl: Style) -> Line<'static> {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let ranges = find_ranges(query, &text);
    if ranges.is_empty() {
        return line;
    }
    let (style, alignment) = (line.style, line.alignment);
    let mut spans = Vec::with_capacity(line.spans.len() + 2 * ranges.len());
    let mut next = 0; // first range that may still apply
    let mut pos = 0; // byte offset of the current span in `text`
    for span in line.spans {
        let (start, end) = (pos, pos + span.content.len());
        let mut cur = start;
        while cur < end {
            while next < ranges.len() && ranges[next].end <= cur {
                next += 1;
            }
            let (seg_end, hit) = match ranges.get(next) {
                Some(r) if r.start <= cur => (r.end.min(end), true),
                Some(r) => (r.start.min(end), false),
                None => (end, false),
            };
            let piece = span.content[cur - start..seg_end - start].to_string();
            let piece_style = if hit {
                span.style.patch(hl)
            } else {
                span.style
            };
            spans.push(Span::styled(piece, piece_style));
            cur = seg_end;
        }
        pos = end;
    }
    let mut out = Line::from(spans).style(style);
    out.alignment = alignment;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_spans_and_crosses_boundaries() {
        let key = Style::new().fg(Color::Blue);
        let line = Line::from(vec![
            Span::styled("\"level\"", key),
            Span::raw(": "),
            Span::raw("\"error\""),
        ]);
        let hl = match_style(true);
        let out = highlight_line(line, "l\": \"err", hl);
        let parts: Vec<(&str, Style)> = out
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect();
        assert_eq!(
            parts,
            [
                ("\"leve", key),
                ("l\"", key.patch(hl)),
                (": ", hl),
                ("\"err", hl),
                ("or\"", Style::new()),
            ]
        );
    }

    #[test]
    fn no_match_returns_line_unchanged() {
        let line = Line::from("abc");
        assert_eq!(highlight_line(line.clone(), "z", match_style(true)), line);
    }
}
