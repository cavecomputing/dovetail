//! Shell scripts (.sh, .bash, or a file with no extension whose `#!` line names sh or bash):
//! which parts of a line to colour, and whether `bash -n` finds a syntax error. Lines are coloured one at a time; a quoted string or a
//! here-document (`<<EOF`) can run on below the line that opens it, so each line also says what
//! the next one starts in.

use std::io::Write;
use std::ops::Range;
use std::process::{Command, Stdio};

use crate::core::syntax::{Found, Token};

/// What a line starts inside, from the lines above it.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellOpen {
    /// A '…' or "…" string, by its quote.
    Quote(u8),
    /// A here-document, up to the line that is just `word` (after any tabs, with `<<-`).
    /// `$names` in it are expanded unless the word was quoted (`<<'EOF'`).
    Heredoc {
        word: String,
        tabs: bool,
        expand: bool,
    },
}

/// Words that shape a script, coloured where a command could start.
const KEYWORDS: [&str; 25] = [
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do",
    "done", "function", "time", "!", "local", "export", "readonly", "declare", "return", "exit",
    "break", "continue", "coproc",
];

/// Whether `text` starts with a `#!` line that runs sh, bash or dash, directly
/// (`#!/bin/bash`) or through env (`#!/usr/bin/env bash`).
pub fn is_shell_script(text: &str) -> bool {
    let Some(shebang) = text.lines().next().and_then(|line| line.strip_prefix("#!")) else {
        return false;
    };
    let mut words = shebang.split_whitespace();
    let name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_string();
    let mut program = words.next().map(name);
    if program.as_deref() == Some("env") {
        // `env -S bash -e` and the like: the first word that isn't a flag.
        program = words.find(|word| !word.starts_with('-')).map(name);
    }
    matches!(program.as_deref(), Some("sh" | "bash" | "dash"))
}

/// The first syntax error `bash -n` finds in `text`, with its line, or `None` if there is none,
/// or bash can't be run. `-n` reads the script without running any of it.
pub fn shell_problem(text: &str) -> Option<String> {
    // On Windows `bash` is usually WSL's, which would start a whole Linux for each check, so
    // scripts aren't checked there. Linux and macOS have bash.
    if cfg!(windows) {
        return None;
    }
    let mut bash = Command::new("bash")
        .arg("-n")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    // bash stops reading at the first error, so a failed write is fine: the error is in stderr.
    let _ = bash.stdin.take()?.write_all(text.as_bytes());
    let output = bash.wait_with_output().ok()?;
    if output.status.success() {
        return None;
    }
    // "bash: line 4: syntax error near unexpected token `fi'"
    let errors = String::from_utf8_lossy(&output.stderr);
    let first = errors.lines().next()?;
    let error = first.strip_prefix("bash: ").unwrap_or(first);
    Some(
        match error.strip_prefix("line ").and_then(|e| e.split_once(": ")) {
            Some((line, reason)) => format!("Shell syntax error at line {line}: {reason}"),
            None => format!("Shell syntax error: {error}"),
        },
    )
}

/// Where a string whose opening `quote` is just before `from` ends: just after its closing
/// quote, or `None` if the line ends first. A backslash escapes the next character in "…".
fn quote_end(bytes: &[u8], from: usize, quote: u8) -> Option<usize> {
    let mut at = from;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' if quote == b'"' => at += 2,
            byte if byte == quote => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}

