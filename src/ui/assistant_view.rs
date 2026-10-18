//! The assistant pane beside the document: its header and the chips saying what it sees, the
//! chat (with each reply's tool use and suggestion cards), and the composer.

use iced::keyboard::{Key, key::Named};
use iced::widget::text::Wrapping;
use iced::widget::text_editor::{Binding, KeyPress, Status};
use iced::widget::{
    button, column, container, opaque, row, scrollable, space, stack, text, text_editor,
};
use iced::{Background, Border, Color, Element, Fill, Font, Padding, Theme};

use crate::ai::client::{Item, ToolCall};
use crate::ai::context;
use crate::ai::tools::{describe, shorten};
use crate::core::opened::Doc;
use crate::ui::app::{App, Message};
use crate::ui::assistant_actions::{AssistantAction, COMPOSER_ID, Card, CardState};
use crate::ui::editor::Decision;
use crate::ui::status_bar::{dot, title};
use crate::ui::theme::{MONO, Roles, bold, fill, muted};

pub const PANE_WIDTH: f32 = 380.0;
/// At or below this window width the pane takes the whole width, as in the mockup.
const NARROW: f32 = 760.0;
/// The longest chip label, in characters.
const CHIP_CHARS: usize = 40;
/// The longest model name the header shows, in characters, so "suggests only" and New chat fit.
const NAME_CHARS: usize = 16;

/// The document `width` pixels wide, with the pane on the right while it shows (or slides). The
/// document narrows to make room; in a narrow window the pane slides over it instead. The layout
/// keeps its shape for the whole slide, so the composer keeps its focus.
pub fn beside(app: &App, width: f32) -> Element<'_, Message> {
    let document = container(app.document_view()).width(Fill);
    let shown = app.assistant.shown(app.now);
    if !app.assistant.open && shown == 0.0 {
        return row![document].into();
    }
    // The pane is laid out at its full width and only the part that has slid in shows, so its
    // contents never squeeze and rewrap mid-slide: a horizontal scrollable gives them all the
    // width they ask for. At least a pixel shows: a widget with no width can't take focus.
    let pane = |full_width: f32| {
        let pane_width = (full_width * shown).round().max(1.0);
        let full = container(pane(app)).width(full_width);
        let hidden = scrollable::Direction::Horizontal(scrollable::Scrollbar::hidden());
        container(scrollable(full).direction(hidden)).width(pane_width)
    };
    if width <= NARROW {
        let pane = pane(width).style(|theme: &Theme| fill(Roles::of(theme).bg_surface));
        let pane = container(opaque(pane)).align_right(Fill);
        return stack![document, pane].into();
    }
    let divider = container(space().width(1).height(Fill))
        .style(|theme: &Theme| fill(Roles::of(theme).border));
    row![document, divider, pane(PANE_WIDTH)]
        .height(Fill)
        .into()
}

fn pane(app: &App) -> Element<'_, Message> {
    let model = app.settings.model.trim();
    let name = if model.is_empty() { "Assistant" } else { model };
    let conversation = &app.assistant.conversation;
    let new_chat = (!conversation.items.is_empty() || conversation.busy)
        .then_some(Message::Assistant(AssistantAction::NewChat));
    let title = row![
        text(shorten(name, NAME_CHARS))
            .size(13)
            .font(bold())
            .wrapping(Wrapping::None),
        muted("suggests only"),
        space::horizontal(),
        chat_button("New chat", Look::Quiet, new_chat),
    ]
    .spacing(8)
    .align_y(iced::Center);
    let head = container(column![title, chips(app)].spacing(10)).padding(Padding {
        top: 14.0,
        right: 16.0,
        bottom: 12.0,
        left: 16.0,
    });
    let rule = container(space().width(Fill).height(1))
        .style(|theme: &Theme| fill(Roles::of(theme).bg_raised));

    let mut bottom = column![].spacing(8).padding(Padding {
        top: 0.0,
        right: 12.0,
        bottom: 12.0,
        left: 12.0,
    });
    let waiting = app.pane.suggestions().len();
    if waiting > 1 {
        bottom = bottom.push(
            row![
                muted(format!("{waiting} suggestions")),
                space::horizontal(),
                chat_button(
                    "Accept all",
                    Look::Quiet,
                    Some(Message::Decide(Decision::AcceptAll))
                ),
                chat_button(
                    "Reject all",
                    Look::Quiet,
                    Some(Message::Decide(Decision::RejectAll))
                ),
            ]
            .spacing(4)
            .padding([0, 4])
            .align_y(iced::Center),
        );
    }
    bottom = bottom.push(composer(app));
    column![head, rule, chat(app), bottom].height(Fill).into()
}

