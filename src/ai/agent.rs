//! A conversation with the assistant and its agent loop, as a plain state machine the UI drives:
//!
//! 1. `ask` with the user's text and a `Snapshot` of the open document.
//! 2. `request`, then stream that `Reply`, passing each piece to `receive`.
//! 3. When `receive` says `SendAgain`, the model's tool calls have run against the snapshot and
//!    their results are in the conversation: go back to step 2. `Finished` ends the turn.
//!
//! Tools never touch the editor: `propose_edit` only adds to `new_suggestions` (and `withdrawn`,
//! the waiting suggestions it replaces), which the pane takes and shows in the document for the
//! user to accept or reject.
//!
//! Every question in the history has an answer after it, so the next request never has two
//! questions in a row, which some servers refuse.

use serde_json::Value;

use crate::ai::client::{Item, Reply, Request, StreamEvent, ToolCall};
use crate::ai::context::{Snapshot, instructions, open_text, turn_note};
use crate::ai::tools::{self, Suggestion, catalogue};
use crate::core::settings::Settings;

/// The most requests one question may take. Each round of tool calls is one more; past this the
/// model is stopped and told nothing more.
pub const MAX_ROUNDS: usize = 8;
/// The answer kept for a question whose tools ran but which got no answer.
const NO_ANSWER: &str = "(Stopped before answering.)";

#[derive(Debug, Clone, Default)]
pub struct Conversation {
    /// Everything said so far, oldest first, exactly as it is sent to the model.
    pub items: Vec<Item>,
    /// The reply arriving now: its text so far.
    pub reply: String,
    /// The tool calls in the reply arriving now.
    calls: Vec<ToolCall>,
    /// Edits the model just proposed, for the pane to take and show.
    pub new_suggestions: Vec<Suggestion>,
    /// The ids of waiting suggestions the model replaced or withdrew, for the pane to remove
    /// before it shows `new_suggestions`.
    pub withdrawn: Vec<u64>,
    /// What the current question sees.
    snapshot: Option<Snapshot>,
    /// The text being edited, sent with the current question only (see `open_text`).
    open_text: String,
    /// Requests made for the current question.
    rounds: usize,
    /// True from `ask` until the turn finishes, fails or is stopped.
    pub busy: bool,
    /// Why the last turn stopped short, in plain words.
    pub error: Option<String>,
    /// A question that ended before anything came back, taken out of the history so the pane
    /// can put it back in the message box.
    pub unanswered: Option<String>,
}

/// What the UI does after `receive`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Next {
    /// Keep reading the reply.
    Wait,
    /// Tools ran; send `request()` again so the model sees their results.
    SendAgain,
    /// The turn is over: answered, or stopped (`error` says why).
    Finished,
}

impl Conversation {
    /// Starts a turn: the user's `text`, about `snapshot`. Call `stop` first if a turn is running.
    pub fn ask(&mut self, text: &str, snapshot: Snapshot) {
        self.items.push(Item::User {
            text: text.to_string(),
            note: turn_note(&snapshot),
        });
        self.open_text = open_text(&snapshot);
        self.snapshot = Some(snapshot);
        self.rounds = 0;
        self.reply.clear();
        self.calls.clear();
        self.error = None;
        self.busy = true;
    }

    /// The request for the next reply. Fails, in words for the pane, when no model is set or
    /// nothing has been asked.
    pub fn request(
        &mut self,
        settings: &Settings,
        api_key: Option<&str>,
    ) -> Result<Request, String> {
        let Some(snapshot) = &self.snapshot else {
            return Err("Ask something first.".to_string());
        };
        // The text goes with the newest question only, so the history doesn't grow by a whole
        // document per question.
        let mut items = self.items.clone();
        let newest = items.iter_mut().rev().find_map(|item| match item {
            Item::User { note, .. } => Some(note),
            _ => None,
        });
        if let Some(note) = newest {
            note.push('\n');
            note.push_str(&self.open_text);
        }
        let request = Request::new(
            settings,
            api_key,
            &instructions(snapshot),
            &catalogue(),
            &items,
        );
        match &request {
            Ok(_) => self.rounds += 1,
            Err(reason) => self.end_turn(Some(reason.clone())),
        }
        request
    }

