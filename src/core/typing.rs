//! What typing does in code, in shell scripts and JSON files: brackets and quotes that close
//! themselves, and Enter that indents a block and lines up the line that ends one.

use std::ops::Range;

use crate::core::shell::shell_ranges;
use crate::core::syntax::{Syntax, Token};

/// The pairs that close themselves in `syntax`, as (opener, closer). None outside code.
fn pairs(syntax: Syntax) -> &'static [(char, char)] {
    match syntax {
        Syntax::Shell => &[
            ('(', ')'),
            ('[', ']'),
            ('{', '}'),
            ('"', '"'),
            ('\'', '\''),
            ('`', '`'),
        ],
        Syntax::Json => &[('[', ']'), ('{', '}'), ('"', '"')],
        _ => &[],
    }
}

/// Whether typing in `syntax` gets these helpers: shell scripts and JSON.
pub fn is_code(syntax: Syntax) -> bool {
    !pairs(syntax).is_empty()
}

/// What typing `typed` over `selection` (a byte range of `text`, empty for a caret) does in
/// `syntax`, when that isn't just putting it in: the range replaced, the text put there, and what
/// is selected after (empty for a caret).
/// - An opener with text selected puts the pair around the text, which stays selected.
/// - An opener before a space, a closer or the end of the line puts in the pair, with the caret
///   between. A quote pairs only where a word doesn't come right before it (so `it's` doesn't).
/// - A closer right before the same closer steps over it.
pub fn typed(
    text: &str,
    selection: Range<usize>,
    typed: char,
    syntax: Syntax,
) -> Option<(Range<usize>, String, Range<usize>)> {
    let at = selection.start;
    let next = text[selection.end..].chars().next();
    let previous = text[..at].chars().next_back();
    let pairs = pairs(syntax);
    if selection.is_empty() && next == Some(typed) && pairs.iter().any(|&(_, c)| c == typed) {
        let after = at + typed.len_utf8();
        return Some((at..after, typed.to_string(), after..after));
    }
    let &(_, closer) = pairs.iter().find(|&&(opener, _)| opener == typed)?;
    let inside = at + typed.len_utf8();
    if !selection.is_empty() {
        let with = format!("{typed}{}{closer}", &text[selection.clone()]);
        return Some((selection.clone(), with, inside..inside + selection.len()));
    }
    let room = match next {
        None => true,
        Some(next) => next.is_whitespace() || pairs.iter().any(|&(_, c)| c == next && c != typed),
    };
    let after_word = previous.is_some_and(|c| c.is_alphanumeric() || c == '\\' || c == typed);
    if !room || typed == closer && after_word {
        return None;
    }
    Some((at..at, format!("{typed}{closer}"), inside..inside))
}

/// Whether the caret at `at` is between an empty pair, such as `(|)`, which Backspace then
/// deletes both halves of.
pub fn in_empty_pair(text: &str, at: usize, syntax: Syntax) -> bool {
    let previous = text[..at].chars().next_back();
    let next = text[at..].chars().next();
    pairs(syntax)
        .iter()
        .any(|&(opener, closer)| previous == Some(opener) && next == Some(closer))
}

/// The spaces at the start of `line`.
fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// `line` without any comment at its end, and its spaces there.
fn code(line: &str, syntax: Syntax) -> &str {
    let end = match syntax {
        Syntax::Shell => shell_ranges(line, None)
            .0
            .iter()
            .find(|(_, token)| *token == Token::Comment)
            .map_or(line.len(), |(range, _)| range.start),
        _ => line.len(),
    };
    line[..end].trim_end()
}

/// The words of a line of shell, split at spaces and `;`.
fn words(code: &str) -> impl DoubleEndedIterator<Item = &str> {
    code.split([' ', '\t', ';']).filter(|word| !word.is_empty())
}

/// Whether a line ending in `code` opens a block, whose lines go in one step: a bracket, or in a
/// script `then`, `do`, `else`, `case … in`, or a case's pattern (`start)`).
fn opens_block(code: &str, syntax: Syntax) -> bool {
    if code.ends_with(['{', '[', '(']) {
        return true;
    }
    if syntax != Syntax::Shell {
        return false;
    }
    let last = words(code).next_back();
    let pattern = code.ends_with(')') && !code.contains('(') && !code.starts_with(')');
    matches!(last, Some("then" | "do" | "else"))
        || last == Some("in") && words(code).next() == Some("case")
        || pattern
}

