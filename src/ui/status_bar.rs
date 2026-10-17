//! The 24px status bar along the bottom, and the menu that opens from its glyph.

use std::f32::consts::TAU;
use std::path::Path;

use iced::widget::{button, column, container, mouse_area, row, space, text, tooltip};
use iced::{Background, Border, Color, Element, Fill, Theme};

use crate::core::opened::Doc;
use crate::core::paths::display_path;
use crate::core::save::SaveState;
use crate::ui::app::{App, Message};
use crate::ui::theme::{self, GLYPH_MENU, GLYPH_PEN, Roles, icon_button, raised_box};

pub const HEIGHT: f32 = 24.0;
const TEXT_SIZE: f32 = 11.0;

pub fn view(app: &App) -> Element<'_, Message> {
    let roles = Roles::of(&app.theme);
    let mut left = row![glyph_button(GLYPH_MENU, Some(Message::ToggleMenu))]
        .spacing(16)
        .align_y(iced::Center);
    if app.editable() {
        left = left.push(save_status(app, roles));
    }
    left = left.push(small(doc_label(&app.doc)));
    // Warnings in orange, each in words: why the text is read-only, anything a menu action
    // couldn't do, and a settings problem (short here, the whole reason on hover).
    for warning in [&app.read_only_reason, &app.notice].into_iter().flatten() {
        left = left.push(small(warning.as_str()).color(roles.accent_warm));
    }
    if let Some(reason) = &app.settings_error {
        let label = small("Settings problem").color(roles.accent_warm);
        left = left.push(hint(label, reason.as_str()));
    }

    let tokens = match app.tokens {
        Some(count) => format!("{count} tokens"),
        None => "… tokens".to_string(),
    };
    let mut right = row![
        small(format!("{} words · ", app.words)),
        hint(small(tokens), "o200k tokenizer · UTF-8"),
    ]
    .align_y(iced::Center);
    if app.settings.show_line_col {
        let position = app.content.cursor().position;
        // iced's column counts bytes; count the characters before the cursor instead.
        let column = app
            .content
            .line(position.line)
            .map_or(0, |line| line.text[..position.column].chars().count());
        right = right.push(small(format!(
            "    Ln {}, Col {}",
            position.line + 1,
            column + 1
        )));
    }
    // Rewrite arrives in phase 2; until then the pen is shown but does nothing.
    right = right.push(space().width(16));
    right = right.push(hint(glyph_button(GLYPH_PEN, None), "Start a rewrite"));

    bar(row![left, space::horizontal(), right]
        .align_y(iced::Center)
        .into())
}

/// The status bar's frame: a 1px top border, then the bar on `bg-page` in small mono text.
pub fn bar(content: Element<'_, Message>) -> Element<'_, Message> {
    column![
        container(space().width(Fill).height(1))
            .style(|theme: &Theme| theme::fill(Roles::of(theme).border)),
        container(content)
            .height(HEIGHT - 1.0)
            .width(Fill)
            .center_y(HEIGHT - 1.0)
            .padding([0, 6])
            .style(|theme: &Theme| container::Style {
                text_color: Some(Roles::of(theme).fg_muted),
                ..theme::fill(Roles::of(theme).bg_page)
            }),
    ]
    .into()
}

/// What the file is: its path, or a reminder that scratch lives in no file.
fn doc_label(doc: &Doc) -> String {
    match doc {
        Doc::Scratch => "Scratch · not saved to a file".to_string(),
        Doc::File { path, .. } | Doc::Project(path) => display_path(path),
    }
}

/// The save dot: aqua when saved (the word on hover), pulsing yellow while saving, orange with
/// the reason when a save failed.
fn save_status<'a>(app: &App, roles: &Roles) -> Element<'a, Message> {
    match app.autosaver.state() {
        SaveState::Saved => hint(dot(roles.accent_aqua), "Saved"),
        SaveState::Saving => {
            let seconds = match app.saving_since {
                Some(start) => app.now.saturating_duration_since(start).as_secs_f32(),
                None => 0.0,
            };
            let alpha = 0.35 + 0.65 * (0.5 + 0.5 * (seconds * TAU).cos());
            hint(dot(roles.accent_text.scale_alpha(alpha)), "Saving")
        }
        SaveState::Failed(reason) => row![
            dot(roles.accent_warm),
            small(reason).color(roles.accent_warm)
        ]
        .spacing(6)
        .align_y(iced::Center)
        .into(),
    }
}

/// Text at the status bar's 11px size.
pub fn small<'a>(words: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(words).size(TEXT_SIZE)
}

