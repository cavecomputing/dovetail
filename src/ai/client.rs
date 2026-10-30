//! Talking to the assistant's server: any OpenAI-compatible API, in either of OpenAI's two request
//! formats, with the reply streamed back as server-sent events.
//!
//! - Responses: `POST {base_url}/responses`. The whole conversation goes as `input` items every
//!   time, so nothing relies on `previous_response_id`, which many compatible servers lack.
//! - Chat Completions: `POST {base_url}/chat/completions`, with `messages`.
//!
//! Replies stream as server-sent events: `sse::SseParser` splits the bytes into events and
//! `Decoder` turns each format's events into `StreamEvent`s. No SDK: a few JSON shapes do.

use std::collections::VecDeque;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::ai::sse::{SseEvent, SseParser};
use crate::ai::tools::Tool;
use crate::core::settings::{DEFAULT_BASE_URL, RequestFormat, Settings};

/// How long the server may take to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The longest silence allowed while a reply streams. Reasoning models can think quietly for a
/// while before their first word.
const READ_TIMEOUT: Duration = Duration::from_secs(300);
/// The most text and tool arguments one reply may bring, and the most bytes read for it. A real
/// reply is a few hundred KB at most; past these the server has run away, and the pane would
/// need gigabytes to lay the text out.
const MAX_REPLY_TEXT: usize = 1_000_000;
const MAX_REPLY_BYTES: usize = 64_000_000;

/// Numbers tool calls that arrive without an id, so every call can be paired with its result.
static UNNAMED_CALLS: AtomicU64 = AtomicU64::new(0);

/// One thing said in a conversation, in the shape both formats share.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// What the user typed, and the app's note about the document at that moment (sent with it,
    /// not shown in the chat).
    User { text: String, note: String },
    /// One reply from the model: its text (maybe empty) and the tools it asked to run.
    Assistant {
        text: String,
        tool_calls: Vec<ToolCall>,
    },
    /// What a tool gave back, for the call with this id.
    ToolResult { call_id: String, output: String },
}

/// The model asking the app to run one tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// Pairs the call with its result.
    pub call_id: String,
    pub name: String,
    /// The arguments as the model wrote them: a JSON object, as text.
    pub arguments: String,
}

/// A piece of a streaming reply.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// The next piece of the reply's text.
    Text(String),
    /// A tool the model wants run, with all its arguments. These come after the text, just
    /// before `Done`.
    ToolCall(ToolCall),
    /// The reply is complete.
    Done,
    /// The request failed or the reply broke off, in plain words. Nothing follows it.
    Failed(String),
}

/// One request, ready to send: where it goes, the key, and the JSON body.
#[derive(Clone)]
pub struct Request {
    url: String,
    api_key: Option<String>,
    format: RequestFormat,
    body: Value,
}

impl Request {
    /// The request for the next reply to `items`, in the format `settings` asks for. Fails, in
    /// words for the pane, when no model is set.
    pub fn new(
        settings: &Settings,
        api_key: Option<&str>,
        instructions: &str,
        tools: &[Tool],
        items: &[Item],
    ) -> Result<Request, String> {
        let model = settings.model.trim();
        if model.is_empty() {
            return Err("Set a model in Settings first.".to_string());
        }
        let base = match settings.base_url.trim().trim_end_matches('/') {
            "" => DEFAULT_BASE_URL,
            base => base,
        };
        let (path, body) = match settings.request_format {
            RequestFormat::Responses => (
                "responses",
                responses_body(model, instructions, tools, items),
            ),
            RequestFormat::ChatCompletions => (
                "chat/completions",
                chat_body(model, instructions, tools, items),
            ),
        };
        Ok(Request {
            url: format!("{base}/{path}"),
            api_key: api_key
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_string),
            format: settings.request_format,
            body,
        })
    }
}

/// The user's text with the app's note in front of it.
fn with_note(text: &str, note: &str) -> String {
    if note.is_empty() {
        text.to_string()
    } else {
        format!("{note}\n\n{text}")
    }
}

