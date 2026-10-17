//! The idea panel: edits one idea (name, feeling note, colour, topic) and offers what can be done
//! with it, raised above the idea strip.

use iced::widget::{
    button, column, container, mouse_area, row, scrollable, space, stack, text, text_input,
};
use iced::{Background, Border, Element, Fill, Padding, Theme};

use crate::project::paint::IdeaId;
use crate::project::weights::Status;
use crate::ui::app::{App, Message};
use crate::ui::idea_actions::IdeaAction;
use crate::ui::ideas_view::{STRIP_HEIGHT, on};
use crate::ui::project_view::ProjectView;
use crate::ui::status_bar::{self, menu_item};
use crate::ui::theme::{Roles, input_style, menu_button, muted, raised_box, subtle};

pub const IDEA_NAME_ID: &str = "idea-name";
pub const NOTE_ID: &str = "idea-note";
pub const MERGE_NAME_ID: &str = "merge-name";

/// The idea panel's state: which idea, and what its fields hold as typed (the manifest gets
/// them trimmed).
pub struct IdeaPanel {
    pub idea: IdeaId,
    pub name: String,
    pub note: String,
    /// "Feeling note…" was clicked, so the note's editor shows in its place.
    pub editing_note: bool,
    pub new_topic: String,
    /// Delete was clicked once; a second click deletes.
    pub confirm_delete: bool,
    /// "Merge with…" picked another idea; the panel asks for the merged idea's name and note.
    pub merge: Option<MergeForm>,
}

pub struct MergeForm {
    pub other: IdeaId,
    pub name: String,
    pub note: String,
}

impl IdeaPanel {
    pub fn new(idea: IdeaId, name: &str, note: &str) -> IdeaPanel {
        IdeaPanel {
            idea,
            name: name.to_string(),
            note: note.to_string(),
            editing_note: false,
            new_topic: String::new(),
            confirm_delete: false,
            merge: None,
        }
    }
}

