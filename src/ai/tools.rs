//! The tools the assistant can call, and running them against a `Snapshot` of the open document.
//! Every tool only reads; `propose_edit` records a suggestion for the user to accept or reject.
//! Results are plain text or JSON, written for the model.

use serde_json::{Value, json};

use crate::ai::client::ToolCall;
use crate::ai::context::{DocKind, ProjectSnapshot, Snapshot};
use crate::core::counts::{Counts, percent_change};
use crate::core::review::clashes;
use crate::core::text_model::position;
use crate::project::ideas::COLOUR_SLOTS;
use crate::project::paint::{IdeaId, Span};
use crate::project::weights::{self, Status, Weight, Weights};

/// The most text one result hands back (about 12,000 tokens). Longer texts are read by lines.
const MAX_RESULT_TEXT: usize = 48_000;
/// The most lines `find_text` lists.
const MAX_MATCHES: usize = 50;
/// The longest line `find_text` shows, in characters.
const MAX_LINE_CHARS: usize = 400;
/// The longest painted passage the idea list quotes, in characters.
const MAX_QUOTE_CHARS: usize = 200;
/// How many characters of painted passages one idea list quotes in all.
const MAX_QUOTED_CHARS: usize = 8_000;
/// What `propose_edit` answers when the suggestion is shown.
const SUGGESTED: &str = "Suggested. The user sees it in the document and will accept or reject it.";
/// Names for the idea colour slots, in the order `ideas::COLOUR_SLOTS` describes.
const COLOUR_NAMES: [&str; COLOUR_SLOTS] = [
    "aqua", "blue", "purple", "yellow", "orange", "green", "grey",
];

/// A tool the model may call, with a JSON schema of its arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

/// An edit the model proposed: replace `find` with `replace`. It is shown as ghost text until
/// the user accepts or rejects it, and never enters the document's text before that.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// Where `find` starts in the snapshot's text, in bytes.
    pub at: usize,
    /// The text it replaces; empty to add `replace` at `at`, the end of the text.
    pub find: String,
    pub replace: String,
    /// The model's reason, for the suggestion card.
    pub why: String,
    /// The idea the new text carries in a project: accepting paints it with this idea.
    pub idea: Option<IdeaId>,
}

/// The tools on offer for this kind of document. The project tools appear only in projects.
pub fn catalogue(snapshot: &Snapshot) -> Vec<Tool> {
    let in_project = matches!(snapshot.doc, DocKind::Project(_));
    let line_range = json!({
        "start_line": { "type": "integer", "description": "First line to read, counting from 1." },
        "end_line": { "type": "integer", "description": "Last line to read, included." },
    });
    let mut edit_arguments = json!({
        "find": {
            "type": "string",
            "description": "Text copied exactly from the document, long enough to occur only \
                once. Empty to add the new text at the end of the document.",
        },
        "replace": { "type": "string", "description": "The new text to put in its place." },
        "why": { "type": "string", "description": "One short sentence on why, shown to the user." },
    });
    if in_project {
        edit_arguments["idea"] = json!({
            "type": "string",
            "description": "The name of the idea the new text carries, if any.",
        });
    }
    let mut tools = vec![
        Tool {
            name: "get_document",
            description: "Read the document (in a project, the draft). Returns the whole text, \
                or the lines asked for. A long document comes back in parts: ask for the rest \
                with start_line and end_line.",
            parameters: json!({ "type": "object", "properties": line_range }),
        },
        Tool {
            name: "get_selection",
            description: "Read the text the user has selected, with its line numbers.",
            parameters: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "find_text",
            description: "Find every line of the document containing some text, ignoring upper \
                and lower case. Returns each line's number and exact text.",
            parameters: json!({
                "type": "object",
                "properties": { "text": { "type": "string", "description": "What to look for." } },
                "required": ["text"],
            }),
        },
        Tool {
            name: "count_tokens",
            description: "Count the words and tokens (OpenAI's o200k tokenizer) of some text, or \
                of the whole document when no text is given.",
            parameters: json!({
                "type": "object",
                "properties": { "text": { "type": "string", "description": "The text to count." } },
            }),
        },
        Tool {
            name: "propose_edit",
            description: "Suggest replacing text in the document, or adding text at its end. The \
                user sees it as ghost text and accepts or rejects it; nothing changes until they \
                accept. Use one call per passage.",
            parameters: json!({
                "type": "object",
                "properties": edit_arguments,
                "required": ["find", "replace", "why"],
            }),
        },
    ];
    if in_project {
        tools.extend([
            Tool {
                name: "get_version",
                description: "Read a committed version's text, or some of its lines.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "number": { "type": "integer", "description": "The version, such as 3 for v3." },
                        "start_line": line_range["start_line"],
                        "end_line": line_range["end_line"],
                    },
                    "required": ["number"],
                }),
            },
            Tool {
                name: "list_versions",
                description: "List the committed versions (number, note, date, size) and the draft.",
                parameters: json!({ "type": "object", "properties": {} }),
            },
            Tool {
                name: "list_ideas",
                description: "List the ideas: name, colour, topic, feeling notes, the versions \
                    each was cut from on purpose, the passages painted with it in the version \
                    shown on the left and in the draft, and its status in the draft against that \
                    version (carried, missing, cut on purpose or unused).",
                parameters: json!({ "type": "object", "properties": {} }),
            },
            Tool {
                name: "get_idea_weights",
                description: "Each idea's and topic's share of the text, a version against the \
                    draft: percent and words/tokens before and after, plus unpainted text and \
                    the totals.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "since": {
                            "type": "integer",
                            "description": "The version to compare the draft with. Default: the one shown on the left.",
                        },
                    },
                }),
            },
        ]);
    }
    tools
}