/// A Responses API body. Tool calls go back as `function_call` items without their server `id`:
/// OpenAI refuses an id whose reasoning item isn't sent along, and the `call_id` is what pairs a
/// call with its `function_call_output`.
fn responses_body(model: &str, instructions: &str, tools: &[Tool], items: &[Item]) -> Value {
    let mut input = Vec::new();
    for item in items {
        match item {
            Item::User { text, note } => {
                input.push(json!({ "role": "user", "content": with_note(text, note) }));
            }
            Item::Assistant { text, tool_calls } => {
                if !text.is_empty() {
                    input.push(json!({ "role": "assistant", "content": text }));
                }
                for call in tool_calls {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call.call_id,
                        "name": call.name,
                        "arguments": call.arguments,
                    }));
                }
            }
            Item::ToolResult { call_id, output } => {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
            }
        }
    }
    // Not strict: strict schemas can't have optional arguments, and Responses makes functions
    // strict unless told otherwise.
    let tools: Vec<Value> = tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
                "strict": false,
            })
        })
        .collect();
    json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "tools": tools,
        "stream": true,
        "store": false,
    })
}

/// A Chat Completions body: the instructions as the system message, each reply's tool calls on
/// its assistant message, and each result as a `tool` message.
fn chat_body(model: &str, instructions: &str, tools: &[Tool], items: &[Item]) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": instructions })];
    for item in items {
        let message = match item {
            Item::User { text, note } => {
                json!({ "role": "user", "content": with_note(text, note) })
            }
            Item::Assistant { text, tool_calls } if tool_calls.is_empty() => {
                json!({ "role": "assistant", "content": text })
            }
            Item::Assistant { text, tool_calls } => {
                let calls: Vec<Value> = tool_calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.call_id,
                            "type": "function",
                            "function": { "name": call.name, "arguments": call.arguments },
                        })
                    })
                    .collect();
                let content = match text.is_empty() {
                    true => Value::Null,
                    false => json!(text),
                };
                json!({ "role": "assistant", "content": content, "tool_calls": calls })
            }
            Item::ToolResult { call_id, output } => {
                json!({ "role": "tool", "tool_call_id": call_id, "content": output })
            }
        };
        messages.push(message);
    }
    let tools: Vec<Value> = tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                },
            })
        })
        .collect();
    json!({ "model": model, "messages": messages, "tools": tools, "stream": true })
}

/// A reply being streamed. The first `next` sends the request; after that each `next` hands out
/// the next piece. Dropping the `Reply` closes the connection, which cancels the reply.
pub struct Reply {
    request: Option<Request>,
    /// Kept to blank it out of error messages: some servers repeat a wrong key.
    api_key: Option<String>,
    response: Option<reqwest::Response>,
    parser: SseParser,
    decoder: Decoder,
    ready: VecDeque<StreamEvent>,
    /// Bytes read so far, checked against `MAX_REPLY_BYTES`.
    received: usize,
    ended: bool,
}

impl Reply {
    pub fn new(request: Request) -> Reply {
        Reply {
            decoder: Decoder::new(request.format),
            api_key: request.api_key.clone(),
            request: Some(request),
            response: None,
            parser: SseParser::default(),
            ready: VecDeque::new(),
            received: 0,
            ended: false,
        }
    }

    /// The next piece of the reply, or `None` after `Done` or `Failed`.
    pub async fn next(&mut self) -> Option<StreamEvent> {
        loop {
            if let Some(mut event) = self.ready.pop_front() {
                if let StreamEvent::Failed(reason) = &mut event
                    && let Some(key) = &self.api_key
                {
                    *reason = reason.replace(key.as_str(), "…");
                }
                if matches!(event, StreamEvent::Done | StreamEvent::Failed(_)) {
                    self.ended = true;
                    self.response = None;
                }
                return Some(event);
            }
            if self.ended {
                return None;
            }
            self.read().await;
        }
    }

    /// Sends the request, or reads the next chunk of the reply, queueing what it completes.
    async fn read(&mut self) {
        if let Some(request) = self.request.take() {
            match send(&request).await {
                Ok(response) => self.response = Some(response),
                Err(reason) => self.ready.push_back(StreamEvent::Failed(reason)),
            }
            return;
        }
        let Some(response) = self.response.as_mut() else {
            self.ended = true;
            return;
        };
        match response.chunk().await {
            Ok(Some(bytes)) => {
                self.received += bytes.len();
                if self.received > MAX_REPLY_BYTES {
                    self.ready
                        .extend(self.decoder.fail(too_long("64MB of events")));
                    return;
                }
                for event in self.parser.feed(&bytes) {
                    self.ready.extend(self.decoder.decode(&event));
                }
            }
            Ok(None) => {
                if let Some(event) = self.parser.finish() {
                    self.ready.extend(self.decoder.decode(&event));
                }
                self.ready.extend(self.decoder.end());
            }
            Err(error) => {
                let reason = format!("The reply broke off. {}", network_words(&error));
                self.ready.push_back(StreamEvent::Failed(reason));
            }
        }
    }
}

