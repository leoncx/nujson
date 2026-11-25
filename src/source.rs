//! Memory-mapped NDJSON source with a line index.
//!
//! The file is mapped once, then scanned for `\n` to record where each line
//! starts and ends. Lines are only decoded on demand, so opening a large file
//! costs one linear scan and ~16 bytes of memory per line.

use std::borrow::Cow;
use std::fs::File;
use std::ops::{Deref, Range};
use std::path::Path;

use anyhow::{Context, Result};
use memmap2::Mmap;

enum Bytes {
    Mapped(Mmap),
    /// Used for empty files: mapping a zero-length file fails on some platforms.
    Empty,
}

impl Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Mapped(m) => m,
            Bytes::Empty => &[],
        }
    }
}

pub struct Source {
    bytes: Bytes,
    /// Byte range of each line, excluding the `\n` and any trailing `\r`.
    lines: Vec<Range<usize>>,
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        let len = file.metadata()?.len();
        let bytes = if len == 0 {
            Bytes::Empty
        } else {
            // SAFETY: the map is read-only. If another process truncates the
            // file while we run, reads may fault; this is the accepted
            // trade-off of mmap-based viewers (less, bat, ripgrep do the same).
            Bytes::Mapped(
                unsafe { Mmap::map(&file) }
                    .with_context(|| format!("cannot map {}", path.display()))?,
            )
        };
        let lines = index_lines(&bytes);
        Ok(Self { bytes, lines })
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Raw bytes of line `i` (0-based), without line terminator.
    pub fn raw(&self, i: usize) -> &[u8] {
        &self.bytes[self.lines[i].clone()]
    }

    /// Line `i` as text; invalid UTF-8 is replaced with U+FFFD.
    pub fn line(&self, i: usize) -> Cow<'_, str> {
        String::from_utf8_lossy(self.raw(i))
    }
}

fn index_lines(data: &[u8]) -> Vec<Range<usize>> {
    let mut lines = Vec::with_capacity(data.len() / 128 + 1);
    let mut start = 0;
    for nl in memchr::memchr_iter(b'\n', data) {
        lines.push(trim_cr(data, start, nl));
        start = nl + 1;
    }
    // Last line without trailing newline.
    if start < data.len() {
        lines.push(trim_cr(data, start, data.len()));
    }
    lines
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

    fn lines_of(data: &[u8]) -> Vec<&[u8]> {
        index_lines(data).into_iter().map(|r| &data[r]).collect()
    }

    #[test]
    fn lf_and_trailing_newline() {
        assert_eq!(lines_of(b"a\nbb\n"), vec![&b"a"[..], b"bb"]);
    }

    #[test]
    fn no_trailing_newline() {
        assert_eq!(lines_of(b"a\nbb"), vec![&b"a"[..], b"bb"]);
    }

    #[test]
    fn crlf() {
        assert_eq!(lines_of(b"a\r\nbb\r\n"), vec![&b"a"[..], b"bb"]);
    }

    #[test]
    fn empty_lines_are_kept() {
        assert_eq!(lines_of(b"a\n\n\nb\n"), vec![&b"a"[..], b"", b"", b"b"]);
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
        assert_eq!(src.len(), 2);
        assert_eq!(src.line(0), "{\"a\":1}");
        assert_eq!(src.line(1), "\u{FFFD}");
    }

    #[test]
    fn open_empty_file() {
        let f = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(Source::open(f.path()).unwrap().len(), 0);
    }
}
