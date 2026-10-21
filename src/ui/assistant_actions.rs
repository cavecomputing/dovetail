//! The assistant half of `App::update`: opening the pane, asking, streaming each reply and
//! running its tools (the loop itself is `ai::agent`), showing proposed edits as suggestions in
//! the document, and the Settings panel's API key and connection test.

use iced::Task;
use iced::widget::operation::{focus, snap_to_end};
use iced::widget::{scrollable, text_editor};

use crate::ai::agent::{Conversation, Next, test_connection};
use crate::ai::client::{Item, Reply, StreamEvent};
use crate::ai::context::{self, DocKind, Snapshot};
use crate::ai::key::{KeyStore, api_key, key_store, set_api_key};
use crate::ai::tools::Suggestion;
use crate::core::opened::Doc;
use crate::core::syntax::Syntax;
use crate::ui::app::{App, Message};
use crate::ui::editor::{self, Decision};
use crate::ui::settings_panel::Connection;
use crate::ui::status_bar;

pub const COMPOSER_ID: &str = "assistant-composer";
pub const CHAT_ID: &str = "assistant-chat";

#[derive(Debug, Clone)]
pub enum AssistantAction {
    /// The status bar's toggle, or Ctrl+L: opens or closes the pane.
    Toggle,
    /// Typing in the composer.
    Composer(text_editor::Action),
    /// Enter in the composer.
    Send,
    Stop,
    /// The header's "new chat": stops the reply, rejects the waiting suggestions and starts over.
    NewChat,
    /// The chat scrolled, by the reader or because the reply grew.
    Scrolled(scrollable::Viewport),
    /// The API key, read for the question with this number.
    KeyRead(u64, Result<Option<String>, String>),
    /// The next piece of a reply to the question with this number.
    Reply(u64, StreamEvent),
    /// Settings: the saved key, and where keys are kept on this machine.
    KeyLoaded(KeyStore, Result<Option<String>, String>),
    KeyTyped(String),
    /// Settings: the key that was saved, where, and what settings.toml now holds of it.
    KeySaved(Result<(String, KeyStore, Option<String>), String>),
    TestConnection,
    Tested(Result<String, String>),
}

/// One window's assistant: whether its pane shows, the conversation and the suggestion cards.
/// It lives as long as the window, so closing and reopening the pane keeps the conversation.
#[derive(Default)]
pub struct Assistant {
    pub open: bool,
    pub conversation: Conversation,
    pub composer: text_editor::Content,
    pub cards: Vec<Card>,
    /// Whether the chat keeps its end in view as the reply grows: until the reader scrolls up,
    /// and again once they scroll back to the end.
    following: bool,
    /// How far down the chat is scrolled, in pixels, as it last said.
    chat_offset: f32,
    /// Numbers the questions, so pieces of a reply that was stopped are ignored.
    question: u64,
    /// The API key for the question being answered, read when it was asked.
    key: Option<String>,
    /// The reply streaming now; dropping it cancels the request.
    reply: Option<iced::task::Handle>,
}

/// A suggested edit as the chat shows it, under the answer to the question that proposed it.
pub struct Card {
    pub suggestion: Suggestion,
    /// Which question proposed it, counting from 1.
    pub turn: usize,
    pub state: CardState,
    /// Whether the model has been told how it was decided, in the note with the next question.
    pub told: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CardState {
    /// Waiting in the document, as the suggestion with this id.
    Waiting(u64),
    Accepted,
    Rejected,
    /// It couldn't be shown, or an edit to the text under it dropped it; says why.
    Gone(String),
}

impl Assistant {
    /// Stops the reply arriving now, keeping the text that came.
    pub fn stop(&mut self) {
        self.question += 1;
        self.reply = None;
        self.conversation.stop();
    }

    /// Stops any reply and empties the chat and its cards.
    pub fn new_chat(&mut self) {
        self.stop();
        self.conversation = Conversation::default();
        self.cards.clear();
    }
}

impl App {
    /// Handles the assistant messages; `App::update` hands every one of them here. A question
    /// that ended before anything came back goes back in the message box.
    pub fn update_assistant(&mut self, action: AssistantAction) -> Task<Message> {
        let task = self.assistant_step(action);
        let assistant = &mut self.assistant;
        if let Some(question) = assistant.conversation.unanswered.take() {
            let typed = assistant.composer.text();
            let text = match typed.trim().is_empty() {
                true => question,
                false => format!("{question}\n{typed}"),
            };
            assistant.composer = text_editor::Content::with_text(&text);
            let end = text_editor::Action::Move(text_editor::Motion::DocumentEnd);
            assistant.composer.perform(end);
        }
        task
    }

