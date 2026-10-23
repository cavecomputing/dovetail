//! The 24px status bar along the bottom, and the menu that opens from its glyph.

use std::f32::consts::TAU;
use std::path::Path;

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, mouse_area, row, space, text, tooltip};
use iced::{Background, Border, Color, Element, Fill, Theme};

use crate::core::opened::Doc;
use crate::core::paths::display_path;
use crate::core::save::SaveState;
use crate::ui::app::{App, Message};
use crate::ui::assistant_actions::AssistantAction;
use crate::ui::assistant_view::{LINE, SIZE, line, subtle};
use crate::ui::find_bar::{FindAction, LINE_KEYS};
use crate::ui::switcher::row_style;
use crate::ui::theme::{self, GLYPH_CLAUDE, GLYPH_MENU, GLYPH_PEN, Roles, icon_button, raised_box};

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
    // A save refused because the file changed on disk: never leave the window stuck. The buttons
    // sit right after the reason, short enough to fit a 640px window.
    if app.disk_conflict {
        let reload = word_button("Reload".to_string(), Some(Message::ReloadFromDisk));
        let words = keys("Replace this text with the file on disk (Ctrl+Z brings it back)");
        left = left.push(hint(reload, words));
        left = left.push(word_button(
            "Keep mine".to_string(),
            Some(Message::KeepMine),
        ));
    }
    // Warnings in orange, each in words: why the text is read-only, anything a menu action
    // couldn't do, where a data file (JSON, YAML, TOML, INI) stops being valid, and a settings
    // problem (short here, the whole reason on hover).
    let notice = app.notice.as_ref().map(|(words, _)| words);
    let warnings = [
        app.read_only_reason.as_ref(),
        notice,
        app.syntax_problem.as_ref(),
    ];
    for warning in warnings.into_iter().flatten() {
        left = left.push(small(warning.as_str()).color(roles.accent_warm));
    }
    if let Some(reason) = &app.settings_error {
        let label = small("Settings problem").color(roles.accent_warm);
        left = left.push(hint(label, reason.as_str()));
    }
    match app.pane.suggestions().len() {
        0 => {}
        1 => left = left.push(small("1 suggestion")),
        waiting => left = left.push(small(format!("{waiting} suggestions"))),
    }
    if app.original.is_some() {
        left = left.push(small(keys("Rewrite: Ctrl+S saves it over the original")));
    }
    // The path comes last, so in a narrow window a long one is what gets cut.
    left = left.push(small(doc_label(&app.doc)));

    let tokens = format!("~{} tokens", app.counts.tokens);
    let mut right = row![
        small(format!("{} words · ", app.counts.words)),
        hint(small(tokens), "about 4 bytes a token · UTF-8"),
    ]
    .align_y(iced::Center);
    if app.settings.show_line_col {
        let position = app.pane.content.cursor().position;
        // iced's column counts bytes; count the characters before the cursor instead.
        let column = app
            .pane
            .content
            .line(position.line)
            .map_or(0, |line| line.text[..position.column].chars().count());
        right = right.push(small(format!(
            "    Ln {}, Col {}",
            position.line + 1,
            column + 1
        )));
    }
    right = right.push(space().width(16));
    if app.editable() {
        right = right.push(rewrite_toggle(app.original.is_some()));
    }
    right = right.push(assistant_toggle(app.assistant.open));

    // The left end takes what room is left and is cut off, so the right end always fits.
    let left = container(left).width(Fill).clip(true);
    bar(row![left, right].spacing(16).align_y(iced::Center).into())
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

/// What the window shows: scratch, or the file's path.
fn doc_label(doc: &Doc) -> String {
    match doc {
        Doc::Scratch => "Scratch".to_string(),
        Doc::File { path, .. } => display_path(path),
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

/// Text at the status bar's 11px size, on one line.
pub fn small<'a>(words: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(words).size(TEXT_SIZE).wrapping(Wrapping::None)
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
pub fn glyph_button<'a>(glyph: &'a str, on_press: Option<Message>) -> Element<'a, Message> {
    button(text(glyph).size(14).center())
        .width(22)
        .height(20)
        .padding(0)
        .style(icon_button)
        .on_press_maybe(on_press)
        .into()
}