    /// Takes the next piece of the reply. Pieces arriving after the turn ended are ignored.
    pub fn receive(&mut self, event: StreamEvent) -> Next {
        if !self.busy {
            return Next::Finished;
        }
        match event {
            StreamEvent::Text(piece) => {
                self.reply.push_str(&piece);
                Next::Wait
            }
            StreamEvent::ToolCall(call) => {
                self.calls.push(call);
                Next::Wait
            }
            StreamEvent::Done => self.finish_reply(),
            StreamEvent::Failed(reason) => {
                self.end_turn(Some(reason));
                Next::Finished
            }
        }
    }

    /// Updates what the tools see. The UI calls it just before passing on a reply's `Done`, so
    /// the tool calls run against the document as it is now, not as it was when asked.
    pub fn see(&mut self, snapshot: Snapshot) {
        self.snapshot = Some(snapshot);
    }

    /// The user stopped the reply (the UI drops its `Reply`). What text came is kept.
    pub fn stop(&mut self) {
        if self.busy {
            self.end_turn(None);
        }
    }

    /// The reply is complete: keeps it, and runs any tools it asked for.
    fn finish_reply(&mut self) -> Next {
        let calls = std::mem::take(&mut self.calls);
        if calls.is_empty() {
            let empty = self.reply.is_empty();
            self.end_turn(empty.then(|| "The model's reply was empty.".to_string()));
            return Next::Finished;
        }
        // Arguments that aren't a JSON object go back as {}: some servers re-read them and fail.
        // The tool's result already told the model they were wrong.
        let mut kept = calls.clone();
        for call in &mut kept {
            let arguments = serde_json::from_str::<Value>(&call.arguments);
            if !arguments.is_ok_and(|arguments| arguments.is_object()) {
                call.arguments = "{}".to_string();
            }
        }
        self.items.push(Item::Assistant {
            text: std::mem::take(&mut self.reply),
            tool_calls: kept,
        });
        let snapshot = self
            .snapshot
            .as_mut()
            .expect("a reply only comes after ask, which sets the snapshot");
        for call in &calls {
            let (output, edit) = tools::run(call, snapshot);
            self.withdrawn.extend(edit.withdrawn);
            if let Some(suggestion) = edit.suggestion {
                // The next call in this reply may not overlap it either.
                snapshot.proposed.push(suggestion.range());
                self.new_suggestions.push(suggestion);
            }
            self.items.push(Item::ToolResult {
                call_id: call.call_id.clone(),
                output,
            });
        }
        // Every call has its result, so asking again later still makes a valid conversation.
        if self.rounds >= MAX_ROUNDS {
            self.end_turn(Some(format!(
                "Stopped after {MAX_ROUNDS} rounds of tool use without an answer. Ask again to \
                 let it carry on."
            )));
            return Next::Finished;
        }
        Next::SendAgain
    }

    /// Ends the turn, keeping any text that arrived. Tool calls without results are dropped, since
    /// both formats refuse a call with no result. A question that got nothing back is taken out
    /// (into `unanswered`); one whose tools ran but got no answer is answered with `NO_ANSWER`.
    fn end_turn(&mut self, error: Option<String>) {
        let text = std::mem::take(&mut self.reply);
        self.calls.clear();
        if !text.is_empty() {
            self.items.push(Item::Assistant {
                text,
                tool_calls: Vec::new(),
            });
        } else if let Some(Item::User { text, .. }) = self.items.last() {
            self.unanswered = Some(text.clone());
            self.items.pop();
        } else if let Some(Item::ToolResult { .. }) = self.items.last() {
            self.items.push(Item::Assistant {
                text: NO_ANSWER.to_string(),
                tool_calls: Vec::new(),
            });
        }
        self.error = error;
        self.busy = false;
    }
}

