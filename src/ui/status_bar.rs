//! The 29px status bar along the bottom, and the menu that opens from its glyph.

use std::f32::consts::TAU;
use std::path::Path;

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, mouse_area, row, space, text, tooltip};
use iced::{Background, Border, Color, Element, Fill, Theme};

use crate::core::counts::Counts;
use crate::core::opened::Doc;
use crate::core::paths::display_path;
use crate::core::save::SaveState;
use crate::ui::app::{App, Message};
use crate::ui::assistant_actions::AssistantAction;
use crate::ui::assistant_view::{LINE, SIZE, line, subtle};
use crate::ui::find_bar::{FindAction, LINE_KEYS};
use crate::ui::switcher::row_style;
use crate::ui::theme::{
    self, CHAR_WIDTH_RATIO, GLYPH_CLAUDE, GLYPH_MENU, GLYPH_PEN, Roles, icon_button, raised_box,
};

pub const HEIGHT: f32 = 29.0 + HIDDEN_BELOW;
const TEXT_SIZE: f32 = 11.0;
/// Room at the bar's left and right ends. macOS rounds the window's corners by far more than the
/// other OSes, and 4px let the curve cut into the corner buttons. On Windows the window's own
/// border covers 1px of each side, so it gets 1px more; Linux has square, borderless corners.
const SIDE_PADDING: f32 = if cfg!(target_os = "macos") {
    12.0
} else if cfg!(windows) {
    5.0
} else {
    4.0
};
/// Room under the buttons that the window's border hides: on Windows the bottom 2px are covered,
/// so the buttons looked 2px from the edge where the sides had 4px. Nothing is hidden elsewhere.
const HIDDEN_BELOW: f32 = if cfg!(windows) { 2.0 } else { 0.0 };

pub fn view(app: &App, width: f32) -> Element<'_, Message> {
    let roles = Roles::of(&app.theme);
    let mut left = row![glyph_button(GLYPH_MENU, Some(Message::ToggleMenu))]
        .spacing(16)
        .align_y(iced::Center);
    // Room already taken in pixels, to fit the path in what's left: the menu button and its gap,
    // the dot and its gap, the 16px gaps between and around the two halves, the right-hand
    // buttons, and a few characters of slack.
    let letter = TEXT_SIZE * CHAR_WIDTH_RATIO;
    let mut used = 2.0 * SIDE_PADDING + 22.0 + 16.0 + 16.0 + 16.0 + 2.0 * 22.0 + 2.0 * letter;
    if app.can_rewrite() {
        used += 22.0;
    }
    if app.editable() {
        left = left.push(save_status(app, roles));
        used += 7.0 + 16.0;
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
        used += 2.0 * 16.0 + 20.0 * letter;
    }
    // Warnings in orange, each in words: why the text is read-only, anything a menu action
    // couldn't do, where a data file (JSON, YAML, TOML, INI) stops being valid, a rewrite's file
    // changed elsewhere, and a settings problem (short here, the whole reason on hover).
    let notice = app.notice.as_ref().map(|(words, _)| words.as_str());
    let warnings = [
        app.read_only_reason.as_deref(),
        notice,
        app.syntax_problem.as_deref(),
        app.stale_draft,
    ];
    for warning in warnings.into_iter().flatten() {
        left = left.push(small(warning).color(roles.accent_warm));
        used += 16.0 + warning.chars().count() as f32 * letter;
    }
    if let Some(reason) = &app.settings_error {
        let label = small("Settings problem").color(roles.accent_warm);
        left = left.push(hint(label, reason.as_str()));
        used += 16.0 + 16.0 * letter;
    }
    match app.pane.suggestions().len() {
        0 => {}
        1 => left = left.push(small("1 suggestion")),
        waiting => left = left.push(small(format!("{waiting} suggestions"))),
    }
    if !app.pane.suggestions().is_empty() {
        used += 16.0 + 14.0 * letter;
    }

    // While rewriting, the original's counts come first: "108 → 120 words".
    let (words, tokens) = match (selected_counts(app), &app.original, &app.original_counts) {
        (Some(chosen), _, _) => (
            format!("{} of {} words · ", chosen.words, app.counts.words),
            format!("~{} of ~{} tokens", chosen.tokens, app.counts.tokens),
        ),
        (_, Some(_), Some(original)) => (
            format!("{} → {} words · ", original.words, app.counts.words),
            format!("~{} → ~{} tokens", original.tokens, app.counts.tokens),
        ),
        _ => (
            format!("{} words · ", app.counts.words),
            format!("~{} tokens", app.counts.tokens),
        ),
    };
    used += (words.chars().count() + tokens.chars().count()) as f32 * letter;
    if app.settings.show_line_col {
        used += 14.0 * letter;
    }
    // The path comes last. A long one loses the front of its folders, never the file's name; the
    // clip below is only the fallback for what the guess above gets wrong.
    let room = ((width - used) / letter).max(10.0) as usize;
    left = left.push(small(shorten_path(&doc_label(&app.doc), room)));
    let mut right = row![
        small(words),
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
    if app.can_rewrite() {
        right = right.push(rewrite_toggle(app.original.is_some(), app.paused_draft));
    }
    right = right.push(assistant_toggle(app.assistant.open));

    // The left end takes what room is left and is cut off, so the right end always fits.
    let left = container(left).width(Fill).clip(true);
    bar(row![left, right].spacing(16).align_y(iced::Center).into())
}

