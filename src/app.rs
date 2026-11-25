use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use ratatui::text::Line;

use crate::clipboard::Clipboard;
use crate::filter::{FilterKind, Job, JobResult, Matcher, Spec};
use crate::projection::Projection;
use crate::prompt::{Outcome, Prompt, PromptKind};
use crate::render::highlight::highlight_line;
use crate::render::pretty::{Theme, pretty};
use crate::source::{Source, View};
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
    pub highlight: Option<String>,
    pub view: Option<String>,
    pub lines: Vec<Line<'static>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Info(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

pub struct App {
    pub path: PathBuf,
    pub source: Arc<Source>,
    pub color: bool,
    /// Colored theme (from the config file).
    pub theme: Theme,
    pub wrap: bool,
    /// List pane width in % of the screen.
    pub list_width: u16,
    pub focus: Focus,
    /// Line indices currently shown in the list (all lines, or filter matches).
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
    /// Filter whose results are currently shown, if any.
    pub filter: Option<Spec>,
    /// Filter being computed in the background; replaces `filter` when done.
    pub job: Option<Job>,
    /// jq view applied to every line for display.
    pub view: Option<Projection>,
    preview_cache: HashMap<usize, String>,
    /// One-off message shown in the footer until the next key press.
    pub status: Option<Status>,
    clipboard: Clipboard,
    /// Whether the key help popup is shown; any key closes it.
    pub show_help: bool,
    /// Open input prompt; while set, it receives all keys.
    pub prompt: Option<Prompt>,
    /// Matcher of `filter`, applied to lines appended while streaming.
    matcher: Option<Box<dyn Matcher>>,
    /// Number of source lines the app has taken into account.
    pub total: usize,
    /// Source generation seen last; a change means the data was replaced.
    generation: u64,
    /// Follow mode: stick to the last line as new lines arrive (when the
    /// selection is on the last line).
    pub follow: bool,
    validity: Vec<Validity>,
    detail_cache: Option<DetailCache>,
    should_quit: bool,
}

impl App {
    pub fn new(path: PathBuf, source: impl Into<Arc<Source>>, color: bool) -> Self {
        let source = source.into();
        let total = source.len();
        let generation = source.generation();
        let visible = (0..total).collect();
        let validity = vec![Validity::Unknown; total];
        Self {
            path,
            source,
            color,
            theme: Theme::colored(),
            wrap: true,
            list_width: 40,
            focus: Focus::List,
            visible,
            selected: 0,
            list_offset: 0,
            detail_scroll: 0,
            detail_hscroll: 0,
            list_height: 1,
            detail_height: 1,
            detail_max_scroll: 0,
            filter: None,
            job: None,
            view: None,
            preview_cache: HashMap::new(),
            status: None,
            clipboard: Clipboard::default(),
            show_help: false,
            prompt: None,
            matcher: None,
            total,
            generation,
            follow: false,
            validity,
            detail_cache: None,
            should_quit: false,
        }
    }

    /// Theme for JSON rendering: the configured one, or plain when colors
    /// are off.
    pub fn active_theme(&self) -> Theme {
        if self.color {
            self.theme
        } else {
            Theme::plain()
        }
    }

    /// Whether more data may still arrive (stdin being read, followed file).
    pub fn is_live(&self) -> bool {
        !self.source.is_complete()
    }

    /// Take into account lines appended to the source since the last call,
    /// or start over if the data was replaced.
    pub fn sync_source(&mut self) {
        let generation = self.source.generation();
        if generation != self.generation {
            self.generation = generation;
            self.job = None;
            self.total = 0;
            self.visible.clear();
            self.validity.clear();
            self.selected = 0;
            self.list_offset = 0;
            self.detail_cache = None;
            self.preview_cache.clear();
            self.status = Some(Status::Info("file was truncated; reloaded".into()));
        }
        if let Some(e) = self.source.error() {
            self.status = Some(Status::Error(e));
        }
        let len = self.source.len();
        if len <= self.total {
            return;
        }
        let at_end = if self.visible.is_empty() {
            self.follow
        } else {
            self.selected + 1 == self.visible.len()
        };
        self.validity.resize(len, Validity::Unknown);
        // While a job runs, the shown (old) filter keeps applying; the job's
        // result is completed with the lines it did not see in finish_job.
        let added = self.matching(self.total..len);
        self.visible.extend(added);
        self.total = len;
        if at_end && self.follow && !self.visible.is_empty() {
            self.select(self.visible.len() - 1);
        }
    }

