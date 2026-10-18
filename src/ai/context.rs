//! What the assistant knows about the open document: the snapshot its tools read, the standing
//! instructions, and the notes sent with each question.
//!
//! The instructions change only when the document's kind or name, the version on the left or the
//! ideas do, so servers that cache prompt prefixes can reuse them. In a project they hold the
//! version on the left and the ideas with their feeling notes. What changes as the user types (the
//! text being edited, the weights, the selection) goes with the newest question.

use std::ops::Range;

use serde_json::Value;

use crate::ai::tools::{self, Suggestion};
use crate::brand::APP_NAME;
use crate::core::counts::{Counts, word_count};
use crate::core::text_model::{TextModel, position};
use crate::project::Project;
use crate::project::manifest::Manifest;
use crate::project::paint::Span;

/// A selection longer than this (in bytes) is not copied into the note; the model reads it with
/// `get_selection` if it needs it.
const NOTE_SELECTION_MAX: usize = 2_000;

/// A copy of what the user has open, taken when they ask. Tools read it, so the conversation
/// never waits on the editor.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub doc: DocKind,
    /// The text being edited: the whole document, or a project's draft.
    pub text: String,
    /// The selected bytes of `text`, if any.
    pub selection: Option<Range<usize>>,
    /// Where the caret is in `text`, in bytes.
    pub caret: usize,
    /// What the pane's focus chip shows (see `focus`).
    pub focus: Option<String>,
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
    Project(Box<ProjectSnapshot>),
}

/// What a project adds to the snapshot.
#[derive(Debug, Clone)]
pub struct ProjectSnapshot {
    pub project: Project,
    /// The draft's paint, as the editor has it now (byte offsets into `Snapshot::text`).
    pub draft_paint: Vec<Span>,
    /// Every committed version as (number, text), oldest first. See `read_versions`.
    pub versions: Vec<(u32, String)>,
    /// The version in the left pane, which weights compare against unless asked otherwise.
    pub shown_version: u32,
}

impl ProjectSnapshot {
    pub fn version_text(&self, number: u32) -> Option<&str> {
        self.versions
            .iter()
            .find(|(version, _)| *version == number)
            .map(|(_, text)| text.as_str())
    }
}

/// Every committed version's text, for `ProjectSnapshot::versions`. Reads git, so call it off the
/// UI thread. A committed text never changes, so the result can be kept and extended after a
/// commit.
pub fn read_versions(project: &Project) -> anyhow::Result<Vec<(u32, String)>> {
    let mut versions = Vec::new();
    for version in &project.manifest.versions {
        versions.push((version.number, project.version_text(version.number)?));
    }
    Ok(versions)
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
    if let DocKind::Project(_) = snapshot.doc {
        text.push_str(
            "- This is a project. The user rewrites the text across versions (v1, v2, ...); the \
draft becomes the next version when they commit it. They paint stretches of text with ideas, named \
concepts with a colour and maybe a topic, to see which meanings survive a rewrite. Feeling notes say \
how an idea should feel. The version on the left and the ideas are below; the newest question \
comes with the draft and the weights. Use list_versions, get_version, list_ideas (which also quotes \
the passages painted with each idea in the draft) and get_idea_weights for more, such as which \
idea shrank most since another version.
- When an edit carries an idea, name it in propose_edit's idea so the accepted text takes that \
idea's colour. A question's focus is the active brush's idea, or the topic or idea painted at the \
caret.
",
        );
    }
    text.push_str("\nWhat is open: ");
    text.push_str(&summary(snapshot));
    if let DocKind::Project(project) = &snapshot.doc {
        let shown = project.shown_version;
        if let Some(version) = project.version_text(shown) {
            let version = tools::read_lines(version, &Value::Null);
            text.push_str(&format!("\n\n<v{shown}>\n{version}\n</v{shown}>"));
        }
        text.push_str(&format!(
            "\n\nThe ideas, with the passages painted with each in v{shown}:\n{}",
            Value::Array(tools::idea_rows(project, None))
        ));
    }
    text
}

/// What goes with the newest question only, since it changes as the user types: the text being
/// edited, cut to fit one tool result, and in a project the weights against the version on the
/// left.
pub fn open_text(snapshot: &Snapshot) -> String {
    let tag = match snapshot.doc {
        DocKind::Project(_) => "draft",
        _ => "document",
    };
    let mut text = format!(
        "<{tag}>\n{}\n</{tag}>",
        tools::read_lines(&snapshot.text, &Value::Null)
    );
    if let DocKind::Project(project) = &snapshot.doc
        && let Some(old_text) = project.version_text(project.shown_version)
    {
        let weights = tools::idea_weights(project, project.shown_version, old_text, &snapshot.text);
        text.push_str(&format!("\nIdea weights: {weights}"));
    }
    text
}

/// One paragraph on the open document, without anything that changes as the user types.
fn summary(snapshot: &Snapshot) -> String {
    let project = match &snapshot.doc {
        DocKind::Scratch => {
            return "the scratch space, a quick notepad that isn't a named file.".to_string();
        }
        DocKind::TextFile { name } => return format!("the plain text file \"{name}\"."),
        DocKind::Project(project) => project,
    };
    let manifest = &project.project.manifest;
    let versions = match manifest.latest_version() {
        0 => "no versions committed yet".to_string(),
        1 => "v1 committed".to_string(),
        latest => format!("v1 to v{latest} committed"),
    };
    format!(
        "the project \"{}\": the draft that becomes v{}, with {versions}. The left pane shows v{}.",
        manifest.name,
        manifest.draft_version(),
        project.shown_version
    )
}