/// Where a variable starting with the `$` at `at` ends: `$name`, `${…}`, or one of `$1`, `$@`,
/// `$#`, `$?`, `$$`, `$!`, `$*`, `$-`. `None` for any other `$`, such as `$(…)`.
fn variable_end(bytes: &[u8], at: usize) -> Option<usize> {
    let next = *bytes.get(at + 1)?;
    match next {
        b'{' => Some(
            bytes[at..]
                .iter()
                .position(|&byte| byte == b'}')
                .map_or(bytes.len(), |close| at + close + 1),
        ),
        b'0'..=b'9' | b'@' | b'#' | b'?' | b'$' | b'!' | b'*' | b'-' => Some(at + 2),
        b'_' | b'a'..=b'z' | b'A'..=b'Z' => Some(
            bytes[at + 1..]
                .iter()
                .position(|&byte| !(byte == b'_' || byte.is_ascii_alphanumeric()))
                .map_or(bytes.len(), |end| at + 1 + end),
        ),
        _ => None,
    }
}

/// Every variable in `range` of the line, which "…" strings and here-documents expand.
fn variables(bytes: &[u8], range: Range<usize>, found: &mut Found) {
    let mut at = range.start;
    while at < range.end {
        match bytes[at] {
            b'\\' => at += 2,
            b'$' => match variable_end(bytes, at) {
                Some(end) => {
                    found.push((at..end.min(range.end), Token::Key));
                    at = end;
                }
                None => at += 1,
            },
            _ => at += 1,
        }
    }
}

/// A quoted string from `start` (its opening quote, or the start of the line when it carries on
/// from above) to `end` (or the end of the line), with the variables in a "…" string.
fn string(bytes: &[u8], start: usize, end: Option<usize>, quote: u8, found: &mut Found) {
    let end = end.unwrap_or(bytes.len());
    if start < end {
        found.push((start..end, Token::String));
    }
    if quote == b'"' {
        variables(bytes, start..end, found);
    }
}

/// Whether `byte` ends a word: a space, or one of the shell's operators and quotes.
fn ends_word(byte: u8) -> bool {
    byte.is_ascii_whitespace() || b";|&()<>\"'`$\\{}".contains(&byte)
}