    /// Lines of `range` accepted by the active filter (all without filter).
    fn matching(&self, range: std::ops::Range<usize>) -> Vec<usize> {
        match &self.matcher {
            None => range.collect(),
            Some(m) => {
                let view = self.source.read();
                range.filter(|&i| m.matches(view.raw(i))).collect()
            }
        }
    }

    /// Original (0-based) line index of the selection, if any.
    pub fn selected_line(&self) -> Option<usize> {
        self.visible.get(self.selected).copied()
    }

    /// Whether line `i` is valid JSON (cached; does not build a `Value`).
    pub fn validity(&mut self, i: usize) -> Validity {
        if self.validity[i] == Validity::Unknown {
            let view = self.source.read();
            let raw = view.raw(i);
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

    /// Query of the active text filter, whose matches get highlighted.
    pub fn highlight_query(&self) -> Option<&str> {
        self.filter
            .as_ref()
            .filter(|f| f.kind == FilterKind::Text)
            .map(|f| f.query.as_str())
    }

    /// Styled lines for the detail pane of line `i`, rendered once per
    /// (line, color, highlight) and cached.
    pub fn detail_lines(&mut self, i: usize) -> &[Line<'static>] {
        let highlight = self.highlight_query().map(str::to_string);
        let view_code = self.view.as_ref().map(|v| v.code.clone());
        let fresh = matches!(&self.detail_cache,
            Some(c) if c.line == i && c.color == self.color
                && c.highlight == highlight && c.view == view_code);
        if !fresh {
            let theme = self.active_theme();
            let projected = {
                let view = self.source.read();
                self.view
                    .as_ref()
                    .and_then(|p| p.render(view.raw(i), &theme))
            };
            let mut lines =
                projected.unwrap_or_else(|| render_detail(&self.source.read(), i, &theme));
            if let Some(q) = &highlight {
                let hl = theme.matched;
                lines = lines
                    .into_iter()
                    .map(|l| highlight_line(l, q, hl))
                    .collect();
            }
            self.detail_cache = Some(DetailCache {
                line: i,
                color: self.color,
                highlight,
                view: view_code,
                lines,
            });
        }
        &self.detail_cache.as_ref().expect("just filled").lines
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            self.sync_source();
            terminal.draw(|frame| ui::draw(frame, self))?;
            // While a filter runs or data streams in, wake up regularly.
            let ready = if self.job.is_some() {
                event::poll(Duration::from_millis(50))?
            } else if self.is_live() {
                event::poll(Duration::from_millis(100))?
            } else {
                true
            };
            if ready && let Event::Key(key) = event::read()? {
                // Windows reports both press and release; only act on press.
                if key.kind == KeyEventKind::Press {
                    self.on_key(key.code, key.modifiers);
                }
            }
            self.poll_job();
        }
        Ok(())
    }

    fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.status = None;
        if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }
        if self.show_help {
            self.show_help = false;
            return;
        }
        if let Some(prompt) = &mut self.prompt {
            match prompt.on_key(code, mods) {
                Outcome::Editing => {}
                Outcome::Cancel => self.prompt = None,
                Outcome::Submit => {
                    // Surrounding spaces may matter in a text query, not in jq.
                    let result = match prompt.kind {
                        PromptKind::Filter(FilterKind::Text) => {
                            let spec = Spec::new(FilterKind::Text, prompt.text.as_str());
                            self.start_filter(spec)
                        }
                        PromptKind::Filter(FilterKind::Jq) => {
                            let spec = Spec::new(FilterKind::Jq, prompt.text.trim());
                            self.start_filter(spec)
                        }
                        PromptKind::View => {
                            let code = prompt.text.trim().to_string();
                            self.set_view(&code)
                        }
                    };
                    match result {
                        Ok(()) => self.prompt = None,
                        Err(e) => {
                            if let Some(p) = &mut self.prompt {
                                p.error = Some(e);
                            }
                        }
                    }
                }
            }
            return;
        }
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Esc if self.job.is_some() => self.job = None,
            KeyCode::Esc if self.filter.is_some() => self.clear_filter(),
            KeyCode::Esc if self.view.is_some() => self.view = None,
            KeyCode::Esc => self.focus = Focus::List,
            KeyCode::Char('/') => self.open_prompt(PromptKind::Filter(FilterKind::Text)),
            KeyCode::Char('|') => self.open_prompt(PromptKind::Filter(FilterKind::Jq)),
            KeyCode::Char('>') => self.open_prompt(PromptKind::View),
            KeyCode::Char('c') => self.color = !self.color,
            KeyCode::Char('y') => self.copy_selected(false),
            KeyCode::Char('Y') => self.copy_selected(true),
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

