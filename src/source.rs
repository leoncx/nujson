//! NDJSON source with a line index.
//!
//! A regular file is memory-mapped once and scanned for `\n` to record where
//! each line starts and ends. Streams (stdin, followed files) are read into a
//! growing in-memory buffer by a background thread and indexed as data
//! arrives. Lines are only decoded on demand.
//!
//! The source is shared (`Arc<Source>`) between the UI and filter threads and
//! guarded by a `RwLock`: readers take a [`View`]; only stream readers write.
//! Never hold a `View` while calling a method that writes.

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::{Deref, Range};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use memmap2::Mmap;

enum Bytes {
    Mapped(Mmap),
    /// Data read into memory (stdin, followed files).
    Owned(Vec<u8>),
}

impl Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Mapped(m) => m,
            Bytes::Owned(v) => v,
        }
    }
}

struct Inner {
    bytes: Bytes,
    /// Byte range of each line, excluding the `\n` and any trailing `\r`.
    lines: Vec<Range<usize>>,
    /// Bytes scanned so far: offset just after the last indexed `\n`.
    scanned: usize,
    /// Whether the last entry of `lines` is an unterminated trailing line.
    has_partial: bool,
    /// No more data will arrive: an unterminated last line is indexed too.
    complete: bool,
    /// Bumped when the data is replaced (followed file truncated).
    generation: u64,
    /// I/O error from the background reader, if any.
    error: Option<String>,
}

impl Inner {
    fn new(bytes: Bytes, complete: bool) -> Self {
        let mut inner = Self {
            bytes,
            lines: Vec::new(),
            scanned: 0,
            has_partial: false,
            complete,
            generation: 0,
            error: None,
        };
        inner.index_more();
        inner
    }

    /// Index lines from `scanned` to the end of the data.
    fn index_more(&mut self) {
        if self.has_partial {
            self.lines.pop();
            self.has_partial = false;
        }
        let Inner {
            bytes,
            lines,
            scanned,
            ..
        } = self;
        let data: &[u8] = bytes;
        let base = *scanned;
        let mut start = base;
        for rel in memchr::memchr_iter(b'\n', &data[base..]) {
            let nl = base + rel;
            lines.push(trim_cr(data, start, nl));
            start = nl + 1;
        }
        *scanned = start;
        if self.complete && start < data.len() {
            self.lines.push(trim_cr(data, start, data.len()));
            self.has_partial = true;
        }
    }
}

pub struct Source {
    inner: RwLock<Inner>,
}

/// Read access to the source; holds a read lock while alive.
pub struct View<'a>(RwLockReadGuard<'a, Inner>);

impl View<'_> {
    pub fn len(&self) -> usize {
        self.0.lines.len()
    }

    /// Raw bytes of line `i` (0-based), without line terminator.
    pub fn raw(&self, i: usize) -> &[u8] {
        &self.0.bytes[self.0.lines[i].clone()]
    }

    /// Line `i` as text; invalid UTF-8 is replaced with U+FFFD.
    pub fn line(&self, i: usize) -> Cow<'_, str> {
        String::from_utf8_lossy(self.raw(i))
    }
}

impl Source {
    /// Memory-map a regular file.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        let len = file.metadata()?.len();
        let bytes = if len == 0 {
            // Mapping a zero-length file fails on some platforms.
            Bytes::Owned(Vec::new())
        } else {
            // SAFETY: the map is read-only. If another process truncates the
            // file while we run, reads may fault; this is the accepted
            // trade-off of mmap-based viewers (less, bat, ripgrep do the same).
            // Follow mode does not use mmap for that reason.
            Bytes::Mapped(
                unsafe { Mmap::map(&file) }
                    .with_context(|| format!("cannot map {}", path.display()))?,
            )
        };
        Ok(Self::with(Inner::new(bytes, true)))
    }

    /// In-memory data, complete.
    #[cfg(test)]
    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self::with(Inner::new(Bytes::Owned(data), true))
    }

    /// Empty source filled later by [`Source::append`] / [`spawn_reader`].
    pub fn streaming() -> Self {
        Self::with(Inner::new(Bytes::Owned(Vec::new()), false))
    }

    fn with(inner: Inner) -> Self {
        Self {
            inner: RwLock::new(inner),
        }
    }

    pub fn read(&self) -> View<'_> {
        View(self.inner.read().unwrap_or_else(|e| e.into_inner()))
    }

    fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }

    pub fn len(&self) -> usize {
        self.read().len()
    }

    /// Whether all data has been read (always true for mapped files).
    pub fn is_complete(&self) -> bool {
        self.read().0.complete
    }

    pub fn generation(&self) -> u64 {
        self.read().0.generation
    }

    pub fn error(&self) -> Option<String> {
        self.read().0.error.clone()
    }

    /// Append streamed data and index the newly completed lines.
    pub fn append(&self, data: &[u8]) {
        let mut inner = self.write();
        match &mut inner.bytes {
            Bytes::Owned(v) => v.extend_from_slice(data),
            Bytes::Mapped(_) => unreachable!("append on a mapped file"),
        }
        inner.index_more();
    }

    /// End of stream: index a final unterminated line.
    pub fn finish(&self) {
        let mut inner = self.write();
        inner.complete = true;
        inner.index_more();
    }

    /// Drop all data (followed file was truncated) and bump the generation.
    pub fn reset(&self) {
        let mut inner = self.write();
        let generation = inner.generation + 1;
        *inner = Inner::new(Bytes::Owned(Vec::new()), false);
        inner.generation = generation;
    }

    fn set_error(&self, e: String) {
        self.write().error = Some(e);
    }
}