/// The assistant pane's speech-bubble toggle, highlighted while the pane is open.
fn assistant_toggle<'a>(open: bool) -> Element<'a, Message> {
    let toggle = glyph_toggle(
        GLYPH_CLAUDE,
        open,
        Message::Assistant(AssistantAction::Toggle),
    );
    let words = match open {
        true => "Close the assistant (Ctrl+L)",
        false => "Assistant: chat about this text (Ctrl+L)",
    };
    hint(toggle, keys(words))
}

/// The pen: starts a rewrite, highlighted while one is under way.
fn rewrite_toggle<'a>(rewriting: bool) -> Element<'a, Message> {
    let toggle = glyph_toggle(GLYPH_PEN, rewriting, Message::ToggleRewrite);
    let words = match rewriting {
        true => {
            "End the rewrite, dropping what Ctrl+S hasn't saved; Ctrl+Z brings it back (Ctrl+R)"
        }
        false => "Rewrite: edit a copy beside the original (Ctrl+R)",
    };
    hint(toggle, keys(words))
}

/// A small glyph button for something that is on or off, highlighted while `on`.
pub fn glyph_toggle<'a>(glyph: &'a str, on: bool, on_press: Message) -> Element<'a, Message> {
    button(text(glyph).size(14).center())
        .width(22)
        .height(20)
        .padding(0)
        .style(move |theme: &Theme, status| {
            let roles = Roles::of(theme);
            match on {
                true => button::Style {
                    background: Some(Background::Color(roles.border)),
                    text_color: roles.accent_text,
                    ..icon_button(theme, status)
                },
                false => icon_button(theme, status),
            }
        })
        .on_press(on_press)
        .into()
}

/// A small framed word button, such as "Keep mine".
pub fn word_button<'a>(label: String, on_press: Option<Message>) -> Element<'a, Message> {
    button(small(label))
        .padding([1, 8])
        .style(|theme: &Theme, status| {
            let roles = Roles::of(theme);
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(Background::Color(if hovered {
                    roles.bg_inset
                } else {
                    roles.bg_raised
                })),
                text_color: if status == button::Status::Disabled {
                    roles.fg_subtle
                } else {
                    roles.fg
                },
                border: Border::default().rounded(4).width(1).color(roles.border),
                ..button::Style::default()
            }
        })
        .on_press_maybe(on_press)
        .into()
}

/// Wraps `content` so `words` show above it on hover.
pub fn hint<'a>(
    content: impl Into<Element<'a, Message>>,
    words: impl text::IntoFragment<'a>,
) -> Element<'a, Message> {
    tooltip(
        content,
        container(small(words)).padding([3, 8]).style(raised_box),
        tooltip::Position::Top,
    )
    .into()
}

/// A shortcut as this OS writes it: "Ctrl+Shift+S" on Linux and Windows is "⇧ ⌘ S" on macOS.
/// Each modifier glyph is followed by a space: in a monospace font they fill their whole cell
/// (⌫ and ⌦ reach into the one before), so written tight they run into each other.
pub fn keys(shortcut: &str) -> String {
    if !cfg!(target_os = "macos") {
        return shortcut.to_string();
    }
    let glyphs = shortcut
        .replace("Ctrl+Shift+", "⇧⌘")
        .replace("Ctrl+", "⌘")
        .replace("Shift+", "⇧")
        .replace("Enter", "↩")
        .replace("Backspace", "⌫");
    let mut written = String::new();
    for glyph in glyphs.chars() {
        written.push(glyph);
        if "⇧⌘⌥⌃".contains(glyph) {
            written.push(' ');
        }
    }
    written
}