    /// Text copied by `y` (pretty JSON, or the raw line if it is not JSON)
    /// or `Y` (raw line).
    pub fn copy_text(&self, raw: bool) -> Option<String> {
        let i = self.selected_line()?;
        let view = self.source.read();
        if !raw && let Some(text) = self.view.as_ref().and_then(|p| p.text(view.raw(i))) {
            return Some(text);
        }
        let line = view.line(i);
        if !raw
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
            && let Ok(pretty) = serde_json::to_string_pretty(&v)
        {
            return Some(pretty);
        }
        Some(line.into_owned())
    }

    fn copy_selected(&mut self, raw: bool) {
        let (Some(i), Some(text)) = (self.selected_line(), self.copy_text(raw)) else {
            return;
        };
        let what = if raw { "raw" } else { "pretty" };
        self.status = Some(match self.clipboard.copy(&text) {
            Ok(dest) => Status::Info(format!(
                "copied line {} ({what}, {} bytes) to the {dest}",
                i + 1,
                text.len()
            )),
            Err(e) => Status::Error(e),
        });
    }

    /// Open the input prompt, pre-filled with the active query of that kind
    /// (or the one still running).
    fn open_prompt(&mut self, kind: PromptKind) {
        let initial = match kind {
            PromptKind::Filter(fk) => {
                let current = self.job.as_ref().map(|j| &j.spec).or(self.filter.as_ref());
                current
                    .filter(|s| s.kind == fk)
                    .map_or(String::new(), |s| s.query.clone())
            }
            PromptKind::View => self.view.as_ref().map_or(String::new(), |v| v.code.clone()),
        };
        self.prompt = Some(Prompt::new(kind, &initial));
    }

    /// Set the jq view expression; empty clears it.
    pub fn set_view(&mut self, code: &str) -> Result<(), String> {
        self.view = if code.is_empty() {
            None
        } else {
            Some(Projection::new(code)?)
        };
        self.preview_cache.clear();
        Ok(())
    }

    /// List text of line `i` under the jq view (cached), or `None` without a
    /// view or when the line is not JSON.
    pub fn view_preview(&mut self, i: usize) -> Option<String> {
        let view = self.view.as_ref()?;
        if let Some(p) = self.preview_cache.get(&i) {
            return Some(p.clone());
        }
        let preview = view.preview(self.source.read().raw(i))?;
        if self.preview_cache.len() > 10_000 {
            self.preview_cache.clear();
        }
        self.preview_cache.insert(i, preview.clone());
        Some(preview)
    }

    /// Start filtering in the background, cancelling any running filter. An
    /// empty query clears the filter. Fails on an invalid jq expression.
    pub fn start_filter(&mut self, spec: Spec) -> Result<(), String> {
        if spec.query.is_empty() {
            self.clear_filter();
            return Ok(());
        }
        let job = Job::spawn(Arc::clone(&self.source), self.total, spec)?;
        self.job = Some(job);
        Ok(())
    }

    /// Apply the result of the background filter if it has finished.
    pub fn poll_job(&mut self) {
        if let Some(job) = &mut self.job
            && let Some(result) = job.poll()
        {
            self.finish_job(result);
        }
    }

    /// Block until the running filter is done and apply it.
    #[cfg(test)]
    pub fn wait_job(&mut self) {
        if let Some(job) = &mut self.job {
            let result = job.wait();
            self.finish_job(result);
        }
    }

