//! Markdown in a .md or .markdown file: which parts of each line to colour, what Enter and Tab do
//! in a list, and the markers Ctrl+B and Ctrl+I put in. Nothing is rendered; the text stays
//! exactly as typed. Lines are read one at a time, each starting in the `Block` the line above
//! left it in: prose, or inside a fenced code block.

use std::ops::Range;
use std::path::Path;

/// The marker Ctrl+B puts around text, for bold.
pub const BOLD: &str = "**";
/// The marker Ctrl+I puts around text, for italic: an underscore, as GitHub's editor uses, so it
/// never runs into a bold `**`.
pub const ITALIC: &str = "_";
/// Ctrl+I's marker against a letter or digit, where Markdown reads a `_` as part of the word.
const ITALIC_IN_A_WORD: &str = "*";

/// Whether `path` is a Markdown file: its extension is .md or .markdown, in any case.
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

/// Where a line starts: in prose, or inside a fenced code block opened by `length` (3 or more)
/// of `fence` (a backtick or a tilde).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Block {
    Prose,
    Code { fence: u8, length: usize },
}

/// What a coloured part of a line is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Markdown {
    /// A `#` heading line.
    Heading,
    /// A thematic break (`---`) or a heading's `===` underline.
    Rule,
    /// A `>` block quote line.
    Quote,
    /// A list item's bullet or number, and its task box if it has one.
    ListMarker,
    /// The ``` or ~~~ line that opens or closes a fenced code block.
    Fence,
    /// A line inside a fenced code block, or a `code span`.
    Code,
    Bold,
    Italic,
    /// `~~Struck~~` text.
    Strike,
    /// A link's `[text]`, or an image's `![alt]`.
    LinkText,
    /// A link's or an image's `(url)`.
    LinkUrl,
}

/// The block the line after `line` starts in, when `line` starts in `block`.
pub fn block_after(line: &str, block: Block) -> Block {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let mark = match trimmed.as_bytes().first() {
        Some(&mark @ (b'`' | b'~')) => mark,
        _ => return block,
    };
    let length = trimmed.bytes().take_while(|&byte| byte == mark).count();
    let info = &trimmed[length..];
    match block {
        // Closed by the same character, at least as many, with nothing after but spaces.
        Block::Code {
            fence,
            length: opened,
        } => {
            if mark == fence && length >= opened && info.trim().is_empty() {
                Block::Prose
            } else {
                block
            }
        }
        // A backtick fence's info string can't hold a backtick: ```x``` is a code span.
        Block::Prose if length >= 3 && !(mark == b'`' && info.contains('`')) => Block::Code {
            fence: mark,
            length,
        },
        Block::Prose => block,
    }
}

/// The coloured parts of `line` when it starts in `block`, as byte ranges with outer ones
/// first (a later range wins where they overlap), and the block the next line starts in.
pub fn markdown_ranges(line: &str, block: Block) -> (Vec<(Range<usize>, Markdown)>, Block) {
    let next = block_after(line, block);
    let whole = |markdown| match line.is_empty() {
        true => Vec::new(),
        false => vec![(0..line.len(), markdown)],
    };
    match (block, next) {
        (Block::Code { .. }, Block::Code { .. }) => return (whole(Markdown::Code), next),
        (Block::Code { .. }, _) | (_, Block::Code { .. }) => return (whole(Markdown::Fence), next),
        _ => {}
    }
    // Headings, rules and quotes start within 3 spaces of the line's start.
    let body = line.trim_start_matches(' ');
    if line.len() - body.len() <= 3 {
        let hashes = body.bytes().take_while(|&byte| byte == b'#').count();
        if (1..=6).contains(&hashes)
            && matches!(body.as_bytes().get(hashes), None | Some(b' ' | b'\t'))
        {
            return (whole(Markdown::Heading), next);
        }
        if is_rule(body) {
            return (whole(Markdown::Rule), next);
        }
        if body.starts_with('>') {
            return (whole(Markdown::Quote), next);
        }
    }
    let mut found = Vec::new();
    let mut from = 0;
    if let Some(marker) = list_marker(line, 0) {
        from = marker.end;
        found.push((marker, Markdown::ListMarker));
    }
    found.extend(inline_ranges(line, from));
    (found, next)
}

