//! The assistant pane beside the document, drawn like a terminal transcript: `#` comment lines
//! saying which model it is and what it sees, your questions after a `>`, each reply's tool use
//! and suggested edits (why, with accept, reject and try shorter: the change itself shows in the
//! document), and the composer as a `>` prompt.

use iced::keyboard::{Key, key::Named};
use iced::widget::text::Wrapping;
use iced::widget::text_editor::{Binding, KeyPress, Status};
use iced::widget::{
    button, column, container, opaque, row, scrollable, space, stack, text, text_editor,
};
use iced::{Background, Border, Color, Element, Fill, Padding, Theme};

use crate::ai::client::{Item, ToolCall};
use crate::ai::tools::{describe, shorten};
use crate::core::opened::Doc;
use crate::ui::app::{App, Message};
use crate::ui::assistant_actions::{AssistantAction, CHAT_ID, COMPOSER_ID, Card, CardState};
use crate::ui::editor::{self, Decision};
use crate::ui::status_bar::{keys, title};
use crate::ui::theme::{CHAR_WIDTH_RATIO, Roles, fill};

pub const PANE_WIDTH: f32 = 380.0;
/// At or below this window width the pane takes the whole width, as in the mockup.
const NARROW: f32 = 760.0;
/// The pane's one text size, and its line height as a multiple of it.
pub const SIZE: f32 = 13.0;
pub const LINE: f32 = 1.6;
/// The width of one character, for indents.
pub const CH: f32 = SIZE * CHAR_WIDTH_RATIO;
/// The longest file name the header shows, in characters.
const SEES_CHARS: usize = 36;
/// The longest model name the header shows, in characters, so "suggests only" and new chat fit.
const NAME_CHARS: usize = 16;

/// The document `width` pixels wide, with the pane on the right while it is open. The document
/// narrows to make room; in a narrow window the pane covers it instead.
pub fn beside(app: &App, width: f32) -> Element<'_, Message> {
    let document = container(app.document_view()).width(Fill);
    if !app.assistant.open {
        return row![document].into();
    }
    if width <= NARROW {
        let pane = container(pane(app))
            .width(width)
            .style(|theme: &Theme| fill(Roles::of(theme).bg_surface));
        let pane = container(opaque(pane)).align_right(Fill);
        return stack![document, pane].into();
    }
    let divider = container(space().width(1).height(Fill))
        .style(|theme: &Theme| fill(Roles::of(theme).border));
    row![document, divider, container(pane(app)).width(PANE_WIDTH)]
        .height(Fill)
        .into()
}

fn pane(app: &App) -> Element<'_, Message> {
    let model = app.settings.model.trim();
    let name = if model.is_empty() { "assistant" } else { model };
    let conversation = &app.assistant.conversation;
    let new_chat = (!conversation.items.is_empty() || conversation.busy)
        .then_some(Message::Assistant(AssistantAction::NewChat));
    let mut head = column![row![
        line(
            format!("# {} · suggests only", shorten(name, NAME_CHARS)),
            subtle
        ),
        space::horizontal(),
        word("new chat", fg_muted, new_chat),
    ]];
    let sees = match &app.doc {
        Doc::Scratch => "scratch".to_string(),
        doc => title(doc),
    };
    head = head.push(line(
        format!("# sees {}", shorten(&sees, SEES_CHARS)),
        subtle,
    ));
    let head = container(head).padding(Padding {
        top: 14.0,
        right: 16.0,
        bottom: 0.0,
        left: 16.0,
    });

    // The keys for suggestions sit above the composer in a column of their own, which is always
    // there, so the composer keeps its place in the tree (and its focus) as they come and go.
    let mut keys_help = column![];
    let waiting = app.pane.suggestions().len();
    if waiting > 0 {
        keys_help = keys_help.push(line(keys("# Ctrl+Enter accepts"), subtle));
        keys_help = keys_help.push(line(keys("# Ctrl+Backspace rejects"), subtle));
    }
    if waiting > 1 {
        keys_help = keys_help.push(
            row![
                line(format!("# {waiting} suggestions"), subtle),
                space::horizontal(),
                word(
                    "accept all",
                    accent_text,
                    Some(Message::Decide(Decision::AcceptAll))
                ),
                word(
                    "reject all",
                    fg_muted,
                    Some(Message::Decide(Decision::RejectAll))
                ),
            ]
            .spacing(CH),
        );
    }
    let bottom = column![keys_help, composer(app)].padding(Padding {
        top: 0.0,
        right: 16.0,
        bottom: 12.0,
        left: 16.0,
    });
    column![head, chat(app), bottom].height(Fill).into()
}