/// Runs `call` against `snapshot` and returns what to send back to the model, plus the new
/// suggestion if it was a successful `propose_edit`.
pub fn run(call: &ToolCall, snapshot: &Snapshot) -> (String, Option<Suggestion>) {
    let arguments = match call.arguments.trim() {
        "" => json!({}),
        text => match serde_json::from_str::<Value>(text) {
            Ok(arguments) if arguments.is_object() => arguments,
            _ => return ("The arguments weren't a JSON object.".to_string(), None),
        },
    };
    let project = match &snapshot.doc {
        DocKind::Project(project) => Some(project.as_ref()),
        _ => None,
    };
    let text = &snapshot.text;
    let output = match (call.name.as_str(), project) {
        ("get_document", _) => read_lines(text, &arguments),
        ("get_selection", _) => selection(snapshot),
        ("find_text", _) => find_text(text, arguments["text"].as_str().unwrap_or("")),
        ("count_tokens", _) => count(arguments["text"].as_str().unwrap_or(text)).to_string(),
        ("propose_edit", _) => return propose_edit(snapshot, project, &arguments),
        ("get_version", Some(project)) => match number(&arguments["number"]) {
            Some(number) => match project.version_text(number) {
                Some(version) => read_lines(version, &arguments),
                None => no_such_version(project, number),
            },
            None => "Say which version, as a number.".to_string(),
        },
        ("list_versions", Some(project)) => list_versions(project, text),
        ("list_ideas", Some(project)) => {
            let ideas = idea_rows(project, Some(text));
            json!({ "compared_with": format!("v{}", project.shown_version), "ideas": ideas })
                .to_string()
        }
        ("get_idea_weights", Some(project)) => {
            let since = number(&arguments["since"]).unwrap_or(project.shown_version);
            match project.version_text(since) {
                Some(old_text) => idea_weights(project, since, old_text, text),
                None => no_such_version(project, since),
            }
        }
        (name, _) => format!("There is no tool called {name} here."),
    };
    (output, None)
}

/// What the chat shows for a tool call that gave back `output`, such as "Searched for "warm"".
pub fn describe(call: &ToolCall, output: &str) -> String {
    let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
    match call.name.as_str() {
        "get_document" => "Read the document".to_string(),
        "get_selection" => "Read the selection".to_string(),
        "find_text" => format!(
            "Searched for \"{}\"",
            arguments["text"].as_str().unwrap_or("")
        ),
        "count_tokens" => "Counted tokens".to_string(),
        "propose_edit" if output == SUGGESTED => "Suggested an edit".to_string(),
        "propose_edit" => "Couldn't suggest an edit".to_string(),
        "get_version" => match number(&arguments["number"]) {
            Some(number) => format!("Read v{number}"),
            None => "Read a version".to_string(),
        },
        "list_versions" => "Looked at the versions".to_string(),
        "list_ideas" => "Looked at the ideas".to_string(),
        "get_idea_weights" => "Weighed the ideas".to_string(),
        name => format!("Used {name}"),
    }
}

/// A whole number from the model, which sometimes writes 3 as 3.0 or "3".
fn number(value: &Value) -> Option<u32> {
    let number = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.trim().trim_start_matches('v').parse().ok()?,
        _ => return None,
    };
    (number >= 0.0 && number <= u32::MAX as f64).then_some(number as u32)
}

