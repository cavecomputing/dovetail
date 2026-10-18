//! What the assistant knows about the open document: the snapshot its tools read, the standing
//! instructions, and the notes sent with each question.
//!
//! The instructions change only when the document's kind or name does, so servers that cache
//! prompt prefixes can reuse them. What changes as the user types (the text and the selection)
//! goes with the newest question.

use std::ops::Range;

use serde_json::Value;

use crate::ai::tools::{self, Suggestion};
use crate::brand::APP_NAME;
use crate::core::counts::{Counts, word_count};
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
    /// The parts of `text` that suggestions still waiting for the user replace. A new suggestion
    /// may not overlap them.
    pub waiting: Vec<Range<usize>>,
}

#[derive(Debug, Clone, Default)]
pub enum DocKind {
    #[default]
    Scratch,
    TextFile {
        name: String,
    },
}

/// The standing instructions: what the app is, the suggest-only rule, how to use the tools, and
/// what is open.
pub fn instructions(snapshot: &Snapshot) -> String {
    let mut text = format!(
        "You are the writing assistant built into {APP_NAME}, a text editor for writing and \
rewriting text, mostly prompts for AI models. You talk with the user about whatever they have open \
and help them improve it.

How you work:
- You never change the document yourself. To suggest a change, call propose_edit. It shows in the \
document as ghost text that the user accepts or rejects, so say \"I suggested\", never \"I changed\".
- The newest question comes with the text as it is now (a long text is cut short: get_document \
reads the rest by lines), its size, the selection and the caret. Copy the text you want to replace \
into propose_edit's find exactly, spaces and punctuation included. Leave find empty to add text at \
the end, such as in an empty document.
- One propose_edit per passage; several calls are fine. Keep the user's voice and formatting, \
including XML tags and {{{{macros}}}}, unless they ask otherwise.
- When no edit is wanted, just answer. Keep replies short and plain: the chat pane is narrow.
- Word and token counts come from count_tokens (OpenAI's o200k tokenizer).
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
        DocKind::Scratch => {
            "the scratch space, a quick notepad that isn't a named file.".to_string()
        }
        DocKind::TextFile { name } => format!("the plain text file \"{name}\"."),
    }
}

/// The note sent in front of a question: the document's size now, the selection, and where the
/// caret is.
pub fn turn_note(snapshot: &Snapshot) -> String {
    let text = &snapshot.text;
    let counts = Counts::of(text);
    let mut note = format!(
        "[Document now: {} lines, {} words",
        text.split('\n').count(),
        counts.words
    );
    if let Some(tokens) = counts.tokens {
        note.push_str(&format!(", {tokens} tokens"));
    }
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

/// What the user is focused on, for the pane's focus chip: the selection, if any.
pub fn focus(text: &str, selection: Option<Range<usize>>) -> Option<String> {
    let words = word_count(&text[selection?]);
    let plural = if words == 1 { "word" } else { "words" };
    Some(format!("selection · {words} {plural}"))
}

/// What the pane's "Try shorter" asks for.
pub fn shorter_request(suggestion: &Suggestion) -> String {
    format!(
        "Try a shorter version of your suggestion to replace \"{}\" with \"{}\".",
        suggestion.find, suggestion.replace
    )
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

        let file = Snapshot {
            doc: DocKind::TextFile {
                name: "notes.txt".to_string(),
            },
            ..scratch("")
        };
        assert!(instructions(&file).ends_with("the plain text file \"notes.txt\"."));
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
    fn the_note_gives_sizes_the_selection_and_the_caret() {
        crate::core::counts::load_tokenizer();
        let note = turn_note(&scratch("one two\nthree"));
        assert!(note.starts_with("[Document now: 2 lines, 3 words, "));
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

    #[test]
    fn focus_is_the_selection() {
        let text = "Let tension build. Act.";
        assert_eq!(focus(text, None), None);
        assert_eq!(
            focus(text, Some(0..11)).as_deref(),
            Some("selection · 2 words")
        );
        assert_eq!(
            focus(text, Some(19..23)).as_deref(),
            Some("selection · 1 word")
        );
    }
}