    fn assistant_step(&mut self, action: AssistantAction) -> Task<Message> {
        match action {
            AssistantAction::Toggle => {
                let assistant = &mut self.assistant;
                assistant.open = !assistant.open;
                return match assistant.open {
                    true => Task::batch([focus(COMPOSER_ID), self.follow()]),
                    false => focus(editor::EDITOR_ID),
                };
            }
            AssistantAction::Composer(action) => self.assistant.composer.perform(action),
            AssistantAction::Scrolled(viewport) => {
                // The offset stays put when only the reply grew: that leaves following as it was.
                // When the reader moved it, the chat follows only if they're at the end.
                let offset = viewport.absolute_offset().y;
                let end = viewport.content_bounds().height - viewport.bounds().height;
                let moved = offset != self.assistant.chat_offset;
                self.assistant.chat_offset = offset;
                if moved {
                    self.assistant.following = end - offset < 2.0;
                }
                if self.assistant.following && end - offset >= 2.0 {
                    return self.follow();
                }
            }
            AssistantAction::Send => {
                let question = self.assistant.composer.text().trim().to_string();
                if !question.is_empty() && self.can_ask() {
                    self.assistant.composer = text_editor::Content::new();
                    // Focus stays in the composer.
                    return Task::batch([focus(COMPOSER_ID), self.ask(&question)]);
                }
            }
            AssistantAction::Stop => {
                self.assistant.stop();
                return focus(COMPOSER_ID);
            }
            AssistantAction::NewChat => {
                self.decide(Decision::RejectAll);
                self.assistant.new_chat();
                return focus(COMPOSER_ID);
            }
            AssistantAction::KeyRead(question, key) if question == self.assistant.question => {
                match key {
                    Ok(key) => {
                        self.assistant.key = key;
                        return self.send_round();
                    }
                    Err(reason) => {
                        self.assistant.stop();
                        self.assistant.conversation.error = Some(reason);
                    }
                }
            }
            AssistantAction::Reply(question, piece) if question == self.assistant.question => {
                // The tools run on `Done`: let them see the document as it is now.
                if piece == StreamEvent::Done {
                    let snapshot = self.snapshot();
                    self.assistant.conversation.see(snapshot);
                }
                let next = self.assistant.conversation.receive(piece);
                self.show_suggestions();
                match next {
                    Next::Wait => {}
                    Next::SendAgain => return self.send_round(),
                    Next::Finished => self.assistant.reply = None,
                }
            }
            AssistantAction::KeyRead(..) | AssistantAction::Reply(..) => {}

            AssistantAction::KeyLoaded(store, key) => {
                self.key_field.store = Some(store);
                match key {
                    Ok(key) => self.key_field.text = key.unwrap_or_default(),
                    Err(reason) => self.key_field.error = Some(reason),
                }
            }
            AssistantAction::KeyTyped(text) => {
                self.key_field.text = text;
                self.connection = None;
                // One save at a time; `KeySaved` saves again if the key changed meanwhile.
                if !self.key_field.saving {
                    return self.save_key();
                }
            }
            AssistantAction::KeySaved(result) => {
                self.key_field.saving = false;
                match result {
                    Ok((saved, store, in_settings)) => {
                        self.key_field.store = Some(store);
                        self.key_field.error = None;
                        self.settings.api_key_fallback = in_settings;
                        self.save_settings();
                        if saved != self.key_field.text {
                            return self.save_key();
                        }
                    }
                    Err(reason) => self.key_field.error = Some(reason),
                }
            }
            AssistantAction::TestConnection => {
                self.connection = Some(Connection::Testing);
                let settings = self.settings.clone();
                return Task::perform(
                    async move {
                        let key = api_key(&settings)?;
                        test_connection(&settings, key.as_deref()).await
                    },
                    |result| Message::Assistant(AssistantAction::Tested(result)),
                );
            }
            AssistantAction::Tested(result) => {
                self.connection = Some(match result {
                    Ok(reply) => Connection::Passed(reply),
                    Err(reason) => Connection::Failed(reason),
                });
            }
        }
        Task::none()
    }

    /// Whether a question can go now: a model is set and no reply is arriving.
    pub fn can_ask(&self) -> bool {
        !self.settings.model.trim().is_empty() && !self.assistant.conversation.busy
    }

