//! XML tags and {{macros}} in a prompt: where they are, for colouring them and for leaving them
//! out of names. Tags and macros are found one line at a time.

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
    while from < line.len() {
        let rest = &line[from..];
        let next_tag = rest.match_indices('<').find(|(at, _)| {
            // A tag starts like `<a`, `</`, `<!` or `<?`, so "a < b" is left alone.
            let after = rest[at + 1..].chars().next();
            after.is_some_and(|c| c.is_alphabetic() || "/!?".contains(c))
        });
        let candidates = [
            next_tag.map(|(at, _)| (at, ">", Markup::Tag)),
            rest.find("{{").map(|at| (at, "}}", Markup::Macro)),
        ];
        let Some((start, close, markup)) = candidates.into_iter().flatten().min_by_key(|c| c.0)
        else {
            break;
        };
        match rest[start..].find(close) {
            Some(length) => {
                let end = start + length + close.len();
                found.push((from + start..from + end, markup));
                from += end;
            }
            None => from += start + 1,
        }
    }
    found
}

/// `text` with each tag and macro replaced by a space, found the same way the editor colours
/// them.
pub fn without_markup(text: &str) -> String {
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let mut plain = line.to_string();
            for (range, _) in markup_ranges(line).into_iter().rev() {
                plain.replace_range(range, " ");
            }
            plain
        })
        .collect();
    lines.join("\n")
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
    fn without_markup_drops_what_the_editor_colours_and_nothing_else() {
        assert_eq!(without_markup("<b>Nyx</b> {{char}}"), " Nyx   ");
        // A tag never runs across a line break, as in the editor.
        assert_eq!(without_markup("if a<b then\nc>d"), "if a<b then\nc>d");
    }
}
