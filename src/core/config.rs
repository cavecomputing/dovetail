//! Config files: YAML (.yaml, .yml), TOML (.toml) and INI (.ini, .cfg, .conf). Which parts of a
//! line to colour, and whether the text is valid. Lines are coloured one at a time; YAML's block
//! scalars (`key: |`) and TOML's multi-line strings (`"""`) run on below the line that opens
//! them, so each line also says what the next one starts in.

use std::ops::Range;

use crate::core::syntax::{Token, line_and_column};

type Found = Vec<(Range<usize>, Token)>;

/// Where a quoted string that starts at `start` ends (just after its closing quote, or at the end
/// of the line if it isn't closed). `"` strings escape with a backslash; in `'` strings two
/// quotes are one.
fn quoted_end(line: &str, start: usize) -> usize {
    let bytes = line.as_bytes();
    let quote = bytes[start];
    let mut at = start + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' if quote == b'"' => at += 2,
            b'\'' if quote == b'\'' && bytes.get(at + 1) == Some(&b'\'') => at += 2,
            byte if byte == quote => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// Where a `#` comment starts in `line`, outside any quotes. In YAML (`after_space`) a `#` only
/// starts one at the start of the line or after a space, so `a#b` is plain text.
fn comment_start(line: &str, after_space: bool) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'"' | b'\'' => at = quoted_end(line, at),
            b'#' if !after_space || at == 0 || bytes[at - 1].is_ascii_whitespace() => {
                return Some(at);
            }
            _ => at += 1,
        }
    }
    None
}

/// How an unquoted value reads: a number, a literal (`true`, `null`, `yes`, ...) or a string.
fn scalar(word: &str) -> Token {
    let lower = word.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off"
    ) {
        return Token::Literal;
    }
    let digits = word.trim_start_matches(['+', '-']);
    let numeric = matches!(lower.trim_start_matches(['+', '-']), "inf" | ".inf" | "nan" | ".nan")
        || word.replace('_', "").parse::<f64>().is_ok()
        || (digits.starts_with("0x") || digits.starts_with("0o") || digits.starts_with("0b"))
            && digits.len() > 2
        // Dates and times, as TOML and YAML write them: 1979-05-27, 07:32:00.
        || digits.starts_with(|c: char| c.is_ascii_digit())
            && digits.chars().all(|c| c.is_ascii_alphanumeric() || "-:.+_ ".contains(c));
    match numeric {
        true => Token::Number,
        false => Token::String,
    }
}

/// `range` of `line` with the spaces at its ends left out.
fn trimmed(line: &str, range: Range<usize>) -> Range<usize> {
    let part = &line[range.clone()];
    let start = range.start + (part.len() - part.trim_start().len());
    let end = range.end - (part.len() - part.trim_end().len());
    start..end.max(start)
}

/// The spaces at the start of `line`.
fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Colours one YAML line. `block` is the indent of the line that opened a block scalar the line
/// is in (`key: |`), if it is in one: lines more indented than that are its text.
pub fn yaml_ranges(line: &str, block: Option<usize>) -> (Found, Option<usize>) {
    let mut found = Vec::new();
    let line_indent = indent(line);
    if let Some(opener) = block {
        if line.trim().is_empty() {
            return (found, block);
        }
        if line_indent > opener {
            found.push((line_indent..line.len(), Token::String));
            return (found, block);
        }
    }
    let comment = comment_start(line, true);
    let end = comment.unwrap_or(line.len());
    let mut at = line_indent;
    let mut next = None;
    if at == 0 && (line.starts_with("---") || line.starts_with("...")) {
        found.push((0..3, Token::Marker));
        at = 3;
    }
    // A list item's `-`, maybe several (`- - a`).
    loop {
        at += line[at..end].len() - line[at..end].trim_start().len();
        let rest = &line[at..end];
        if rest == "-" || rest.starts_with("- ") {
            found.push((at..at + 1, Token::Marker));
            at += 1;
        } else {
            break;
        }
    }
    // A key: a quoted or plain scalar followed by `:` and a space (or the end).
    let rest = &line[at..end];
    let key_end = match rest.as_bytes().first() {
        Some(b'"' | b'\'') => {
            let close = quoted_end(line, at);
            line[close..end]
                .trim_start()
                .starts_with(':')
                .then_some(close)
        }
        Some(b'{' | b'[') | None => None,
        Some(_) => rest
            .match_indices(':')
            .map(|(colon, _)| at + colon)
            .find(|&colon| colon + 1 == end || line.as_bytes()[colon + 1] == b' '),
    };
    if let Some(key_end) = key_end {
        found.push((trimmed(line, at..key_end), Token::Key));
        at = key_end + line[key_end..end].find(':').map_or(0, |colon| colon + 1);
    }
    // The value: anchors, aliases and tags first, then a block scalar's `|` or `>`, a quoted
    // string, a flow collection, or a plain scalar.
    loop {
        at += line[at..end].len() - line[at..end].trim_start().len();
        let rest = &line[at..end];
        if !rest.starts_with(['&', '*', '!']) {
            break;
        }
        let length = rest.find(' ').unwrap_or(rest.len());
        found.push((at..at + length, Token::Marker));
        at += length;
    }
    let value = trimmed(line, at..end);
    match line[value.clone()].as_bytes().first() {
        None => {}
        Some(b'|' | b'>') => {
            found.push((value, Token::Marker));
            next = Some(line_indent);
        }
        Some(b'"' | b'\'') => {
            found.push((value.start..quoted_end(line, value.start), Token::String))
        }
        Some(b'{' | b'[') => flow(line, value, &mut found),
        Some(_) => found.push((value.clone(), scalar(&line[value]))),
    }
    if let Some(comment) = comment {
        found.push((comment..line.len(), Token::Comment));
    }
    (found, next)
}