/// A thematic break (3 or more `-`, `*` or `_`, spaces allowed between) or a heading's
/// underline (`===`), from its first character.
fn is_rule(body: &str) -> bool {
    let mut marks = body.chars().filter(|c| !matches!(c, ' ' | '\t'));
    let Some(first) = marks.next().filter(|c| "-*_=".contains(*c)) else {
        return false;
    };
    let mut count = 1;
    for mark in marks {
        if mark != first {
            return false;
        }
        count += 1;
    }
    count >= 3
}

/// Where a list item's marker is, after any spaces from byte `from`: `-`, `*`, `+`, or a number
/// with `.` or `)`, followed by a space, and a task box (`[ ]`, `[x]`) after it, if there is one.
fn list_marker(line: &str, from: usize) -> Option<Range<usize>> {
    let start = line.len() - line[from..].trim_start_matches([' ', '\t']).len();
    let rest = &line.as_bytes()[start..];
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let marker = match rest.first()? {
        b'-' | b'*' | b'+' => 1,
        _ if (1..=9).contains(&digits) && matches!(rest.get(digits), Some(b'.' | b')')) => {
            digits + 1
        }
        _ => return None,
    };
    if rest.get(marker) != Some(&b' ') {
        return None;
    }
    let after = &rest[marker + 1..];
    let boxed = [b"[ ]", b"[x]", b"[X]"]
        .iter()
        .any(|task| after.starts_with(*task))
        && matches!(after.get(3), None | Some(b' '));
    Some(start..start + marker + if boxed { 4 } else { 0 })
}

/// What Enter does on a Markdown list item or quote line (see `list_enter`).
#[derive(Debug, PartialEq)]
pub enum ListEnter {
    /// Start the next item: type a newline and this prefix (the same indentation and `>` quote
    /// markers, the same bullet or the next number, and an empty task box if there was one).
    Continue(String),
    /// The item is empty and indented: unindent it one step, as Shift+Tab does.
    Unindent,
    /// The item is empty: delete these bytes of the text, its marker and task box (or, on a quote
    /// line with nothing else, its quote markers).
    Delete(Range<usize>),
}

/// What Enter does at byte `caret` of `text`, a Markdown file, when the caret's line is a list
/// item or a `>` quote and the caret is after the line's prefix (its indentation, quote markers,
/// list marker and task box). `None` means a plain new line: on any other line, with the caret
/// in the prefix, or in fenced code.
pub fn list_enter(text: &str, caret: usize) -> Option<ListEnter> {
    let (line_start, line) = prose_line(text, caret)?;
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let mut lead = indent;
    while line[lead..].starts_with('>') {
        lead = line.len() - line[lead + 1..].trim_start_matches([' ', '\t']).len();
    }
    // `* * *` is a thematic break, not a list.
    let marker = list_marker(line, lead).filter(|_| !is_rule(&line[lead..]));
    let prefix_end = marker
        .as_ref()
        .map_or(lead, |marker| (marker.end + 1).min(line.len()));
    if caret - line_start < prefix_end || (marker.is_none() && lead == indent) {
        return None;
    }
    if !line[prefix_end..].trim().is_empty() {
        let next = marker.map_or(String::new(), |marker| next_marker(&line[marker]));
        return Some(ListEnter::Continue(format!("{}{next}", &line[..lead])));
    }
    let peeled = match marker {
        // Indented with spaces, which Shift+Tab can take off (it leaves a tab alone).
        Some(_) if lead == indent && line.starts_with(' ') => return Some(ListEnter::Unindent),
        Some(_) => lead..line.len(),
        None => indent..line.len(),
    };
    Some(ListEnter::Delete(
        line_start + peeled.start..line_start + peeled.end,
    ))
}

/// Whether the line byte `at` of `text` is on is a list item (a quote's items aren't counted),
/// outside fenced code.
pub fn is_list_item(text: &str, at: usize) -> bool {
    prose_line(text, at).is_some_and(|(_, line)| list_marker(line, 0).is_some() && !is_rule(line))
}