/// Sends `request` and returns the response once the server has accepted it, or why not.
async fn send(request: &Request) -> Result<reqwest::Response, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        .map_err(|error| format!("Couldn't start a connection: {}", innermost(&error)))?;
    let mut post = client
        .post(&request.url)
        .header("Accept", "text/event-stream")
        .json(&request.body);
    if let Some(key) = &request.api_key {
        post = post.bearer_auth(key);
    }
    let response = post.send().await.map_err(|error| network_words(&error))?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(http_error_words(status, &body))
}

/// A failed status in plain words: the server's own message when its body has one, and a hint
/// for the usual mistakes.
fn http_error_words(status: StatusCode, body: &str) -> String {
    let message = match serde_json::from_str::<Value>(body) {
        Ok(json) => error_message(&json).unwrap_or_default(),
        // An HTML error page says nothing useful in a sentence.
        Err(_) if body.trim_start().starts_with('<') => String::new(),
        Err(_) => body.trim().chars().take(300).collect(),
    };
    let reason = status.canonical_reason().unwrap_or("error");
    let mut words = format!("The server answered {} {reason}", status.as_u16());
    if !message.is_empty() {
        words.push_str(": ");
        words.push_str(message.trim_end_matches('.'));
    }
    words.push('.');
    words.push_str(match status.as_u16() {
        401 | 403 => " Check the API key in Settings.",
        404 => " Check the base URL, model and request format in Settings.",
        429 => " Too many requests or out of credit: try again in a moment.",
        _ => "",
    });
    words
}