/// The coloured parts of one line of a script: comments, strings, variables, keywords,
/// assignments (`NAME=`), function names, and here-documents. `open` is what the line starts
/// inside, from the line above; returns what the next line starts inside.
pub fn shell_ranges(line: &str, open: Option<ShellOpen>) -> (Found, Option<ShellOpen>) {
    let bytes = line.as_bytes();
    let mut found = Found::new();
    let mut at = 0;
    match open {
        Some(ShellOpen::Heredoc { word, tabs, expand }) => {
            let body = match tabs {
                true => line.trim_start_matches('\t'),
                false => line,
            };
            if body == word {
                found.push((0..line.len(), Token::Marker));
                return (found, None);
            }
            if !line.is_empty() {
                found.push((0..line.len(), Token::String));
            }
            if expand {
                variables(bytes, 0..line.len(), &mut found);
            }
            return (found, Some(ShellOpen::Heredoc { word, tabs, expand }));
        }
        Some(ShellOpen::Quote(quote)) => {
            let end = quote_end(bytes, 0, quote);
            string(bytes, 0, end, quote, &mut found);
            match end {
                Some(end) => at = end,
                None => return (found, Some(ShellOpen::Quote(quote))),
            }
        }
        None => {}
    }
    // Whether a command could start here, where keywords and assignments count.
    let mut command = at == 0;
    // The keyword that began the statement, for the `in` of `for x in` and `case $x in`.
    let mut statement = "";
    let mut heredoc = None;
    while at < bytes.len() {
        match bytes[at] {
            byte if byte.is_ascii_whitespace() => at += 1,
            b'#' if at == 0 || ends_word(bytes[at - 1]) => {
                found.push((at..bytes.len(), Token::Comment));
                break;
            }
            quote @ (b'\'' | b'"') => {
                let end = quote_end(bytes, at + 1, quote);
                string(bytes, at, end, quote, &mut found);
                match end {
                    Some(end) => at = end,
                    None => return (found, Some(ShellOpen::Quote(quote))),
                }
                command = false;
            }
            b'$' => match variable_end(bytes, at) {
                Some(end) => {
                    found.push((at..end, Token::Key));
                    at = end;
                    command = false;
                }
                None => at += 1,
            },
            // A backslash escapes the character after it, which may be more than one byte.
            b'\\' => at += 1 + line[at + 1..].chars().next().map_or(0, char::len_utf8),
            b'<' if line[at..].starts_with("<<") && !line[at..].starts_with("<<<") => {
                let start = at;
                at += 2;
                let tabs = bytes.get(at) == Some(&b'-');
                at += usize::from(tabs);
                while bytes.get(at).is_some_and(|byte| *byte == b' ') {
                    at += 1;
                }
                let quoted = matches!(bytes.get(at), Some(b'\'' | b'"'));
                at += usize::from(quoted);
                let word_start = at;
                while at < bytes.len() && !ends_word(bytes[at]) {
                    at += 1;
                }
                let word = line[word_start..at].to_string();
                at += usize::from(quoted && matches!(bytes.get(at), Some(b'\'' | b'"')));
                if !word.is_empty() {
                    found.push((start..at, Token::Marker));
                    heredoc.get_or_insert(ShellOpen::Heredoc {
                        word,
                        tabs,
                        expand: !quoted,
                    });
                }
            }
            b';' | b'|' | b'&' | b'(' | b')' | b'{' | b'}' | b'`' => {
                command = true;
                at += 1;
            }
            // A redirection (`>`, `>>`, `<`, `<<<`) is followed by a file name or a word.
            b'<' | b'>' => {
                command = false;
                while matches!(bytes.get(at), Some(b'<' | b'>')) {
                    at += 1;
                }
            }
            _ => {
                let start = at;
                while at < bytes.len() && !ends_word(bytes[at]) && bytes[at] != b'=' {
                    at += 1;
                }
                // A word with `=` in it, like `--opt=x`, is one word; `NAME=` is an assignment.
                let name = &line[start..at];
                let assignment = bytes.get(at) == Some(&b'=')
                    && name.starts_with(|c: char| c == '_' || c.is_ascii_alphabetic())
                    && name.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric());
                while at < bytes.len() && !ends_word(bytes[at]) {
                    at += 1;
                }
                let word = &line[start..at];
                let in_word = word == "in" && matches!(statement, "for" | "case" | "select");
                if command && KEYWORDS.contains(&word) || in_word {
                    found.push((start..at, Token::Literal));
                    if matches!(word, "for" | "case" | "select" | "function") {
                        statement = word;
                    }
                    // After `for`, `case` and `function` comes a name, not a command.
                    command = !matches!(word, "for" | "case" | "select" | "function" | "in");
                } else if command && assignment {
                    found.push((start..start + name.len(), Token::Key));
                    // `NAME=value cmd`: a command can still follow the assignment.
                } else if statement == "function"
                    || command && line[at..].trim_start().starts_with("()")
                {
                    found.push((start..at, Token::Key));
                    statement = "";
                    command = false;
                } else {
                    command = false;
                }
            }
        }
    }
    (found, heredoc)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The coloured text of `line`, with what each part is.
    fn parts(line: &str, open: Option<ShellOpen>) -> Vec<(&str, Token)> {
        let (found, _) = shell_ranges(line, open);
        found
            .into_iter()
            .map(|(range, token)| (&line[range], token))
            .collect()
    }

    #[test]
    fn a_shebang_naming_sh_or_bash_makes_a_shell_script() {
        assert!(is_shell_script("#!/bin/bash\necho hi"));
        assert!(is_shell_script("#!/bin/sh"));
        assert!(is_shell_script("#!/usr/bin/env bash\n"));
        assert!(is_shell_script("#! /usr/bin/env -S bash -e"));
        assert!(!is_shell_script("#!/usr/bin/env python3\n"));
        assert!(!is_shell_script("#!/usr/bin/zsh"));
        assert!(!is_shell_script("echo hi\n#!/bin/bash"));
        assert!(!is_shell_script(""));
    }

    #[test]
    fn keywords_variables_strings_and_comments() {
        use Token::*;
        assert_eq!(
            parts(r#"if [ -f "$file" ]; then # check"#, None),
            [
                ("if", Literal),
                (r#""$file""#, String),
                ("$file", Key),
                ("then", Literal),
                ("# check", Comment),
            ]
        );
        assert_eq!(
            parts("for f in *.txt; do echo ${f%.txt} $1; done", None),
            [
                ("for", Literal),
                ("in", Literal),
                ("do", Literal),
                ("${f%.txt}", Key),
                ("$1", Key),
                ("done", Literal),
            ]
        );
        // A keyword only counts where a command could start, and `#` only after a space.
        assert_eq!(parts("echo if done a#b 'x $y'", None), [("'x $y'", String)]);
    }

    #[test]
    fn assignments_and_function_names() {
        use Token::*;
        assert_eq!(
            parts("NAME=world local count=0", None),
            [("NAME", Key), ("local", Literal), ("count", Key)]
        );
        assert_eq!(parts("greet() {", None), [("greet", Key)]);
        assert_eq!(
            parts("function greet {", None),
            [("function", Literal), ("greet", Key)]
        );
        assert_eq!(parts("ls --color=auto", None), []);
    }

    #[test]
    fn a_string_runs_on_to_the_lines_below() {
        let (found, open) = shell_ranges(r#"echo "one"#, None);
        assert_eq!(found, [(5..9, Token::String)]);
        assert_eq!(open, Some(ShellOpen::Quote(b'"')));
        let (found, open) = shell_ranges(r#"two $x" done"#, open);
        assert_eq!(found, [(0..7, Token::String), (4..6, Token::Key)]);
        assert_eq!(open, None, "closed, and `done` isn't a command");
    }

    #[test]
    fn a_heredoc_runs_to_its_word() {
        use Token::*;
        let (found, open) = shell_ranges("cat <<-'EOF' > out", None);
        assert_eq!(found, [(4..12, Marker)]);
        let (found, open) = shell_ranges("  $not expanded", open);
        assert_eq!(found, [(0..15, String)]);
        let (found, open) = shell_ranges("\tEOF", open);
        assert_eq!((found, open), (vec![(0..4, Marker)], None));
        // Unquoted, its variables are expanded; without `-`, an indented word doesn't end it.
        let (_, open) = shell_ranges("cat <<EOF", None);
        assert_eq!(
            parts("hi $USER", open.clone()),
            [("hi $USER", String), ("$USER", Key)]
        );
        let (_, open) = shell_ranges("  EOF", open);
        assert!(open.is_some());
        assert_eq!(parts("x <<< 'y' >> log 2>&1 < in", None), [("'y'", String)]);
    }

    // `bash -n` runs only on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn bash_n_finds_the_first_syntax_error() {
        assert_eq!(shell_problem("if true; then\n  echo hi\nfi\n"), None);
        assert_eq!(
            shell_problem("if true; then\n  echo hi\nfi\nfi\n").as_deref(),
            Some("Shell syntax error at line 4: syntax error near unexpected token `fi'")
        );
        // macOS's bash 3.2 may count the line of an unclosed quote differently.
        let unclosed = shell_problem("echo \"open\n").expect("an unclosed quote");
        assert!(
            unclosed.starts_with("Shell syntax error at line "),
            "{unclosed}"
        );
        assert!(unclosed.ends_with("unexpected EOF while looking for matching `\"'"));
    }

    #[test]
    fn any_line_is_coloured_without_getting_stuck() {
        let lines = [
            "\\", "a\\é b", "$", "${", "<", ">", "<<", "<<-", "<<'", "x=", "=", "'", "\"", "é$é",
            "#", "()", "function", "ab\\", "${é",
        ];
        for line in lines {
            shell_ranges(line, None);
            shell_ranges(line, Some(ShellOpen::Quote(b'"')));
        }
    }
}
