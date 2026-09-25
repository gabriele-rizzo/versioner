use std::ops::Range;

/// A `key = value` line, with the key spelled out in full from its table
/// header: `version` under `[package]` becomes `package.version`.
pub(crate) struct Entry {
    pub(crate) key: String,
    /// The `[[array]]` block the entry sits in, numbered through the file.
    pub(crate) block: Option<usize>,
    /// Raw value text.
    pub(crate) value: Range<usize>,
    /// Contents of the value when it's a single-line basic or literal string.
    pub(crate) string: Option<Range<usize>>,
}

/// Every top-level entry in document order. Values are skipped whole, so a
/// `[section]` inside a multi-line string or array is never taken for a header.
pub(crate) fn entries(source: &str) -> Vec<Entry> {
    let bytes = source.as_bytes();
    let mut entries = Vec::new();
    let mut table = String::new();
    let mut block = None;
    let mut blocks = 0;
    let mut cursor = 0;

    while cursor < bytes.len() {
        cursor = skip_blank(bytes, cursor);
        let end = line_end(bytes, cursor);

        match bytes.get(cursor) {
            None => break,
            Some(b'\n' | b'\r') => {
                cursor += 1;
                continue;
            }
            Some(b'#') => {
                cursor = end;
                continue;
            }
            Some(b'[') => {
                let array = bytes.get(cursor + 1) == Some(&b'[');
                let start = cursor + if array { 2 } else { 1 };

                if let Some(close) = source[start..end].find(']') {
                    table = normalize(&source[start..start + close]);
                    block = array.then(|| {
                        blocks += 1;
                        blocks
                    });
                }

                cursor = end;
                continue;
            }
            Some(_) => {}
        }

        let Some(equals) = find_equals(bytes, cursor, end) else {
            cursor = end;
            continue;
        };

        let key = normalize(&source[cursor..equals]);
        let value = skip_blank(bytes, equals + 1);
        let after = skip_value(bytes, value);

        entries.push(Entry {
            key: match table.is_empty() {
                true => key,
                false => format!("{table}.{key}"),
            },
            block,
            value: value..after,
            string: single_line_string(bytes, value, after),
        });

        cursor = line_end(bytes, after);
    }

    entries
}

/// Span of the string at a dotted `path` outside any `[[array]]` block.
pub(crate) fn string_at(source: &str, path: &[&str]) -> Option<Range<usize>> {
    let key = path.join(".");

    entries(source)
        .into_iter()
        .find(|entry| entry.block.is_none() && entry.key == key)?
        .string
}

/// One `[[package]]` block of a Cargo.lock or uv.lock.
pub(crate) struct Package {
    pub(crate) name: String,
    /// Raw `source` value; local packages have none (Cargo) or an
    /// `editable`/`virtual` one (uv).
    pub(crate) source: Option<String>,
    pub(crate) version: Option<Range<usize>>,
}

pub(crate) fn packages(source: &str) -> Vec<Package> {
    let mut packages: Vec<(usize, Package)> = Vec::new();

    for entry in entries(source) {
        let Some(block) = entry.block else { continue };

        let Some(field) = entry.key.strip_prefix("package.") else {
            continue;
        };

        if packages.last().is_none_or(|(last, _)| *last != block) {
            let empty = Package {
                name: String::new(),
                source: None,
                version: None,
            };

            packages.push((block, empty));
        }

        let package = &mut packages.last_mut().expect("just pushed").1;

        match field {
            "name" => {
                package.name = entry
                    .string
                    .map(|span| source[span].to_owned())
                    .unwrap_or_default()
            }
            "source" => package.source = Some(source[entry.value].to_owned()),
            "version" => package.version = entry.string,
            _ => {}
        }
    }

    packages.into_iter().map(|(_, package)| package).collect()
}

/// `a . "b.c"` → `a.b.c`. Quoted segments keep their dots; escapes are left as is.
fn normalize(text: &str) -> String {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;

    for character in text.chars() {
        match (quote, character) {
            (Some(open), character) if character == open => quote = None,
            (Some(_), character) => current.push(character),
            (None, '"' | '\'') => quote = Some(character),
            (None, '.') => parts.push(std::mem::take(&mut current)),
            (None, character) if character.is_whitespace() => {}
            (None, character) => current.push(character),
        }
    }

    parts.push(current);
    parts.join(".")
}

/// The `=` that ends a key, skipping over quoted key segments.
fn find_equals(bytes: &[u8], from: usize, end: usize) -> Option<usize> {
    let mut cursor = from;

    while cursor < end {
        match bytes[cursor] {
            b'=' => return Some(cursor),
            quote @ (b'"' | b'\'') => {
                cursor += 1;

                while cursor < end && bytes[cursor] != quote {
                    cursor += 1;
                }
            }
            _ => {}
        }

        cursor += 1;
    }

    None
}