/// The message in an error body or event: `{"error": {"message": …}}`, `{"error": "…"}` or
/// `{"message": …}`.
fn error_message(json: &Value) -> Option<String> {
    let error = json.get("error").unwrap_or(json);
    match error {
        Value::String(text) => Some(text.clone()),
        _ => error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// A network failure in plain words.
fn network_words(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "The server stopped answering (timed out).".to_string();
    }
    let host = error
        .url()
        .and_then(|url| url.host_str())
        .unwrap_or("the server");
    if error.is_connect() {
        format!("Couldn't reach {host}: {}", innermost(error))
    } else {
        format!("The connection to {host} failed: {}", innermost(error))
    }
}

/// The deepest cause of an error, which says what actually went wrong ("Connection refused").
fn innermost(error: &dyn Error) -> String {
    let mut cause = error;
    while let Some(source) = cause.source() {
        cause = source;
    }
    cause.to_string()
}

fn too_long(what: &str) -> String {
    format!("The reply was cut off: it went past {what}, far more than a real reply.")
}

/// Turns one format's events into `StreamEvent`s. Text goes out as it comes; tool calls are
/// collected piece by piece and go out together when the reply finishes.
struct Decoder {
    format: RequestFormat,
    /// Tool calls so far, each with the key its later pieces name it by.
    calls: Vec<(String, ToolCall)>,
    /// Whether any event made sense, to tell a stream from some other answer.
    heard: bool,
    /// Bytes of text so far, checked with the tool arguments against `MAX_REPLY_TEXT`.
    text: usize,
    /// Set once the reply finished or failed; anything after is ignored.
    finished: bool,
}

impl Decoder {
    fn new(format: RequestFormat) -> Decoder {
        Decoder {
            format,
            calls: Vec::new(),
            heard: false,
            text: 0,
            finished: false,
        }
    }

    fn decode(&mut self, event: &SseEvent) -> Vec<StreamEvent> {
        if self.finished {
            return Vec::new();
        }
        // OpenAI ends chat streams this way; some servers end Responses streams this way too.
        if event.data.trim() == "[DONE]" {
            return self.finish();
        }
        let Ok(json) = serde_json::from_str::<Value>(&event.data) else {
            return Vec::new();
        };
        self.heard = true;
        let events = match self.format {
            RequestFormat::Responses => self.responses_event(&event.event, &json),
            RequestFormat::ChatCompletions => self.chat_event(&json),
        };
        self.text += events
            .iter()
            .map(|event| match event {
                StreamEvent::Text(text) => text.len(),
                _ => 0,
            })
            .sum::<usize>();
        let arguments: usize = self
            .calls
            .iter()
            .map(|(_, call)| call.arguments.len())
            .sum();
        if self.text + arguments > MAX_REPLY_TEXT {
            return self.fail(too_long("1MB"));
        }
        events
    }

    /// The stream ended. A reply that never said it finished is taken as finished, since the
    /// server closed it properly (a cut connection is an error before this).
    fn end(&mut self) -> Vec<StreamEvent> {
        if self.finished {
            return Vec::new();
        }
        if !self.heard {
            return self.fail(
                "The server's answer wasn't a stream of events. Check the base URL and request \
                 format in Settings."
                    .to_string(),
            );
        }
        self.finish()
    }

    /// Responses events. The type is in the JSON's `type`, and usually in `event:` too.
    fn responses_event(&mut self, name: &str, json: &Value) -> Vec<StreamEvent> {
        let kind = json["type"].as_str().unwrap_or(name);
        match kind {
            "response.output_text.delta" => text_event(&json["delta"]),
            "response.output_item.added" | "response.output_item.done"
                if json["item"]["type"] == "function_call" =>
            {
                let item = &json["item"];
                let call = self.call(item_key(&item["id"], &json["output_index"]));
                if let Some(call_id) = item["call_id"].as_str() {
                    call.call_id = call_id.to_string();
                }
                if let Some(name) = item["name"].as_str() {
                    call.name = name.to_string();
                }
                match item["arguments"].as_str() {
                    Some(arguments) if !arguments.is_empty() => {
                        call.arguments = arguments.to_string()
                    }
                    _ => {}
                }
                Vec::new()
            }
            "response.function_call_arguments.delta" => {
                let call = self.call(item_key(&json["item_id"], &json["output_index"]));
                call.arguments
                    .push_str(json["delta"].as_str().unwrap_or(""));
                Vec::new()
            }
            "response.function_call_arguments.done" => {
                let call = self.call(item_key(&json["item_id"], &json["output_index"]));
                if let Some(arguments) = json["arguments"].as_str() {
                    call.arguments = arguments.to_string();
                }
                Vec::new()
            }
            "response.completed" => self.finish(),
            "response.incomplete" => {
                let reason = json["response"]["incomplete_details"]["reason"]
                    .as_str()
                    .unwrap_or("no reason given");
                self.fail(format!("The reply stopped early ({reason})."))
            }
            "response.failed" => {
                let message = error_message(&json["response"])
                    .unwrap_or_else(|| "no reason given".to_string());
                self.fail(format!("The server couldn't finish the reply: {message}"))
            }
            "error" => {
                let message = error_message(json).unwrap_or_else(|| "no reason given".to_string());
                self.fail(format!("The server sent an error: {message}"))
            }
            _ => Vec::new(),
        }
    }

    /// Chat Completions chunks: text in `choices[0].delta.content`, tool calls in pieces under
    /// `delta.tool_calls` by `index`, and `finish_reason` once it's done.
    fn chat_event(&mut self, json: &Value) -> Vec<StreamEvent> {
        if json.get("error").is_some() {
            let message = error_message(json).unwrap_or_else(|| "no reason given".to_string());
            return self.fail(format!("The server sent an error: {message}"));
        }
        // A chunk with no choices carries only usage.
        let Some(choice) = json["choices"].get(0) else {
            return Vec::new();
        };
        let delta = &choice["delta"];
        let mut events = text_event(&delta["content"]);
        events.extend(text_event(&delta["refusal"]));
        for (position, piece) in delta["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            // Some servers leave out `index`; then the id, or the place in this chunk, says
            // which call a piece belongs to.
            let key = match (&piece["index"], &piece["id"]) {
                (Value::Number(index), _) => format!("#{index}"),
                (_, Value::String(id)) => id.clone(),
                _ => format!("#{position}"),
            };
            let call = self.call(key);
            if let Some(id) = piece["id"].as_str() {
                call.call_id = id.to_string();
            }
            if let Some(name) = piece["function"]["name"].as_str() {
                call.name.push_str(name);
            }
            if let Some(arguments) = piece["function"]["arguments"].as_str() {
                call.arguments.push_str(arguments);
            }
        }
        match choice["finish_reason"].as_str() {
            Some("length") => {
                events.extend(self.fail("The reply stopped early (length limit).".to_string()))
            }
            // Any other reason ends the reply. Some servers say "stop" even after tool calls.
            Some(_) => events.extend(self.finish()),
            None => {}
        }
        events
    }

    /// The call that pieces name by `key`, started if new.
    fn call(&mut self, key: String) -> &mut ToolCall {
        let at = match self.calls.iter().position(|(known, _)| *known == key) {
            Some(at) => at,
            None => {
                let call = ToolCall {
                    call_id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                };
                self.calls.push((key, call));
                self.calls.len() - 1
            }
        };
        &mut self.calls[at].1
    }

    /// The reply is complete: hands out its tool calls, then `Done`.
    fn finish(&mut self) -> Vec<StreamEvent> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut events: Vec<StreamEvent> = std::mem::take(&mut self.calls)
            .into_iter()
            .map(|(_, mut call)| {
                if call.call_id.is_empty() {
                    let number = UNNAMED_CALLS.fetch_add(1, Ordering::Relaxed);
                    call.call_id = format!("call_{number}");
                }
                StreamEvent::ToolCall(call)
            })
            .collect();
        events.push(StreamEvent::Done);
        events
    }

    fn fail(&mut self, reason: String) -> Vec<StreamEvent> {
        self.finished = true;
        vec![StreamEvent::Failed(reason)]
    }
}