/// Whether `line` (without its indent) ends a block, so it lines up with the line that opened
/// it: a closing bracket, or in a script `fi`, `done`, `esac`, `else` or `elif`.
fn closes_block(line: &str, syntax: Syntax) -> bool {
    line.starts_with(['}', ']', ')'])
        || syntax == Syntax::Shell
            && matches!(
                words(line).next(),
                Some("fi" | "done" | "esac" | "else" | "elif")
            )
}

/// Whether a script's line ending in `code` ends a case's branch with `;;`, after which the next
/// branch's pattern goes a step back out. Not on a one-line branch (`a) echo a ;;`), whose
/// pattern is on the same line.
fn ends_branch(code: &str, syntax: Syntax) -> bool {
    syntax == Syntax::Shell
        && code.ends_with(";;")
        && !words(code).next().is_some_and(|word| word.ends_with(')'))
}

/// What Enter does in code, in place of `range` (a byte range of `text`), when it isn't the
/// plain new line: the range replaced, the text put there, and where the caret goes.
/// - After a line that opens a block, the new line goes in one step (`tab_spaces` spaces); after
///   a script's `;;`, one step out.
/// - Between a pair of brackets, such as `{|}`, the closer goes down a line too, with the new line
///   between them.
/// - A line that ends a block (`}`, `fi`, `done`, ...) lines up with the block's first line, the
///   way it would have been typed.
///
/// `None` with only spaces before the caret, where the plain new line moves them down.
pub fn enter(
    text: &str,
    range: Range<usize>,
    tab_spaces: usize,
    syntax: Syntax,
) -> Option<(Range<usize>, String, usize)> {
    let line_start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let before = &text[line_start..range.start];
    let words = before.trim_start_matches(' ');
    if words.trim().is_empty() {
        return None;
    }
    let mut base = indent(before);
    if closes_block(words, syntax) {
        let above = text[..line_start.saturating_sub(1)]
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty());
        if let Some(above) = above {
            let above_code = code(above, syntax);
            let target = if opens_block(above_code, syntax) {
                indent(above)
            } else if ends_branch(above_code, syntax) {
                indent(above).saturating_sub(2 * tab_spaces)
            } else {
                indent(above).saturating_sub(tab_spaces)
            };
            base = base.min(target);
        }
    }
    let line = format!("{}{words}", " ".repeat(base));
    let code = code(before, syntax);
    let new_indent = if opens_block(code, syntax) {
        base + tab_spaces
    } else if ends_branch(code, syntax) {
        base.saturating_sub(tab_spaces)
    } else {
        base
    };
    let mut with = format!("{line}\n{}", " ".repeat(new_indent));
    let caret = line_start + with.len();
    let closer = text[range.end..].chars().next();
    let between = pairs(syntax)
        .iter()
        .any(|&(opener, c)| opener != c && before.ends_with(opener) && closer == Some(c));
    if between {
        with.push('\n');
        with.push_str(&" ".repeat(base));
    }
    Some((line_start..range.end, with, caret))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Types `c` into `text` at the `|` (or over the text between two `|`s), and shows the result
    /// the same way, or `None` when it's just typed.
    fn type_in(text: &str, c: char, syntax: Syntax) -> Option<String> {
        let marks: Vec<usize> = text.match_indices('|').map(|(at, _)| at).collect();
        let plain = text.replace('|', "");
        let selection = match marks[..] {
            [at] => at..at,
            [start, end] => start..end - 1,
            _ => unreachable!(),
        };
        let (range, with, selected) = typed(&plain, selection, c, syntax)?;
        let mut result = plain.clone();
        result.replace_range(range, &with);
        match selected.is_empty() {
            true => result.insert(selected.start, '|'),
            false => {
                result.insert(selected.end, '|');
                result.insert(selected.start, '|');
            }
        }
        Some(result)
    }

    #[test]
    fn openers_close_themselves_and_closers_step_over() {
        let shell = Syntax::Shell;
        assert_eq!(type_in("f|", '{', shell).as_deref(), Some("f{|}"));
        assert_eq!(type_in("echo |", '"', shell).as_deref(), Some("echo \"|\""));
        assert_eq!(type_in("x(|)", '[', shell).as_deref(), Some("x([|])"));
        assert_eq!(type_in("[ |]", ']', shell).as_deref(), Some("[ ]|"));
        assert_eq!(type_in("\"a|\"", '"', shell).as_deref(), Some("\"a\"|"));
        // Not before a word, nor a quote after one.
        assert_eq!(type_in("|word", '(', shell), None);
        assert_eq!(type_in("it|", '\'', shell), None);
        // A selection gets the pair around it and stays selected.
        assert_eq!(
            type_in("echo |$x|", '"', shell).as_deref(),
            Some("echo \"|$x|\"")
        );
        // JSON has no ' or ` pairs, and outside code nothing pairs.
        assert_eq!(type_in("|", '\'', Syntax::Json), None);
        assert_eq!(type_in("|", '{', Syntax::Json).as_deref(), Some("{|}"));
        assert_eq!(type_in("|", '{', Syntax::Plain), None);
        assert_eq!(type_in("|)", ')', Syntax::Plain), None);
    }

    #[test]
    fn backspace_takes_an_empty_pair() {
        assert!(in_empty_pair("x()", 2, Syntax::Shell));
        assert!(in_empty_pair("\"\"", 1, Syntax::Json));
        assert!(!in_empty_pair("(a)", 1, Syntax::Shell));
        assert!(!in_empty_pair("''", 1, Syntax::Json));
    }

    /// Presses Enter at the `|` in `text`, and shows the result with the caret as `|`.
    fn press_enter(text: &str, syntax: Syntax) -> Option<String> {
        let at = text.find('|').expect("a caret");
        let plain = text.replace('|', "");
        let (range, with, caret) = enter(&plain, at..at, 4, syntax)?;
        let mut result = plain.clone();
        result.replace_range(range, &with);
        result.insert(caret, '|');
        Some(result)
    }

    #[test]
    fn enter_after_an_opener_indents_the_block() {
        let shell = Syntax::Shell;
        assert_eq!(
            press_enter("if x; then|", shell).unwrap(),
            "if x; then\n    |"
        );
        assert_eq!(
            press_enter("  for f in *; do # each|", shell).unwrap(),
            "  for f in *; do # each\n      |"
        );
        assert_eq!(
            press_enter("case $1 in|", shell).unwrap(),
            "case $1 in\n    |"
        );
        assert_eq!(
            press_enter("    start)|", shell).unwrap(),
            "    start)\n        |"
        );
        assert_eq!(
            press_enter("        ;;|", shell).unwrap(),
            "        ;;\n    |"
        );
        assert_eq!(
            press_enter("    a) echo a ;;|", shell).unwrap(),
            "    a) echo a ;;\n    |"
        );
        assert_eq!(
            press_enter("    echo $(date)|", shell).unwrap(),
            "    echo $(date)\n    |"
        );
        assert_eq!(press_enter("    |", shell), None);
        assert_eq!(
            press_enter("\"list\": [|", Syntax::Json).unwrap(),
            "\"list\": [\n    |"
        );
    }

    #[test]
    fn enter_between_brackets_opens_the_block() {
        assert_eq!(
            press_enter("main() {|}", Syntax::Shell).unwrap(),
            "main() {\n    |\n}"
        );
        assert_eq!(
            press_enter("  \"a\": {|},", Syntax::Json).unwrap(),
            "  \"a\": {\n      |\n  },"
        );
    }

    #[test]
    fn enter_on_a_closing_line_lines_it_up_with_its_block() {
        let shell = Syntax::Shell;
        assert_eq!(
            press_enter("if x; then\n    echo\n    fi|", shell).unwrap(),
            "if x; then\n    echo\nfi\n|"
        );
        assert_eq!(
            press_enter("if x; then\n    echo\n    else|", shell).unwrap(),
            "if x; then\n    echo\nelse\n    |"
        );
        assert_eq!(
            press_enter("if x; then\n    fi|", shell).unwrap(),
            "if x; then\nfi\n|",
            "an empty block"
        );
        assert_eq!(
            press_enter(
                "case $1 in\n    a)\n        echo\n        ;;\n    esac|",
                shell
            )
            .unwrap(),
            "case $1 in\n    a)\n        echo\n        ;;\nesac\n|"
        );
        // Already lined up, or further out, it stays.
        assert_eq!(
            press_enter("for a; do\n    x\ndone|", shell).unwrap(),
            "for a; do\n    x\ndone\n|"
        );
        assert_eq!(
            press_enter("{\n    \"a\": 1\n    }|", Syntax::Json).unwrap(),
            "{\n    \"a\": 1\n}\n|"
        );
        // `fine` isn't `fi`.
        assert_eq!(
            press_enter("if x; then\n    fine|", shell).unwrap(),
            "if x; then\n    fine\n    |"
        );
    }
}