/// One line of the pane's text in a colour from the palette, wrapping when it's long.
pub fn line<'a>(words: impl text::IntoFragment<'a>, color: fn(&Roles) -> Color) -> text::Text<'a> {
    text(words)
        .size(SIZE)
        .line_height(LINE)
        .style(move |theme: &Theme| text::Style {
            color: Some(color(Roles::of(theme))),
        })
}

pub fn subtle(roles: &Roles) -> Color {
    roles.fg_subtle
}

fn fg_muted(roles: &Roles) -> Color {
    roles.fg_muted
}

pub fn accent_text(roles: &Roles) -> Color {
    roles.accent_text
}

/// The conversation: each question after a `>`, replies as plain text, each reply's tool use as
/// subtle lines, and the edits each question brought.
fn chat(app: &App) -> Element<'_, Message> {
    let conversation = &app.assistant.conversation;
    let mut entries: Vec<Element<'_, Message>> = Vec::new();
    // Tool lines in a row are one entry, so they sit together.
    let mut steps: Vec<Element<'_, Message>> = Vec::new();
    if conversation.items.is_empty() && !conversation.busy {
        entries.push(empty_chat(app));
    }
    let mut turn = 0;
    // The calls of the last reply, so each result can say how its call went.
    let mut calls: &[ToolCall] = &[];
    for item in &conversation.items {
        match item {
            Item::User { text, .. } => {
                flush(&mut entries, &mut steps);
                entries.extend(cards(app, turn));
                turn += 1;
                entries.push(prompt(text));
            }
            Item::Assistant { text, tool_calls } => {
                if !text.is_empty() {
                    flush(&mut entries, &mut steps);
                    entries.push(line(text.as_str(), |roles| roles.fg).into());
                }
                calls = tool_calls;
            }
            Item::ToolResult { call_id, output } => {
                if let Some(call) = calls.iter().find(|call| &call.call_id == call_id) {
                    steps.push(line(describe(call, output), subtle).into());
                }
            }
        }
    }
    if !conversation.reply.is_empty() {
        flush(&mut entries, &mut steps);
        entries.push(line(conversation.reply.as_str(), |roles| roles.fg).into());
    } else if conversation.busy {
        steps.push(line("working…", subtle).into());
    }
    flush(&mut entries, &mut steps);
    entries.extend(cards(app, turn));
    if let Some(error) = &conversation.error {
        entries.push(line(error.as_str(), |roles| roles.accent_warm).into());
    }
    scrollable(column(entries).spacing(SIZE * LINE).padding(Padding {
        top: SIZE * LINE,
        right: 16.0,
        bottom: SIZE * LINE,
        left: 16.0,
    }))
    .id(CHAT_ID)
    .on_scroll(|viewport| Message::Assistant(AssistantAction::Scrolled(viewport)))
    .height(Fill)
    .into()
}

fn flush<'a>(entries: &mut Vec<Element<'a, Message>>, steps: &mut Vec<Element<'a, Message>>) {
    if !steps.is_empty() {
        entries.push(column(std::mem::take(steps)).into());
    }
}

/// Before the first question: how to start, or where to set a model first.
fn empty_chat(app: &App) -> Element<'_, Message> {
    if app.settings.model.trim().is_empty() {
        return column![
            line(
                "# set a model in Settings to start; any OpenAI-compatible server works",
                subtle
            ),
            word("open settings", accent_text, Some(Message::OpenSettings)),
        ]
        .into();
    }
    line(
        "# ask about the text: it can read and search it, count tokens and suggest edits, which \
         show in the text for you to accept or reject",
        subtle,
    )
    .into()
}

/// A question, after a `>` in the accent.
fn prompt(words: &str) -> Element<'_, Message> {
    row![
        line(">", accent_text),
        line(words, |roles| roles.fg).width(Fill)
    ]
    .spacing(CH)
    .into()
}

/// The cards for the edits question `turn` brought, after its answer.
fn cards(app: &App, turn: usize) -> impl Iterator<Item = Element<'_, Message>> {
    app.assistant
        .cards
        .iter()
        .filter(move |card| card.turn == turn)
        .map(move |card| card_view(app, card))
}