/// What the assistant is focused on (accent) and what it can see.
fn chips(app: &App) -> Element<'_, Message> {
    let mut chips = row![].spacing(6);
    if let Some(focus) = context::focus(app.pane.model.text(), app.pane.selection()) {
        chips = chips.push(chip(format!("Focus: {focus}"), true));
    }
    let sees = match &app.doc {
        Doc::Scratch => "Sees scratch".to_string(),
        doc => format!("Sees {}", title(doc)),
    };
    chips = chips.push(chip(sees, false));
    chips.wrap().vertical_spacing(6).into()
}

fn chip<'a>(words: String, focus: bool) -> Element<'a, Message> {
    container(
        text(shorten(&words, CHIP_CHARS))
            .size(11)
            .wrapping(Wrapping::None),
    )
    .padding([3, 8])
    .style(move |theme: &Theme| {
        let roles = Roles::of(theme);
        let (color, border) = match focus {
            true => (roles.accent_text, roles.accent_text.scale_alpha(0.5)),
            false => (roles.fg_muted, roles.border),
        };
        container::Style {
            background: Some(Background::Color(roles.bg_inset)),
            text_color: Some(color),
            border: Border::default().rounded(999).width(1).color(border),
            ..container::Style::default()
        }
    })
    .into()
}

/// The conversation: the user's messages as bubbles on the right, replies as plain text, each
/// reply's tool use as small muted lines, and the cards for the edits each question brought.
fn chat(app: &App) -> Element<'_, Message> {
    let conversation = &app.assistant.conversation;
    let mut entries: Vec<Element<'_, Message>> = Vec::new();
    // Tool lines in a row are one entry, so they sit close together.
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
                entries.push(bubble(text));
            }
            Item::Assistant { text, tool_calls } => {
                if !text.is_empty() {
                    flush(&mut entries, &mut steps);
                    entries.push(prose(text));
                }
                calls = tool_calls;
            }
            Item::ToolResult { call_id, output } => {
                if let Some(call) = calls.iter().find(|call| &call.call_id == call_id) {
                    steps.push(step(describe(call, output)));
                }
            }
        }
    }
    if !conversation.reply.is_empty() {
        flush(&mut entries, &mut steps);
        entries.push(prose(&conversation.reply));
    } else if conversation.busy {
        steps.push(step("Working…".to_string()));
    }
    flush(&mut entries, &mut steps);
    entries.extend(cards(app, turn));
    if let Some(error) = &conversation.error {
        entries.push(
            text(error.as_str())
                .size(13)
                .style(|theme: &Theme| text::Style {
                    color: Some(Roles::of(theme).accent_warm),
                })
                .into(),
        );
    }
    scrollable(column(entries).spacing(20).padding(Padding {
        top: 20.0,
        right: 16.0,
        bottom: 24.0,
        left: 16.0,
    }))
    .anchor_bottom()
    .height(Fill)
    .into()
}

fn flush<'a>(entries: &mut Vec<Element<'a, Message>>, steps: &mut Vec<Element<'a, Message>>) {
    if !steps.is_empty() {
        entries.push(column(std::mem::take(steps)).spacing(4).into());
    }
}

/// Before the first question: how to start, or where to set a model first.
fn empty_chat(app: &App) -> Element<'_, Message> {
    if app.settings.model.trim().is_empty() {
        return column![
            muted_prose("Set a model in Settings to start. Any OpenAI-compatible server works."),
            chat_button("Open Settings", Look::Framed, Some(Message::OpenSettings)),
        ]
        .spacing(10)
        .into();
    }
    muted_prose(
        "Ask about the text. It can read and search it, count tokens and suggest edits, which \
         show in the text for you to accept or reject.",
    )
}

fn bubble(words: &str) -> Element<'_, Message> {
    container(
        container(text(words).size(14))
            .padding([8, 12])
            .max_width(PANE_WIDTH * 0.8)
            .style(|theme: &Theme| {
                let roles = Roles::of(theme);
                container::Style {
                    background: Some(Background::Color(roles.bg_raised)),
                    text_color: Some(roles.fg),
                    border: Border::default().rounded(10),
                    ..container::Style::default()
                }
            }),
    )
    .align_right(Fill)
    .into()
}

fn prose(words: &str) -> Element<'_, Message> {
    text(words).size(14).line_height(1.6).into()
}

fn muted_prose(words: &str) -> Element<'_, Message> {
    text(words)
        .size(13)
        .line_height(1.6)
        .style(|theme: &Theme| text::Style {
            color: Some(Roles::of(theme).fg_muted),
        })
        .into()
}