/// Where a background reader gets its data.
pub enum Stream {
    Stdin,
    /// Read a non-regular file (named pipe, `<(cmd)`) to its end.
    File(PathBuf),
    /// Read the file, then keep polling it for appended data (like `tail -f`).
    /// A file that shrinks is re-read from the start.
    Follow(PathBuf),
}

const READ_CHUNK: usize = 256 * 1024;
const FOLLOW_INTERVAL: Duration = Duration::from_millis(250);

/// Start a thread that fills `source` from `stream`. The thread ends at EOF
/// (stdin) or with the process (follow).
pub fn spawn_reader(source: Arc<Source>, stream: Stream) {
    thread::spawn(move || {
        let result = match stream {
            Stream::Stdin => read_to_end(&source, io::stdin().lock()).map(|()| source.finish()),
            Stream::File(path) => File::open(&path)
                .with_context(|| format!("cannot open {}", path.display()))
                .and_then(|f| read_to_end(&source, f))
                .map(|()| source.finish()),
            Stream::Follow(path) => follow(&source, &path),
        };
        if let Err(e) = result {
            source.set_error(format!("{e:#}"));
        }
    });
}

fn read_to_end(source: &Source, mut reader: impl Read) -> Result<()> {
    let mut buf = vec![0; READ_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => source.append(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e).context("read error"),
        }
    }
}

fn follow(source: &Source, path: &Path) -> Result<()> {
    let open = || File::open(path).with_context(|| format!("cannot open {}", path.display()));
    let mut file = open()?;
    let mut offset = 0u64;
    loop {
        let len = file.metadata()?.len();
        if len < offset {
            // Truncated (e.g. copytruncate log rotation): start over.
            source.reset();
            file = open()?;
            offset = 0;
        }
        if len > offset {
            file.seek(SeekFrom::Start(offset))?;
            let mut chunk = (&mut file).take(len - offset);
            read_to_end(source, &mut chunk)?;
            offset = len;
        }
        thread::sleep(FOLLOW_INTERVAL);
    }
}

fn trim_cr(data: &[u8], start: usize, end: usize) -> Range<usize> {
    if end > start && data[end - 1] == b'\r' {
        start..end - 1
    } else {
        start..end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn lines_of(data: &[u8]) -> Vec<Vec<u8>> {
        let src = Source::from_bytes(data.to_vec());
        let v = src.read();
        (0..v.len()).map(|i| v.raw(i).to_vec()).collect()
    }

    fn strs(lines: &[&str]) -> Vec<Vec<u8>> {
        lines.iter().map(|l| l.as_bytes().to_vec()).collect()
    }

    #[test]
    fn lf_and_trailing_newline() {
        assert_eq!(lines_of(b"a\nbb\n"), strs(&["a", "bb"]));
    }

    #[test]
    fn no_trailing_newline() {
        assert_eq!(lines_of(b"a\nbb"), strs(&["a", "bb"]));
    }

    #[test]
    fn crlf() {
        assert_eq!(lines_of(b"a\r\nbb\r\n"), strs(&["a", "bb"]));
    }

    #[test]
    fn empty_lines_are_kept() {
        assert_eq!(lines_of(b"a\n\n\nb\n"), strs(&["a", "", "", "b"]));
    }

    #[test]
    fn empty_input() {
        assert!(lines_of(b"").is_empty());
    }

    #[test]
    fn open_file_and_lossy_utf8() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"{\"a\":1}\r\n\xff\n").unwrap();
        let src = Source::open(f.path()).unwrap();
        let v = src.read();
        assert_eq!(v.len(), 2);
        assert_eq!(v.line(0), "{\"a\":1}");
        assert_eq!(v.line(1), "\u{FFFD}");
    }

    #[test]
    fn open_empty_file() {
        let f = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(Source::open(f.path()).unwrap().len(), 0);
    }

    #[test]
    fn streaming_indexes_complete_lines_until_finish() {
        let src = Source::streaming();
        src.append(b"{\"a\":1}\n{\"b\"");
        assert_eq!(src.len(), 1, "partial line not shown yet");
        src.append(b":2}\r\n{\"c\":");
        assert_eq!(src.len(), 2);
        assert_eq!(src.read().line(1), "{\"b\":2}");
        assert!(!src.is_complete());
        src.append(b"3}");
        src.finish();
        assert!(src.is_complete());
        assert_eq!(src.len(), 3);
        assert_eq!(src.read().line(2), "{\"c\":3}");
    }

    #[test]
    fn reset_clears_and_bumps_generation() {
        let src = Source::streaming();
        src.append(b"x\n");
        src.reset();
        assert_eq!(src.len(), 0);
        assert_eq!(src.generation(), 1);
    }

    fn wait_for(cond: impl Fn() -> bool) {
        for _ in 0..200 {
            if cond() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out");
    }

    #[test]
    fn follow_reads_appends_and_truncation() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"1\n2\n").unwrap();
        let src = Arc::new(Source::streaming());
        spawn_reader(Arc::clone(&src), Stream::Follow(f.path().to_path_buf()));
        wait_for(|| src.len() == 2);

        f.write_all(b"3\n").unwrap();
        wait_for(|| src.len() == 3);

        f.as_file().set_len(0).unwrap();
        f.as_file().seek(SeekFrom::Start(0)).unwrap();
        f.write_all(b"new\n").unwrap();
        wait_for(|| src.generation() == 1 && src.len() == 1);
        assert_eq!(src.read().line(0), "new");
    }
}