/// The menu panel, raised above the status bar's left end. Clicking anywhere else closes it.
pub fn menu(app: &App) -> Element<'_, Message> {
    let mut items = column![
        menu_item("new file", "Ctrl+N", Message::NewFile),
        menu_item("open", "Ctrl+O", Message::OpenDialog),
        menu_item("open recent", "Ctrl+P", Message::OpenSwitcher),
    ];
    // Scratch has no file, so Ctrl+S saves it to one; a file can move to another.
    items = match &app.doc {
        Doc::Scratch => items.push(menu_item("save to a file", "Ctrl+S", Message::SaveAs)),
        Doc::File { .. } => items.push(menu_item("save as", "Ctrl+Shift+S", Message::SaveAs)),
    };
    if app.editable() {
        let rewrite = match app.original {
            Some(_) => "end rewrite",
            None => "rewrite",
        };
        items = items.push(menu_item(rewrite, "Ctrl+R", Message::ToggleRewrite));
    }
    let line_numbers = match app.settings.line_numbers {
        true => "hide line numbers",
        false => "show line numbers",
    };
    items = items
        .push(menu_item("find", "Ctrl+F", Message::Find(FindAction::Open)))
        .push(menu_item(
            "replace",
            "Ctrl+Shift+F",
            Message::Find(FindAction::OpenReplace),
        ))
        .push(menu_item(
            "go to line",
            LINE_KEYS,
            Message::Find(FindAction::OpenLine),
        ))
        .push(menu_item(
            line_numbers,
            "Ctrl+Shift+L",
            Message::ToggleLineNumbers,
        ))
        .push(menu_item("settings", "Ctrl+,", Message::OpenSettings))
        .push(menu_item("scratch", "Ctrl+0", Message::OpenScratch));
    // The recent files come last, so the commands above stay put however many there are.
    let current = match &app.doc {
        Doc::File { path, .. } => Some(path),
        Doc::Scratch => None,
    };
    let recent: Vec<_> = app
        .recent
        .iter()
        .filter(|path| Some(*path) != current)
        .take(MENU_RECENT)
        .collect();
    let mut files = column![menu_heading(match recent.is_empty() {
        true => "# recent: nothing yet",
        false => "# recent",
    })];
    for path in recent {
        files = files.push(menu_item(
            file_name(path),
            "",
            Message::OpenPath(path.clone()),
        ));
    }

    // Drawn like the Ctrl+P switcher: plain rows in the assistant pane's type, `#` for a heading.
    let items = column![items, files].spacing(SIZE * LINE * 0.5).width(300);
    let panel = container(items).padding([8, 0]).style(raised_box);
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

/// The most recent files the menu lists; Ctrl+P has the rest.
const MENU_RECENT: usize = 6;

/// A menu row: its label, and its shortcut dimmed on the right.
fn menu_item<'a>(
    label: impl Into<String>,
    shortcut: &str,
    message: Message,
) -> Element<'a, Message> {
    let label = text(label.into())
        .size(SIZE)
        .line_height(LINE)
        .wrapping(Wrapping::None);
    button(row![
        container(label).width(Fill).clip(true),
        line(keys(shortcut), subtle).wrapping(Wrapping::None),
    ])
    .width(Fill)
    .padding([0, 12])
    .style(|theme, status| row_style(theme, status, false))
    .on_press(message)
    .into()
}

fn menu_heading<'a>(label: &'a str) -> Element<'a, Message> {
    container(line(label, subtle)).padding([0, 12]).into()
}

/// The file name for the window title.
pub fn title(doc: &Doc) -> String {
    match doc {
        Doc::Scratch => "Scratch".to_string(),
        Doc::File { path, .. } => file_name(path),
    }
}

/// `path`'s file name, for the title and the recent lists.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_are_written_as_this_os_writes_them() {
        let written = [keys("Ctrl+Shift+S"), keys("Previous (Shift+Enter)")];
        match cfg!(target_os = "macos") {
            true => assert_eq!(written, ["⇧ ⌘ S", "Previous (⇧ ↩)"]),
            false => assert_eq!(written, ["Ctrl+Shift+S", "Previous (Shift+Enter)"]),
        }
    }
}