/// `text`, or the lines `start_line..=end_line` of it, cut to fit one result with a note saying
/// how to read on.
pub fn read_lines(text: &str, arguments: &Value) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let start = number(&arguments["start_line"]);
    let end = number(&arguments["end_line"]);
    if start.is_none() && end.is_none() && text.len() <= MAX_RESULT_TEXT {
        return match text.is_empty() {
            true => "(The text is empty.)".to_string(),
            false => text.to_string(),
        };
    }
    let first = start.unwrap_or(1).max(1) as usize;
    let last = (end.map_or(lines.len(), |end| end as usize)).min(lines.len());
    if first > last {
        return format!("There are {} lines; nothing in that range.", lines.len());
    }
    // Always at least one line (cut short if it alone is too long), then as many more as fit.
    let mut shown = fitted(lines[first - 1]).to_string();
    let mut upto = first;
    for line in &lines[first..last] {
        if shown.len() + 1 + line.len() > MAX_RESULT_TEXT {
            break;
        }
        shown.push('\n');
        shown.push_str(line);
        upto += 1;
    }
    let mut result = format!("[Lines {first}-{upto} of {}]\n{shown}", lines.len());
    if shown.len() < lines[first - 1].len() {
        result.push_str(&format!(
            "\n[Line {first} is cut short: it is too long for one result.]"
        ));
    }
    if upto < last {
        result.push_str(&format!(
            "\n[More follows: ask for start_line {} to read on.]",
            upto + 1
        ));
    }
    result
}

/// The first `MAX_RESULT_TEXT` bytes of `text`, or less to end on a whole character.
fn fitted(text: &str) -> &str {
    &text[..text.floor_char_boundary(MAX_RESULT_TEXT)]
}

/// `text` cut to `chars` characters, with "…" when it was longer.
pub fn shorten(text: &str, chars: usize) -> String {
    let mut short: String = text.chars().take(chars).collect();
    if short.len() < text.len() {
        short.push('…');
    }
    short
}

fn selection(snapshot: &Snapshot) -> String {
    let text = &snapshot.text;
    let range = snapshot.selection.clone().unwrap_or(0..0);
    match text.get(range.clone()) {
        Some(selected) if !selected.is_empty() => {
            let first = position(text, range.start).0 + 1;
            let last = position(text, range.end).0 + 1;
            let shown = fitted(selected);
            let mut result = format!("[Selected, lines {first}-{last}]\n{shown}");
            if shown.len() < selected.len() {
                result.push_str("\n[The selection is cut short: read the rest with get_document.]");
            }
            result
        }
        _ => "Nothing is selected.".to_string(),
    }
}

/// Every line holding `query` (ignoring case), with its number and exact text.
fn find_text(text: &str, query: &str) -> String {
    if query.is_empty() {
        return "Say what text to find.".to_string();
    }
    // Lowercasing can change byte lengths but never adds or removes a line break, so counting
    // breaks in the lowercased text still gives the right line.
    let lowered = text.to_lowercase();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut matches: Vec<Value> = Vec::new();
    let mut total = 0;
    // The line of the last match, counted on from the last match rather than from the start.
    let mut line = 1;
    let mut counted_to = 0;
    let mut last_line = 0;
    for (at, _) in lowered.match_indices(&query.to_lowercase()) {
        total += 1;
        if matches.len() == MAX_MATCHES {
            continue;
        }
        line += lowered[counted_to..at].matches('\n').count();
        counted_to = at;
        if line == last_line {
            continue;
        }
        last_line = line;
        let shown = shorten(lines[line - 1], MAX_LINE_CHARS);
        matches.push(json!({ "line": line, "text": shown }));
    }
    if total == 0 {
        return format!("No matches for \"{query}\".");
    }
    json!({ "matches": total, "lines": matches }).to_string()
}

/// Words and tokens; tokens are null while the tokenizer is still loading.
fn count(text: &str) -> Value {
    let counts = Counts::of(text);
    match counts.tokens {
        Some(tokens) => json!({ "words": counts.words, "tokens": tokens }),
        None => json!({
            "words": counts.words,
            "tokens": null,
            "note": "The tokenizer is still loading; try again in a moment.",
        }),
    }
}

