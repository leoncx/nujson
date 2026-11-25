//! jq-like filter backed by jaq. A line is kept when the expression yields at
//! least one value that is neither `false` nor `null`. Lines that are not
//! valid JSON, or on which the expression errors, are dropped.

use jaq_core::load::{Arena, File, Loader, lex, parse};
use jaq_core::{Ctx, ValT, Vars, compile, data, unwrap_valr};
use jaq_json::Val;

use super::Matcher;

type Data = data::JustLut<Val>;
type Filter = compile::Filter<jaq_core::Native<Data>>;

/// Upper bound on outputs inspected per line, so generators such as
/// `repeat(false)` cannot hang the filter.
const MAX_OUTPUTS: usize = 1024;

pub struct JqMatcher {
    filter: Filter,
}

impl JqMatcher {
    /// Parse and compile `code`. Errors are human-readable, one per line.
    pub fn new(code: &str) -> Result<Self, String> {
        let defs = jaq_core::defs()
            .chain(jaq_std::defs())
            .chain(jaq_json::defs());
        let funs = jaq_core::funs()
            .chain(jaq_std::funs())
            .chain(jaq_json::funs());

        let arena = Arena::default();
        let modules = Loader::new(defs)
            .load(&arena, File { code, path: () })
            .map_err(|errs| {
                let msgs: Vec<String> = errs
                    .into_iter()
                    .flat_map(|(_, e)| load_error_messages(code, e))
                    .collect();
                msgs.join("; ")
            })?;
        let filter = jaq_core::Compiler::default()
            .with_funs(funs)
            .compile(modules)
            .map_err(|errs| {
                let msgs: Vec<String> = errs
                    .into_iter()
                    .flat_map(|(_, es)| es)
                    .map(|(name, undef)| format!("undefined {} `{name}`", undefined_kind(&undef)))
                    .collect();
                msgs.join("; ")
            })?;
        Ok(Self { filter })
    }

    fn keep(&self, input: Val) -> bool {
        let ctx = Ctx::<Data>::new(&self.filter.lut, Vars::new([]));
        self.filter
            .id
            .run((ctx, input))
            .map(unwrap_valr)
            .take(MAX_OUTPUTS)
            .any(|out| matches!(out, Ok(v) if v.as_bool()))
    }
}

impl Matcher for JqMatcher {
    fn matches(&self, raw: &[u8]) -> bool {
        match jaq_json::read::parse_single(raw) {
            Ok(v) => self.keep(v),
            Err(_) => false,
        }
    }
}

fn undefined_kind(u: &compile::Undefined) -> String {
    match u {
        compile::Undefined::Filter(arity) => format!("filter (arity {arity})"),
        other => other.as_str().to_string(),
    }
}

fn load_error_messages(code: &str, e: jaq_core::load::Error<&str>) -> Vec<String> {
    use jaq_core::load::Error;
    match e {
        Error::Io(errs) => errs
            .into_iter()
            .map(|(p, msg)| format!("{p}: {msg}"))
            .collect(),
        Error::Lex(errs) => errs
            .into_iter()
            .map(|(expect, found): lex::Error<&str>| expected(code, expect.as_str(), found))
            .collect(),
        Error::Parse(errs) => errs
            .into_iter()
            .map(|(expect, found): parse::Error<&str>| expected(code, expect.as_str(), found))
            .collect(),
    }
}

/// "expected X at column N, found `...`"; `found` is a slice of `code`.
fn expected(code: &str, what: &str, found: &str) -> String {
    let offset = (found.as_ptr() as usize).wrapping_sub(code.as_ptr() as usize);
    let col = code
        .get(..offset)
        .map_or(0, |before| before.chars().count())
        + 1;
    let found: String = found.chars().take(12).collect();
    if found.is_empty() {
        format!("expected {what} at column {col}, found end of input")
    } else {
        format!("expected {what} at column {col}, found `{found}`")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keep(code: &str, line: &str) -> bool {
        JqMatcher::new(code).unwrap().matches(line.as_bytes())
    }

    #[test]
    fn boolean_expressions() {
        assert!(keep(r#".level == "error""#, r#"{"level":"error"}"#));
        assert!(!keep(r#".level == "error""#, r#"{"level":"info"}"#));
        assert!(keep(".code > 400", r#"{"code":503}"#));
    }

    #[test]
    fn select_and_truthiness() {
        assert!(keep("select(.ok)", r#"{"ok":true}"#));
        assert!(!keep("select(.ok)", r#"{"ok":false}"#));
        assert!(keep(".user", r#"{"user":{"id":1}}"#));
        assert!(!keep(".user", r#"{"other":1}"#), "null is falsy");
        assert!(keep(".[] | . > 2", "[1,2,3]"), "any output truthy");
    }

    #[test]
    fn std_functions_available() {
        assert!(keep(
            r#".msg | test("^conn")"#,
            r#"{"msg":"connection refused"}"#
        ));
        assert!(keep(r#"has("a") and (keys | length) == 1"#, r#"{"a":1}"#));
    }

    #[test]
    fn invalid_json_and_runtime_errors_are_dropped() {
        assert!(!keep(".a", "not json"));
        assert!(!keep(".a", "[1]"), "indexing array with string errors");
    }

    #[test]
    fn infinite_generators_are_bounded() {
        assert!(!keep("repeat(false)", "{}"));
    }

    #[test]
    fn compile_errors_are_readable() {
        let err = JqMatcher::new(".a ==").err().unwrap();
        assert!(err.starts_with("expected term at column 6"), "{err}");
        let err = JqMatcher::new("nosuchfn(1)").err().unwrap();
        assert!(
            err.contains("undefined filter") && err.contains("nosuchfn"),
            "{err}"
        );
        let err = JqMatcher::new(r#"{"a": 1"#).err().unwrap();
        assert!(err.contains("expected"), "{err}");
    }
}
