//! What the assistant knows about the open document: the snapshot its tools read, the standing
//! instructions, and the notes sent with each question.
//!
//! The instructions change only when the document's kind or name does, so servers that cache
//! prompt prefixes can reuse them. What changes as the user types (the text and the selection)
//! goes with the newest question.

use std::ops::Range;
use std::path::Path;

use serde_json::Value;

use crate::ai::tools::{self, Suggestion};
use crate::brand::APP_NAME;
use crate::core::counts::Counts;
use crate::core::rewrite::Changes;
use crate::core::syntax::Syntax;
use crate::core::text_model::position;

/// A selection longer than this (in bytes) is not copied into the note; the model reads it with
/// `get_selection` if it needs it.
const NOTE_SELECTION_MAX: usize = 2_000;

/// A copy of what the user has open, taken when they ask. Tools read it, so the conversation
/// never waits on the editor.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub doc: DocKind,
    /// The document's text.
    pub text: String,
    /// The selected bytes of `text`, if any.
    pub selection: Option<Range<usize>>,
    /// Where the caret is in `text`, in bytes.
    pub caret: usize,
    /// The window can't change the text (another window has it, or it can't be saved), so no
    /// edit can be suggested.
    pub read_only: bool,
    /// While the user is rewriting: the original, which the document is a draft of.
    pub original: Option<String>,
    /// While rewriting: what the document changes from `original`, as the split lights it.
    pub changes: Changes,
    /// The suggestions still waiting for the user, in text order. A new suggestion over one
    /// replaces it.
    pub waiting: Vec<Waiting>,
    /// The parts of `text` that suggestions made in the reply being answered replace. A new
    /// suggestion may not overlap them.
    pub proposed: Vec<Range<usize>>,
    /// The waiting suggestion the caret is on, if it is on one.
    pub caret_on: Option<u64>,
    /// How the user decided suggestions since the model was last told, one `decided` line each,
    /// for the next question's note.
    pub decided: Vec<String>,
}

/// A suggestion waiting for the user: put `replace` in place of `range` of the snapshot's text.
#[derive(Debug, Clone)]
pub struct Waiting {
    /// What the model calls it, and `withdraw_edit` takes it back by. Later suggestions have
    /// bigger ones.
    pub id: u64,
    pub range: Range<usize>,
    pub replace: String,
}

impl Snapshot {
    /// The kind of text open, as the editor colours and checks it.
    pub fn syntax(&self) -> Syntax {
        match &self.doc {
            DocKind::Scratch { markdown: false } => Syntax::Plain,
            DocKind::Scratch { markdown: true } => Syntax::Markdown,
            DocKind::TextFile { name } => Syntax::of(Path::new(name), &self.text),
        }
    }
}

#[derive(Debug, Clone)]
pub enum DocKind {
    /// Scratch, written in Markdown when Settings says so.
    Scratch {
        markdown: bool,
    },
    TextFile {
        name: String,
    },
}

impl Default for DocKind {
    fn default() -> DocKind {
        DocKind::Scratch { markdown: false }
    }
}

/// The standing instructions: what the app is, the suggest-only rule, how to use the tools, and
/// what is open.
pub fn instructions(snapshot: &Snapshot) -> String {
    let mut text = format!(
        "You are the assistant built into {APP_NAME}, a text editor for writing and rewriting \
text (mostly prompts for AI models), and for scripts and config files. You talk with the user about \
whatever they have open and help them improve it.

How you work:
- You never change the document yourself. To suggest a change, call propose_edit. It shows in the \
document, marked, until the user accepts or rejects it, so say \"I suggested\", never \"I changed\". \
The next question's note says which they accepted and rejected; don't suggest a rejected edit again \
unless asked.
- Suggestions that wait for the user are listed in the note, numbered. A message sent while any \
wait is about them unless it is clearly about something else: \"shorter\" or \"use we\" means rework them. \
Call propose_edit again for the same text and the new one replaces the waiting one; \
withdraw_edit takes one back. When several wait, \"this\" means the one the caret is on, otherwise the \
newest (the highest number).
- The newest question comes with the text as it is now (a long text is cut short: get_document \
reads the rest by lines), its size, the selection and the caret. Copy the text you want to replace \
into propose_edit's find exactly, spaces and punctuation included. Leave find empty to add text at \
the end, such as in an empty document.
- One propose_edit per passage; several calls are fine. Keep the user's voice and formatting, \
including XML tags and {{{{macros}}}}, unless they ask otherwise.
- In a script or config file, keep its indentation, quoting and style exactly. An edit \
that would leave a JSON, YAML, TOML, INI or shell file invalid still shows, but propose_edit's \
result says what breaks, so you can correct it.
- When no edit is wanted, just answer. Keep replies short and plain: the chat pane is narrow.
- When the note says the user is rewriting, the document is their draft of an original text, which \
get_original reads by lines; get_changes lists what the draft changes from it. Suggest edits to \
the draft only.
- Word and token counts come from count_tokens (tokens are an estimate, about four bytes each).
"
    );
    text.push_str("\nWhat is open: ");
    text.push_str(&summary(snapshot));
    text
}