/// The line byte `at` of `text` is on, and where it starts, unless it is inside fenced code.
fn prose_line(text: &str, at: usize) -> Option<(usize, &str)> {
    let start = text[..at].rfind('\n').map_or(0, |newline| newline + 1);
    let end = text[at..]
        .find('\n')
        .map_or(text.len(), |newline| at + newline);
    let block = text[..start]
        .lines()
        .fold(Block::Prose, |block, line| block_after(line, block));
    (block == Block::Prose).then(|| (start, &text[start..end]))
}

/// The marker for the item after one marked `marker` (such as `-`, `9.` or `- [x]`): the same
/// bullet or the next number, an empty task box if it had one, and a space.
fn next_marker(marker: &str) -> String {
    let (bullet, task) = match marker.split_once(' ') {
        Some((bullet, _)) => (bullet, "[ ] "),
        None => (marker, ""),
    };
    let digits = bullet.trim_end_matches(['.', ')']);
    match digits.parse::<u64>() {
        Ok(number) => format!("{}{} {task}", number + 1, &bullet[digits.len()..]),
        Err(_) => format!("{bullet} {task}"),
    }
}

/// Ctrl+B and Ctrl+I: puts `marker` around `selection` (a byte range of `text`, empty for just
/// the caret), or takes it off when the selection already has it, just outside or at its ends.
/// Italic is a `*` instead when a letter or digit is right outside the selection, and comes off
/// as either. With just the caret, only an empty pair around it comes off, not the end of one span and the
/// start of the next. Returns the range of `text` to replace, what to put there, and what to
/// select afterwards: the same text, or the caret between a new pair.
pub fn toggle_marker(
    text: &str,
    selection: Range<usize>,
    marker: &str,
) -> (Range<usize>, String, Range<usize>) {
    let (start, end) = (selection.start, selection.end);
    let selected = &text[selection.clone()];
    let markers = match marker {
        ITALIC => &[ITALIC, ITALIC_IN_A_WORD][..],
        _ => &[marker][..],
    };
    for &marker in markers {
        let length = marker.len();
        if ends_with_marker(&text[..start], marker)
            && starts_with_marker(&text[end..], marker)
            && (start < end || opens_a_pair(text, start, marker))
        {
            let outer = start - length..end + length;
            return (outer, selected.to_string(), start - length..end - length);
        }
        if selected.len() >= 2 * length
            && starts_with_marker(selected, marker)
            && ends_with_marker(selected, marker)
        {
            let inside = selected[length..selected.len() - length].to_string();
            return (selection, inside, start..end - 2 * length);
        }
    }
    let in_a_word =
        is_word(text[..start].chars().next_back()) || is_word(text[end..].chars().next());
    let marker = match marker == ITALIC && in_a_word {
        true => ITALIC_IN_A_WORD,
        false => marker,
    };
    let length = marker.len();
    let wrapped = format!("{marker}{selected}{marker}");
    (selection, wrapped, start + length..end + length)
}

/// Whether the `marker` that ends just before byte `at` of `text` opens a pair rather than closing
/// one: it is the odd one out of the markers on its line so far. A `_` inside a word, as in
/// snake_case, isn't a marker.
fn opens_a_pair(text: &str, at: usize, marker: &str) -> bool {
    let line = &text[text[..at].rfind('\n').map_or(0, |newline| newline + 1)..at];
    let markers = line.match_indices(marker).filter(|&(found, _)| {
        let (before, after) = (&line[..found], &line[found + marker.len()..]);
        marker != ITALIC || !(is_word(before.chars().next_back()) && is_word(after.chars().next()))
    });
    markers.count() % 2 == 1
}

/// Whether `text` ends with `marker`. Italic's `*` counts only as the last of an odd run of them,
/// so bold's `**` is never taken for it.
fn ends_with_marker(text: &str, marker: &str) -> bool {
    match marker {
        ITALIC_IN_A_WORD => (text.len() - text.trim_end_matches('*').len()) % 2 == 1,
        _ => text.ends_with(marker),
    }
}

/// Whether `text` starts with `marker`, as `ends_with_marker` reads its end.
fn starts_with_marker(text: &str, marker: &str) -> bool {
    match marker {
        ITALIC_IN_A_WORD => (text.len() - text.trim_start_matches('*').len()) % 2 == 1,
        _ => text.starts_with(marker),
    }
}