/// The panel that edits one idea, raised above the strip's left end: its name, feeling note,
/// colour and topic, then what can be done with it. Clicking anywhere else closes it.
pub fn panel<'a>(
    app: &'a App,
    project: &'a ProjectView,
    panel: &'a IdeaPanel,
) -> Element<'a, Message> {
    let roles = Roles::of(&app.theme);
    let manifest = &project.project.manifest;
    let Some(idea) = manifest.idea(panel.idea) else {
        return space().into();
    };
    let version = manifest.draft_version();
    if let Some(form) = &panel.merge {
        let merge = on(IdeaAction::Merge);
        let other = manifest
            .idea(form.other)
            .map_or("", |other| other.name.as_str());
        let items = column![
            text(format!("Merge {} with {other}", idea.name)).size(13),
            label("Name of the merged idea".to_string()),
            field(
                "",
                &form.name,
                MERGE_NAME_ID,
                IdeaAction::MergeName,
                merge.clone()
            ),
            label(format!("Its feeling note for v{version}")),
            field(
                "How it should feel",
                &form.note,
                "merge-note",
                IdeaAction::MergeNote,
                merge.clone()
            ),
            row![
                space::horizontal(),
                word("cancel", false, on(IdeaAction::OpenPanel(idea.id))),
                word("merge ⏎", true, merge),
            ]
            .spacing(4),
        ]
        .spacing(6)
        .width(300);
        return raised_panel(container(items).padding(10).style(raised_box).into());
    }
    let note: Element<'a, Message> = if panel.editing_note {
        column![
            label(format!("Feeling note for v{version}")),
            // Enter closes the editor again.
            field(
                "How it should feel",
                &panel.note,
                NOTE_ID,
                IdeaAction::Note,
                on(IdeaAction::OpenPanel(idea.id))
            ),
        ]
        .spacing(6)
        .into()
    } else {
        button(
            row![
                text("Feeling note…").size(13),
                container(muted(panel.note.as_str())).width(Fill).clip(true),
            ]
            .spacing(12)
            .align_y(iced::Center),
        )
        .width(Fill)
        .padding([4, 8])
        .style(menu_button)
        .on_press(on(IdeaAction::OpenNote(idea.id)))
        .into()
    };
    let mut colours = row![].spacing(6);
    for slot in 0..crate::project::ideas::COLOUR_SLOTS {
        let chosen = slot == idea.colour;
        colours = colours.push(
            button(space().width(14).height(14))
                .padding(0)
                .style(move |theme: &Theme, _| {
                    let roles = Roles::of(theme);
                    button::Style {
                        background: Some(Background::Color(roles.idea(slot))),
                        border: Border::default()
                            .rounded(8)
                            .width(if chosen { 2 } else { 0 })
                            .color(roles.fg),
                        ..button::Style::default()
                    }
                })
                .on_press(on(IdeaAction::Colour(slot))),
        );
    }
    let mut topics = row![word(
        "none",
        idea.topic.is_none(),
        on(IdeaAction::Topic(None))
    )]
    .spacing(4);
    for topic in &manifest.topics {
        let chosen = idea.topic == Some(topic.id);
        topics = topics.push(word(
            &topic.name,
            chosen,
            on(IdeaAction::Topic(Some(topic.id))),
        ));
    }
    let mut topic_field = row![
        text_input("New topic…", &panel.new_topic)
            .on_input(|name| on(IdeaAction::TopicName(name)))
            .on_submit(on(IdeaAction::AddTopic))
            .size(12)
            .padding([3, 8])
            .style(input_style),
        word("add", false, on(IdeaAction::AddTopic)),
    ]
    .spacing(4);
    if idea.topic.is_some() {
        topic_field = topic_field.push(word("rename", false, on(IdeaAction::RenameTopic)));
    }
    let cut = project
        .weights
        .ideas
        .iter()
        .any(|row| row.idea == idea.id && row.status == Status::Cut);
    let mut items = column![
        field(
            "Name",
            &panel.name,
            IDEA_NAME_ID,
            IdeaAction::Name,
            Message::CloseMenu
        ),
        note,
        label("Colour".to_string()),
        colours,
        label("Topic".to_string()),
        topics.wrap().vertical_spacing(4),
        topic_field,
        space().height(4),
        menu_item(
            "Make idea from selection".to_string(),
            "Ctrl+I",
            on(IdeaAction::FromSelection)
        ),
        menu_item(
            "Erase it from selection".to_string(),
            "",
            on(IdeaAction::Erase(Some(idea.id)))
        ),
        menu_item("Split selection off".to_string(), "", on(IdeaAction::Split)),
        menu_item(
            match cut {
                true => "Undo cut on purpose".to_string(),
                false => "Mark cut on purpose".to_string(),
            },
            "",
            on(IdeaAction::ToggleCut(idea.id)),
        ),
    ]
    .spacing(6)
    .width(300);
    let others: Vec<_> = manifest
        .live_ideas()
        .filter(|other| other.id != idea.id)
        .collect();
    if !others.is_empty() {
        items = items.push(label("Merge with…".to_string()));
        for other in others {
            let name = format!("  {}", other.name);
            items = items.push(menu_item(name, "", on(IdeaAction::MergeWith(other.id))));
        }
    }
    let delete = if panel.confirm_delete {
        "Click again to delete it everywhere"
    } else {
        "Delete idea"
    };
    items = items.push(
        button(text(delete).size(13).color(roles.accent_warm))
            .width(Fill)
            .padding([4, 8])
            .style(menu_button)
            .on_press(on(IdeaAction::Delete)),
    );
    raised_panel(
        container(scrollable(items))
            .padding(10)
            .style(raised_box)
            .into(),
    )
}

/// `panel` raised above the strip's left end; clicking anywhere else closes it.
fn raised_panel(panel: Element<'_, Message>) -> Element<'_, Message> {
    stack![
        mouse_area(space().width(Fill).height(Fill)).on_press(Message::CloseMenu),
        container(panel)
            .align_left(Fill)
            .align_bottom(Fill)
            .padding(Padding {
                top: 40.0,
                bottom: status_bar::HEIGHT + STRIP_HEIGHT + 4.0,
                left: 16.0,
                right: 0.0,
            }),
    ]
    .into()
}

fn label<'a>(words: String) -> Element<'a, Message> {
    text(words).size(11).style(subtle).into()
}

/// A text field that sends `on_input` as it is typed in and `on_submit` on Enter.
fn field<'a>(
    placeholder: &str,
    value: &str,
    id: &'static str,
    on_input: fn(String) -> IdeaAction,
    on_submit: Message,
) -> Element<'a, Message> {
    text_input(placeholder, value)
        .id(id)
        .on_input(move |typed| on(on_input(typed)))
        .on_submit(on_submit)
        .size(13)
        .padding([4, 8])
        .style(input_style)
        .into()
}

/// A small framed word, yellow when `chosen`.
fn word<'a>(words: &str, chosen: bool, message: Message) -> Element<'a, Message> {
    button(text(words.to_string()).size(12))
        .padding([2, 8])
        .style(move |theme: &Theme, status| {
            let roles = Roles::of(theme);
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: None,
                text_color: if chosen {
                    roles.accent_text
                } else {
                    roles.fg_muted
                },
                border: Border::default()
                    .rounded(4)
                    .width(1)
                    .color(match chosen || hovered {
                        true => roles.fg_subtle,
                        false => roles.border,
                    }),
                ..button::Style::default()
            }
        })
        .on_press(message)
        .into()
}
