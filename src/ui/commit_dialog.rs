//! The Commit dialog, which turns the draft into the next version, with the Commit check's idea
//! rows (spec section 7).

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, space, text, text_input};
use iced::{Border, Color, Element, Fill, Padding, Theme};

use crate::project::paint::IdeaId;
use crate::ui::app::{App, Message};
use crate::ui::idea_actions::IdeaAction;
use crate::ui::project_view::{ProjectView, modal};
use crate::ui::status_bar;
use crate::ui::theme::{Roles, bold, dialog_box, fill, input_style, muted, subtle};

pub const COMMIT_NOTE_ID: &str = "commit-note";

/// The Commit dialog: the next version's number, how the counts change, a note, and the two
/// ways out (Esc goes back, Enter commits), with a row for the ideas carried, one per idea cut on
/// purpose and one per missing idea. With any missing, a "Heads up" says so and "mark cut"
/// quiets them.
pub fn commit_dialog<'a>(
    app: &'a App,
    project: &'a ProjectView,
    note: &'a str,
) -> Element<'a, Message> {
    let roles = Roles::of(&app.theme);
    let summary = match &project.commit_check {
        Some(check) => {
            let tokens = match (check.old.tokens, check.new.tokens) {
                (Some(old), Some(new)) => format!("{old} → {new} tokens · "),
                _ => String::new(),
            };
            format!("{tokens}{} → {} words", check.old.words, check.new.words)
        }
        None => String::new(),
    };
    let next = project.project.latest_version() + 1;

    let note_field = text_input("Note for this version (optional)", note)
        .id(COMMIT_NOTE_ID)
        .on_input(Message::CommitNote)
        .on_submit(Message::Commit)
        .size(13)
        .padding([5, 8])
        .style(input_style);

    let manifest = &project.project.manifest;
    let name = |id: &IdeaId| manifest.idea(*id).map_or("", |idea| idea.name.as_str());
    let mut ideas = column![];
    let mut missing = false;
    if let Some(check) = &project.commit_check {
        missing = !check.missing.is_empty();
        if !check.carried.is_empty() {
            let names: Vec<&str> = check.carried.iter().map(name).collect();
            let count = match check.carried.len() {
                1 => "1 idea carried".to_string(),
                n => format!("{n} ideas carried"),
            };
            let line = check_line(roles.accent_aqua, count, roles.fg, names.join(", "), false);
            ideas = ideas.push(line);
        }
        for id in &check.cut {
            let line = check_line(
                roles.fg_subtle,
                name(id),
                roles.fg_muted,
                "cut on purpose",
                false,
            );
            ideas = ideas.push(line);
        }
        for id in &check.missing {
            let warm = roles.accent_warm;
            let line = check_line(warm, name(id), warm, "missing, no text left", true);
            ideas = ideas.push(line);
        }
        if missing {
            let names: Vec<&str> = check.missing.iter().map(name).collect();
            let verb = if names.len() == 1 { "has" } else { "have" };
            let heads_up = row![
                text("Heads up")
                    .size(12)
                    .font(bold())
                    .color(roles.accent_warm),
                text(format!(
                    "{} {verb} no text left in the draft. Go back, mark cut, or commit anyway.",
                    names.join(", ")
                ))
                .size(12)
                .color(roles.fg_muted),
            ]
            .spacing(8);
            ideas = ideas.push(space().height(6)).push(heads_up);
        }
    }
    let mut buttons = row![space::horizontal()].spacing(6);
    if missing {
        let cut = Message::Idea(IdeaAction::CutMissing);
        buttons = buttons.push(dialog_button("mark cut", "", false, Some(cut)));
    }
    let commit = if missing { "commit anyway" } else { "commit" };
    buttons = buttons
        .push(dialog_button(
            "go back",
            "esc",
            false,
            Some(Message::CloseMenu),
        ))
        .push(dialog_button(
            commit,
            "⏎",
            true,
            (!app.busy).then_some(Message::Commit),
        ));
    let card = column![
        row![
            text(format!("Commit v{next}")).size(13),
            space::horizontal(),
            muted(summary)
        ]
        .align_y(iced::Center),
        note_field,
        ideas,
        column![
            container(space().width(Fill).height(1)).style(move |_: &Theme| fill(roles.border)),
            buttons
        ]
        .spacing(12),
    ]
    .spacing(16);
    let card = container(card)
        .width(520)
        .padding(Padding {
            top: 20.0,
            right: 22.0,
            bottom: 16.0,
            left: 22.0,
        })
        .style(dialog_box);
    modal(card.into())
}

/// One Commit check line, 26px tall: a dot, the words, and a note on the right, all in the
/// warning colour when `warn`.
fn check_line<'a>(
    dot: Color,
    words: impl text::IntoFragment<'a>,
    color: Color,
    note: impl text::IntoFragment<'a>,
    warn: bool,
) -> Element<'a, Message> {
    // A long list of names wraps onto more lines, right-aligned.
    let note = muted(note).wrapping(Wrapping::Word).align_x(iced::Right);
    row![
        container(status_bar::dot(dot)).center_y(26),
        text(words).size(13).color(color),
        if warn {
            note.color(color)
        } else {
            note.style(subtle)
        }
        .width(Fill),
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}

/// A word button with its key hint, as in the Commit mockup; `primary` is the yellow default.
fn dialog_button<'a>(
    label: &'a str,
    key: &'a str,
    primary: bool,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    button(row![text(label).size(12), muted(key)].spacing(6))
        .padding([4, 10])
        .style(move |theme: &Theme, status| {
            let roles = Roles::of(theme);
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: None,
                text_color: match (primary, hovered) {
                    (true, _) => roles.accent_text,
                    (false, true) => roles.fg,
                    (false, false) => roles.fg_muted,
                },
                border: Border::default()
                    .rounded(4)
                    .width(1)
                    .color(if primary || hovered {
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
