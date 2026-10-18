//! Which kind of text a document is (plain, Markdown, JSON, YAML, TOML, INI or a shell script,
//! by its file's extension), the parts of a data file's line that get coloured, whether its
//! text is valid, and commenting lines out with Ctrl+/.

use std::ops::{Range, RangeInclusive};
use std::path::Path;

use crate::core::config::{ini_problem, toml_problem, yaml_problem};
use crate::core::json::json_problem;
use crate::core::markdown::is_markdown;
use crate::core::shell::{is_shell_script, shell_problem};
use crate::core::text_model::offset;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Syntax {
    /// Scratch, a .txt, or any other file.
    Plain,
    Markdown,
    Json,
    Yaml,
    Toml,
    Ini,
    /// A .sh or .bash file, or one with no extension whose `#!` line names sh or bash.
    Shell,
}

impl Syntax {
    /// The syntax of the file at `path`, by its extension in any case, or for a file with no
    /// extension by the `#!` line at the top of its `text`.
    pub fn of(path: &Path, text: &str) -> Syntax {
        if is_markdown(path) {
            return Syntax::Markdown;
        }
        let extension = path.extension().and_then(|extension| extension.to_str());
        match extension.map(str::to_ascii_lowercase).as_deref() {
            Some("json") => Syntax::Json,
            Some("yaml" | "yml") => Syntax::Yaml,
            Some("toml") => Syntax::Toml,
            Some("ini" | "cfg" | "conf") => Syntax::Ini,
            Some("sh" | "bash") => Syntax::Shell,
            None if is_shell_script(text) => Syntax::Shell,
            _ => Syntax::Plain,
        }
    }

    /// Why `text` isn't valid in this syntax, with where, or `None` if it is. Plain text and
    /// Markdown have nothing to check.
    pub fn problem(self, text: &str) -> Option<String> {
        match self {
            Syntax::Plain | Syntax::Markdown => None,
            Syntax::Json => json_problem(text),
            Syntax::Yaml => yaml_problem(text),
            Syntax::Toml => toml_problem(text),
            Syntax::Ini => ini_problem(text),
            Syntax::Shell => shell_problem(text),
        }
    }

    /// What starts a comment line, for Ctrl+/: `#` in scripts and config files. Plain text,
    /// Markdown and JSON have none.
    pub fn comment_marker(self) -> Option<&'static str> {
        match self {
            Syntax::Shell | Syntax::Yaml | Syntax::Toml | Syntax::Ini => Some("#"),
            Syntax::Plain | Syntax::Markdown | Syntax::Json => None,
        }
    }
}

/// Ctrl+/: the edits, in text order, that comment out `lines` of `text` by putting `marker` and a
/// space in at the smallest indent among them, or that take it (and one space after it) off when
/// every one of them already starts with it. Blank lines are left alone.
pub fn toggle_comments(
    text: &str,
    lines: RangeInclusive<usize>,
    marker: &str,
) -> Vec<(Range<usize>, String)> {
    // Each non-blank line as (where it starts, its indent, the rest).
    let lines: Vec<(usize, usize, &str)> = lines
        .map(|line| {
            let start = offset(text, line, 0);
            let content = text[start..].split('\n').next().unwrap_or_default();
            let rest = content.trim_start_matches(' ');
            (start, content.len() - rest.len(), rest)
        })
        .filter(|(_, _, rest)| !rest.trim().is_empty())
        .collect();
    if lines.iter().all(|(_, _, rest)| rest.starts_with(marker)) {
        return lines
            .into_iter()
            .map(|(start, indent, rest)| {
                let at = start + indent;
                let space = usize::from(rest[marker.len()..].starts_with(' '));
                (at..at + marker.len() + space, String::new())
            })
            .collect();
    }
    let indent = lines
        .iter()
        .map(|(_, indent, _)| *indent)
        .min()
        .unwrap_or(0);
    lines
        .into_iter()
        .map(|(start, _, _)| (start + indent..start + indent, format!("{marker} ")))
        .collect()
}

/// What a coloured part of a data file's line (JSON, YAML, TOML, INI) is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Token {
    /// A name a value is given under: a JSON or YAML key, a TOML or INI key.
    Key,
    String,
    Number,
    /// `true`, `false`, `null` and their kin.
    Literal,
    Comment,
    /// An INI `[section]` or a TOML `[table]`.
    Section,
    /// YAML's structure: `---`, a list's `-`, `|` and `>`, anchors, aliases and tags.
    Marker,
}

/// The 1-based line and column of byte `at` in `text`, as problems name them.
pub fn line_and_column(text: &str, at: usize) -> (usize, usize) {
    let before = &text[..at.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |start| start.chars().count())
        + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_syntax_comes_from_its_extension() {
        let syntax = |name| Syntax::of(Path::new(name), "");
        assert_eq!(syntax("notes.txt"), Syntax::Plain);
        assert_eq!(syntax("notes.MD"), Syntax::Markdown);
        assert_eq!(syntax("export.json"), Syntax::Json);
        assert_eq!(syntax("compose.yml"), Syntax::Yaml);
        assert_eq!(syntax("config.YAML"), Syntax::Yaml);
        assert_eq!(syntax("Cargo.toml"), Syntax::Toml);
        assert_eq!(syntax("php.ini"), Syntax::Ini);
        assert_eq!(syntax("setup.cfg"), Syntax::Ini);
        assert_eq!(syntax("toml"), Syntax::Plain);
        assert_eq!(syntax("deploy.sh"), Syntax::Shell);
        assert_eq!(syntax("init.BASH"), Syntax::Shell);
    }

    #[test]
    fn a_file_with_no_extension_can_be_a_shell_script_by_its_first_line() {
        let script = "#!/usr/bin/env bash\necho hi\n";
        assert_eq!(Syntax::of(Path::new("bin/deploy"), script), Syntax::Shell);
        assert_eq!(Syntax::of(Path::new("notes.txt"), script), Syntax::Plain);
        assert_eq!(
            Syntax::of(Path::new("bin/deploy"), "echo hi"),
            Syntax::Plain
        );
    }

    /// `text` with `toggle_comments` done on `lines`.
    fn toggled(text: &str, lines: RangeInclusive<usize>) -> String {
        let mut result = text.to_string();
        for (range, with) in toggle_comments(text, lines, "#").into_iter().rev() {
            result.replace_range(range, &with);
        }
        result
    }

    #[test]
    fn ctrl_slash_comments_lines_at_their_indent_and_back() {
        let text = "if x; then\n    echo a\n\n      echo b\nfi";
        let commented = toggled(text, 1..=3);
        assert_eq!(commented, "if x; then\n    # echo a\n\n    #   echo b\nfi");
        assert_eq!(toggled(&commented, 1..=3), text);
        // With some lines commented and some not, all are commented.
        assert_eq!(toggled("# a\nb", 0..=1), "# # a\n# b");
        // A comment without a space comes off too.
        assert_eq!(toggled("  #a", 0..=0), "  a");
        assert_eq!(toggled("\n\n", 0..=1), "\n\n");
        assert_eq!(Syntax::Json.comment_marker(), None);
        assert_eq!(Syntax::Yaml.comment_marker(), Some("#"));
    }

    #[test]
    fn lines_and_columns_count_from_one_in_characters() {
        let text = "ab\nçd\n";
        assert_eq!(line_and_column(text, 0), (1, 1));
        assert_eq!(line_and_column(text, 3), (2, 1));
        assert_eq!(line_and_column(text, 5), (2, 2), "after the two-byte ç");
        assert_eq!(line_and_column(text, 99), (3, 1));
    }
}