/// How Responses events name an output item: by its id, or by its place when it has none.
fn item_key(id: &Value, index: &Value) -> String {
    match id {
        Value::String(id) => id.clone(),
        _ => format!("#{index}"),
    }
}

/// A text piece, if `value` is a non-empty string.
fn text_event(value: &Value) -> Vec<StreamEvent> {
    match value.as_str() {
        Some(text) if !text.is_empty() => vec![StreamEvent::Text(text.to_string())],
        _ => Vec::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// What the mock server saw of one request.
    #[derive(Debug)]
    pub(crate) struct Seen {
        pub path: String,
        pub authorization: Option<String>,
        pub body: Value,
    }

    /// A tiny HTTP server on a free local port. Each connection gets the next canned
    /// `(status, body)`, written a few bytes at a time so the client meets chunk boundaries.
    /// Returns its base URL (with `/v1`) and a handle that yields what it saw.
    pub(crate) async fn serve(
        replies: Vec<(u16, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<Seen>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, body) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                seen.push(read_request(&mut socket).await);
                let kind = match status {
                    200 => "text/event-stream",
                    _ => "application/json",
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nConnection: close\r\n\r\n"
                );
                socket.write_all(head.as_bytes()).await.unwrap();
                for piece in body.as_bytes().chunks(7) {
                    socket.write_all(piece).await.unwrap();
                    socket.flush().await.unwrap();
                }
                socket.shutdown().await.unwrap();
            }
            seen
        });
        (base, handle)
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> Seen {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        let head_end = loop {
            let read = socket.read(&mut buffer).await.unwrap();
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(at) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&bytes[..head_end]).to_string();
        let header = |name: &str| {
            head.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_string())
            })
        };
        let length: usize = header("content-length").unwrap().parse().unwrap();
        while bytes.len() < head_end + length {
            let read = socket.read(&mut buffer).await.unwrap();
            bytes.extend_from_slice(&buffer[..read]);
        }
        Seen {
            path: head.split_whitespace().nth(1).unwrap().to_string(),
            authorization: header("authorization"),
            body: serde_json::from_slice(&bytes[head_end..head_end + length]).unwrap(),
        }
    }

    pub(crate) fn settings_for(base: &str, format: RequestFormat) -> Settings {
        Settings {
            base_url: base.to_string(),
            model: "test-model".to_string(),
            request_format: format,
            ..Settings::default()
        }
    }

    pub(crate) async fn all_events(mut reply: Reply) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        while let Some(event) = reply.next().await {
            events.push(event);
        }
        events
    }

    fn user(text: &str) -> Item {
        Item::User {
            text: text.to_string(),
            note: String::new(),
        }
    }

    fn call(call_id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            call_id: call_id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    fn text(piece: &str) -> StreamEvent {
        StreamEvent::Text(piece.to_string())
    }

    fn find_tool() -> Tool {
        Tool {
            name: "find_text",
            description: "Find text.",
            parameters: json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
        }
    }

    /// A Responses stream as OpenAI sends it: text, then a function call whose arguments come
    /// in pieces.
    pub(crate) const RESPONSES_STREAM: &str = "event: response.created\n\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n\
event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"delta\":\"Let me \"}\n\n\
event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"delta\":\"look. ü\"}\n\n\
event: response.output_item.added\n\
data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_A\",\"name\":\"find_text\",\"arguments\":\"\"}}\n\n\
event: response.function_call_arguments.delta\n\
data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":1,\"delta\":\"{\\\"text\\\":\"}\n\n\
: keep-alive\n\n\
event: response.function_call_arguments.delta\n\
data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":1,\"delta\":\"\\\"warm\\\"}\"}\n\n\
event: response.function_call_arguments.done\n\
data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"output_index\":1,\"arguments\":\"{\\\"text\\\":\\\"warm\\\"}\"}\n\n\
event: response.output_item.done\n\
data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_A\",\"name\":\"find_text\",\"arguments\":\"{\\\"text\\\":\\\"warm\\\"}\"}}\n\n\
event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n";

    /// A Chat Completions stream with two tool calls whose pieces interleave by index.
    pub(crate) const CHAT_STREAM: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Checking\"},\"finish_reason\":null}]}\r\n\r\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"find_text\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\r\n\r\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_2\",\"type\":\"function\",\"function\":{\"name\":\"count_tokens\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\r\n\r\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"text\\\":\"}}]},\"finish_reason\":null}]}\r\n\r\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"warm\\\"}\"}}]},\"finish_reason\":null}]}\r\n\r\n\
data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\r\n\r\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9}}\r\n\r\n\
data: [DONE]\r\n\r\n";

    /// Every event in `stream`, decoded as `format` without a network.
    fn decoded(format: RequestFormat, chunks: &[&[u8]]) -> Vec<StreamEvent> {
        let mut parser = SseParser::default();
        let mut decoder = Decoder::new(format);
        let mut events = Vec::new();
        for chunk in chunks {
            for event in parser.feed(chunk) {
                events.extend(decoder.decode(&event));
            }
        }
        if let Some(event) = parser.finish() {
            events.extend(decoder.decode(&event));
        }
        events.extend(decoder.end());
        events
    }

    #[test]
    fn a_reply_past_a_megabyte_is_cut_off() {
        let delta = |size: usize| {
            format!(
                "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{}\"}}\n\n",
                "x".repeat(size)
            )
        };
        let stream = format!(
            "{}{}data: {{\"type\":\"response.completed\"}}\n\n",
            delta(600_000),
            delta(500_000)
        );
        let events = decoded(RequestFormat::Responses, &[stream.as_bytes()]);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], text(&"x".repeat(600_000)));
        assert_eq!(
            events[1],
            StreamEvent::Failed(
                "The reply was cut off: it went past 1MB, far more than a real reply.".to_string()
            )
        );
        // Tool arguments count too: one call's `replace` of 1MB is refused with it.
        let call = format!(
            "data: {{\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"propose_edit\",\"arguments\":\"{}\"}}}}\n\n",
            "y".repeat(1_000_001)
        );
        let events = decoded(RequestFormat::Responses, &[call.as_bytes()]);
        assert!(matches!(events.as_slice(), [StreamEvent::Failed(_)]));
    }

    #[test]
    fn responses_events_decode_to_text_then_the_tool_call() {
        let events = decoded(RequestFormat::Responses, &[RESPONSES_STREAM.as_bytes()]);
        assert_eq!(
            events,
            vec![
                text("Let me "),
                text("look. ü"),
                StreamEvent::ToolCall(call("call_A", "find_text", r#"{"text":"warm"}"#)),
                StreamEvent::Done,
            ]
        );
    }

    #[test]
    fn responses_tool_calls_build_from_deltas_alone() {
        // A server that sends no `.done` events, and no `event:` lines.
        let stream = "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_9\",\"type\":\"function_call\",\"call_id\":\"call_9\",\"name\":\"get_document\"}}\n\n\
data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_9\",\"output_index\":0,\"delta\":\"{}\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{}}\n\n\
data: [DONE]\n\n";
        assert_eq!(
            decoded(RequestFormat::Responses, &[stream.as_bytes()]),
            vec![
                StreamEvent::ToolCall(call("call_9", "get_document", "{}")),
                StreamEvent::Done,
            ]
        );
    }

    #[test]
    fn chat_chunks_decode_with_tool_calls_by_index() {
        let events = decoded(RequestFormat::ChatCompletions, &[CHAT_STREAM.as_bytes()]);
        assert_eq!(
            events,
            vec![
                text("Checking"),
                StreamEvent::ToolCall(call("call_1", "find_text", r#"{"text":"warm"}"#)),
                StreamEvent::ToolCall(call("call_2", "count_tokens", "{}")),
                StreamEvent::Done,
            ]
        );
    }

    #[test]
    fn chat_calls_without_an_id_get_one() {
        let stream = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"get_selection\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"stop\"}]}\n\n";
        let events = decoded(RequestFormat::ChatCompletions, &[stream.as_bytes()]);
        match &events[0] {
            StreamEvent::ToolCall(call) => {
                assert_eq!(call.name, "get_selection");
                assert!(call.call_id.starts_with("call_"));
            }
            other => panic!("expected a tool call, got {other:?}"),
        }
        assert_eq!(events[1], StreamEvent::Done);
    }

    #[test]
    fn errors_in_the_stream_are_reported_in_words() {
        let failed = |format, stream: &str| match decoded(format, &[stream.as_bytes()]).pop() {
            Some(StreamEvent::Failed(reason)) => reason,
            other => panic!("expected a failure, got {other:?}"),
        };
        let reason = failed(
            RequestFormat::Responses,
            "event: error\ndata: {\"type\":\"error\",\"code\":\"x\",\"message\":\"Overloaded\"}\n\n",
        );
        assert_eq!(reason, "The server sent an error: Overloaded");
        let reason = failed(
            RequestFormat::Responses,
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"Boom\"}}}\n\n",
        );
        assert!(reason.contains("Boom"));
        let reason = failed(
            RequestFormat::Responses,
            "data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
        );
        assert!(reason.contains("max_output_tokens"));
        let reason = failed(
            RequestFormat::ChatCompletions,
            "data: {\"error\":{\"message\":\"Rate limited\"}}\n\n",
        );
        assert!(reason.contains("Rate limited"));
        let reason = failed(RequestFormat::ChatCompletions, "{\"not\":\"a stream\"}");
        assert!(reason.contains("wasn't a stream"));
    }

    #[test]
    fn a_stream_closed_without_a_finish_still_ends() {
        let stream = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}";
        assert_eq!(
            decoded(RequestFormat::ChatCompletions, &[stream.as_bytes()]),
            vec![text("Hi"), StreamEvent::Done]
        );
    }

    #[test]
    fn bodies_carry_tool_calls_and_results_in_each_format() {
        let items = vec![
            Item::User {
                text: "Find warm".to_string(),
                note: "[note]".to_string(),
            },
            Item::Assistant {
                text: String::new(),
                tool_calls: vec![call("call_1", "find_text", r#"{"text":"warm"}"#)],
            },
            Item::ToolResult {
                call_id: "call_1".to_string(),
                output: "line 3".to_string(),
            },
            Item::Assistant {
                text: "On line 3.".to_string(),
                tool_calls: Vec::new(),
            },
        ];
        let tools = [find_tool()];

        let body = responses_body("m", "Be kind.", &tools, &items);
        assert_eq!(body["instructions"], "Be kind.");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(
            body["input"],
            json!([
                { "role": "user", "content": "[note]\n\nFind warm" },
                { "type": "function_call", "call_id": "call_1", "name": "find_text", "arguments": "{\"text\":\"warm\"}" },
                { "type": "function_call_output", "call_id": "call_1", "output": "line 3" },
                { "role": "assistant", "content": "On line 3." },
            ])
        );
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "find_text");
        assert_eq!(body["tools"][0]["strict"], false);
        assert_eq!(body["tools"][0]["parameters"]["type"], "object");

        let body = chat_body("m", "Be kind.", &tools, &items);
        assert_eq!(
            body["messages"],
            json!([
                { "role": "system", "content": "Be kind." },
                { "role": "user", "content": "[note]\n\nFind warm" },
                { "role": "assistant", "content": null, "tool_calls": [
                    { "id": "call_1", "type": "function", "function": { "name": "find_text", "arguments": "{\"text\":\"warm\"}" } }
                ] },
                { "role": "tool", "tool_call_id": "call_1", "content": "line 3" },
                { "role": "assistant", "content": "On line 3." },
            ])
        );
        assert_eq!(body["tools"][0]["function"]["name"], "find_text");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn a_request_needs_a_model_and_joins_the_url_tidily() {
        let mut settings = settings_for("http://localhost:11434/v1/ ", RequestFormat::Responses);
        let request = Request::new(&settings, Some(" "), "", &[], &[]).unwrap();
        assert_eq!(request.url, "http://localhost:11434/v1/responses");
        assert_eq!(request.api_key, None);

        settings.request_format = RequestFormat::ChatCompletions;
        settings.base_url = String::new();
        let request = Request::new(&settings, None, "", &[], &[]).unwrap();
        assert_eq!(request.url, format!("{DEFAULT_BASE_URL}/chat/completions"));

        settings.model = "  ".to_string();
        let error = Request::new(&settings, None, "", &[], &[]).err().unwrap();
        assert_eq!(error, "Set a model in Settings first.");
    }

    #[tokio::test]
    async fn streams_a_responses_reply_over_http() {
        let (base, server) = serve(vec![(200, RESPONSES_STREAM.to_string())]).await;
        let settings = settings_for(&format!("{base}/"), RequestFormat::Responses);
        let request = Request::new(
            &settings,
            Some("sk-test"),
            "Be brief.",
            &[find_tool()],
            &[user("hi")],
        )
        .unwrap();
        let events = all_events(Reply::new(request)).await;
        assert_eq!(events.len(), 4);
        assert_eq!(events[0], text("Let me "));
        assert_eq!(
            events[2],
            StreamEvent::ToolCall(call("call_A", "find_text", r#"{"text":"warm"}"#))
        );
        assert_eq!(events[3], StreamEvent::Done);

        let seen = server.await.unwrap();
        assert_eq!(seen[0].path, "/v1/responses");
        assert_eq!(seen[0].authorization.as_deref(), Some("Bearer sk-test"));
        assert_eq!(seen[0].body["model"], "test-model");
        assert_eq!(seen[0].body["input"][0]["content"], "hi");
    }

    #[tokio::test]
    async fn streams_a_chat_reply_over_http_without_a_key() {
        let (base, server) = serve(vec![(200, CHAT_STREAM.to_string())]).await;
        let settings = settings_for(&base, RequestFormat::ChatCompletions);
        let request =
            Request::new(&settings, None, "Be brief.", &[find_tool()], &[user("hi")]).unwrap();
        let events = all_events(Reply::new(request)).await;
        assert_eq!(events.len(), 4);
        assert_eq!(events[0], text("Checking"));
        assert_eq!(events[3], StreamEvent::Done);

        let seen = server.await.unwrap();
        assert_eq!(seen[0].path, "/v1/chat/completions");
        assert_eq!(seen[0].authorization, None);
        assert_eq!(seen[0].body["messages"][0]["role"], "system");
    }

    #[tokio::test]
    async fn error_bodies_become_plain_words_without_the_key() {
        let body = r#"{"error":{"message":"Incorrect API key provided: sk-secret-123.","type":"invalid_request_error"}}"#;
        let (base, _server) = serve(vec![(401, body.to_string())]).await;
        let settings = settings_for(&base, RequestFormat::Responses);
        let request = Request::new(&settings, Some("sk-secret-123"), "", &[], &[]).unwrap();
        let events = all_events(Reply::new(request)).await;
        let [StreamEvent::Failed(reason)] = events.as_slice() else {
            panic!("expected one failure, got {events:?}");
        };
        assert_eq!(
            reason,
            "The server answered 401 Unauthorized: Incorrect API key provided: …. Check the API \
             key in Settings."
        );
        assert!(!reason.contains("sk-secret"));

        let (base, _server) = serve(vec![(404, "Not here".to_string())]).await;
        let settings = settings_for(&base, RequestFormat::ChatCompletions);
        let request = Request::new(&settings, None, "", &[], &[]).unwrap();
        let events = all_events(Reply::new(request)).await;
        assert_eq!(
            events,
            vec![StreamEvent::Failed(
                "The server answered 404 Not Found: Not here. Check the base URL, model and \
                 request format in Settings."
                    .to_string()
            )]
        );
    }

    #[tokio::test]
    async fn an_unreachable_server_is_reported() {
        // Bind and drop, so nothing listens on the port.
        let port = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let settings = settings_for(
            &format!("http://127.0.0.1:{port}/v1"),
            RequestFormat::Responses,
        );
        let request = Request::new(&settings, None, "", &[], &[]).unwrap();
        let events = all_events(Reply::new(request)).await;
        let [StreamEvent::Failed(reason)] = events.as_slice() else {
            panic!("expected one failure, got {events:?}");
        };
        assert!(reason.starts_with("Couldn't reach 127.0.0.1"), "{reason}");
    }
}
