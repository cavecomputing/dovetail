//! The tools the assistant can call, and running them against a `Snapshot` of the open document.
//! Every tool only reads; `propose_edit` records a suggestion for the user to accept or reject.
//! Results are plain text or JSON, written for the model.

use serde_json::{Value, json};

use crate::ai::client::ToolCall;
use crate::ai::context::Snapshot;
use crate::core::counts::Counts;
use crate::core::review::clashes;
use crate::core::text_model::position;

/// The most text one result hands back (about 12,000 tokens). Longer texts are read by lines.
const MAX_RESULT_TEXT: usize = 48_000;
/// The most lines `find_text` lists.
const MAX_MATCHES: usize = 50;
/// The longest line `find_text` shows, in characters.
const MAX_LINE_CHARS: usize = 400;
/// What `propose_edit` answers when the suggestion is shown.
const SUGGESTED: &str = "Suggested. The user sees it in the document and will accept or reject it.";

/// A tool the model may call, with a JSON schema of its arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

/// An edit the model proposed: replace `find` with `replace`. It is shown in the document until
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
}

/// The tools on offer.
pub fn catalogue() -> Vec<Tool> {
    let line_range = json!({
        "start_line": { "type": "integer", "description": "First line to read, counting from 1." },
        "end_line": { "type": "integer", "description": "Last line to read, included." },
    });
    let edit_arguments = json!({
        "find": {
            "type": "string",
            "description": "Text copied exactly from the document, long enough to occur only \
                once. Empty to add the new text at the end of the document.",
        },
        "replace": { "type": "string", "description": "The new text to put in its place." },
        "why": { "type": "string", "description": "One short sentence on why, shown to the user." },
    });
    vec![
        Tool {
            name: "get_document",
            description: "Read the document. Returns the whole text, or the lines asked for. A \
                long document comes back in parts: ask for the rest with start_line and \
                end_line.",
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
                user sees it marked in the document and accepts or rejects it; nothing changes until they \
                accept. Use one call per passage.",
            parameters: json!({
                "type": "object",
                "properties": edit_arguments,
                "required": ["find", "replace", "why"],
            }),
        },
    ]
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
    let text = &snapshot.text;
    let output = match call.name.as_str() {
        "get_document" => read_lines(text, &arguments),
        "get_selection" => selection(snapshot),
        "find_text" => find_text(text, arguments["text"].as_str().unwrap_or("")),
        "count_tokens" => count(arguments["text"].as_str().unwrap_or(text)).to_string(),
        "propose_edit" => return propose_edit(snapshot, &arguments),
        name => format!("There is no tool called {name} here."),
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
        "propose_edit" if output.starts_with(SUGGESTED) => "Suggested an edit".to_string(),
        "propose_edit" => "Couldn't suggest an edit".to_string(),
        name => format!("Used {name}"),
    }
}

/// A whole number from the model, which sometimes writes 3 as 3.0 or "3".
fn number(value: &Value) -> Option<u32> {
    let number = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.trim().parse().ok()?,
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

/// `find` and `replace` without what both have at their start and end, and how many bytes were
/// left off the start. A model often copies a line or a few around what it changes; the
/// suggestion, and the red and green the editor shows, is only what really changes: whole lines,
/// or within one line whole words.
fn changed_part<'a>(find: &'a str, replace: &'a str) -> (usize, &'a str, &'a str) {
    let (head, find, replace) = trim_shared(find, replace, false);
    let one_line = |part: &str| !part.trim_end_matches('\n').contains('\n');
    if !(one_line(find) && one_line(replace)) {
        return (head, find, replace);
    }
    let (more, find, replace) = trim_shared(find, replace, true);
    (head + more, find, replace)
}