/// Index just past the value starting at `start`, however many lines it spans.
fn skip_value(bytes: &[u8], start: usize) -> usize {
    let length = bytes.len();

    match bytes.get(start) {
        Some(&quote @ (b'"' | b'\'')) => {
            let triple = [quote; 3];
            let escapes = quote == b'"';

            if bytes[start..].starts_with(&triple) {
                let mut cursor = start + 3;

                while cursor < length {
                    if escapes && bytes[cursor] == b'\\' {
                        cursor += 2;
                        continue;
                    }

                    if bytes[cursor..].starts_with(&triple) {
                        // Up to two quotes may sit right before the closing three.
                        let mut end = cursor + 3;

                        while end < cursor + 5 && bytes.get(end) == Some(&quote) {
                            end += 1;
                        }

                        return end;
                    }

                    cursor += 1;
                }

                return length;
            }

            let mut cursor = start + 1;

            while cursor < length && bytes[cursor] != b'\n' {
                if escapes && bytes[cursor] == b'\\' {
                    cursor += 2;
                    continue;
                }

                if bytes[cursor] == quote {
                    return cursor + 1;
                }

                cursor += 1;
            }

            cursor.min(length)
        }
        Some(b'[' | b'{') => {
            let mut depth = 0usize;
            let mut cursor = start;

            while cursor < length {
                match bytes[cursor] {
                    b'"' | b'\'' => {
                        cursor = skip_value(bytes, cursor);
                        continue;
                    }
                    b'#' => {
                        cursor = line_end(bytes, cursor);
                        continue;
                    }
                    b'[' | b'{' => depth += 1,
                    b']' | b'}' => {
                        depth -= 1;

                        if depth == 0 {
                            return cursor + 1;
                        }
                    }
                    _ => {}
                }

                cursor += 1;
            }

            length
        }
        _ => {
            let mut cursor = start;

            while cursor < length && !matches!(bytes[cursor], b'\n' | b'\r' | b'#') {
                cursor += 1;
            }

            cursor
        }
    }
}

fn single_line_string(bytes: &[u8], start: usize, after: usize) -> Option<Range<usize>> {
    let &quote = bytes.get(start)?;

    let closed = matches!(quote, b'"' | b'\'')
        && after >= start + 2
        && bytes[after - 1] == quote
        && !bytes[start..].starts_with(&[quote; 3]);

    closed.then(|| start + 1..after - 1)
}

fn skip_blank(bytes: &[u8], from: usize) -> usize {
    let mut cursor = from;

    while matches!(bytes.get(cursor), Some(b' ' | b'\t')) {
        cursor += 1;
    }

    cursor
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from.min(bytes.len())..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(bytes.len(), |offset| from + offset)
}

#[cfg(test)]
mod tests {
    use super::{packages, string_at};

    fn at<'a>(source: &'a str, path: &[&str]) -> Option<&'a str> {
        string_at(source, path).map(|span| &source[span])
    }

    #[test]
    fn finds_a_key_under_its_table() {
        let source =
            "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n\n[dependencies]\nversion = \"no\"\n";

        assert_eq!(at(source, &["package", "version"]), Some("1.2.3"));
        assert_eq!(at(source, &["dependencies", "version"]), Some("no"));
    }

    #[test]
    fn reads_nested_headers_and_dotted_keys() {
        let source = "[workspace.package]\nversion = \"2.0.0\"\n";
        assert_eq!(
            at(source, &["workspace", "package", "version"]),
            Some("2.0.0")
        );

        let source = "package.version = '3.0.0'\n";
        assert_eq!(at(source, &["package", "version"]), Some("3.0.0"));

        let source = "[ tool . \"poetry\" ]\nversion='4.0.0' # trailing comment\n";
        assert_eq!(at(source, &["tool", "poetry", "version"]), Some("4.0.0"));
    }

    #[test]
    fn is_not_fooled_by_headers_inside_values() {
        let source = r#"
[project]
description = """
[tool.poetry]
version = "fake"
"""
matrix = [
  [1, 2],   # [tool.poetry]
  ["[x]", '''[y]'''],
]
version = "1.0.0"
"#;

        assert_eq!(at(source, &["project", "version"]), Some("1.0.0"));
        assert_eq!(at(source, &["tool", "poetry", "version"]), None);
    }

    #[test]
    fn skips_escaped_quotes() {
        let source = "[package]\ndescription = \"say \\\"version = 9\\\"\"\nversion = \"1.0.0\"\n";

        assert_eq!(at(source, &["package", "version"]), Some("1.0.0"));
    }

    #[test]
    fn only_reports_single_line_strings() {
        assert_eq!(at("version = 3\n", &["version"]), None);
        assert_eq!(at("version = \"\"\"1.0.0\"\"\"\n", &["version"]), None);
        assert_eq!(
            at(
                "[package]\nversion.workspace = true\n",
                &["package", "version"]
            ),
            None
        );
    }

    #[test]
    fn keeps_array_blocks_out_of_plain_lookups() {
        let source = "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n";

        assert_eq!(at(source, &["package", "version"]), None);
    }

    #[test]
    fn groups_lockfile_packages() {
        let source = r#"version = 4

[[package]]
name = "demo"
version = "0.1.0"
dependencies = [
 "serde",
]

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

        let found = packages(source);
        let versions: Vec<_> = found
            .iter()
            .map(|package| {
                (
                    package.name.as_str(),
                    &source[package.version.clone().unwrap()],
                )
            })
            .collect();

        assert_eq!(versions, [("demo", "0.1.0"), ("serde", "1.0.0")]);
        assert!(found[0].source.is_none());
        assert!(found[1].source.as_deref().unwrap().contains("registry"));
    }

    #[test]
    fn keeps_inline_table_sources_whole() {
        let source =
            "[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\nsource = { editable = \".\" }\n";

        assert_eq!(
            packages(source)[0].source.as_deref(),
            Some("{ editable = \".\" }")
        );
    }
}
