//! Which kind of text a document is (plain, Markdown, JSON, YAML, TOML, INI or a shell script,
//! by its file's extension), the parts of a data file's line that get coloured, and whether its
//! text is valid.

use std::path::Path;

use crate::core::config::{ini_problem, toml_problem, yaml_problem};
use crate::core::json::json_problem;
use crate::core::markdown::is_markdown;
use crate::core::shell::is_shell_script;

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
            Syntax::Plain | Syntax::Markdown | Syntax::Shell => None,
            Syntax::Json => json_problem(text),
            Syntax::Yaml => yaml_problem(text),
            Syntax::Toml => toml_problem(text),
            Syntax::Ini => ini_problem(text),
        }
    }
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

    #[test]
    fn lines_and_columns_count_from_one_in_characters() {
        let text = "ab\nçd\n";
        assert_eq!(line_and_column(text, 0), (1, 1));
        assert_eq!(line_and_column(text, 3), (2, 1));
        assert_eq!(line_and_column(text, 5), (2, 2), "after the two-byte ç");
        assert_eq!(line_and_column(text, 99), (3, 1));
    }
}