/// Settings' "test connection": a one-line question with the usual tools on offer, so a server
/// that can't take tools fails here too. Returns the reply's text (maybe empty, if the model
/// called a tool instead), or why it failed, in words.
pub async fn test_connection(settings: &Settings, api_key: Option<&str>) -> Result<String, String> {
    let question = Item::User {
        text: "Reply with the single word OK.".to_string(),
        note: String::new(),
    };
    let request = Request::new(
        settings,
        api_key,
        "This is a connection test.",
        &catalogue(),
        &[question],
    )?;
    let mut reply = Reply::new(request);
    let mut text = String::new();
    while let Some(event) = reply.next().await {
        match event {
            StreamEvent::Text(piece) => text.push_str(&piece),
            StreamEvent::ToolCall(_) | StreamEvent::Done => {}
            StreamEvent::Failed(reason) => return Err(reason),
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::client::tests::{all_events, serve, settings_for};
    use crate::ai::context::Waiting;
    use crate::ai::context::tests::scratch as snapshot;
    use crate::core::settings::RequestFormat;

    fn settings() -> Settings {
        settings_for("http://localhost/v1", RequestFormat::Responses)
    }

    fn tool_call(call_id: &str, name: &str, arguments: &str) -> StreamEvent {
        StreamEvent::ToolCall(ToolCall {
            call_id: call_id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        })
    }

    #[test]
    fn a_tool_call_runs_and_its_result_goes_back_before_the_answer() {
        let mut talk = Conversation::default();
        talk.ask("Make it warmer", snapshot("Stay cold."));
        assert!(talk.busy);
        talk.request(&settings(), None).unwrap();
        assert_eq!(
            talk.receive(StreamEvent::Text("Looking.".into())),
            Next::Wait
        );
        let edit = r#"{"find":"cold","replace":"warm","why":"Warmer."}"#;
        assert_eq!(
            talk.receive(tool_call("c1", "propose_edit", edit)),
            Next::Wait
        );
        assert_eq!(talk.receive(StreamEvent::Done), Next::SendAgain);

        let suggestions = std::mem::take(&mut talk.new_suggestions);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].replace, "warm");
        let Item::ToolResult { call_id, output } = &talk.items[2] else {
            panic!("expected a tool result, got {:?}", talk.items[2]);
        };
        assert_eq!(call_id, "c1");
        assert!(output.starts_with("Suggested."));

        talk.request(&settings(), None).unwrap();
        talk.receive(StreamEvent::Text("I suggested ".into()));
        talk.receive(StreamEvent::Text("\"warm\".".into()));
        assert_eq!(talk.receive(StreamEvent::Done), Next::Finished);
        assert!(!talk.busy);
        assert_eq!(talk.error, None);
        assert_eq!(talk.items.len(), 4);
        assert_eq!(
            talk.items[3],
            Item::Assistant {
                text: "I suggested \"warm\".".to_string(),
                tool_calls: Vec::new(),
            }
        );
    }

    #[test]
    fn edits_in_one_reply_may_not_overlap() {
        let mut talk = Conversation::default();
        talk.ask("Warmer", snapshot("Stay cold."));
        talk.request(&settings(), None).unwrap();
        talk.receive(tool_call(
            "c1",
            "propose_edit",
            r#"{"find":"cold","replace":"warm"}"#,
        ));
        talk.receive(tool_call(
            "c2",
            "propose_edit",
            r#"{"find":"cold.","replace":"hot."}"#,
        ));
        talk.receive(StreamEvent::Done);
        assert_eq!(talk.new_suggestions.len(), 1);
        let Item::ToolResult { output, .. } = &talk.items[3] else {
            panic!("expected a tool result, got {:?}", talk.items[3]);
        };
        assert!(output.contains("overlaps"), "{output}");
    }

    #[test]
    fn a_waiting_suggestion_is_replaced_by_the_next_edit_over_it() {
        let mut talk = Conversation::default();
        let mut waiting = snapshot("Stay cold.");
        waiting.waiting.push(Waiting {
            id: 2,
            range: 5..9,
            replace: "warm".to_string(),
        });
        talk.ask("Hotter", waiting);
        talk.request(&settings(), None).unwrap();
        let edit = r#"{"find":"cold","replace":"hot","why":"Hotter."}"#;
        talk.receive(tool_call("c1", "propose_edit", edit));
        talk.receive(StreamEvent::Done);
        assert_eq!(talk.new_suggestions.len(), 1);
        assert_eq!(talk.withdrawn, [2]);
    }

    #[tokio::test]
    async fn the_text_goes_with_the_newest_question_only() {
        let answer = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Yes.\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{}}\n\n";
        let (base, server) =
            serve(vec![(200, answer.to_string()), (200, answer.to_string())]).await;
        let settings = settings_for(&base, RequestFormat::Responses);
        let mut talk = Conversation::default();
        for (question, text) in [("First?", "old text"), ("Second?", "new text")] {
            talk.ask(question, snapshot(text));
            let reply = Reply::new(talk.request(&settings, None).unwrap());
            for event in all_events(reply).await {
                talk.receive(event);
            }
        }
        let seen = server.await.unwrap();
        let first = &seen[0].body["input"][0]["content"];
        assert!(
            first
                .as_str()
                .unwrap()
                .contains("<document>\nold text\n</document>")
        );
        let input = &seen[1].body["input"];
        let (earlier, newest) = (input[0]["content"].as_str(), input[2]["content"].as_str());
        assert!(!earlier.unwrap().contains("<document>"), "{earlier:?}");
        assert!(
            newest
                .unwrap()
                .contains("<document>\nnew text\n</document>")
        );
        assert!(newest.unwrap().ends_with("Second?"));
    }

    #[test]
    fn the_loop_stops_after_the_most_rounds() {
        let mut talk = Conversation::default();
        talk.ask("Keep looking", snapshot("text"));
        for round in 1..=MAX_ROUNDS {
            talk.request(&settings(), None).unwrap();
            talk.receive(tool_call(&format!("c{round}"), "get_document", "{}"));
            let next = talk.receive(StreamEvent::Done);
            match round == MAX_ROUNDS {
                true => assert_eq!(next, Next::Finished),
                false => assert_eq!(next, Next::SendAgain),
            }
        }
        assert!(!talk.busy);
        assert!(
            talk.error
                .as_deref()
                .unwrap()
                .starts_with("Stopped after 8 rounds")
        );
        // The last call still got its result, and the question an answer after it.
        let last_two = &talk.items[talk.items.len() - 2..];
        assert!(matches!(last_two[0], Item::ToolResult { .. }));
        assert_eq!(
            last_two[1],
            Item::Assistant {
                text: NO_ANSWER.to_string(),
                tool_calls: Vec::new(),
            }
        );

        // A new question starts the count again.
        talk.ask("Go on", snapshot("text"));
        assert_eq!(talk.error, None);
        talk.request(&settings(), None).unwrap();
        talk.receive(tool_call("again", "get_document", "{}"));
        assert_eq!(talk.receive(StreamEvent::Done), Next::SendAgain);
    }

    #[test]
    fn failures_and_stops_keep_the_text_and_drop_unanswered_calls() {
        let mut talk = Conversation::default();
        talk.ask("Hi", snapshot(""));
        talk.request(&settings(), None).unwrap();
        talk.receive(StreamEvent::Text("Half a".into()));
        talk.receive(tool_call("c1", "get_document", "{}"));
        assert_eq!(
            talk.receive(StreamEvent::Failed("Gone.".into())),
            Next::Finished
        );
        assert_eq!(talk.error.as_deref(), Some("Gone."));
        assert_eq!(
            talk.items.last(),
            Some(&Item::Assistant {
                text: "Half a".to_string(),
                tool_calls: Vec::new(),
            })
        );

        talk.ask("Again", snapshot(""));
        talk.request(&settings(), None).unwrap();
        talk.receive(StreamEvent::Text("Some".into()));
        talk.stop();
        assert!(!talk.busy);
        assert_eq!(talk.error, None);
        assert_eq!(talk.items.len(), 4);
        // A piece from a reply that was stopped changes nothing.
        assert_eq!(
            talk.receive(StreamEvent::Text("late".into())),
            Next::Finished
        );
        assert_eq!(talk.reply, "");

        let mut no_model = settings();
        no_model.model.clear();
        talk.ask("Hello?", snapshot(""));
        assert_eq!(
            talk.request(&no_model, None).err().as_deref(),
            Some("Set a model in Settings first.")
        );
        assert!(!talk.busy);
        assert_eq!(
            talk.error.as_deref(),
            Some("Set a model in Settings first.")
        );
        // Nothing came back, so the question leaves the history, for the pane to put back.
        assert_eq!(talk.unanswered.as_deref(), Some("Hello?"));
        assert_eq!(talk.items.len(), 4);
    }

    #[test]
    fn a_question_that_got_nothing_back_never_stays_in_the_history() {
        // Stopped before anything came.
        let mut talk = Conversation::default();
        talk.ask("Hi", snapshot(""));
        talk.request(&settings(), None).unwrap();
        talk.stop();
        assert!(talk.items.is_empty());
        assert_eq!(talk.unanswered.take().as_deref(), Some("Hi"));

        // An empty reply: the question goes, and the pane says why.
        talk.ask("Hi", snapshot(""));
        talk.request(&settings(), None).unwrap();
        assert_eq!(talk.receive(StreamEvent::Done), Next::Finished);
        assert!(talk.items.is_empty());
        assert_eq!(talk.error.as_deref(), Some("The model's reply was empty."));

        // Failed after a tool ran: the question stays, answered so the next one may follow.
        talk.ask("Hi", snapshot(""));
        talk.request(&settings(), None).unwrap();
        talk.receive(tool_call("c1", "get_document", ""));
        assert_eq!(talk.receive(StreamEvent::Done), Next::SendAgain);
        talk.request(&settings(), None).unwrap();
        talk.receive(StreamEvent::Failed("Gone.".into()));
        assert_eq!(talk.items.len(), 4);
        assert_eq!(
            talk.items[3],
            Item::Assistant {
                text: NO_ANSWER.to_string(),
                tool_calls: Vec::new(),
            }
        );
    }

    #[test]
    fn arguments_that_are_not_an_object_are_kept_as_an_empty_one() {
        let mut talk = Conversation::default();
        talk.ask("Hi", snapshot("text"));
        talk.request(&settings(), None).unwrap();
        talk.receive(tool_call("c1", "get_document", ""));
        talk.receive(tool_call("c2", "find_text", "{\"text\": \"te"));
        talk.receive(StreamEvent::Done);
        let Item::Assistant { tool_calls, .. } = &talk.items[1] else {
            panic!("expected the reply")
        };
        assert_eq!(tool_calls[0].arguments, "{}");
        assert_eq!(tool_calls[1].arguments, "{}");
        let Item::ToolResult { output, .. } = &talk.items[3] else {
            panic!("expected a tool result")
        };
        assert_eq!(output, "The arguments weren't a JSON object.");
    }

    #[test]
    fn tools_read_the_newest_snapshot() {
        let mut talk = Conversation::default();
        talk.ask("Make it warmer", snapshot("Stay cold."));
        talk.request(&settings(), None).unwrap();
        let edit = r#"{"find":"cool","replace":"warm","why":"Warmer."}"#;
        talk.receive(tool_call("c1", "propose_edit", edit));
        // The user typed while the reply came: the edit is checked against the text as it is now.
        talk.see(snapshot("Stay cool."));
        assert_eq!(talk.receive(StreamEvent::Done), Next::SendAgain);
        assert_eq!(talk.new_suggestions[0].at, 5);
    }

    #[tokio::test]
    async fn test_connection_gives_the_reply_or_the_reason() {
        let ok = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"OK\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{}}\n\n";
        let refused = r#"{"error":{"message":"Incorrect API key provided."}}"#;
        let (base, server) = serve(vec![(200, ok.to_string()), (401, refused.to_string())]).await;
        let settings = settings_for(&base, RequestFormat::Responses);
        assert_eq!(
            test_connection(&settings, Some("sk-test")).await,
            Ok("OK".to_string())
        );
        let failed = test_connection(&settings, Some("sk-test"))
            .await
            .unwrap_err();
        assert!(failed.starts_with("The server answered 401"), "{failed}");
        // The test offers the tools, so a server that can't take them fails here too.
        let seen = server.await.unwrap();
        assert!(
            seen[0].body["tools"]
                .as_array()
                .is_some_and(|tools| !tools.is_empty())
        );

        let mut no_model = settings.clone();
        no_model.model.clear();
        assert_eq!(
            test_connection(&no_model, None).await.err().as_deref(),
            Some("Set a model in Settings first.")
        );
    }

    /// The whole loop over HTTP: the mock model asks for a tool, then answers; the second
    /// request must carry the call and its result.
    async fn full_loop(format: RequestFormat, first: &str, second: &str) -> Vec<serde_json::Value> {
        let (base, server) = serve(vec![(200, first.to_string()), (200, second.to_string())]).await;
        let settings = settings_for(&base, format);
        let mut talk = Conversation::default();
        talk.ask("Where is warm?", snapshot("cold\nwarm"));
        loop {
            let reply = Reply::new(talk.request(&settings, Some("sk-test")).unwrap());
            let mut next = Next::Wait;
            for event in all_events(reply).await {
                next = talk.receive(event);
            }
            if next != Next::SendAgain {
                break;
            }
        }
        assert_eq!(talk.error, None);
        assert_eq!(
            talk.items.last(),
            Some(&Item::Assistant {
                text: "Line 2.".to_string(),
                tool_calls: Vec::new(),
            })
        );
        server
            .await
            .unwrap()
            .into_iter()
            .map(|seen| seen.body)
            .collect()
    }

    #[tokio::test]
    async fn the_loop_runs_over_http_in_the_responses_format() {
        let first = "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_W\",\"name\":\"find_text\",\"arguments\":\"{\\\"text\\\":\\\"warm\\\"}\"}}\n\n\
data: {\"type\":\"response.completed\",\"response\":{}}\n\n";
        let second = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Line 2.\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{}}\n\n";
        let bodies = full_loop(RequestFormat::Responses, first, second).await;
        let input = &bodies[1]["input"];
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_W");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_W");
        assert!(input[2]["output"].as_str().unwrap().contains("\"line\":2"));
        assert_eq!(bodies[1]["instructions"], bodies[0]["instructions"]);
    }

    #[tokio::test]
    async fn the_loop_runs_over_http_in_the_chat_format() {
        let first = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_C\",\"function\":{\"name\":\"find_text\",\"arguments\":\"{\\\"text\\\":\\\"warm\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n\
data: [DONE]\n\n";
        let second = "data: {\"choices\":[{\"delta\":{\"content\":\"Line 2.\"},\"finish_reason\":\"stop\"}]}\n\n\
data: [DONE]\n\n";
        let bodies = full_loop(RequestFormat::ChatCompletions, first, second).await;
        let messages = &bodies[1]["messages"];
        assert_eq!(messages[2]["tool_calls"][0]["id"], "call_C");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "call_C");
        assert!(
            messages[3]["content"]
                .as_str()
                .unwrap()
                .contains("\"line\":2")
        );
    }
}
