//! XML tags and {{macros}} in a prompt: where they are, for colouring them. Tags and macros are
//! found one line at a time.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Markup {
    Tag,
    Macro,
}

/// Finds `<tags ...>` and `{{macros}}` within one line, as byte ranges.
pub fn markup_ranges(line: &str) -> Vec<(Range<usize>, Markup)> {
    let mut found = Vec::new();
    let mut from = 0;
    // Where the next tag and the next macro start, at or after `from`. Each is looked for again
    // only once `from` has passed it, and never once the line has no more that close: looking
    // through the rest of the line after every one found took seconds on a long line of tags.
    let mut tag = tag_start(line, 0);
    let mut macro_start = line.find("{{");
    loop {
        if tag.is_some_and(|at| at < from) {
            tag = tag_start(line, from);
        }
        if macro_start.is_some_and(|at| at < from) {
            macro_start = line[from..].find("{{").map(|at| from + at);
        }
        let (start, close, markup) = match (tag, macro_start) {
            (Some(tag), Some(macro_start)) if macro_start < tag => {
                (macro_start, "}}", Markup::Macro)
            }
            (Some(tag), _) => (tag, ">", Markup::Tag),
            (None, Some(macro_start)) => (macro_start, "}}", Markup::Macro),
            (None, None) => break,
        };
        match line[start..].find(close) {
            Some(length) => {
                let end = start + length + close.len();
                found.push((start..end, markup));
                from = end;
            }
            // Nothing closes this one, so nothing closes a later one of its kind either.
            None => {
                match markup {
                    Markup::Tag => tag = None,
                    Markup::Macro => macro_start = None,
                }
                from = start + 1;
            }
        }
    }
    found
}

/// Where the first tag at or after byte `from` of `line` starts. A tag starts like `<a`, `</`,
/// `<!` or `<?`, so "a < b" is left alone.
fn tag_start(line: &str, from: usize) -> Option<usize> {
    let rest = &line[from..];
    let (at, _) = rest.match_indices('<').find(|(at, _)| {
        let after = rest[at + 1..].chars().next();
        after.is_some_and(|c| c.is_alphabetic() || "/!?".contains(c))
    })?;
    Some(from + at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(line: &str) -> Vec<(&str, Markup)> {
        markup_ranges(line)
            .into_iter()
            .map(|(range, markup)| (&line[range], markup))
            .collect()
    }

    #[test]
    fn finds_tags_and_macros() {
        assert_eq!(
            marked(r#"<system_instructions status="enabled">Hi {{char}}</system_instructions>"#),
            vec![
                (r#"<system_instructions status="enabled">"#, Markup::Tag),
                ("{{char}}", Markup::Macro),
                ("</system_instructions>", Markup::Tag),
            ]
        );
        assert_eq!(
            marked("{{#x}}…{{/x}}"),
            vec![("{{#x}}", Markup::Macro), ("{{/x}}", Markup::Macro)]
        );
    }

    #[test]
    fn plain_prose_is_left_alone() {
        assert!(marked("if a < b and c > d, it's fine").is_empty());
        assert!(marked("an unclosed <tag and {{macro").is_empty());
        assert!(marked("").is_empty());
    }

    #[test]
    fn one_kind_left_unclosed_leaves_the_other_found() {
        assert_eq!(
            marked("<b {{x}} and {{y}}"),
            vec![("{{x}}", Markup::Macro), ("{{y}}", Markup::Macro)]
        );
        assert_eq!(
            marked("{{x <b> and </b>"),
            vec![("<b>", Markup::Tag), ("</b>", Markup::Tag)]
        );
        // A macro inside a tag is part of the tag.
        assert_eq!(
            marked("a < b <i {{x}}>{{y}}"),
            vec![("<i {{x}}>", Markup::Tag), ("{{y}}", Markup::Macro)]
        );
    }

    #[test]
    fn a_long_line_of_tags_is_read_once() {
        // Minified HTML or SVG: looking through the rest of the line after each tag took 2 s
        // for this much, on every draw.
        let closed = "<a>b</a>".repeat(25_000);
        assert_eq!(markup_ranges(&closed).len(), 50_000);
        let unclosed = "<a {{b ".repeat(30_000);
        assert!(markup_ranges(&unclosed).is_empty());
    }
}