pub fn dot<'a>(color: Color) -> Element<'a, Message> {
    container(space().width(7).height(7))
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            border: Border::default().rounded(4),
            ..container::Style::default()
        })
        .into()
}

/// A small glyph button; `None` draws it without a click action.
fn glyph_button<'a>(glyph: &'a str, on_press: Option<Message>) -> Element<'a, Message> {
    button(text(glyph).size(14).center())
        .width(22)
        .height(20)
        .padding(0)
        .style(icon_button)
        .on_press_maybe(on_press)
        .into()
}

/// Wraps `content` so `words` show above it on hover.
pub fn hint<'a>(content: impl Into<Element<'a, Message>>, words: &'a str) -> Element<'a, Message> {
    tooltip(
        content,
        container(small(words)).padding([3, 8]).style(raised_box),
        tooltip::Position::Top,
    )
    .into()
}

/// The menu panel, raised above the status bar's left end. Clicking anywhere else closes it.
pub fn menu(app: &App) -> Element<'_, Message> {
    let mut items = column![
        menu_item("Open…".to_string(), "", Message::OpenDialog),
        menu_heading("Recent"),
    ];
    if app.recent.is_empty() {
        items = items.push(menu_heading("  nothing yet"));
    }
    let current = match &app.doc {
        Doc::File { path, .. } | Doc::Project(path) => Some(path),
        Doc::Scratch => None,
    };
    for path in app.recent.iter().filter(|path| Some(*path) != current) {
        items = items.push(menu_item(
            file_name(path),
            "",
            Message::OpenPath(path.clone()),
        ));
    }
    items = items
        .push(menu_item(
            "Settings".to_string(),
            "Ctrl+,",
            Message::OpenSettings,
        ))
        .push(menu_item(
            "Scratch".to_string(),
            "Ctrl+0",
            Message::OpenScratch,
        ));

    let panel = container(items.width(260)).padding(4).style(raised_box);
    iced::widget::stack![
        mouse_area(space().width(Fill).height(Fill)).on_press(Message::CloseMenu),
        container(panel)
            .align_left(Fill)
            .align_bottom(Fill)
            .padding(iced::Padding {
                bottom: HEIGHT + 4.0,
                left: 6.0,
                ..iced::Padding::ZERO
            }),
    ]
    .into()
}

fn menu_item<'a>(label: String, shortcut: &'a str, message: Message) -> Element<'a, Message> {
    button(
        row![
            text(label).size(13),
            space::horizontal(),
            text(shortcut).size(11).style(|theme: &Theme| text::Style {
                color: Some(Roles::of(theme).fg_muted),
            })
        ]
        .align_y(iced::Center),
    )
    .width(Fill)
    .padding([4, 8])
    .style(|theme: &Theme, status| {
        let roles = Roles::of(theme);
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: hovered.then_some(Background::Color(roles.bg_inset)),
            text_color: roles.fg,
            border: Border::default().rounded(4),
            ..button::Style::default()
        }
    })
    .on_press(message)
    .into()
}

fn menu_heading<'a>(label: &'a str) -> Element<'a, Message> {
    container(text(label).size(11).style(|theme: &Theme| text::Style {
        color: Some(Roles::of(theme).fg_subtle),
    }))
    .padding([4, 8])
    .into()
}

/// The file name for the window title.
pub fn title(doc: &Doc) -> String {
    match doc {
        Doc::Scratch => "Scratch".to_string(),
        Doc::File { path, .. } | Doc::Project(path) => file_name(path),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .to_string()
}
