//! What a project window shows of its ideas: the idea strip above the status bar and the
//! feeling-note card beside the draft's caret. The panel that edits one idea is `idea_panel`.

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::Wrapping;
use iced::widget::{
    button, column, container, mouse_area, responsive, row, scrollable, space, text,
};
use iced::{Background, Border, Element, Fill, Padding, Theme, mouse};

use crate::project::weights::{IdeaWeight, Status};
use crate::ui::app::{App, Message};
use crate::ui::idea_actions::IdeaAction;
use crate::ui::project_view::ProjectView;
use crate::ui::status_bar;
use crate::ui::theme::{Roles, bold, fill, raised_box, subtle};

/// The strip's height, so the idea panel can open just above it.
pub const STRIP_HEIGHT: f32 = 48.0;
pub const STRIP_ID: &str = "idea-strip";
/// The feeling-note card's width, and about how tall it gets.
const CARD_WIDTH: f32 = 200.0;
const CARD_HEIGHT: f32 = 110.0;

pub fn on(message: IdeaAction) -> Message {
    Message::Idea(message)
}

/// The idea strip: an "IDEAS" label, a chip per idea (colour, name, old and new bars, `old% →
/// new%`), an Unpainted chip, and "+ New idea". Shown in every project, so the first idea has
/// somewhere to start. Click a chip to pick it as the brush, right-click it to edit it.
pub fn strip<'a>(app: &'a App, project: &'a ProjectView) -> Element<'a, Message> {
    let manifest = &project.project.manifest;
    let editable = app.editable();
    let mut chips = row![
        text("IDEAS")
            .size(11)
            .font(bold())
            .style(|theme: &Theme| text::Style {
                color: Some(Roles::of(theme).fg_muted)
            })
    ]
    .spacing(8)
    .align_y(iced::Center);
    for row_weight in &project.weights.ideas {
        let Some(idea) = manifest.idea(row_weight.idea) else {
            continue;
        };
        let active = app.pane.model.brush == Some(idea.id);
        let chip = chip(&idea.name, idea.colour, row_weight);
        let chip = button(chip)
            .padding([6, 10])
            .style(move |theme: &Theme, status| {
                chip_style(theme, status, row_weight.status, active)
            })
            .on_press_maybe(editable.then_some(on(IdeaAction::ToggleBrush(idea.id))));
        let chip = mouse_area(chip);
        chips = chips.push(match editable {
            true => chip.on_right_press(on(IdeaAction::OpenPanel(idea.id))),
            false => chip,
        });
    }
    let unpainted = IdeaWeight {
        idea: 0,
        weight: project.weights.unpainted,
        status: Status::Carried,
    };
    chips = chips.push(
        container(chip("Unpainted", usize::MAX, &unpainted))
            .padding([6, 10])
            .style(|theme: &Theme| {
                let roles = Roles::of(theme);
                container::Style {
                    background: Some(Background::Color(roles.bg_raised)),
                    text_color: Some(roles.fg),
                    border: Border::default().rounded(6).width(1).color(roles.border),
                    ..container::Style::default()
                }
            }),
    );
    let new_idea = button(text("+ New idea").size(12))
        .padding([6, 10])
        .style(ghost_button)
        .on_press_maybe(editable.then_some(on(IdeaAction::New)));
    // Chips that don't fit scroll sideways, with a thin bar to show there are more; the mouse
    // wheel scrolls them too. "+ New idea" stays in view after them.
    let bar = Scrollbar::new().width(2).scroller_width(2).spacing(3);
    let chips = scrollable(chips)
        .id(STRIP_ID)
        .direction(Direction::Horizontal(bar))
        .width(Fill);
    let chips = mouse_area(chips).on_scroll(|delta| on(IdeaAction::StripScroll(delta)));
    column![
        container(space().width(Fill).height(1))
            .style(|theme: &Theme| fill(Roles::of(theme).border)),
        container(row![chips, new_idea].spacing(8).align_y(iced::Center))
            .padding([0, 16])
            .height(STRIP_HEIGHT - 1.0)
            .center_y(STRIP_HEIGHT - 1.0),
    ]
    .into()
}