/// The note sent in front of a question: the document's size now, the selection, and where the
/// caret and focus are.
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
            note.push_str(&format!(". Nothing selected; the caret is on line {line}."));
            if let Some(focus) = &snapshot.focus {
                note.push_str(&format!(" Focus: {focus}."));
            }
            note.push(']');
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

/// What the user is focused on, for the pane's focus chip and the note on each question: the
/// selection, or in a project the brush's idea, or the topic (else the idea) of the paint at the
/// caret.
pub fn focus(
    model: &TextModel,
    selection: Option<Range<usize>>,
    caret: usize,
    manifest: Option<&Manifest>,
) -> Option<String> {
    if let Some(range) = selection {
        let words = word_count(&model.text()[range]);
        let plural = if words == 1 { "word" } else { "words" };
        return Some(format!("selection · {words} {plural}"));
    }
    let manifest = manifest?;
    if let Some(brush) = model.brush {
        return Some(manifest.idea(brush)?.name.clone());
    }
    let span = model
        .paint
        .iter()
        .find(|span| span.start <= caret && caret <= span.end)?;
    let idea = manifest.idea(span.idea)?;
    let topic = idea
        .topic
        .and_then(|topic| manifest.topics.iter().find(|t| t.id == topic));
    Some(topic.map_or(&idea.name, |topic| &topic.name).clone())
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
    use crate::project::manifest::{Manifest, Version};
    use std::path::PathBuf;

    pub(crate) fn project_snapshot(text: &str) -> Snapshot {
        let mut manifest = Manifest::new("Nyx");
        for number in 1..=3 {
            manifest.versions.push(Version {
                number,
                note: format!("note {number}"),
                committed_at: 1_760_270_400,
                paint: Vec::new(),
            });
        }
        let pacing = manifest.add_topic("Pacing");
        let slow = manifest.add_idea("Slow burn");
        manifest.set_topic(slow, Some(pacing));
        manifest.add_idea("Agency");
        Snapshot {
            doc: DocKind::Project(Box::new(ProjectSnapshot {
                project: Project {
                    dir: PathBuf::from("/nowhere"),
                    manifest,
                },
                draft_paint: Vec::new(),
                versions: vec![(1, "one".into()), (2, "two".into()), (3, "three".into())],
                shown_version: 3,
            })),
            ..scratch(text)
        }
    }

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
        assert!(!before.contains("list_ideas"));
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
    fn project_instructions_hold_the_version_on_the_left_and_the_ideas() {
        let text = instructions(&project_snapshot("draft"));
        assert!(text.contains("list_ideas"));
        assert!(text.contains(
            "the project \"Nyx\": the draft that becomes v4, with v1 to v3 committed. The left \
             pane shows v3.\n\n<v3>\nthree\n</v3>\n\nThe ideas, with the passages painted with \
             each in v3:\n["
        ));
        assert!(text.contains(r#""name":"Slow burn""#));
        assert!(text.contains(r#""topic":"Pacing""#));
        assert!(!text.contains("status"), "that changes as the draft does");
        assert_eq!(text, instructions(&project_snapshot("a longer draft")));
    }

    #[test]
    fn the_newest_question_brings_the_text_and_the_weights() {
        crate::core::counts::load_tokenizer();
        assert_eq!(
            open_text(&scratch("one\ntwo")),
            "<document>\none\ntwo\n</document>"
        );
        let draft = open_text(&project_snapshot("Let tension build."));
        assert!(draft.starts_with("<draft>\nLet tension build.\n</draft>\nIdea weights: {"));
        assert!(draft.contains(r#""compared":"v3 -> draft""#));
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
        let focused = Snapshot {
            caret: 9,
            focus: Some("Agency".to_string()),
            ..scratch("one two\nthree")
        };
        assert!(turn_note(&focused).ends_with("the caret is on line 2. Focus: Agency.]"));

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
    fn focus_is_the_selection_the_brush_or_the_paint_at_the_caret() {
        let snapshot = project_snapshot("");
        let DocKind::Project(project) = &snapshot.doc else {
            unreachable!()
        };
        let manifest = &project.project.manifest;
        let mut ideas = manifest.live_ideas().map(|idea| idea.id);
        let (slow_burn, agency) = (ideas.next().unwrap(), ideas.next().unwrap());
        let mut model = TextModel::new("Let tension build. Act.".to_string());
        model.paint = vec![
            Span {
                idea: slow_burn,
                start: 0,
                end: 18,
                quote: "Let tension build.".to_string(),
            },
            Span {
                idea: agency,
                start: 19,
                end: 23,
                quote: "Act.".to_string(),
            },
        ];
        // Slow burn has a topic, which names it; Agency has none.
        assert_eq!(
            focus(&model, None, 4, Some(manifest)).as_deref(),
            Some("Pacing")
        );
        assert_eq!(
            focus(&model, None, 20, Some(manifest)).as_deref(),
            Some("Agency")
        );
        assert_eq!(focus(&model, None, 4, None), None);
        model.brush = Some(agency);
        assert_eq!(
            focus(&model, None, 4, Some(manifest)).as_deref(),
            Some("Agency")
        );
        assert_eq!(
            focus(&model, Some(0..11), 4, None).as_deref(),
            Some("selection · 2 words")
        );
    }
}