/// What goes with the newest question only, since it changes as the user types: the text, cut
/// to fit one tool result.
pub fn open_text(snapshot: &Snapshot) -> String {
    format!(
        "<document>\n{}\n</document>",
        tools::read_lines(&snapshot.text, &Value::Null)
    )
}

/// One paragraph on the open document, without anything that changes as the user types.
fn summary(snapshot: &Snapshot) -> String {
    match &snapshot.doc {
        DocKind::Scratch { markdown: false } => {
            "the scratch space, a quick notepad that isn't a named file.".to_string()
        }
        DocKind::Scratch { markdown: true } => {
            "the scratch space, a quick Markdown notepad that isn't a named file.".to_string()
        }
        DocKind::TextFile { name } => {
            let kind = match snapshot.syntax() {
                Syntax::Plain => "plain text file",
                Syntax::Markdown => "Markdown file",
                Syntax::Json => "JSON file",
                Syntax::Yaml => "YAML file",
                Syntax::Toml => "TOML file",
                Syntax::Ini => "INI config file",
                Syntax::Shell => "shell script",
                Syntax::SshConfig => "SSH config file",
            };
            format!("the {kind} \"{name}\".")
        }
    }
}

/// One suggestion the user decided, for the note: `outcome` (accepted, rejected, ...) and what
/// it would have replaced with what.
pub fn decided(suggestion: &Suggestion, outcome: &str) -> String {
    format!(
        "{outcome} \"{}\" → \"{}\"",
        tools::shorten(&suggestion.find, 60),
        tools::shorten(&suggestion.replace, 60)
    )
}

