//! Finds the exact bytes of a value inside a manifest or lockfile, so an edit
//! can rewrite that value and leave the rest of the file byte-for-byte intact.
//! The scanners are deliberately small, so nothing trusts them blindly: spans
//! are checked against a real parse before and after every rewrite.

pub(crate) mod json;
pub(crate) mod toml;

use std::ops::Range;

/// The string at a key `path` in a parsed JSON document.
pub(crate) fn json_string<'a>(document: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(document, |value, key| value.get(key))?
        .as_str()
}

/// The string at a key `path` in a parsed TOML document.
pub(crate) fn toml_string<'a>(document: &'a ::toml::Table, path: &[&str]) -> Option<&'a str> {
    let (last, parents) = path.split_last()?;

    let table = parents
        .iter()
        .try_fold(document, |table, key| table.get(*key)?.as_table())?;

    table.get(*last)?.as_str()
}

/// `source` with every span replaced by `value`. Spans must not overlap.
pub(crate) fn replace(source: &str, spans: &[Range<usize>], value: &str) -> String {
    let mut spans = spans.to_vec();
    spans.sort_by_key(|span| span.start);

    let mut result = String::with_capacity(source.len());
    let mut cursor = 0;

    for span in spans {
        result.push_str(&source[cursor..span.start]);
        result.push_str(value);
        cursor = span.end;
    }

    result.push_str(&source[cursor..]);
    result
}

#[cfg(test)]
mod tests {
    use super::{json_string, replace, toml_string};
    use serde_json::json;

    #[test]
    fn replaces_spans_in_any_order() {
        let source = "a=1 b=1 c=1";

        assert_eq!(replace(source, &[10..11, 2..3], "22"), "a=22 b=1 c=22");
        assert_eq!(replace(source, &[], "x"), source);
    }

    #[test]
    fn looks_up_strings_by_path() {
        let document = json!({"packages": {"": {"version": "1.0.0"}}, "n": 1});

        assert_eq!(
            json_string(&document, &["packages", "", "version"]),
            Some("1.0.0")
        );
        assert_eq!(json_string(&document, &["n"]), None);

        let document: ::toml::Table = "[tool.poetry]\nversion = \"2.0.0\"\n".parse().unwrap();

        assert_eq!(
            toml_string(&document, &["tool", "poetry", "version"]),
            Some("2.0.0")
        );
        assert_eq!(toml_string(&document, &["tool", "version"]), None);
    }
}