/// Whether `c` is a letter or digit, which a `_` next to it joins into a word.
fn is_word(c: Option<char>) -> bool {
    c.is_some_and(char::is_alphanumeric)
}

/// Code spans, bold, italic, struck text and links in `line`, from byte `from`.
fn inline_ranges(line: &str, from: usize) -> Vec<(Range<usize>, Markdown)> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    // Runs of `*`, `_` or `~` that may still be closed: (character, start, length).
    let mut openers: Vec<(u8, usize, usize)> = Vec::new();
    let mut at = from;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte == b'\\' && bytes.get(at + 1).is_some_and(u8::is_ascii_punctuation) {
            at += 2;
            continue;
        }
        if let Some((text, url)) = link(bytes, at) {
            at = url.end;
            found.push((text, Markdown::LinkText));
            found.push((url, Markdown::LinkUrl));
            continue;
        }
        if !matches!(byte, b'`' | b'*' | b'_' | b'~') {
            at += 1;
            continue;
        }
        let run = bytes[at..].iter().take_while(|&&b| b == byte).count();
        if byte == b'`' {
            // Closed by the next run of exactly as many backticks, or else just backticks.
            if let Some(end) = closing_run(bytes, at + run, run) {
                found.push((at..end, Markdown::Code));
                at = end;
                continue;
            }
        } else if matches!((byte, run), (b'*' | b'_', 1..=3) | (b'~', 2)) {
            // An opener needs a non-space after it and a closer one before it, and `_` counts
            // only at the edge of a word, so snake_case_names stay plain.
            let before = line[..at].chars().next_back();
            let after = line[at + run..].chars().next();
            let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
            let underscore = byte == b'_';
            let opens = after.is_some_and(|c| !c.is_whitespace()) && !(underscore && word(before));
            let closes = before.is_some_and(|c| !c.is_whitespace()) && !(underscore && word(after));
            // A closer pairs with the latest opener of its length. A run of three also pairs with
            // one of one or two, and the rest of it closes or stays open on its own, so
            // `**a *b***` and `***a** b*` are each bold and italic. `left` is what hasn't closed.
            let mut left = run;
            while let Some(index) = openers.iter().rposition(|&(b, _, length)| {
                closes && left > 0 && b == byte && (length == left || length.max(left) == 3)
            }) {
                let (_, start, length) = openers[index];
                let used = length.min(left);
                openers.truncate(index);
                if length > used {
                    openers.push((byte, start, length - used));
                }
                let markdown = match (byte, used) {
                    (b'~', _) => Markdown::Strike,
                    (_, 1) => Markdown::Italic,
                    // No bold italic face is bundled, so ***this*** is bold.
                    _ => Markdown::Bold,
                };
                let closer = at + run - left;
                found.push((start + length - used..closer + used, markdown));
                left -= used;
            }
            if left == run && opens {
                openers.push((byte, at, run));
            }
        }
        at += run;
    }
    // Emphasis is found when it closes, after anything inside it; outer ranges go first.
    found.sort_by_key(|(range, _)| range.start);
    found
}

/// Where the first run of exactly `length` backticks from byte `at` ends.
fn closing_run(bytes: &[u8], mut at: usize, length: usize) -> Option<usize> {
    while at < bytes.len() {
        let run = bytes[at..].iter().take_while(|&&b| b == b'`').count();
        if run == length {
            return Some(at + run);
        }
        at += run.max(1);
    }
    None
}