    /// Asks `question` about the document as it is now. The key is read off the UI thread (the
    /// keyring may ask to be unlocked), then `KeyRead` sends the request.
    fn ask(&mut self, question: &str) -> Task<Message> {
        let mut snapshot = self.snapshot();
        // Tell the model how its suggestions went since it last heard.
        for card in self.assistant.cards.iter_mut().filter(|card| !card.told) {
            let outcome = match &card.state {
                CardState::Waiting(_) => continue,
                CardState::Accepted => "accepted".to_string(),
                CardState::Rejected => "rejected".to_string(),
                CardState::Gone(reason) => format!("dropped ({reason})"),
            };
            snapshot
                .decided
                .push(context::decided(&card.suggestion, &outcome));
            card.told = true;
        }
        self.assistant.conversation.ask(question, snapshot);
        self.assistant.question += 1;
        let question = self.assistant.question;
        let settings = self.settings.clone();
        let key = Task::perform(async move { api_key(&settings) }, move |key| {
            Message::Assistant(AssistantAction::KeyRead(question, key))
        });
        Task::batch([key, self.follow()])
    }

    /// Scrolls the chat to its end, and keeps it there as the reply grows until the reader
    /// scrolls up.
    fn follow(&mut self) -> Task<Message> {
        self.assistant.following = true;
        snap_to_end(CHAT_ID)
    }

    /// Sends the conversation and streams the reply, one `Reply` message per piece. When the
    /// request can't be made, the conversation says why.
    fn send_round(&mut self) -> Task<Message> {
        let assistant = &mut self.assistant;
        let Ok(request) = assistant
            .conversation
            .request(&self.settings, assistant.key.as_deref())
        else {
            return Task::none();
        };
        let question = assistant.question;
        let pieces = iced::futures::stream::unfold(Reply::new(request), |mut reply| async move {
            reply.next().await.map(|piece| (piece, reply))
        });
        let (task, handle) = Task::run(pieces, move |piece| {
            Message::Assistant(AssistantAction::Reply(question, piece))
        })
        .abortable();
        assistant.reply = Some(handle.abort_on_drop());
        task
    }

    /// What the assistant sees: the text, its selection and caret, and the suggestions waiting
    /// in it.
    pub fn snapshot(&self) -> Snapshot {
        let doc = match &self.doc {
            Doc::Scratch => DocKind::Scratch {
                markdown: self.syntax() == Syntax::Markdown,
            },
            doc => DocKind::TextFile {
                name: status_bar::title(doc),
            },
        };
        Snapshot {
            doc,
            text: self.pane.model.text().to_string(),
            selection: self.pane.selection(),
            caret: self.pane.caret(),
            read_only: !self.editable(),
            waiting: self
                .pane
                .suggestions()
                .iter()
                .map(|s| s.range.clone())
                .collect(),
            decided: Vec::new(),
        }
    }

    /// Shows each edit the model just proposed as a suggestion in the document, with its card in
    /// the chat. The tools checked them against the text as it is, so the pane takes them.
    fn show_suggestions(&mut self) {
        let conversation = &mut self.assistant.conversation;
        let new: Vec<Suggestion> = std::mem::take(&mut conversation.new_suggestions);
        if new.is_empty() {
            return;
        }
        let turn = conversation
            .items
            .iter()
            .filter(|item| matches!(item, Item::User { .. }))
            .count();
        for suggestion in new {
            let range = suggestion.range();
            let state = match self.pane.suggest(range, &suggestion.replace) {
                Ok(id) => CardState::Waiting(id),
                Err(reason) => CardState::Gone(reason),
            };
            self.assistant.cards.push(Card {
                turn,
                state,
                told: false,
                suggestion,
            });
        }
    }

    /// Marks each waiting card whose suggestion has left the document as `decided`.
    pub fn settle_cards(&mut self, decided: CardState) {
        for card in &mut self.assistant.cards {
            let CardState::Waiting(id) = card.state else {
                continue;
            };
            if self.pane.suggestions().iter().all(|s| s.id != id) {
                card.state = decided.clone();
            }
        }
    }

    /// Reads the saved key and where keys are kept, off the UI thread, for the Settings panel.
    pub fn load_key(&self) -> Task<Message> {
        let settings = self.settings.clone();
        Task::perform(
            async move { (key_store(), api_key(&settings)) },
            |(store, key)| Message::Assistant(AssistantAction::KeyLoaded(store, key)),
        )
    }

    /// Saves the key field off the UI thread: to the keyring, or settings.toml without one.
    fn save_key(&mut self) -> Task<Message> {
        self.key_field.saving = true;
        let key = self.key_field.text.clone();
        let mut settings = self.settings.clone();
        Task::perform(
            async move {
                set_api_key(&key, &mut settings)
                    .map(|store| (key, store, settings.api_key_fallback))
            },
            |saved| Message::Assistant(AssistantAction::KeySaved(saved)),
        )
    }
}
