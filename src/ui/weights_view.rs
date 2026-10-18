//! The idea weights view (Ctrl+Shift+W): each idea's share of the old version and the draft,
//! under its topic, then Unpainted, with missing ideas flagged.

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, scrollable, space, stack, text, tooltip};
use iced::{Background, Border, Color, Element, Fill, Theme};

use crate::core::counts::Counts;
use crate::project::weights::{IdeaWeight, Status, Weight};
use crate::ui::app::{App, Message};
use crate::ui::idea_actions::IdeaAction;
use crate::ui::ideas_view::{on, percent};
use crate::ui::project_view::{ProjectView, modal};
use crate::ui::status_bar;
use crate::ui::theme::{Roles, dialog_box, fill, raised_box, subtle};

/// What the weights view counts in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Measure {
    Percent,
    Tokens,
    Words,
}

/// The idea weights view: each idea's share of the old version and of the draft, under its
/// topic, then Unpainted, with missing ideas flagged and a way to mark them cut on purpose.
pub fn view<'a>(app: &'a App, project: &'a ProjectView) -> Element<'a, Message> {
    let roles = Roles::of(&app.theme);
    let manifest = &project.project.manifest;
    let weights = &project.weights;
    let measure = project.measure;
    let version = manifest.draft_version();
    // Bars fill at 50%, or at the biggest share if that is more.
    let scale = weights
        .ideas
        .iter()
        .flat_map(|row| [row.weight.old_share, row.weight.new_share])
        .fold(50.0, f64::max);

    let toggle = |words: &'a str, choice: Measure| {
        button(text(words).size(12))
            .padding(0)
            .style(move |theme: &Theme, _| button::Style {
                text_color: match measure == choice {
                    true => Roles::of(theme).accent_text,
                    false => Roles::of(theme).fg_subtle,
                },
                ..button::Style::default()
            })
            .on_press(on(IdeaAction::Measure(choice)))
    };
    let header = row![
        text("Idea weights").size(13),
        text(format!("v{} → draft", project.reference_version))
            .size(12)
            .style(subtle),
        space::horizontal(),
        toggle("%", Measure::Percent),
        text("·").size(12).style(subtle),
        toggle("tokens", Measure::Tokens),
        text("·").size(12).style(subtle),
        toggle("words", Measure::Words),
    ]
    .spacing(10)
    .align_y(iced::Center);

    let amount = |counts: Counts, share: f64| match measure {
        Measure::Percent => percent(share),
        Measure::Tokens => counts
            .tokens
            .map_or("…".to_string(), |tokens| tokens.to_string()),
        Measure::Words => counts.words.to_string(),
    };
    let value = |weight: Weight| {
        let old = amount(weight.old, weight.old_share);
        format!("{old} → {}", amount(weight.new, weight.new_share))
    };
    let idea_row = |row: &IdeaWeight| -> Element<'a, Message> {
        let Some(idea) = manifest.idea(row.idea) else {
            return space().into();
        };
        let colour = roles.idea(idea.colour);
        let (label, color, action) = match row.status {
            Status::Missing => (
                format!("{} → missing", amount(row.weight.old, row.weight.old_share)),
                roles.accent_warm,
                Some(("mark cut", IdeaAction::ToggleCut(idea.id))),
            ),
            Status::Cut => (
                "cut on purpose".to_string(),
                roles.fg_subtle,
                Some(("undo", IdeaAction::ToggleCut(idea.id))),
            ),
            _ => (value(row.weight), roles.fg_muted, None),
        };
        let note = manifest.note(idea.id, version).to_string();
        // Hovering the name shows every version's feeling note, newest first.
        let history = idea.notes.iter().rev().map(|old| {
            let when = if old.version == version {
                "draft".to_string()
            } else {
                format!("v{}", old.version)
            };
            let words = if old.text.is_empty() {
                "(none)"
            } else {
                &old.text
            };
            format!("{when:>5}  {words}")
        });
        weight_row(
            Some(colour),
            text(idea.name.clone()).size(13).color(roles.fg),
            note,
            history.collect(),
            Some((row.weight, colour, scale)),
            text(label).size(12).color(color),
            action,
        )
    };

    let mut rows = column![];
    for topic in &weights.topics {
        let name = manifest
            .topics
            .iter()
            .find(|t| t.id == topic.topic)
            .map_or(String::new(), |t| t.name.clone());
        if topic.topic != weights.topics[0].topic {
            rows = rows.push(space().height(14));
        }
        rows = rows.push(weight_row(
            None,
            text(name).size(12).style(subtle),
            String::new(),
            Vec::new(),
            None,
            text(value(topic.weight)).size(12).style(subtle),
            Some(("delete", IdeaAction::DeleteTopic(topic.topic))),
        ));
        for row in &weights.ideas {
            if manifest
                .idea(row.idea)
                .is_some_and(|idea| idea.topic == Some(topic.topic))
            {
                rows = rows.push(idea_row(row));
            }
        }
    }
    if !weights.topics.is_empty() {
        rows = rows.push(space().height(14));
    }
    let known = |topic| weights.topics.iter().any(|t| Some(t.topic) == topic);
    for row in &weights.ideas {
        if manifest
            .idea(row.idea)
            .is_some_and(|idea| !known(idea.topic))
        {
            rows = rows.push(idea_row(row));
        }
    }
    rows = rows.push(weight_row(
        Some(roles.fg_subtle),
        text("Unpainted").size(13).color(roles.fg),
        String::new(),
        Vec::new(),
        Some((weights.unpainted, roles.fg_subtle, scale)),
        text(value(weights.unpainted))
            .size(12)
            .color(roles.fg_muted),
        None,
    ));

    let missing: Vec<&str> = weights
        .ideas
        .iter()
        .filter(|row| row.status == Status::Missing)
        .filter_map(|row| manifest.idea(row.idea).map(|idea| idea.name.as_str()))
        .collect();
    let mut footer = row![].spacing(8).align_y(iced::Center);
    if !missing.is_empty() {
        let verb = if missing.len() == 1 { "has" } else { "have" };
        footer = footer.push(status_bar::dot(roles.accent_warm)).push(row![
            text(missing.join(", ")).size(12).color(roles.accent_warm),
            text(format!(" {verb} no text in the draft"))
                .size(12)
                .color(roles.fg_muted),
        ]);
    }
    footer = footer.push(space::horizontal()).push(
        text(format!(
            "faint = v{} · overlaps count twice",
            project.reference_version
        ))
        .size(12)
        .style(subtle),
    );

    let card = column![
        header,
        container(scrollable(rows)).height(Fill),
        column![
            container(space().width(Fill).height(1)).style(move |_: &Theme| fill(roles.border)),
            footer
        ]
        .spacing(14),
    ]
    .spacing(20);
    let card = container(card)
        .width(880)
        .height(640)
        .padding([28, 32])
        .style(dialog_box);
    modal(card.into())
}