fn propose_edit(
    snapshot: &Snapshot,
    project: Option<&ProjectSnapshot>,
    arguments: &Value,
) -> (String, Option<Suggestion>) {
    let refuse = |reason: String| (reason, None);
    let text = &snapshot.text;
    let find = arguments["find"].as_str().unwrap_or("");
    let replace = arguments["replace"].as_str().unwrap_or("");
    if snapshot.read_only {
        let reason = "This window is read-only, so describe the change in words instead.";
        return refuse(reason.to_string());
    }
    if find == replace {
        return refuse("replace is the same as find, so that changes nothing.".to_string());
    }
    let found: Vec<usize> = match find.is_empty() {
        // An empty find adds the new text at the end.
        true => vec![text.len()],
        false => text.match_indices(find).map(|(at, _)| at).collect(),
    };
    let at = match found.as_slice() {
        [] => {
            return refuse(
                "Not found. find must match the document exactly, including spaces, line \
                 breaks and punctuation. Read it with get_document or find_text and copy the \
                 text."
                    .to_string(),
            );
        }
        [at] => *at,
        _ => {
            return refuse(format!(
                "That text occurs {} times. Include more of the text around it so it occurs once.",
                found.len()
            ));
        }
    };
    let range = at..at + find.len();
    if snapshot.waiting.iter().any(|other| clashes(other, &range)) {
        return refuse(
            "That overlaps another suggestion still waiting for the user. Change different \
             text, or wait for them to accept or reject it."
                .to_string(),
        );
    }
    let idea_name = arguments["idea"].as_str().map(str::trim).unwrap_or("");
    let idea = match project {
        Some(project) if !idea_name.is_empty() => {
            let manifest = &project.project.manifest;
            let found = manifest
                .live_ideas()
                .find(|idea| idea.name.trim().eq_ignore_ascii_case(idea_name));
            match found {
                Some(idea) => Some(idea.id),
                None => {
                    let names: Vec<&str> = manifest
                        .live_ideas()
                        .map(|idea| idea.name.as_str())
                        .collect();
                    return refuse(format!(
                        "There is no idea called \"{idea_name}\". The ideas are: {}. Leave idea \
                         out if none fits.",
                        names.join(", ")
                    ));
                }
            }
        }
        _ => None,
    };
    let suggestion = Suggestion {
        at,
        find: find.to_string(),
        replace: replace.to_string(),
        why: arguments["why"].as_str().unwrap_or("").to_string(),
        idea,
    };
    (SUGGESTED.to_string(), Some(suggestion))
}

fn no_such_version(project: &ProjectSnapshot, number: u32) -> String {
    match project.project.manifest.latest_version() {
        0 => format!("There is no v{number}: nothing has been committed yet."),
        latest => format!("There is no v{number}. The versions are v1 to v{latest}."),
    }
}

fn list_versions(project: &ProjectSnapshot, draft: &str) -> String {
    let manifest = &project.project.manifest;
    let mut rows: Vec<Value> = manifest
        .versions
        .iter()
        .map(|version| {
            let counts = Counts::of(project.version_text(version.number).unwrap_or(""));
            json!({
                "version": format!("v{}", version.number),
                "note": version.note,
                "committed": version.committed_on(),
                "words": counts.words,
                "tokens": counts.tokens,
            })
        })
        .collect();
    let counts = Counts::of(draft);
    rows.push(json!({
        "version": format!("draft (becomes v{})", manifest.draft_version()),
        "words": counts.words,
        "tokens": counts.tokens,
    }));
    Value::Array(rows).to_string()
}

/// One row per idea: name, colour, topic, feeling notes, cut marks, and the passages painted with
/// it in the version shown on the left. With the `draft`, also its passages there and its status
/// against that version, the way the weights view sees it.
pub fn idea_rows(project: &ProjectSnapshot, draft: Option<&str>) -> Vec<Value> {
    let manifest = &project.project.manifest;
    let since = project.shown_version;
    let old_text = project.version_text(since).unwrap_or("");
    let old_paint = project.project.version_paint(since, old_text);
    let weights = draft.map(|draft| weights_against(project, since, old_text, draft));
    let version_name = |number: u32| match number == manifest.draft_version() {
        true => "draft".to_string(),
        false => format!("v{number}"),
    };
    let mut quoted = 0;
    let mut rows = Vec::new();
    for idea in manifest.live_ideas() {
        let notes: Vec<Value> = idea
            .notes
            .iter()
            .filter(|note| !note.text.trim().is_empty())
            .map(|note| json!({ "version": version_name(note.version), "text": note.text }))
            .collect();
        let cut: Vec<String> = idea.cut_in.iter().map(|&v| version_name(v)).collect();
        let mut row = json!({
            "name": idea.name,
            "colour": COLOUR_NAMES.get(idea.colour).copied().unwrap_or("grey"),
            "topic": topic_name(project, idea.topic),
            "feeling_notes": notes,
            "cut_on_purpose_in": cut,
        });
        row[format!("painted_in_v{since}")] =
            json!(passages(old_text, &old_paint, idea.id, &mut quoted));
        if let (Some(draft), Some(weights)) = (draft, &weights) {
            row["painted_in_draft"] =
                json!(passages(draft, &project.draft_paint, idea.id, &mut quoted));
            let status = weights.ideas.iter().find(|row| row.idea == idea.id);
            row["status"] = json!(status.map_or("unused", |row| status_word(row.status)));
        }
        rows.push(row);
    }
    rows
}

