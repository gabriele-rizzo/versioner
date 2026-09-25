use std::ops::Range;

/// Byte range of the contents of the string at `path`, a chain of object keys
/// starting from the top-level object. `None` when a key is missing or the
/// value there isn't a string.
pub(crate) fn string_at(source: &str, path: &[&str]) -> Option<Range<usize>> {
    let bytes = source.as_bytes();
    let (last, parents) = path.split_last()?;
    let mut cursor = skip_whitespace(bytes, 0);

    for key in parents {
        cursor = member(source, cursor, key)?;
    }

    let value = member(source, cursor, last)?;

    match bytes.get(value) {
        Some(b'"') => string_span(bytes, value).map(|(span, _)| span),
        _ => None,
    }
}

/// Start of the value stored under `key` in the object opening at `open`.
/// The first occurrence wins; callers re-parse to catch duplicate keys.
fn member(source: &str, open: usize, key: &str) -> Option<usize> {
    let bytes = source.as_bytes();

    if bytes.get(open) != Some(&b'{') {
        return None;
    }

    let mut cursor = skip_whitespace(bytes, open + 1);

    while bytes.get(cursor) == Some(&b'"') {
        let (_, after) = string_span(bytes, cursor)?;
        let name = serde_json::from_str::<String>(&source[cursor..after]).ok()?;

        let colon = skip_whitespace(bytes, after);

        if bytes.get(colon) != Some(&b':') {
            return None;
        }

        let value = skip_whitespace(bytes, colon + 1);

        if name == key {
            return Some(value);
        }

        cursor = skip_whitespace(bytes, skip_value(bytes, value)?);

        if bytes.get(cursor) == Some(&b',') {
            cursor = skip_whitespace(bytes, cursor + 1);
        }
    }

    None
}

/// Index just past the value starting at `start`.
fn skip_value(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'"' => string_span(bytes, start).map(|(_, after)| after),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut cursor = start;

            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'"' => {
                        cursor = string_span(bytes, cursor)?.1;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;

                        if depth == 0 {
                            return Some(cursor + 1);
                        }
                    }
                    _ => {}
                }

                cursor += 1;
            }

            None
        }
        _ => {
            let mut cursor = start;

            while !matches!(
                bytes.get(cursor),
                None | Some(b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            ) {
                cursor += 1;
            }

            Some(cursor)
        }
    }
}

/// Span of a string's contents plus the index just past its closing quote.
fn string_span(bytes: &[u8], quote: usize) -> Option<(Range<usize>, usize)> {
    let mut cursor = quote + 1;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'"' => return Some((quote + 1..cursor, cursor + 1)),
            _ => cursor += 1,
        }
    }

    None
}

fn skip_whitespace(bytes: &[u8], from: usize) -> usize {
    let mut cursor = from;

    while matches!(bytes.get(cursor), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        cursor += 1;
    }

    cursor
}

#[cfg(test)]
mod tests {
    use super::string_at;

    fn version_of(source: &str) -> Option<&str> {
        string_at(source, &["version"]).map(|span| &source[span])
    }

    #[test]
    fn finds_the_top_level_version() {
        assert_eq!(version_of(r#"{"version": "1.2.3"}"#), Some("1.2.3"));
        assert_eq!(
            version_of("{\n\t\"version\": \"1.2.3\"\n}\n"),
            Some("1.2.3")
        );
    }

    #[test]
    fn ignores_a_nested_version() {
        let source = r#"{"engines": {"version": "nested"}, "version": "1.2.3"}"#;

        assert_eq!(version_of(source), Some("1.2.3"));
    }

    #[test]
    fn ignores_version_used_as_a_value() {
        let source = r#"{"type": "version", "version": "1.2.3"}"#;

        assert_eq!(version_of(source), Some("1.2.3"));
    }

    #[test]
    fn ignores_version_inside_an_array() {
        let source = r#"{"keywords": ["version", {"version": "x"}], "version": "1.2.3"}"#;

        assert_eq!(version_of(source), Some("1.2.3"));
    }

    #[test]
    fn skips_numbers_booleans_and_null() {
        let source = r#"{"private": true, "n": -1.5e3, "x": null, "version": "1.2.3"}"#;

        assert_eq!(version_of(source), Some("1.2.3"));
    }

    #[test]
    fn tolerates_whitespace_around_the_colon() {
        assert_eq!(version_of("{\"version\"\n  :\t\"1.2.3\"}"), Some("1.2.3"));
    }

    #[test]
    fn skips_escaped_quotes_and_multibyte_text() {
        let source = r#"{"description": "héllo \"version\": ✓", "version": "1.2.3"}"#;

        assert_eq!(version_of(source), Some("1.2.3"));
    }

    #[test]
    fn spans_only_the_value() {
        let source = r#"{"version": "1.2.3"}"#;
        let span = string_at(source, &["version"]).unwrap();

        assert_eq!(&source[span.start - 1..span.end + 1], r#""1.2.3""#);
    }

    #[test]
    fn follows_a_path_of_keys() {
        let source = r#"{
            "version": "1.0.0",
            "packages": {
                "": {"name": "root", "version": "1.0.0"},
                "node_modules/backend": {"link": true},
                "backend": {"version": "2.0.0"}
            }
        }"#;

        let at = |path: &[&str]| string_at(source, path).map(|span| &source[span]);

        assert_eq!(at(&["packages", "", "version"]), Some("1.0.0"));
        assert_eq!(at(&["packages", "backend", "version"]), Some("2.0.0"));
        assert_eq!(at(&["packages", "node_modules/backend", "version"]), None);
        assert_eq!(at(&["packages", "missing", "version"]), None);
    }

    #[test]
    fn finds_nothing_without_a_top_level_version() {
        assert_eq!(version_of(r#"{"name": "demo"}"#), None);
        assert_eq!(version_of(r#"{"a": {"version": "1.2.3"}}"#), None);
        assert_eq!(version_of(r#"[]"#), None);
    }

    #[test]
    fn finds_nothing_when_the_version_is_not_a_string() {
        assert_eq!(version_of(r#"{"version": 3}"#), None);
        assert_eq!(version_of(r#"{"version": null}"#), None);
    }
}