/// A link `[text](url)` or an image `![alt](url)` at byte `at`: its bracketed part and its
/// `(url)`, which may hold balanced brackets of its own.
fn link(bytes: &[u8], at: usize) -> Option<(Range<usize>, Range<usize>)> {
    let open = match &bytes[at..] {
        [b'!', b'[', ..] => at + 1,
        [b'[', ..] => at,
        _ => return None,
    };
    let close = open + bytes[open..].iter().position(|&b| b == b']')?;
    if bytes.get(close + 1) != Some(&b'(') {
        return None;
    }
    let mut depth = 0;
    for (end, &byte) in bytes.iter().enumerate().skip(close + 1) {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((at..close + 1, close + 1..end + 1));
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use Markdown::*;

    /// The coloured parts of `line`, starting in prose, which must also be where it ends.
    fn marked(line: &str) -> Vec<(&str, Markdown)> {
        let (ranges, next) = markdown_ranges(line, Block::Prose);
        assert_eq!(next, Block::Prose, "{line:?} opened a fence");
        ranges
            .into_iter()
            .map(|(range, markdown)| (&line[range], markdown))
            .collect()
    }

    /// Each line's coloured parts, reading `lines` from the top in prose.
    fn read(lines: &[&'static str]) -> Vec<Vec<(&'static str, Markdown)>> {
        let mut block = Block::Prose;
        let mut found = Vec::new();
        for line in lines {
            let (ranges, next) = markdown_ranges(line, block);
            block = next;
            found.push(
                ranges
                    .into_iter()
                    .map(|(range, markdown)| (&line[range], markdown))
                    .collect(),
            );
        }
        found
    }

    #[test]
    fn markdown_files_go_by_their_extension() {
        for name in [
            "notes.md",
            "README.MD",
            "a.b/notes.Markdown",
            "/x/y.markdown",
        ] {
            assert!(is_markdown(Path::new(name)), "{name}");
        }
        for name in ["notes.txt", "notes", "notes.mdx", "md", ".md.txt"] {
            assert!(!is_markdown(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn headings_are_one_to_six_hashes_and_a_space() {
        for line in ["# Title", "###### Six", "   ## Three spaces in", "#", "## "] {
            assert_eq!(marked(line), vec![(line, Heading)], "{line:?}");
        }
        for line in ["####### Seven", "#hashtag", "    # four spaces is not"] {
            assert!(marked(line).iter().all(|(_, m)| *m != Heading), "{line:?}");
        }
    }

    #[test]
    fn rules_underlines_and_quotes_take_the_whole_line() {
        for line in [
            "---",
            "***",
            "___",
            "- - -",
            " * * * ",
            "=====",
            "----------",
        ] {
            assert_eq!(marked(line), vec![(line, Rule)], "{line:?}");
        }
        assert!(marked("--").is_empty());
        assert!(marked("-*-").is_empty());
        for line in ["> A quote with **bold**", "   > three spaces", ">"] {
            assert_eq!(marked(line), vec![(line, Quote)], "{line:?}");
        }
        assert_eq!(marked("a > b"), vec![]);
    }

    #[test]
    fn list_markers_and_task_boxes() {
        assert_eq!(marked("- item"), vec![("-", ListMarker)]);
        assert_eq!(marked("      * deep"), vec![("*", ListMarker)]);
        assert_eq!(marked("+ plus"), vec![("+", ListMarker)]);
        assert_eq!(marked("1. one"), vec![("1.", ListMarker)]);
        assert_eq!(marked("  12) twelve"), vec![("12)", ListMarker)]);
        assert_eq!(marked("- [ ] to do"), vec![("- [ ]", ListMarker)]);
        assert_eq!(marked("3. [x] done"), vec![("3. [x]", ListMarker)]);
        assert_eq!(marked("- [X]"), vec![("- [X]", ListMarker)]);
        assert_eq!(marked("- [y] not a box"), vec![("-", ListMarker)]);
        assert_eq!(
            marked("- *item* text"),
            vec![("-", ListMarker), ("*item*", Italic)]
        );
        for line in [
            "-item",
            "1.5 litres",
            "1234567890. too long",
            "**bold** start",
        ] {
            assert!(
                marked(line).iter().all(|(_, m)| *m != ListMarker),
                "{line:?}"
            );
        }
        assert_eq!(
            marked("* * *"),
            vec![("* * *", Rule)],
            "a break, not a list"
        );
    }

    #[test]
    fn fenced_code_runs_from_its_fence_to_the_closing_one() {
        assert_eq!(
            read(&["```rust", "let x = *y*;", "", "```", "*after*"]),
            vec![
                vec![("```rust", Fence)],
                vec![("let x = *y*;", Code)],
                vec![],
                vec![("```", Fence)],
                vec![("*after*", Italic)],
            ]
        );
        // Closed only by the same character, at least as many, with nothing after but spaces.
        assert_eq!(
            read(&[
                "  ````md",
                "```",
                "~~~~",
                "```` not yet",
                "`````  ",
                "# done"
            ]),
            vec![
                vec![("  ````md", Fence)],
                vec![("```", Code)],
                vec![("~~~~", Code)],
                vec![("```` not yet", Code)],
                vec![("`````  ", Fence)],
                vec![("# done", Heading)],
            ]
        );
        // An unclosed fence runs to the end; a tilde fence's info may hold backticks.
        let (_, block) = markdown_ranges("~~~ `info`", Block::Prose);
        assert_eq!(
            block,
            Block::Code {
                fence: b'~',
                length: 3
            }
        );
        assert_eq!(
            markdown_ranges("# not a heading", block),
            (vec![(0..15, Code)], block)
        );
        // A backtick fence's can't, so this is a code span.
        assert_eq!(marked("```x``` y"), vec![("```x```", Code)]);
    }

    #[test]
    fn code_spans_match_backtick_runs_of_the_same_length() {
        assert_eq!(marked("a `b` c"), vec![("`b`", Code)]);
        assert_eq!(marked("``a ` b`` c"), vec![("``a ` b``", Code)]);
        assert_eq!(marked("an ` alone"), vec![]);
        assert_eq!(marked("`` x ` y"), vec![]);
        // Nothing inside a code span is emphasis.
        assert_eq!(
            marked("`*not* [a](b)` *yes*"),
            vec![("`*not* [a](b)`", Code), ("*yes*", Italic)]
        );
    }

    #[test]
    fn bold_italic_and_struck_text() {
        assert_eq!(marked("**bold**"), vec![("**bold**", Bold)]);
        assert_eq!(marked("__bold__ x"), vec![("__bold__", Bold)]);
        assert_eq!(marked("an *italic* word"), vec![("*italic*", Italic)]);
        assert_eq!(marked("_italic_"), vec![("_italic_", Italic)]);
        assert_eq!(marked("***both***"), vec![("***both***", Bold)]);
        assert_eq!(marked("~~gone~~"), vec![("~~gone~~", Strike)]);
        assert_eq!(marked("~one~"), vec![]);
        // Outer first, so what is inside wins.
        assert_eq!(
            marked("**bold *it* `c`**"),
            vec![("**bold *it* `c`**", Bold), ("*it*", Italic), ("`c`", Code)]
        );
        // Mismatched runs don't pair up, but a run of three is bold and italic together.
        assert_eq!(marked("**half*"), vec![]);
        assert_eq!(
            marked("**bold *italic***"),
            vec![("**bold *italic***", Bold), ("*italic*", Italic)]
        );
        assert_eq!(
            marked("*a **b***"),
            vec![("*a **b***", Italic), ("**b**", Bold)]
        );
        assert_eq!(
            marked("***bold** italic*"),
            vec![("***bold** italic*", Italic), ("**bold**", Bold)]
        );
        assert_eq!(
            marked("_a __b___ c"),
            vec![("_a __b___", Italic), ("__b__", Bold)]
        );
    }

    #[test]
    fn emphasis_needs_text_against_its_markers() {
        for line in [
            "2 * 3 * 4",
            "** not bold **",
            "snake_case_names and {{snake_case}}",
            "a_b_ and _c_d",
            r"\*escaped\* and \_too\_",
            "*no close",
            "*across",
        ] {
            assert_eq!(marked(line), vec![], "{line:?}");
        }
        assert_eq!(marked("(_italic_)"), vec![("_italic_", Italic)]);
        assert_eq!(marked("a*b*c"), vec![("*b*", Italic)]);
        assert_eq!(marked(r"\**x*"), vec![("*x*", Italic)]);
        assert_eq!(marked("a ~~~b~~ c **** d"), vec![]);
    }

    #[test]
    fn links_and_images() {
        assert_eq!(
            marked("see [the docs](https://x.org/a_b_(c)) now"),
            vec![
                ("[the docs]", LinkText),
                ("(https://x.org/a_b_(c))", LinkUrl)
            ]
        );
        assert_eq!(
            marked("![alt *x*](pic.png)"),
            vec![("![alt *x*]", LinkText), ("(pic.png)", LinkUrl)]
        );
        assert_eq!(
            marked("*see [a](b)*"),
            vec![
                ("*see [a](b)*", Italic),
                ("[a]", LinkText),
                ("(b)", LinkUrl)
            ]
        );
        for line in ["[no url]", "[a] (b)", "[a](unclosed", "!not an image"] {
            assert_eq!(marked(line), vec![], "{line:?}");
        }
    }

    #[test]
    fn ranges_fall_on_character_boundaries() {
        assert_eq!(
            marked("é *ü* 🙂 **日本** `語` ~~ñ~~ [😀](é)"),
            vec![
                ("*ü*", Italic),
                ("**日本**", Bold),
                ("`語`", Code),
                ("~~ñ~~", Strike),
                ("[😀]", LinkText),
                ("(é)", LinkUrl),
            ]
        );
        assert_eq!(marked("- 日本語"), vec![("-", ListMarker)]);
        assert_eq!(marked("_é_ é_é_é"), vec![("_é_", Italic)]);
        assert_eq!(marked("# 見出し"), vec![("# 見出し", Heading)]);
        assert_eq!(marked(""), vec![]);
        assert_eq!(read(&["```", ""]), vec![vec![("```", Fence)], vec![]]);
    }

    /// What Enter does with the caret at the `|` in `text`.
    fn enter_at(text: &str) -> Option<ListEnter> {
        let caret = text.find('|').expect("a caret");
        list_enter(&text.replace('|', ""), caret)
    }

    #[test]
    fn enter_continues_a_list_item_or_quote() {
        let next = |text| match enter_at(text) {
            Some(ListEnter::Continue(prefix)) => prefix,
            other => panic!("{text:?}: {other:?}"),
        };
        assert_eq!(next("- item|"), "- ");
        assert_eq!(next("* a|b"), "* ");
        assert_eq!(next("+ x|"), "+ ");
        assert_eq!(next("1. one|"), "2. ");
        assert_eq!(next("9) nine|"), "10) ");
        assert_eq!(next("- [x] done|"), "- [ ] ");
        assert_eq!(next("3. [ ] to do|"), "4. [ ] ");
        assert_eq!(next("    - nested|"), "    - ");
        assert_eq!(next("> quoted|"), "> ");
        assert_eq!(next(">> deeper|"), ">> ");
        assert_eq!(next("  > - in a quote|"), "  > - ");
        assert_eq!(next("> 1) x|"), "> 2) ");
        assert_eq!(next("- café 🙂 日本|"), "- ");
        assert_eq!(next("- |text"), "- ", "at the start of the item's text");
        assert_eq!(next("a\n- b|\nc"), "- ");
        assert_eq!(next("```\nx\n```\n- after a fence|"), "- ");
    }

    #[test]
    fn enter_on_an_empty_item_takes_off_one_layer() {
        assert_eq!(enter_at("  - |"), Some(ListEnter::Unindent));
        assert_eq!(enter_at("a\n- |"), Some(ListEnter::Delete(2..4)));
        assert_eq!(enter_at("1. [ ]   |"), Some(ListEnter::Delete(0..9)));
        assert_eq!(enter_at("- [x]|"), Some(ListEnter::Delete(0..5)));
        // In a quote, the marker goes and the quote stays; then the quote goes.
        assert_eq!(enter_at("> - |\nb"), Some(ListEnter::Delete(2..4)));
        assert_eq!(enter_at(">> |"), Some(ListEnter::Delete(0..3)));
        assert_eq!(enter_at("  >|"), Some(ListEnter::Delete(2..3)));
        // Shift+Tab can't take off a tab, so the marker goes instead.
        assert_eq!(enter_at("\t- |"), Some(ListEnter::Delete(1..3)));
    }

    #[test]
    fn enter_is_plain_in_the_prefix_in_code_and_on_other_lines() {
        for text in [
            "|- item",
            "-| item",
            "  |  - item",
            "- [|x] done",
            ">| quote",
            "plain text|",
            "# heading|",
            "* * *|",
            "- - -|",
            "-item|",
            "1.5 litres|",
            "|",
            "```\n- in code|\n```",
            "~~~\n> quoted code|",
        ] {
            assert_eq!(enter_at(text), None, "{text:?}");
        }
    }

    #[test]
    fn list_items_are_lines_with_a_marker_outside_fenced_code() {
        let text = "- a\n  1. b é\n> - c\n* * *\ntext\n```\n- d\n```\n- [ ] e";
        let line_starts = std::iter::once(0).chain(text.match_indices('\n').map(|(at, _)| at + 1));
        let items: Vec<bool> = line_starts.map(|at| is_list_item(text, at)).collect();
        assert_eq!(
            items,
            [true, true, false, false, false, false, false, false, true]
        );
        assert!(!is_list_item("", 0));
    }

    #[test]
    fn markers_go_around_the_selection_or_come_off() {
        // The text afterwards, and what is selected in it.
        let toggled = |text: &str, selection: Range<usize>, marker| {
            let (replaced, with, selected) = toggle_marker(text, selection, marker);
            let mut after = text.to_string();
            after.replace_range(replaced, &with);
            let chosen = after[selected].to_string();
            (after, chosen)
        };
        let pair = |after: &str, chosen: &str| (after.to_string(), chosen.to_string());
        assert_eq!(
            toggled("a word b", 2..6, BOLD),
            pair("a **word** b", "word")
        );
        assert_eq!(
            toggled("a word b", 2..6, ITALIC),
            pair("a _word_ b", "word")
        );
        // Markers just outside the selection, or at its ends, come off.
        assert_eq!(
            toggled("a **word** b", 4..8, BOLD),
            pair("a word b", "word")
        );
        assert_eq!(
            toggled("a **word** b", 2..10, BOLD),
            pair("a word b", "word")
        );
        assert_eq!(toggled("_x_", 0..3, ITALIC), pair("x", "x"));
        // Italic inside bold is a marker of its own.
        assert_eq!(
            toggled("**word**", 2..6, ITALIC),
            pair("**_word_**", "word")
        );
        // Against a letter or digit italic is a `*`, which Markdown reads inside a word too, and
        // it comes off as `_` or `*`, never taking a star from bold.
        assert_eq!(toggled("é日本🙂", 2..8, ITALIC), pair("é*日本*🙂", "日本"));
        assert_eq!(
            toggled("foobarbaz", 3..6, ITALIC),
            pair("foo*bar*baz", "bar")
        );
        assert_eq!(marked("foo*bar*baz"), vec![("*bar*", Italic)]);
        assert_eq!(
            toggled("foo*bar*baz", 4..7, ITALIC),
            pair("foobarbaz", "bar")
        );
        assert_eq!(toggled("a *b* c", 2..5, ITALIC), pair("a b c", "b"));
        assert_eq!(toggled("***b***", 3..4, ITALIC), pair("**b**", "b"));
        assert_eq!(toggled("***b***", 3..4, BOLD), pair("*b*", "b"));
        assert_eq!(toggled("x*b*", 2..3, BOLD), pair("x***b***", "b"));
        assert_eq!(
            toggle_marker("ab", 1..1, ITALIC),
            (1..1, "**".to_string(), 2..2)
        );
        assert_eq!(
            toggle_marker("a**b", 2..2, ITALIC),
            (1..3, String::new(), 1..1)
        );
        // Nothing selected: a pair goes in around the caret, and comes out again.
        assert_eq!(
            toggle_marker("ab", 1..1, BOLD),
            (1..1, "****".to_string(), 3..3)
        );
        assert_eq!(
            toggle_marker("a****b", 3..3, BOLD),
            (1..5, String::new(), 1..1)
        );
        assert_eq!(
            toggle_marker("", 0..0, ITALIC),
            (0..0, "__".to_string(), 1..1)
        );
        assert_eq!(
            toggle_marker("**a** x****", 9..9, BOLD),
            (7..11, String::new(), 7..7)
        );
        assert_eq!(
            toggle_marker("snake_case __", 12..12, ITALIC),
            (11..13, String::new(), 11..11)
        );
        // Between two spans, a new pair goes in rather than the spans running together.
        assert_eq!(
            toggle_marker("**a****b**", 5..5, BOLD),
            (5..5, "****".to_string(), 7..7)
        );
        assert_eq!(
            toggle_marker("_a__b_", 3..3, ITALIC),
            (3..3, "__".to_string(), 4..4)
        );
    }
}
