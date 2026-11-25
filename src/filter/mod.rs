//! Line filters. A filter decides, from a line's raw bytes, whether it is kept.
//! Filters run on a background thread ([`Job`]) so the UI stays responsive.

pub mod jq;
pub mod text;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use crate::source::Source;

pub trait Matcher {
    fn matches(&self, raw: &[u8]) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    /// Plain-text filter (`/`).
    Text,
    /// jq expression (`|`).
    Jq,
}

impl FilterKind {
    pub fn symbol(self) -> &'static str {
        match self {
            FilterKind::Text => "/",
            FilterKind::Jq => "|",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub kind: FilterKind,
    pub query: String,
}

impl Spec {
    pub fn new(kind: FilterKind, query: impl Into<String>) -> Self {
        Self {
            kind,
            query: query.into(),
        }
    }

    /// Build the matcher; fails with a readable message on invalid jq.
    pub fn matcher(&self) -> Result<Box<dyn Matcher>, String> {
        Ok(match self.kind {
            FilterKind::Text => Box::new(text::TextMatcher::new(&self.query)),
            FilterKind::Jq => Box::new(jq::JqMatcher::new(&self.query)?),
        })
    }
}

/// Lines per work unit; also the granularity of progress and cancellation.
const CHUNK: usize = 8192;

/// Final outcome of a job: matching line indices, or why it failed.
pub type JobResult = Result<Vec<usize>, String>;

/// A filter running over all lines on background threads (one per core).
/// Dropping the job cancels it.
pub struct Job {
    pub spec: Spec,
    pub total: usize,
    done: Arc<AtomicUsize>,
    rx: Receiver<JobResult>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    /// Compile `spec` on the caller's thread, so errors are reported
    /// immediately, then start filtering the first `total` lines; lines
    /// appended later are the caller's business.
    pub fn spawn(source: Arc<Source>, total: usize, spec: Spec) -> Result<Self, String> {
        spec.matcher()?;
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicUsize::new(0));
        {
            let (spec, cancel, done) = (spec.clone(), Arc::clone(&cancel), Arc::clone(&done));
            thread::spawn(move || {
                let result = run_parallel(&source, total, &spec, &cancel, &done);
                if !cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                }
            });
        }
        Ok(Self {
            spec,
            total,
            done,
            rx,
            cancel,
        })
    }

    /// Returns the outcome once the job has finished.
    pub fn poll(&mut self) -> Option<JobResult> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("filter thread died".into())),
        }
    }

    /// Block until finished.
    #[cfg(test)]
    pub fn wait(&mut self) -> JobResult {
        self.rx
            .recv()
            .unwrap_or_else(|_| Err("filter thread died".into()))
    }

    pub fn percent(&self) -> usize {
        let done = self.done.load(Ordering::Relaxed);
        (done * 100).checked_div(self.total).unwrap_or(100)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Split the lines into chunks handed out to one worker per core. Each worker
/// builds its own matcher (jaq values are not thread-safe).
fn run_parallel(
    source: &Source,
    total: usize,
    spec: &Spec,
    cancel: &AtomicBool,
    done: &AtomicUsize,
) -> JobResult {
    let n_chunks = total.div_ceil(CHUNK);
    let workers = thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(n_chunks)
        .max(1);
    let next_chunk = AtomicUsize::new(0);

    let mut parts: Vec<(usize, Vec<usize>)> = thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let matcher = spec.matcher().expect("validated in Job::spawn");
                    let mut parts = Vec::new();
                    while !cancel.load(Ordering::Relaxed) {
                        let chunk = next_chunk.fetch_add(1, Ordering::Relaxed);
                        if chunk >= n_chunks {
                            break;
                        }
                        let start = chunk * CHUNK;
                        let end = (start + CHUNK).min(total);
                        // One read lock per chunk, so streamed appends are
                        // not blocked for long.
                        let view = source.read();
                        if end > view.len() {
                            break; // data was reset (followed file truncated)
                        }
                        let hits = (start..end)
                            .filter(|&i| matcher.matches(view.raw(i)))
                            .collect();
                        drop(view);
                        parts.push((chunk, hits));
                        done.fetch_add(end - start, Ordering::Relaxed);
                    }
                    parts
                })
            })
            .collect();
        let mut all = Vec::new();
        for h in handles {
            all.extend(h.join().map_err(|_| "filter crashed".to_string())?);
        }
        Ok::<_, String>(all)
    })?;

    parts.sort_unstable_by_key(|(chunk, _)| *chunk);
    Ok(parts.into_iter().flat_map(|(_, hits)| hits).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn source(n: usize) -> Arc<Source> {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for i in 0..n {
            writeln!(f, "{{\"i\":{i}}}").unwrap();
        }
        Arc::new(Source::open(f.path()).unwrap())
    }

    #[test]
    fn job_runs_across_chunks() {
        // Several chunks, so results from different workers must be merged
        // back in order.
        let src = source(CHUNK * 2 + 5);
        let n = src.len();
        let mut job = Job::spawn(src, n, Spec::new(FilterKind::Jq, ".i % 1000 == 0")).unwrap();
        let lines = job.wait().unwrap();
        assert_eq!(lines, (0..=16000).step_by(1000).collect::<Vec<_>>());
        assert_eq!(job.percent(), 100);
    }

    #[test]
    fn invalid_jq_fails_at_spawn() {
        assert!(Job::spawn(source(1), 1, Spec::new(FilterKind::Jq, ".[")).is_err());
    }

    #[test]
    fn empty_source() {
        let mut job = Job::spawn(source(0), 0, Spec::new(FilterKind::Text, "x")).unwrap();
        assert!(job.wait().unwrap().is_empty());
        assert_eq!(job.percent(), 100);
    }
}