    fn finish_job(&mut self, result: JobResult) {
        let Some(job) = self.job.take() else { return };
        match result {
            Ok(mut lines) => {
                self.matcher = job.spec.matcher().ok();
                self.filter = Some(job.spec.clone());
                // Lines appended while the job ran.
                lines.extend(self.matching(job.total..self.total));
                self.set_visible(lines);
            }
            Err(e) => {
                self.status = Some(Status::Error(format!(
                    "{}{}: {e}",
                    job.spec.kind.symbol(),
                    job.spec.query
                )))
            }
        }
    }

    /// Run a text filter to completion (tests only).
    #[cfg(test)]
    pub fn text_now(&mut self, q: &str) {
        self.start_filter(Spec::new(FilterKind::Text, q)).unwrap();
        self.wait_job();
    }

    pub fn clear_filter(&mut self) {
        self.job = None;
        self.filter = None;
        self.matcher = None;
        self.set_visible((0..self.total).collect());
    }

    /// Replace the visible lines, keeping the selection on the same original
    /// line when it is still shown, otherwise on the next one after it.
    fn set_visible(&mut self, visible: Vec<usize>) {
        let prev = self.selected_line();
        self.visible = visible;
        self.list_offset = 0;
        let pos = prev.map_or(0, |l| self.visible.partition_point(|&v| v < l));
        self.selected = pos.min(self.visible.len().saturating_sub(1));
        if self.selected_line() != prev {
            self.detail_scroll = 0;
            self.detail_hscroll = 0;
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

fn render_detail(source: &View, i: usize, theme: &Theme) -> Vec<Line<'static>> {
    use ratatui::style::{Color, Modifier, Style};

    let raw = source.raw(i);
    if raw.iter().all(u8::is_ascii_whitespace) {
        return vec![Line::styled(
            "(empty line)",
            Style::new().fg(Color::DarkGray),
        )];
    }
    match serde_json::from_slice::<serde_json::Value>(raw) {
        Ok(value) => pretty(&value, theme),
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

    fn type_str(a: &mut App, s: &str) {
        for c in s.chars() {
            press(a, KeyCode::Char(c));
        }
    }

    #[test]
    fn text_filter_via_prompt_and_esc_clears() {
        // Lines: {"i":0} .. {"i":11}
        let mut a = app_with_lines(12);
        press(&mut a, KeyCode::Char('/'));
        assert!(a.prompt.is_some());
        // Keys typed in the prompt must not trigger shortcuts (q, c, ...).
        type_str(&mut a, "i\":1");
        press(&mut a, KeyCode::Enter);
        assert!(a.prompt.is_none());
        assert!(!a.should_quit);
        a.wait_job();
        assert_eq!(a.filter, Some(Spec::new(FilterKind::Text, "i\":1")));
        assert_eq!(a.visible, vec![1, 10, 11]);

        press(&mut a, KeyCode::Down);
        assert_eq!(a.selected_line(), Some(10));
        press(&mut a, KeyCode::Esc);
        assert_eq!(a.filter, None);
        assert_eq!(a.visible.len(), 12);
        assert_eq!(a.selected_line(), Some(10), "selection kept after clear");
    }

    #[test]
    fn filter_selects_next_match_after_previous_selection() {
        let mut a = app_with_lines(12);
        a.select(5);
        a.text_now("1");
        assert_eq!(a.visible, vec![1, 10, 11]);
        assert_eq!(a.selected_line(), Some(10));
        a.select(0);
        a.text_now("1}");
        assert_eq!(a.visible, vec![1, 11]);
        assert_eq!(a.selected_line(), Some(1));
    }

    #[test]
    fn filter_with_no_match_and_empty_query() {
        let mut a = app_with_lines(3);
        a.text_now("zzz");
        assert!(a.visible.is_empty());
        assert_eq!(a.selected_line(), None);
        press(&mut a, KeyCode::Down);
        a.text_now("");
        assert_eq!(a.filter, None);
        assert_eq!(a.visible.len(), 3);
    }

    #[test]
    fn prompt_is_prefilled_and_esc_cancels() {
        let mut a = app_with_lines(3);
        a.text_now("1");
        press(&mut a, KeyCode::Char('/'));
        assert_eq!(a.prompt.as_ref().unwrap().text, "1");
        press(&mut a, KeyCode::Esc);
        assert!(a.prompt.is_none());
        assert_eq!(a.filter.as_ref().unwrap().query, "1", "cancel keeps filter");
        // The jq prompt is not pre-filled with a text query.
        press(&mut a, KeyCode::Char('|'));
        assert_eq!(a.prompt.as_ref().unwrap().text, "");
    }

    #[test]
    fn jq_filter_via_prompt() {
        let mut a = app_with_lines(12);
        press(&mut a, KeyCode::Char('|'));
        type_str(&mut a, ".i >= 10");
        press(&mut a, KeyCode::Enter);
        assert!(a.prompt.is_none());
        a.wait_job();
        assert_eq!(a.visible, vec![10, 11]);
        assert_eq!(a.filter.as_ref().unwrap().kind, FilterKind::Jq);
    }

    #[test]
    fn invalid_jq_keeps_prompt_open_with_error() {
        let mut a = app_with_lines(3);
        a.text_now("1");
        press(&mut a, KeyCode::Char('|'));
        type_str(&mut a, ".i ==");
        press(&mut a, KeyCode::Enter);
        let p = a.prompt.as_ref().expect("prompt stays open");
        assert!(p.error.as_deref().unwrap().contains("expected"));
        assert!(a.job.is_none());
        assert_eq!(a.visible, vec![1], "previous filter untouched");
        // Editing clears the error.
        press(&mut a, KeyCode::Char(' '));
        assert!(a.prompt.as_ref().unwrap().error.is_none());
    }

    #[test]
    fn esc_cancels_running_job_then_clears_filter() {
        let mut a = app_with_lines(3);
        a.text_now("1");
        a.start_filter(Spec::new(FilterKind::Text, "2")).unwrap();
        press(&mut a, KeyCode::Esc);
        assert!(a.job.is_none());
        assert_eq!(a.filter.as_ref().unwrap().query, "1");
        press(&mut a, KeyCode::Esc);
        assert!(a.filter.is_none());
    }

    #[test]
    fn help_popup_closes_on_any_key() {
        let mut a = app_with_lines(3);
        press(&mut a, KeyCode::Char('?'));
        assert!(a.show_help);
        press(&mut a, KeyCode::Char('q'));
        assert!(!a.show_help);
        assert!(!a.should_quit, "key closing help is not processed further");
    }

    #[test]
    fn text_filter_highlights_detail() {
        let mut a = app_with_lines(3);
        a.text_now("\"i\"");
        let spans = &a.detail_lines(0)[1].spans;
        let hl = Theme::colored().matched;
        assert!(
            spans
                .iter()
                .any(|s| s.content == "\"i\"" && s.style == Theme::colored().key.patch(hl)),
            "{spans:?}"
        );
        // jq filters do not highlight.
        a.start_filter(Spec::new(FilterKind::Jq, ".i")).unwrap();
        a.wait_job();
        assert!(
            a.detail_lines(1)[1]
                .spans
                .iter()
                .all(|s| s.style.bg.is_none())
        );
    }

    fn streaming_app(follow: bool) -> (App, Arc<Source>) {
        let src = Arc::new(Source::streaming());
        let mut a = App::new(PathBuf::from("stdin"), Arc::clone(&src), true);
        a.follow = follow;
        (a, src)
    }

    fn line(i: usize) -> String {
        format!("{{\"i\":{i}}}\n")
    }

    #[test]
    fn streamed_lines_are_added_and_filtered() {
        let (mut a, src) = streaming_app(false);
        assert!(a.is_live());
        src.append(line(0).as_bytes());
        a.sync_source();
        assert_eq!(a.visible, vec![0]);

        a.text_now("1");
        src.append((line(1) + &line(2) + &line(11)).as_bytes());
        a.sync_source();
        assert_eq!(a.visible, vec![1, 3], "new lines go through the filter");
        assert_eq!(a.selected_line(), Some(1), "no follow: selection stays");

        a.clear_filter();
        assert_eq!(a.visible, vec![0, 1, 2, 3]);
        src.finish();
        assert!(!a.is_live());
    }

    #[test]
    fn lines_appended_during_a_job_are_filtered_when_it_ends() {
        let (mut a, src) = streaming_app(false);
        src.append((line(1) + &line(2)).as_bytes());
        a.sync_source();
        a.start_filter(Spec::new(FilterKind::Text, "1")).unwrap();
        src.append((line(10) + &line(3)).as_bytes());
        a.sync_source(); // job still pending or done: either way no loss
        a.wait_job();
        assert_eq!(a.visible, vec![0, 2]);
    }

    #[test]
    fn follow_sticks_to_last_line_only_when_on_it() {
        let (mut a, src) = streaming_app(true);
        src.append((line(0) + &line(1)).as_bytes());
        a.sync_source();
        assert_eq!(a.selected_line(), Some(1), "starts at the end");
        src.append(line(2).as_bytes());
        a.sync_source();
        assert_eq!(a.selected_line(), Some(2));
        press(&mut a, KeyCode::Up);
        src.append(line(3).as_bytes());
        a.sync_source();
        assert_eq!(a.selected_line(), Some(1), "moved away: stays put");
    }

    #[test]
    fn reset_source_restarts_app_state() {
        let (mut a, src) = streaming_app(true);
        src.append((line(0) + &line(1)).as_bytes());
        a.sync_source();
        src.reset();
        src.append(line(7).as_bytes());
        a.sync_source();
        assert_eq!(a.total, 1);
        assert_eq!(a.visible, vec![0]);
        assert!(matches!(&a.status, Some(Status::Info(m)) if m.contains("truncated")));
    }

    #[test]
    fn copy_text_pretty_and_raw() {
        let src = Source::from_bytes(b"{\"b\":1,\"a\":[1]}\nnot json\n".to_vec());
        let mut a = App::new(PathBuf::from("x"), src, true);
        assert_eq!(
            a.copy_text(false).unwrap(),
            "{\n  \"b\": 1,\n  \"a\": [\n    1\n  ]\n}"
        );
        assert_eq!(a.copy_text(true).unwrap(), "{\"b\":1,\"a\":[1]}");
        press(&mut a, KeyCode::Down);
        assert_eq!(a.copy_text(false).unwrap(), "not json");
    }

    #[test]
    fn jq_view_via_prompt_changes_preview_detail_and_copy() {
        let src = Source::from_bytes(b"{\"a\":{\"b\":1},\"m\":\"hi\"}\nnot json\n".to_vec());
        let mut a = App::new(PathBuf::from("x"), src, false);
        press(&mut a, KeyCode::Char('>'));
        type_str(&mut a, ".a");
        press(&mut a, KeyCode::Enter);
        assert!(a.prompt.is_none());
        let text = |lines: &[Line]| lines.iter().map(|l| l.to_string()).collect::<Vec<_>>();
        assert_eq!(text(a.detail_lines(0)), ["{", "  \"b\": 1", "}"]);
        assert_eq!(a.view_preview(0).as_deref(), Some(r#"{"b":1}"#));
        assert_eq!(a.copy_text(false).unwrap(), "{\n  \"b\": 1\n}");
        assert_eq!(a.copy_text(true).unwrap(), r#"{"a":{"b":1},"m":"hi"}"#);
        // Non-JSON lines are shown as before.
        assert_eq!(a.view_preview(1), None);
        assert!(text(a.detail_lines(1))[0].starts_with("✗ invalid JSON"));

        // Reopening the prompt is pre-filled; Esc clears the view.
        press(&mut a, KeyCode::Char('>'));
        assert_eq!(a.prompt.as_ref().unwrap().text, ".a");
        press(&mut a, KeyCode::Esc);
        press(&mut a, KeyCode::Esc);
        assert!(a.view.is_none());
        assert_eq!(text(a.detail_lines(0))[1], "  \"a\": {");
    }

    #[test]
    fn invalid_view_keeps_prompt_open() {
        let mut a = app_with_lines(1);
        press(&mut a, KeyCode::Char('>'));
        type_str(&mut a, ".[");
        press(&mut a, KeyCode::Enter);
        assert!(a.prompt.as_ref().unwrap().error.is_some());
        assert!(a.view.is_none());
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