/// A tool the model used, as one small muted line.
fn step<'a>(words: String) -> Element<'a, Message> {
    text(words)
        .size(12)
        .style(|theme: &Theme| text::Style {
            color: Some(Roles::of(theme).fg_subtle),
        })
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

/// A waiting suggestion: "Edit", its size change, the new text, and Accept, Reject and Try
/// shorter. A decided one shrinks to one muted line saying how it went.
fn card_view<'a>(app: &'a App, card: &'a Card) -> Element<'a, Message> {
    let color = Roles::of(&app.theme).accent_cool;
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
            // Wraps, so a long reason is read in full.
            let line = muted(format!("Edit · {outcome}")).wrapping(Wrapping::WordOrGlyph);
            return row![dot(color), line]
                .spacing(8)
                .align_y(iced::Center)
                .into();
        }
    };
    let mut size = format!("{:+} words", card.words);
    if let Some(tokens) = card.tokens {
        size.push_str(&format!(" · {tokens:+} tokens"));
    }
    let new_text = match suggestion.replace.is_empty() {
        true => format!("deletes \u{201c}{}\u{201d}", suggestion.find),
        false => format!("\u{201c}{}\u{201d}", suggestion.replace),
    };
    let italic = Font {
        style: iced::font::Style::Italic,
        ..MONO
    };
    let mut body = column![
        row![
            dot(color),
            container(text("Edit").size(12)).width(Fill),
            muted(size),
        ]
        .spacing(8)
        .align_y(iced::Center),
        text(new_text).size(14).font(italic),
    ]
    .spacing(10);
    if !suggestion.why.trim().is_empty() {
        body = body.push(step(suggestion.why.clone()));
    }
    let shorter = app
        .can_ask()
        .then_some(Message::Assistant(AssistantAction::TryShorter(id)));
    body = body.push(
        row![
            chat_button(
                "Accept",
                Look::Primary,
                Some(Message::Decide(Decision::Accept(Some(id))))
            ),
            chat_button(
                "Reject",
                Look::Framed,
                Some(Message::Decide(Decision::Reject(Some(id))))
            ),
            chat_button("Try shorter", Look::Quiet, shorter),
        ]
        .spacing(8),
    );
    container(body)
        .padding(12)
        .width(Fill)
        .style(|theme: &Theme| {
            let roles = Roles::of(theme);
            container::Style {
                background: Some(Background::Color(roles.bg_raised)),
                text_color: Some(roles.fg),
                border: Border::default().rounded(10).width(1).color(roles.border),
                ..container::Style::default()
            }
        })
        .into()
}

/// The message box and Send (Stop while a reply is coming). Enter sends; Shift+Enter starts a
/// new line.
fn composer(app: &App) -> Element<'_, Message> {
    let assistant = &app.assistant;
    let placeholder = if app.settings.model.trim().is_empty() {
        "Set a model in Settings first"
    } else {
        "Ask about this text…"
    };
    let input = text_editor(&assistant.composer)
        .id(COMPOSER_ID)
        .placeholder(placeholder)
        .on_action(|action| Message::Assistant(AssistantAction::Composer(action)))
        .key_binding(composer_keys)
        .size(14)
        .padding([5, 0])
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
    let action = match assistant.conversation.busy {
        true => chat_button(
            "Stop",
            Look::Framed,
            Some(Message::Assistant(AssistantAction::Stop)),
        ),
        false => chat_button(
            "Send",
            Look::Primary,
            app.can_ask()
                .then_some(Message::Assistant(AssistantAction::Send)),
        ),
    };
    container(row![input, action].spacing(8).align_y(iced::Bottom))
        .padding(Padding {
            top: 6.0,
            right: 6.0,
            bottom: 6.0,
            left: 12.0,
        })
        .style(|theme: &Theme| {
            let roles = Roles::of(theme);
            container::Style {
                background: Some(Background::Color(roles.bg_raised)),
                border: Border::default().rounded(10).width(1).color(roles.border),
                ..container::Style::default()
            }
        })
        .into()
}

fn composer_keys(press: KeyPress) -> Option<Binding<Message>> {
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

/// How a chat button looks: filled (the main action), framed, or just muted words.
#[derive(Clone, Copy)]
enum Look {
    Primary,
    Framed,
    Quiet,
}

/// A button in the pane; `None` draws it disabled.
fn chat_button<'a>(label: &'a str, look: Look, on_press: Option<Message>) -> Element<'a, Message> {
    button(text(label).size(13).wrapping(Wrapping::None))
        .padding([5, 12])
        .style(move |theme: &Theme, status| {
            let roles = Roles::of(theme);
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let disabled = status == button::Status::Disabled;
            let (background, text_color, border) = match look {
                Look::Primary => (Some(roles.btn_bg), roles.btn_fg, Color::TRANSPARENT),
                Look::Framed => (
                    Some(if hovered {
                        roles.bg_inset
                    } else {
                        roles.bg_raised
                    }),
                    roles.fg,
                    roles.border,
                ),
                Look::Quiet => (
                    hovered.then_some(roles.bg_inset),
                    roles.fg_muted,
                    Color::TRANSPARENT,
                ),
            };
            button::Style {
                background: background.map(|color| {
                    Background::Color(if disabled {
                        color.scale_alpha(0.5)
                    } else {
                        color
                    })
                }),
                text_color: if disabled {
                    text_color.scale_alpha(0.6)
                } else {
                    text_color
                },
                border: Border::default().rounded(6).width(1).color(border),
                ..button::Style::default()
            }
        })
        .on_press_maybe(on_press)
        .into()
}