/// The passages of `text` that `paint` paints with `idea`, each cut short, until `quoted`
/// (characters quoted so far, over all ideas) reaches `MAX_QUOTED_CHARS`.
fn passages(text: &str, paint: &[Span], idea: IdeaId, quoted: &mut usize) -> Vec<String> {
    let mut quotes = Vec::new();
    for span in paint.iter().filter(|span| span.idea == idea) {
        let Some(passage) = text.get(span.start..span.end) else {
            continue;
        };
        if *quoted >= MAX_QUOTED_CHARS {
            quotes.push("(more not quoted)".to_string());
            break;
        }
        let quote = shorten(passage, MAX_QUOTE_CHARS);
        *quoted += quote.chars().count();
        quotes.push(quote);
    }
    quotes
}

/// Each idea's and topic's share of version `since` (`old_text`) and of the draft, as JSON.
pub fn idea_weights(project: &ProjectSnapshot, since: u32, old_text: &str, draft: &str) -> String {
    let manifest = &project.project.manifest;
    let weights = weights_against(project, since, old_text, draft);
    let ideas: Vec<Value> = weights
        .ideas
        .iter()
        .map(|row| {
            let name = manifest
                .idea(row.idea)
                .map_or("?", |idea| idea.name.as_str());
            let mut fields = weight_fields(&row.weight);
            fields["idea"] = json!(name);
            fields["status"] = json!(status_word(row.status));
            fields
        })
        .collect();
    let topics: Vec<Value> = weights
        .topics
        .iter()
        .map(|row| {
            let mut fields = weight_fields(&row.weight);
            fields["topic"] = json!(topic_name(project, Some(row.topic)));
            fields
        })
        .collect();
    let total = weights.total;
    let measure = match (total.old.tokens, total.new.tokens) {
        (Some(_), Some(_)) => "tokens",
        _ => "words (the tokenizer is still loading)",
    };
    let change = percent_change(total.old, total.new).map(|percent| format!("{percent:+}%"));
    json!({
        "compared": format!("v{since} -> draft"),
        "shares_measured_in": measure,
        "ideas": ideas,
        "topics": topics,
        "unpainted": weight_fields(&weights.unpainted),
        "total": {
            "words_before": total.old.words,
            "words_after": total.new.words,
            "tokens_before": total.old.tokens,
            "tokens_after": total.new.tokens,
            "change": change,
        },
    })
    .to_string()
}

/// The weights of every idea in version `since` (`old_text`) against the draft.
fn weights_against(project: &ProjectSnapshot, since: u32, old_text: &str, draft: &str) -> Weights {
    let old_paint = project.project.version_paint(since, old_text);
    weights::weights(
        &project.project.manifest,
        since,
        old_text,
        &old_paint,
        draft,
        &project.draft_paint,
    )
}

fn weight_fields(weight: &Weight) -> Value {
    let rounded = |share: f64| (share * 10.0).round() / 10.0;
    json!({
        "percent_before": rounded(weight.old_share),
        "percent_after": rounded(weight.new_share),
        "words_before": weight.old.words,
        "words_after": weight.new.words,
        "tokens_before": weight.old.tokens,
        "tokens_after": weight.new.tokens,
    })
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Carried => "carried",
        Status::Cut => "cut on purpose",
        Status::Missing => "missing",
        Status::Unused => "unused",
    }
}

