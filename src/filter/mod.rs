//! Line filters. A filter decides, from a line's raw bytes, whether it is kept.

pub mod text;

use crate::source::Source;

pub trait Matcher {
    fn matches(&self, raw: &[u8]) -> bool;
}

/// Indices of all lines of `source` accepted by `matcher`.
pub fn run(source: &Source, matcher: &dyn Matcher) -> Vec<usize> {
    (0..source.len())
        .filter(|&i| matcher.matches(source.raw(i)))
        .collect()
}