/// One row of the weights view: a dot, the name and its note (with `history`, the note of each
/// version, on hover), the old (faint) and new bars, the value, and an action such as "mark cut".
fn weight_row<'a>(
    dot: Option<Color>,
    name: text::Text<'a>,
    note: String,
    history: Vec<String>,
    bar: Option<(Weight, Color, f64)>,
    value: text::Text<'a>,
    action: Option<(&'a str, IdeaAction)>,
) -> Element<'a, Message> {
    let mut left = row![].spacing(8).align_y(iced::Center);
    if let Some(color) = dot {
        left = left.push(status_bar::dot(color));
    }
    left = left.push(name.wrapping(Wrapping::None));
    left = left.push(text(note).size(12).style(subtle).wrapping(Wrapping::None));
    let bars: Element<'a, Message> = match bar {
        Some((weight, color, scale)) => {
            let piece = |share: f64, alpha: f32| {
                container(space().width((180.0 * share / scale) as f32).height(3)).style(
                    move |_: &Theme| container::Style {
                        background: Some(Background::Color(color.scale_alpha(alpha))),
                        border: Border::default().rounded(2),
                        ..container::Style::default()
                    },
                )
            };
            container(stack![
                piece(weight.old_share, 0.3),
                piece(weight.new_share, 1.0)
            ])
            .width(180)
            .style(|theme: &Theme| container::Style {
                background: Some(Background::Color(Roles::of(theme).bg_raised)),
                border: Border::default().rounded(2),
                ..container::Style::default()
            })
            .into()
        }
        None => space().width(180).into(),
    };
    let action: Element<'a, Message> = match action {
        Some((words, message)) => button(text(words).size(12))
            .padding(0)
            .style(|theme: &Theme, status| button::Style {
                text_color: match status {
                    button::Status::Hovered | button::Status::Pressed => {
                        Roles::of(theme).accent_text
                    }
                    _ => Roles::of(theme).fg_subtle,
                },
                ..button::Style::default()
            })
            .on_press(on(message))
            .into(),
        None => space().into(),
    };
    let left = container(left).width(Fill).clip(true);
    let left: Element<'a, Message> = if history.is_empty() {
        left.into()
    } else {
        let mut notes = column![text("Feeling notes").size(11).style(subtle)].spacing(4);
        for line in history {
            notes = notes.push(text(line).size(12));
        }
        let notes = container(notes).padding([6, 10]).style(raised_box);
        tooltip(left, notes, tooltip::Position::Bottom).into()
    };
    row![
        left,
        bars,
        container(value.wrapping(Wrapping::None))
            .width(130)
            .align_right(130),
        container(action).width(64).align_right(64),
    ]
    .spacing(20)
    .height(28)
    .align_y(iced::Center)
    .into()
}