fn topic_name(project: &ProjectSnapshot, topic: Option<u32>) -> Option<String> {
    let topics = &project.project.manifest.topics;
    let topic = topics
        .iter()
        .find(|candidate| Some(candidate.id) == topic)?;
    Some(topic.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::context::tests::{project_snapshot, scratch};

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            call_id: "call_1".to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    fn output(snapshot: &Snapshot, name: &str, arguments: Value) -> String {
        run(&call(name, arguments), snapshot).0
    }

    fn as_json(text: &str) -> Value {
        serde_json::from_str(text).unwrap_or_else(|_| panic!("not JSON: {text}"))
    }

    #[test]
    fn project_tools_are_offered_only_in_projects() {
        let names = |snapshot: &Snapshot| -> Vec<&str> {
            catalogue(snapshot).iter().map(|tool| tool.name).collect()
        };
        let plain = names(&scratch(""));
        assert_eq!(
            plain,
            [
                "get_document",
                "get_selection",
                "find_text",
                "count_tokens",
                "propose_edit"
            ]
        );
        let project = project_snapshot("");
        assert_eq!(names(&project).len(), 9);
        assert!(names(&project).contains(&"get_idea_weights"));
        let edit_schema = |snapshot: &Snapshot| {
            catalogue(snapshot)
                .into_iter()
                .find(|tool| tool.name == "propose_edit")
                .unwrap()
                .parameters
        };
        assert!(
            edit_schema(&scratch(""))["properties"]
                .get("idea")
                .is_none()
        );
        assert!(edit_schema(&project)["properties"].get("idea").is_some());

        // A project tool called anyway in a plain document is refused in words.
        assert_eq!(
            output(&scratch(""), "list_ideas", json!({})),
            "There is no tool called list_ideas here."
        );
    }

    #[test]
    fn get_document_reads_all_or_some_lines() {
        let snapshot = scratch("one\ntwo\nthree\nfour");
        assert_eq!(
            output(&snapshot, "get_document", json!({})),
            "one\ntwo\nthree\nfour"
        );
        assert_eq!(
            output(
                &snapshot,
                "get_document",
                json!({ "start_line": 2, "end_line": "3" })
            ),
            "[Lines 2-3 of 4]\ntwo\nthree"
        );
        assert_eq!(
            output(&snapshot, "get_document", json!({ "start_line": 9 })),
            "There are 4 lines; nothing in that range."
        );
        assert_eq!(
            output(&scratch(""), "get_document", json!({})),
            "(The text is empty.)"
        );

        // A long document comes back in parts.
        let line = "x".repeat(999);
        let long = vec![line.as_str(); 100].join("\n");
        let first = output(&scratch(&long), "get_document", json!({}));
        assert!(first.starts_with("[Lines 1-48 of 100]\n"));
        assert!(first.ends_with("[More follows: ask for start_line 49 to read on.]"));
        assert!(first.len() < MAX_RESULT_TEXT + 200);

        // So does one very long line, cut on a character boundary.
        let line = "ü".repeat(30_000);
        let cut = output(
            &scratch(&format!("{line}\nnext")),
            "get_document",
            json!({}),
        );
        assert!(cut.starts_with("[Lines 1-1 of 2]\nüü"));
        assert!(cut.ends_with(
            "ü\n[Line 1 is cut short: it is too long for one result.]\n[More follows: ask for \
             start_line 2 to read on.]"
        ));
        assert!(cut.len() < MAX_RESULT_TEXT + 200);
    }

    #[test]
    fn get_selection_gives_the_text_and_its_lines() {
        let mut snapshot = scratch("alpha\nbeta gamma\ndelta");
        assert_eq!(
            output(&snapshot, "get_selection", json!({})),
            "Nothing is selected."
        );
        snapshot.selection = Some(6..10);
        assert_eq!(
            output(&snapshot, "get_selection", json!({})),
            "[Selected, lines 2-2]\nbeta"
        );
        // A huge selection is cut to one result's size.
        let mut all = scratch(&"word ".repeat(20_000));
        all.selection = Some(0..all.text.len());
        let cut = output(&all, "get_selection", json!({}));
        assert!(cut.ends_with("[The selection is cut short: read the rest with get_document.]"));
        assert!(cut.len() < MAX_RESULT_TEXT + 200);
    }

    #[test]
    fn find_text_lists_lines_ignoring_case_and_unicode() {
        let snapshot = scratch("Ünïcode WARM start\nnothing\nwarm and warm\nİstanbul is warm");
        let found = as_json(&output(&snapshot, "find_text", json!({ "text": "warm" })));
        assert_eq!(found["matches"], 4);
        assert_eq!(
            found["lines"],
            json!([
                { "line": 1, "text": "Ünïcode WARM start" },
                { "line": 3, "text": "warm and warm" },
                { "line": 4, "text": "İstanbul is warm" },
            ])
        );
        let found = as_json(&output(&snapshot, "find_text", json!({ "text": "üNÏ" })));
        assert_eq!(found["lines"][0]["line"], 1);
        assert_eq!(
            output(&snapshot, "find_text", json!({ "text": "cold" })),
            "No matches for \"cold\"."
        );
        assert_eq!(
            output(&snapshot, "find_text", json!({})),
            "Say what text to find."
        );

        // Past the most lines listed, matches are only counted.
        let many = "a b\n".repeat(80);
        let found = as_json(&output(
            &scratch(&many),
            "find_text",
            json!({ "text": "a" }),
        ));
        assert_eq!(found["matches"], 80);
        assert_eq!(found["lines"].as_array().unwrap().len(), MAX_MATCHES);
        assert_eq!(found["lines"][49]["line"], 50);
    }

    #[test]
    fn count_tokens_counts_given_text_or_the_document() {
        crate::core::counts::load_tokenizer();
        let snapshot = scratch("one two three");
        let whole = as_json(&output(&snapshot, "count_tokens", json!({})));
        assert_eq!(whole["words"], 3);
        assert!(whole["tokens"].as_u64().unwrap() >= 3);
        let part = as_json(&output(&snapshot, "count_tokens", json!({ "text": "one" })));
        assert_eq!(part, json!({ "words": 1, "tokens": 1 }));
    }

    #[test]
    fn propose_edit_records_a_suggestion_only_for_exact_unique_text() {
        let mut snapshot = scratch("Keep it warm. Keep it short, ünïcode.");
        let (said, suggestion) = run(
            &call(
                "propose_edit",
                json!({ "find": "short, ünïcode", "replace": "brief", "why": "Tighter." }),
            ),
            &snapshot,
        );
        assert_eq!(said, SUGGESTED);
        let suggestion = suggestion.unwrap();
        assert_eq!(suggestion.at, snapshot.text.find("short").unwrap());
        assert_eq!(
            (suggestion.replace.as_str(), suggestion.why.as_str()),
            ("brief", "Tighter.")
        );
        assert_eq!(suggestion.idea, None);

        let refused = |arguments: Value, snapshot: &Snapshot| {
            let (said, suggestion) = run(&call("propose_edit", arguments), snapshot);
            assert!(suggestion.is_none());
            said
        };
        assert!(
            refused(json!({ "find": "cold", "replace": "x" }), &snapshot).starts_with("Not found.")
        );
        assert!(
            refused(json!({ "find": "Keep it", "replace": "x" }), &snapshot)
                .contains("occurs 2 times")
        );
        assert!(
            refused(json!({ "find": "warm", "replace": "warm" }), &snapshot)
                .contains("changes nothing")
        );
        assert!(refused(json!({}), &snapshot).contains("changes nothing"));
        snapshot
            .waiting
            .push(suggestion.at..suggestion.at + suggestion.find.len());
        let overlapping = json!({ "find": "it short", "replace": "x" });
        assert!(refused(overlapping, &snapshot).contains("overlaps"));
        // Next to a waiting suggestion is fine.
        let beside = json!({ "find": "Keep it warm.", "replace": "Stay warm." });
        assert!(run(&call("propose_edit", beside), &snapshot).1.is_some());

        let broken = ToolCall {
            arguments: "{not json".to_string(),
            ..call("propose_edit", json!({}))
        };
        assert_eq!(
            run(&broken, &snapshot).0,
            "The arguments weren't a JSON object."
        );

        // A read-only window can't take suggestions, and the model hears so.
        snapshot.read_only = true;
        let said = refused(
            json!({ "find": "Keep it warm.", "replace": "x" }),
            &snapshot,
        );
        assert!(said.starts_with("This window is read-only"));
    }

    #[test]
    fn propose_edit_with_an_empty_find_adds_text_at_the_end() {
        let (said, suggestion) = run(
            &call(
                "propose_edit",
                json!({ "find": "", "replace": "Dear Sam,", "why": "w" }),
            ),
            &scratch(""),
        );
        assert_eq!(said, SUGGESTED);
        assert_eq!(suggestion.unwrap().at, 0);
        let mut snapshot = scratch("Hi.");
        let add = json!({ "replace": "\nBye." });
        let suggestion = run(&call("propose_edit", add.clone()), &snapshot)
            .1
            .unwrap();
        assert_eq!((suggestion.at, suggestion.find.as_str()), (3, ""));
        // A second addition at the end clashes with the first, as it would in the pane.
        snapshot.waiting.push(3..3);
        assert!(
            run(&call("propose_edit", add), &snapshot)
                .0
                .contains("overlaps")
        );
    }

    #[test]
    fn propose_edit_in_a_project_takes_an_idea_by_name() {
        let snapshot = project_snapshot("Let tension build.");
        let (_, suggestion) = run(
            &call(
                "propose_edit",
                json!({ "find": "build", "replace": "build slowly", "why": "w", "idea": "slow BURN" }),
            ),
            &snapshot,
        );
        let DocKind::Project(project) = &snapshot.doc else {
            unreachable!()
        };
        let slow_burn = project.project.manifest.live_ideas().next().unwrap().id;
        assert_eq!(suggestion.unwrap().idea, Some(slow_burn));

        let said = output(
            &snapshot,
            "propose_edit",
            json!({ "find": "build", "replace": "x", "why": "w", "idea": "Menace" }),
        );
        assert_eq!(
            said,
            "There is no idea called \"Menace\". The ideas are: Slow burn, Agency. Leave idea out \
             if none fits."
        );
    }

    #[test]
    fn project_tools_read_versions_ideas_and_weights() {
        crate::core::counts::load_tokenizer();
        let mut snapshot = project_snapshot("Let tension build slowly. Act.");
        let DocKind::Project(project) = &mut snapshot.doc else {
            unreachable!()
        };
        // Paint "three" in v3 and the start of the draft with Slow burn, and give it notes.
        let slow_burn = project.project.manifest.live_ideas().next().unwrap().id;
        project.project.manifest.versions[2].paint = vec![Span {
            idea: slow_burn,
            start: 0,
            end: 5,
            quote: "three".to_string(),
        }];
        project.draft_paint = vec![Span {
            idea: slow_burn,
            start: 0,
            end: 24,
            quote: "Let tension build slowly".to_string(),
        }];
        project
            .project
            .manifest
            .set_note(slow_burn, 4, "patient, not dull");
        project.project.manifest.set_note(slow_burn, 2, "tense");
        let agency = project.project.manifest.live_ideas().nth(1).unwrap().id;
        project.project.manifest.set_cut(agency, 2, true);

        assert_eq!(
            output(&snapshot, "get_version", json!({ "number": 2 })),
            "two"
        );
        assert_eq!(
            output(&snapshot, "get_version", json!({ "number": "v3" })),
            "three"
        );
        assert_eq!(
            output(&snapshot, "get_version", json!({ "number": 7 })),
            "There is no v7. The versions are v1 to v3."
        );

        let versions = as_json(&output(&snapshot, "list_versions", json!({})));
        assert_eq!(versions[0]["version"], "v1");
        assert_eq!(versions[0]["note"], "note 1");
        assert_eq!(versions[0]["committed"], "Oct 12");
        assert_eq!(versions[3]["version"], "draft (becomes v4)");
        assert_eq!(versions[3]["words"], 5);

        let ideas = as_json(&output(&snapshot, "list_ideas", json!({})));
        assert_eq!(ideas["compared_with"], "v3");
        let slow = &ideas["ideas"][0];
        assert_eq!(slow["name"], "Slow burn");
        assert_eq!(slow["topic"], "Pacing");
        assert_eq!(slow["status"], "carried");
        assert_eq!(
            slow["feeling_notes"],
            json!([{ "version": "v2", "text": "tense" }, { "version": "draft", "text": "patient, not dull" }])
        );
        assert!(slow["colour"].is_string());
        assert_eq!(slow["painted_in_v3"], json!(["three"]));
        assert_eq!(
            slow["painted_in_draft"],
            json!(["Let tension build slowly"])
        );
        assert_eq!(ideas["ideas"][1]["status"], "cut on purpose");
        assert_eq!(ideas["ideas"][1]["cut_on_purpose_in"], json!(["v2"]));
        assert_eq!(ideas["ideas"][1]["painted_in_draft"], json!([]));

        let weights = as_json(&output(&snapshot, "get_idea_weights", json!({})));
        assert_eq!(weights["compared"], "v3 -> draft");
        assert_eq!(weights["shares_measured_in"], "tokens");
        assert_eq!(weights["ideas"][0]["idea"], "Slow burn");
        assert_eq!(weights["ideas"][0]["percent_before"], 100.0);
        assert_eq!(weights["ideas"][0]["words_after"], 4);
        assert_eq!(weights["topics"][0]["topic"], "Pacing");
        assert_eq!(weights["unpainted"]["words_after"], 1);
        assert_eq!(weights["total"]["words_before"], 1);

        // Against v2, where Slow burn had no paint.
        let weights = as_json(&output(
            &snapshot,
            "get_idea_weights",
            json!({ "since": 2 }),
        ));
        assert_eq!(weights["compared"], "v2 -> draft");
        assert_eq!(weights["ideas"][0]["percent_before"], 0.0);
        assert!(output(&snapshot, "get_idea_weights", json!({ "since": 0 })).contains("no v0"));
    }

    #[test]
    fn tool_calls_read_as_short_activity_lines() {
        assert_eq!(
            describe(&call("find_text", json!({ "text": "warm" })), ""),
            "Searched for \"warm\""
        );
        assert_eq!(
            describe(&call("get_version", json!({ "number": 3 })), ""),
            "Read v3"
        );
        assert_eq!(describe(&call("mystery", json!({})), ""), "Used mystery");
        let edit = call("propose_edit", json!({}));
        assert_eq!(describe(&edit, SUGGESTED), "Suggested an edit");
        assert_eq!(describe(&edit, "Not found."), "Couldn't suggest an edit");
    }

    #[test]
    fn idea_list_quotes_stop_at_the_budget() {
        let mut snapshot = project_snapshot(&"warm ".repeat(2_000));
        let DocKind::Project(project) = &mut snapshot.doc else {
            unreachable!()
        };
        let slow_burn = project.project.manifest.live_ideas().next().unwrap().id;
        project.draft_paint = (0..100)
            .map(|n| Span {
                idea: slow_burn,
                start: n * 100,
                end: n * 100 + 99,
                quote: String::new(),
            })
            .collect();
        let rows = idea_rows(project, Some(&snapshot.text));
        let quotes = rows[0]["painted_in_draft"].as_array().unwrap();
        // 81 quotes of 99 characters pass 8,000; then one line says there are more.
        assert_eq!(quotes.len(), 82);
        assert_eq!(quotes[81], "(more not quoted)");
        // Without the draft, nothing that changes as it is typed.
        let rows = idea_rows(project, None);
        assert!(rows[0].get("status").is_none() && rows[0].get("painted_in_draft").is_none());
    }
}