/// `find` and `replace` without the pieces at their start and end that both have: whole lines,
/// or with `words` whole words (each with the spaces or line break after it).
fn trim_shared<'a>(find: &'a str, replace: &'a str, words: bool) -> (usize, &'a str, &'a str) {
    let pieces = |text: &'a str| -> Vec<&'a str> {
        match words {
            true => text.split_inclusive(char::is_whitespace).collect(),
            false => text.split_inclusive('\n').collect(),
        }
    };
    let (old, new) = (pieces(find), pieces(replace));
    let first = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let last = old[first..]
        .iter()
        .rev()
        .zip(new[first..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let length = |pieces: &[&str]| pieces.iter().map(|piece| piece.len()).sum::<usize>();
    let head = length(&old[..first]);
    let find_end = find.len() - length(&old[old.len() - last..]);
    let replace_end = replace.len() - length(&new[new.len() - last..]);
    (head, &find[head..find_end], &replace[head..replace_end])
}

fn propose_edit(snapshot: &Snapshot, arguments: &Value) -> (String, Option<Suggestion>) {
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
    // The suggestion covers only the lines or words that change, so the document shows just
    // those; the validity check below looks at the whole edit, which is the same.
    let (skipped, find_part, replace_part) = changed_part(find, replace);
    let range = at + skipped..at + skipped + find_part.len();
    if snapshot.waiting.iter().any(|other| clashes(other, &range)) {
        return refuse(
            "That overlaps another suggestion still waiting for the user. Change different \
             text, or wait for them to accept or reject it."
                .to_string(),
        );
    }
    let suggestion = Suggestion {
        at: range.start,
        find: find_part.to_string(),
        replace: replace_part.to_string(),
        why: arguments["why"].as_str().unwrap_or("").to_string(),
    };
    // In a file the editor checks (JSON, YAML, TOML, INI, a shell script) that is valid now, say
    // if the edit alone would break it. It still shows: another edit may complete it.
    let syntax = snapshot.syntax();
    let mut edited = text.clone();
    edited.replace_range(range, replace_part);
    let broken = match syntax.problem(text) {
        None => syntax.problem(&edited),
        Some(_) => None,
    };
    let output = match broken {
        None => SUGGESTED.to_string(),
        Some(problem) => format!(
            "{SUGGESTED} But on its own it would leave the file invalid: {problem}. If no other \
             edit of yours fixes that, suggest a corrected one."
        ),
    };
    (output, Some(suggestion))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::context::DocKind;
    use crate::ai::context::tests::scratch;

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
    fn the_tools_on_offer() {
        let names: Vec<&str> = catalogue().iter().map(|tool| tool.name).collect();
        assert_eq!(
            names,
            [
                "get_document",
                "get_selection",
                "find_text",
                "count_tokens",
                "propose_edit"
            ]
        );
        // A tool that isn't there is refused in words.
        assert_eq!(
            output(&scratch(""), "mystery", json!({})),
            "There is no tool called mystery here."
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
    fn a_suggestion_covers_only_the_lines_that_change() {
        let yaml = "web:\n  image: nginx\n  restart: always\n";
        let arguments = json!({
            "find": "  image: nginx\n  restart: always\n",
            "replace": "  image: nginx\n  ports: [80]\n  restart: always\n",
            "why": "x",
        });
        let (_, suggestion) = run(&call("propose_edit", arguments), &scratch(yaml));
        let suggestion = suggestion.expect("suggested");
        assert_eq!(suggestion.at, yaml.find("  restart").unwrap());
        assert_eq!(suggestion.find, "");
        assert_eq!(suggestion.replace, "  ports: [80]\n");
        assert_eq!(changed_part("very big", "huge"), (0, "very big", "huge"));
        assert_eq!(changed_part("a\nb\nc", "a\nB\nc"), (2, "b\n", "B\n"));
        assert_eq!(changed_part("x\n", "x"), (0, "x\n", "x"));
        let (old, new) = ("The big red house.", "The big blue house.");
        assert_eq!(changed_part(old, new), (8, "red ", "blue "));
        let (old, new) = ("a\nb c\nd", "a\nb C\nd");
        assert_eq!(changed_part(old, new), (4, "c\n", "C\n"));
        assert_eq!(changed_part("a b", "a new b"), (2, "", "new "));
    }

    #[test]
    fn propose_edit_says_when_an_edit_would_break_a_checked_file() {
        let json = Snapshot {
            doc: DocKind::TextFile {
                name: "data.json".to_string(),
            },
            ..scratch("{\"a\": 1, \"b\": 2}")
        };
        let edit = |find: &str, replace: &str| {
            let arguments = json!({ "find": find, "replace": replace, "why": "x" });
            run(&call("propose_edit", arguments), &json)
        };
        let (output, suggestion) = edit("\"b\": 2", "\"b\": 3");
        assert_eq!(output, SUGGESTED);
        assert!(suggestion.is_some());
        let (output, suggestion) = edit(", \"b\": 2", ", \"b\" 2");
        assert!(output.starts_with(SUGGESTED), "it still shows");
        assert!(
            output.contains("invalid: Invalid JSON at line 1"),
            "{output}"
        );
        assert!(suggestion.is_some());
        // Plain text has nothing to break.
        let (output, _) = run(
            &call(
                "propose_edit",
                json!({ "find": "{", "replace": "[", "why": "x" }),
            ),
            &scratch("{"),
        );
        assert_eq!(output, SUGGESTED);
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
    fn tool_calls_read_as_short_activity_lines() {
        assert_eq!(
            describe(&call("find_text", json!({ "text": "warm" })), ""),
            "Searched for \"warm\""
        );
        assert_eq!(describe(&call("mystery", json!({})), ""), "Used mystery");
        let edit = call("propose_edit", json!({}));
        assert_eq!(describe(&edit, SUGGESTED), "Suggested an edit");
        assert_eq!(describe(&edit, "Not found."), "Couldn't suggest an edit");
    }
}