/// Colours a YAML flow collection (`{a: 1, b: [x, "y"]}`) in `range`: keys before a `:`, quoted
/// strings, and scalars between the brackets and commas.
fn flow(line: &str, range: Range<usize>, found: &mut Found) {
    let bytes = line.as_bytes();
    let mut at = range.start;
    while at < range.end {
        match bytes[at] {
            b'"' | b'\'' => {
                let close = quoted_end(line, at).min(range.end);
                let key = line[close..range.end].trim_start().starts_with(':');
                found.push((at..close, if key { Token::Key } else { Token::String }));
                at = close;
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' | b' ' => at += 1,
            _ => {
                let length = line[at..range.end]
                    .find([',', ']', '}', ':'])
                    .unwrap_or(range.end - at);
                let word = trimmed(line, at..at + length);
                let key = bytes.get(at + length) == Some(&b':');
                let token = if key {
                    Token::Key
                } else {
                    scalar(&line[word.clone()])
                };
                found.push((word, token));
                at += length.max(1);
            }
        }
    }
}

/// Colours one TOML line. `open` is the closing quotes (`"""` or `'''`) of a multi-line string
/// the line starts inside, if it does.
pub fn toml_ranges(line: &str, open: Option<&'static str>) -> (Found, Option<&'static str>) {
    let mut found = Vec::new();
    let mut at = 0;
    if let Some(close) = open {
        match line.find(close) {
            Some(end) => {
                found.push((0..end + close.len(), Token::String));
                at = end + close.len();
            }
            None => {
                found.push((0..line.len(), Token::String));
                return (found, open);
            }
        }
    }
    // A `[table]` or `[[array of tables]]` header.
    let start = at + (line[at..].len() - line[at..].trim_start().len());
    if open.is_none() && line[start..].starts_with('[') {
        let end = comment_start(line, false).unwrap_or(line.len());
        found.push((trimmed(line, start..end), Token::Section));
        at = end;
    }
    let bytes = line.as_bytes();
    while at < bytes.len() {
        match bytes[at] {
            b'#' => {
                found.push((at..line.len(), Token::Comment));
                break;
            }
            b'"' | b'\'' => {
                let quotes = if bytes[at] == b'"' { "\"\"\"" } else { "'''" };
                if line[at..].starts_with(quotes) {
                    // A multi-line string: it ends on this line, or runs on below.
                    match line[at + 3..].find(quotes) {
                        Some(end) => {
                            let close = at + 3 + end + 3;
                            found.push((at..close, Token::String));
                            at = close;
                        }
                        None => {
                            found.push((at..line.len(), Token::String));
                            return (found, Some(quotes));
                        }
                    }
                    continue;
                }
                let close = quoted_end(line, at);
                let key = line[close..].trim_start().starts_with('=');
                found.push((at..close, if key { Token::Key } else { Token::String }));
                at = close;
            }
            byte if byte.is_ascii_alphanumeric() || b"+-_".contains(&byte) => {
                let length = line[at..]
                    .find(|c: char| !(c.is_ascii_alphanumeric() || "+-_.:".contains(c)))
                    .unwrap_or(line.len() - at);
                let word = &line[at..at + length];
                let key = line[at + length..].trim_start().starts_with('=');
                let token = match key {
                    true => Token::Key,
                    false => match scalar(word) {
                        // TOML has no bare strings: only true and false are words.
                        Token::Literal if word != "true" && word != "false" => Token::String,
                        token => token,
                    },
                };
                // A dotted key's parts, or a word TOML doesn't know, isn't coloured as a string.
                if token != Token::String {
                    found.push((at..at + length, token));
                }
                at += length;
            }
            _ => at += 1,
        }
    }
    (found, None)
}

/// Colours one INI line: `; comment` or `# comment`, `[section]`, or `key = value` (or
/// `key: value`).
pub fn ini_ranges(line: &str) -> Found {
    let start = indent(line);
    let rest = line[start..].trim_end();
    let end = start + rest.len();
    if rest.starts_with([';', '#']) {
        return vec![(start..end, Token::Comment)];
    }
    if rest.starts_with('[') {
        return vec![(start..end, Token::Section)];
    }
    let Some(separator) = line[start..end].find(['=', ':']).map(|at| start + at) else {
        return Vec::new();
    };
    let mut found = vec![(trimmed(line, start..separator), Token::Key)];
    let value = trimmed(line, separator + 1..end);
    if !value.is_empty() {
        found.push((value.clone(), scalar(&line[value])));
    }
    found
}

/// Why `text` isn't valid YAML, with where, or `None` if it is.
pub fn yaml_problem(text: &str) -> Option<String> {
    let error = saphyr_parser::Parser::new_from_str(text).find_map(Result::err)?;
    let marker = error.marker();
    Some(format!(
        "Invalid YAML at line {}, column {}: {}",
        marker.line(),
        marker.col(),
        error.info()
    ))
}

/// Why `text` isn't valid TOML, with where, or `None` if it is.
pub fn toml_problem(text: &str) -> Option<String> {
    let error = text.parse::<toml::Table>().err()?;
    let (line, column) = line_and_column(text, error.span().map_or(0, |span| span.start));
    let reason = error.message().trim_end_matches('\n');
    Some(format!(
        "Invalid TOML at line {line}, column {column}: {reason}"
    ))
}

/// Why `text` isn't a valid INI file, or `None` if it is. INI has no one standard, so this only
/// asks that every line is a `[section]`, a `key = value` (or `key: value`), a comment, blank, or
/// indented to carry on the value above it.
pub fn ini_problem(text: &str) -> Option<String> {
    let mut after_value = false;
    for (index, line) in text.lines().enumerate() {
        let rest = line.trim();
        // An indented line under a value carries that value on, as Python's configparser reads it.
        let carries_on = indent(line) > 0 && after_value;
        let problem = if rest.is_empty() || rest.starts_with([';', '#']) || carries_on {
            None
        } else if rest.starts_with('[') {
            (!rest.ends_with(']')).then_some("a [section] needs its closing ]")
        } else if rest.find(['=', ':']).is_some_and(|at| at > 0) {
            None
        } else {
            Some("expected key = value, a [section] or a ; comment")
        };
        if let Some(reason) = problem {
            return Some(format!(
                "Invalid INI at line {}, column {}: {reason}",
                index + 1,
                indent(line) + 1
            ));
        }
        after_value = !rest.is_empty() && !rest.starts_with([';', '#', '[']);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use Token::*;

    fn looks(line: &str, found: Found) -> Vec<(&str, Token)> {
        found
            .into_iter()
            .map(|(range, look)| (&line[range], look))
            .collect()
    }

    fn yaml(line: &str) -> Vec<(&str, Token)> {
        looks(line, yaml_ranges(line, None).0)
    }

    #[test]
    fn yaml_keys_values_lists_and_comments() {
        assert_eq!(
            yaml("name: Nano Bear # who"),
            vec![("name", Key), ("Nano Bear", String), ("# who", Comment)]
        );
        assert_eq!(
            yaml("  - port: 8080"),
            vec![("-", Marker), ("port", Key), ("8080", Number)]
        );
        assert_eq!(yaml("- yes"), vec![("-", Marker), ("yes", Literal)]);
        assert_eq!(
            yaml("\"quoted key\": 'it''s'"),
            vec![("\"quoted key\"", Key), ("'it''s'", String)]
        );
        assert_eq!(
            yaml("url: http://a.b/c#d"),
            vec![("url", Key), ("http://a.b/c#d", String)]
        );
        assert_eq!(yaml("base: &base"), vec![("base", Key), ("&base", Marker)]);
        assert_eq!(yaml("---"), vec![("---", Marker)]);
        assert_eq!(
            yaml("tags: [cozy, \"bear\", 3]"),
            vec![
                ("tags", Key),
                ("cozy", String),
                ("\"bear\"", String),
                ("3", Number)
            ]
        );
        assert_eq!(
            yaml("on: {a: 1}"),
            vec![("on", Key), ("a", Key), ("1", Number)]
        );
    }

    #[test]
    fn a_yaml_block_scalar_colours_the_lines_under_it_as_text() {
        let (found, block) = yaml_ranges("prompt: |", None);
        assert_eq!(
            looks("prompt: |", found),
            vec![("prompt", Key), ("|", Marker)]
        );
        assert_eq!(block, Some(0));
        // Indented lines are its text, even one that looks like a key; a blank line stays in it.
        let (found, block) = yaml_ranges("  Note: be kind", block);
        assert_eq!(
            looks("  Note: be kind", found),
            vec![("Note: be kind", String)]
        );
        let (_, block) = yaml_ranges("", block);
        assert_eq!(block, Some(0));
        // A line back at the key's indent ends it.
        let (found, block) = yaml_ranges("next: 1", block);
        assert_eq!(looks("next: 1", found), vec![("next", Key), ("1", Number)]);
        assert_eq!(block, None);
    }

    fn toml(line: &str) -> Vec<(&str, Token)> {
        looks(line, toml_ranges(line, None).0)
    }

    #[test]
    fn toml_tables_keys_values_and_comments() {
        assert_eq!(
            toml("[package] # the crate"),
            vec![("[package]", Section), ("# the crate", Comment)]
        );
        assert_eq!(toml("[[bin]]"), vec![("[[bin]]", Section)]);
        assert_eq!(
            toml("name = \"dovetail\""),
            vec![("name", Key), ("\"dovetail\"", String)]
        );
        assert_eq!(
            toml("a.b = 1_000 # n"),
            vec![("a.b", Key), ("1_000", Number), ("# n", Comment)]
        );
        assert_eq!(toml("ok = false"), vec![("ok", Key), ("false", Literal)]);
        assert_eq!(
            toml("when = 1979-05-27T07:32:00Z"),
            vec![("when", Key), ("1979-05-27T07:32:00Z", Number)]
        );
        assert_eq!(
            toml("dep = { version = \"1\", features = [\"x\"] }"),
            vec![
                ("dep", Key),
                ("version", Key),
                ("\"1\"", String),
                ("features", Key),
                ("\"x\"", String)
            ]
        );
        assert_eq!(
            toml("\"quoted.key\" = 'path\\here'"),
            vec![("\"quoted.key\"", Key), ("'path\\here'", String)]
        );
    }

    #[test]
    fn a_toml_multi_line_string_runs_on_to_its_closing_quotes() {
        let (found, open) = toml_ranges("text = \"\"\"first", None);
        assert_eq!(
            looks("text = \"\"\"first", found),
            vec![("text", Key), ("\"\"\"first", String)]
        );
        assert_eq!(open, Some("\"\"\""));
        let (found, open) = toml_ranges("a = 1 # not a key", open);
        assert_eq!(
            looks("a = 1 # not a key", found),
            vec![("a = 1 # not a key", String)]
        );
        let (found, open) = toml_ranges("last\"\"\" # done", open);
        assert_eq!(
            looks("last\"\"\" # done", found),
            vec![("last\"\"\"", String), ("# done", Comment)]
        );
        assert_eq!(open, None);
    }

    #[test]
    fn ini_sections_keys_values_and_comments() {
        let ini = |line| looks(line, ini_ranges(line));
        assert_eq!(ini("[server]"), vec![("[server]", Section)]);
        assert_eq!(ini("; note"), vec![("; note", Comment)]);
        assert_eq!(ini("port = 8080"), vec![("port", Key), ("8080", Number)]);
        assert_eq!(ini("debug: on"), vec![("debug", Key), ("on", Literal)]);
        assert_eq!(
            ini("path=/var/www"),
            vec![("path", Key), ("/var/www", String)]
        );
        assert_eq!(ini("empty ="), vec![("empty", Key)]);
    }

    #[test]
    fn problems_say_where_and_why() {
        assert_eq!(yaml_problem("a: 1\nb:\n  - x\n"), None);
        assert_eq!(yaml_problem(""), None);
        let problem = yaml_problem("a: 1\n b: 2\n").unwrap();
        assert!(
            problem.starts_with("Invalid YAML at line 2, column "),
            "{problem}"
        );
        assert_eq!(toml_problem("[a]\nb = 1\n"), None);
        assert!(
            toml_problem("[a]\nb = \n")
                .unwrap()
                .starts_with("Invalid TOML at line 2, column ")
        );
        assert_eq!(ini_problem("; c\n[s]\nk = v\n  more of v\n\nx: y\n"), None);
        assert_eq!(
            ini_problem("[s]\nk = v\njust words\n"),
            Some(
                "Invalid INI at line 3, column 1: expected key = value, a [section] or a ; comment"
                    .to_string()
            )
        );
        assert_eq!(
            ini_problem("[s\n"),
            Some("Invalid INI at line 1, column 1: a [section] needs its closing ]".to_string())
        );
    }
}