/// The status bar's frame: a 1px top border, then the bar on `bg-page` in small mono text.
/// The 20px button chips sit 4px from the window edge, more at the ends on macOS.
pub fn bar(content: Element<'_, Message>) -> Element<'_, Message> {
    column![
        container(space().width(Fill).height(1))
            .style(|theme: &Theme| theme::fill(Roles::of(theme).border)),
        container(content)
            .height(HEIGHT - 1.0)
            .width(Fill)
            .center_y(HEIGHT - 1.0)
            .padding(iced::Padding {
                top: 0.0,
                right: SIDE_PADDING,
                bottom: HIDDEN_BELOW,
                left: SIDE_PADDING,
            })
            .style(|theme: &Theme| container::Style {
                text_color: Some(Roles::of(theme).fg_muted),
                ..theme::fill(Roles::of(theme).bg_page)
            }),
    ]
    .into()
}

/// The longest selection counted each frame; a bigger one leaves the whole text's counts showing.
const SELECTION_COUNT_LIMIT: usize = 2_000_000;

/// The counts of the selected text, shown as "12 of 44 words". Not while rewriting, where the bar
/// already shows two sets of counts.
pub fn selected_counts(app: &App) -> Option<Counts> {
    let range = app
        .pane
        .selection()
        .filter(|range| range.len() <= SELECTION_COUNT_LIMIT)?;
    app.original
        .is_none()
        .then(|| Counts::of(&app.pane.model.text()[range]))
}

/// `label` cut from the front to fit `room` characters, keeping its end: `…/folder/name.txt`.
/// It starts at a folder boundary when it can, so no name is cut in half.
fn shorten_path(label: &str, room: usize) -> String {
    let count = label.chars().count();
    if count <= room {
        return label.to_string();
    }
    let tail: String = label.chars().skip(count - (room - 1)).collect();
    match tail.find(['/', '\\']) {
        Some(at) if at + 1 < tail.len() => format!("…{}", &tail[at..]),
        _ => format!("…{tail}"),
    }
}

/// What the window shows: scratch, or the file's path.
fn doc_label(doc: &Doc) -> String {
    match doc {
        Doc::Scratch => "Scratch".to_string(),
        Doc::File { path, .. } => display_path(path),
    }
}

/// The save dot: aqua when saved (the word on hover), pulsing yellow while saving, orange with
/// the reason when a save failed. While rewriting it is about the file: steady yellow until
/// Ctrl+S writes the draft there.
fn save_status<'a>(app: &App, roles: &Roles) -> Element<'a, Message> {
    let state = app.autosaver.state();
    if !matches!(state, SaveState::Failed(_)) && app.rewrite_unsaved() {
        let words = keys("Not in the file yet: Ctrl+S saves it there");
        return hint(dot(roles.accent_text), words);
    }
    match state {
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

/// The pen: starts a rewrite, highlighted while one is under way and faintly while a paused one
/// waits for Ctrl+R.
fn rewrite_toggle<'a>(rewriting: bool, paused: bool) -> Element<'a, Message> {
    let (strength, words) = match (rewriting, paused) {
        (true, _) => (Some(1.0), "pause rewrite (Ctrl+R)"),
        (false, true) => (Some(PAUSED), "resume rewrite (Ctrl+R)"),
        (false, false) => (None, "Rewrite: edit a copy beside the original (Ctrl+R)"),
    };
    hint(
        lit_glyph(GLYPH_PEN, strength, Message::ToggleRewrite),
        keys(words),
    )
}

/// How strongly the pen is lit while a paused rewrite waits.
const PAUSED: f32 = 0.5;

/// A small glyph button for something that is on or off, highlighted while `on`.
pub fn glyph_toggle<'a>(glyph: &'a str, on: bool, on_press: Message) -> Element<'a, Message> {
    lit_glyph(glyph, on.then_some(1.0), on_press)
}

/// A small glyph button, highlighted at `strength` (0 to 1), or plain with none.
fn lit_glyph<'a>(glyph: &'a str, strength: Option<f32>, on_press: Message) -> Element<'a, Message> {
    button(text(glyph).size(14).center())
        .width(22)
        .height(20)
        .padding(0)
        .style(move |theme: &Theme, status| {
            let roles = Roles::of(theme);
            match strength {
                Some(strength) => button::Style {
                    background: Some(Background::Color(roles.border.scale_alpha(strength))),
                    text_color: roles.accent_text.scale_alpha(strength.max(0.7)),
                    ..icon_button(theme, status)
                },
                None => icon_button(theme, status),
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
    if app.can_rewrite() {
        let rewrite = match (&app.original, app.paused_draft) {
            (Some(_), _) => "pause rewrite",
            (None, true) => "resume rewrite",
            (None, false) => "rewrite",
        };
        items = items.push(menu_item(rewrite, "Ctrl+R", Message::ToggleRewrite));
        if app.original.is_some() {
            let reset = Message::ResetRewrite;
            items = items.push(menu_item("reset rewrite", "Ctrl+Shift+R", reset));
        }
    }
    let line_numbers = match app.settings.line_numbers {
        true => "hide line numbers",
        false => "show line numbers",
    };
    let word_wrap = match app.settings.word_wrap {
        true => "turn word wrap off",
        false => "turn word wrap on",
    };
    items = items
        .push(menu_item("undo", "Ctrl+Z", Message::Undo))
        .push(menu_item("redo", "Ctrl+Shift+Z", Message::Redo))
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
        .push(menu_item(
            word_wrap,
            "Ctrl+Shift+E",
            Message::ToggleWordWrap,
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

    #[test]
    fn a_long_path_loses_its_folders_before_its_file_name() {
        let path = "~/projects/client/2026/reports/final-final/the-long-file-name.txt";
        assert_eq!(shorten_path(path, 100), path);
        assert_eq!(
            shorten_path(path, 46),
            "…/reports/final-final/the-long-file-name.txt"
        );
        // A name too long on its own is cut from the front too, with the end kept.
        assert_eq!(shorten_path(path, 12), "…le-name.txt");
        assert_eq!(shorten_path("Scratch", 10), "Scratch");
    }
}