/// A waiting suggestion, as the call that made it with the line it's on, why, and accept, reject
/// and try shorter. What it changes shows in the document. The current one (what Ctrl+Enter and
/// Ctrl+Backspace decide) has its accept lit. A decided one shrinks to one line saying how it
/// went.
fn card_view<'a>(app: &'a App, card: &'a Card) -> Element<'a, Message> {
    let suggestion = &card.suggestion;
    let id = match &card.state {
        CardState::Waiting(id) => *id,
        decided => {
            let outcome = match decided {
                CardState::Accepted => "accepted",
                CardState::Rejected => "rejected",
                CardState::Gone(reason) => reason.as_str(),
                CardState::Waiting(_) => "",
            };
            return line(format!("propose_edit · {outcome}"), subtle).into();
        }
    };
    let text_now = app.pane.model.text();
    let place = app
        .pane
        .suggestions()
        .iter()
        .find(|waiting| waiting.id == id)
        .map(|waiting| {
            let line = text_now[..waiting.range.start].matches('\n').count() + 1;
            format!("(line {line})")
        })
        .unwrap_or_default();
    let call = row![
        line("propose_edit", |roles| roles.accent_cool),
        line(place, subtle)
    ];
    let mut card_column = column![call];
    if !suggestion.why.trim().is_empty() {
        let why = line(format!("# {}", suggestion.why.trim()), subtle);
        card_column = card_column.push(container(why).padding(Padding::default().left(2.0 * CH)));
    }
    let shorter = app
        .can_ask()
        .then_some(Message::Assistant(AssistantAction::TryShorter(id)));
    let current = app.pane.current_suggestion() == Some(id);
    let buttons = row![
        word(
            "accept",
            if current { accent_text } else { fg_muted },
            Some(Message::Decide(Decision::Accept(Some(id))))
        ),
        word(
            "reject",
            fg_muted,
            Some(Message::Decide(Decision::Reject(Some(id))))
        ),
        word("try shorter", fg_muted, shorter),
    ]
    .spacing(CH);
    // Indented two characters, the buttons' text lined up with the line above inside their
    // padding.
    card_column
        .push(container(buttons).padding(Padding::default().left(2.0 * CH - 4.0)))
        .into()
}

/// The composer: a `>` prompt and the message box, and stop while a reply is coming. Enter
/// sends; Shift+Enter starts a new line.
fn composer(app: &App) -> Element<'_, Message> {
    let assistant = &app.assistant;
    let reviewing = app.pane.reviewing();
    let placeholder = if app.settings.model.trim().is_empty() {
        "set a model in Settings first"
    } else {
        "ask about this text · Enter sends"
    };
    let input = text_editor(&assistant.composer)
        .id(COMPOSER_ID)
        .placeholder(placeholder)
        .on_action(|action| Message::Assistant(AssistantAction::Composer(action)))
        .key_binding(move |press| composer_keys(press, reviewing))
        .size(SIZE)
        .line_height(LINE)
        .padding(0)
        .max_height(160.0)
        .style(|theme: &Theme, _status| {
            let roles = Roles::of(theme);
            text_editor::Style {
                background: Background::Color(Color::TRANSPARENT),
                border: Border::default(),
                placeholder: roles.fg_subtle,
                value: roles.fg,
                selection: roles.accent_cool.scale_alpha(0.35),
            }
        });
    let mut prompt = row![line(">", accent_text), input].spacing(CH);
    if assistant.conversation.busy {
        prompt = prompt.push(word(
            "stop",
            accent_text,
            Some(Message::Assistant(AssistantAction::Stop)),
        ));
    }
    container(prompt)
        .padding(Padding::default().top(SIZE * LINE))
        .into()
}

/// Enter sends and Shift+Enter starts a new line. While suggestions wait (`reviewing`), Ctrl+Enter
/// and Ctrl+Backspace are left to the app, which accepts and rejects with them.
fn composer_keys(press: KeyPress, reviewing: bool) -> Option<Binding<Message>> {
    if reviewing && editor::decides(&press) {
        return None;
    }
    let enter = matches!(press.key.as_ref(), Key::Named(Named::Enter));
    if enter && !press.modifiers.shift() && matches!(press.status, Status::Focused { .. }) {
        return Some(Binding::Custom(Message::Assistant(AssistantAction::Send)));
    }
    let command = press.modifiers.command();
    match Binding::from_key_press(press) {
        // Ctrl+, and the like are the app's shortcuts, never typing.
        Some(Binding::Insert(_)) if command => None,
        binding => binding,
    }
}

/// A word that is a button, as Settings' values are: in `color`, framed while hovered, and
/// dimmed when `on_press` is `None`.
fn word<'a>(
    label: &'a str,
    color: fn(&Roles) -> Color,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    button(
        text(label)
            .size(SIZE)
            .line_height(LINE)
            .wrapping(Wrapping::None),
    )
    .padding([0, 4])
    .style(move |theme: &Theme, status| {
        let roles = Roles::of(theme);
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let text_color = match status {
            button::Status::Disabled => roles.fg_subtle.scale_alpha(0.6),
            _ => color(roles),
        };
        button::Style {
            background: None,
            text_color,
            border: Border::default().rounded(4).width(1).color(if hovered {
                roles.border
            } else {
                Color::TRANSPARENT
            }),
            ..button::Style::default()
        }
    })
    .on_press_maybe(on_press)
    .into()
}