/// A chip's inside. `colour` is a colour slot, or `usize::MAX` for the Unpainted chip's border
/// colour.
fn chip<'a>(name: &str, colour: usize, row: &IdeaWeight) -> Element<'a, Message> {
    let weight = row.weight;
    let new_label = match row.status {
        Status::Missing => "missing".to_string(),
        Status::Cut => "cut on purpose".to_string(),
        _ => percent(weight.new_share),
    };
    let colour_of = move |theme: &Theme| {
        let roles = Roles::of(theme);
        match colour {
            usize::MAX => roles.border,
            slot => roles.idea(slot),
        }
    };
    // About 1.1px per percent, as in the mockup, and at most the bars' 44px.
    let bar = move |share: f64, alpha: f32| {
        let width = match share {
            0.0 => 0.0,
            share => (share * 1.1).clamp(2.0, 44.0) as f32,
        };
        container(space().width(width).height(3)).style(move |theme: &Theme| container::Style {
            background: Some(Background::Color(colour_of(theme).scale_alpha(alpha))),
            border: Border::default().rounded(2),
            ..container::Style::default()
        })
    };
    row![
        container(space().width(8).height(8)).style(move |theme: &Theme| container::Style {
            background: Some(Background::Color(colour_of(theme))),
            border: Border::default().rounded(4),
            ..container::Style::default()
        }),
        text(name.to_string()).size(12).wrapping(Wrapping::None),
        container(column![bar(weight.old_share, 0.35), bar(weight.new_share, 1.0)].spacing(2))
            .width(44),
        text(format!("{} →", percent(weight.old_share)))
            .size(12)
            .wrapping(Wrapping::None)
            .style(|theme: &Theme| text::Style {
                color: Some(Roles::of(theme).fg_muted)
            }),
        text(new_label).size(12).wrapping(Wrapping::None),
    ]
    .spacing(8)
    .align_y(iced::Center)
    .into()
}

/// A chip: raised, an orange-tinged border when missing, flat and muted when cut on purpose, and
/// a yellow double border when it is the brush.
fn chip_style(theme: &Theme, status: button::Status, idea: Status, active: bool) -> button::Style {
    let roles = Roles::of(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let (border, width) = match idea {
        _ if active => (roles.accent_text, 2.0),
        Status::Missing => (roles.accent_warm.scale_alpha(0.6), 1.0),
        _ if hovered => (roles.fg_subtle, 1.0),
        _ => (roles.border, 1.0),
    };
    let cut = idea == Status::Cut;
    button::Style {
        background: (!cut).then_some(Background::Color(roles.bg_raised)),
        text_color: if cut { roles.fg_muted } else { roles.fg },
        border: Border::default().rounded(6).width(width).color(border),
        ..button::Style::default()
    }
}

fn ghost_button(theme: &Theme, status: button::Status) -> button::Style {
    let roles = Roles::of(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(roles.bg_raised)),
        text_color: if hovered { roles.fg } else { roles.fg_muted },
        border: Border::default().rounded(6),
        ..button::Style::default()
    }
}

pub fn percent(share: f64) -> String {
    format!("{}%", share.round())
}

/// The active brush's feeling note, in a small card beside the draft's caret: to the right of the
/// text column when there is room, otherwise just under the caret's line (over it near the
/// bottom of the window). It also says what the newest version's note was, when the draft's
/// differs. Clicking it opens the note's editor.
pub fn note_card<'a>(app: &'a App, project: &'a ProjectView) -> Option<Element<'a, Message>> {
    let manifest = &project.project.manifest;
    let idea = manifest.idea(app.pane.model.brush?)?;
    let caret = project.caret?;
    let area = project.text_area;
    let roles = Roles::of(&app.theme);
    let note = manifest.note(idea.id, manifest.draft_version());
    let footer = match manifest.previous_note(idea.id) {
        Some((version, before)) => format!("feeling note · v{version} said \"{before}\""),
        None => "feeling note".to_string(),
    };
    let card = move || {
        let body = match note {
            "" => text("No feeling note yet. Click to write one.")
                .size(13)
                .style(subtle),
            note => text(note).size(13).color(roles.fg),
        };
        let card = column![
            row![
                status_bar::dot(roles.idea(idea.colour)),
                text(idea.name.to_uppercase())
                    .size(11)
                    .font(bold())
                    .color(roles.accent_text)
            ]
            .spacing(8)
            .align_y(iced::Center),
            body,
            text(footer.clone()).size(11).color(roles.fg_muted),
        ]
        .spacing(6);
        mouse_area(
            container(card)
                .width(CARD_WIDTH)
                .padding([10, 12])
                .style(raised_box),
        )
        .interaction(mouse::Interaction::Pointer)
        .on_press(on(IdeaAction::OpenNote(idea.id)))
    };
    Some(
        responsive(move |size| {
            let text_right = area.x + area.width;
            let (left, top) = if size.width - text_right >= CARD_WIDTH + 32.0 {
                (text_right + 16.0, caret.y - 10.0)
            } else {
                let left = (caret.x - 12.0).min(size.width - CARD_WIDTH - 16.0);
                (left.max(area.x - 12.0), caret.y + caret.height + 6.0)
            };
            let place = container(card()).width(Fill).height(Fill);
            let bottom_bars = status_bar::HEIGHT + STRIP_HEIGHT;
            if top + CARD_HEIGHT > size.height - bottom_bars {
                let bottom = size.height - caret.y + 6.0;
                place.align_bottom(Fill).padding(Padding {
                    left,
                    bottom,
                    ..Padding::ZERO
                })
            } else {
                place.padding(Padding {
                    left,
                    top,
                    ..Padding::ZERO
                })
            }
            .into()
        })
        .into(),
    )
}