/// The note sent in front of a question: how the user decided the model's last suggestions, the
/// document's size now, the selection, and where the caret is.
pub fn turn_note(snapshot: &Snapshot) -> String {
    let text = &snapshot.text;
    let counts = Counts::of(text);
    let mut note = String::new();
    if !snapshot.decided.is_empty() {
        note.push_str(&format!(
            "[Since your last reply the user decided your suggested edits: {}. A rejected edit \
left the text as it was.]\n",
            snapshot.decided.join("; ")
        ));
    }
    if !snapshot.waiting.is_empty() {
        note.push_str("[Your suggestions still waiting for the user:\n");
        for waiting in &snapshot.waiting {
            let line = position(text, waiting.range.start).0 + 1;
            let find = text.get(waiting.range.clone()).unwrap_or("");
            let here = match snapshot.caret_on == Some(waiting.id) {
                true => " (the caret is on it)",
                false => "",
            };
            note.push_str(&format!(
                "{}. line {line}: \"{}\" → \"{}\"{here}\n",
                waiting.id,
                tools::shorten(find, 60),
                tools::shorten(&waiting.replace, 60)
            ));
        }
        note.push_str("]\n");
    }
    if snapshot.original.is_some() {
        note.push_str(
            "[The user is rewriting: the document is a draft of an original, which stays as it \
was. get_original reads it; get_changes lists what the draft changes.]\n",
        );
    }
    note.push_str(&format!(
        "[Document now: {} lines, {} words, about {} tokens",
        text.split('\n').count(),
        counts.words,
        counts.tokens
    ));
    let selected = snapshot
        .selection
        .clone()
        .and_then(|range| Some((range.clone(), text.get(range)?)))
        .filter(|(_, selected)| !selected.is_empty());
    match selected {
        None => {
            let line = position(text, snapshot.caret.min(text.len())).0 + 1;
            note.push_str(&format!(
                ". Nothing selected; the caret is on line {line}.]"
            ));
        }
        Some((range, selected)) => {
            let first = position(text, range.start).0 + 1;
            let last = position(text, range.end).0 + 1;
            let lines = match first == last {
                true => format!("line {first}"),
                false => format!("lines {first}-{last}"),
            };
            if selected.len() <= NOTE_SELECTION_MAX {
                note.push_str(&format!(
                    ". Selected, {lines}:]\n<selection>\n{selected}\n</selection>"
                ));
            } else {
                note.push_str(&format!(
                    ". Selected: {lines}, {} characters; get_selection reads it.]",
                    selected.chars().count()
                ));
            }
        }
    }
    note
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn scratch(text: &str) -> Snapshot {
        Snapshot {
            text: text.to_string(),
            ..Snapshot::default()
        }
    }

    #[test]
    fn instructions_name_the_document_and_stay_put_while_typing() {
        let before = instructions(&scratch("a"));
        assert!(before.contains(APP_NAME));
        assert!(before.contains("propose_edit"));
        assert!(before.contains("{{macros}}"));
        assert!(before.ends_with("the scratch space, a quick notepad that isn't a named file."));
        let typed = Snapshot {
            selection: Some(0..3),
            caret: 3,
            ..scratch("a lot more text")
        };
        assert_eq!(before, instructions(&typed));
        let markdown = Snapshot {
            doc: DocKind::Scratch { markdown: true },
            ..scratch("a")
        };
        assert!(
            instructions(&markdown).ends_with("a quick Markdown notepad that isn't a named file.")
        );

        let file = Snapshot {
            doc: DocKind::TextFile {
                name: "notes.txt".to_string(),
            },
            ..scratch("")
        };
        assert!(instructions(&file).ends_with("the plain text file \"notes.txt\"."));
        let script = Snapshot {
            doc: DocKind::TextFile {
                name: "deploy".to_string(),
            },
            ..scratch("#!/bin/bash\necho hi\n")
        };
        assert!(instructions(&script).ends_with("the shell script \"deploy\"."));
    }

    #[test]
    fn the_newest_question_brings_the_text() {
        assert_eq!(
            open_text(&scratch("one\ntwo")),
            "<document>\none\ntwo\n</document>"
        );
        // A long text is cut to one tool result's size, saying how to read on.
        let long = vec!["x".repeat(999); 100].join("\n");
        let cut = open_text(&scratch(&long));
        assert!(cut.contains("[More follows: ask for start_line 49 to read on.]"));
        assert!(cut.len() < 50_000);
    }

    #[test]
    fn the_note_says_how_suggestions_were_decided() {
        let suggestion = |find: &str, replace: &str| Suggestion {
            at: 0,
            find: find.to_string(),
            replace: replace.to_string(),
            why: String::new(),
        };
        let mut snapshot = scratch("Stay cold.");
        assert!(turn_note(&snapshot).starts_with("[Document now:"));
        snapshot.decided = vec![
            decided(&suggestion("cold", "warm"), "rejected"),
            decided(&suggestion("Stay", "Be"), "accepted"),
        ];
        let note = turn_note(&snapshot);
        assert!(note.starts_with(
            "[Since your last reply the user decided your suggested edits: rejected \"cold\" → \
\"warm\"; accepted \"Stay\" → \"Be\". A rejected edit left the text as it was.]\n[Document now:"
        ));
    }

    #[test]
    fn the_note_lists_the_suggestions_still_waiting() {
        let mut snapshot = scratch("Stay cold.\nBe brief.");
        assert!(!turn_note(&snapshot).contains("waiting"));
        snapshot.waiting = vec![
            Waiting {
                id: 4,
                range: 5..9,
                replace: "warm".to_string(),
            },
            Waiting {
                id: 6,
                range: 11..20,
                replace: "Be short.".to_string(),
            },
        ];
        snapshot.caret_on = Some(6);
        assert!(turn_note(&snapshot).starts_with(
            "[Your suggestions still waiting for the user:\n4. line 1: \"cold\" → \"warm\"\n\
6. line 2: \"Be brief.\" → \"Be short.\" (the caret is on it)\n]\n[Document now:"
        ));
    }

    #[test]
    fn the_note_says_when_the_document_is_a_rewrite() {
        let mut snapshot = scratch("Stay cold.");
        assert!(!turn_note(&snapshot).contains("rewriting"));
        snapshot.original = Some("Stay hot.".to_string());
        assert!(turn_note(&snapshot).starts_with("[The user is rewriting: "));
    }

    #[test]
    fn the_note_gives_sizes_the_selection_and_the_caret() {
        let note = turn_note(&scratch("one two\nthree"));
        assert!(note.starts_with("[Document now: 2 lines, 3 words, about 4 tokens. "));
        assert!(note.ends_with("tokens. Nothing selected; the caret is on line 1.]"));
        let lower = Snapshot {
            caret: 9,
            ..scratch("one two\nthree")
        };
        assert!(turn_note(&lower).ends_with("the caret is on line 2.]"));

        let text = "first line\nsecond ünïcode line\nthird";
        let start = text.find("second").unwrap();
        let selected = |text: &str, range: Range<usize>| Snapshot {
            selection: Some(range),
            ..scratch(text)
        };
        let note = turn_note(&selected(text, start..text.len()));
        assert!(note.ends_with(
            ". Selected, lines 2-3:]\n<selection>\nsecond ünïcode line\nthird\n</selection>"
        ));

        let long = "word ".repeat(1_000);
        let note = turn_note(&selected(&long, 0..long.len()));
        assert!(note.ends_with(". Selected: line 1, 5000 characters; get_selection reads it.]"));

        // A selection that isn't on character boundaries is ignored rather than trusted.
        let note = turn_note(&selected("ü", 0..1));
        assert!(note.ends_with("Nothing selected; the caret is on line 1.]"));
    }
}
