//! JSON files (.json): which parts of a line to colour, whether the text is valid JSON, and the
//! text laid out again (Format) or squeezed onto one line (Minify).

use std::ops::Range;

use crate::core::syntax::{Token, quoted_end};

/// Finds the keys, strings, numbers and words in one line, as byte ranges. A JSON string can't
/// hold a line break, so each line can be coloured on its own; one left open runs to the end.
pub fn json_ranges(line: &str) -> Vec<(Range<usize>, Token)> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let look = match bytes[at] {
            b'"' => {
                at = quoted_end(line, at);
                match line[at..].trim_start().starts_with(':') {
                    true => Token::Key,
                    false => Token::String,
                }
            }
            b'-' | b'0'..=b'9' => {
                while at < bytes.len()
                    && matches!(bytes[at], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    at += 1;
                }
                Token::Number
            }
            b'a'..=b'z' => {
                while at < bytes.len() && bytes[at].is_ascii_lowercase() {
                    at += 1;
                }
                if !matches!(&line[start..at], "true" | "false" | "null") {
                    continue;
                }
                Token::Literal
            }
            _ => {
                at += 1;
                continue;
            }
        };
        found.push((start..at, look));
    }
    found
}

/// Why `text` isn't valid JSON, with where, or `None` if it is (or holds nothing yet).
pub fn json_problem(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    // `IgnoredAny` checks the text without building anything from it, so a big file is quick.
    let error = serde_json::from_str::<serde::de::IgnoredAny>(text).err()?;
    // serde_json ends its message with " at line 3 column 5"; say where first instead.
    let message = error.to_string();
    let reason = message.split(" at line ").next().unwrap_or(&message);
    Some(format!(
        "Invalid JSON at line {}, column {}: {reason}",
        error.line(),
        error.column()
    ))
}

/// `text` laid out again: each value on its own line, indented `indent` spaces a level, or with
/// `indent` as `None` all on one line with no spaces (Minify). Only the spaces and line breaks
/// between values change, so keys keep their order and numbers their exact digits. Text that
/// isn't valid JSON is refused, with why.
pub fn reformat(text: &str, indent: Option<usize>) -> Result<String, String> {
    if let Some(problem) = json_problem(text) {
        return Err(problem);
    }
    if text.trim().is_empty() {
        return Ok(text.to_string());
    }
    let new_line = |formatted: &mut String, depth: usize| {
        if let Some(width) = indent {
            formatted.push('\n');
            formatted.push_str(&" ".repeat(width * depth));
        }
    };
    let mut formatted = String::with_capacity(text.len());
    let mut depth = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                formatted.push(c);
                while let Some(c) = chars.next() {
                    formatted.push(c);
                    match c {
                        '\\' => formatted.extend(chars.next()),
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '{' | '[' => {
                formatted.push(c);
                while chars.next_if(|c| c.is_whitespace()).is_some() {}
                // An empty object or list stays as `{}` or `[]`.
                match chars.next_if(|next| matches!(next, '}' | ']')) {
                    Some(close) => formatted.push(close),
                    None => {
                        depth += 1;
                        new_line(&mut formatted, depth);
                    }
                }
            }
            '}' | ']' => {
                depth -= 1;
                new_line(&mut formatted, depth);
                formatted.push(c);
            }
            ',' => {
                formatted.push(c);
                new_line(&mut formatted, depth);
            }
            ':' => formatted.push_str(if indent.is_some() { ": " } else { ":" }),
            c if c.is_whitespace() => {}
            c => formatted.push(c),
        }
    }
    // A formatted file ends with a line break, as editors and tools expect.
    if indent.is_some() {
        formatted.push('\n');
    }
    Ok(formatted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looks(line: &str) -> Vec<(&str, Token)> {
        json_ranges(line)
            .into_iter()
            .map(|(range, look)| (&line[range], look))
            .collect()
    }

    #[test]
    fn keys_strings_numbers_and_words_are_found() {
        assert_eq!(
            looks(r#"  "name": "Nano \"Bear\"", "age": -1.5e3, "ok": true, "x": null,"#),
            vec![
                (r#""name""#, Token::Key),
                (r#""Nano \"Bear\"""#, Token::String),
                (r#""age""#, Token::Key),
                ("-1.5e3", Token::Number),
                (r#""ok""#, Token::Key),
                ("true", Token::Literal),
                (r#""x""#, Token::Key),
                ("null", Token::Literal),
            ]
        );
        // A string left open runs to the end of the line; a stray word isn't coloured.
        assert_eq!(looks(r#"[nope, "open"#), vec![(r#""open"#, Token::String)]);
        assert_eq!(
            looks(r#""ends in \"#),
            vec![(r#""ends in \"#, Token::String)]
        );
    }

    #[test]
    fn a_problem_says_where_and_why() {
        assert_eq!(json_problem(r#"{"a": [1, 2]}"#), None);
        assert_eq!(
            json_problem("  \n"),
            None,
            "a new, empty file isn't a problem yet"
        );
        assert_eq!(
            json_problem("{\n  \"a\": 1\n  \"b\": 2\n}"),
            Some("Invalid JSON at line 3, column 3: expected `,` or `}`".to_string())
        );
    }

    #[test]
    fn format_lays_out_each_value_on_its_own_line() {
        let text =
            r#"{"name":"a, b: {c}","tags":[],"nested":{"n":1.50,"list":[true,null]},"e":{}}"#;
        assert_eq!(
            reformat(text, Some(2)).unwrap(),
            "{\n  \"name\": \"a, b: {c}\",\n  \"tags\": [],\n  \"nested\": {\n    \"n\": 1.50,\n    \
             \"list\": [\n      true,\n      null\n    ]\n  },\n  \"e\": {}\n}\n"
        );
    }

    #[test]
    fn minify_undoes_format_and_keeps_strings_whole() {
        let text = "{\n  \"say\": \"two  spaces \\\" and \\\\\",\n  \"list\": [ 1 , 2 ]\n}\n";
        let minified = reformat(text, None).unwrap();
        assert_eq!(minified, r#"{"say":"two  spaces \" and \\","list":[1,2]}"#);
        assert_eq!(
            reformat(&minified, Some(2)).unwrap(),
            text.replace("[ 1 , 2 ]", "[\n    1,\n    2\n  ]")
        );
    }

    #[test]
    fn invalid_json_is_not_reformatted() {
        assert_eq!(
            reformat("[1,", Some(4)),
            Err("Invalid JSON at line 1, column 3: EOF while parsing a value".to_string())
        );
    }
}
